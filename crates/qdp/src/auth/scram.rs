//! SCRAM-SHA-256-PLUS with `tls-exporter` channel binding (RFC 5802, RFC 7677,
//! RFC 9266), as QDP carries it in Auth Data (Section 4.4.4).
//!
//! Both sides are typestate machines: each step consumes the previous state, so
//! the steps cannot be taken out of order. Nonces are supplied by the caller,
//! which keeps this module free of a random number generator.

use ring::{digest, hmac, pbkdf2};
use thiserror::Error;

use crate::auth::base64;
use crate::auth::precis::{prepare_password, prepare_username};

/// The length of the `tls-exporter` channel binding data (RFC 9266).
pub const CHANNEL_BINDING_LENGTH: usize = 32;

/// The TLS exporter label of the channel binding data (RFC 9266).
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-Channel-Binding";

/// The lowest iteration count a host may use and a client accepts.
pub const MIN_ITERATIONS: u32 = 4096;

/// The highest iteration count a client accepts.
pub const MAX_ITERATIONS: u32 = 10_000_000;

/// The GS2 header QDP requires: channel binding with `tls-exporter` and no
/// authorization identity.
const GS2_HEADER: &str = "p=tls-exporter,,";

/// The length of SHA-256 output, and of every SCRAM key.
const KEY_LENGTH: usize = 32;

/// The length of the salt a host derives for a user name it does not know.
const DERIVED_SALT_LENGTH: usize = 16;

/// The HMAC input that derives the client key.
const CLIENT_KEY_LABEL: &[u8] = b"Client Key";

/// The HMAC input that derives the server key.
const SERVER_KEY_LABEL: &[u8] = b"Server Key";

/// The prefix of the HMAC input that derives the salt for an unknown user.
const UNKNOWN_USER_SALT_LABEL: &[u8] = b"QDP unknown user salt ";

/// The prefix of the HMAC input that derives the keys for an unknown user.
const UNKNOWN_USER_KEY_LABEL: &[u8] = b"QDP unknown user key ";

/// A SCRAM key: a SHA-256 or HMAC-SHA-256 output.
pub type Key = [u8; KEY_LENGTH];

/// The `tls-exporter` channel binding data of the connection (RFC 9266).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelBinding(pub [u8; CHANNEL_BINDING_LENGTH]);

/// Hides the binding data, which is derived from the connection's secrets.
impl std::fmt::Debug for ChannelBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ChannelBinding(..)")
    }
}

/// An error in a SCRAM exchange. Every error fails the attempt.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScramError {
    /// A message does not have the syntax of RFC 5802.
    #[error("malformed SCRAM message")]
    Malformed,
    /// The user name or password cannot be prepared with RFC 8265.
    #[error("the user name or password contains disallowed characters")]
    InvalidCredentials,
    /// The client did not use the GS2 header QDP requires.
    #[error("unsupported GS2 header")]
    UnsupportedGs2Header,
    /// The channel binding does not match this connection.
    #[error("channel binding mismatch")]
    ChannelBindingMismatch,
    /// The server's nonce does not extend the client's.
    #[error("nonce mismatch")]
    NonceMismatch,
    /// The iteration count is outside the range QDP allows. The client closes the
    /// connection with `QDP_AUTH_FAILED`.
    #[error("iteration count {0} is outside the allowed range")]
    IterationCount(u32),
    /// The client's proof is wrong: the password is wrong or the user unknown.
    #[error("authentication failed")]
    InvalidProof,
    /// The server's signature is wrong. The client closes the connection with
    /// `QDP_AUTH_FAILED`.
    #[error("the host's signature does not match")]
    InvalidServerSignature,
    /// The server reported an error.
    #[error("the host reported a SCRAM error: {0}")]
    ServerError(String),
}

/// Returns HMAC-SHA-256 of `message` with `key`.
fn hmac_sha256(key: &[u8], message: &[u8]) -> Key {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let tag = hmac::sign(&key, message);
    let mut output = [0; KEY_LENGTH];
    output.copy_from_slice(tag.as_ref());
    output
}

/// Returns SHA-256 of `message`.
fn sha256(message: &[u8]) -> Key {
    let mut output = [0; KEY_LENGTH];
    output.copy_from_slice(digest::digest(&digest::SHA256, message).as_ref());
    output
}

/// Returns the bytewise exclusive or of two keys.
fn xor(left: &Key, right: &Key) -> Key {
    std::array::from_fn(|index| left[index] ^ right[index])
}

