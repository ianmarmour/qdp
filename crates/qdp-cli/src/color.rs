//! Conversion of decoded frames to RGB for display.

use qdp_codec::Yuv420Frame;

/// The number of fractional bits of the fixed-point conversion coefficients.
const FRACTION_BITS: u32 = 16;

/// The luma of black in limited range.
const LUMA_OFFSET: i32 = 16;

/// The chroma of a neutral colour.
const CHROMA_OFFSET: i32 = 128;

/// 255 / 219: scales limited-range luma to full range.
const LUMA_SCALE: i32 = 76_309;

/// The red contribution of Cr in BT.709, scaled from limited range:
/// 2 × (1 − Kr) × 255 / 224.
const RED_FROM_CR: i32 = 117_489;

/// The green contribution of Cb in BT.709, scaled from limited range.
const GREEN_FROM_CB: i32 = 13_975;

/// The green contribution of Cr in BT.709, scaled from limited range.
const GREEN_FROM_CR: i32 = 34_925;

/// The blue contribution of Cb in BT.709, scaled from limited range:
/// 2 × (1 − Kb) × 255 / 224.
const BLUE_FROM_CB: i32 = 138_438;

/// Half of one unit of the fixed-point result, for rounding.
const ROUNDING: i32 = 1 << (FRACTION_BITS - 1);

/// An RGB image with one `0x00RRGGBB` pixel per element, as `softbuffer`
/// expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbFrame {
    /// The width in pixels.
    pub width: usize,
    /// The height in pixels.
    pub height: usize,
    /// The pixels, row by row.
    pub pixels: Vec<u32>,
}

/// Returns one fixed-point colour component as a byte.
fn component(value: i32) -> u32 {
    ((value + ROUNDING) >> FRACTION_BITS).clamp(0, 255) as u32
}

/// Converts a limited-range BT.709 YUV 4:2:0 frame, the format QDP's standard
/// dynamic range streams use, to RGB.
pub fn yuv420_to_rgb(frame: &Yuv420Frame) -> RgbFrame {
    let (width, height) = (frame.width(), frame.height());
    let chroma_width = frame.chroma_width();
    let mut pixels = vec![0; width * height];
    for (row, output_row) in pixels.chunks_exact_mut(width).enumerate() {
        let luma_row = &frame.y()[row * width..(row + 1) * width];
        let chroma_offset = (row / 2) * chroma_width;
        let u_row = &frame.u()[chroma_offset..chroma_offset + chroma_width];
        let v_row = &frame.v()[chroma_offset..chroma_offset + chroma_width];
        for (column, pixel) in output_row.iter_mut().enumerate() {
            let luma = (i32::from(luma_row[column]) - LUMA_OFFSET) * LUMA_SCALE;
            let cb = i32::from(u_row[column / 2]) - CHROMA_OFFSET;
            let cr = i32::from(v_row[column / 2]) - CHROMA_OFFSET;
            let red = component(luma + RED_FROM_CR * cr);
            let green = component(luma - GREEN_FROM_CB * cb - GREEN_FROM_CR * cr);
            let blue = component(luma + BLUE_FROM_CB * cb);
            *pixel = (red << 16) | (green << 8) | blue;
        }
    }
    RgbFrame {
        width,
        height,
        pixels,
    }
}

/// Where a scaled picture sits in a window: centred, with its aspect ratio
/// kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The picture's left edge in the window, in pixels.
    pub left: usize,
    /// The picture's top edge.
    pub top: usize,
    /// The picture's width in the window.
    pub width: usize,
    /// The picture's height in the window.
    pub height: usize,
}

impl Placement {
    /// Returns where a `frame_width × frame_height` picture is drawn in a
    /// `target_width × target_height` window, or `None` if either is empty.
    pub fn fit(
        frame_width: usize,
        frame_height: usize,
        target_width: usize,
        target_height: usize,
    ) -> Option<Self> {
        if frame_width == 0 || frame_height == 0 || target_width == 0 || target_height == 0 {
            return None;
        }
        // Fit by the dimension that limits: compare the aspect ratios by
        // cross-multiplying.
        let (width, height) = if target_width * frame_height <= target_height * frame_width {
            (
                target_width,
                (frame_height * target_width / frame_width).max(1),
            )
        } else {
            (
                (frame_width * target_height / frame_height).max(1),
                target_height,
            )
        };
        Some(Self {
            left: (target_width - width) / 2,
            top: (target_height - height) / 2,
            width,
            height,
        })
    }

