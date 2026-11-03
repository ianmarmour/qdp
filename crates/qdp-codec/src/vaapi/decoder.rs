//! The VA-API decoder: frames parsed by [`FrameParser`] are handed to the
//! GPU's AV1 decoder with their parameters, each into one of a pool of NV12
//! surfaces that the reference slots and the pictures returned share.

use std::ffi::c_int;
use std::sync::{Arc, OnceLock};

use tracing::info;

use super::av1_parameters::{PictureParameters, tile_groups};
use super::ffi::{self, ConfigAttrib, SurfaceAttrib, VaId};
use super::picture::{DecoderDisplay, SurfacePool, VaapiPicture, unavailable};
use super::{Display, PictureBuffers, render_nodes};
use crate::frame_parser::{FrameParser, NUM_REF_FRAMES, ParsedFrame};
use crate::sequence::SequenceHeader;
use crate::{CodecError, DecodedPicture, VideoDecoder, Yuv420Frame};

/// The surfaces of a session: one for each reference slot, one for the
/// frame being decoded, and the rest for pictures a window still shows.
const SURFACES: usize = 16;

/// A hardware AV1 decoder using VA-API, on GPUs that decode AV1 (AMD RX 6000
/// and later, Intel Xe and later).
///
/// Only 8-bit 4:2:0 streams without film grain are decoded in hardware;
/// others report [`CodecError::HardwareUnavailable`], which hands them to
/// software.
pub struct VaapiDecoder {
    /// The display, shared with the pictures.
    device: Arc<DecoderDisplay>,
    /// The frame headers read so far, and the reference slots' state.
    parser: FrameParser,
    /// The context and surfaces for the current sequence header, once one
    /// arrived.
    session: Option<Session>,
    /// Which GPU and driver decode, for logs.
    description: String,
}

impl std::fmt::Debug for VaapiDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaapiDecoder")
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// The context and surfaces for one sequence's frames.
struct Session {
    /// The sequence header they were made for.
    sequence: SequenceHeader,
    /// The decoder's configuration.
    config: VaId,
    /// The decoding context.
    context: VaId,
    /// The surfaces frames are decoded into.
    pool: Arc<SurfacePool>,
    /// A hold on each surface, which pictures share: a surface is free when
    /// no picture holds it and no reference slot names it.
    holds: Vec<Arc<()>>,
    /// The surface each reference slot holds the frame of.
    slots: [Option<usize>; NUM_REF_FRAMES],
}

impl VaapiDecoder {
    /// Returns a decoder if this computer's GPU decodes AV1, once a short
    /// stream decoded with it has matched rav1d's decoding exactly.
    pub fn new() -> Result<Self, CodecError> {
        let decoder = Self::open()?;
        /// The outcome of the check, made once per process.
        static VERIFIED: OnceLock<Result<(), String>> = OnceLock::new();
        VERIFIED
            .get_or_init(|| crate::hardware_check::verify(Self::open))
            .clone()
            .map_err(CodecError::HardwareUnavailable)?;
        Ok(decoder)
    }

    /// Opens the first GPU whose driver decodes AV1, without checking its
    /// pictures.
    fn open() -> Result<Self, CodecError> {
        let va = ffi::libva().map_err(CodecError::HardwareUnavailable)?;
        let mut reasons = Vec::new();
        for node in render_nodes().map_err(unavailable)? {
            match Display::open(va, &node) {
                Ok(display)
                    if display.has_entrypoint(ffi::PROFILE_AV1_MAIN, ffi::ENTRYPOINT_DECODE) =>
                {
                    let description = format!(
                        "VA-API on {} ({})",
                        display.path.display(),
                        display.vendor()
                    );
                    return Ok(Self {
                        device: Arc::new(DecoderDisplay::new(display)),
                        parser: FrameParser::new(),
                        session: None,
                        description,
                    });
                }
                Ok(display) => reasons.push(format!(
                    "{} ({}) cannot decode AV1",
                    display.path.display(),
                    display.vendor()
                )),
                Err(error) => reasons.push(error.to_string()),
            }
        }
        if reasons.is_empty() {
            reasons.push("this computer has no GPU render nodes".to_owned());
        }
        Err(CodecError::HardwareUnavailable(reasons.join("; ")))
    }

