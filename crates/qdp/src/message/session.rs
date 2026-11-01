//! Hello, authentication and configuration messages (Section 4).

use std::time::Duration;

use crate::message::{Capability, MessagePayload};
use crate::registry::MessageType;
use crate::wire::{Reader, VarInt, WireError, Writer};

/// The longest Client Name, Host Name or Reason, in bytes (Sections 4.3 and 4.4.1).
pub const MAX_NAME_LENGTH: usize = 255;

/// The longest Auth Data of any authentication message, in bytes (Section 4.4.1).
pub const MAX_AUTH_DATA_LENGTH: usize = 8192;

/// The longest resumption token, in bytes (Section 4.4.7).
pub const MAX_RESUMPTION_TOKEN_LENGTH: usize = 256;

/// A minor version of the specification (Section 4.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MinorVersion(pub VarInt);

impl MinorVersion {
    /// The minor version this crate implements.
    pub const CURRENT: Self = Self(VarInt::ZERO);
}

/// An authentication method (Sections 4.4 and 12.7).
///
/// Unknown methods are representable, because a host may offer methods that a
/// client does not implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthMethod(pub VarInt);

impl AuthMethod {
    /// `CLIENT_CERTIFICATE`: the certificate presented in the TLS handshake
    /// (Section 4.4.2).
    pub const CLIENT_CERTIFICATE: Self = Self(VarInt::from_u32(0x01));
    /// `TOKEN`: an opaque bearer token (Section 4.4.3).
    pub const TOKEN: Self = Self(VarInt::from_u32(0x02));
    /// `SCRAM_SHA_256_PLUS`: a password exchange bound to the TLS connection
    /// (Section 4.4.4).
    pub const SCRAM_SHA_256_PLUS: Self = Self(VarInt::from_u32(0x03));
    /// `ANONYMOUS`: no authentication (Section 4.4.5).
    pub const ANONYMOUS: Self = Self(VarInt::from_u32(0x04));
    /// `PASSWORD`: a user name and password checked by the host's operating system
    /// (Section 4.4.8).
    pub const PASSWORD: Self = Self(VarInt::from_u32(0x05));
    /// `PAIRING`: a four-digit code the host shows, which the client's user
    /// types (Section 4.4.9).
    pub const PAIRING: Self = Self(VarInt::from_u32(0x06));
    /// `TOTP`: a time-based one-time password from an authenticator app, as
    /// a second factor (Section 4.4.10).
    pub const TOTP: Self = Self(VarInt::from_u32(0x09));
}

/// The outcome of an authentication attempt (Section 4.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AuthStatus {
    /// The client is authenticated.
    Success = 0x00,
    /// The attempt failed; the client may start another.
    FailedRetry = 0x01,
    /// The attempt failed; the host will close the connection.
    FailedFinal = 0x02,
    /// The attempt succeeded, and the user must also authenticate with
    /// another method (Section 4.4.1).
    NextFactor = 0x03,
}

impl From<AuthStatus> for VarInt {
    fn from(status: AuthStatus) -> Self {
        VarInt::from(status as u8)
    }
}

impl TryFrom<VarInt> for AuthStatus {
    type Error = WireError;

    fn try_from(value: VarInt) -> Result<Self, Self::Error> {
        match value.into_inner() {
            0x00 => Ok(Self::Success),
            0x01 => Ok(Self::FailedRetry),
            0x02 => Ok(Self::FailedFinal),
            0x03 => Ok(Self::NextFactor),
            _ => Err(WireError::InvalidValue { field: "Status" }),
        }
    }
}

/// A `CLIENT_HELLO` message (Section 4.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHello {
    /// The highest minor version the client implements.
    pub minor_version: MinorVersion,
    /// A product name and version for logs, at most 255 bytes.
    pub client_name: String,
    /// The client's capabilities, at most 64.
    pub capabilities: Vec<Capability>,
}

impl MessagePayload for ClientHello {
    const MESSAGE_TYPE: MessageType = MessageType::CLIENT_HELLO;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.minor_version.0);
        writer.write_string_at_most(&self.client_name, MAX_NAME_LENGTH);
        Capability::encode_list(&self.capabilities, writer);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            minor_version: MinorVersion(reader.read_varint()?),
            client_name: reader
                .read_string_at_most("Client Name", MAX_NAME_LENGTH)?
                .to_owned(),
            capabilities: Capability::decode_list(reader)?,
        })
    }
}

