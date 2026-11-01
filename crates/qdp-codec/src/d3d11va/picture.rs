//! A picture a D3D11 decoder decoded: a slice of its NV12 texture array, on
//! the GPU.

use std::sync::Arc;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};

use crate::{CodecError, Yuv420Frame};

/// A decoded picture in a slice of a decoder's texture array, which the
/// decoder does not write again while the picture exists.
pub struct D3d11Picture {
    /// The texture array, NV12.
    texture: ID3D11Texture2D,
    /// The slice of the array the picture is in.
    slice: u32,
    /// The width and height shown, which may be less than the texture's.
    visible: (usize, usize),
    /// Whether shaders may sample the texture; otherwise a window must copy
    /// the slice into a texture of its own first.
    shader_readable: bool,
    /// Held while the picture exists, which keeps the decoder from using
    /// the slice again.
    _hold: Arc<()>,
}

// SAFETY: textures are free threaded objects of the shared device, whose
// immediate context is thread safe; the decoder no longer writes a slice
// while a picture of it exists.
unsafe impl Send for D3d11Picture {}

impl std::fmt::Debug for D3d11Picture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("D3d11Picture")
            .field("slice", &self.slice)
            .field("visible", &self.visible)
            .finish_non_exhaustive()
    }
}

impl D3d11Picture {
    /// Wraps slice `slice` of `texture`, of which `visible` is shown.
    pub(crate) fn new(
        texture: ID3D11Texture2D,
        slice: u32,
        visible: (usize, usize),
        shader_readable: bool,
        hold: Arc<()>,
    ) -> Self {
        Self {
            texture,
            slice,
            visible,
            shader_readable,
            _hold: hold,
        }
    }

    /// Returns the NV12 texture array the picture is in, a texture of the
    /// device [`video_device`](crate::video_device) returns.
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// Returns the slice of the texture array the picture is in.
    pub fn slice(&self) -> u32 {
        self.slice
    }

    /// Returns whether shaders may sample the texture directly; if not, the
    /// slice must be copied into a texture that allows it first.
    pub fn shader_readable(&self) -> bool {
        self.shader_readable
    }

    /// Returns the width and height shown, in pixels.
    pub fn visible_size(&self) -> (usize, usize) {
        self.visible
    }

    /// Returns the picture with at most `width × height` pixels shown.
    pub fn cropped(mut self, width: usize, height: usize) -> Self {
        self.visible = (self.visible.0.min(width), self.visible.1.min(height));
        self
    }

    /// Copies the picture into memory, through a texture the processor can
    /// read.
    pub fn to_memory(&self) -> Result<Yuv420Frame, CodecError> {
        let error = |what: &'static str| {
            move |error: windows::core::Error| CodecError::Decoder {
                what,
                code: error.code().0,
            }
        };
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the texture is valid and the description valid to write.
        unsafe { self.texture.GetDesc(&mut description) };
        // SAFETY: a texture's device is valid while the texture is.
        let device: ID3D11Device =
            unsafe { self.texture.GetDevice() }.map_err(error("find the decoder's device"))?;
        let staging_description = D3D11_TEXTURE2D_DESC {
            ArraySize: 1,
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..description
        };
        let mut staging = None;
        // SAFETY: the description is valid and the output valid to write.
        unsafe { device.CreateTexture2D(&staging_description, None, Some(&mut staging)) }
            .map_err(error("make a texture to read the picture"))?;
        let staging = staging.ok_or(CodecError::Decoder {
            what: "make a texture to read the picture",
            code: 0,
        })?;
        // SAFETY: the device's immediate context is valid while the device is.
        let context =
            unsafe { device.GetImmediateContext() }.map_err(error("find the device's context"))?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are NV12 of the same size on the same device;
        // the slice exists; mapping waits for the copy, and the mapping is
        // read only while the texture is mapped.
        let frame = unsafe {
            context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &self.texture, self.slice, None);
            context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(error("map the picture"))?;
            let pitch = mapped.RowPitch as usize;
            let rows = description.Height as usize;
            let data = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * rows * 3 / 2);
            let (luma, chroma) = data.split_at(pitch * rows);
            let frame = Yuv420Frame::from_nv12(
                self.visible.0,
                self.visible.1,
                (luma, pitch),
                (chroma, pitch),
            );
            context.Unmap(&staging, 0);
            frame
        };
        frame.ok_or(CodecError::UnsupportedFormat)
    }
}
