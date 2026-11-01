//! The gateway's QUIC server (Section 13): each connection's control stream,
//! where the user authenticates with the directory and TOTP and chooses a
//! host, then the tunnel that forwards the client's QDP connection to the
//! host, a datagram at a time.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use qdp::auth::totp::TotpSecret;
use qdp::gateway::assertion::{IdentityAssertion, NONCE_LENGTH};
use qdp::gateway::{
    GATEWAY_PRE_AUTHENTICATION_BUDGET, GATEWAY_VERSION, GatewayErrorCode, GatewayMessage,
    GatewayWelcome, HostEntry, NextFactor, OpenFailed, OpenFailure, Opened, SessionToken,
};
use qdp::message::{AuthMethod, AuthResult, AuthStart, AuthStatus, PasswordAuthData};
use qdp::wire::ControlStreamDecoder;
use qdp_quic::KeyPin;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use ring::rand::{SecureRandom, SystemRandom};
use tokio::net::UdpSocket;

use crate::config::{Config, Enrollment};
use crate::directory::{Directory, DirectoryError, DirectoryUser};
use crate::keys::GatewayKeys;
use crate::limiter::FailureLimiter;
use crate::policy::{self, RoutedTarget};
use crate::tokens::SessionTokens;
use crate::totp_store::TotpStore;

/// How long a client has to open its control stream and authenticate,
/// enrolling a TOTP secret included.
const AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long a tunnel may carry nothing before the gateway closes it
/// (Section 13.6).
const TUNNEL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How often the gateway looks for an idle tunnel.
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// The failed attempts allowed on one connection (Section 4.4.1).
const MAX_FAILED_ATTEMPTS: u32 = 3;

/// The most tunnels the gateway runs at once, beyond which it answers
/// `BUSY` (Section 13.5).
const MAX_TUNNELS: usize = 4096;

/// The largest UDP datagram a host sends.
const MAX_UDP_PAYLOAD: usize = 65535;

/// The bytes read from the control stream at a time.
const READ_CHUNK: usize = 4096;

/// How long the gateway waits for its last message, such as a final failed
/// `AUTH_RESULT`, to be acknowledged before closing the connection, which
/// discards unsent stream data.
const FINAL_DELIVERY_TIMEOUT: Duration = Duration::from_secs(1);

/// The reason of every failed attempt, so that failures do not tell users
/// apart (Section 4.4.1).
const FAILED_REASON: &str = "Sign-in failed";

/// The reason of an attempt the directory could not answer.
const UNAVAILABLE_REASON: &str = "The directory cannot be reached; try again later";

/// The methods a client may start with (Section 13.4).
const FIRST_METHODS: [AuthMethod; 2] = [AuthMethod::PASSWORD, AuthMethod::TOKEN];

/// Why a connection ends early.
#[derive(Debug)]
enum End {
    /// The gateway closes it with this code and reason.
    Close(GatewayErrorCode, &'static str),
    /// The client went away.
    Gone,
}

impl From<quinn::ReadError> for End {
    fn from(_: quinn::ReadError) -> Self {
        Self::Gone
    }
}

impl From<quinn::WriteError> for End {
    fn from(_: quinn::WriteError) -> Self {
        Self::Gone
    }
}

/// A gateway serving users.
pub struct Gateway<D> {
    /// Its configuration.
    config: Config,
    /// The directory users are checked against.
    directory: D,
    /// Its keys.
    keys: GatewayKeys,
    /// The users' TOTP secrets.
    totp: Mutex<TotpStore>,
    /// Failed sign-ins.
    limiter: Mutex<FailureLimiter>,
    /// Session tokens issued.
    tokens: Mutex<SessionTokens>,
    /// The tunnels running.
    tunnels: AtomicUsize,
}

impl<D: Directory> Gateway<D> {
    /// Returns a gateway with `config`, checking users against `directory`.
    pub fn new(config: Config, directory: D, keys: GatewayKeys, totp: TotpStore) -> Self {
        Self {
            limiter: Mutex::new(FailureLimiter::new(&config.limits)),
            tokens: Mutex::new(SessionTokens::new(config.token_lifetime())),
            config,
            directory,
            keys,
            totp: Mutex::new(totp),
            tunnels: AtomicUsize::new(0),
        }
    }

