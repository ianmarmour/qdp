//! Authentication policy for hosts and credentials for clients (Section 4.4).

use std::collections::HashMap;

use qdp::auth::precis::prepare_username;
use qdp::auth::scram::{
    ChannelBinding, ClientFirst, ScramClient, ScramClientFinal, ScramCredentials, ScramServer,
};
use qdp::message::{AuthMethod, AuthResult, AuthStatus};
use qdp::session::AuthResponse;
use ring::rand::{SecureRandom, SystemRandom};
use tracing::{debug, info};

use crate::RuntimeError;

/// The number of random bytes in a SCRAM nonce.
const NONCE_LENGTH: usize = 18;

/// The length of the secret from which credentials for unknown users derive.
const HOST_SECRET_LENGTH: usize = 32;

/// The PBKDF2 iteration count of credentials derived from passwords, which
/// OWASP recommends for PBKDF2-HMAC-SHA256.
const PASSWORD_ITERATIONS: u32 = 600_000;

/// The iteration count of the stand-in credentials for unknown users, the same
/// as for known users so that the two cannot be told apart.
const UNKNOWN_USER_ITERATIONS: u32 = PASSWORD_ITERATIONS;

/// The length of a random salt.
const SALT_LENGTH: usize = 16;

/// Returns `length` bytes from the system's secure random number generator.
fn random_bytes<const LENGTH: usize>() -> Result<[u8; LENGTH], RuntimeError> {
    let mut bytes = [0; LENGTH];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| RuntimeError::Random)?;
    Ok(bytes)
}

/// Compares two byte strings in time that depends only on their lengths.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Which clients a host accepts.
#[derive(Debug, Clone)]
pub struct HostAuthenticator {
    /// Tokens accepted with the `TOKEN` method.
    tokens: Vec<Vec<u8>>,
    /// SCRAM credentials by prepared user name.
    users: HashMap<String, ScramCredentials>,
    /// The secret from which credentials for unknown user names derive.
    secret: [u8; HOST_SECRET_LENGTH],
}

impl HostAuthenticator {
    /// Creates an authenticator that accepts no one.
    pub fn new() -> Result<Self, RuntimeError> {
        Ok(Self {
            tokens: Vec::new(),
            users: HashMap::new(),
            secret: random_bytes()?,
        })
    }

    /// Accepts `token` with the `TOKEN` method.
    pub fn add_token(&mut self, token: Vec<u8>) {
        self.tokens.push(token);
    }

    /// Accepts the user `username` with SCRAM-SHA-256-PLUS. The name must already
    /// be prepared with RFC 8265, as [`HostAuthenticator::add_password`] does.
    pub fn add_user(&mut self, username: String, credentials: ScramCredentials) {
        self.users.insert(username, credentials);
    }

    /// Accepts `username` with `password` over SCRAM-SHA-256-PLUS, deriving the
    /// stored credentials with a random salt. Only the credentials are kept, not
    /// the password.
    pub fn add_password(&mut self, username: &str, password: &str) -> Result<(), RuntimeError> {
        let username = prepare_username(username).map_err(|_| RuntimeError::InvalidUsername)?;
        let salt = random_bytes::<SALT_LENGTH>()?.to_vec();
        let credentials = ScramCredentials::derive(password, salt, PASSWORD_ITERATIONS)?;
        self.users.insert(username, credentials);
        Ok(())
    }

    /// Returns the methods to offer, in order of preference.
    pub fn methods(&self) -> Vec<AuthMethod> {
        let mut methods = Vec::new();
        if !self.tokens.is_empty() {
            methods.push(AuthMethod::TOKEN);
        }
        if !self.users.is_empty() {
            methods.push(AuthMethod::SCRAM_SHA_256_PLUS);
        }
        methods
    }
}

/// The host's side of one connection's authentication attempts.
#[derive(Debug)]
pub(crate) struct HostAuthExchange<'a> {
    /// The policy.
    authenticator: &'a HostAuthenticator,
    /// The connection's channel binding.
    binding: ChannelBinding,
    /// A SCRAM exchange waiting for the client-final-message.
    scram: Option<ScramServer>,
}

