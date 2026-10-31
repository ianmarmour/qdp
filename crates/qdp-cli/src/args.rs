//! Command-line arguments.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;

use qdp_codec::CodecPreference;
use qdp_quic::KeyPin;

/// The usage text printed by `qdp help` and on errors.
pub const USAGE: &str = "\
Usage:
  qdp serve [options]          Serve this machine's desktop
  qdp connect HOST [options]   Connect to a host, by address or name,
                               with an optional :PORT (default 4433)
  qdp pin [--identity DIR]     Print this host's key pin for clients
  qdp help                     Show this help

Serve options:
  --listen ADDRESS     Address to listen on (default 0.0.0.0:4433)
  --identity DIR       Directory holding the host's key (default ./qdp-identity)
  --token TOKEN        Accept clients presenting TOKEN (repeatable)
  --user NAME          Accept NAME with the password in QDP_PASSWORD
  --source KIND        auto, screen or test-pattern (default auto: the screen,
                       or the test pattern if it cannot be captured)
  --size WIDTHxHEIGHT  Size of the test pattern (default 1280x720)
  --fps N              Frames per second (default 60)
  --encoder KIND       auto, hardware or software (default auto: the GPU's
                       encoder when it has one, otherwise software)
  --speed N            Software encoder speed preset, 0 to 10 (default 10)
  --quantizer N        Encoder quantizer, 1 to 255 (default 100)

Connect options:
  --pin HEX            The host's key pin, from `qdp pin` (required)
  --token TOKEN        Authenticate with TOKEN
  --user NAME          Authenticate as NAME with the password in QDP_PASSWORD
  --server-name NAME   Name sent in the TLS handshake (default qdp-host)
  --frames N           Disconnect after N frames
  --output FILE        Write the decoded video to FILE as YUV4MPEG2
  --headless           Open no window; only log statistics
  --decoder KIND       auto, hardware or software (default auto: the GPU's
                       decoder when it has one, otherwise software)

Set QDP_LOG to error, warn, info, debug or trace to change the log level.";

/// The port QDP listens on by default.
const DEFAULT_PORT: u16 = 4433;

/// The default test pattern width.
const DEFAULT_WIDTH: usize = 1280;

/// The default test pattern height.
const DEFAULT_HEIGHT: usize = 720;

/// The default frame rate.
const DEFAULT_FPS: u32 = 60;

/// The default encoder speed preset: the fastest.
const DEFAULT_SPEED: u8 = 10;

/// The default encoder quantizer.
const DEFAULT_QUANTIZER: u8 = 100;

/// The default directory of the host's identity.
const DEFAULT_IDENTITY_DIR: &str = "qdp-identity";

/// The default server name sent in the TLS handshake.
pub const DEFAULT_SERVER_NAME: &str = "qdp-host";

/// The length of a key pin in hexadecimal digits.
const PIN_HEX_LENGTH: usize = 64;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Serve the desktop.
    Serve(ServeArgs),
    /// Connect to a host.
    Connect(ConnectArgs),
    /// Print the host's key pin.
    Pin {
        /// The directory of the host's identity.
        identity: PathBuf,
    },
    /// Show the usage text.
    Help,
}

/// What the host streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// The screen if it can be captured, otherwise the test pattern.
    Auto,
    /// The screen.
    Screen,
    /// A moving test pattern.
    TestPattern,
}

/// The options of `qdp serve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeArgs {
    /// The address to listen on.
    pub listen: SocketAddr,
    /// The directory of the host's identity.
    pub identity: PathBuf,
    /// The accepted tokens.
    pub tokens: Vec<String>,
    /// The accepted user, whose password is in `QDP_PASSWORD`.
    pub user: Option<String>,
    /// What to stream.
    pub source: SourceKind,
    /// The size of the test pattern.
    pub size: (usize, usize),
    /// The frame rate.
    pub fps: u32,
    /// Whether to encode in hardware or software.
    pub encoder: CodecPreference,
    /// The encoder speed preset.
    pub speed: u8,
    /// The encoder quantizer.
    pub quantizer: u8,
}