    /// Returns an endpoint listening on `address` with the gateway's key.
    pub fn endpoint(&self, address: SocketAddr) -> io::Result<Endpoint> {
        let config =
            qdp_quic::gateway_server_config(self.keys.tls.clone_key()).map_err(io::Error::other)?;
        Endpoint::server(config, address)
    }

    /// Serves the connections `endpoint` accepts until it closes.
    pub async fn serve(self: Arc<Self>, endpoint: Endpoint) {
        while let Some(incoming) = endpoint.accept().await {
            let gateway = Arc::clone(&self);
            tokio::spawn(async move {
                match incoming.await {
                    Ok(connection) => gateway.serve_connection(connection).await,
                    Err(error) => tracing::debug!(%error, "a handshake failed"),
                }
            });
        }
    }

    /// Serves one connection until it ends.
    async fn serve_connection(&self, connection: Connection) {
        let address = connection.remote_address();
        let client_pin = qdp_quic::peer_key_pin(&connection);
        let mut session = Session {
            gateway: self,
            connection: &connection,
            address,
            client_pin,
        };
        if let Err(End::Close(code, reason)) = session.run().await {
            connection.close(quinn_code(code), reason.as_bytes());
        }
    }
}

/// Serves users with `config`, checking them against its LDAP directory,
/// until `stop` completes, as when the user interrupts or the system stops
/// the service, then closes every connection with `GATEWAY_SHUTDOWN`.
pub async fn serve_until(config: Config, stop: impl Future<Output = ()>) -> Result<(), String> {
    let directory = crate::directory::LdapDirectory::new(&config.directory)
        .map_err(|error| error.to_string())?;
    let keys =
        GatewayKeys::load(&config).map_err(|error| format!("cannot load the keys: {error}"))?;
    let totp = TotpStore::load(&config.state_directory).map_err(|error| error.to_string())?;
    let listen = config.listen;
    let gateway = Arc::new(Gateway::new(config, directory, keys, totp));
    let endpoint = gateway
        .endpoint(listen)
        .map_err(|error| format!("cannot listen on {listen}: {error}"))?;
    tracing::info!(address = %listen, "serving");
    let serving = tokio::spawn(Arc::clone(&gateway).serve(endpoint.clone()));
    stop.await;
    tracing::info!("shutting down");
    endpoint.close(
        quinn_code(GatewayErrorCode::SHUTDOWN),
        b"the gateway is shutting down",
    );
    endpoint.wait_idle().await;
    serving.abort();
    Ok(())
}

/// Returns a gateway error code as quinn's.
fn quinn_code(code: GatewayErrorCode) -> quinn::VarInt {
    quinn::VarInt::from_u64(code.0.into_inner()).unwrap_or(quinn::VarInt::MAX)
}

/// Returns the time now, in seconds since the Unix epoch.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Locks `mutex`, whatever a panic elsewhere left in it.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How far a user has authenticated on a connection.
enum Progress {
    /// Not at all.
    Start,
    /// The directory knows them; TOTP remains, with a secret to enroll if
    /// they have none.
    Identified {
        /// Who they are.
        user: DirectoryUser,
        /// The secret offered for enrollment, kept once a code proves it.
        enrolling: Option<TotpSecret>,
    },
}

/// What an attempt came to.
enum Outcome {
    /// The user is authenticated.
    Success(DirectoryUser),
    /// The user must pass TOTP next.
    NextFactor(NextFactor),
    /// The attempt failed, for this reason.
    Failed(&'static str),
}

/// One connection's control stream.
struct Session<'a, D> {
    /// The gateway.
    gateway: &'a Gateway<D>,
    /// The connection.
    connection: &'a Connection,
    /// The client's address.
    address: SocketAddr,
    /// The pin of the certificate the client presented, if any.
    client_pin: Option<KeyPin>,
}

/// Reads gateway messages from the control stream.
struct Messages {
    /// The stream.
    receive: RecvStream,
    /// The messages split from it.
    decoder: ControlStreamDecoder,
}

impl Messages {
    /// Returns the next message, or `Gone` once the stream ends.
    async fn next(&mut self) -> Result<GatewayMessage, End> {
        let mut chunk = [0; READ_CHUNK];
        loop {
            match self.decoder.next_message() {
                Ok(Some(frame)) => {
                    return GatewayMessage::decode(&frame).map_err(|_| {
                        End::Close(GatewayErrorCode::PROTOCOL_VIOLATION, "malformed message")
                    });
                }
                Ok(None) => {}
                Err(_) => {
                    return Err(End::Close(
                        GatewayErrorCode::PROTOCOL_VIOLATION,
                        "message too large",
                    ));
                }
            }
            match self.receive.read(&mut chunk).await? {
                Some(read) => self.decoder.push(&chunk[..read]),
                None => return Err(End::Gone),
            }
        }
    }
}

/// Writes `message` to the control stream.
async fn send(stream: &mut SendStream, message: GatewayMessage) -> Result<(), End> {
    let Some(frame) = message.to_frame() else {
        return Ok(());
    };
    let bytes = frame
        .to_bytes()
        .map_err(|_| End::Close(GatewayErrorCode::INTERNAL_ERROR, "message too large"))?;
    stream.write_all(&bytes).await?;
    Ok(())
}

impl<D: Directory> Session<'_, D> {
    /// Runs the connection: authentication, then choosing hosts, then the
    /// tunnel.
    async fn run(&mut self) -> Result<(), End> {
        let (mut stream, mut messages, user) =
            tokio::time::timeout(AUTHENTICATION_TIMEOUT, self.authenticate())
                .await
                .map_err(|_| {
                    End::Close(GatewayErrorCode::AUTH_FAILED, "authentication timed out")
                })??;
        messages.decoder.clear_budget();
        loop {
            match messages.next().await? {
                GatewayMessage::HostsRequest => {
                    let hosts = policy::reachable_hosts(&self.gateway.config, &user)
                        .into_iter()
                        .map(|host| HostEntry {
                            host_id: host.id.clone(),
                            name: host.name.clone(),
                            description: host.description.clone(),
                        })
                        .collect();
                    send(&mut stream, GatewayMessage::Hosts(hosts)).await?;
                }
                GatewayMessage::Open(open) => match self.open(&user, &open.target).await {
                    Ok((opened, socket, host)) => {
                        send(&mut stream, GatewayMessage::Opened(opened)).await?;
                        return self.tunnel(&user, socket, &host, messages).await;
                    }
                    Err(failure) => send(&mut stream, GatewayMessage::OpenFailed(failure)).await?,
                },
                GatewayMessage::Unknown(_) => {}
                _ => {
                    return Err(End::Close(
                        GatewayErrorCode::PROTOCOL_VIOLATION,
                        "unexpected message",
                    ));
                }
            }
        }
    }

