//! Authentication policy for hosts and credentials for clients (Section 4.4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use qdp::auth::pairing::{ClientPairing, HostPairing, PairingCode, PairingKeys, PairingStart};

use qdp::auth::precis::prepare_username;
use qdp::auth::scram::{
    ChannelBinding, ClientFirst, ScramClient, ScramClientFinal, ScramCredentials, ScramServer,
};
use qdp::message::{AuthMethod, AuthResult, AuthStatus, PasswordAuthData, ResumptionToken};
use qdp::session::AuthResponse;
use qdp_quic::KeyPin;
use ring::rand::{SecureRandom, SystemRandom};
use tracing::{debug, info};

use crate::RuntimeError;
use crate::admission::AdmissionTicket;
use crate::pairing::{CODE_LIFETIME, Pairing, PairingOutcome, PairingSlot};
use crate::resumption::{
    ClientIdentity, DEFAULT_RESUMPTION_LIFETIME, IssuedToken, ResumptionTokens,
};

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
pub(crate) fn random_bytes<const LENGTH: usize>() -> Result<[u8; LENGTH], RuntimeError> {
    let mut bytes = [0; LENGTH];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| RuntimeError::Random)?;
    Ok(bytes)
}

/// Compares two byte strings in time that depends only on their lengths.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Which clients a host accepts. Clones share the resumption tokens they
/// issue.
#[derive(Debug, Clone)]
pub struct HostAuthenticator {
    /// Tokens accepted with the `TOKEN` method.
    tokens: Vec<Vec<u8>>,
    /// SCRAM credentials by prepared user name.
    users: HashMap<String, ScramCredentials>,
    /// The keys of the client certificates accepted with the
    /// `CLIENT_CERTIFICATE` method, which clones share, so that a client that
    /// pairs in one session is accepted in the next.
    client_keys: Arc<Mutex<Vec<KeyPin>>>,
    /// The pairing policy, if the host pairs with clients (Section 4.4.9).
    pairing: Option<Pairing>,
    /// The secret from which credentials for unknown user names derive.
    secret: [u8; HOST_SECRET_LENGTH],
    /// The resumption tokens issued to sessions (Section 4.4.7).
    resumption: ResumptionTokens,
    /// How long resumption tokens remain valid after their connection ends,
    /// or `None` if the host issues none.
    resumption_lifetime: Option<Duration>,
}

impl HostAuthenticator {
    /// Creates an authenticator that accepts no one, and that gives the
    /// clients it accepts resumption tokens valid for
    /// [`DEFAULT_RESUMPTION_LIFETIME`].
    pub fn new() -> Result<Self, RuntimeError> {
        Ok(Self {
            tokens: Vec::new(),
            users: HashMap::new(),
            client_keys: Arc::default(),
            pairing: None,
            secret: random_bytes()?,
            resumption: ResumptionTokens::default(),
            resumption_lifetime: Some(DEFAULT_RESUMPTION_LIFETIME),
        })
    }

    /// Sets how long the resumption tokens given to clients remain valid
    /// after their connection ends, or with `None` stops giving them
    /// (Section 4.4.7).
    pub fn set_resumption_lifetime(&mut self, lifetime: Option<Duration>) {
        self.resumption_lifetime = lifetime;
    }

    /// Accepts `token` with the `TOKEN` method.
    pub fn add_token(&mut self, token: Vec<u8>) {
        self.tokens.push(token);
    }

    /// Accepts clients presenting a certificate whose key hashes to `pin`,
    /// with the `CLIENT_CERTIFICATE` method (Section 4.4.2).
    pub fn add_client_key(&mut self, pin: KeyPin) {
        self.authorize_client_key(pin);
    }

    /// Pairs with clients as `pairing` decides, accepting each paired
    /// client's key from then on (Section 4.4.9).
    pub fn enable_pairing(&mut self, pairing: Pairing) {
        self.pairing = Some(pairing);
    }

    /// Accepts `pin` with `CLIENT_CERTIFICATE`, in every clone.
    fn authorize_client_key(&self, pin: KeyPin) {
        let mut keys = self.client_keys();
        if !keys.contains(&pin) {
            keys.push(pin);
        }
    }

