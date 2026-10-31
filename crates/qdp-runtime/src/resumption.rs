//! Resumption tokens (Section 4.4.7). A host gives each session's client a
//! token with which it can reconnect without asking the user for
//! credentials again: valid once, only for the identity that authenticated
//! the session, and only until a lifetime after the connection ends. The
//! client keeps it in memory only, never in storage (Section 11.2).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use qdp::message::ResumptionToken;
use ring::digest::{SHA256, SHA256_OUTPUT_LEN, digest};

use crate::RuntimeError;
use crate::auth::{ClientCredentials, constant_time_eq, random_bytes};

/// The length of the tokens a host issues, in bytes.
const TOKEN_LENGTH: usize = 32;

/// The most tokens a host keeps; issuing another forgets the oldest.
const MAX_TOKENS: usize = 16;

/// How long a host's resumption tokens remain valid after their connection
/// ends, unless it chooses otherwise.
pub const DEFAULT_RESUMPTION_LIFETIME: Duration = Duration::from_secs(300);

/// Who a client authenticated as, which a resumption token carries over to
/// the session it resumes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ClientIdentity {
    /// A holder of the host's token at this position among its tokens.
    Token(usize),
    /// One of the host's users, by prepared name.
    User(String),
}

/// A token the host issued.
#[derive(Debug)]
struct Issued {
    /// The token's SHA-256 digest; the token itself is not kept.
    digest: [u8; SHA256_OUTPUT_LEN],
    /// The identity it resumes.
    identity: ClientIdentity,
    /// Identifies the token among those issued.
    number: u64,
    /// When it stops being valid, once its connection has ended.
    expires_at: Option<Instant>,
}

/// The tokens a host issued and has not seen redeemed or expire.
#[derive(Debug, Default)]
struct State {
    /// The tokens, oldest first.
    issued: Vec<Issued>,
    /// The number of the last token issued.
    last_number: u64,
}

impl State {
    /// Forgets the tokens that expired by `now`.
    fn expire(&mut self, now: Instant) {
        self.issued
            .retain(|token| token.expires_at.is_none_or(|expires_at| expires_at > now));
    }
}

/// The resumption tokens a host has issued, shared by its sessions.
#[derive(Debug, Clone, Default)]
pub(crate) struct ResumptionTokens {
    /// The tokens.
    state: Arc<Mutex<State>>,
}

impl ResumptionTokens {
    /// Locks the tokens.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Issues a token that resumes `identity`, returning the message that
    /// gives it to the client and the guard that starts its `lifetime` when
    /// dropped, as the connection ends.
    pub(crate) fn issue(
        &self,
        identity: ClientIdentity,
        lifetime: Duration,
    ) -> Result<(ResumptionToken, IssuedToken), RuntimeError> {
        let token = random_bytes::<TOKEN_LENGTH>()?;
        let mut state = self.lock();
        state.expire(Instant::now());
        if state.issued.len() >= MAX_TOKENS {
            state.issued.remove(0);
        }
        state.last_number += 1;
        let number = state.last_number;
        state.issued.push(Issued {
            digest: token_digest(&token),
            identity,
            number,
            expires_at: None,
        });
        let message = ResumptionToken {
            token: token.to_vec(),
            lifetime,
        };
        let guard = IssuedToken {
            tokens: self.clone(),
            number,
            lifetime,
        };
        Ok((message, guard))
    }

    /// Redeems `token` at `now`, returning the identity it resumes if it is
    /// valid. A token is redeemed at most once.
    pub(crate) fn redeem(&self, token: &[u8], now: Instant) -> Option<ClientIdentity> {
        let digest = token_digest(token);
        let mut state = self.lock();
        state.expire(now);
        let position = state
            .issued
            .iter()
            .position(|issued| constant_time_eq(&issued.digest, &digest))?;
        Some(state.issued.remove(position).identity)
    }
}

/// Returns the SHA-256 digest of `token`.
fn token_digest(token: &[u8]) -> [u8; SHA256_OUTPUT_LEN] {
    let mut bytes = [0; SHA256_OUTPUT_LEN];
    bytes.copy_from_slice(digest(&SHA256, token).as_ref());
    bytes
}

/// A token issued to a session, whose lifetime starts when the guard is
/// dropped as the session's connection ends.
#[derive(Debug)]
pub(crate) struct IssuedToken {
    /// The host's tokens.
    tokens: ResumptionTokens,
    /// The token's number.
    number: u64,
    /// How long it remains valid after the connection ends.
    lifetime: Duration,
}

impl Drop for IssuedToken {
    fn drop(&mut self) {
        let expires_at = Instant::now() + self.lifetime;
        let mut state = self.tokens.lock();
        if let Some(issued) = state
            .issued
            .iter_mut()
            .find(|issued| issued.number == self.number)
        {
            issued.expires_at = Some(expires_at);
        }
    }
}