    /// Accepts the control stream and authenticates the user, returning the
    /// stream and who they are.
    async fn authenticate(&mut self) -> Result<(SendStream, Messages, DirectoryUser), End> {
        let (mut stream, receive) = self.connection.accept_bi().await.map_err(|_| End::Gone)?;
        let mut messages = Messages {
            receive,
            decoder: ControlStreamDecoder::with_budget(GATEWAY_PRE_AUTHENTICATION_BUDGET),
        };
        let GatewayMessage::Hello(hello) = messages.next().await? else {
            return Err(End::Close(
                GatewayErrorCode::PROTOCOL_VIOLATION,
                "GATEWAY_HELLO must come first",
            ));
        };
        tracing::debug!(client = hello.client_name, address = %self.address, "a client says hello");
        send(
            &mut stream,
            GatewayMessage::Welcome(GatewayWelcome {
                version: GATEWAY_VERSION.min(hello.version),
                gateway_name: self.gateway.config.name.clone(),
                auth_methods: FIRST_METHODS.to_vec(),
            }),
        )
        .await?;
        let mut progress = Progress::Start;
        let mut failures = 0;
        loop {
            let start = match messages.next().await? {
                GatewayMessage::AuthStart(start) => start,
                GatewayMessage::Unknown(_) => continue,
                _ => {
                    return Err(End::Close(
                        GatewayErrorCode::PROTOCOL_VIOLATION,
                        "only authentication messages are allowed before it succeeds",
                    ));
                }
            };
            let outcome = self.attempt(&mut progress, start).await;
            let (status, auth_data, reason) = match outcome {
                Outcome::Success(user) => {
                    self.signed_in(&mut stream, &user).await?;
                    return Ok((stream, messages, user));
                }
                Outcome::NextFactor(next) => (AuthStatus::NextFactor, next.to_bytes(), ""),
                Outcome::Failed(reason) => {
                    progress = Progress::Start;
                    failures += 1;
                    let status = if failures >= MAX_FAILED_ATTEMPTS {
                        AuthStatus::FailedFinal
                    } else {
                        AuthStatus::FailedRetry
                    };
                    (status, Vec::new(), reason)
                }
            };
            send(
                &mut stream,
                GatewayMessage::AuthResult(AuthResult {
                    status,
                    auth_data,
                    reason: reason.to_owned(),
                }),
            )
            .await?;
            if status == AuthStatus::FailedFinal {
                if stream.finish().is_ok() {
                    let _ = tokio::time::timeout(FINAL_DELIVERY_TIMEOUT, stream.stopped()).await;
                }
                return Err(End::Close(GatewayErrorCode::AUTH_FAILED, FAILED_REASON));
            }
        }
    }