    /// Locks the accepted client keys.
    fn client_keys(&self) -> std::sync::MutexGuard<'_, Vec<KeyPin>> {
        self.client_keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns whether a client presenting a certificate with `pin` is
    /// accepted with `CLIENT_CERTIFICATE`.
    fn accepts_client_key(&self, pin: &KeyPin) -> bool {
        self.client_keys().contains(pin)
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

    /// Returns whether the host accepts anyone at all: a token, a user, a
    /// client key, or a client that pairs.
    pub fn accepts_anyone(&self) -> bool {
        !self.tokens.is_empty()
            || !self.users.is_empty()
            || !self.client_keys().is_empty()
            || self.pairing.is_some()
    }

    /// Returns the methods to offer, in order of preference, on a connection
    /// whose client presented a certificate with `client_key`, if any:
    /// `CLIENT_CERTIFICATE` and `PAIRING` only to a client that presented
    /// one (Sections 4.4.2 and 4.4.9), and `TOKEN` also for resumption
    /// tokens, if the host accepts anyone to issue them to.
    pub fn methods(&self, client_key: Option<KeyPin>) -> Vec<AuthMethod> {
        let mut methods = Vec::new();
        let presented = client_key.is_some();
        if presented && (!self.client_keys().is_empty() || self.pairing.is_some()) {
            methods.push(AuthMethod::CLIENT_CERTIFICATE);
        }
        if !self.tokens.is_empty() || (self.accepts_anyone() && self.resumption_lifetime.is_some())
        {
            methods.push(AuthMethod::TOKEN);
        }
        if !self.users.is_empty() {
            methods.push(AuthMethod::SCRAM_SHA_256_PLUS);
        }
        if presented && self.pairing.is_some() {
            methods.push(AuthMethod::PAIRING);
        }
        methods
    }

    /// Issues a resumption token that resumes `identity`, if the host gives
    /// them, returning the message that gives it to the client and the guard
    /// that starts its lifetime when dropped, as the connection ends.
    pub(crate) fn issue_resumption(
        &self,
        identity: ClientIdentity,
    ) -> Result<Option<(ResumptionToken, IssuedToken)>, RuntimeError> {
        self.resumption_lifetime
            .map(|lifetime| self.resumption.issue(identity, lifetime))
            .transpose()
    }

    /// Returns who presents `token` with the `TOKEN` method: the holder of
    /// one of the host's tokens, or the identity a valid resumption token
    /// resumes, which it redeems.
    fn token_identity(&self, token: &[u8]) -> Option<ClientIdentity> {
        if let Some(position) = self
            .tokens
            .iter()
            .position(|accepted| constant_time_eq(accepted, token))
        {
            return Some(ClientIdentity::Token(position));
        }
        self.resumption_lifetime?;
        let identity = self.resumption.redeem(token, Instant::now())?;
        // A token resumes only an identity the host still accepts.
        let accepted = match &identity {
            ClientIdentity::Token(position) => *position < self.tokens.len(),
            ClientIdentity::User(user) => self.users.contains_key(user),
            ClientIdentity::Certificate(pin) => self.accepts_client_key(pin),
        };
        info!(accepted, "resumption token redeemed");
        accepted.then_some(identity)
    }
}

/// The host's side of one connection's authentication attempts.
#[derive(Debug)]
pub(crate) struct HostAuthExchange<'a> {
    /// The policy.
    authenticator: &'a HostAuthenticator,
    /// The connection's channel binding.
    binding: ChannelBinding,
    /// The key of the certificate the client presented, if any.
    client_key: Option<KeyPin>,
    /// A SCRAM exchange waiting for the client-final-message.
    scram: Option<ScramServer>,
    /// The user the last SCRAM attempt was for, if it is one of the host's.
    user: Option<String>,
    /// Whether that user is locked after repeated failures, so that the
    /// attempt fails whatever the client proves.
    locked: bool,
    /// Who the client authenticated as, once it has.
    identity: Option<ClientIdentity>,
    /// A pairing exchange waiting for the client's share and confirmation.
    pairing: Option<PendingPairing>,
}