    /// Makes sure the session suits the stream's sequence header, making a
    /// new one when the stream's size or format changed.
    fn prepare(&mut self) -> Result<(), CodecError> {
        let sequence = self
            .parser
            .sequence()
            .ok_or(CodecError::MalformedBitstream(
                "a frame before the sequence header",
            ))?;
        if self
            .session
            .as_ref()
            .is_some_and(|session| same_format(&session.sequence, sequence))
        {
            return Ok(());
        }
        if sequence.profile != 0
            || sequence.bit_depth != 8
            || sequence.monochrome
            || !(sequence.subsampling_x && sequence.subsampling_y)
        {
            return Err(CodecError::HardwareUnavailable(format!(
                "only 8-bit 4:2:0 AV1 is decoded in hardware, not profile {} at {} bits",
                sequence.profile, sequence.bit_depth
            )));
        }
        if sequence.film_grain {
            return Err(CodecError::HardwareUnavailable(
                "streams with film grain are decoded in software".to_owned(),
            ));
        }
        // The old session goes first, so that the GPU never holds both.
        self.session = None;
        let session = Session::new(&self.device, sequence.clone()).map_err(unavailable)?;
        info!(
            width = sequence.max_width,
            height = sequence.max_height,
            decoder = self.description,
            "decoding AV1 with VA-API"
        );
        self.session = Some(session);
        Ok(())
    }

    /// Decodes a temporal unit, returning the picture it shows.
    fn decode_unit(&mut self, temporal_unit: &[u8]) -> Result<VaapiPicture, CodecError> {
        let frames = self.parser.parse(temporal_unit)?;
        self.prepare()?;
        let display = &self.device.display;
        let session = self.session.as_mut().ok_or(CodecError::NoPicture)?;
        let mut shown = None;
        for frame in &frames {
            let header = &frame.header;
            if header.show_existing_frame {
                let surface = session.slots[usize::from(header.frame_to_show_map_idx)]
                    .ok_or(CodecError::MalformedBitstream("an empty slot shown"))?;
                session.refresh(header.refresh_frame_flags, surface);
                shown = Some((surface, header.size));
                continue;
            }
            if header.film_grain.apply_grain {
                return Err(CodecError::HardwareUnavailable(
                    "frames with film grain are decoded in software".to_owned(),
                ));
            }
            let surface = session.free_surface().ok_or(CodecError::Decoder {
                what: "find a free surface",
                code: 0,
            })?;
            session
                .decode(display, temporal_unit, frame, surface)
                .map_err(unavailable)?;
            session.refresh(header.refresh_frame_flags, surface);
            if header.show_frame {
                shown = Some((surface, header.size));
            }
        }
        let (surface, size) = shown.ok_or(CodecError::NoPicture)?;
        Ok(VaapiPicture::new(
            session.pool.clone(),
            surface,
            (size.upscaled_width as usize, size.height as usize),
            session.holds[surface].clone(),
        ))
    }
}

/// Returns whether two sequence headers' frames fit the same session.
fn same_format(a: &SequenceHeader, b: &SequenceHeader) -> bool {
    (
        a.max_width,
        a.max_height,
        a.profile,
        a.bit_depth,
        a.monochrome,
        a.film_grain,
    ) == (
        b.max_width,
        b.max_height,
        b.profile,
        b.bit_depth,
        b.monochrome,
        b.film_grain,
    )
}

