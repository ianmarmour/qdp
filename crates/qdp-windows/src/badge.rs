//! The stream window's badge: a few lines over the picture's top-left
//! corner saying how the stream goes, such as that it is reconnecting, or
//! its statistics, which no title bar shows in full screen.
//!
//! The lines are drawn with GDI, white on black, into a bitmap whose
//! brightness becomes the text's coverage, over a translucent dark panel.
//! GDI is a C interface, so this module uses `unsafe`.

#![allow(unsafe_code)]

use qdp_desktop::Overlay;
use windows::Win32::Foundation::{COLORREF, SIZE};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, FF_DONTCARE, FW_SEMIBOLD, GdiFlush, GetTextExtentPoint32W, HGDIOBJ,
    OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::core::w;

/// The text's height at 100 % scale, in pixels.
const TEXT_PIXELS: f64 = 15.0;

/// The space around the text, inside the panel, at 100 % scale.
const PADDING_PIXELS: f64 = 10.0;

/// The panel's distance from the window's corner, at 100 % scale.
pub(crate) const MARGIN_PIXELS: f64 = 16.0;

/// The panel's corner radius, at 100 % scale.
const RADIUS_PIXELS: f64 = 8.0;

/// The panel's opacity, out of 255.
const PANEL_ALPHA: f64 = 150.0;

/// White, as GDI names colors.
const WHITE: COLORREF = COLORREF(0x00ff_ffff);

/// The bytes of a pixel.
const PIXEL_BYTES: usize = 4;

/// Returns `lines` drawn as a badge at `scale` times their size at 100 %,
/// its top-left corner at the window's margin, or `None` if there are no
/// lines or GDI cannot draw them.
pub(crate) fn render(lines: &[String], scale: f64) -> Option<Overlay> {
    if lines.is_empty() {
        return None;
    }
    let pixels = |length: f64| (length * scale).round().max(1.0) as i32;
    let padding = pixels(PADDING_PIXELS);
    let lines: Vec<Vec<u16>> = lines
        .iter()
        .map(|line| line.encode_utf16().collect())
        .collect();
    // SAFETY: each object made is selected only into the DC made here, and
    // deleted, once deselected, before returning; the bitmap's bits are
    // read only while it exists, after GDI finished drawing into it.
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let font = CreateFontW(
            -pixels(TEXT_PIXELS),
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            FF_DONTCARE.0 as u32,
            w!("Segoe UI"),
        );
        let previous_font = SelectObject(dc, HGDIOBJ(font.0));
        let sizes: Vec<SIZE> = lines
            .iter()
            .map(|line| {
                let mut size = SIZE::default();
                let _ = GetTextExtentPoint32W(dc, line, &mut size);
                size
            })
            .collect();
        let line_height = sizes.iter().map(|size| size.cy).max().unwrap_or(0);
        let width = sizes.iter().map(|size| size.cx).max().unwrap_or(0) + 2 * padding;
        let height = line_height * lines.len() as i32 + 2 * padding;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows top first.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        let drawn = match bitmap {
            Ok(bitmap) if !bits.is_null() => {
                let previous_bitmap = SelectObject(dc, HGDIOBJ(bitmap.0));
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, WHITE);
                for (index, line) in lines.iter().enumerate() {
                    let _ = TextOutW(dc, padding, padding + line_height * index as i32, line);
                }
                let _ = GdiFlush();
                let length = width as usize * height as usize * PIXEL_BYTES;
                let coverage = std::slice::from_raw_parts(bits.cast::<u8>(), length);
                let rgba = compose(coverage, (width as usize, height as usize), scale);
                SelectObject(dc, previous_bitmap);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                Some(rgba)
            }
            _ => None,
        };
        SelectObject(dc, previous_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteDC(dc);
        let margin = f64::from(pixels(MARGIN_PIXELS));
        drawn.map(|rgba| Overlay {
            width: width as usize,
            height: height as usize,
            rgba: rgba.into(),
            left: margin,
            top: margin,
        })
    }
}

/// Returns the badge's RGBA pixels, with straight alpha, from GDI's drawing
/// of white text on black, `size` pixels, BGRA: the text white where it
/// covers, over a translucent dark panel with corners rounded at `scale`.
fn compose(coverage: &[u8], (width, height): (usize, usize), scale: f64) -> Vec<u8> {
    let radius = RADIUS_PIXELS * scale;
    let mut rgba = Vec::with_capacity(width * height * PIXEL_BYTES);
    for y in 0..height {
        for x in 0..width {
            let pixel = &coverage[(y * width + x) * PIXEL_BYTES..][..3];
            let text = f64::from(pixel[0].max(pixel[1]).max(pixel[2])) / 255.0;
            let panel = PANEL_ALPHA / 255.0 * corner_coverage((x, y), (width, height), radius);
            // The text over the panel: white where it covers, black below.
            let alpha = text + panel * (1.0 - text);
            let white = if alpha > 0.0 { text / alpha } else { 0.0 };
            let channel = (white * 255.0).round() as u8;
            rgba.extend_from_slice(&[channel, channel, channel, (alpha * 255.0).round() as u8]);
        }
    }
    rgba
}

/// Returns how much of pixel `(x, y)` a `size`-pixel rectangle with corners
/// rounded by `radius` covers, from 0 to 1, smoothing the corners' edges.
fn corner_coverage((x, y): (usize, usize), (width, height): (usize, usize), radius: f64) -> f64 {
    let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
    let (width, height) = (width as f64, height as f64);
    let corner_x = x.clamp(radius, (width - radius).max(radius));
    let corner_y = y.clamp(radius, (height - radius).max(radius));
    let distance = ((x - corner_x).powi(2) + (y - corner_y).powi(2)).sqrt();
    (radius + 0.5 - distance).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text covers the panel white; the panel alone is translucent black,
    /// and its corners fade out.
    #[test]
    fn composes_text_over_the_panel() {
        let size = (40, 20);
        let mut coverage = vec![0; size.0 * size.1 * PIXEL_BYTES];
        // Full coverage in the middle.
        let middle = (10 * size.0 + 20) * PIXEL_BYTES;
        coverage[middle..middle + 3].copy_from_slice(&[255, 255, 255]);
        let rgba = compose(&coverage, size, 1.0);
        assert_eq!(&rgba[middle..middle + 4], &[255, 255, 255, 255]);
        let panel = (10 * size.0 + 2) * PIXEL_BYTES;
        assert_eq!(&rgba[panel..panel + 4], &[0, 0, 0, 150]);
        assert_eq!(rgba[3], 0, "the corner is clear");
    }

    /// The rounded rectangle covers its inside fully and its corners not at
    /// all.
    #[test]
    fn rounds_corners() {
        assert_eq!(corner_coverage((20, 10), (40, 20), 8.0), 1.0);
        assert_eq!(corner_coverage((0, 0), (40, 20), 8.0), 0.0);
        assert_eq!(corner_coverage((8, 0), (40, 20), 8.0), 1.0);
    }

    /// Nothing is drawn without lines.
    #[test]
    fn draws_nothing_without_lines() {
        assert!(render(&[], 1.0).is_none());
    }
}
