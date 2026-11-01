//! Command-line arguments.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use qdp_codec::CodecPreference;
use qdp_desktop::Microphone;
use qdp_desktop::{DeviceSelector, RealMonitors};
use qdp_quic::KeyPin;
use qdp_runtime::{ClipboardDirections, PassthroughPolicy};

use crate::identity::Role;

/// The usage text printed by `qdp help` and on errors.
pub const USAGE: &str = "\
Usage:
  qdp serve [options]          Serve this machine's desktop
  qdp connect HOST [options]   Connect to a host, by address or name,
                               with an optional :PORT (default 4433)
  qdp pin [--identity DIR]     Print this host's key pin for clients
  qdp pin --client [--identity DIR]
                               Print this client's key pin for hosts
  qdp help                     Show this help

Serve options:
  --listen ADDRESS     Address to listen on (default 0.0.0.0:4433)
  --identity DIR       Directory holding the host's key (default ./qdp-identity)
  --token TOKEN        Accept clients presenting TOKEN (repeatable)
  --client-key HEX     Accept clients presenting the key HEX, from their
                       `qdp pin --client` (repeatable)
  --user NAME          Accept NAME with the password in QDP_PASSWORD
  --source KIND        auto, screen, virtual or test-pattern (default auto: the
                       screen, or the test pattern if it cannot be captured;
                       virtual adds a monitor at the client's display and
                       streams it, on KDE Plasma)
  --size WIDTHxHEIGHT  Size of the test pattern (default 1280x720)
  --fps N              Frames per second (default 60)
  --encoder KIND       auto, hardware or software (default auto: the GPU's
                       encoder when it has one, otherwise software)
  --speed N            Software encoder speed preset, 0 to 10 (default 10)
  --quantizer N        Encoder quantizer, 1 to 255 (default 100)
  --clipboard WAY      both, send, receive or off: share the clipboard's text
                       both ways, only this machine's, only the client's, or
                       not at all (default both)
  --audio on|off       Stream this machine's sound, and present clients'
                       microphones to its applications where the platform
                       allows (default on)
  --passthrough WHICH  games, all or off: which HID devices clients may pass
                       through to this machine's applications (default games,
                       game controllers only; all includes keyboards, mice
                       and security keys)
  --resume SECONDS     How long a client whose connection dropped may
                       reconnect without its credentials, 0 for never
                       (default 300)
  --lock on|off        Lock this machine's desktop when a session ends other
                       than by its client disconnecting, as when the network
                       drops (default on)
  --real-monitors on|off
                       With --source virtual, keep this machine's monitors on
                       beside the virtual one, or turn them off for the
                       session so that its windows move to it (default off)

Connect options:
  --pin HEX            The host's key pin, from `qdp pin` (required)
  --token TOKEN        Authenticate with TOKEN
  --user NAME          Authenticate as NAME with the password in QDP_PASSWORD
  --identity DIR       Directory holding this client's key, with which it
                       authenticates when given neither --token nor --user
                       (default ./qdp-client-identity)
  --server-name NAME   Name sent in the TLS handshake (default qdp-host)
  --frames N           Disconnect after N frames
  --output FILE        Write the decoded video to FILE as YUV4MPEG2
  --headless           Open no window; only log statistics
  --resolution WxH     Ask the host for video WIDTHxHEIGHT pixels, whatever
                       the window's size, and stretch it to fill the window,
                       to stream less at a large or full-screen window
                       (default: the window's size)
  --decoder KIND       auto, hardware or software (default auto: the GPU's
                       decoder when it has one, otherwise software)
  --decoder-process on|off
                       Decode the host's video in a separate process, so that
                       a decoder bug a malicious stream exploits cannot reach
                       the session; slower (default off)
  --clipboard WAY      both, send, receive or off: share the clipboard's text
                       both ways, only this machine's, only the host's, or
                       not at all (default both)
  --audio on|off       Play the host's sound (default on)
  --microphone         Send this machine's microphone to the host
  --touchpad           Send the touchpad's fingers for the host's own gestures,
                       rather than moving the pointer (Linux clients; needs
                       the input group)
  --passthrough ID     Pass the HID device VENDOR:PRODUCT, in hexadecimal as
                       lsusb shows it, through to the host as it is
                       (repeatable; Linux clients)

Set QDP_LOG to error, warn, info, debug or trace to change the log level, and
add TARGET=LEVEL after a comma to change it for one part, as in
QDP_LOG=info,qdp::window=debug.";

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

/// The default directory of the client's identity.
const DEFAULT_CLIENT_IDENTITY_DIR: &str = "qdp-client-identity";

/// The longest `--resume` a host accepts: a day.
const MAX_RESUME_SECONDS: u64 = 86_400;

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
    /// Decode video for a client that runs `connect` with
    /// `--decoder-process on`, on standard input and output. Not for
    /// people to run.
    Decoder,
    /// Print the key pin of the host's or the client's identity.
    Pin {
        /// The directory of the identity.
        identity: PathBuf,
        /// Whose identity it is.
        role: Role,
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
    /// A virtual monitor at the client's display.
    Virtual,
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
    /// The keys of the accepted client certificates.
    pub client_keys: Vec<KeyPin>,
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
    /// Which way to share the clipboard, if at all.
    pub clipboard: Option<ClipboardDirections>,
    /// Whether to stream sound.
    pub audio: Audio,
    /// Which HID devices clients may pass through.
    pub passthrough: PassthroughPolicy,
    /// How long resumption tokens remain valid after their connection
    /// ends, or `None` to issue none.
    pub resumption: Option<Duration>,
    /// What happens to the desktop when a session ends other than by its
    /// client disconnecting.
    pub lock: LockPolicy,
    /// What becomes of the real monitors while a virtual one is streamed.
    pub real_monitors: RealMonitors,
}

/// What a host does with its desktop when a session ends other than by its
/// client disconnecting (Section 11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockPolicy {
    /// The desktop is locked, so that it is not left unlocked.
    Lock,
    /// The desktop is left as it is.
    Leave,
}

