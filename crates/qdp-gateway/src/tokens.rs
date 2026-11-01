//! Session tokens (Section 13.4): a client that signed in with both factors
//! gets one, and opens later connections with it, to another host or after
//! its network changed, without asking its user again. Tokens are kept in
//! memory, so restarting the gateway signs everyone out; only their SHA-256
//! hashes are kept, and each is bound to the client's key.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use qdp::gateway::MAX_SESSION_TOKEN_LENGTH;
use qdp_quic::KeyPin;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};

use crate::directory::DirectoryUser;

/// The number of random bytes of a token.
const TOKEN_LENGTH: usize = 32;

/// The hash a token is kept by.
type TokenHash = [u8; 32];

/// What a token stands for.
#[derive(Debug, Clone)]
struct Session {
    /// The user who signed in.
    user: DirectoryUser,
    /// The key of the client the token was issued to, if it presented one.
    client_pin: Option<KeyPin>,
    /// When the token expires.
    expires: Instant,
}

/// The session tokens issued.
#[derive(Debug)]
pub struct SessionTokens {
    /// The sessions, by their tokens' hashes.
    sessions: HashMap<TokenHash, Session>,
    /// How long a token lasts.
    lifetime: Duration,
}

impl SessionTokens {
    /// Returns an empty set of tokens that each last `lifetime`.
    pub fn new(lifetime: Duration) -> Self {
        Self {
            sessions: HashMap::new(),
            lifetime,
        }
    }

    /// Returns how long a token lasts.
    pub fn lifetime(&self) -> Duration {
        self.lifetime
    }

    /// Issues a token for `user` on the client with `client_pin`, or `None`
    /// if no random bytes can be had.
    pub fn issue(
        &mut self,
        user: &DirectoryUser,
        client_pin: Option<KeyPin>,
        now: Instant,
    ) -> Option<Vec<u8>> {
        self.sessions.retain(|_, session| session.expires > now);
        let mut token = vec![0; TOKEN_LENGTH];
        SystemRandom::new().fill(&mut token).ok()?;
        self.sessions.insert(
            hash(&token),
            Session {
                user: user.clone(),
                client_pin,
                expires: now + self.lifetime,
            },
        );
        Some(token)
    }

    /// Returns the user `token` stands for, if it is valid at `now` on the
    /// client with `client_pin`.
    pub fn redeem(
        &mut self,
        token: &[u8],
        client_pin: Option<KeyPin>,
        now: Instant,
    ) -> Option<DirectoryUser> {
        if token.len() > MAX_SESSION_TOKEN_LENGTH {
            return None;
        }
        let key = hash(token);
        let session = self.sessions.get(&key)?;
        if session.expires <= now {
            self.sessions.remove(&key);
            return None;
        }
        // A token issued to a client with a key is good only with that key.
        if session.client_pin.is_some() && session.client_pin != client_pin {
            return None;
        }
        Some(session.user.clone())
    }

    /// Revokes every token of the user named `name`, returning how many.
    pub fn revoke(&mut self, name: &str) -> usize {
        let before = self.sessions.len();
        self.sessions
            .retain(|_, session| !session.user.name.eq_ignore_ascii_case(name));
        before - self.sessions.len()
    }
}

/// Returns the hash a token is kept by.
fn hash(token: &[u8]) -> TokenHash {
    let mut hash = [0; 32];
    hash.copy_from_slice(digest(&SHA256, token).as_ref());
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a user named `name`.
    fn user(name: &str) -> DirectoryUser {
        DirectoryUser {
            name: name.to_owned(),
            display_name: String::new(),
            groups: Vec::new(),
        }
    }

    #[test]
    fn redeems_tokens_on_their_client_until_they_expire() {
        let mut tokens = SessionTokens::new(Duration::from_secs(3600));
        let now = Instant::now();
        let pin = KeyPin([1; 32]);
        let token = tokens.issue(&user("alice"), Some(pin), now).unwrap();
        assert_eq!(tokens.redeem(&token, Some(pin), now).unwrap().name, "alice");
        // Again, as for another host.
        assert!(tokens.redeem(&token, Some(pin), now).is_some());
        assert!(tokens.redeem(&token, Some(KeyPin([2; 32])), now).is_none());
        assert!(tokens.redeem(&token, None, now).is_none());
        assert!(tokens.redeem(b"guess", Some(pin), now).is_none());
        assert!(
            tokens
                .redeem(&token, Some(pin), now + Duration::from_secs(3600))
                .is_none()
        );
        let unbound = tokens.issue(&user("bob"), None, now).unwrap();
        assert!(tokens.redeem(&unbound, Some(pin), now).is_some());
        assert_eq!(tokens.revoke("BOB"), 1);
        assert!(tokens.redeem(&unbound, None, now).is_none());
    }
}
