//! The gateway protocol (Section 13): the messages of the control stream of a
//! client's connection to a gateway, which authenticates its user and then
//! tunnels the client's QDP connection to a host inside a private network.
//!
//! Gateway messages are framed as QDP control messages are (Section 3.6.1), so
//! [`ControlStreamDecoder`] splits the stream and [`GatewayMessage::decode`]
//! reads each frame, whose type is then a [`GatewayMessageType`]. The
//! authentication messages are QDP's own ([`AuthStart`], [`AuthContinue`] and
//! [`AuthResult`]), with the [`AuthStatus::NextFactor`] status.

pub mod assertion;

use std::time::Duration;

use crate::discovery::PIN_LENGTH;
#[cfg(doc)]
use crate::message::AuthStatus;
use crate::message::{AuthContinue, AuthMethod, AuthResult, AuthStart, MessagePayload};
use crate::registry::MessageType;
#[cfg(doc)]
use crate::wire::ControlStreamDecoder;
use crate::wire::{ControlMessageFrame, Reader, VarInt, WireError, Writer};

/// The ALPN token of the gateway protocol of this draft (Section 13.2).
pub const GATEWAY_ALPN: &[u8] = b"qdp-gateway-draft-00";

/// The UDP port gateways listen on by default (Section 13.2).
pub const DEFAULT_GATEWAY_PORT: u16 = 443;

/// The version of the gateway protocol this crate implements (Section 13.3).
pub const GATEWAY_VERSION: VarInt = VarInt::ZERO;

/// The bytes of control messages a gateway accepts before authentication
/// succeeds, 16 KiB (Section 13.3).
pub const GATEWAY_PRE_AUTHENTICATION_BUDGET: u64 = 16 << 10;

/// The largest UDP payload a tunnel must carry, which the outer connection's
/// datagrams must fit before the inner connection starts (Section 13.2).
pub const MIN_TUNNEL_PAYLOAD: usize = 1200;

/// The longest Client Name or Gateway Name, in bytes (Section 13.3).
pub const MAX_DEVICE_NAME_LENGTH: usize = 64;

/// The longest Host ID, Description, Server Name or Reason, in bytes
/// (Section 13.5).
pub const MAX_FIELD_LENGTH: usize = 255;

/// The longest host Name, in bytes (Section 13.5).
pub const MAX_HOST_NAME_LENGTH: usize = 64;

/// The longest session token, in bytes (Section 13.4).
pub const MAX_SESSION_TOKEN_LENGTH: usize = 256;

/// The most entries a `HOSTS` message or method list holds.
const MAX_ENTRIES: u64 = 4096;

/// A gateway control message type (Section 12.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GatewayMessageType(pub VarInt);

impl GatewayMessageType {
    /// `GATEWAY_HELLO`, client to gateway.
    pub const GATEWAY_HELLO: Self = Self(VarInt::from_u32(0x01));
    /// `GATEWAY_WELCOME`, gateway to client.
    pub const GATEWAY_WELCOME: Self = Self(VarInt::from_u32(0x02));
    /// `AUTH_START`, client to gateway.
    pub const AUTH_START: Self = Self(MessageType::AUTH_START.0);
    /// `AUTH_CONTINUE`, both directions.
    pub const AUTH_CONTINUE: Self = Self(MessageType::AUTH_CONTINUE.0);
    /// `AUTH_RESULT`, gateway to client.
    pub const AUTH_RESULT: Self = Self(MessageType::AUTH_RESULT.0);
    /// `SESSION_TOKEN`, gateway to client.
    pub const SESSION_TOKEN: Self = Self(VarInt::from_u32(0x06));
    /// `HOSTS_REQUEST`, client to gateway.
    pub const HOSTS_REQUEST: Self = Self(VarInt::from_u32(0x07));
    /// `HOSTS`, gateway to client.
    pub const HOSTS: Self = Self(VarInt::from_u32(0x08));
    /// `OPEN`, client to gateway.
    pub const OPEN: Self = Self(VarInt::from_u32(0x09));
    /// `OPENED`, gateway to client.
    pub const OPENED: Self = Self(VarInt::from_u32(0x0A));
    /// `OPEN_FAILED`, gateway to client.
    pub const OPEN_FAILED: Self = Self(VarInt::from_u32(0x0B));
}

/// A QUIC application error code closing a gateway connection (Section 13.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GatewayErrorCode(pub VarInt);

