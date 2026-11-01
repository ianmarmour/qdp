//! The D3D11 video decoder: frames parsed by [`FrameParser`] are handed to
//! the GPU's AV1 decoder through DXVA, each into a slice of an NV12 texture
//! array that the reference slots and the pictures returned share.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::{debug, info};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_DECODER, D3D11_BIND_SHADER_RESOURCE, D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
    D3D11_TEX2D_VDOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VDOV_DIMENSION_TEXTURE2D,
    D3D11_VIDEO_DECODER_BUFFER_BITSTREAM, D3D11_VIDEO_DECODER_BUFFER_DESC,
    D3D11_VIDEO_DECODER_BUFFER_PICTURE_PARAMETERS, D3D11_VIDEO_DECODER_BUFFER_SLICE_CONTROL,
    D3D11_VIDEO_DECODER_BUFFER_TYPE, D3D11_VIDEO_DECODER_CONFIG, D3D11_VIDEO_DECODER_DESC,
    D3D11_VIDEO_DECODER_OUTPUT_VIEW_DESC, D3D11_VIDEO_DECODER_OUTPUT_VIEW_DESC_0, ID3D11Device,
    ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDecoder, ID3D11VideoDecoderOutputView,
    ID3D11VideoDevice,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::core::{GUID, HRESULT, Interface};

use super::dxva::{self, NO_TEXTURE, PicParams};
use super::picture::D3d11Picture;
use crate::frame_parser::{FrameParser, NUM_REF_FRAMES, ParsedFrame};
use crate::sequence::SequenceHeader;
use crate::{CodecError, DecodedPicture, VideoDecoder, Yuv420Frame};

/// How the decoder describes itself.
const DESCRIPTION: &str = "D3D11 video (hardware)";

/// The slices of the texture array: one for each reference slot, one for
/// the frame being decoded, and the rest for pictures a window still shows.
const SLICES: usize = 16;

/// The alignment of the texture array's width and height.
const SURFACE_ALIGNMENT: u32 = 128;

/// How often `DecoderBeginFrame` is tried while the decoder is busy.
const BEGIN_FRAME_ATTEMPTS: u32 = 50;

/// How long to wait between those attempts.
const BEGIN_FRAME_RETRY: Duration = Duration::from_millis(2);

/// `E_PENDING`: the decoder is busy with an earlier frame.
const E_PENDING: HRESULT = HRESULT(0x8000_000a_u32 as i32);

/// `ConfigBitstreamRaw` of a configuration that takes the bitstream as it is.
const BITSTREAM_RAW: u32 = 1;

/// `guidConfigBitstreamEncryption` of a configuration without encryption:
/// `DXVA_NoEncrypt`.
const NO_ENCRYPTION: GUID = GUID::from_u128(0x1b81bed0_a0c7_11d3_b984_00c04f2e73c5);

/// Returns a decoding error for a failed call.
fn failed(what: &'static str) -> impl Fn(windows::core::Error) -> CodecError {
    move |error| CodecError::HardwareUnavailable(format!("{what}: {error}"))
}

/// A hardware AV1 decoder using D3D11 video, on GPUs that decode AV1
/// (NVIDIA RTX 30 and later, AMD RX 6000 and later, Intel Xe and later,
/// Qualcomm Snapdragon X).
///
/// Only 8-bit 4:2:0 streams are decoded in hardware; others report
/// [`CodecError::HardwareUnavailable`], which hands them to software.
pub struct D3d11Decoder {
    /// The shared device.
    device: ID3D11Device,
    /// Its video interface.
    video_device: ID3D11VideoDevice,
    /// Its immediate context's video interface.
    video_context: ID3D11VideoContext,
    /// The frame headers read so far, and the reference slots' state.
    parser: FrameParser,
    /// The decoder and textures for the current sequence header, once one
    /// arrived.
    session: Option<Session>,
}

// SAFETY: the device is free threaded and its immediate context made thread
// safe when it was created; the decoder is used from one thread at a time,
// which `&mut self` on every method guarantees.
unsafe impl Send for D3d11Decoder {}

impl std::fmt::Debug for D3d11Decoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("D3d11Decoder")
            .finish_non_exhaustive()
    }
}

/// The decoder and texture array for one sequence's frames.
struct Session {
    /// The sequence header they were made for.
    sequence: SequenceHeader,
    /// The DXVA decoder.
    decoder: ID3D11VideoDecoder,
    /// The texture array frames are decoded into.
    texture: ID3D11Texture2D,
    /// Whether shaders may sample the texture array.
    shader_readable: bool,
    /// A decoder output view of each slice.
    views: Vec<ID3D11VideoDecoderOutputView>,
    /// A hold on each slice, which pictures share: a slice is free when no
    /// picture holds it and no reference slot names it.
    holds: Vec<Arc<()>>,
    /// The slice each reference slot holds the frame of.
    slots: [Option<usize>; NUM_REF_FRAMES],
}