/// Where `qdp connect` shows the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// In a window.
    Window,
    /// Nowhere: the stream is only counted, and written to the file
    /// `--output` names.
    Headless,
}

/// The options of `qdp connect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectArgs {
    /// The host's address.
    pub address: SocketAddr,
    /// The host's key pin.
    pub pin: KeyPin,
    /// The token to authenticate with.
    pub token: Option<String>,
    /// The user to authenticate as, with the password in `QDP_PASSWORD`.
    pub user: Option<String>,
    /// The name sent in the TLS handshake.
    pub server_name: String,
    /// The number of frames after which to disconnect.
    pub frames: Option<u64>,
    /// Where to write the decoded video.
    pub output: Option<PathBuf>,
    /// Where to show the stream.
    pub presentation: Presentation,
    /// Whether to decode in hardware or software.
    pub decoder: CodecPreference,
}

/// An error in the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgsError(pub String);

impl std::fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Returns an error for `message`.
fn error(message: impl Into<String>) -> ArgsError {
    ArgsError(message.into())
}

/// Returns the value after option `name`.
fn value(name: &str, arguments: &mut impl Iterator<Item = String>) -> Result<String, ArgsError> {
    arguments
        .next()
        .ok_or_else(|| error(format!("{name} needs a value")))
}

