//! QR codes, as the clients show a gateway's TOTP enrollment for the user's
//! authenticator app to scan (Section 4.4.10): the code's modules, for each
//! client to draw with its own toolkit.

use qrcode::{Color, EcLevel, QrCode};

/// The light modules around a QR code that readers need to find it.
pub const QUIET_ZONE: usize = 4;

/// A QR code's modules, row by row, without the quiet zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrModules {
    /// The modules on a side.
    size: usize,
    /// Whether each module is dark, row by row.
    dark: Vec<bool>,
}

impl QrModules {
    /// Encodes `text`, with the error correction that keeps codes on a
    /// screen readable, or returns `None` if it is too long for a QR code.
    pub fn encode(text: &str) -> Option<Self> {
        let code = QrCode::with_error_correction_level(text, EcLevel::M).ok()?;
        Some(Self {
            size: code.width(),
            dark: code
                .to_colors()
                .into_iter()
                .map(|color| color == Color::Dark)
                .collect(),
        })
    }

    /// Returns the modules on a side.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns whether the module at column `x` and row `y` is dark; those
    /// outside the code, in the quiet zone, are light.
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.dark[y * self.size + x]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_an_enrollment() {
        let uri = "otpauth://totp/Example%20Corp:alice?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP\
                   &issuer=Example%20Corp&algorithm=SHA1&digits=6&period=30";
        let code = QrModules::encode(uri).unwrap();
        // At least the smallest code's 21 modules a side, with the dark
        // corners of its finder patterns.
        assert!(code.size() >= 21);
        assert!(code.is_dark(0, 0));
        assert!(code.is_dark(code.size() - 1, 0));
        assert!(!code.is_dark(code.size(), 0));
        assert!(QrModules::encode(&"x".repeat(8000)).is_none());
    }
}