/// A resumption token a client received.
#[derive(Clone, PartialEq, Eq)]
struct Received {
    /// The token.
    token: Vec<u8>,
    /// How long it remains valid after the connection ends.
    lifetime: Duration,
    /// When the connection ended, once it has.
    ended_at: Option<Instant>,
}

/// Where a client keeps the resumption token of its latest session, so that
/// it can reconnect without asking for credentials again (Section 4.4.7).
/// The token stays in memory and is never stored (Section 11.2). Clones
/// share the token.
#[derive(Clone, Default)]
pub struct ResumptionSlot {
    /// The latest token.
    latest: Arc<Mutex<Option<Received>>>,
}

impl ResumptionSlot {
    /// Locks the token.
    fn lock(&self) -> MutexGuard<'_, Option<Received>> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Keeps `token`, replacing any earlier one.
    pub(crate) fn received(&self, token: &ResumptionToken) {
        *self.lock() = Some(Received {
            token: token.token.clone(),
            lifetime: token.lifetime,
            ended_at: None,
        });
    }

    /// Notes that the token's connection ended at `now`, which starts its
    /// lifetime.
    pub(crate) fn connection_ended(&self, now: Instant) {
        if let Some(received) = self.lock().as_mut() {
            received.ended_at.get_or_insert(now);
        }
    }

    /// Returns whether a token is kept that is still valid at `now`.
    pub fn is_valid(&self, now: Instant) -> bool {
        self.lock()
            .as_ref()
            .is_some_and(|received| valid_at(received, now))
    }

    /// Takes the credentials that resume the latest session at `now`, if its
    /// token is still valid. A token is used at most once.
    pub fn take_credentials(&self, now: Instant) -> Option<ClientCredentials> {
        self.lock()
            .take()
            .filter(|received| valid_at(received, now))
            .map(|received| ClientCredentials::Token(received.token))
    }
}

/// Returns whether `received` is still valid at `now`.
fn valid_at(received: &Received, now: Instant) -> bool {
    received
        .ended_at
        .is_none_or(|ended_at| now.saturating_duration_since(ended_at) < received.lifetime)
}

/// Shows whether a token is kept but never its bytes, because it is a
/// bearer credential.
impl fmt::Debug for ResumptionSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumptionSlot")
            .field("holds_token", &self.lock().is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token is redeemed once, for its identity, and expires its lifetime
    /// after its connection ends.
    #[test]
    fn redeems_tokens_once() {
        let tokens = ResumptionTokens::default();
        let lifetime = Duration::from_secs(60);
        let (message, guard) = tokens
            .issue(ClientIdentity::User("ian".to_owned()), lifetime)
            .unwrap();
        assert_eq!(message.token.len(), TOKEN_LENGTH);
        assert_eq!(message.lifetime, lifetime);
        let now = Instant::now();
        assert_eq!(tokens.redeem(b"guess", now), None);
        // While the connection lasts, the token stays valid.
        assert_eq!(
            tokens.redeem(&message.token, now + lifetime * 10),
            Some(ClientIdentity::User("ian".to_owned()))
        );
        assert_eq!(tokens.redeem(&message.token, now), None);
        drop(guard);

        let (message, guard) = tokens.issue(ClientIdentity::Token(0), lifetime).unwrap();
        drop(guard);
        let ended = Instant::now();
        assert_eq!(tokens.redeem(&message.token, ended + lifetime), None);

        let (message, _guard) = tokens.issue(ClientIdentity::Token(1), lifetime).unwrap();
        for _ in 0..MAX_TOKENS {
            tokens.issue(ClientIdentity::Token(2), lifetime).unwrap();
        }
        assert_eq!(tokens.redeem(&message.token, Instant::now()), None);
    }

    /// A client's token is valid until its lifetime after the connection
    /// ended, and is taken once.
    #[test]
    fn keeps_the_latest_token() {
        let slot = ResumptionSlot::default();
        let now = Instant::now();
        assert!(!slot.is_valid(now));
        let lifetime = Duration::from_secs(60);
        slot.received(&ResumptionToken {
            token: b"first".to_vec(),
            lifetime,
        });
        slot.received(&ResumptionToken {
            token: b"second".to_vec(),
            lifetime,
        });
        assert_eq!(format!("{slot:?}"), "ResumptionSlot { holds_token: true }");
        slot.connection_ended(now);
        assert!(slot.is_valid(now + lifetime / 2));
        assert!(!slot.is_valid(now + lifetime));
        assert!(slot.take_credentials(now + lifetime).is_none());
        assert!(!slot.is_valid(now));

        slot.received(&ResumptionToken {
            token: b"third".to_vec(),
            lifetime,
        });
        slot.connection_ended(now);
        let Some(ClientCredentials::Token(token)) = slot.take_credentials(now) else {
            panic!("expected the token");
        };
        assert_eq!(token, b"third");
        assert!(slot.take_credentials(now).is_none());
    }
}