/// What the client's touchpad does (Section 8.4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TouchpadMode {
    /// The client's platform turns it into pointer movement and scrolling,
    /// with the user's own settings.
    Pointer,
    /// Its raw contacts go to the host, which recognises the gestures.
    Raw,
}

/// Whether a session carries sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Audio {
    /// Sound plays.
    On,
    /// No sound.
    Off,
}

/// The command a client runs itself with to start its decoder process.
pub const DECODER_COMMAND: &str = "decoder";

/// Where a client decodes the host's video (Section 11.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecoderPlacement {
    /// In the client's own process, which is faster.
    InProcess,
    /// In a separate process, which isolates it from the session.
    SeparateProcess,
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
    /// The directory of the client's identity, with which it authenticates
    /// when given neither a token nor a user.
    pub identity: PathBuf,
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
    /// Where to decode the host's video.
    pub decoder_process: DecoderPlacement,
    /// Which way to share the clipboard, if at all.
    pub clipboard: Option<ClipboardDirections>,
    /// Whether to play the host's sound.
    pub audio: Audio,
    /// Whether the user turned the microphone on.
    pub microphone: Microphone,
    /// The HID devices the user chose to pass through.
    pub passthrough: Vec<DeviceSelector>,
    /// What the touchpad does.
    pub touchpad: TouchpadMode,
    /// The video size to ask the host for, in pixels, whatever the window's,
    /// if the user chose one.
    pub resolution: Option<(u32, u32)>,
}

/// The smallest side of a video the host can send (Section 6.3).
const MIN_VIDEO_SIDE: u32 = 16;

/// The largest side of a video the host can send (Section 6.3).
const MAX_VIDEO_SIDE: u32 = 16384;

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

