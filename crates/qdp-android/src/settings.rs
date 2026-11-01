//! What the app connects with: the host's address and key pin, as the user
//! typed them, and the display the video is shown on.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use qdp_quic::KeyPin;
use qdp_runtime::DisplaySize;

/// The port hosts listen on unless told otherwise.
pub const DEFAULT_PORT: u16 = 4433;

/// The name the client asks the host's certificate for. Hosts are
/// identified by their key pin, so any name does.
pub const SERVER_NAME: &str = "qdp-host";

/// The hexadecimal digits of a key pin, as `qdp pin` prints it.
const PIN_HEX_LENGTH: usize = 64;

/// The smallest display side the host accepts, in pixels.
const MIN_DISPLAY_SIDE: u32 = 16;

/// The largest display side the host accepts, in pixels.
const MAX_DISPLAY_SIDE: u32 = 16_384;

/// Resolves the host's address: an IP address or a host name, with or
/// without a port, on [`DEFAULT_PORT`] when there is none.
pub fn resolve_address(text: &str) -> Result<SocketAddr, String> {
    let text = text.trim();
    if let Ok(address) = text.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let has_port = text
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok());
    let resolved = if has_port {
        text.to_socket_addrs()
    } else {
        (text, DEFAULT_PORT).to_socket_addrs()
    };
    resolved
        .map_err(|error| format!("cannot resolve {text}: {error}"))?
        .next()
        .ok_or_else(|| format!("{text} has no address"))
}

/// Parses a key pin: 64 hexadecimal digits, with `:` and spaces ignored.
pub fn parse_pin(text: &str) -> Result<KeyPin, String> {
    let invalid = || "a pin is 64 hexadecimal digits, as `qdp pin` prints".to_owned();
    let digits: String = text
        .chars()
        .filter(|character| *character != ':' && !character.is_whitespace())
        .collect();
    if digits.len() != PIN_HEX_LENGTH || !digits.is_ascii() {
        return Err(invalid());
    }
    let mut pin = [0; PIN_HEX_LENGTH / 2];
    for (index, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok(KeyPin(pin))
}

/// Returns `pin` as 64 lower-case hexadecimal digits, as `qdp pin` prints it.
pub fn format_pin(pin: KeyPin) -> String {
    pin.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns the display a session asks the host for: `width × height`
/// pixels refreshing at `refresh_millihertz`, each side kept to what hosts
/// accept.
pub fn display_size(width: u32, height: u32, refresh_millihertz: u32) -> DisplaySize {
    let side = |pixels: u32| pixels.clamp(MIN_DISPLAY_SIDE, MAX_DISPLAY_SIDE);
    DisplaySize {
        width: side(width),
        height: side(height),
        refresh_millihertz,
        scale_percent: DisplaySize::UNSCALED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Addresses take the default port unless they name one.
    #[test]
    fn resolves_addresses() {
        assert_eq!(
            resolve_address("192.168.1.44").unwrap(),
            "192.168.1.44:4433".parse().unwrap()
        );
        assert_eq!(
            resolve_address(" 192.168.1.44:5000 ").unwrap(),
            "192.168.1.44:5000".parse().unwrap()
        );
        assert_eq!(
            resolve_address("::1").unwrap(),
            "[::1]:4433".parse().unwrap()
        );
    }

    /// Pins are 64 hexadecimal digits, however they are grouped.
    #[test]
    fn parses_pins() {
        let digits = "c86778b4a2712823d32658299a55d5f5a9fdfc60adf310e849a4c59029ca850a";
        let pin = parse_pin(digits).unwrap();
        assert_eq!(pin.0[0], 0xc8);
        assert_eq!(pin.0[31], 0x0a);
        let grouped: String = digits
            .as_bytes()
            .chunks(4)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(parse_pin(&grouped).unwrap(), pin);
        assert_eq!(format_pin(pin), digits);
        assert!(parse_pin("c867").is_err());
        assert!(parse_pin(&"zz".repeat(32)).is_err());
    }

    /// Display sides are kept to what hosts accept.
    #[test]
    fn keeps_display_sizes_in_range() {
        let display = display_size(1920, 1080, 120_000);
        assert_eq!((display.width, display.height), (1920, 1080));
        assert_eq!(display.refresh_millihertz, 120_000);
        let tiny = display_size(4, 100_000, 60_000);
        assert_eq!(
            (tiny.width, tiny.height),
            (MIN_DISPLAY_SIDE, MAX_DISPLAY_SIDE)
        );
    }
}
