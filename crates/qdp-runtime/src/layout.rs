//! How a host monitor's video maps onto the monitor it captures. The video
//! may be smaller than the capture, to fit the client's display area or the
//! largest frame it decodes; positions in the host desktop count pixels of
//! the video, and the host converts them to its operating system's pixels
//! (Section 1.5.6).

use qdp::wire::VarInt;

use crate::host_input::{InputAction, TouchPoint};
use crate::media::{CursorState, PointerPosition};

/// The smallest monitor side QDP allows (Section 6.3).
const MIN_SIDE: usize = 16;

/// The largest monitor side QDP allows (Section 6.3).
const MAX_SIDE: usize = 16384;

/// The sizes of a captured monitor and of its video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VideoLayout {
    /// The captured monitor's width and height, in the operating system's
    /// pixels.
    pub(crate) capture: (usize, usize),
    /// The video's width and height, in pixels of the host desktop.
    pub(crate) video: (usize, usize),
}

impl VideoLayout {
    /// Returns the layout that shows a `capture`-sized monitor at the
    /// largest size within every one of `bounds`, keeping its shape. The
    /// video is never larger than the capture.
    pub(crate) fn fit(capture: (usize, usize), bounds: &[(usize, usize)]) -> Self {
        let (width, height) = (capture.0.max(1), capture.1.max(1));
        // The tightest bound, as the fraction `numerator / denominator` of
        // the capture's size.
        let (mut numerator, mut denominator) = (1, 1);
        let sides = bounds
            .iter()
            .chain(&[(MAX_SIDE, MAX_SIDE)])
            .filter(|&&(bound_width, bound_height)| bound_width > 0 && bound_height > 0)
            .flat_map(|&(bound_width, bound_height)| {
                [(bound_width, width), (bound_height, height)]
            });
        for (bound, length) in sides {
            if bound * denominator < numerator * length {
                (numerator, denominator) = (bound, length);
            }
        }
        if numerator >= denominator {
            return Self {
                capture,
                video: capture,
            };
        }
        // Even sides keep the chroma planes exactly half the size.
        let side =
            |length: usize| ((length * numerator / denominator) & !1).clamp(MIN_SIDE, MAX_SIDE);
        Self {
            capture,
            video: (side(width), side(height)),
        }
    }

    /// Returns whether the video is scaled from the capture.
    pub(crate) fn is_scaled(&self) -> bool {
        self.capture != self.video
    }

    /// Converts a horizontal host desktop position to the capture's.
    fn x_to_capture(&self, x: i32) -> i32 {
        rescale(x, self.capture.0, self.video.0)
    }

    /// Converts a vertical host desktop position to the capture's.
    fn y_to_capture(&self, y: i32) -> i32 {
        rescale(y, self.capture.1, self.video.1)
    }

    /// Converts a position in the capture's pixels to the host desktop's.
    pub(crate) fn position_in_video(&self, (x, y): (i32, i32)) -> (i32, i32) {
        (
            rescale(x, self.video.0, self.capture.0),
            rescale(y, self.video.1, self.capture.1),
        )
    }

    /// Returns the Scale to send with a cursor image drawn for `scale`, so
    /// that the client draws it at its size relative to the captured monitor
    /// rather than to the smaller video (Section 9.2).
    pub(crate) fn cursor_scale(&self, scale: VarInt) -> VarInt {
        let scaled =
            u128::from(scale.into_inner()) * self.capture.0 as u128 / self.video.0.max(1) as u128;
        u64::try_from(scaled)
            .ok()
            .and_then(|scaled| VarInt::new(scaled).ok())
            .unwrap_or(VarInt::MAX)
    }

    /// Returns a captured pointer with its position in the host desktop.
    pub(crate) fn cursor_in_video(&self, state: &CursorState) -> CursorState {
        let position = match state.position {
            PointerPosition::At(x, y) => {
                let (x, y) = self.position_in_video((x, y));
                PointerPosition::At(x, y)
            }
            other => other,
        };
        CursorState {
            position,
            shape: state.shape.clone(),
        }
    }

