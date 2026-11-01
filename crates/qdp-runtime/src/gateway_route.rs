//! A client's way to a host through a gateway (Section 13): it signs in to
//! the gateway, with the session token of an earlier sign-in or else the
//! user's directory password and a TOTP code, enrolling the user's
//! authenticator app first if the gateway offers a secret, then opens the
//! host and returns the tunnel a session connects to it through.
//!
//! The client asks its user through a [`SignInPrompt`], and keeps the
//! session token the gateway gives in [`Tunnel::session_token`] for later
//! connections, so that they ask nothing until it expires.

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use qdp::gateway::{NextFactor, SessionToken};
use qdp_quic::{
    ClientEndpoint, GatewayClient, HostVerification, KeyPin, SignIn, TlsIdentity, TransportError,
};

/// Where a gateway is, how it is verified, and the host to open through it.
#[derive(Debug, Clone)]
pub struct GatewayRoute {
    /// The gateway's address.
    pub address: SocketAddr,
    /// The gateway's name, sent in the TLS handshake, which its certificate
    /// must be valid for when the system's certificate authorities verify it.
    pub server_name: String,
    /// How the gateway's certificate is verified: by its pin, or by the
    /// system's certificate authorities.
    pub verification: HostVerification,
    /// The host to open: an ID the gateway lists, or the host's name or
    /// address inside the network, with an optional port (Section 13.5).
    pub target: String,
}

/// A user's directory name and password.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The user's name in the directory.
    pub user: String,
    /// The user's password.
    pub password: String,
}

/// Hides the password.
impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// What the gateway asks the user for, for a [`SignInPrompt`] to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInRequest {
    /// The user's directory name and password.
    Credentials {
        /// The gateway's name for users.
        gateway: String,
        /// Why the last attempt failed, if one did.
        retry: Option<String>,
    },
    /// A TOTP code from the user's authenticator app.
    Code {
        /// The gateway's name for users.
        gateway: String,
        /// An `otpauth://totp/` URI with a new secret, for the user to add
        /// to their app first, as a QR code, if they have none yet.
        enrollment: Option<String>,
        /// Why the last attempt failed, if one did.
        retry: Option<String>,
    },
}

/// The user's answer to a [`SignInRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInAnswer {
    /// The name and password asked for.
    Credentials(Credentials),
    /// The code asked for.
    Code(String),
}

/// The future of a [`SignInPrompt`]'s answer: `None` if the user gave up.
pub type AnswerFuture = Pin<Box<dyn Future<Output = Option<SignInAnswer>> + Send>>;

/// Asks a client's user what a gateway's sign-in needs.
#[derive(Clone)]
pub struct SignInPrompt {
    /// Asks the user.
    ask: Arc<dyn Fn(SignInRequest) -> AnswerFuture + Send + Sync>,
}

impl fmt::Debug for SignInPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignInPrompt")
            .finish_non_exhaustive()
    }
}

impl SignInPrompt {
    /// Returns a prompt that asks the user with `ask`.
    pub fn new(ask: impl Fn(SignInRequest) -> AnswerFuture + Send + Sync + 'static) -> Self {
        Self { ask: Arc::new(ask) }
    }

    /// Asks the user for the credentials of `request`, or `None` if they
    /// gave up or answered something else.
    async fn credentials(&self, request: SignInRequest) -> Option<Credentials> {
        match (self.ask)(request).await? {
            SignInAnswer::Credentials(credentials) => Some(credentials),
            SignInAnswer::Code(_) => None,
        }
    }

    /// Asks the user for the code of `request`, or `None` if they gave up or
    /// answered something else.
    async fn code(&self, request: SignInRequest) -> Option<String> {
        match (self.ask)(request).await? {
            SignInAnswer::Code(code) => Some(code),
            SignInAnswer::Credentials(_) => None,
        }
    }
}

/// The tunnel to a host through a gateway, which a session connects
/// through as it would to the host itself.
#[derive(Debug)]
pub struct Tunnel {
    /// The endpoint whose connections go through the tunnel, presenting the
    /// client's identity.
    pub endpoint: ClientEndpoint,
    /// The address to connect its connection to.
    pub address: SocketAddr,
    /// The name to send in the inner TLS handshake.
    pub server_name: String,
    /// The host's key pin, if the gateway knows it.
    pub host_pin: Option<KeyPin>,
    /// The identity assertion to authenticate to the host with, as a token,
    /// if the gateway gave one (Section 13.7).
    pub assertion: Option<Vec<u8>>,
    /// A session token from this sign-in, for the client to keep and sign in
    /// with next time, if the gateway gave one.
    pub session_token: Option<SessionToken>,
}