impl<'a> HostAuthExchange<'a> {
    /// Starts authentication on a connection with `binding`.
    pub(crate) fn new(authenticator: &'a HostAuthenticator, binding: ChannelBinding) -> Self {
        Self {
            authenticator,
            binding,
            scram: None,
        }
    }

    /// Answers `AUTH_START`.
    pub(crate) fn start(&mut self, method: AuthMethod, auth_data: &[u8]) -> AuthResponse {
        match method {
            AuthMethod::TOKEN => {
                let accepted = self
                    .authenticator
                    .tokens
                    .iter()
                    .any(|token| constant_time_eq(token, auth_data));
                info!(accepted, "token authentication");
                if accepted {
                    AuthResponse::Accept(Vec::new())
                } else {
                    AuthResponse::Reject
                }
            }
            AuthMethod::SCRAM_SHA_256_PLUS => self.start_scram(auth_data),
            _ => AuthResponse::Reject,
        }
    }

    /// Answers a SCRAM client-first-message.
    fn start_scram(&mut self, client_first: &[u8]) -> AuthResponse {
        let Ok(first) = ClientFirst::parse(client_first) else {
            return AuthResponse::Reject;
        };
        let Ok(nonce) = random_bytes::<NONCE_LENGTH>() else {
            return AuthResponse::Reject;
        };
        let credentials = self.authenticator.users.get(&first.username).cloned();
        debug!(known = credentials.is_some(), "SCRAM authentication");
        let (server, server_first) = ScramServer::new(
            first,
            credentials,
            &self.authenticator.secret,
            UNKNOWN_USER_ITERATIONS,
            &nonce,
            self.binding,
        );
        self.scram = Some(server);
        AuthResponse::Continue(server_first)
    }

    /// Answers `AUTH_CONTINUE`.
    pub(crate) fn continue_exchange(&mut self, auth_data: &[u8]) -> AuthResponse {
        let Some(server) = self.scram.take() else {
            return AuthResponse::Reject;
        };
        match server.handle_client_final(auth_data) {
            Ok(server_final) => {
                info!("SCRAM authentication succeeded");
                AuthResponse::Accept(server_final)
            }
            Err(error) => {
                info!(%error, "SCRAM authentication failed");
                AuthResponse::Reject
            }
        }
    }
}

/// How a client proves its identity.
#[derive(Clone, PartialEq, Eq)]
pub enum ClientCredentials {
    /// A token for the `TOKEN` method.
    Token(Vec<u8>),
    /// A user name and password for SCRAM-SHA-256-PLUS.
    Password {
        /// The user name.
        username: String,
        /// The password.
        password: String,
    },
}

/// Hides the secrets.
impl std::fmt::Debug for ClientCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token(_) => formatter.write_str("Token(..)"),
            Self::Password { username, .. } => formatter
                .debug_struct("Password")
                .field("username", username)
                .finish_non_exhaustive(),
        }
    }
}

/// The state of the client's authentication.
#[derive(Debug)]
enum ClientAuthState {
    /// No attempt has started.
    Idle,
    /// A token was sent.
    Token,
    /// The client-first-message was sent.
    ScramFirst(ScramClient),
    /// The client-final-message was sent.
    ScramFinal(ScramClientFinal),
}

/// The client's side of authentication.
#[derive(Debug)]
pub(crate) struct ClientAuthExchange {
    /// The credentials.
    credentials: ClientCredentials,
    /// The connection's channel binding.
    binding: ChannelBinding,
    /// The progress of the exchange.
    state: ClientAuthState,
}

impl ClientAuthExchange {
    /// Prepares authentication with `credentials` on a connection with `binding`.
    pub(crate) fn new(credentials: ClientCredentials, binding: ChannelBinding) -> Self {
        Self {
            credentials,
            binding,
            state: ClientAuthState::Idle,
        }
    }

    /// Returns the method and Auth Data of `AUTH_START`, given the methods the host
    /// offers.
    pub(crate) fn start(
        &mut self,
        offered: &[AuthMethod],
    ) -> Result<(AuthMethod, Vec<u8>), RuntimeError> {
        match &self.credentials {
            ClientCredentials::Token(token) if offered.contains(&AuthMethod::TOKEN) => {
                self.state = ClientAuthState::Token;
                Ok((AuthMethod::TOKEN, token.clone()))
            }
            ClientCredentials::Password { username, password }
                if offered.contains(&AuthMethod::SCRAM_SHA_256_PLUS) =>
            {
                let nonce = random_bytes::<NONCE_LENGTH>()?;
                let (client, first) = ScramClient::new(username, password, &nonce, self.binding)?;
                self.state = ClientAuthState::ScramFirst(client);
                Ok((AuthMethod::SCRAM_SHA_256_PLUS, first))
            }
            _ => Err(RuntimeError::NoCommonAuthMethod),
        }
    }