impl GatewayErrorCode {
    /// The client is done, or the host session ended.
    pub const NO_ERROR: Self = Self(VarInt::from_u32(0x00));
    /// An implementation fault.
    pub const INTERNAL_ERROR: Self = Self(VarInt::from_u32(0x01));
    /// The peer broke a rule of Section 13.
    pub const PROTOCOL_VIOLATION: Self = Self(VarInt::from_u32(0x02));
    /// Authentication failed.
    pub const AUTH_FAILED: Self = Self(VarInt::from_u32(0x03));
    /// No version in common.
    pub const VERSION: Self = Self(VarInt::from_u32(0x04));
    /// The tunnel carried nothing for too long.
    pub const IDLE: Self = Self(VarInt::from_u32(0x05));
    /// The path cannot carry 1200-byte datagrams.
    pub const PATH_TOO_SMALL: Self = Self(VarInt::from_u32(0x06));
    /// The gateway's administrator ended the connection, or revoked the user.
    pub const POLICY: Self = Self(VarInt::from_u32(0x07));
    /// The gateway is shutting down.
    pub const SHUTDOWN: Self = Self(VarInt::from_u32(0x08));
}

/// Why a gateway could not open a host (Section 13.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpenFailure(pub VarInt);

impl OpenFailure {
    /// The gateway knows no host by that ID, or the user may not reach it.
    pub const UNKNOWN_HOST: Self = Self(VarInt::from_u32(0x01));
    /// The gateway cannot reach the host.
    pub const UNREACHABLE: Self = Self(VarInt::from_u32(0x02));
    /// The gateway has reached a limit, such as of tunnels.
    pub const BUSY: Self = Self(VarInt::from_u32(0x03));
}

/// A `GATEWAY_HELLO` message (Section 13.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayHello {
    /// The highest version of the protocol the client implements.
    pub version: VarInt,
    /// The client device's name for users, at most 64 bytes.
    pub client_name: String,
}

/// A `GATEWAY_WELCOME` message (Section 13.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayWelcome {
    /// The version the gateway uses.
    pub version: VarInt,
    /// The gateway's name for users, at most 64 bytes.
    pub gateway_name: String,
    /// The methods the client may start an attempt with.
    pub auth_methods: Vec<AuthMethod>,
}

/// A `SESSION_TOKEN` message (Section 13.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionToken {
    /// The token, at most 256 bytes.
    pub token: Vec<u8>,
    /// How long it remains valid.
    pub lifetime: Duration,
}

/// A host a user may reach, in a `HOSTS` message (Section 13.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    /// The gateway's identifier of the host, at most 255 bytes.
    pub host_id: String,
    /// The host's name for users, at most 64 bytes.
    pub name: String,
    /// More about the host, at most 255 bytes, or empty.
    pub description: String,
}

/// An `OPEN` message (Section 13.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    /// The host to open: a Host ID from `HOSTS`, or the host's name or
    /// address inside the private network, with an optional port.
    pub target: String,
}

/// An `OPENED` message (Section 13.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The key pin the client verifies the host's certificate against, if the
    /// gateway knows it; otherwise the client verifies the host itself.
    pub host_pin: Option<[u8; PIN_LENGTH]>,
    /// The name the client sends in the inner TLS handshake.
    pub server_name: String,
    /// An identity assertion for the host, or empty (Section 13.7).
    pub assertion: Vec<u8>,
}

/// An `OPEN_FAILED` message (Section 13.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFailed {
    /// Why the host could not be opened.
    pub code: OpenFailure,
    /// A human-readable explanation, at most 255 bytes.
    pub reason: String,
}

/// The Auth Data of an `AUTH_RESULT` with the `NEXT_FACTOR` status: the
/// methods the next attempt may use, and a TOTP secret to enroll, if the
/// gateway offers one (Section 13.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextFactor {
    /// The methods the next attempt may use.
    pub auth_methods: Vec<AuthMethod>,
    /// An `otpauth://totp/` URI with a new secret for the user to enroll.
    pub enrollment: Option<String>,
}

impl NextFactor {
    /// Returns the encoded Auth Data.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        write_methods(&self.auth_methods, &mut writer);
        if let Some(enrollment) = &self.enrollment {
            writer.write_string(enrollment);
        }
        writer.into_bytes()
    }

    /// Reads encoded Auth Data.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(bytes);
        let auth_methods = read_methods(&mut reader)?;
        let enrollment = if reader.is_empty() {
            None
        } else {
            Some(reader.read_string()?.to_owned())
        };
        Ok(Self {
            auth_methods,
            enrollment,
        })
    }
}