/// A `HOST_HELLO` message (Section 4.3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostHello {
    /// The highest minor version the host implements.
    pub minor_version: MinorVersion,
    /// A name for the host, at most 255 bytes, which may be empty.
    pub host_name: String,
    /// The methods the host accepts, in its order of preference. Never empty.
    pub auth_methods: Vec<AuthMethod>,
    /// The host's capabilities, at most 64.
    pub capabilities: Vec<Capability>,
}

impl MessagePayload for HostHello {
    const MESSAGE_TYPE: MessageType = MessageType::HOST_HELLO;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.minor_version.0);
        writer.write_string_at_most(&self.host_name, MAX_NAME_LENGTH);
        writer.write_count(self.auth_methods.len());
        for method in &self.auth_methods {
            writer.write_varint(method.0);
        }
        Capability::encode_list(&self.capabilities, writer);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let minor_version = MinorVersion(reader.read_varint()?);
        let host_name = reader
            .read_string_at_most("Host Name", MAX_NAME_LENGTH)?
            .to_owned();
        let method_count = reader.read_count("Auth Method Count", u64::MAX)?;
        if method_count == 0 {
            return Err(WireError::InvalidValue {
                field: "Auth Method Count",
            });
        }
        let auth_methods = (0..method_count)
            .map(|_| reader.read_varint().map(AuthMethod))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            minor_version,
            host_name,
            auth_methods,
            capabilities: Capability::decode_list(reader)?,
        })
    }
}

/// An `AUTH_START` message (Section 4.4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStart {
    /// The method of this attempt.
    pub auth_method: AuthMethod,
    /// Method-specific data, at most 8192 bytes.
    pub auth_data: Vec<u8>,
}

impl MessagePayload for AuthStart {
    const MESSAGE_TYPE: MessageType = MessageType::AUTH_START;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.auth_method.0);
        writer.write_octets(&self.auth_data);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            auth_method: AuthMethod(reader.read_varint()?),
            auth_data: read_auth_data(reader)?,
        })
    }
}

/// An `AUTH_CONTINUE` message (Section 4.4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContinue {
    /// Method-specific data, at most 8192 bytes.
    pub auth_data: Vec<u8>,
}

impl MessagePayload for AuthContinue {
    const MESSAGE_TYPE: MessageType = MessageType::AUTH_CONTINUE;

    fn encode(&self, writer: &mut Writer) {
        writer.write_octets(&self.auth_data);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            auth_data: read_auth_data(reader)?,
        })
    }
}

/// An `AUTH_RESULT` message (Section 4.4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResult {
    /// The outcome of the attempt.
    pub status: AuthStatus,
    /// Method-specific data, at most 8192 bytes.
    pub auth_data: Vec<u8>,
    /// A human-readable explanation, at most 255 bytes.
    pub reason: String,
}

impl MessagePayload for AuthResult {
    const MESSAGE_TYPE: MessageType = MessageType::AUTH_RESULT;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.status.into());
        writer.write_octets(&self.auth_data);
        writer.write_string_at_most(&self.reason, MAX_NAME_LENGTH);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            status: AuthStatus::try_from(reader.read_varint()?)?,
            auth_data: read_auth_data(reader)?,
            reason: reader
                .read_string_at_most("Reason", MAX_NAME_LENGTH)?
                .to_owned(),
        })
    }
}

/// Reads an Auth Data field of at most [`MAX_AUTH_DATA_LENGTH`] bytes.
fn read_auth_data(reader: &mut Reader<'_>) -> Result<Vec<u8>, WireError> {
    reader
        .read_octets_at_most("Auth Data", MAX_AUTH_DATA_LENGTH)
        .map(<[u8]>::to_vec)
}

/// The Auth Data of an `AUTH_START` for the `PASSWORD` method (Section 4.4.8).
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordAuthData {
    /// The user's account name.
    pub user_name: String,
    /// The user's password.
    pub password: String,
}