impl D3d11Decoder {
    /// Returns a decoder if this PC's GPU decodes AV1, once a short stream
    /// decoded with it has matched rav1d's decoding exactly.
    pub fn new() -> Result<Self, CodecError> {
        let decoder = Self::open()?;
        /// The outcome of the check, made once per process.
        static VERIFIED: OnceLock<Result<(), String>> = OnceLock::new();
        VERIFIED
            .get_or_init(verify)
            .clone()
            .map_err(CodecError::HardwareUnavailable)?;
        Ok(decoder)
    }

    /// Opens the decoder, without checking its pictures.
    fn open() -> Result<Self, CodecError> {
        let device = super::device::video_device()?;
        let video_device: ID3D11VideoDevice = device
            .cast()
            .map_err(failed("the device does not decode video"))?;
        // SAFETY: the device is valid; the calls have no other contract.
        let profiles = unsafe { video_device.GetVideoDecoderProfileCount() };
        let has_av1 = (0..profiles).any(|index| {
            // SAFETY: the index is below the count.
            unsafe { video_device.GetVideoDecoderProfile(index) }
                .is_ok_and(|profile| profile == D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0)
        });
        // SAFETY: the profile is a valid GUID.
        let nv12 = unsafe {
            video_device
                .CheckVideoDecoderFormat(&D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, DXGI_FORMAT_NV12)
        }
        .is_ok_and(|supported| supported.as_bool());
        if !has_av1 || !nv12 {
            return Err(CodecError::HardwareUnavailable(
                "this PC's GPU has no AV1 decoder".to_owned(),
            ));
        }
        // SAFETY: the device's immediate context is valid while it is.
        let context =
            unsafe { device.GetImmediateContext() }.map_err(failed("the device has no context"))?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(failed("the device context does not decode video"))?;
        Ok(Self {
            device,
            video_device,
            video_context,
            parser: FrameParser::new(),
            session: None,
        })
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
        let session = Session::new(&self.device, &self.video_device, sequence.clone())?;
        info!(
            width = sequence.max_width,
            height = sequence.max_height,
            "decoding AV1 with D3D11 video"
        );
        self.session = Some(session);
        Ok(())
    }

    /// Decodes a temporal unit, returning the picture it shows.
    fn decode_unit(&mut self, temporal_unit: &[u8]) -> Result<D3d11Picture, CodecError> {
        let frames = self.parser.parse(temporal_unit)?;
        self.prepare()?;
        let session = self.session.as_mut().ok_or(CodecError::NoPicture)?;
        let mut shown = None;
        for frame in &frames {
            let header = &frame.header;
            if header.show_existing_frame {
                let slice = session.slots[usize::from(header.frame_to_show_map_idx)]
                    .ok_or(CodecError::MalformedBitstream("an empty slot shown"))?;
                session.refresh(header.refresh_frame_flags, slice);
                shown = Some((slice, header.size));
                continue;
            }
            let slice = session.free_slice().ok_or(CodecError::Decoder {
                what: "find a free surface",
                code: 0,
            })?;
            session.decode(&self.video_context, temporal_unit, frame, slice)?;
            session.refresh(header.refresh_frame_flags, slice);
            if header.show_frame {
                shown = Some((slice, header.size));
            }
        }
        let (slice, size) = shown.ok_or(CodecError::NoPicture)?;
        Ok(D3d11Picture::new(
            session.texture.clone(),
            slice as u32,
            (size.upscaled_width as usize, size.height as usize),
            session.shader_readable,
            session.holds[slice].clone(),
        ))
    }
}

/// Returns whether two sequence headers' frames fit the same decoder.
fn same_format(a: &SequenceHeader, b: &SequenceHeader) -> bool {
    (
        a.max_width,
        a.max_height,
        a.profile,
        a.bit_depth,
        a.monochrome,
    ) == (
        b.max_width,
        b.max_height,
        b.profile,
        b.bit_depth,
        b.monochrome,
    )
}