/// Compares two keys in time independent of their contents.
fn constant_time_eq(left: &Key, right: &Key) -> bool {
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

/// Escapes a prepared user name for the `n=` attribute (RFC 5802, Section 5.1).
fn escape_username(username: &str) -> String {
    username.replace('=', "=3D").replace(',', "=2C")
}

/// Reverses [`escape_username`], failing on any other `=` sequence.
fn unescape_username(escaped: &str) -> Result<String, ScramError> {
    let mut username = String::with_capacity(escaped.len());
    let mut rest = escaped;
    while let Some(position) = rest.find('=') {
        username.push_str(&rest[..position]);
        let escape = rest
            .get(position..position + 3)
            .ok_or(ScramError::Malformed)?;
        match escape {
            "=3D" => username.push('='),
            "=2C" => username.push(','),
            _ => return Err(ScramError::Malformed),
        }
        rest = &rest[position + 3..];
    }
    username.push_str(rest);
    Ok(username)
}

/// Returns whether `nonce` is a valid SCRAM nonce: printable ASCII without
/// commas.
fn is_valid_nonce(nonce: &str) -> bool {
    !nonce.is_empty()
        && nonce
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b',')
}

/// Splits a SCRAM message into its attributes, requiring the given names in
/// order. Attributes after them are ignored.
fn attributes<const N: usize>(message: &str, names: [char; N]) -> Result<[&str; N], ScramError> {
    let mut parts = message.split(',');
    let mut values = [""; N];
    for (value, name) in values.iter_mut().zip(names) {
        let part = parts.next().ok_or(ScramError::Malformed)?;
        *value = part
            .strip_prefix(name)
            .and_then(|part| part.strip_prefix('='))
            .ok_or(ScramError::Malformed)?;
    }
    Ok(values)
}

/// Returns the value of the `c=` attribute: the GS2 header followed by the
/// channel binding data, in Base64.
fn channel_binding_attribute(gs2_header: &str, binding: &[u8]) -> String {
    let mut data = gs2_header.as_bytes().to_vec();
    data.extend_from_slice(binding);
    base64::encode(&data)
}

/// The keys a host stores for one user instead of the password (RFC 5802,
/// Section 3).
#[derive(Clone, PartialEq, Eq)]
pub struct ScramCredentials {
    /// The user's salt.
    pub salt: Vec<u8>,
    /// The PBKDF2 iteration count, at least [`MIN_ITERATIONS`].
    pub iterations: u32,
    /// SHA-256 of the client key.
    pub stored_key: Key,
    /// The server key.
    pub server_key: Key,
}

/// Hides the keys, which allow offline guessing of the password.
impl std::fmt::Debug for ScramCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScramCredentials")
            .field("iterations", &self.iterations)
            .finish_non_exhaustive()
    }
}

impl ScramCredentials {
    /// Derives the stored credentials for `password`, for example when an
    /// administrator sets a user's password.
    pub fn derive(password: &str, salt: Vec<u8>, iterations: u32) -> Result<Self, ScramError> {
        if iterations < MIN_ITERATIONS {
            return Err(ScramError::IterationCount(iterations));
        }
        let salted_password = salted_password(password, &salt, iterations)?;
        let client_key = hmac_sha256(&salted_password, CLIENT_KEY_LABEL);
        Ok(Self {
            salt,
            iterations,
            stored_key: sha256(&client_key),
            server_key: hmac_sha256(&salted_password, SERVER_KEY_LABEL),
        })
    }

    /// Returns credentials for a user name the host does not know, derived from
    /// `host_secret` so that every attempt for that name sees the same salt
    /// (Section 4.4.4). No password matches them.
    pub fn for_unknown_user(username: &str, host_secret: &[u8], iterations: u32) -> Self {
        let salt_input = [UNKNOWN_USER_SALT_LABEL, username.as_bytes()].concat();
        let key_input = [UNKNOWN_USER_KEY_LABEL, username.as_bytes()].concat();
        let key = hmac_sha256(host_secret, &key_input);
        Self {
            salt: hmac_sha256(host_secret, &salt_input)[..DERIVED_SALT_LENGTH].to_vec(),
            iterations,
            stored_key: sha256(&key),
            server_key: key,
        }
    }
}

