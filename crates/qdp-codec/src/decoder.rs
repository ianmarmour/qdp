//! AV1 decoding with rav1d.
//!
//! rav1d exposes the C interface of dav1d, so this module is the one place in
//! QDP that uses `unsafe`: every call is wrapped in a safe method, and each
//! `unsafe` block states the contract it relies on, from rav1d's documentation.

#![allow(unsafe_code)]

use std::mem::MaybeUninit;
use std::ptr::NonNull;

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I420;
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

use crate::{CodecError, Yuv420Frame};

/// The bit depth of the pictures this decoder returns.
const BIT_DEPTH: i32 = 8;

/// The largest number of frames the decoder holds before returning a picture.
/// One gives the lowest latency: each temporal unit produces its picture at once.
const MAX_FRAME_DELAY: i32 = 1;

/// Converts a rav1d result code into an error, with `EAGAIN` as `Ok(false)`.
fn check(result: i32, what: &'static str) -> Result<bool, CodecError> {
    match result {
        0 => Ok(true),
        code if code == -libc::EAGAIN => Ok(false),
        code => Err(CodecError::Decoder { what, code }),
    }
}

/// An AV1 decoder for QDP streams: each temporal unit holds exactly one shown
/// frame, so each call to [`Av1Decoder::decode`] returns one picture.
pub struct Av1Decoder {
    /// The decoder context, from `dav1d_open` and not yet closed.
    context: Dav1dContext,
}

// SAFETY: rav1d contexts are internally synchronized and may be used from any
// thread; dav1d documents that a context may be used from one thread at a time,
// which `&mut self` on every method guarantees.
unsafe impl Send for Av1Decoder {}

impl std::fmt::Debug for Av1Decoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Av1Decoder").finish_non_exhaustive()
    }
}

impl Av1Decoder {
    /// Opens a decoder that uses `threads` threads, or 0 to choose automatically.
    pub fn new(threads: u32) -> Result<Self, CodecError> {
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `settings` is valid to write to; `dav1d_default_settings`
        // initializes every field.
        unsafe { dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr())) };
        // SAFETY: initialized by `dav1d_default_settings` above.
        let mut settings = unsafe { settings.assume_init() };
        settings.n_threads = i32::try_from(threads).unwrap_or(i32::MAX);
        settings.max_frame_delay = MAX_FRAME_DELAY;

        let mut context: Option<Dav1dContext> = None;
        // SAFETY: `context` is valid to write to and `settings` valid to read.
        let result = unsafe {
            dav1d_open(
                Some(NonNull::from(&mut context)),
                Some(NonNull::from(&mut settings)),
            )
        };
        check(result.0, "open")?;
        let context = context.ok_or(CodecError::Decoder {
            what: "open",
            code: 0,
        })?;
        Ok(Self { context })
    }

    /// Decodes one temporal unit and returns its picture.
    pub fn decode(&mut self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        if temporal_unit.is_empty() {
            return Err(CodecError::MalformedBitstream("empty temporal unit"));
        }
        let mut data = Dav1dData::default();
        // SAFETY: `data` is valid to write to; on success it owns a buffer of
        // `temporal_unit.len()` bytes starting at the returned pointer.
        let buffer =
            unsafe { dav1d_data_create(Some(NonNull::from(&mut data)), temporal_unit.len()) };
        if buffer.is_null() {
            return Err(CodecError::Decoder {
                what: "allocate data",
                code: 0,
            });
        }
        // SAFETY: `buffer` holds `temporal_unit.len()` writable bytes that do not
        // overlap `temporal_unit`.
        unsafe {
            std::ptr::copy_nonoverlapping(temporal_unit.as_ptr(), buffer, temporal_unit.len())
        };

        // SAFETY: the context is open, and `data` is valid to read and write.
        let sent = unsafe { dav1d_send_data(Some(self.context), Some(NonNull::from(&mut data))) };
        if data.sz > 0 {
            // SAFETY: `data` still owns its buffer, which is released here.
            unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
        }
        if !check(sent.0, "send data")? {
            return Err(CodecError::Decoder {
                what: "send data",
                code: -libc::EAGAIN,
            });
        }

        let mut picture = Dav1dPicture::default();
        // SAFETY: the context is open, and `picture` is valid to write to.
        let received =
            unsafe { dav1d_get_picture(Some(self.context), Some(NonNull::from(&mut picture))) };
        if !check(received.0, "get picture")? {
            return Err(CodecError::NoPicture);
        }
        let frame = copy_picture(&picture);
        // SAFETY: `picture` was filled by `dav1d_get_picture` and is released
        // once, here.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut picture))) };
        frame
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        let mut context = Some(self.context);
        // SAFETY: the context is open and is not used after this.
        unsafe { dav1d_close(Some(NonNull::from(&mut context))) };
    }
}

/// Copies an 8-bit 4:2:0 picture's planes into a [`Yuv420Frame`].
fn copy_picture(picture: &Dav1dPicture) -> Result<Yuv420Frame, CodecError> {
    if picture.p.layout != DAV1D_PIXEL_LAYOUT_I420 || picture.p.bpc != BIT_DEPTH {
        return Err(CodecError::UnsupportedFormat);
    }
    let width = usize::try_from(picture.p.w).map_err(|_| CodecError::UnsupportedFormat)?;
    let height = usize::try_from(picture.p.h).map_err(|_| CodecError::UnsupportedFormat)?;
    let mut frame = Yuv420Frame::black(width, height);
    let (chroma_width, chroma_height) = (frame.chroma_width(), frame.chroma_height());
    let luma_stride = picture.stride[0];
    let chroma_stride = picture.stride[1];
    let planes = [
        (width, height, luma_stride),
        (chroma_width, chroma_height, chroma_stride),
        (chroma_width, chroma_height, chroma_stride),
    ];
    for (index, (plane_width, plane_height, stride)) in planes.into_iter().enumerate() {
        let source = picture.data[index]
            .ok_or(CodecError::UnsupportedFormat)?
            .cast::<u8>();
        let destination = match index {
            0 => frame.y_mut(),
            1 => frame.u_mut(),
            _ => frame.v_mut(),
        };
        for (row, destination_row) in destination
            .chunks_exact_mut(plane_width)
            .take(plane_height)
            .enumerate()
        {
            let offset = row as isize * stride;
            // SAFETY: rav1d guarantees each plane holds `plane_height` rows of at
            // least `plane_width` bytes, `stride` bytes apart, valid until the
            // picture is released, which happens after this copy.
            let source_row =
                unsafe { std::slice::from_raw_parts(source.as_ptr().offset(offset), plane_width) };
            destination_row.copy_from_slice(source_row);
        }
    }
    Ok(frame)
}