    /// Answers a successful sign-in, with a session token.
    async fn signed_in(
        &mut self,
        stream: &mut SendStream,
        user: &DirectoryUser,
    ) -> Result<(), End> {
        tracing::info!(
            user = user.name,
            address = %self.address,
            client = ?self.client_pin,
            "signed in"
        );
        send(
            stream,
            GatewayMessage::AuthResult(AuthResult {
                status: AuthStatus::Success,
                auth_data: Vec::new(),
                reason: String::new(),
            }),
        )
        .await?;
        let (token, lifetime) = {
            let mut tokens = lock(&self.gateway.tokens);
            (
                tokens.issue(user, self.client_pin, Instant::now()),
                tokens.lifetime(),
            )
        };
        if let Some(token) = token {
            send(
                stream,
                GatewayMessage::SessionToken(SessionToken { token, lifetime }),
            )
            .await?;
        }
        Ok(())
    }

    /// Runs one authentication attempt.
    async fn attempt(&self, progress: &mut Progress, start: AuthStart) -> Outcome {
        let now = Instant::now();
        let ip = self.address.ip();
        if !lock(&self.gateway.limiter).address_allowed(ip, now) {
            tracing::warn!(address = %self.address, "too many failures from this address");
            return Outcome::Failed(FAILED_REASON);
        }
        match (
            std::mem::replace(progress, Progress::Start),
            start.auth_method,
        ) {
            (Progress::Start, AuthMethod::PASSWORD) => {
                let Ok(password) = PasswordAuthData::decode(&start.auth_data) else {
                    return Outcome::Failed(FAILED_REASON);
                };
                self.check_password(progress, &password, now).await
            }
            (Progress::Start, AuthMethod::TOKEN) => {
                let user =
                    lock(&self.gateway.tokens).redeem(&start.auth_data, self.client_pin, now);
                match user {
                    Some(user) => Outcome::Success(user),
                    None => {
                        lock(&self.gateway.limiter).record_failure(None, ip, now);
                        Outcome::Failed(FAILED_REASON)
                    }
                }
            }
            (Progress::Identified { user, enrolling }, AuthMethod::TOTP) => {
                self.check_code(user, enrolling, &start.auth_data, now)
            }
            (_, method) => {
                tracing::debug!(?method, "an attempt with a method not offered now");
                lock(&self.gateway.limiter).record_failure(None, ip, now);
                Outcome::Failed(FAILED_REASON)
            }
        }
    }

    /// Checks a password with the directory, and asks for TOTP next.
    async fn check_password(
        &self,
        progress: &mut Progress,
        password: &PasswordAuthData,
        now: Instant,
    ) -> Outcome {
        let ip = self.address.ip();
        let typed = password.user_name.as_str();
        if !lock(&self.gateway.limiter).user_allowed(typed, now) {
            tracing::warn!(user = typed, "too many failures for this user");
            return Outcome::Failed(FAILED_REASON);
        }
        let user = match self
            .gateway
            .directory
            .authenticate(typed, &password.password)
            .await
        {
            Ok(user) => user,
            Err(DirectoryError::Refused) => {
                tracing::info!(user = typed, address = %self.address, "the directory refused a password");
                lock(&self.gateway.limiter).record_failure(Some(typed), ip, now);
                return Outcome::Failed(FAILED_REASON);
            }
            Err(DirectoryError::Unavailable(error)) => {
                tracing::error!(%error, "the directory is unavailable");
                return Outcome::Failed(UNAVAILABLE_REASON);
            }
        };
        let enrolled = lock(&self.gateway.totp).get(&user.name).is_some();
        let enrolling = if enrolled {
            None
        } else if self.gateway.config.totp.enrollment == Enrollment::FirstLogin {
            match TotpSecret::generate() {
                Ok(secret) => Some(secret),
                Err(_) => return Outcome::Failed(FAILED_REASON),
            }
        } else {
            tracing::warn!(
                user = user.name,
                "has no TOTP secret; enroll one with qdp-gateway totp enroll"
            );
            return Outcome::Failed(FAILED_REASON);
        };
        let next = NextFactor {
            auth_methods: vec![AuthMethod::TOTP],
            enrollment: enrolling
                .as_ref()
                .map(|secret| secret.enrollment_uri(self.gateway.config.issuer(), &user.name)),
        };
        *progress = Progress::Identified { user, enrolling };
        Outcome::NextFactor(next)
    }

