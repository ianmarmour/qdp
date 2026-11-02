//! What the app connects with: the host's address and key pin, as the user
//! typed them, and the display the video is shown on.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

use qdp_quic::KeyPin;
use qdp_runtime::DisplaySize;

/// The port hosts listen on unless told otherwise.
pub const DEFAULT_PORT: u16 = 4433;

/// The name the client asks the host's certificate for. Hosts are
/// identified by their key pin, so any name does.
pub const SERVER_NAME: &str = "qdp-host";

/// The name sent to a gateway given by its IP address.
const GATEWAY_ADDRESS_NAME: &str = "qdp-gateway";

/// The hexadecimal digits of a key pin, as `qdp pin` prints it.
const PIN_HEX_LENGTH: usize = 64;

/// The smallest display side the host accepts, in pixels.
const MIN_DISPLAY_SIDE: u32 = 16;

/// The largest display side the host accepts, in pixels.
const MAX_DISPLAY_SIDE: u32 = 16_384;

/// Resolves the host's address: an IP address or a host name, with or
/// without a port, on [`DEFAULT_PORT`] when there is none.
///
/// An IP address may name the interface it was found on after a `%`, as
/// Apple's Network framework writes the addresses Bonjour finds, such as
/// `192.168.1.44%en0`: IPv4 addresses need none, so it is dropped, and the
/// system resolves the interfaces of IPv6 ones, such as `fe80::1%en0`.
pub fn resolve_address(text: &str) -> Result<SocketAddr, String> {
    resolve_address_on(text, DEFAULT_PORT)
}

/// Resolves an address as [`resolve_address`] does, on `default_port` when
/// it names none, as a gateway's is on 443.
pub fn resolve_address_on(text: &str, default_port: u16) -> Result<SocketAddr, String> {
    let text = text.trim();
    if let Ok(address) = text.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port));
    }
    let (host, port) = host_and_port(text);
    let port = port.unwrap_or(default_port);
    let ipv4_with_interface = host
        .split_once('%')
        .and_then(|(address, _interface)| address.parse::<Ipv4Addr>().ok());
    if let Some(ip) = ipv4_with_interface {
        return Ok(SocketAddr::new(IpAddr::V4(ip), port));
    }
    (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("cannot resolve {text}: {error}"))?
        .next()
        .ok_or_else(|| format!("{text} has no address"))
}

/// Returns the name of the host an address names, without its port, as a
/// gateway's certificate names it; the generic `qdp-gateway` for an IP
/// address, which a pinned gateway's certificate need not name.
pub fn gateway_server_name(text: &str) -> String {
    let (host, _) = host_and_port(text.trim());
    if host.parse::<IpAddr>().is_ok() || host.is_empty() {
        GATEWAY_ADDRESS_NAME.to_owned()
    } else {
        host.to_owned()
    }
}

/// Splits an address into its host, without the brackets around an IPv6
/// address, and its port, if it names one.
fn host_and_port(text: &str) -> (&str, Option<u16>) {
    if let Some((host, rest)) = text
        .strip_prefix('[')
        .and_then(|bracketed| bracketed.split_once(']'))
    {
        return (
            host,
            rest.strip_prefix(':').and_then(|port| port.parse().ok()),
        );
    }
    match text.rsplit_once(':') {
        // A host name or IPv4 address has no colon of its own.
        Some((host, port)) if !host.contains(':') => match port.parse() {
            Ok(port) => (host, Some(port)),
            Err(_) => (text, None),
        },
        _ => (text, None),
    }
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

    /// The interface Apple's Network framework writes after the IPv4
    /// addresses Bonjour finds is dropped.
    #[test]
    fn drops_interfaces_of_ipv4_addresses() {
        assert_eq!(
            resolve_address("192.168.1.44%en0:4433").unwrap(),
            "192.168.1.44:4433".parse().unwrap()
        );
        assert_eq!(
            resolve_address("192.168.1.44%en0").unwrap(),
            "192.168.1.44:4433".parse().unwrap()
        );
    }

    /// Gateways are named as their certificates name them, and found on 443.
    #[test]
    fn names_gateways() {
        assert_eq!(
            gateway_server_name("gw.corp.example.com"),
            "gw.corp.example.com"
        );
        assert_eq!(
            gateway_server_name("gw.corp.example.com:8443"),
            "gw.corp.example.com"
        );
        assert_eq!(gateway_server_name("192.0.2.9:443"), GATEWAY_ADDRESS_NAME);
        assert_eq!(gateway_server_name("[2001:db8::9]"), GATEWAY_ADDRESS_NAME);
        assert_eq!(
            resolve_address_on("192.0.2.9", 443).unwrap(),
            "192.0.2.9:443".parse().unwrap()
        );
    }

    /// Hosts and ports are told apart, with IPv6 addresses in brackets.
    #[test]
    fn splits_hosts_and_ports() {
        assert_eq!(
            host_and_port("my-pc.local:5000"),
            ("my-pc.local", Some(5000))
        );
        assert_eq!(host_and_port("my-pc.local"), ("my-pc.local", None));
        assert_eq!(
            host_and_port("[fe80::1%en0]:4433"),
            ("fe80::1%en0", Some(4433))
        );
        assert_eq!(host_and_port("[fe80::1%en0]"), ("fe80::1%en0", None));
        assert_eq!(host_and_port("fe80::1%en0"), ("fe80::1%en0", None));
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
