//! Drawing the software decoder's pictures into the app's surface, which the
//! processor fills: in YV12 when the surface takes it, which is a copy of
//! each plane, and in RGBX otherwise. Filling a surface's buffer goes through
//! its raw pointer, so this module uses `unsafe`.

#![allow(unsafe_code)]

use ndk::hardware_buffer_format::HardwareBufferFormat;
use ndk::native_window::NativeWindow;
use qdp_codec::{Yuv420Frame, video_surface};
use tracing::{info, warn};

use crate::pixels::{RGBX_BYTES, fill_rgbx, fill_yv12, yv12_size};

/// Android's YV12 format: a luma plane, then V, then U.
const FORMAT_YV12: i32 = 0x3231_5659;

/// How the surface's buffers hold pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// YV12: the picture's planes, copied.
    Yv12,
    /// RGBX: the picture converted on the processor.
    Rgbx,
}

impl Layout {
    /// Returns the buffer format of the layout.
    fn format(self) -> HardwareBufferFormat {
        match self {
            Self::Yv12 => HardwareBufferFormat::from(FORMAT_YV12),
            Self::Rgbx => HardwareBufferFormat::R8G8B8X8_UNORM,
        }
    }
}

/// The surface the painter draws into, with its buffers set up.
#[derive(Debug)]
struct Target {
    /// The surface.
    window: NativeWindow,
    /// The size its buffers were set to, in pixels.
    size: (usize, usize),
    /// How they hold pixels.
    layout: Layout,
}

/// Draws pictures in memory into the app's video surface.
#[derive(Debug, Default)]
pub struct SoftwareWindow {
    /// The surface drawn into last.
    target: Option<Target>,
    /// Whether the surface refused YV12, so that RGBX is used from then on.
    no_yv12: bool,
}

impl SoftwareWindow {
    /// Returns a painter that has drawn nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws `frame` into the app's surface, scaled to fill it.
    pub fn draw(&mut self, frame: &Yuv420Frame) -> Result<(), String> {
        let window = video_surface().ok_or("the app has no video surface")?;
        // YV12 buffers have an even size.
        let size = (frame.width() & !1, frame.height() & !1);
        if size.0 == 0 || size.1 == 0 {
            return Ok(());
        }
        let current = self
            .target
            .as_ref()
            .is_some_and(|target| target.window == window && target.size == size);
        if !current {
            self.target = Some(self.set_up(window, size)?);
        }
        let Some(target) = &self.target else {
            return Ok(());
        };
        match target.fill(frame) {
            Err(FillError::NotYv12) => {
                warn!("the surface does not take YV12; drawing in RGBX");
                self.no_yv12 = true;
                self.target = None;
                Ok(())
            }
            Err(FillError::Failed(error)) => Err(error),
            Ok(()) => Ok(()),
        }
    }

    /// Sets `window`'s buffers to `size`, in YV12 if it takes it.
    fn set_up(&self, window: NativeWindow, size: (usize, usize)) -> Result<Target, String> {
        let (width, height) = (
            i32::try_from(size.0).map_err(|_| "the picture is too wide")?,
            i32::try_from(size.1).map_err(|_| "the picture is too tall")?,
        );
        let layouts: &[Layout] = if self.no_yv12 {
            &[Layout::Rgbx]
        } else {
            &[Layout::Yv12, Layout::Rgbx]
        };
        let mut last_error = None;
        for &layout in layouts {
            match window.set_buffers_geometry(width, height, Some(layout.format())) {
                Ok(()) => {
                    info!(
                        width,
                        height,
                        ?layout,
                        "drawing decoded pictures on the processor"
                    );
                    return Ok(Target {
                        window,
                        size,
                        layout,
                    });
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(format!(
            "cannot set up the surface's buffers: {}",
            last_error.map_or_else(String::new, |error| error.to_string())
        ))
    }
}

/// Why a picture was not drawn.
#[derive(Debug)]
enum FillError {
    /// The surface handed out a buffer that is not YV12.
    NotYv12,
    /// Drawing failed.
    Failed(String),
}

impl Target {
    /// Draws `frame` into the surface's next buffer and posts it.
    fn fill(&self, frame: &Yuv420Frame) -> Result<(), FillError> {
        let mut buffer = self
            .window
            .lock(None)
            .map_err(|error| FillError::Failed(format!("cannot lock the surface: {error}")))?;
        let (width, height) = (
            self.size.0.min(buffer.width()),
            self.size.1.min(buffer.height()),
        );
        let stride = buffer.stride();
        let format = i32::from(buffer.format());
        let bytes = match self.layout {
            Layout::Yv12 if format != FORMAT_YV12 => return Err(FillError::NotYv12),
            Layout::Yv12 => yv12_size(stride, buffer.height()),
            Layout::Rgbx => stride * buffer.height() * RGBX_BYTES,
        };
        // SAFETY: a locked buffer is `stride` pixels by its height in its
        // format, which for YV12 includes the two chroma planes, and is
        // ours until the lock is dropped, below.
        let pixels = unsafe { std::slice::from_raw_parts_mut(buffer.bits().cast::<u8>(), bytes) };
        match self.layout {
            Layout::Yv12 => fill_yv12(pixels, stride, frame, width, height),
            Layout::Rgbx => fill_rgbx(pixels, stride, frame, width, height),
        }
        // Dropping the lock posts the buffer.
        drop(buffer);
        Ok(())
    }
}