/// Parses a video size written as `WIDTHxHEIGHT`, each side from 16 to
/// 16384 pixels.
fn parse_resolution(text: &str) -> Result<(u32, u32), ArgsError> {
    let invalid = || {
        error(format!(
            "invalid resolution: {text}; each side is {MIN_VIDEO_SIDE} to {MAX_VIDEO_SIDE} pixels"
        ))
    };
    let (width, height) = parse_size(text).map_err(|_| invalid())?;
    let side = |value: usize| {
        u32::try_from(value)
            .ok()
            .filter(|value| (MIN_VIDEO_SIDE..=MAX_VIDEO_SIDE).contains(value))
            .ok_or_else(invalid)
    };
    Ok((side(width)?, side(height)?))
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
        Some(DECODER_COMMAND) => Ok(Command::Decoder),
        Some("pin") => {
            let mut identity = None;
            let mut role = Role::Host;
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--identity" => identity = Some(value("--identity", &mut arguments)?.into()),
                    "--client" => role = Role::Client,
                    other => return Err(error(format!("unknown option: {other}"))),
                }
            }
            let default = match role {
                Role::Host => DEFAULT_IDENTITY_DIR,
                Role::Client => DEFAULT_CLIENT_IDENTITY_DIR,
            };
            Ok(Command::Pin {
                identity: identity.unwrap_or_else(|| PathBuf::from(default)),
                role,
            })
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
        client_keys: Vec::new(),
        user: None,
        size: (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        fps: DEFAULT_FPS,
        source: SourceKind::Auto,
        encoder: CodecPreference::Auto,
        speed: DEFAULT_SPEED,
        quantizer: DEFAULT_QUANTIZER,
        clipboard: Some(ClipboardDirections::Both),
        audio: Audio::On,
        passthrough: PassthroughPolicy::GameControllers,
        resumption: Some(qdp_runtime::DEFAULT_RESUMPTION_LIFETIME),
        lock: LockPolicy::Lock,
        real_monitors: RealMonitors::Off,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--listen" => serve.listen = parse("--listen", &mut arguments)?,
            "--identity" => serve.identity = value("--identity", &mut arguments)?.into(),
            "--token" => serve.tokens.push(value("--token", &mut arguments)?),
            "--client-key" => serve
                .client_keys
                .push(parse_pin(&value("--client-key", &mut arguments)?)?),
            "--user" => serve.user = Some(value("--user", &mut arguments)?),
            "--size" => serve.size = parse_size(&value("--size", &mut arguments)?)?,
            "--fps" => serve.fps = parse("--fps", &mut arguments)?,
            "--source" => {
                serve.source = match value("--source", &mut arguments)?.as_str() {
                    "auto" => SourceKind::Auto,
                    "screen" => SourceKind::Screen,
                    "test-pattern" => SourceKind::TestPattern,
                    "virtual" => SourceKind::Virtual,
                    other => {
                        return Err(error(format!(
                            "--source must be auto, screen, virtual or test-pattern, not {other}"
                        )));
                    }
                }
            }
            "--encoder" => {
                serve.encoder = parse_preference("--encoder", &value("--encoder", &mut arguments)?)?
            }
            "--speed" => serve.speed = parse("--speed", &mut arguments)?,
            "--quantizer" => serve.quantizer = parse("--quantizer", &mut arguments)?,
            "--clipboard" => {
                serve.clipboard = parse_clipboard(&value("--clipboard", &mut arguments)?)?
            }
            "--audio" => serve.audio = parse_audio(&value("--audio", &mut arguments)?)?,
            "--passthrough" => {
                serve.passthrough = parse_passthrough(&value("--passthrough", &mut arguments)?)?;
            }
            "--resume" => {
                let seconds: u64 = parse("--resume", &mut arguments)?;
                if seconds > MAX_RESUME_SECONDS {
                    return Err(error(format!(
                        "--resume must be at most {MAX_RESUME_SECONDS} seconds"
                    )));
                }
                serve.resumption = (seconds > 0).then(|| Duration::from_secs(seconds));
            }
            "--lock" => {
                serve.lock = match value("--lock", &mut arguments)?.as_str() {
                    "on" => LockPolicy::Lock,
                    "off" => LockPolicy::Leave,
                    other => return Err(error(format!("--lock must be on or off, not {other}"))),
                }
            }
            "--real-monitors" => {
                serve.real_monitors = match value("--real-monitors", &mut arguments)?.as_str() {
                    "on" => RealMonitors::On,
                    "off" => RealMonitors::Off,
                    other => {
                        return Err(error(format!(
                            "--real-monitors must be on or off, not {other}"
                        )));
                    }
                }
            }
            other => return Err(error(format!("unknown option: {other}"))),
        }
    }
    if serve.tokens.is_empty() && serve.user.is_none() && serve.client_keys.is_empty() {
        return Err(error(
            "serve needs --token, --user or --client-key, or no client can connect",
        ));
    }
    if serve.fps == 0 {
        return Err(error("--fps must be above 0"));
    }
    Ok(serve)
}

/// Parses the value of `--audio`.
fn parse_audio(text: &str) -> Result<Audio, ArgsError> {
    match text {
        "on" => Ok(Audio::On),
        "off" => Ok(Audio::Off),
        other => Err(error(format!("--audio must be on or off, not {other}"))),
    }
}

/// Parses the value of `--passthrough`.
fn parse_passthrough(text: &str) -> Result<PassthroughPolicy, ArgsError> {
    match text {
        "games" => Ok(PassthroughPolicy::GameControllers),
        "all" => Ok(PassthroughPolicy::Any),
        "off" => Ok(PassthroughPolicy::Off),
        other => Err(error(format!(
            "--passthrough must be games, all or off, not {other}"
        ))),
    }
}