/// A pairing exchange the host answered, waiting for the client's message.
#[derive(Debug)]
struct PendingPairing {
    /// The host's side of the exchange.
    exchange: HostPairing,
    /// The exchange's place, which ends with it.
    slot: PairingSlot,
    /// When the code expires.
    expires_at: Instant,
}

impl<'a> HostAuthExchange<'a> {
    /// Starts authentication on a connection with `binding`, whose client
    /// presented a certificate with `client_key`, if any.
    pub(crate) fn new(
        authenticator: &'a HostAuthenticator,
        binding: ChannelBinding,
        client_key: Option<KeyPin>,
    ) -> Self {
        Self {
            authenticator,
            binding,
            client_key,
            scram: None,
            user: None,
            locked: false,
            identity: None,
            pairing: None,
        }
    }

    /// Returns who the client authenticated as, once it has.
    pub(crate) fn identity(&self) -> Option<&ClientIdentity> {
        self.identity.as_ref()
    }

    /// Returns the user the last attempt was for, if it was a SCRAM attempt
    /// as one of the host's users.
    pub(crate) fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Answers `AUTH_START` on the connection `ticket` admitted.
    pub(crate) fn start(
        &mut self,
        method: AuthMethod,
        auth_data: &[u8],
        ticket: &AdmissionTicket,
    ) -> AuthResponse {
        self.user = None;
        self.locked = false;
        self.identity = None;
        // A new attempt abandons a pairing exchange in progress.
        self.pairing = None;
        match method {
            AuthMethod::TOKEN => {
                let identity = self.authenticator.token_identity(auth_data);
                info!(accepted = identity.is_some(), "token authentication");
                match identity {
                    Some(identity) => {
                        self.identity = Some(identity);
                        AuthResponse::Accept(Vec::new())
                    }
                    None => AuthResponse::Reject,
                }
            }
            AuthMethod::CLIENT_CERTIFICATE => self.start_certificate(auth_data),
            AuthMethod::SCRAM_SHA_256_PLUS => self.start_scram(auth_data, ticket),
            AuthMethod::PAIRING => self.start_pairing(auth_data, ticket),
            _ => AuthResponse::Reject,
        }
    }

    /// Answers `AUTH_START` for `PAIRING` (Section 4.4.9): if pairing may
    /// start, chooses a code, has the host's user shown it, and sends the
    /// host's share.
    fn start_pairing(&mut self, auth_data: &[u8], ticket: &AdmissionTicket) -> AuthResponse {
        let (Some(pairing), Some(client_key)) = (&self.authenticator.pairing, self.client_key)
        else {
            return AuthResponse::Reject;
        };
        let Ok(start) = PairingStart::decode(auth_data) else {
            return AuthResponse::Reject;
        };
        let now = Instant::now();
        let slot = match pairing.begin(&start.device_name, now) {
            Ok(slot) => slot,
            Err(refusal) => {
                info!(device = start.device_name, %refusal, "pairing refused");
                return AuthResponse::Reject;
            }
        };
        let random = SystemRandom::new();
        let keys = PairingKeys {
            client: client_key.0,
            host: pairing.host_key().0,
        };
        let started = PairingCode::generate(&random).and_then(|code| {
            HostPairing::start(&code, &keys, &self.binding.0, &random)
                .map(|exchange| (code, exchange))
        });
        let Ok((code, exchange)) = started else {
            return AuthResponse::Reject;
        };
        info!(
            device = start.device_name,
            address = %ticket.address(),
            "a client asks to pair"
        );
        pairing.tell(crate::pairing::PairingEvent::Requested {
            device_name: start.device_name,
            address: ticket.address(),
            code,
        });
        let share = exchange.share().to_vec();
        self.pairing = Some(PendingPairing {
            exchange,
            slot,
            expires_at: now + CODE_LIFETIME,
        });
        AuthResponse::Continue(share)
    }