    /// Converts a window position to a position in the picture's pixels,
    /// clamped to the picture.
    pub fn to_frame(self, x: f64, y: f64, frame_width: usize, frame_height: usize) -> (f64, f64) {
        let scale = |position: f64, start: usize, size: usize, frame: usize| {
            let frame = frame as f64;
            ((position - start as f64) * frame / size as f64).clamp(0.0, (frame - 1.0).max(0.0))
        };
        (
            scale(x, self.left, self.width, frame_width),
            scale(y, self.top, self.height, frame_height),
        )
    }
}

/// Draws `frame` into a `target_width × target_height` buffer, scaled to fit
/// with its aspect ratio kept and black bars around it.
pub fn scale_into(frame: &RgbFrame, target: &mut [u32], target_width: usize, target_height: usize) {
    target.fill(0);
    let Some(placement) = Placement::fit(frame.width, frame.height, target_width, target_height)
    else {
        return;
    };
    let source_columns: Vec<usize> = (0..placement.width)
        .map(|column| column * frame.width / placement.width)
        .collect();
    for row in 0..placement.height {
        let source_row = row * frame.height / placement.height;
        let source = &frame.pixels[source_row * frame.width..(source_row + 1) * frame.width];
        let start = (placement.top + row) * target_width + placement.left;
        let destination = &mut target[start..start + placement.width];
        for (pixel, &column) in destination.iter_mut().zip(&source_columns) {
            *pixel = source[column];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a 2 × 2 frame of one colour.
    fn solid(y: u8, u: u8, v: u8) -> Yuv420Frame {
        Yuv420Frame::from_planes(2, 2, vec![y; 4], vec![u], vec![v]).unwrap()
    }

    /// Splits a pixel into its red, green and blue components.
    fn rgb(pixel: u32) -> (u32, u32, u32) {
        ((pixel >> 16) & 0xff, (pixel >> 8) & 0xff, pixel & 0xff)
    }

    /// Limited-range black, white and the BT.709 primaries convert correctly.
    #[test]
    fn converts_reference_colours() {
        assert_eq!(
            rgb(yuv420_to_rgb(&solid(16, 128, 128)).pixels[0]),
            (0, 0, 0)
        );
        assert_eq!(
            rgb(yuv420_to_rgb(&solid(235, 128, 128)).pixels[0]),
            (255, 255, 255)
        );
        let near = |actual: u32, expected: u32| actual.abs_diff(expected) <= 2;
        // BT.709 limited-range red, green and blue.
        for ((y, u, v), (r, g, b)) in [
            ((63, 102, 240), (255, 0, 0)),
            ((173, 42, 26), (0, 255, 0)),
            ((32, 240, 118), (0, 0, 255)),
        ] {
            let (red, green, blue) = rgb(yuv420_to_rgb(&solid(y, u, v)).pixels[0]);
            assert!(
                near(red, r) && near(green, g) && near(blue, b),
                "{red} {green} {blue}"
            );
        }
    }

    /// Scaling keeps the aspect ratio and centres the image.
    #[test]
    fn scales_with_letterboxing() {
        let frame = RgbFrame {
            width: 2,
            height: 1,
            pixels: vec![0x00ff_0000, 0x0000_00ff],
        };
        let mut target = vec![1; 4 * 4];
        scale_into(&frame, &mut target, 4, 4);
        // A 2:1 image in a square: two rows of picture between bars.
        assert_eq!(&target[0..4], [0; 4]);
        assert_eq!(
            &target[4..8],
            [0x00ff_0000, 0x00ff_0000, 0x0000_00ff, 0x0000_00ff]
        );
        assert_eq!(
            &target[8..12],
            [0x00ff_0000, 0x00ff_0000, 0x0000_00ff, 0x0000_00ff]
        );
        assert_eq!(&target[12..16], [0; 4]);
    }

    /// Window positions map back to picture pixels, clamped to the picture.
    #[test]
    fn maps_window_positions() {
        // A 1280 × 720 picture in a 1000 × 1000 window: 1000 × 562, 219 down.
        let placement = Placement::fit(1280, 720, 1000, 1000).unwrap();
        assert_eq!(
            (
                placement.left,
                placement.top,
                placement.width,
                placement.height
            ),
            (0, 219, 1000, 562)
        );
        let (x, y) = placement.to_frame(500.0, 219.0 + 281.0, 1280, 720);
        assert!(
            (x - 640.0).abs() < 0.01 && (y - 360.0).abs() < 1.0,
            "{x} {y}"
        );
        assert_eq!(placement.to_frame(-5.0, 2000.0, 1280, 720), (0.0, 719.0));
        assert_eq!(Placement::fit(0, 10, 10, 10), None);
    }

    /// Empty sizes leave a black buffer and do not panic.
    #[test]
    fn handles_empty_sizes() {
        let frame = RgbFrame {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        let mut target = vec![7; 4];
        scale_into(&frame, &mut target, 2, 2);
        assert_eq!(target, [0; 4]);
    }
}