impl Session {
    /// Makes the decoder and texture array for `sequence`'s frames.
    fn new(
        device: &ID3D11Device,
        video_device: &ID3D11VideoDevice,
        sequence: SequenceHeader,
    ) -> Result<Self, CodecError> {
        let width = sequence.max_width.next_multiple_of(SURFACE_ALIGNMENT);
        let height = sequence.max_height.next_multiple_of(SURFACE_ALIGNMENT);
        let description = D3D11_VIDEO_DECODER_DESC {
            Guid: D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
            SampleWidth: width,
            SampleHeight: height,
            OutputFormat: DXGI_FORMAT_NV12,
        };
        let config = choose_config(video_device, &description)?;
        // SAFETY: the description and configuration are valid.
        let decoder = unsafe { video_device.CreateVideoDecoder(&description, &config) }
            .map_err(failed("cannot make an AV1 decoder"))?;
        let (texture, shader_readable) = make_texture(device, width, height)?;
        let views = (0..SLICES as u32)
            .map(|slice| {
                let view_description = D3D11_VIDEO_DECODER_OUTPUT_VIEW_DESC {
                    DecodeProfile: D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
                    ViewDimension: D3D11_VDOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_DECODER_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VDOV { ArraySlice: slice },
                    },
                };
                let mut view = None;
                // SAFETY: the texture is a decoder texture array with this
                // slice, and the output valid to write.
                unsafe {
                    video_device.CreateVideoDecoderOutputView(
                        &texture,
                        &view_description,
                        Some(&mut view),
                    )
                }
                .map_err(failed("cannot make a decoder output view"))?;
                view.ok_or(CodecError::HardwareUnavailable(
                    "Direct3D returned no decoder output view".to_owned(),
                ))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            sequence,
            decoder,
            texture,
            shader_readable,
            views,
            holds: (0..SLICES).map(|_| Arc::new(())).collect(),
            slots: [None; NUM_REF_FRAMES],
        })
    }

    /// Returns a slice no reference slot and no picture holds.
    fn free_slice(&self) -> Option<usize> {
        (0..SLICES).find(|&slice| {
            Arc::strong_count(&self.holds[slice]) == 1 && !self.slots.contains(&Some(slice))
        })
    }

    /// Stores `slice` in the slots `refresh_frame_flags` names.
    fn refresh(&mut self, refresh_frame_flags: u8, slice: usize) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if refresh_frame_flags & (1 << index) != 0 {
                *slot = Some(slice);
            }
        }
    }

    /// Decodes `frame` of `temporal_unit` into `slice`.
    fn decode(
        &self,
        context: &ID3D11VideoContext,
        temporal_unit: &[u8],
        frame: &ParsedFrame,
        slice: usize,
    ) -> Result<(), CodecError> {
        let textures = self
            .slots
            .map(|slot| slot.map_or(NO_TEXTURE, |slice| slice as u8));
        let params = PicParams::new(&self.sequence, frame, &textures, slice as u8);
        self.begin_frame(context, slice)?;
        let result = self.submit(context, temporal_unit, frame, &params);
        // SAFETY: a frame was begun on this decoder.
        let ended = unsafe { context.DecoderEndFrame(&self.decoder) };
        result?;
        ended.map_err(failed("cannot finish decoding a frame"))
    }

    /// Begins decoding into `slice`, waiting while the decoder is busy.
    fn begin_frame(&self, context: &ID3D11VideoContext, slice: usize) -> Result<(), CodecError> {
        let mut attempts = 0;
        loop {
            // SAFETY: the decoder and view are valid, and no content key is
            // used.
            match unsafe { context.DecoderBeginFrame(&self.decoder, &self.views[slice], 0, None) } {
                Err(error) if error.code() == E_PENDING && attempts < BEGIN_FRAME_ATTEMPTS => {
                    attempts += 1;
                    std::thread::sleep(BEGIN_FRAME_RETRY);
                }
                result => return result.map_err(failed("cannot begin decoding a frame")),
            }
        }
    }

    /// Fills and submits the frame's picture parameters, bitstream and tile
    /// descriptions.
    fn submit(
        &self,
        context: &ID3D11VideoContext,
        temporal_unit: &[u8],
        frame: &ParsedFrame,
        params: &PicParams,
    ) -> Result<(), CodecError> {
        let params_size = self.fill(
            context,
            D3D11_VIDEO_DECODER_BUFFER_PICTURE_PARAMETERS,
            |buffer| {
                let bytes = params.as_bytes();
                buffer
                    .get_mut(..bytes.len())
                    .ok_or_else(|| too_small("picture parameters"))?
                    .copy_from_slice(bytes);
                Ok(bytes.len())
            },
        )?;
        let mut tiles = Vec::new();
        let bitstream_size =
            self.fill(context, D3D11_VIDEO_DECODER_BUFFER_BITSTREAM, |buffer| {
                tiles = dxva::copy_tiles(temporal_unit, frame, buffer)?;
                Ok(dxva::bitstream_len(frame))
            })?;
        let tile_bytes = dxva::tile_bytes(&tiles);
        let tiles_size = self.fill(
            context,
            D3D11_VIDEO_DECODER_BUFFER_SLICE_CONTROL,
            |buffer| {
                buffer
                    .get_mut(..tile_bytes.len())
                    .ok_or_else(|| too_small("tile descriptions"))?
                    .copy_from_slice(&tile_bytes);
                Ok(tile_bytes.len())
            },
        )?;
        let description =
            |kind: D3D11_VIDEO_DECODER_BUFFER_TYPE, size: usize| D3D11_VIDEO_DECODER_BUFFER_DESC {
                BufferType: kind,
                DataSize: size as u32,
                ..D3D11_VIDEO_DECODER_BUFFER_DESC::default()
            };
        let descriptions = [
            description(D3D11_VIDEO_DECODER_BUFFER_PICTURE_PARAMETERS, params_size),
            description(D3D11_VIDEO_DECODER_BUFFER_BITSTREAM, bitstream_size),
            description(D3D11_VIDEO_DECODER_BUFFER_SLICE_CONTROL, tiles_size),
        ];
        // SAFETY: the buffers described were filled and released.
        unsafe { context.SubmitDecoderBuffers(&self.decoder, &descriptions) }
            .map_err(failed("cannot submit a frame"))
    }

    /// Fills the decoder's buffer of `kind` with `write`, which returns the
    /// bytes it wrote, and releases it.
    fn fill(
        &self,
        context: &ID3D11VideoContext,
        kind: D3D11_VIDEO_DECODER_BUFFER_TYPE,
        write: impl FnOnce(&mut [u8]) -> Result<usize, CodecError>,
    ) -> Result<usize, CodecError> {
        let mut size = 0u32;
        let mut data = std::ptr::null_mut();
        // SAFETY: the outputs are valid to write; a frame was begun.
        unsafe { context.GetDecoderBuffer(&self.decoder, kind, &mut size, &mut data) }
            .map_err(failed("cannot get a decoder buffer"))?;
        let written = if data.is_null() {
            Err(too_small("a decoder buffer"))
        } else {
            // SAFETY: the decoder gave a buffer of `size` writable bytes,
            // valid until it is released below.
            let buffer =
                unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), size as usize) };
            write(buffer)
        };
        // SAFETY: the buffer was got above and is released once.
        let released = unsafe { context.ReleaseDecoderBuffer(&self.decoder, kind) };
        let written = written?;
        released.map_err(failed("cannot release a decoder buffer"))?;
        Ok(written)
    }
}