    /// Answers the client's share and confirmation, accepting the client's
    /// key from now on if it typed the code shown.
    fn finish_pairing(&mut self, pending: PendingPairing, auth_data: &[u8]) -> AuthResponse {
        let now = Instant::now();
        let confirmation = if now <= pending.expires_at {
            pending.exchange.finish(auth_data).ok()
        } else {
            info!("pairing failed: the code expired");
            None
        };
        match (confirmation, self.client_key) {
            (Some(confirmation), Some(client_key)) => {
                self.authenticator.authorize_client_key(client_key);
                pending.slot.end(PairingOutcome::Paired { client_key }, now);
                info!("pairing succeeded");
                self.identity = Some(ClientIdentity::Certificate(client_key));
                AuthResponse::Accept(confirmation.to_vec())
            }
            _ => {
                info!("pairing failed: the code typed was not the one shown");
                pending.slot.end(PairingOutcome::Failed, now);
                AuthResponse::Reject
            }
        }
    }

    /// Answers `AUTH_START` for the `CLIENT_CERTIFICATE` method, which
    /// carries no Auth Data, accepting a client whose certificate has one of
    /// the host's client keys (Section 4.4.2).
    fn start_certificate(&mut self, auth_data: &[u8]) -> AuthResponse {
        let accepted = self
            .client_key
            .filter(|pin| auth_data.is_empty() && self.authenticator.accepts_client_key(pin));
        info!(
            accepted = accepted.is_some(),
            "client certificate authentication"
        );
        match accepted {
            Some(pin) => {
                self.identity = Some(ClientIdentity::Certificate(pin));
                AuthResponse::Accept(Vec::new())
            }
            None => AuthResponse::Reject,
        }
    }

    /// Answers a SCRAM client-first-message. A locked user's exchange runs
    /// with its own credentials, so that it looks like any other, and fails
    /// only at the end.
    fn start_scram(&mut self, client_first: &[u8], ticket: &AdmissionTicket) -> AuthResponse {
        let Ok(first) = ClientFirst::parse(client_first) else {
            return AuthResponse::Reject;
        };
        let Ok(nonce) = random_bytes::<NONCE_LENGTH>() else {
            return AuthResponse::Reject;
        };
        let credentials = self.authenticator.users.get(&first.username).cloned();
        debug!(known = credentials.is_some(), "SCRAM authentication");
        if credentials.is_some() {
            self.locked = ticket.user_locked(&first.username);
            self.user = Some(first.username.clone());
        }
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
        if let Some(pending) = self.pairing.take() {
            return self.finish_pairing(pending, auth_data);
        }
        let Some(server) = self.scram.take() else {
            return AuthResponse::Reject;
        };
        match server.handle_client_final(auth_data) {
            Ok(_) if self.locked => {
                info!("SCRAM authentication refused: the user failed too often recently");
                AuthResponse::Reject
            }
            Ok(server_final) => {
                info!("SCRAM authentication succeeded");
                self.identity = self.user.clone().map(ClientIdentity::User);
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
    /// The certificate the client presents in the TLS handshake, for the
    /// `CLIENT_CERTIFICATE` method; the connection's endpoint must present
    /// one (Section 4.4.2).
    Certificate,
    /// A token for the `TOKEN` method.
    Token(Vec<u8>),
    /// A user name and password for SCRAM-SHA-256-PLUS, or for the
    /// `PASSWORD` method with a host that offers only that (Section 4.4.8).
    Password {
        /// The user name.
        username: String,
        /// The password.
        password: String,
    },
    /// Pairing with a code the host shows, which the client's
    /// [`crate::PairingPrompt`] asks its user for; the connection's endpoint
    /// must present the certificate whose key is `client_key` (Section 4.4.9).
    Pairing {
        /// The device's name, which the host shows with the code.
        device_name: String,
        /// The key of the certificate the client presents, SPAKE2's
        /// identity A.
        client_key: KeyPin,
    },
}

/// Hides the secrets.
impl std::fmt::Debug for ClientCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Certificate => formatter.write_str("Certificate"),
            Self::Token(_) => formatter.write_str("Token(..)"),
            Self::Password { username, .. } => formatter
                .debug_struct("Password")
                .field("username", username)
                .finish_non_exhaustive(),
            Self::Pairing { device_name, .. } => formatter
                .debug_struct("Pairing")
                .field("device_name", device_name)
                .finish_non_exhaustive(),
        }
    }
}

