//! Shows the session's video in the app's surface: MediaCodec decodes
//! straight into it, and the software decoder's pictures are drawn there.

use qdp_app::sink::Presenter;
use qdp_codec::DecodedPicture;
use qdp_runtime::FrameInfo;

/// Shows decoded pictures in the app's surface.
pub struct AndroidPresenter {
    /// Draws pictures in memory, from the software decoder.
    painter: Painter,
}

impl AndroidPresenter {
    /// Returns a presenter for the app's surface.
    pub fn new() -> Self {
        Self {
            painter: Painter::new(),
        }
    }
}

impl Default for AndroidPresenter {
    fn default() -> Self {
        Self::new()
    }
}

impl Presenter for AndroidPresenter {
    /// Shows `frame`: MediaCodec's in the surface it decoded for, and the
    /// software decoder's by drawing it there.
    fn show(&mut self, frame: DecodedPicture, _info: &FrameInfo) -> Result<(), String> {
        match frame {
            #[cfg(target_os = "android")]
            DecodedPicture::Surface(picture) => picture.render().map_err(|error| error.to_string()),
            frame => frame
                .with_memory(|memory| self.painter.draw(memory))
                .map_err(|error| error.to_string())
                .and_then(|drawn| drawn),
        }
    }
}

/// Draws pictures in memory into the app's surface.
#[cfg(target_os = "android")]
use crate::software::SoftwareWindow as Painter;

/// Draws pictures in memory: only the app's surface can show them.
#[cfg(not(target_os = "android"))]
struct Painter;

#[cfg(not(target_os = "android"))]
impl Painter {
    /// Returns the painter.
    fn new() -> Self {
        Self
    }

    /// Refuses to draw `frame`, having nowhere to.
    fn draw(&mut self, frame: &qdp_codec::Yuv420Frame) -> Result<(), String> {
        let _ = frame;
        Err("pictures are drawn only into an Android surface".to_owned())
    }
}