/// Returns the error for a decoder buffer too small for its contents.
fn too_small(what: &str) -> CodecError {
    CodecError::HardwareUnavailable(format!("the decoder's buffer is too small for {what}"))
}

/// Returns the decoder configuration that takes the bitstream as it is,
/// without encryption.
fn choose_config(
    video_device: &ID3D11VideoDevice,
    description: &D3D11_VIDEO_DECODER_DESC,
) -> Result<D3D11_VIDEO_DECODER_CONFIG, CodecError> {
    // SAFETY: the description is valid.
    let count = unsafe { video_device.GetVideoDecoderConfigCount(description) }
        .map_err(failed("cannot list AV1 decoder configurations"))?;
    let mut fallback = None;
    for index in 0..count {
        let mut config = D3D11_VIDEO_DECODER_CONFIG::default();
        // SAFETY: the index is below the count and the output valid to write.
        if unsafe { video_device.GetVideoDecoderConfig(description, index, &mut config) }.is_err()
            || config.ConfigBitstreamRaw != BITSTREAM_RAW
        {
            continue;
        }
        if config.guidConfigBitstreamEncryption == NO_ENCRYPTION {
            return Ok(config);
        }
        fallback.get_or_insert(config);
    }
    fallback.ok_or(CodecError::HardwareUnavailable(
        "the AV1 decoder takes no raw bitstream".to_owned(),
    ))
}