/// Any gateway control message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayMessage {
    /// `GATEWAY_HELLO`.
    Hello(GatewayHello),
    /// `GATEWAY_WELCOME`.
    Welcome(GatewayWelcome),
    /// `AUTH_START`.
    AuthStart(AuthStart),
    /// `AUTH_CONTINUE`.
    AuthContinue(AuthContinue),
    /// `AUTH_RESULT`.
    AuthResult(AuthResult),
    /// `SESSION_TOKEN`.
    SessionToken(SessionToken),
    /// `HOSTS_REQUEST`.
    HostsRequest,
    /// `HOSTS`.
    Hosts(Vec<HostEntry>),
    /// `OPEN`.
    Open(Open),
    /// `OPENED`.
    Opened(Opened),
    /// `OPEN_FAILED`.
    OpenFailed(OpenFailed),
    /// A message of a type this crate does not know, which receivers ignore.
    Unknown(GatewayMessageType),
}

impl GatewayMessage {
    /// Reads a frame from the control stream.
    pub fn decode(frame: &ControlMessageFrame) -> Result<Self, WireError> {
        let reader = &mut Reader::new(&frame.payload);
        let message = match GatewayMessageType(frame.message_type.0) {
            GatewayMessageType::GATEWAY_HELLO => Self::Hello(GatewayHello {
                version: reader.read_varint()?,
                client_name: read_string(reader, "Client Name", MAX_DEVICE_NAME_LENGTH)?,
            }),
            GatewayMessageType::GATEWAY_WELCOME => Self::Welcome(GatewayWelcome {
                version: reader.read_varint()?,
                gateway_name: read_string(reader, "Gateway Name", MAX_DEVICE_NAME_LENGTH)?,
                auth_methods: read_methods(reader)?,
            }),
            GatewayMessageType::AUTH_START => Self::AuthStart(AuthStart::decode(reader)?),
            GatewayMessageType::AUTH_CONTINUE => Self::AuthContinue(AuthContinue::decode(reader)?),
            GatewayMessageType::AUTH_RESULT => Self::AuthResult(AuthResult::decode(reader)?),
            GatewayMessageType::SESSION_TOKEN => Self::SessionToken(SessionToken {
                token: reader
                    .read_octets_at_most("Token", MAX_SESSION_TOKEN_LENGTH)?
                    .to_vec(),
                lifetime: Duration::from_secs(reader.read_varint()?.into_inner()),
            }),
            GatewayMessageType::HOSTS_REQUEST => Self::HostsRequest,
            GatewayMessageType::HOSTS => {
                let count = reader.read_count("Host Count", MAX_ENTRIES)?;
                let hosts = (0..count)
                    .map(|_| {
                        Ok(HostEntry {
                            host_id: read_string(reader, "Host ID", MAX_FIELD_LENGTH)?,
                            name: read_string(reader, "Name", MAX_HOST_NAME_LENGTH)?,
                            description: read_string(reader, "Description", MAX_FIELD_LENGTH)?,
                        })
                    })
                    .collect::<Result<_, WireError>>()?;
                Self::Hosts(hosts)
            }
            GatewayMessageType::OPEN => Self::Open(Open {
                target: read_string(reader, "Target", MAX_FIELD_LENGTH)?,
            }),
            GatewayMessageType::OPENED => Self::Opened(Opened {
                host_pin: match reader.read_octets_at_most("Host Pin", PIN_LENGTH)? {
                    [] => None,
                    pin => Some(
                        pin.try_into()
                            .map_err(|_| WireError::InvalidValue { field: "Host Pin" })?,
                    ),
                },
                server_name: read_string(reader, "Server Name", MAX_FIELD_LENGTH)?,
                assertion: reader
                    .read_octets_at_most("Assertion", assertion::MAX_ASSERTION_LENGTH)?
                    .to_vec(),
            }),
            GatewayMessageType::OPEN_FAILED => Self::OpenFailed(OpenFailed {
                code: OpenFailure(reader.read_varint()?),
                reason: read_string(reader, "Reason", MAX_FIELD_LENGTH)?,
            }),
            other => Self::Unknown(other),
        };
        Ok(message)
    }

