//! Decoded pictures: in memory, or, from a hardware decoder, still in the
//! buffer it decoded into, which the GPU can draw from without a copy.

use crate::{CodecError, Yuv420Frame};

/// A decoded picture.
#[derive(Debug)]
pub enum DecodedPicture {
    /// A picture in memory.
    Memory(Yuv420Frame),
    /// A picture in the pixel buffer VideoToolbox decoded into, which Metal
    /// draws from directly.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    PixelBuffer(crate::PixelBufferPicture),
    /// A picture MediaCodec decoded, rendered into the app's video surface
    /// when shown.
    #[cfg(target_os = "android")]
    Surface(crate::SurfacePicture),
    /// A picture in the texture array a D3D11 video decoder decoded into,
    /// which Direct3D draws from directly.
    #[cfg(windows)]
    D3d11(crate::D3d11Picture),
    /// A picture in the surface a VA-API decoder decoded into.
    #[cfg(target_os = "linux")]
    Vaapi(crate::VaapiPicture),
}

impl DecodedPicture {
    /// Returns the width and height shown, in pixels.
    pub fn size(&self) -> (usize, usize) {
        match self {
            Self::Memory(frame) => (frame.width(), frame.height()),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Self::PixelBuffer(picture) => picture.visible_size(),
            #[cfg(target_os = "android")]
            Self::Surface(picture) => picture.visible_size(),
            #[cfg(windows)]
            Self::D3d11(picture) => picture.visible_size(),
            #[cfg(target_os = "linux")]
            Self::Vaapi(picture) => picture.visible_size(),
        }
    }

    /// Returns the picture cut to at most `width × height` pixels, as a
    /// stream whose decoder pads its pictures shows them.
    pub fn cropped(self, width: usize, height: usize) -> Self {
        match self {
            Self::Memory(frame) => Self::Memory(frame.cropped(width, height)),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Self::PixelBuffer(picture) => Self::PixelBuffer(picture.cropped(width, height)),
            #[cfg(target_os = "android")]
            Self::Surface(picture) => Self::Surface(picture.cropped(width, height)),
            #[cfg(windows)]
            Self::D3d11(picture) => Self::D3d11(picture.cropped(width, height)),
            #[cfg(target_os = "linux")]
            Self::Vaapi(picture) => Self::Vaapi(picture.cropped(width, height)),
        }
    }

    /// Returns the picture in memory, if it is in memory.
    pub fn in_memory(&self) -> Option<&Yuv420Frame> {
        match self {
            Self::Memory(frame) => Some(frame),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Self::PixelBuffer(_) => None,
            #[cfg(target_os = "android")]
            Self::Surface(_) => None,
            #[cfg(windows)]
            Self::D3d11(_) => None,
            #[cfg(target_os = "linux")]
            Self::Vaapi(_) => None,
        }
    }

    /// Calls `use_frame` with the picture in memory: the picture itself if
    /// it is in memory, or a copy of it.
    pub fn with_memory<T>(
        &self,
        use_frame: impl FnOnce(&Yuv420Frame) -> T,
    ) -> Result<T, CodecError> {
        match self {
            Self::Memory(frame) => Ok(use_frame(frame)),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Self::PixelBuffer(picture) => Ok(use_frame(&picture.to_memory()?)),
            #[cfg(target_os = "android")]
            Self::Surface(_) => Err(surface_unreadable()),
            #[cfg(windows)]
            Self::D3d11(picture) => Ok(use_frame(&picture.to_memory()?)),
            #[cfg(target_os = "linux")]
            Self::Vaapi(picture) => Ok(use_frame(&picture.to_memory()?)),
        }
    }

    /// Returns the picture in memory, copying it there if it is not.
    pub fn into_memory(self) -> Result<Yuv420Frame, CodecError> {
        match self {
            Self::Memory(frame) => Ok(frame),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Self::PixelBuffer(picture) => picture.to_memory(),
            #[cfg(target_os = "android")]
            Self::Surface(_) => Err(surface_unreadable()),
            #[cfg(windows)]
            Self::D3d11(picture) => picture.to_memory(),
            #[cfg(target_os = "linux")]
            Self::Vaapi(picture) => picture.to_memory(),
        }
    }
}

/// Returns the error for reading a picture MediaCodec rendered into the
/// video surface, which the processor cannot read.
#[cfg(target_os = "android")]
fn surface_unreadable() -> CodecError {
    CodecError::HardwareUnavailable(
        "a picture MediaCodec decodes into the video surface cannot be read".to_owned(),
    )
}

impl From<Yuv420Frame> for DecodedPicture {
    fn from(frame: Yuv420Frame) -> Self {
        Self::Memory(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture in memory is cropped, borrowed and taken as it is.
    #[test]
    fn keeps_pictures_in_memory() {
        let picture = DecodedPicture::from(Yuv420Frame::black(64, 34));
        assert_eq!(picture.size(), (64, 34));
        let picture = picture.cropped(64, 32);
        assert_eq!(picture.size(), (64, 32));
        assert_eq!(picture.in_memory().map(Yuv420Frame::height), Some(32));
        assert_eq!(picture.with_memory(Yuv420Frame::width).unwrap(), 64);
        assert_eq!(picture.into_memory().unwrap(), Yuv420Frame::black(64, 32));
    }
}
