//! Where the picture sits in the app's view: centred, with its aspect ratio
//! kept, as the app also computes to map touches and the pointer to the
//! picture.

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
}