/// Parses the value after option `name`.
fn parse<T: std::str::FromStr>(
    name: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<T, ArgsError> {
    let text = value(name, arguments)?;
    text.parse()
        .map_err(|_| error(format!("invalid value for {name}: {text}")))
}

/// Parses `WIDTHxHEIGHT`.
fn parse_size(text: &str) -> Result<(usize, usize), ArgsError> {
    let invalid = || error(format!("invalid size: {text}"));
    let (width, height) = text.split_once('x').ok_or_else(invalid)?;
    let width = width.parse().map_err(|_| invalid())?;
    let height = height.parse().map_err(|_| invalid())?;
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    Ok((width, height))
}

/// Parses a key pin written as 64 hexadecimal digits.
pub fn parse_pin(text: &str) -> Result<KeyPin, ArgsError> {
    let invalid = || error("a pin is 64 hexadecimal digits, as `qdp pin` prints");
    let digits: String = text.chars().filter(|character| *character != ':').collect();
    if digits.len() != PIN_HEX_LENGTH || !digits.is_ascii() {
        return Err(invalid());
    }
    let mut pin = [0; PIN_HEX_LENGTH / 2];
    for (index, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok(KeyPin(pin))
}

/// Resolves the host's address: an IP address or a host name, with or without a
/// port, using the default port when there is none.
pub fn resolve_address(text: &str) -> Result<SocketAddr, ArgsError> {
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
        .map_err(|resolve_error| error(format!("cannot resolve {text}: {resolve_error}")))?
        .next()
        .ok_or_else(|| error(format!("{text} has no address")))
}

/// Formats a key pin as 64 hexadecimal digits.
pub fn format_pin(pin: KeyPin) -> String {
    pin.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Parses the command line, without the program name.
pub fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Command, ArgsError> {
    let mut arguments = arguments.into_iter();
    match arguments.next().as_deref() {
        Some("serve") => parse_serve(arguments).map(Command::Serve),
        Some("connect") => parse_connect(arguments).map(Command::Connect),
        Some("pin") => {
            let mut identity = PathBuf::from(DEFAULT_IDENTITY_DIR);
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--identity" => identity = value("--identity", &mut arguments)?.into(),
                    other => return Err(error(format!("unknown option: {other}"))),
                }
            }
            Ok(Command::Pin { identity })
        }
        Some("help" | "--help" | "-h") | None => Ok(Command::Help),
        Some(other) => Err(error(format!("unknown command: {other}"))),
    }
}

/// Parses the options of `qdp serve`.
fn parse_serve(mut arguments: impl Iterator<Item = String>) -> Result<ServeArgs, ArgsError> {
    let mut serve = ServeArgs {
        listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
        identity: PathBuf::from(DEFAULT_IDENTITY_DIR),
        tokens: Vec::new(),
        user: None,
        size: (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        fps: DEFAULT_FPS,
        source: SourceKind::Auto,
        encoder: CodecPreference::Auto,
        speed: DEFAULT_SPEED,
        quantizer: DEFAULT_QUANTIZER,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--listen" => serve.listen = parse("--listen", &mut arguments)?,
            "--identity" => serve.identity = value("--identity", &mut arguments)?.into(),
            "--token" => serve.tokens.push(value("--token", &mut arguments)?),
            "--user" => serve.user = Some(value("--user", &mut arguments)?),
            "--size" => serve.size = parse_size(&value("--size", &mut arguments)?)?,
            "--fps" => serve.fps = parse("--fps", &mut arguments)?,
            "--source" => {
                serve.source = match value("--source", &mut arguments)?.as_str() {
                    "auto" => SourceKind::Auto,
                    "screen" => SourceKind::Screen,
                    "test-pattern" => SourceKind::TestPattern,
                    other => {
                        return Err(error(format!(
                            "--source must be auto, screen or test-pattern, not {other}"
                        )));
                    }
                }
            }
            "--encoder" => {
                serve.encoder = parse_preference("--encoder", &value("--encoder", &mut arguments)?)?
            }
            "--speed" => serve.speed = parse("--speed", &mut arguments)?,
            "--quantizer" => serve.quantizer = parse("--quantizer", &mut arguments)?,
            other => return Err(error(format!("unknown option: {other}"))),
        }
    }
    if serve.tokens.is_empty() && serve.user.is_none() {
        return Err(error(
            "serve needs --token or --user, or no client can connect",
        ));
    }
    if serve.fps == 0 {
        return Err(error("--fps must be above 0"));
    }
    Ok(serve)
}

/// Parses the value of `--encoder` or `--decoder`.
fn parse_preference(option: &str, text: &str) -> Result<CodecPreference, ArgsError> {
    match text {
        "auto" => Ok(CodecPreference::Auto),
        "hardware" => Ok(CodecPreference::Hardware),
        "software" => Ok(CodecPreference::Software),
        other => Err(error(format!(
            "{option} must be auto, hardware or software, not {other}"
        ))),
    }
}

/// Parses the arguments of `qdp connect`.
fn parse_connect(mut arguments: impl Iterator<Item = String>) -> Result<ConnectArgs, ArgsError> {
    let address = arguments
        .next()
        .ok_or_else(|| error("connect needs the host's address, such as 192.0.2.7"))?;
    let address = resolve_address(&address)?;
    let mut pin = None;
    let mut connect = ConnectArgs {
        address,
        pin: KeyPin([0; PIN_HEX_LENGTH / 2]),
        token: None,
        user: None,
        server_name: DEFAULT_SERVER_NAME.to_owned(),
        frames: None,
        output: None,
        presentation: Presentation::Window,
        decoder: CodecPreference::Auto,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--pin" => pin = Some(parse_pin(&value("--pin", &mut arguments)?)?),
            "--token" => connect.token = Some(value("--token", &mut arguments)?),
            "--user" => connect.user = Some(value("--user", &mut arguments)?),
            "--server-name" => connect.server_name = value("--server-name", &mut arguments)?,
            "--frames" => connect.frames = Some(parse("--frames", &mut arguments)?),
            "--output" => connect.output = Some(value("--output", &mut arguments)?.into()),
            "--headless" => connect.presentation = Presentation::Headless,
            "--decoder" => {
                connect.decoder =
                    parse_preference("--decoder", &value("--decoder", &mut arguments)?)?
            }
            other => return Err(error(format!("unknown option: {other}"))),
        }
    }
    connect.pin = pin.ok_or_else(|| error("connect needs --pin with the host's key pin"))?;
    if connect.token.is_some() == connect.user.is_some() {
        return Err(error("connect needs exactly one of --token and --user"));
    }
    Ok(connect)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a command line written as one string.
    fn parse_line(line: &str) -> Result<Command, ArgsError> {
        parse_args(line.split_whitespace().map(str::to_owned))
    }

    /// `serve` options are parsed with their defaults.
    #[test]
    fn serve() {
        let Command::Serve(serve) =
            parse_line("serve --token abc --size 640x480 --fps 30").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.tokens, ["abc"]);
        assert_eq!(serve.size, (640, 480));
        assert_eq!(serve.fps, 30);
        assert_eq!(serve.listen.port(), DEFAULT_PORT);
        assert_eq!(serve.encoder, CodecPreference::Auto);
        assert_eq!(serve.source, SourceKind::Auto);
        let Command::Serve(serve) = parse_line("serve --token abc --source test-pattern").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.source, SourceKind::TestPattern);
        assert!(parse_line("serve --token abc --source camera").is_err());
        let Command::Serve(serve) = parse_line("serve --token abc --encoder software").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.encoder, CodecPreference::Software);
        assert!(parse_line("serve --token abc --encoder gpu").is_err());
        assert!(parse_line("serve").is_err());
        assert!(parse_line("serve --token a --size 0x5").is_err());
    }

    /// `connect` needs an address, a pin and one credential.
    #[test]
    fn connect() {
        let pin = "ab".repeat(32);
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7:4433 --pin {pin} --user ian --frames 10"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.address.port(), 4433);
        assert_eq!(connect.pin, KeyPin([0xab; 32]));
        assert_eq!(connect.frames, Some(10));
        assert_eq!(connect.presentation, Presentation::Window);
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --headless"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.presentation, Presentation::Headless);
        assert_eq!(connect.decoder, CodecPreference::Auto);
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --decoder software"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.decoder, CodecPreference::Software);
        assert!(parse_line(&format!("connect 192.0.2.7:4433 --pin {pin}")).is_err());
        assert!(parse_line("connect 192.0.2.7:4433 --token t").is_err());
        assert!(parse_line("connect nowhere --pin x --token t").is_err());
    }

    /// Addresses may omit the port and may be host names.
    #[test]
    fn addresses() {
        let resolve = |text| resolve_address(text).unwrap();
        assert_eq!(resolve("192.0.2.7"), "192.0.2.7:4433".parse().unwrap());
        assert_eq!(resolve("192.0.2.7:5000"), "192.0.2.7:5000".parse().unwrap());
        assert_eq!(
            resolve("2001:db8::1"),
            "[2001:db8::1]:4433".parse().unwrap()
        );
        assert_eq!(
            resolve("[2001:db8::1]:5000"),
            "[2001:db8::1]:5000".parse().unwrap()
        );
        assert!(resolve("localhost").ip().is_loopback());
        assert_eq!(resolve("localhost:5000").port(), 5000);
        assert!(resolve_address("no-such-host.invalid").is_err());
    }

    /// Pins round-trip through their hexadecimal form, with or without colons.
    #[test]
    fn pins() {
        let pin = KeyPin(std::array::from_fn(|index| index as u8));
        assert_eq!(parse_pin(&format_pin(pin)).unwrap(), pin);
        let with_colons = format_pin(pin)
            .as_bytes()
            .chunks(2)
            .map(|pair| std::str::from_utf8(pair).unwrap())
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(parse_pin(&with_colons).unwrap(), pin);
        assert!(parse_pin("abc").is_err());
    }

    /// No arguments, or help, shows the usage text.
    #[test]
    fn help() {
        assert_eq!(parse_line("").unwrap(), Command::Help);
        assert_eq!(parse_line("help").unwrap(), Command::Help);
        assert!(parse_line("bogus").is_err());
    }
}