    /// Converts the positions of an input action from the host desktop to
    /// the capture's.
    pub(crate) fn action_in_capture(&self, action: InputAction) -> InputAction {
        if !self.is_scaled() {
            return action;
        }
        match action {
            InputAction::PointerMoveTo { x, y } => InputAction::PointerMoveTo {
                x: self.x_to_capture(x),
                y: self.y_to_capture(y),
            },
            InputAction::Touch(points) => InputAction::Touch(
                points
                    .into_iter()
                    .map(|point| TouchPoint {
                        x: self.x_to_capture(point.x),
                        y: self.y_to_capture(point.y),
                        ..point
                    })
                    .collect(),
            ),
            InputAction::Pen(mut report) => {
                report.x = self.x_to_capture(report.x);
                report.y = self.y_to_capture(report.y);
                InputAction::Pen(report)
            }
            other => other,
        }
    }
}

/// Returns `value × to ÷ from`, saturating.
fn rescale(value: i32, to: usize, from: usize) -> i32 {
    let scaled = i64::from(value) * to as i64 / from.max(1) as i64;
    i32::try_from(scaled).unwrap_or(if scaled < 0 { i32::MIN } else { i32::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_input::TouchPhase;

    /// The video fits every bound, keeps its shape and is never enlarged.
    #[test]
    fn fits_bounds() {
        let uhd = (3840, 2160);
        assert_eq!(VideoLayout::fit(uhd, &[]).video, uhd);
        assert_eq!(VideoLayout::fit(uhd, &[(7680, 4320)]).video, uhd);
        assert_eq!(VideoLayout::fit(uhd, &[(1920, 1200)]).video, (1920, 1080));
        assert_eq!(
            VideoLayout::fit(uhd, &[(2560, 1600), (1280, 1280)]).video,
            (1280, 720)
        );
        assert_eq!(VideoLayout::fit(uhd, &[(0, 0)]).video, uhd);
        assert_eq!(
            VideoLayout::fit((1366, 768), &[(683, 400)]).video,
            (682, 384)
        );
        assert_eq!(VideoLayout::fit((40000, 100), &[]).video, (16384, 40));
        assert_eq!(VideoLayout::fit((4000, 20), &[(100, 100)]).video, (100, 16));
    }

    /// Positions convert between the video and the capture, and cursor
    /// images keep their size relative to the capture.
    #[test]
    fn converts_positions() {
        let layout = VideoLayout::fit((3840, 2160), &[(1920, 1080)]);
        assert!(layout.is_scaled());
        assert_eq!(
            layout.action_in_capture(InputAction::PointerMoveTo { x: 256, y: -512 }),
            InputAction::PointerMoveTo { x: 512, y: -1024 }
        );
        let touch = InputAction::Touch(vec![TouchPoint {
            id: 1,
            phase: TouchPhase::Down,
            x: 100,
            y: 10,
            pressure: 0,
        }]);
        let InputAction::Touch(points) = layout.action_in_capture(touch) else {
            panic!("not a touch");
        };
        assert_eq!((points[0].x, points[0].y), (200, 20));
        assert_eq!(
            layout.action_in_capture(InputAction::KeyDown(4)),
            InputAction::KeyDown(4)
        );
        assert_eq!(layout.position_in_video((1000, 2000)), (500, 1000));
        assert_eq!(
            layout.cursor_scale(VarInt::from_u32(100)),
            VarInt::from_u32(200)
        );
        let pointer = CursorState {
            position: PointerPosition::At(640, 360),
            shape: None,
        };
        assert_eq!(
            layout.cursor_in_video(&pointer).position,
            PointerPosition::At(320, 180)
        );
        let unscaled = VideoLayout::fit((1920, 1080), &[]);
        assert_eq!(unscaled.position_in_video((7, 9)), (7, 9));
        assert_eq!(
            unscaled.cursor_scale(VarInt::from_u32(150)),
            VarInt::from_u32(150)
        );
    }
}