/// Parses the value of `--clipboard`: `None` for `off`.
fn parse_clipboard(text: &str) -> Result<Option<ClipboardDirections>, ArgsError> {
    match text {
        "both" => Ok(Some(ClipboardDirections::Both)),
        "send" => Ok(Some(ClipboardDirections::SendOnly)),
        "receive" => Ok(Some(ClipboardDirections::ReceiveOnly)),
        "off" => Ok(None),
        other => Err(error(format!(
            "--clipboard must be both, send, receive or off, not {other}"
        ))),
    }
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
        identity: PathBuf::from(DEFAULT_CLIENT_IDENTITY_DIR),
        server_name: DEFAULT_SERVER_NAME.to_owned(),
        frames: None,
        output: None,
        presentation: Presentation::Window,
        decoder: CodecPreference::Auto,
        decoder_process: DecoderPlacement::InProcess,
        clipboard: Some(ClipboardDirections::Both),
        audio: Audio::On,
        microphone: Microphone::Off,
        passthrough: Vec::new(),
        touchpad: TouchpadMode::Pointer,
        resolution: None,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--pin" => pin = Some(parse_pin(&value("--pin", &mut arguments)?)?),
            "--token" => connect.token = Some(value("--token", &mut arguments)?),
            "--user" => connect.user = Some(value("--user", &mut arguments)?),
            "--identity" => connect.identity = value("--identity", &mut arguments)?.into(),
            "--server-name" => connect.server_name = value("--server-name", &mut arguments)?,
            "--frames" => connect.frames = Some(parse("--frames", &mut arguments)?),
            "--output" => connect.output = Some(value("--output", &mut arguments)?.into()),
            "--headless" => connect.presentation = Presentation::Headless,
            "--decoder" => {
                connect.decoder =
                    parse_preference("--decoder", &value("--decoder", &mut arguments)?)?
            }
            "--decoder-process" => {
                connect.decoder_process = match value("--decoder-process", &mut arguments)?.as_str()
                {
                    "on" => DecoderPlacement::SeparateProcess,
                    "off" => DecoderPlacement::InProcess,
                    other => {
                        return Err(error(format!(
                            "--decoder-process must be on or off, not {other}"
                        )));
                    }
                }
            }
            "--clipboard" => {
                connect.clipboard = parse_clipboard(&value("--clipboard", &mut arguments)?)?
            }
            "--audio" => connect.audio = parse_audio(&value("--audio", &mut arguments)?)?,
            "--microphone" => connect.microphone = Microphone::On,
            "--touchpad" => connect.touchpad = TouchpadMode::Raw,
            "--resolution" => {
                connect.resolution =
                    Some(parse_resolution(&value("--resolution", &mut arguments)?)?);
            }
            "--passthrough" => {
                let selector = value("--passthrough", &mut arguments)?;
                connect.passthrough.push(selector.parse().map_err(error)?);
            }
            other => return Err(error(format!("unknown option: {other}"))),
        }
    }
    connect.pin = pin.ok_or_else(|| error("connect needs --pin with the host's key pin"))?;
    if connect.token.is_some() && connect.user.is_some() {
        return Err(error("connect takes --token or --user, not both"));
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
        let Command::Serve(serve) = parse_line("serve --token abc --source virtual").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.source, SourceKind::Virtual);
        assert!(parse_line("serve --token abc --source camera").is_err());
        let Command::Serve(serve) = parse_line("serve --token abc --encoder software").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.encoder, CodecPreference::Software);
        assert!(parse_line("serve --token abc --encoder gpu").is_err());
        assert_eq!(serve.clipboard, Some(ClipboardDirections::Both));
        let Command::Serve(serve) = parse_line("serve --token abc --clipboard receive").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.clipboard, Some(ClipboardDirections::ReceiveOnly));
        assert!(parse_line("serve --token abc --clipboard sideways").is_err());
        assert_eq!(serve.passthrough, PassthroughPolicy::GameControllers);
        let Command::Serve(serve) = parse_line("serve --token abc --passthrough off").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.passthrough, PassthroughPolicy::Off);
        assert!(parse_line("serve --token abc --passthrough keyboards").is_err());
        assert_eq!(serve.audio, Audio::On);
        let Command::Serve(serve) = parse_line("serve --token abc --audio off").unwrap() else {
            panic!("expected serve");
        };
        assert_eq!(serve.audio, Audio::Off);
        assert!(parse_line("serve --token abc --audio loud").is_err());
        assert_eq!(
            serve.resumption,
            Some(qdp_runtime::DEFAULT_RESUMPTION_LIFETIME)
        );
        let Command::Serve(serve) = parse_line("serve --token abc --resume 0").unwrap() else {
            panic!("expected serve");
        };
        assert_eq!(serve.resumption, None);
        let Command::Serve(serve) = parse_line("serve --token abc --resume 60").unwrap() else {
            panic!("expected serve");
        };
        assert_eq!(serve.resumption, Some(Duration::from_secs(60)));
        assert!(parse_line("serve --token abc --resume 100000").is_err());
        assert_eq!(serve.lock, LockPolicy::Lock);
        let Command::Serve(serve) = parse_line("serve --token abc --lock off").unwrap() else {
            panic!("expected serve");
        };
        assert_eq!(serve.lock, LockPolicy::Leave);
        assert!(parse_line("serve --token abc --lock maybe").is_err());
        assert_eq!(serve.real_monitors, RealMonitors::Off);
        let Command::Serve(serve) = parse_line("serve --token abc --real-monitors on").unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.real_monitors, RealMonitors::On);
        assert!(parse_line("serve --token abc --real-monitors maybe").is_err());
        assert!(parse_line("serve").is_err());
        let pin = "ab".repeat(PIN_HEX_LENGTH / 2);
        let Command::Serve(serve) = parse_line(&format!("serve --client-key {pin}")).unwrap()
        else {
            panic!("expected serve");
        };
        assert_eq!(serve.client_keys, [KeyPin([0xab; PIN_HEX_LENGTH / 2])]);
        assert!(parse_line("serve --client-key nothex").is_err());
        assert_eq!(
            parse_line("pin --client").unwrap(),
            Command::Pin {
                identity: PathBuf::from(DEFAULT_CLIENT_IDENTITY_DIR),
                role: Role::Client,
            }
        );
        assert_eq!(
            parse_line("pin --identity keys").unwrap(),
            Command::Pin {
                identity: PathBuf::from("keys"),
                role: Role::Host,
            }
        );
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
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --clipboard off"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.clipboard, None);
        assert_eq!(connect.microphone, Microphone::Off);
        assert!(connect.passthrough.is_empty());
        assert_eq!(connect.touchpad, TouchpadMode::Pointer);
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --touchpad"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.touchpad, TouchpadMode::Raw);
        assert_eq!(connect.resolution, None);
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --resolution 1920x1080"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.resolution, Some((1920, 1080)));
        for invalid in ["1920", "8x8", "20000x1080", "x1080"] {
            assert!(
                parse_line(&format!(
                    "connect 192.0.2.7 --pin {pin} --token t --resolution {invalid}"
                ))
                .is_err(),
                "{invalid}"
            );
        }
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --passthrough 054c:09cc --passthrough 057e:2009"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.passthrough.len(), 2);
        assert!(connect.passthrough[1].matches(0x057e, 0x2009));
        assert!(
            parse_line(&format!(
                "connect 192.0.2.7 --pin {pin} --token t --passthrough gamepad"
            ))
            .is_err()
        );
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --microphone --audio off"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(
            (connect.audio, connect.microphone),
            (Audio::Off, Microphone::On)
        );
        // Without a token or a user, the client authenticates with its key.
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7:4433 --pin {pin} --identity keys"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!((connect.token, connect.user), (None, None));
        assert_eq!(connect.identity, PathBuf::from("keys"));
        assert!(parse_line(&format!("connect 192.0.2.7 --pin {pin} --token t --user u")).is_err());
        assert!(parse_line("connect 192.0.2.7:4433 --token t").is_err());
        assert_eq!(connect.decoder_process, DecoderPlacement::InProcess);
        let Command::Connect(connect) = parse_line(&format!(
            "connect 192.0.2.7 --pin {pin} --token t --decoder-process on"
        ))
        .unwrap() else {
            panic!("expected connect");
        };
        assert_eq!(connect.decoder_process, DecoderPlacement::SeparateProcess);
        assert!(
            parse_line(&format!(
                "connect 192.0.2.7 --pin {pin} --token t --decoder-process maybe"
            ))
            .is_err()
        );
        assert_eq!(parse_line("decoder").unwrap(), Command::Decoder);
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