impl PasswordAuthData {
    /// Returns the encoded Auth Data.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_string(&self.user_name);
        writer.write_string(&self.password);
        writer.into_bytes()
    }

    /// Decodes Auth Data received with the `PASSWORD` method.
    pub fn decode(auth_data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(auth_data);
        Ok(Self {
            user_name: reader.read_string()?.to_owned(),
            password: reader.read_string()?.to_owned(),
        })
    }
}

/// Shows the user name but never the password, so that it cannot reach logs.
impl std::fmt::Debug for PasswordAuthData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PasswordAuthData")
            .field("user_name", &self.user_name)
            .finish_non_exhaustive()
    }
}

/// A `CLIENT_READY` message, which has no fields (Section 4.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientReady;

impl MessagePayload for ClientReady {
    const MESSAGE_TYPE: MessageType = MessageType::CLIENT_READY;

    fn encode(&self, _writer: &mut Writer) {}

    fn decode(_reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self)
    }
}

/// A `SESSION_START` message, which has no fields (Section 4.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionStart;

impl MessagePayload for SessionStart {
    const MESSAGE_TYPE: MessageType = MessageType::SESSION_START;

    fn encode(&self, _writer: &mut Writer) {}

    fn decode(_reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self)
    }
}

/// A `RESUMPTION_TOKEN` message (Section 4.4.7).
#[derive(Clone, PartialEq, Eq)]
pub struct ResumptionToken {
    /// The opaque token, at most 256 bytes.
    pub token: Vec<u8>,
    /// How long the token remains valid after the connection ends, in whole seconds.
    pub lifetime: Duration,
}

impl MessagePayload for ResumptionToken {
    const MESSAGE_TYPE: MessageType = MessageType::RESUMPTION_TOKEN;

    fn encode(&self, writer: &mut Writer) {
        writer.write_octets(&self.token);
        writer.write_varint(VarInt::new(self.lifetime.as_secs()).unwrap_or(VarInt::MAX));
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            token: reader
                .read_octets_at_most("Token", MAX_RESUMPTION_TOKEN_LENGTH)?
                .to_vec(),
            lifetime: Duration::from_secs(reader.read_varint()?.into_inner()),
        })
    }
}