impl Session {
    /// Makes the configuration, surfaces and context for `sequence`'s
    /// frames.
    fn new(device: &Arc<DecoderDisplay>, sequence: SequenceHeader) -> Result<Self, CodecError> {
        let display = &device.display;
        let mut attributes = [ConfigAttrib {
            attrib_type: ffi::ATTRIB_RT_FORMAT,
            value: ffi::RT_FORMAT_YUV420,
        }];
        let mut config = ffi::VA_INVALID_ID;
        // SAFETY: `attributes` is a valid array of its length.
        let status = unsafe {
            (display.va.create_config)(
                display.handle,
                ffi::PROFILE_AV1_MAIN,
                ffi::ENTRYPOINT_DECODE,
                attributes.as_mut_ptr(),
                attributes.len() as c_int,
                &mut config,
            )
        };
        display.check(status, "vaCreateConfig")?;
        let mut surfaces = vec![ffi::VA_INVALID_ID; SURFACES];
        let mut format = [SurfaceAttrib::integer(
            ffi::SURFACE_ATTRIB_PIXEL_FORMAT,
            ffi::FOURCC_NV12 as i32,
        )];
        // SAFETY: `surfaces` holds as many IDs as are asked for, and the
        // attribute list is valid.
        let status = unsafe {
            (display.va.create_surfaces)(
                display.handle,
                ffi::RT_FORMAT_YUV420,
                sequence.max_width,
                sequence.max_height,
                surfaces.as_mut_ptr(),
                SURFACES as u32,
                format.as_mut_ptr().cast(),
                format.len() as u32,
            )
        };
        if let Err(error) = display.check(status, "vaCreateSurfaces") {
            // SAFETY: the configuration was created above and is unused.
            unsafe { (display.va.destroy_config)(display.handle, config) };
            return Err(error);
        }
        let pool = Arc::new(SurfacePool {
            device: Arc::clone(device),
            surfaces,
        });
        let mut targets = pool.surfaces.clone();
        let mut context = ffi::VA_INVALID_ID;
        // SAFETY: the configuration and surfaces are valid, and `targets`
        // holds the surfaces' IDs.
        let status = unsafe {
            (display.va.create_context)(
                display.handle,
                config,
                sequence.max_width as c_int,
                sequence.max_height as c_int,
                ffi::PROGRESSIVE,
                targets.as_mut_ptr(),
                targets.len() as c_int,
                &mut context,
            )
        };
        let session = Self {
            sequence,
            config,
            context,
            pool,
            holds: (0..SURFACES).map(|_| Arc::new(())).collect(),
            slots: [None; NUM_REF_FRAMES],
        };
        display.check(status, "vaCreateContext")?;
        Ok(session)
    }

    /// Returns a surface no reference slot and no picture holds.
    fn free_surface(&self) -> Option<usize> {
        (0..SURFACES).find(|&surface| {
            Arc::strong_count(&self.holds[surface]) == 1 && !self.slots.contains(&Some(surface))
        })
    }

