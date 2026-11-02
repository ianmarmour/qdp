//! The QR code that adds a gateway to the user's authenticator app, as a
//! picture: black modules on white, with the quiet zone readers need, each
//! module a square of pixels, so that it stays sharp unscaled.

use gtk::prelude::*;
use gtk::{gdk, glib};
use qrcode::{Color, EcLevel, QrCode};

/// The light modules around the code that readers need to find it.
const QUIET_ZONE: usize = 4;

/// The pixels on a module's side: small enough for the dialog that shows the
/// code to fit a small window, large enough for a phone's camera.
const MODULE_PIXELS: usize = 4;

/// A dark pixel.
const DARK: u8 = 0;

/// A light pixel.
const LIGHT: u8 = 255;

/// Returns the code of `text` as a picture, or `None` if it is too long
/// for a QR code.
pub fn texture(text: &str) -> Option<gdk::Texture> {
    let (side, drawn) = pixels(text)?;
    let side_i32 = i32::try_from(side).ok()?;
    let texture = gdk::MemoryTexture::new(
        side_i32,
        side_i32,
        gdk::MemoryFormat::G8,
        &glib::Bytes::from_owned(drawn),
        side,
    );
    Some(texture.upcast())
}

/// Returns the pixels on the picture's side, and its pixels, a byte each,
/// row by row.
fn pixels(text: &str) -> Option<(usize, Vec<u8>)> {
    let code = QrCode::with_error_correction_level(text, EcLevel::M).ok()?;
    let modules = code.width();
    let side = (modules + 2 * QUIET_ZONE) * MODULE_PIXELS;
    let mut pixels = vec![LIGHT; side * side];
    for (index, color) in code.to_colors().into_iter().enumerate() {
        if color != Color::Dark {
            continue;
        }
        let x = (index % modules + QUIET_ZONE) * MODULE_PIXELS;
        let y = (index / modules + QUIET_ZONE) * MODULE_PIXELS;
        for row in y..y + MODULE_PIXELS {
            pixels[row * side + x..row * side + x + MODULE_PIXELS].fill(DARK);
        }
    }
    Some((side, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_modules_as_squares_inside_the_quiet_zone() {
        let (side, drawn) = pixels("otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(drawn.len(), side * side);
        let at = |x: usize, y: usize| drawn[y * side + x];
        let zone = QUIET_ZONE * MODULE_PIXELS;
        // The quiet zone is light; the finder pattern's corner, after it,
        // is dark to the end of its module.
        assert_eq!(at(zone - 1, zone), LIGHT);
        assert_eq!(at(zone, zone), DARK);
        assert_eq!(at(zone + MODULE_PIXELS - 1, zone + MODULE_PIXELS - 1), DARK);
        assert!(pixels("x".repeat(8000).as_str()).is_none());
    }
}
