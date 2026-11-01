//! Where the picture sits in the app's view: centred, with its aspect ratio
//! kept, as the app also computes to map touches and the pointer to the
//! picture, both ways; and the pointer kept in the area a host confines it
//! to.

use qdp::message::DesktopRect;

/// Positions in pixels (Section 1.5.6).
const POSITION_UNITS: f64 = 256.0;

/// A rectangle in the layer's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Its left edge.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
    /// Its width.
    pub width: f64,
    /// Its height.
    pub height: f64,
}

impl Rect {
    /// Returns where a position on a `video`-sized picture, in its pixels,
    /// falls in the layer, with the picture here.
    pub fn to_layer(self, video: (usize, usize), (x, y): (f64, f64)) -> (f64, f64) {
        (
            self.left + x * self.width / video.0.max(1) as f64,
            self.top + y * self.height / video.1.max(1) as f64,
        )
    }

    /// Returns where a position in the layer falls on a `video`-sized
    /// picture here, in its pixels, kept on the picture.
    pub fn to_video(self, video: (usize, usize), (x, y): (f64, f64)) -> (f64, f64) {
        let scale = |position: f64, start: f64, size: f64, picture: usize| {
            let picture = picture as f64;
            ((position - start) * picture / size.max(1.0)).clamp(0.0, (picture - 1.0).max(0.0))
        };
        (
            scale(x, self.left, self.width, video.0),
            scale(y, self.top, self.height, video.1),
        )
    }

    /// Returns whether a position in the layer is on the picture.
    pub fn contains(self, (x, y): (f64, f64)) -> bool {
        (self.left..self.left + self.width).contains(&x)
            && (self.top..self.top + self.height).contains(&y)
    }
}

/// Returns where a `video` picture, its width and height in pixels, is
/// drawn in a `layer` of that many pixels, or `None` if either is empty.
pub fn fit(video: (usize, usize), layer: (u32, u32)) -> Option<Rect> {
    let (video_width, video_height) = (video.0 as f64, video.1 as f64);
    let (layer_width, layer_height) = (f64::from(layer.0), f64::from(layer.1));
    if video_width <= 0.0 || video_height <= 0.0 || layer_width <= 0.0 || layer_height <= 0.0 {
        return None;
    }
    let scale = (layer_width / video_width).min(layer_height / video_height);
    let (width, height) = (
        (video_width * scale).round(),
        (video_height * scale).round(),
    );
    Some(Rect {
        left: ((layer_width - width) / 2.0).floor(),
        top: ((layer_height - height) / 2.0).floor(),
        width,
        height,
    })
}

/// Returns a point of the host's monitor, in pixels, moved into `area`, in
/// positions: 1/256 pixel (Section 1.5.6).
pub fn confined(area: DesktopRect, x: f64, y: f64) -> (f64, f64) {
    let pixels = |position: f64| position / POSITION_UNITS;
    let left = pixels(area.x.into_inner() as f64);
    let top = pixels(area.y.into_inner() as f64);
    // The last position inside, a 1/256 pixel short of the far edges.
    let right = pixels((area.x.into_inner() + area.width.into_inner() as i64 - 1) as f64);
    let bottom = pixels((area.y.into_inner() + area.height.into_inner() as i64 - 1) as f64);
    (
        x.clamp(left, right.max(left)),
        y.clamp(top, bottom.max(top)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture fills the side that limits it and is centred on the other.
    #[test]
    fn fits_pictures() {
        let wide = fit((1920, 1080), (2000, 2000)).unwrap();
        assert_eq!((wide.width, wide.height), (2000.0, 1125.0));
        assert_eq!((wide.left, wide.top), (0.0, 437.0));
        let exact = fit((3840, 2160), (3840, 2160)).unwrap();
        assert_eq!(
            exact,
            Rect {
                left: 0.0,
                top: 0.0,
                width: 3840.0,
                height: 2160.0
            }
        );
        let tall = fit((1000, 2000), (3000, 1000)).unwrap();
        assert_eq!(
            (tall.width, tall.height, tall.left),
            (500.0, 1000.0, 1250.0)
        );
        assert_eq!(fit((0, 1080), (100, 100)), None);
        assert_eq!(fit((1920, 1080), (0, 100)), None);
    }

    /// Positions map between the layer and the picture both ways, those off
    /// the picture onto its nearest edge.
    #[test]
    fn maps_positions() {
        // A 1920 × 1080 picture shown at half size, 100 pixels down.
        let rect = fit((1920, 1080), (960, 740)).unwrap();
        assert_eq!((rect.left, rect.top), (0.0, 100.0));
        assert_eq!(rect.to_layer((1920, 1080), (960.0, 540.0)), (480.0, 370.0));
        assert_eq!(rect.to_video((1920, 1080), (480.0, 370.0)), (960.0, 540.0));
        assert_eq!(rect.to_video((1920, 1080), (-5.0, 10.0)), (0.0, 0.0));
        assert_eq!(
            rect.to_video((1920, 1080), (2000.0, 900.0)),
            (1919.0, 1079.0)
        );
        assert!(rect.contains((0.0, 100.0)));
        assert!(!rect.contains((480.0, 50.0)));
        assert!(!rect.contains((960.0, 300.0)));
    }
}