/// Why a host could not be opened through a gateway.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatewayFailure {
    /// The gateway could not be reached, or the connection to it was lost:
    /// trying again may help.
    #[error("cannot reach the gateway: {0}")]
    Unreachable(String),
    /// The gateway refused the user, or the host.
    #[error("{0}")]
    Refused(String),
    /// The user gave up signing in.
    #[error("signing in to the gateway was cancelled")]
    Cancelled,
}

impl From<TransportError> for GatewayFailure {
    fn from(error: TransportError) -> Self {
        Self::Unreachable(error.to_string())
    }
}

/// Signs in to the gateway of `route` as `client_name`, presenting
/// `identity`, which the host's assertion is bound to: with `saved_token`,
/// a session token from an earlier sign-in, if given and still accepted,
/// and otherwise with what `prompt` asks the user. Then opens the route's
/// host, and returns the tunnel to it.
pub async fn open_tunnel(
    route: &GatewayRoute,
    identity: &TlsIdentity,
    client_name: &str,
    saved_token: Option<&[u8]>,
    prompt: &SignInPrompt,
) -> Result<Tunnel, GatewayFailure> {
    let mut client = GatewayClient::connect(
        route.address,
        &route.server_name,
        route.verification.clone(),
        Some(identity.clone_key()),
        client_name,
    )
    .await?;
    let gateway = client.gateway_name().to_owned();
    let mut signed_in = false;
    if let Some(token) = saved_token {
        match client.sign_in_with_token(token).await? {
            SignIn::SignedIn => signed_in = true,
            SignIn::Failed { reason, last: true } => return Err(GatewayFailure::Refused(reason)),
            // An expired token: the user signs in again.
            SignIn::Failed { last: false, .. } | SignIn::NextFactor(_) => {}
        }
    }
    if !signed_in {
        sign_in(&mut client, &gateway, prompt).await?;
    }
    let opened = client
        .open(&route.target)
        .await?
        .map_err(|failed| GatewayFailure::Refused(failed.reason))?;
    let session_token = client.take_session_token();
    let (mut endpoint, address) = client.into_tunnel()?;
    endpoint.set_identity(identity.clone_key());
    Ok(Tunnel {
        endpoint,
        address,
        server_name: opened.server_name,
        host_pin: opened.host_pin.map(KeyPin),
        assertion: (!opened.assertion.is_empty()).then_some(opened.assertion),
        session_token,
    })
}

/// Signs in with the directory password and a TOTP code, asking the user
/// with `prompt` again after each failure the gateway allows another
/// attempt after. A wrong code starts the user over at the password
/// (Section 4.4.1), which is sent again without asking them.
async fn sign_in(
    client: &mut GatewayClient,
    gateway: &str,
    prompt: &SignInPrompt,
) -> Result<(), GatewayFailure> {
    let mut retry = None;
    // Credentials the directory accepted, sent again after a wrong code.
    let mut accepted: Option<Credentials> = None;
    loop {
        let credentials = match accepted.take() {
            Some(credentials) => credentials,
            None => prompt
                .credentials(SignInRequest::Credentials {
                    gateway: gateway.to_owned(),
                    retry: retry.take(),
                })
                .await
                .ok_or(GatewayFailure::Cancelled)?,
        };
        let outcome = client
            .sign_in_with_password(&credentials.user, &credentials.password)
            .await?;
        let enrollment = match outcome {
            SignIn::SignedIn => return Ok(()),
            SignIn::NextFactor(NextFactor { enrollment, .. }) => enrollment,
            SignIn::Failed { reason, last } => {
                if last {
                    return Err(GatewayFailure::Refused(reason));
                }
                retry = Some(reason);
                continue;
            }
        };
        let code = prompt
            .code(SignInRequest::Code {
                gateway: gateway.to_owned(),
                enrollment,
                retry: retry.take(),
            })
            .await
            .ok_or(GatewayFailure::Cancelled)?;
        match client.sign_in_with_code(&code).await? {
            SignIn::SignedIn => return Ok(()),
            SignIn::Failed { reason, last: true } => return Err(GatewayFailure::Refused(reason)),
            SignIn::Failed {
                reason,
                last: false,
            } => {
                retry = Some(reason);
                accepted = Some(credentials);
            }
            SignIn::NextFactor(_) => {
                return Err(GatewayFailure::Refused(
                    "the gateway asks for a third factor, which this client does not know"
                        .to_owned(),
                ));
            }
        }
    }
}