    /// Stores `surface` in the slots `refresh_frame_flags` names.
    fn refresh(&mut self, refresh_frame_flags: u8, surface: usize) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if refresh_frame_flags & (1 << index) != 0 {
                *slot = Some(surface);
            }
        }
    }

    /// Decodes `frame` of `temporal_unit` into `surface`.
    fn decode(
        &self,
        display: &Display,
        temporal_unit: &[u8],
        frame: &ParsedFrame,
        surface: usize,
    ) -> Result<(), CodecError> {
        let surfaces = &self.pool.surfaces;
        let slots = self
            .slots
            .map(|slot| slot.map_or(ffi::VA_INVALID_ID, |index| surfaces[index]));
        let params = PictureParameters::new(&self.sequence, frame, surfaces[surface], &slots);
        let mut buffers = PictureBuffers {
            display,
            ids: Vec::new(),
        };
        buffers.add(self.context, ffi::BUFFER_DECODE_PICTURE_PARAMETERS, &params)?;
        for group in tile_groups(frame) {
            buffers.add_array(
                self.context,
                ffi::BUFFER_DECODE_TILE_PARAMETERS,
                &group.tiles,
            )?;
            let data = temporal_unit
                .get(group.data)
                .ok_or(CodecError::MalformedBitstream("truncated tile group"))?;
            buffers.add_bytes(self.context, ffi::BUFFER_DECODE_TILE_DATA, data)?;
        }
        // SAFETY: the context and surface are valid.
        let status =
            unsafe { (display.va.begin_picture)(display.handle, self.context, surfaces[surface]) };
        display.check(status, "vaBeginPicture")?;
        // SAFETY: the buffers belong to this context and stay alive until
        // `buffers` drops, after the picture has ended.
        let rendered = unsafe {
            (display.va.render_picture)(
                display.handle,
                self.context,
                buffers.ids.as_mut_ptr(),
                buffers.ids.len() as c_int,
            )
        };
        // SAFETY: a picture was begun on this context.
        let ended = unsafe { (display.va.end_picture)(display.handle, self.context) };
        display.check(rendered, "vaRenderPicture")?;
        display.check(ended, "vaEndPicture")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let display = &self.pool.device.display;
        // SAFETY: the context and configuration were created on this
        // display; the surfaces outlive them in the pool, as libva requires.
        unsafe {
            if self.context != ffi::VA_INVALID_ID {
                (display.va.destroy_context)(display.handle, self.context);
            }
            (display.va.destroy_config)(display.handle, self.config);
        }
    }
}

impl VideoDecoder for VaapiDecoder {
    fn decode(&mut self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        self.decode_unit(temporal_unit)?.to_memory()
    }

    fn decode_picture(&mut self, temporal_unit: &[u8]) -> Result<DecodedPicture, CodecError> {
        self.decode_unit(temporal_unit).map(DecodedPicture::Vaapi)
    }

    fn description(&self) -> &str {
        &self.description
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decoder opens, after decoding a stream exactly as rav1d does, or
    /// says why this computer cannot decode AV1 in hardware; CI's runners
    /// have no GPU, so there it explains.
    #[test]
    fn opens_or_explains() {
        match VaapiDecoder::new() {
            Ok(decoder) => assert!(decoder.description().starts_with("VA-API on ")),
            Err(CodecError::HardwareUnavailable(reason)) => assert!(!reason.is_empty()),
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    /// Decodes the IVF file `QDP_TEST_AV1_STREAM` names with the first GPU's
    /// decoder, without checking its pictures, and reads each picture into
    /// memory, so that what the decoder gives the driver can be traced
    /// (`LIBVA_TRACE`) and compared with another decoder's.
    #[test]
    #[ignore = "needs a VA-API driver and a stream"]
    fn decodes_a_stream_for_tracing() {
        let path =
            std::env::var("QDP_TEST_AV1_STREAM").expect("QDP_TEST_AV1_STREAM names an IVF file");
        let file = std::fs::read(path).unwrap();
        let mut decoder = VaapiDecoder::open().unwrap();
        let mut shown = 0;
        for unit in ivf_frames(&file) {
            let picture = decoder.decode_unit(unit).unwrap();
            let frame = picture.to_memory().unwrap();
            assert_eq!((frame.width(), frame.height()), picture.visible_size());
            shown += 1;
        }
        assert!(shown > 0);
    }

    /// Returns the frames of an IVF file.
    fn ivf_frames(file: &[u8]) -> Vec<&[u8]> {
        /// The bytes of the file header.
        const FILE_HEADER: usize = 32;
        /// The bytes of each frame's header.
        const FRAME_HEADER: usize = 12;
        let mut frames = Vec::new();
        let mut offset = FILE_HEADER;
        while offset + FRAME_HEADER <= file.len() {
            let size = u32::from_le_bytes(file[offset..offset + 4].try_into().unwrap()) as usize;
            offset += FRAME_HEADER;
            frames.push(&file[offset..offset + size]);
            offset += size;
        }
        frames
    }
}