/// Returns PBKDF2-HMAC-SHA-256 of the prepared password.
fn salted_password(password: &str, salt: &[u8], iterations: u32) -> Result<Key, ScramError> {
    let password = prepare_password(password).map_err(|_| ScramError::InvalidCredentials)?;
    let iterations =
        std::num::NonZeroU32::new(iterations).ok_or(ScramError::IterationCount(iterations))?;
    let mut output = [0; KEY_LENGTH];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        password.as_bytes(),
        &mut output,
    );
    Ok(output)
}

/// A client that has sent its client-first-message.
#[derive(Debug)]
pub struct ScramClient {
    /// The prepared password.
    password: String,
    /// The GS2 header sent.
    gs2_header: &'static str,
    /// The channel binding data of the connection.
    binding: Vec<u8>,
    /// The client's nonce.
    client_nonce: String,
    /// The client-first-message without its GS2 header.
    client_first_bare: String,
}

/// A client that has sent its client-final-message and waits for the server's
/// signature.
#[derive(Debug)]
pub struct ScramClientFinal {
    /// The signature the server must send.
    expected_server_signature: Key,
}

impl ScramClient {
    /// Starts an exchange and returns the client-first-message for `AUTH_START`.
    ///
    /// `nonce` must hold at least 18 bytes from a cryptographically secure random
    /// number generator.
    pub fn new(
        username: &str,
        password: &str,
        nonce: &[u8],
        binding: ChannelBinding,
    ) -> Result<(Self, Vec<u8>), ScramError> {
        Self::with_gs2_header(
            username,
            password,
            &base64::encode(nonce),
            GS2_HEADER,
            &binding.0,
        )
    }

    /// Starts an exchange with any GS2 header and binding, for testing against the
    /// vectors of RFC 7677.
    fn with_gs2_header(
        username: &str,
        password: &str,
        client_nonce: &str,
        gs2_header: &'static str,
        binding: &[u8],
    ) -> Result<(Self, Vec<u8>), ScramError> {
        let username = prepare_username(username).map_err(|_| ScramError::InvalidCredentials)?;
        let password = prepare_password(password).map_err(|_| ScramError::InvalidCredentials)?;
        let client_first_bare = format!("n={},r={client_nonce}", escape_username(&username));
        let message = format!("{gs2_header}{client_first_bare}").into_bytes();
        let client = Self {
            password,
            gs2_header,
            binding: binding.to_vec(),
            client_nonce: client_nonce.to_owned(),
            client_first_bare,
        };
        Ok((client, message))
    }

    /// Processes the server-first-message from `AUTH_CONTINUE` and returns the
    /// client-final-message to send in `AUTH_CONTINUE`.
    pub fn handle_server_first(
        self,
        server_first: &[u8],
    ) -> Result<(ScramClientFinal, Vec<u8>), ScramError> {
        let server_first = std::str::from_utf8(server_first).map_err(|_| ScramError::Malformed)?;
        let [nonce, salt, iterations] = attributes(server_first, ['r', 's', 'i'])?;
        if !nonce.starts_with(&self.client_nonce)
            || nonce.len() == self.client_nonce.len()
            || !is_valid_nonce(nonce)
        {
            return Err(ScramError::NonceMismatch);
        }
        let salt = base64::decode(salt).ok_or(ScramError::Malformed)?;
        let iterations: u32 = iterations.parse().map_err(|_| ScramError::Malformed)?;
        if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
            return Err(ScramError::IterationCount(iterations));
        }

        let salted_password = salted_password(&self.password, &salt, iterations)?;
        let client_key = hmac_sha256(&salted_password, CLIENT_KEY_LABEL);
        let stored_key = sha256(&client_key);
        let server_key = hmac_sha256(&salted_password, SERVER_KEY_LABEL);

        let without_proof = format!(
            "c={},r={nonce}",
            channel_binding_attribute(self.gs2_header, &self.binding)
        );
        let auth_message = format!("{},{server_first},{without_proof}", self.client_first_bare);
        let client_signature = hmac_sha256(&stored_key, auth_message.as_bytes());
        let proof = xor(&client_key, &client_signature);
        let message = format!("{without_proof},p={}", base64::encode(&proof)).into_bytes();
        let next = ScramClientFinal {
            expected_server_signature: hmac_sha256(&server_key, auth_message.as_bytes()),
        };
        Ok((next, message))
    }
}