    /// Checks a TOTP code, keeping an enrolled secret once a code proves it.
    fn check_code(
        &self,
        user: DirectoryUser,
        enrolling: Option<TotpSecret>,
        code: &[u8],
        now: Instant,
    ) -> Outcome {
        let code = String::from_utf8_lossy(code);
        let mut totp = lock(&self.gateway.totp);
        let (secret, last_step) = match (&enrolling, totp.get(&user.name)) {
            (Some(secret), _) => (secret.clone(), None),
            (None, Some(entry)) => (entry.secret.clone(), entry.last_step),
            (None, None) => return Outcome::Failed(FAILED_REASON),
        };
        let Some(step) = secret.verify(&code, unix_now(), last_step) else {
            drop(totp);
            tracing::info!(user = user.name, address = %self.address, "a TOTP code was refused");
            lock(&self.gateway.limiter).record_failure(Some(&user.name), self.address.ip(), now);
            return Outcome::Failed(FAILED_REASON);
        };
        let kept = match enrolling {
            Some(secret) => totp
                .set(&user.name, secret)
                .and_then(|()| totp.accept(&user.name, step)),
            None => totp.accept(&user.name, step),
        };
        drop(totp);
        if let Err(error) = kept {
            // A code that cannot be marked used could be used again.
            tracing::error!(%error, "cannot keep the TOTP secrets");
            return Outcome::Failed(FAILED_REASON);
        }
        lock(&self.gateway.limiter).record_success(&user.name);
        Outcome::Success(user)
    }

    /// Opens `target` for `user`: checks policy, finds the host's address and
    /// opens a socket to it, returning the `OPENED` to send, the socket and
    /// the host's name for logs.
    async fn open(
        &self,
        user: &DirectoryUser,
        target: &str,
    ) -> Result<(Opened, UdpSocket, String), OpenFailed> {
        let config = &self.gateway.config;
        let unknown = || OpenFailed {
            code: OpenFailure::UNKNOWN_HOST,
            reason: "No such host".to_owned(),
        };
        if self.gateway.tunnels.load(Ordering::Relaxed) >= MAX_TUNNELS {
            return Err(OpenFailed {
                code: OpenFailure::BUSY,
                reason: "The gateway is busy".to_owned(),
            });
        }
        let (address, host_pin, server_name) =
            if let Some(host) = policy::reachable_host(config, user, target) {
                let address = tokio::net::lookup_host(host.address.as_str())
                    .await
                    .ok()
                    .and_then(|mut addresses| addresses.next())
                    .ok_or_else(unknown)?;
                (address, Some(host.key_pin()), host.server_name.clone())
            } else {
                let routed = RoutedTarget::parse(target).ok_or_else(unknown)?;
                let addresses: Vec<SocketAddr> = match routed.address() {
                    Some(address) => vec![SocketAddr::new(address, routed.port)],
                    None => tokio::net::lookup_host((routed.name.as_str(), routed.port))
                        .await
                        .map(Iterator::collect)
                        .unwrap_or_default(),
                };
                let ips: Vec<IpAddr> = addresses.iter().map(SocketAddr::ip).collect();
                if !policy::routes_allow(config, user, &routed, &ips) {
                    tracing::info!(
                        user = user.name,
                        target,
                        "refused a target the policy does not allow"
                    );
                    return Err(unknown());
                }
                let address = *addresses.first().ok_or_else(unknown)?;
                (address, None, routed.name)
            };
        let local: SocketAddr = if address.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let unreachable = |error: io::Error| {
            tracing::warn!(%error, %address, "cannot open a socket to a host");
            OpenFailed {
                code: OpenFailure::UNREACHABLE,
                reason: "The gateway cannot reach the host".to_owned(),
            }
        };
        let socket = UdpSocket::bind(local).await.map_err(unreachable)?;
        socket.connect(address).await.map_err(unreachable)?;
        let assertion = match (host_pin, self.client_pin) {
            (Some(host_pin), Some(client_pin)) if config.assertions.enabled => {
                self.assertion(user, host_pin, client_pin)
            }
            _ => Vec::new(),
        };
        let opened = Opened {
            host_pin: host_pin.map(|pin| pin.0),
            server_name,
            assertion,
        };
        Ok((opened, socket, format!("{target} ({address})")))
    }