    /// Returns the frame that carries the message, or `None` for an unknown
    /// one, which has nothing to send.
    pub fn to_frame(&self) -> Option<ControlMessageFrame> {
        let mut writer = Writer::new();
        let message_type = match self {
            Self::Hello(hello) => {
                writer.write_varint(hello.version);
                writer.write_string_at_most(&hello.client_name, MAX_DEVICE_NAME_LENGTH);
                GatewayMessageType::GATEWAY_HELLO
            }
            Self::Welcome(welcome) => {
                writer.write_varint(welcome.version);
                writer.write_string_at_most(&welcome.gateway_name, MAX_DEVICE_NAME_LENGTH);
                write_methods(&welcome.auth_methods, &mut writer);
                GatewayMessageType::GATEWAY_WELCOME
            }
            Self::AuthStart(start) => {
                start.encode(&mut writer);
                GatewayMessageType::AUTH_START
            }
            Self::AuthContinue(next) => {
                next.encode(&mut writer);
                GatewayMessageType::AUTH_CONTINUE
            }
            Self::AuthResult(result) => {
                result.encode(&mut writer);
                GatewayMessageType::AUTH_RESULT
            }
            Self::SessionToken(token) => {
                writer.write_octets(&token.token);
                writer.write_varint(VarInt::new(token.lifetime.as_secs()).unwrap_or(VarInt::MAX));
                GatewayMessageType::SESSION_TOKEN
            }
            Self::HostsRequest => GatewayMessageType::HOSTS_REQUEST,
            Self::Hosts(hosts) => {
                writer.write_count(hosts.len());
                for host in hosts {
                    writer.write_string_at_most(&host.host_id, MAX_FIELD_LENGTH);
                    writer.write_string_at_most(&host.name, MAX_HOST_NAME_LENGTH);
                    writer.write_string_at_most(&host.description, MAX_FIELD_LENGTH);
                }
                GatewayMessageType::HOSTS
            }
            Self::Open(open) => {
                writer.write_string_at_most(&open.target, MAX_FIELD_LENGTH);
                GatewayMessageType::OPEN
            }
            Self::Opened(opened) => {
                writer.write_octets(opened.host_pin.as_ref().map_or(&[][..], |pin| &pin[..]));
                writer.write_string_at_most(&opened.server_name, MAX_FIELD_LENGTH);
                writer.write_octets(&opened.assertion);
                GatewayMessageType::OPENED
            }
            Self::OpenFailed(failed) => {
                writer.write_varint(failed.code.0);
                writer.write_string_at_most(&failed.reason, MAX_FIELD_LENGTH);
                GatewayMessageType::OPEN_FAILED
            }
            Self::Unknown(_) => return None,
        };
        Some(ControlMessageFrame::new(
            MessageType(message_type.0),
            writer.into_bytes(),
        ))
    }
}

/// Reads a String of at most `limit` bytes.
fn read_string(
    reader: &mut Reader<'_>,
    field: &'static str,
    limit: usize,
) -> Result<String, WireError> {
    Ok(reader.read_string_at_most(field, limit)?.to_owned())
}

/// Writes a list of methods: their count, then each.
fn write_methods(methods: &[AuthMethod], writer: &mut Writer) {
    writer.write_count(methods.len());
    for method in methods {
        writer.write_varint(method.0);
    }
}