impl ScramClientFinal {
    /// Verifies the server-final-message from a successful `AUTH_RESULT`. On
    /// failure the client closes the connection with `QDP_AUTH_FAILED`.
    pub fn verify_server_final(self, server_final: &[u8]) -> Result<(), ScramError> {
        let server_final = std::str::from_utf8(server_final).map_err(|_| ScramError::Malformed)?;
        if let Some(error) = server_final.strip_prefix("e=") {
            return Err(ScramError::ServerError(error.to_owned()));
        }
        let [signature] = attributes(server_final, ['v'])?;
        let signature = base64::decode(signature).ok_or(ScramError::Malformed)?;
        let signature: Key = signature
            .try_into()
            .map_err(|_| ScramError::InvalidServerSignature)?;
        if constant_time_eq(&signature, &self.expected_server_signature) {
            Ok(())
        } else {
            Err(ScramError::InvalidServerSignature)
        }
    }
}

/// The parts of a client-first-message the host needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientFirst {
    /// The prepared user name.
    pub username: String,
    /// The client's nonce.
    client_nonce: String,
    /// The client-first-message without its GS2 header.
    bare: String,
    /// The GS2 header the client sent.
    gs2_header: String,
}

impl ClientFirst {
    /// Parses the client-first-message from `AUTH_START`, requiring the GS2 header
    /// `p=tls-exporter,,`.
    pub fn parse(client_first: &[u8]) -> Result<Self, ScramError> {
        let message = std::str::from_utf8(client_first).map_err(|_| ScramError::Malformed)?;
        let bare = message
            .strip_prefix(GS2_HEADER)
            .ok_or(ScramError::UnsupportedGs2Header)?;
        Self::parse_bare(GS2_HEADER, bare)
    }

    /// Parses a client-first-message without its GS2 header.
    fn parse_bare(gs2_header: &str, bare: &str) -> Result<Self, ScramError> {
        let [username, client_nonce] = attributes(bare, ['n', 'r'])?;
        if !is_valid_nonce(client_nonce) {
            return Err(ScramError::Malformed);
        }
        let username = prepare_username(&unescape_username(username)?)
            .map_err(|_| ScramError::InvalidCredentials)?;
        Ok(Self {
            username,
            client_nonce: client_nonce.to_owned(),
            bare: bare.to_owned(),
            gs2_header: gs2_header.to_owned(),
        })
    }
}

/// A host that has sent its server-first-message.
#[derive(Debug)]
pub struct ScramServer {
    /// The user's stored credentials, or derived ones for an unknown user.
    credentials: ScramCredentials,
    /// Whether the user is known; unknown users always fail at the last step.
    known_user: bool,
    /// The channel binding data of the connection.
    binding: Vec<u8>,
    /// The client's first message.
    client_first: ClientFirst,
    /// The server-first-message sent.
    server_first: String,
    /// The combined nonce.
    nonce: String,
}

impl ScramServer {
    /// Answers a parsed client-first-message and returns the server-first-message
    /// for `AUTH_CONTINUE`.
    ///
    /// `credentials` is the user's stored credentials, or `None` for a user name
    /// the host does not know, in which case credentials derived from
    /// `host_secret` stand in so that the exchange looks the same until it fails at
    /// the last step. `nonce` must hold at least 18 bytes from a cryptographically
    /// secure random number generator.
    pub fn new(
        client_first: ClientFirst,
        credentials: Option<ScramCredentials>,
        host_secret: &[u8],
        default_iterations: u32,
        nonce: &[u8],
        binding: ChannelBinding,
    ) -> (Self, Vec<u8>) {
        Self::with_server_nonce(
            client_first,
            credentials,
            host_secret,
            default_iterations,
            &base64::encode(nonce),
            &binding.0,
        )
    }

    /// Answers with any server nonce and binding, for testing against the vectors
    /// of RFC 7677.
    fn with_server_nonce(
        client_first: ClientFirst,
        credentials: Option<ScramCredentials>,
        host_secret: &[u8],
        default_iterations: u32,
        server_nonce: &str,
        binding: &[u8],
    ) -> (Self, Vec<u8>) {
        let known_user = credentials.is_some();
        let credentials = credentials.unwrap_or_else(|| {
            ScramCredentials::for_unknown_user(
                &client_first.username,
                host_secret,
                default_iterations,
            )
        });
        let nonce = format!("{}{server_nonce}", client_first.client_nonce);
        let server_first = format!(
            "r={nonce},s={},i={}",
            base64::encode(&credentials.salt),
            credentials.iterations
        );
        let message = server_first.clone().into_bytes();
        let server = Self {
            credentials,
            known_user,
            binding: binding.to_vec(),
            client_first,
            server_first,
            nonce,
        };
        (server, message)
    }