/// The state of the client's authentication.
#[derive(Debug)]
enum ClientAuthState {
    /// No attempt has started.
    Idle,
    /// The client asked the host to accept its certificate.
    Certificate,
    /// A token was sent.
    Token,
    /// A user name and password were sent with the `PASSWORD` method.
    Password,
    /// The client-first-message was sent.
    ScramFirst(ScramClient),
    /// The client-final-message was sent.
    ScramFinal(ScramClientFinal),
    /// A pairing exchange was asked for; the host's share comes next.
    PairingStarted,
    /// The client's share and confirmation were sent; the host's
    /// confirmation comes next.
    PairingSent(ClientPairing),
}

/// The client's side of authentication.
#[derive(Debug)]
pub(crate) struct ClientAuthExchange {
    /// The credentials.
    credentials: ClientCredentials,
    /// The connection's channel binding.
    binding: ChannelBinding,
    /// The key of the certificate the host presented, if known: SPAKE2's
    /// identity B when pairing.
    host_key: Option<KeyPin>,
    /// The progress of the exchange.
    state: ClientAuthState,
    /// The host the client paired with, once the host proved it showed the
    /// code.
    paired_host: Option<KeyPin>,
}

impl ClientAuthExchange {
    /// Prepares authentication with `credentials` on a connection with
    /// `binding`, to a host that presented a certificate with `host_key`.
    pub(crate) fn new(
        credentials: ClientCredentials,
        binding: ChannelBinding,
        host_key: Option<KeyPin>,
    ) -> Self {
        Self {
            credentials,
            binding,
            host_key,
            state: ClientAuthState::Idle,
            paired_host: None,
        }
    }

    /// Returns whether the exchange waits for the code the host shows: the
    /// host's share has come, or comes next.
    pub(crate) fn awaits_pairing_code(&self) -> bool {
        matches!(self.state, ClientAuthState::PairingStarted)
    }

    /// Returns the host the client paired with, once the host proved it
    /// showed the code.
    pub(crate) fn paired_host(&self) -> Option<KeyPin> {
        self.paired_host
    }

    /// Answers the host's share with the `code` the user typed.
    pub(crate) fn continue_pairing(
        &mut self,
        host_share: &[u8],
        code: &str,
    ) -> Result<Vec<u8>, RuntimeError> {
        let ClientCredentials::Pairing { client_key, .. } = &self.credentials else {
            return Err(RuntimeError::UnexpectedAuthStep);
        };
        let (Some(host_key), ClientAuthState::PairingStarted) = (self.host_key, &self.state) else {
            return Err(RuntimeError::UnexpectedAuthStep);
        };
        let code = PairingCode::parse(code).ok_or(RuntimeError::InvalidPairingCode)?;
        let keys = PairingKeys {
            client: client_key.0,
            host: host_key.0,
        };
        let (pairing, message) = ClientPairing::respond(
            &code,
            &keys,
            &self.binding.0,
            host_share,
            &SystemRandom::new(),
        )
        .map_err(|error| RuntimeError::AuthenticationFailed(error.to_string()))?;
        self.state = ClientAuthState::PairingSent(pairing);
        Ok(message)
    }

