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

/// Draws `frame` into a `target_width × target_height` buffer, scaled to fit
/// with its aspect ratio kept and black bars around it.
pub fn scale_into(frame: &RgbFrame, target: &mut [u32], target_width: usize, target_height: usize) {
    target.fill(0);
    if frame.width == 0 || frame.height == 0 || target_width == 0 || target_height == 0 {
        return;
    }
    // Fit by the dimension that limits: compare the aspect ratios by
    // cross-multiplying.
    let (scaled_width, scaled_height) =
        if target_width * frame.height <= target_height * frame.width {
            (
                target_width,
                (frame.height * target_width / frame.width).max(1),
            )
        } else {
            (
                (frame.width * target_height / frame.height).max(1),
                target_height,
            )
        };
    let left = (target_width - scaled_width) / 2;
    let top = (target_height - scaled_height) / 2;
    let source_columns: Vec<usize> = (0..scaled_width)
        .map(|column| column * frame.width / scaled_width)
        .collect();
    for row in 0..scaled_height {
        let source_row = row * frame.height / scaled_height;
        let source = &frame.pixels[source_row * frame.width..(source_row + 1) * frame.width];
        let start = (top + row) * target_width + left;
        let destination = &mut target[start..start + scaled_width];
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