    /// Returns an identity assertion of `user` for the host with `host_pin`.
    fn assertion(&self, user: &DirectoryUser, host_pin: KeyPin, client_pin: KeyPin) -> Vec<u8> {
        let mut nonce = [0; NONCE_LENGTH];
        if SystemRandom::new().fill(&mut nonce).is_err() {
            return Vec::new();
        }
        let issued_at = unix_now();
        IdentityAssertion {
            host_pin: host_pin.0,
            client_pin: client_pin.0,
            issued_at,
            expires_at: issued_at + self.gateway.config.assertions.lifetime_seconds,
            nonce,
            user_name: user.name.clone(),
            display_name: user.display_name.clone(),
            groups: policy::reported_groups(&self.gateway.config, user),
        }
        .sign(&self.gateway.keys.assertion)
    }

    /// Runs the tunnel to `socket` until the connection closes, goes idle, or
    /// the client breaks a rule on the control stream (Section 13.6).
    async fn tunnel(
        &self,
        user: &DirectoryUser,
        socket: UdpSocket,
        host: &str,
        mut messages: Messages,
    ) -> Result<(), End> {
        self.gateway.tunnels.fetch_add(1, Ordering::Relaxed);
        tracing::info!(user = user.name, address = %self.address, host, "opened a tunnel");
        let started = Instant::now();
        let last_activity = AtomicU64::new(0);
        let touch = || {
            let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            last_activity.store(elapsed, Ordering::Relaxed);
        };
        let sent = AtomicU64::new(0);
        let received = AtomicU64::new(0);
        let to_host = async {
            loop {
                let Ok(datagram) = self.connection.read_datagram().await else {
                    return Err(End::Gone);
                };
                touch();
                sent.fetch_add(datagram.len() as u64, Ordering::Relaxed);
                // A host that is not listening yet makes sends fail; the
                // client's connection retries.
                let _ = socket.send(&datagram).await;
            }
        };
        let to_client = async {
            let mut buffer = vec![0; MAX_UDP_PAYLOAD];
            loop {
                let Ok(length) = socket.recv(&mut buffer).await else {
                    // An ICMP error from the host; later datagrams still come.
                    continue;
                };
                touch();
                received.fetch_add(length as u64, Ordering::Relaxed);
                match self
                    .connection
                    .send_datagram(Bytes::copy_from_slice(&buffer[..length]))
                {
                    Ok(()) | Err(quinn::SendDatagramError::TooLarge) => {}
                    Err(_) => return Err(End::Gone),
                }
            }
        };
        let control = async {
            loop {
                match messages.next().await? {
                    GatewayMessage::Unknown(_) | GatewayMessage::HostsRequest => {}
                    _ => {
                        return Err(End::Close(
                            GatewayErrorCode::PROTOCOL_VIOLATION,
                            "one tunnel a connection",
                        ));
                    }
                }
            }
        };
        let idle = async {
            let mut interval = tokio::time::interval(IDLE_CHECK_INTERVAL);
            loop {
                interval.tick().await;
                let quiet = started
                    .elapsed()
                    .saturating_sub(Duration::from_millis(last_activity.load(Ordering::Relaxed)));
                if quiet >= TUNNEL_IDLE_TIMEOUT {
                    return Err(End::Close(GatewayErrorCode::IDLE, "the tunnel was idle"));
                }
            }
        };
        let ended: Result<(), End> = tokio::select! {
            ended = to_host => ended,
            ended = to_client => ended,
            ended = control => ended,
            ended = idle => ended,
        };
        self.gateway.tunnels.fetch_sub(1, Ordering::Relaxed);
        tracing::info!(
            user = user.name,
            host,
            seconds = started.elapsed().as_secs(),
            bytes_to_host = sent.load(Ordering::Relaxed),
            bytes_to_client = received.load(Ordering::Relaxed),
            "closed a tunnel"
        );
        ended
    }
}