/// Makes the NV12 texture array frames are decoded into, which shaders may
/// sample where the driver allows it.
fn make_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, bool), CodecError> {
    let description = |bind: u32| D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: SLICES as u32,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let decoder = D3D11_BIND_DECODER.0 as u32;
    for (bind, readable) in [
        (decoder | D3D11_BIND_SHADER_RESOURCE.0 as u32, true),
        (decoder, false),
    ] {
        let mut texture = None;
        // SAFETY: the description is valid and the output valid to write.
        let made = unsafe { device.CreateTexture2D(&description(bind), None, Some(&mut texture)) };
        match (made, texture) {
            (Ok(()), Some(texture)) => {
                if !readable {
                    debug!("the decoder's textures cannot be sampled; they will be copied");
                }
                return Ok((texture, readable));
            }
            (result, _) => debug!(?result, readable, "decoder texture not made"),
        }
    }
    Err(CodecError::HardwareUnavailable(
        "cannot make the decoder's NV12 textures".to_owned(),
    ))
}

impl VideoDecoder for D3d11Decoder {
    fn decode(&mut self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        self.decode_unit(temporal_unit)?.to_memory()
    }

    fn decode_picture(&mut self, temporal_unit: &[u8]) -> Result<DecodedPicture, CodecError> {
        self.decode_unit(temporal_unit).map(DecodedPicture::D3d11)
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }
}

/// The frames of the stream [`verify`] decodes.
const VERIFY_FRAMES: usize = 4;

/// The size of that stream: wide enough for two tiles.
const VERIFY_SIZE: (usize, usize) = (512, 256);

/// Decodes a short stream with the hardware decoder and with rav1d, and
/// returns why the hardware decoder cannot be used if their pictures differ
/// at all: AV1 decoding is exact, so any difference is a fault, here or in
/// the driver.
fn verify() -> Result<(), String> {
    let (width, height) = VERIFY_SIZE;
    let mut encoder = crate::Av1Encoder::new(crate::EncoderSettings {
        width,
        height,
        frame_rate: 60,
        speed: crate::FASTEST_SPEED,
        quantizer: 100,
        threads: 1,
    })
    .map_err(|error| error.to_string())?;
    let mut hardware = D3d11Decoder::open().map_err(|error| error.to_string())?;
    let mut software = crate::Av1Decoder::new(1).map_err(|error| error.to_string())?;
    for index in 0..VERIFY_FRAMES {
        let frame = verification_frame(width, height, index);
        let request = if index == 0 {
            crate::KeyFrameRequest::Force
        } else {
            crate::KeyFrameRequest::Auto
        };
        let units = encoder
            .encode(&frame, request)
            .map_err(|error| error.to_string())?;
        for unit in units {
            let expected = software
                .decode(&unit.temporal_unit)
                .map_err(|error| error.to_string())?;
            let decoded = hardware
                .decode(&unit.temporal_unit)
                .map_err(|error| format!("hardware AV1 decoding failed: {error}"))?;
            if decoded != expected {
                let psnr = decoded.luma_psnr(&expected).unwrap_or(0.0);
                return Err(format!(
                    "hardware AV1 decoding differs from rav1d's ({psnr:.1} dB on frame {index})"
                ));
            }
        }
    }
    info!("hardware AV1 decoding matches rav1d's");
    Ok(())
}

/// Returns frame `index` of the verification stream: a scrolling gradient
/// with a moving square and coloured chroma, for prediction and filters to
/// act on.
fn verification_frame(width: usize, height: usize, index: usize) -> Yuv420Frame {
    let square = 32 + 8 * index;
    let y = (0..height)
        .flat_map(|row| (0..width).map(move |column| (row, column)))
        .map(|(row, column)| {
            if (square..square + 64).contains(&row) && (square..square + 64).contains(&column) {
                235
            } else {
                ((column + row + 6 * index) % 200) as u8 + 16
            }
        })
        .collect();
    let chroma = |offset: usize| {
        (0..width * height / 4)
            .map(|i| (64 + (i % (width / 2) + offset + 2 * index) % 128) as u8)
            .collect()
    };
    Yuv420Frame::from_planes(width, height, y, chroma(0), chroma(40))
        .expect("the planes fit the frame")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decoder opens, after decoding a stream exactly as rav1d does, or
    /// says why this PC cannot decode AV1 in hardware; CI's runners have no
    /// GPU that does, so there it explains.
    #[test]
    fn opens_or_explains() {
        match D3d11Decoder::new() {
            Ok(decoder) => assert_eq!(decoder.description(), DESCRIPTION),
            Err(CodecError::HardwareUnavailable(reason)) => assert!(!reason.is_empty()),
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    /// The verification frames differ from one to the next, so that inter
    /// frames have motion to code.
    #[test]
    fn verification_frames_move() {
        let (width, height) = VERIFY_SIZE;
        let first = verification_frame(width, height, 0);
        let second = verification_frame(width, height, 1);
        assert_eq!((first.width(), first.height()), VERIFY_SIZE);
        assert_ne!(first, second);
    }
}