    /// Verifies the client-final-message from `AUTH_CONTINUE` and returns the
    /// server-final-message for a successful `AUTH_RESULT`. On error the host
    /// answers with a failed `AUTH_RESULT` carrying no Auth Data.
    pub fn handle_client_final(self, client_final: &[u8]) -> Result<Vec<u8>, ScramError> {
        let client_final = std::str::from_utf8(client_final).map_err(|_| ScramError::Malformed)?;
        let [binding, nonce] = attributes(client_final, ['c', 'r'])?;
        let (without_proof, proof) = client_final
            .rsplit_once(",p=")
            .ok_or(ScramError::Malformed)?;
        let expected_binding =
            channel_binding_attribute(&self.client_first.gs2_header, &self.binding);
        if binding != expected_binding {
            return Err(ScramError::ChannelBindingMismatch);
        }
        if nonce != self.nonce {
            return Err(ScramError::NonceMismatch);
        }
        let proof: Key = base64::decode(proof)
            .ok_or(ScramError::Malformed)?
            .try_into()
            .map_err(|_| ScramError::InvalidProof)?;

        let auth_message = format!(
            "{},{},{without_proof}",
            self.client_first.bare, self.server_first
        );
        let client_signature = hmac_sha256(&self.credentials.stored_key, auth_message.as_bytes());
        let client_key = xor(&proof, &client_signature);
        let proof_matches = constant_time_eq(&sha256(&client_key), &self.credentials.stored_key);
        if !(proof_matches && self.known_user) {
            return Err(ScramError::InvalidProof);
        }
        let server_signature = hmac_sha256(&self.credentials.server_key, auth_message.as_bytes());
        Ok(format!("v={}", base64::encode(&server_signature)).into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client nonce of the RFC 7677 example.
    const RFC7677_CLIENT_NONCE: &str = "rOprNGfwEbeRWgbNEkqO";

    /// The server nonce suffix of the RFC 7677 example.
    const RFC7677_SERVER_NONCE: &str = "%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";

    /// The salt of the RFC 7677 example.
    const RFC7677_SALT: &str = "W22ZaJ0SNY7soEsUEjb6gQ==";

    /// Returns a channel binding for tests.
    fn binding(byte: u8) -> ChannelBinding {
        ChannelBinding([byte; CHANNEL_BINDING_LENGTH])
    }

    /// The client reproduces the exchange of RFC 7677, Section 3.
    #[test]
    fn rfc7677_client() {
        let (client, client_first) =
            ScramClient::with_gs2_header("user", "pencil", RFC7677_CLIENT_NONCE, "n,,", &[])
                .unwrap();
        assert_eq!(client_first, b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO");

        let server_first =
            format!("r={RFC7677_CLIENT_NONCE}{RFC7677_SERVER_NONCE},s={RFC7677_SALT},i=4096");
        let (client, client_final) = client.handle_server_first(server_first.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(client_final).unwrap(),
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
             p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        client
            .verify_server_final(b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
            .unwrap();
    }

    /// The server reproduces the exchange of RFC 7677, Section 3.
    #[test]
    fn rfc7677_server() {
        let salt = base64::decode(RFC7677_SALT).unwrap();
        let credentials = ScramCredentials::derive("pencil", salt, 4096).unwrap();
        let client_first =
            ClientFirst::parse_bare("n,,", &format!("n=user,r={RFC7677_CLIENT_NONCE}")).unwrap();
        let (server, server_first) = ScramServer::with_server_nonce(
            client_first,
            Some(credentials),
            b"secret",
            4096,
            RFC7677_SERVER_NONCE,
            &[],
        );
        assert_eq!(
            String::from_utf8(server_first).unwrap(),
            format!("r={RFC7677_CLIENT_NONCE}{RFC7677_SERVER_NONCE},s={RFC7677_SALT},i=4096")
        );
        let server_final = server
            .handle_client_final(
                b"c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
                  p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=",
            )
            .unwrap();
        assert_eq!(
            server_final,
            b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="
        );
    }

    /// Runs an exchange with the QDP GS2 header and returns the server's verdict.
    fn exchange(
        password: &str,
        client_binding: ChannelBinding,
        server_binding: ChannelBinding,
        known_user: bool,
    ) -> Result<(), ScramError> {
        let stored = ScramCredentials::derive("correct horse", b"salt".to_vec(), 4096).unwrap();
        let (client, client_first) =
            ScramClient::new("Ian", password, &[7; 18], client_binding).unwrap();
        let parsed = ClientFirst::parse(&client_first)?;
        assert_eq!(parsed.username, "ian");
        let (server, server_first) = ScramServer::new(
            parsed,
            known_user.then_some(stored),
            b"host secret",
            4096,
            &[9; 18],
            server_binding,
        );
        let (client, client_final) = client.handle_server_first(&server_first)?;
        let server_final = server.handle_client_final(&client_final)?;
        client.verify_server_final(&server_final)
    }

    /// A full exchange with channel binding succeeds with the right password.
    #[test]
    fn successful_exchange() {
        exchange("correct horse", binding(1), binding(1), true).unwrap();
    }

    /// A wrong password, an unknown user and a relayed exchange all fail.
    #[test]
    fn failed_exchanges() {
        assert_eq!(
            exchange("wrong", binding(1), binding(1), true),
            Err(ScramError::InvalidProof)
        );
        assert_eq!(
            exchange("correct horse", binding(1), binding(1), false),
            Err(ScramError::InvalidProof)
        );
        assert_eq!(
            exchange("correct horse", binding(1), binding(2), true),
            Err(ScramError::ChannelBindingMismatch)
        );
    }

    /// Unknown users see the same salt on every attempt.
    #[test]
    fn unknown_user_salt_is_stable() {
        let first = ScramCredentials::for_unknown_user("nobody", b"secret", 4096);
        let second = ScramCredentials::for_unknown_user("nobody", b"secret", 4096);
        let other = ScramCredentials::for_unknown_user("somebody", b"secret", 4096);
        assert_eq!(first.salt, second.salt);
        assert_ne!(first.salt, other.salt);
        assert_eq!(first.salt.len(), DERIVED_SALT_LENGTH);
    }

    /// The host requires the `tls-exporter` GS2 header.
    #[test]
    fn requires_channel_binding() {
        assert_eq!(
            ClientFirst::parse(b"n,,n=user,r=abc"),
            Err(ScramError::UnsupportedGs2Header)
        );
        assert_eq!(
            ClientFirst::parse(b"p=tls-unique,,n=user,r=abc"),
            Err(ScramError::UnsupportedGs2Header)
        );
    }

    /// The client rejects iteration counts outside the allowed range and nonces
    /// that do not extend its own.
    #[test]
    fn client_checks_server_first() {
        let start = || {
            ScramClient::new("user", "pencil", &[1; 18], binding(0))
                .unwrap()
                .0
        };
        let nonce = base64::encode(&[1; 18]);
        assert_eq!(
            start()
                .handle_server_first(format!("r={nonce}x,s=c2FsdA==,i=1000").as_bytes())
                .unwrap_err(),
            ScramError::IterationCount(1000)
        );
        assert_eq!(
            start()
                .handle_server_first(b"r=other,s=c2FsdA==,i=4096")
                .unwrap_err(),
            ScramError::NonceMismatch
        );
        assert_eq!(
            start()
                .handle_server_first(format!("r={nonce},s=c2FsdA==,i=4096").as_bytes())
                .unwrap_err(),
            ScramError::NonceMismatch
        );
    }

    /// The client rejects a wrong server signature and reports server errors.
    #[test]
    fn client_checks_server_final() {
        let (client, _) = ScramClient::new("user", "pencil", &[1; 18], binding(0)).unwrap();
        let nonce = base64::encode(&[1; 18]);
        let (client, _) = client
            .handle_server_first(format!("r={nonce}x,s=c2FsdA==,i=4096").as_bytes())
            .unwrap();
        assert_eq!(
            client.verify_server_final(b"e=invalid-proof"),
            Err(ScramError::ServerError("invalid-proof".to_owned()))
        );
    }

    /// User names with `=` and `,` are escaped and unescaped.
    #[test]
    fn username_escaping() {
        assert_eq!(escape_username("a=b,c"), "a=3Db=2Cc");
        assert_eq!(unescape_username("a=3Db=2Cc").unwrap(), "a=b,c");
        assert_eq!(unescape_username("a=4"), Err(ScramError::Malformed));
    }

    /// Stored credentials never show their keys in debug output.
    #[test]
    fn credentials_debug_hides_keys() {
        let credentials = ScramCredentials::derive("pencil", b"salt".to_vec(), 4096).unwrap();
        let text = format!("{credentials:?}");
        assert!(text.contains("4096"));
        assert!(!text.contains("stored_key"));
    }
}