/// Shows the token's length but never its bytes, because it is a bearer credential.
impl std::fmt::Debug for ResumptionToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResumptionToken")
            .field("token_len", &self.token.len())
            .field("lifetime", &self.lifetime)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Av1Profiles, NO_LEVEL_LIMIT};
    use crate::message::{InputClasses, VideoCodec, VideoCodecCapability, VideoCodecFlags};

    /// Encodes `message` and decodes it again.
    fn round_trip<M: MessagePayload>(message: &M) -> M {
        let frame = message.to_frame();
        assert_eq!(frame.message_type, M::MESSAGE_TYPE);
        M::decode(&mut Reader::new(&frame.payload)).unwrap()
    }

    /// Returns a typical set of client capabilities.
    fn capabilities() -> Vec<Capability> {
        vec![
            Capability::VideoCodec(VideoCodecCapability {
                codec: VideoCodec::AV1,
                profiles: Av1Profiles::MAIN,
                max_bit_depth: 10,
                max_width: VarInt::from_u32(3840),
                max_height: VarInt::from_u32(2160),
                max_pixel_rate: VarInt::ZERO,
                flags: VideoCodecFlags::HARDWARE_PROFILE_0,
                max_level: NO_LEVEL_LIMIT,
            }),
            Capability::Input(InputClasses::KEYBOARD.union(InputClasses::MOUSE)),
        ]
    }

    /// Hello messages round-trip.
    #[test]
    fn hello_round_trip() {
        let client = ClientHello {
            minor_version: MinorVersion::CURRENT,
            client_name: "qdp-client/0.1.0".to_owned(),
            capabilities: capabilities(),
        };
        assert_eq!(round_trip(&client), client);

        let host = HostHello {
            minor_version: MinorVersion::CURRENT,
            host_name: String::new(),
            auth_methods: vec![AuthMethod::TOKEN, AuthMethod::SCRAM_SHA_256_PLUS],
            capabilities: capabilities(),
        };
        assert_eq!(round_trip(&host), host);
    }

    /// A `HOST_HELLO` without authentication methods is an error.
    #[test]
    fn host_hello_without_methods() {
        let host = HostHello {
            minor_version: MinorVersion::CURRENT,
            host_name: "desk".to_owned(),
            auth_methods: Vec::new(),
            capabilities: Vec::new(),
        };
        let frame = host.to_frame();
        assert_eq!(
            HostHello::decode(&mut Reader::new(&frame.payload)),
            Err(WireError::InvalidValue {
                field: "Auth Method Count"
            })
        );
    }

    /// A Client Name longer than 255 bytes is an error, and a sender cuts
    /// one that long to fit.
    #[test]
    fn client_name_too_long() {
        let name = "x".repeat(MAX_NAME_LENGTH + 1);
        let mut writer = Writer::new();
        writer.write_varint(MinorVersion::CURRENT.0);
        writer.write_string(&name);
        writer.write_varint(VarInt::ZERO);
        assert!(matches!(
            ClientHello::decode(&mut Reader::new(writer.as_bytes())),
            Err(WireError::LimitExceeded { .. })
        ));

        let client = ClientHello {
            minor_version: MinorVersion::CURRENT,
            client_name: name,
            capabilities: Vec::new(),
        };
        let sent = round_trip(&client);
        assert_eq!(sent.client_name, "x".repeat(MAX_NAME_LENGTH));
    }

    /// Authentication messages round-trip.
    #[test]
    fn auth_round_trip() {
        let start = AuthStart {
            auth_method: AuthMethod::SCRAM_SHA_256_PLUS,
            auth_data: b"p=tls-exporter,,n=user,r=nonce".to_vec(),
        };
        assert_eq!(round_trip(&start), start);

        let next = AuthContinue {
            auth_data: b"r=nonce,s=salt,i=4096".to_vec(),
        };
        assert_eq!(round_trip(&next), next);

        for status in [
            AuthStatus::Success,
            AuthStatus::FailedRetry,
            AuthStatus::FailedFinal,
            AuthStatus::NextFactor,
        ] {
            let result = AuthResult {
                status,
                auth_data: Vec::new(),
                reason: "authentication failed".to_owned(),
            };
            assert_eq!(round_trip(&result), result);
        }
    }

    /// An unknown authentication status is an error.
    #[test]
    fn unknown_auth_status() {
        let mut reader = Reader::new(&[0x04, 0x00, 0x00]);
        assert_eq!(
            AuthResult::decode(&mut reader),
            Err(WireError::InvalidValue { field: "Status" })
        );
    }

    /// Auth Data longer than 8192 bytes is an error.
    #[test]
    fn auth_data_too_long() {
        let next = AuthContinue {
            auth_data: vec![0; MAX_AUTH_DATA_LENGTH + 1],
        };
        let frame = next.to_frame();
        assert!(matches!(
            AuthContinue::decode(&mut Reader::new(&frame.payload)),
            Err(WireError::LimitExceeded { .. })
        ));
    }

    /// `PASSWORD` Auth Data round-trips and is never shown in debug output.
    #[test]
    fn password_auth_data() {
        let data = PasswordAuthData {
            user_name: "ian".to_owned(),
            password: "hunter2".to_owned(),
        };
        assert_eq!(PasswordAuthData::decode(&data.to_bytes()).unwrap(), data);
        assert!(!format!("{data:?}").contains("hunter2"));
    }

    /// The empty messages encode with an empty payload.
    #[test]
    fn empty_messages() {
        assert!(ClientReady.to_frame().payload.is_empty());
        assert!(SessionStart.to_frame().payload.is_empty());
        assert_eq!(round_trip(&ClientReady), ClientReady);
        assert_eq!(round_trip(&SessionStart), SessionStart);
    }

    /// `RESUMPTION_TOKEN` round-trips and its token never appears in debug output.
    #[test]
    fn resumption_token() {
        let token = ResumptionToken {
            token: vec![0x5a; 32],
            lifetime: Duration::from_secs(3600),
        };
        assert_eq!(round_trip(&token), token);
        assert!(!format!("{token:?}").contains("90"));

        let oversized = ResumptionToken {
            token: vec![0; MAX_RESUMPTION_TOKEN_LENGTH + 1],
            lifetime: Duration::ZERO,
        };
        let frame = oversized.to_frame();
        assert!(matches!(
            ResumptionToken::decode(&mut Reader::new(&frame.payload)),
            Err(WireError::LimitExceeded { .. })
        ));
    }
}