/// Reads a list of methods.
fn read_methods(reader: &mut Reader<'_>) -> Result<Vec<AuthMethod>, WireError> {
    let count = reader.read_count("Method Count", MAX_ENTRIES)?;
    (0..count)
        .map(|_| reader.read_varint().map(AuthMethod))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::AuthStatus;
    use crate::wire::ControlStreamDecoder;

    /// Encodes `message`, splits it from a stream as a receiver does, and
    /// decodes it.
    fn round_trip(message: &GatewayMessage) -> GatewayMessage {
        let bytes = message.to_frame().unwrap().to_bytes().unwrap();
        let mut decoder = ControlStreamDecoder::with_budget(GATEWAY_PRE_AUTHENTICATION_BUDGET);
        decoder.push(&bytes);
        let frame = decoder.next_message().unwrap().unwrap();
        GatewayMessage::decode(&frame).unwrap()
    }

    #[test]
    fn round_trips_every_message() {
        let messages = [
            GatewayMessage::Hello(GatewayHello {
                version: GATEWAY_VERSION,
                client_name: "Ian's laptop".to_owned(),
            }),
            GatewayMessage::Welcome(GatewayWelcome {
                version: GATEWAY_VERSION,
                gateway_name: "Example Corp".to_owned(),
                auth_methods: vec![AuthMethod::PASSWORD, AuthMethod::TOKEN],
            }),
            GatewayMessage::AuthStart(AuthStart {
                auth_method: AuthMethod::TOTP,
                auth_data: b"123456".to_vec(),
            }),
            GatewayMessage::AuthContinue(AuthContinue {
                auth_data: vec![1, 2, 3],
            }),
            GatewayMessage::AuthResult(AuthResult {
                status: AuthStatus::NextFactor,
                auth_data: NextFactor {
                    auth_methods: vec![AuthMethod::TOTP],
                    enrollment: None,
                }
                .to_bytes(),
                reason: String::new(),
            }),
            GatewayMessage::SessionToken(SessionToken {
                token: vec![7; 32],
                lifetime: Duration::from_secs(12 * 60 * 60),
            }),
            GatewayMessage::HostsRequest,
            GatewayMessage::Hosts(vec![
                HostEntry {
                    host_id: "build-01".to_owned(),
                    name: "Build machine".to_owned(),
                    description: "Rack 4, London".to_owned(),
                },
                HostEntry {
                    host_id: "desk-ian".to_owned(),
                    name: "Ian's desk".to_owned(),
                    description: String::new(),
                },
            ]),
            GatewayMessage::Open(Open {
                target: "build-01".to_owned(),
            }),
            GatewayMessage::Open(Open {
                target: "build-02.corp.example.com:4433".to_owned(),
            }),
            GatewayMessage::Opened(Opened {
                host_pin: Some([0xab; PIN_LENGTH]),
                server_name: "qdp-host".to_owned(),
                assertion: vec![9; 300],
            }),
            GatewayMessage::Opened(Opened {
                host_pin: None,
                server_name: "build-02.corp.example.com".to_owned(),
                assertion: Vec::new(),
            }),
            GatewayMessage::OpenFailed(OpenFailed {
                code: OpenFailure::UNKNOWN_HOST,
                reason: "No such host".to_owned(),
            }),
        ];
        for message in messages {
            assert_eq!(round_trip(&message), message);
        }
    }

    #[test]
    fn encodes_next_factor_with_and_without_enrollment() {
        let plain = NextFactor {
            auth_methods: vec![AuthMethod::TOTP],
            enrollment: None,
        };
        assert_eq!(plain.to_bytes(), [0x01, 0x09]);
        assert_eq!(NextFactor::from_bytes(&plain.to_bytes()).unwrap(), plain);
        let enrolling = NextFactor {
            auth_methods: vec![AuthMethod::TOTP],
            enrollment: Some("otpauth://totp/Example:ian?secret=JBSWY3DPEHPK3PXP".to_owned()),
        };
        assert_eq!(
            NextFactor::from_bytes(&enrolling.to_bytes()).unwrap(),
            enrolling
        );
    }

    #[test]
    fn keeps_unknown_messages_to_ignore() {
        let frame = ControlMessageFrame::new(MessageType(VarInt::from_u32(0x30)), vec![1, 2]);
        assert_eq!(
            GatewayMessage::decode(&frame).unwrap(),
            GatewayMessage::Unknown(GatewayMessageType(VarInt::from_u32(0x30)))
        );
        assert!(
            GatewayMessage::Unknown(GatewayMessageType(VarInt::ZERO))
                .to_frame()
                .is_none()
        );
    }

    #[test]
    fn rejects_a_partial_host_pin() {
        let mut writer = Writer::new();
        writer.write_octets(&[0xab; 16]);
        writer.write_string("qdp-host");
        writer.write_octets(&[]);
        let frame = ControlMessageFrame::new(
            MessageType(GatewayMessageType::OPENED.0),
            writer.into_bytes(),
        );
        assert_eq!(
            GatewayMessage::decode(&frame),
            Err(WireError::InvalidValue { field: "Host Pin" })
        );
    }

    #[test]
    fn rejects_overlong_fields() {
        let mut writer = Writer::new();
        writer.write_varint(GATEWAY_VERSION);
        writer.write_string(&"x".repeat(MAX_DEVICE_NAME_LENGTH + 1));
        let frame = ControlMessageFrame::new(
            MessageType(GatewayMessageType::GATEWAY_HELLO.0),
            writer.into_bytes(),
        );
        assert!(matches!(
            GatewayMessage::decode(&frame),
            Err(WireError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn limits_messages_before_authentication() {
        let mut decoder = ControlStreamDecoder::with_budget(GATEWAY_PRE_AUTHENTICATION_BUDGET);
        let frame = ControlMessageFrame::new(
            MessageType(GatewayMessageType::AUTH_START.0),
            vec![0; usize::try_from(GATEWAY_PRE_AUTHENTICATION_BUDGET).unwrap() + 1],
        );
        decoder.push(&frame.to_bytes().unwrap());
        assert!(matches!(
            decoder.next_message(),
            Err(WireError::MessageTooLarge { .. })
        ));
    }
}