    /// Returns the Auth Data answering the host's `AUTH_CONTINUE`.
    pub(crate) fn continue_exchange(&mut self, auth_data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
        match std::mem::replace(&mut self.state, ClientAuthState::Idle) {
            ClientAuthState::ScramFirst(client) => {
                let (next, client_final) = client.handle_server_first(auth_data)?;
                self.state = ClientAuthState::ScramFinal(next);
                Ok(client_final)
            }
            _ => Err(RuntimeError::UnexpectedAuthStep),
        }
    }

    /// Checks the host's `AUTH_RESULT`, verifying the host's SCRAM signature on
    /// success.
    pub(crate) fn finish(&mut self, result: &AuthResult) -> Result<(), RuntimeError> {
        let state = std::mem::replace(&mut self.state, ClientAuthState::Idle);
        if result.status != AuthStatus::Success {
            return Err(RuntimeError::AuthenticationFailed(result.reason.clone()));
        }
        if let ClientAuthState::ScramFinal(client) = state {
            client.verify_server_final(&result.auth_data)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a channel binding for tests.
    fn binding() -> ChannelBinding {
        ChannelBinding([7; 32])
    }

    /// Tokens are accepted only when they match exactly.
    #[test]
    fn tokens() {
        let mut authenticator = HostAuthenticator::new().unwrap();
        authenticator.add_token(b"secret".to_vec());
        assert_eq!(authenticator.methods(), [AuthMethod::TOKEN]);
        let mut exchange = HostAuthExchange::new(&authenticator, binding());
        assert_eq!(
            exchange.start(AuthMethod::TOKEN, b"secret"),
            AuthResponse::Accept(Vec::new())
        );
        assert_eq!(
            exchange.start(AuthMethod::TOKEN, b"secrets"),
            AuthResponse::Reject
        );
    }

    /// A client and host complete a SCRAM exchange, and a wrong password fails.
    #[test]
    fn scram_exchange() {
        let mut authenticator = HostAuthenticator::new().unwrap();
        authenticator.add_user(
            "ian".to_owned(),
            ScramCredentials::derive("pencil", b"salt".to_vec(), 4096).unwrap(),
        );
        for (password, succeeds) in [("pencil", true), ("wrong", false)] {
            let mut host = HostAuthExchange::new(&authenticator, binding());
            let mut client = ClientAuthExchange::new(
                ClientCredentials::Password {
                    username: "ian".to_owned(),
                    password: password.to_owned(),
                },
                binding(),
            );
            let (method, first) = client.start(&authenticator.methods()).unwrap();
            let AuthResponse::Continue(server_first) = host.start(method, &first) else {
                panic!("expected a server-first-message");
            };
            let client_final = client.continue_exchange(&server_first).unwrap();
            let result = match host.continue_exchange(&client_final) {
                AuthResponse::Accept(auth_data) => AuthResult {
                    status: AuthStatus::Success,
                    auth_data,
                    reason: String::new(),
                },
                _ => AuthResult {
                    status: AuthStatus::FailedRetry,
                    auth_data: Vec::new(),
                    reason: "authentication failed".to_owned(),
                },
            };
            assert_eq!(client.finish(&result).is_ok(), succeeds, "{password}");
        }
    }

    /// A client without credentials for any offered method cannot start.
    #[test]
    fn no_common_method() {
        let mut client =
            ClientAuthExchange::new(ClientCredentials::Token(b"t".to_vec()), binding());
        assert!(matches!(
            client.start(&[AuthMethod::SCRAM_SHA_256_PLUS]),
            Err(RuntimeError::NoCommonAuthMethod)
        ));
        assert_eq!(
            format!("{:?}", ClientCredentials::Token(b"secret".to_vec())),
            "Token(..)"
        );
    }
}