    /// Returns the method and Auth Data of `AUTH_START`, given the methods the host
    /// offers. A password goes with SCRAM-SHA-256-PLUS where the host offers
    /// it, which never reveals it, and with `PASSWORD` only otherwise: the
    /// client always verifies the host's key or certificate first (Section
    /// 11.2).
    pub(crate) fn start(
        &mut self,
        offered: &[AuthMethod],
    ) -> Result<(AuthMethod, Vec<u8>), RuntimeError> {
        match &self.credentials {
            ClientCredentials::Certificate if offered.contains(&AuthMethod::CLIENT_CERTIFICATE) => {
                self.state = ClientAuthState::Certificate;
                Ok((AuthMethod::CLIENT_CERTIFICATE, Vec::new()))
            }
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
            ClientCredentials::Pairing { device_name, .. }
                if offered.contains(&AuthMethod::PAIRING) =>
            {
                self.state = ClientAuthState::PairingStarted;
                let start = PairingStart {
                    device_name: device_name.clone(),
                };
                Ok((AuthMethod::PAIRING, start.to_bytes()))
            }
            ClientCredentials::Password { username, password }
                if offered.contains(&AuthMethod::PASSWORD) =>
            {
                self.state = ClientAuthState::Password;
                let auth_data = PasswordAuthData {
                    user_name: username.clone(),
                    password: password.clone(),
                };
                Ok((AuthMethod::PASSWORD, auth_data.to_bytes()))
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
    /// success (Section 4.4.4): a host that reports success before the
    /// exchange reached its signature has not proved it knows the password.
    pub(crate) fn finish(&mut self, result: &AuthResult) -> Result<(), RuntimeError> {
        let state = std::mem::replace(&mut self.state, ClientAuthState::Idle);
        if result.status != AuthStatus::Success {
            return Err(RuntimeError::AuthenticationFailed(result.reason.clone()));
        }
        match state {
            ClientAuthState::ScramFinal(client) => {
                Ok(client.verify_server_final(&result.auth_data)?)
            }
            // Only a host that showed the code derived its confirmation.
            ClientAuthState::PairingSent(pairing) => {
                pairing.verify_host(&result.auth_data).map_err(|_| {
                    RuntimeError::AuthenticationFailed(
                        "the host did not prove it showed the code".to_owned(),
                    )
                })?;
                self.paired_host = self.host_key;
                Ok(())
            }
            ClientAuthState::Certificate | ClientAuthState::Token | ClientAuthState::Password => {
                Ok(())
            }
            ClientAuthState::Idle
            | ClientAuthState::ScramFirst(_)
            | ClientAuthState::PairingStarted => Err(RuntimeError::UnexpectedAuthStep),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    use crate::admission::{Admission, AdmissionLimits};

    /// Returns a channel binding for tests.
    fn binding() -> ChannelBinding {
        ChannelBinding([7; 32])
    }

    /// Returns the ticket of a connection from the loopback address.
    fn ticket(admission: &Admission) -> AdmissionTicket {
        admission
            .admit(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .expect("the connection is admitted")
    }

    /// Tokens are accepted only when they match exactly.
    #[test]
    fn tokens() {
        let mut authenticator = HostAuthenticator::new().unwrap();
        authenticator.add_token(b"secret".to_vec());
        assert_eq!(authenticator.methods(None), [AuthMethod::TOKEN]);
        let admission = Admission::default();
        let ticket = ticket(&admission);
        let mut exchange = HostAuthExchange::new(&authenticator, binding(), None);
        assert_eq!(
            exchange.start(AuthMethod::TOKEN, b"secret", &ticket),
            AuthResponse::Accept(Vec::new())
        );
        assert_eq!(
            exchange.start(AuthMethod::TOKEN, b"secrets", &ticket),
            AuthResponse::Reject
        );
        assert_eq!(exchange.user(), None);
    }

    /// A resumption token works once, and only for an identity the host
    /// accepts; a host that issues none offers `TOKEN` only for its own
    /// tokens.
    #[test]
    fn resumption_tokens() {
        let mut authenticator = HostAuthenticator::new().unwrap();
        authenticator.add_user(
            "ian".to_owned(),
            ScramCredentials::derive("pencil", b"salt".to_vec(), 4096).unwrap(),
        );
        assert_eq!(
            authenticator.methods(None),
            [AuthMethod::TOKEN, AuthMethod::SCRAM_SHA_256_PLUS]
        );
        let admission = Admission::default();
        let ticket = ticket(&admission);
        let mut exchange = HostAuthExchange::new(&authenticator, binding(), None);
        for (user, accepted) in [("ian", true), ("ada", false)] {
            let identity = ClientIdentity::User(user.to_owned());
            let (message, _guard) = authenticator
                .issue_resumption(identity.clone())
                .unwrap()
                .unwrap();
            let response = exchange.start(AuthMethod::TOKEN, &message.token, &ticket);
            assert_eq!(
                response == AuthResponse::Accept(Vec::new()),
                accepted,
                "{user}"
            );
            assert_eq!(exchange.identity() == Some(&identity), accepted, "{user}");
            assert_eq!(
                exchange.start(AuthMethod::TOKEN, &message.token, &ticket),
                AuthResponse::Reject
            );
        }

        authenticator.set_resumption_lifetime(None);
        assert_eq!(
            authenticator.methods(None),
            [AuthMethod::SCRAM_SHA_256_PLUS]
        );
        assert!(
            authenticator
                .issue_resumption(ClientIdentity::User("ian".to_owned()))
                .unwrap()
                .is_none()
        );
        assert!(HostAuthenticator::new().unwrap().methods(None).is_empty());
    }

    /// A client and host complete a SCRAM exchange, and a wrong password
    /// fails; once a user failed too often, so does the right one.
    #[test]
    fn scram_exchange() {
        let mut authenticator = HostAuthenticator::new().unwrap();
        authenticator.add_user(
            "ian".to_owned(),
            ScramCredentials::derive("pencil", b"salt".to_vec(), 4096).unwrap(),
        );
        let admission = Admission::new(AdmissionLimits {
            failures_per_user: 1,
            ..AdmissionLimits::default()
        });
        let ticket = ticket(&admission);
        for (password, succeeds) in [("pencil", true), ("wrong", false), ("pencil", false)] {
            let mut host = HostAuthExchange::new(&authenticator, binding(), None);
            let mut client = ClientAuthExchange::new(
                ClientCredentials::Password {
                    username: "ian".to_owned(),
                    password: password.to_owned(),
                },
                binding(),
                None,
            );
            let (method, first) = client.start(&authenticator.methods(None)).unwrap();
            let AuthResponse::Continue(server_first) = host.start(method, &first, &ticket) else {
                panic!("expected a server-first-message");
            };
            assert_eq!(host.user(), Some("ian"));
            let client_final = client.continue_exchange(&server_first).unwrap();
            let result = match host.continue_exchange(&client_final) {
                AuthResponse::Accept(auth_data) => AuthResult {
                    status: AuthStatus::Success,
                    auth_data,
                    reason: String::new(),
                },
                _ => {
                    ticket.failed(host.user());
                    AuthResult {
                        status: AuthStatus::FailedRetry,
                        auth_data: Vec::new(),
                        reason: "authentication failed".to_owned(),
                    }
                }
            };
            assert_eq!(client.finish(&result).is_ok(), succeeds, "{password}");
        }
    }

    /// A client presenting one of the host's client keys is accepted with
    /// `CLIENT_CERTIFICATE`, which is offered only to clients that present a
    /// certificate, and a resumption token resumes it.
    #[test]
    fn client_certificates() {
        let pin = |byte: u8| KeyPin([byte; 32]);
        let mut authenticator = HostAuthenticator::new().unwrap();
        authenticator.add_client_key(pin(1));
        assert!(authenticator.accepts_anyone());
        assert_eq!(authenticator.methods(None), [AuthMethod::TOKEN]);
        assert_eq!(
            authenticator.methods(Some(pin(2))),
            [AuthMethod::CLIENT_CERTIFICATE, AuthMethod::TOKEN]
        );
        let admission = Admission::default();
        let ticket = ticket(&admission);
        for (client_key, auth_data, accepted) in [
            (Some(pin(1)), &b""[..], true),
            (Some(pin(1)), &b"extra"[..], false),
            (Some(pin(2)), &b""[..], false),
            (None, &b""[..], false),
        ] {
            let mut host = HostAuthExchange::new(&authenticator, binding(), client_key);
            let response = host.start(AuthMethod::CLIENT_CERTIFICATE, auth_data, &ticket);
            assert_eq!(
                response == AuthResponse::Accept(Vec::new()),
                accepted,
                "{client_key:?} {auth_data:?}"
            );
        }
        let mut client = ClientAuthExchange::new(ClientCredentials::Certificate, binding(), None);
        assert_eq!(
            client.start(&authenticator.methods(Some(pin(1)))).unwrap(),
            (AuthMethod::CLIENT_CERTIFICATE, Vec::new())
        );
        let success = AuthResult {
            status: AuthStatus::Success,
            reason: String::new(),
            auth_data: Vec::new(),
        };
        assert!(client.finish(&success).is_ok());

        let identity = ClientIdentity::Certificate(pin(1));
        let (message, _guard) = authenticator
            .issue_resumption(identity.clone())
            .unwrap()
            .unwrap();
        let mut host = HostAuthExchange::new(&authenticator, binding(), None);
        let response = host.start(AuthMethod::TOKEN, &message.token, &ticket);
        assert_eq!(response, AuthResponse::Accept(Vec::new()));
        assert_eq!(host.identity(), Some(&identity));
    }

    /// A password goes with SCRAM where the host offers it, and with
    /// `PASSWORD` only to a host that offers nothing else.
    #[test]
    fn password_method() {
        let credentials = ClientCredentials::Password {
            username: "ian".to_owned(),
            password: "pencil".to_owned(),
        };
        let mut client = ClientAuthExchange::new(credentials.clone(), binding(), None);
        let both = [AuthMethod::PASSWORD, AuthMethod::SCRAM_SHA_256_PLUS];
        assert_eq!(
            client.start(&both).unwrap().0,
            AuthMethod::SCRAM_SHA_256_PLUS
        );
        let mut client = ClientAuthExchange::new(credentials, binding(), None);
        let (method, auth_data) = client.start(&[AuthMethod::PASSWORD]).unwrap();
        assert_eq!(method, AuthMethod::PASSWORD);
        let sent = PasswordAuthData::decode(&auth_data).unwrap();
        assert_eq!(
            (sent.user_name.as_str(), sent.password.as_str()),
            ("ian", "pencil")
        );
        let success = AuthResult {
            status: AuthStatus::Success,
            reason: String::new(),
            auth_data: Vec::new(),
        };
        assert!(client.finish(&success).is_ok());
        let mut certificate =
            ClientAuthExchange::new(ClientCredentials::Certificate, binding(), None);
        assert!(matches!(
            certificate.start(&[AuthMethod::TOKEN]),
            Err(RuntimeError::NoCommonAuthMethod)
        ));
    }

    /// A client without credentials for any offered method cannot start.
    #[test]
    fn no_common_method() {
        let mut client =
            ClientAuthExchange::new(ClientCredentials::Token(b"t".to_vec()), binding(), None);
        assert!(matches!(
            client.start(&[AuthMethod::SCRAM_SHA_256_PLUS]),
            Err(RuntimeError::NoCommonAuthMethod)
        ));
        assert_eq!(
            format!("{:?}", ClientCredentials::Token(b"secret".to_vec())),
            "Token(..)"
        );
    }

    /// A host that reports success before the SCRAM exchange reached its
    /// signature is refused, while a token's success needs no signature.
    #[test]
    fn requires_the_signature() {
        let success = AuthResult {
            status: AuthStatus::Success,
            reason: String::new(),
            auth_data: Vec::new(),
        };
        let mut client = ClientAuthExchange::new(
            ClientCredentials::Password {
                username: "ian".to_owned(),
                password: "pencil".to_owned(),
            },
            binding(),
            None,
        );
        client.start(&[AuthMethod::SCRAM_SHA_256_PLUS]).unwrap();
        assert!(matches!(
            client.finish(&success),
            Err(RuntimeError::UnexpectedAuthStep)
        ));
        let mut token =
            ClientAuthExchange::new(ClientCredentials::Token(b"t".to_vec()), binding(), None);
        token.start(&[AuthMethod::TOKEN]).unwrap();
        assert!(token.finish(&success).is_ok());
        assert_eq!(
            RuntimeError::UnexpectedAuthStep.close_code(),
            qdp::registry::ErrorCode::AUTH_FAILED
        );
        assert_eq!(
            RuntimeError::ProtocolViolation(String::new()).close_code(),
            qdp::registry::ErrorCode::PROTOCOL_VIOLATION
        );
    }
}
