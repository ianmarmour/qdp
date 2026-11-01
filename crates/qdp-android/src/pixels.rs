//! Copying the software decoder's pictures into the buffers of the app's
//! surface: in YV12, Android's own YUV layout, or in RGBX for a surface that
//! does not take it.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use qdp_codec::Yuv420Frame;

/// The alignment of the rows of a YV12 buffer's chroma planes, in bytes.
const YV12_CHROMA_ALIGNMENT: usize = 16;

/// The bytes of an RGBX pixel.
pub const RGBX_BYTES: usize = 4;

/// The fractional bits of the YUV to RGB coefficients.
const FRACTION_BITS: u32 = 8;

/// BT.709's limited-range YUV to RGB coefficients, in 8-bit fixed point:
/// luma, red from V, green from U, green from V, and blue from U.
const COEFFICIENTS: [i32; 5] = [298, 459, -55, -136, 541];

/// The luma value of black in limited range.
const LIMITED_BLACK: i32 = 16;

/// The chroma value of a neutral colour.
const NEUTRAL_CHROMA: i32 = 128;

/// Returns the bytes of a YV12 buffer of `height` rows of `stride` pixels.
pub fn yv12_size(stride: usize, height: usize) -> usize {
    let chroma_stride = stride.div_ceil(2).next_multiple_of(YV12_CHROMA_ALIGNMENT);
    stride * height + 2 * chroma_stride * height.div_ceil(2)
}

/// Copies the top-left `width × height` pixels of `frame` into `buffer`, a
/// YV12 buffer whose rows are `stride` pixels: its luma plane, then its V
/// plane, then its U plane, each chroma row aligned to 16 bytes.
pub fn fill_yv12(
    buffer: &mut [u8],
    stride: usize,
    frame: &Yuv420Frame,
    width: usize,
    height: usize,
) {
    let chroma_stride = stride.div_ceil(2).next_multiple_of(YV12_CHROMA_ALIGNMENT);
    let chroma_height = height.div_ceil(2);
    let (luma, chroma) = buffer.split_at_mut(stride * height);
    let (v, u) = chroma.split_at_mut(chroma_stride * chroma_height);
    let chroma_width = width.div_ceil(2);
    copy_plane(luma, stride, frame.y(), frame.width(), width, height);
    copy_plane(
        v,
        chroma_stride,
        frame.v(),
        frame.chroma_width(),
        chroma_width,
        chroma_height,
    );
    copy_plane(
        u,
        chroma_stride,
        frame.u(),
        frame.chroma_width(),
        chroma_width,
        chroma_height,
    );
}

/// Copies `rows` rows of `width` bytes from `source`, whose rows are
/// `source_stride` bytes, into `target`, whose rows are `target_stride`.
fn copy_plane(
    target: &mut [u8],
    target_stride: usize,
    source: &[u8],
    source_stride: usize,
    width: usize,
    rows: usize,
) {
    for (target_row, source_row) in target
        .chunks_mut(target_stride)
        .zip(source.chunks(source_stride))
        .take(rows)
    {
        let width = width.min(target_row.len()).min(source_row.len());
        target_row[..width].copy_from_slice(&source_row[..width]);
    }
}

/// Converts the top-left `width × height` pixels of `frame` into `buffer`,
/// an RGBX buffer whose rows are `stride` pixels.
pub fn fill_rgbx(
    buffer: &mut [u8],
    stride: usize,
    frame: &Yuv420Frame,
    width: usize,
    height: usize,
) {
    let [luma, red_v, green_u, green_v, blue_u] = COEFFICIENTS;
    let half = 1 << (FRACTION_BITS - 1);
    let clamp = |value: i32| ((value + half) >> FRACTION_BITS).clamp(0, 255) as u8;
    let chroma_width = frame.chroma_width();
    for (row, target) in buffer
        .chunks_mut(stride * RGBX_BYTES)
        .take(height)
        .enumerate()
    {
        let y_row = &frame.y()[row * frame.width()..][..width];
        let chroma_row = (row / 2) * chroma_width;
        for (column, pixel) in target[..width * RGBX_BYTES]
            .chunks_exact_mut(RGBX_BYTES)
            .enumerate()
        {
            let y = luma * (i32::from(y_row[column]) - LIMITED_BLACK);
            let u = i32::from(frame.u()[chroma_row + column / 2]) - NEUTRAL_CHROMA;
            let v = i32::from(frame.v()[chroma_row + column / 2]) - NEUTRAL_CHROMA;
            pixel.copy_from_slice(&[
                clamp(y + red_v * v),
                clamp(y + green_u * u + green_v * v),
                clamp(y + blue_u * u),
                u8::MAX,
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a 4 × 2 frame whose luma counts up from 16, with U 100 and V
    /// 200.
    fn frame() -> Yuv420Frame {
        Yuv420Frame::from_planes(4, 2, (16..24).collect(), vec![100; 2], vec![200; 2]).unwrap()
    }

    /// The planes land at YV12's offsets: V before U, chroma rows aligned.
    #[test]
    fn fills_yv12_buffers() {
        let stride = 16;
        let mut buffer = vec![0; yv12_size(stride, 2)];
        assert_eq!(buffer.len(), 16 * 2 + 2 * 16);
        fill_yv12(&mut buffer, stride, &frame(), 4, 2);
        assert_eq!(&buffer[..4], &[16, 17, 18, 19]);
        assert_eq!(&buffer[16..20], &[20, 21, 22, 23]);
        assert_eq!(&buffer[32..34], &[200, 200]);
        assert_eq!(&buffer[48..50], &[100, 100]);
        assert_eq!(buffer[34], 0);
    }

    /// Limited-range black is black, and rows follow the stride.
    #[test]
    fn fills_rgbx_buffers() {
        let black = Yuv420Frame::from_planes(2, 2, vec![16; 4], vec![128], vec![128]).unwrap();
        let stride = 3;
        let mut buffer = vec![7; stride * 2 * RGBX_BYTES];
        fill_rgbx(&mut buffer, stride, &black, 2, 2);
        assert_eq!(&buffer[..8], &[0, 0, 0, 255, 0, 0, 0, 255]);
        assert_eq!(&buffer[8..12], &[7; 4]);
        assert_eq!(&buffer[12..16], &[0, 0, 0, 255]);
        let white = Yuv420Frame::from_planes(2, 2, vec![235; 4], vec![128], vec![128]).unwrap();
        fill_rgbx(&mut buffer, stride, &white, 2, 2);
        assert_eq!(&buffer[..4], &[255, 255, 255, 255]);
    }
}
