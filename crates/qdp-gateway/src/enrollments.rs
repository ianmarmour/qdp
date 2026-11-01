//! The TOTP secrets offered to users who have none yet, until a code proves
//! their app holds one (Section 4.4.10). A wrong code starts a user over at
//! their password (Section 4.4.1); offering them the same secret again for a
//! while lets them retype the code without adding another secret to their
//! app. They are kept in memory only, as no secret is kept for good before a
//! code is accepted.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use qdp::auth::totp::{SecretError, TotpSecret};

/// How long a secret offered is offered again.
const OFFER_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// The secrets offered, by lowercase user name, with when each was first.
#[derive(Debug, Default)]
pub struct PendingEnrollments {
    /// The secrets, with when each was made.
    offered: HashMap<String, (TotpSecret, Instant)>,
}

impl PendingEnrollments {
    /// Returns the secret to offer `user` at `now`: the one offered before,
    /// if it is recent, or a new one.
    pub fn offer(&mut self, user: &str, now: Instant) -> Result<TotpSecret, SecretError> {
        self.offered
            .retain(|_, (_, made)| now.duration_since(*made) < OFFER_LIFETIME);
        if let Some((secret, _)) = self.offered.get(&user.to_lowercase()) {
            return Ok(secret.clone());
        }
        let secret = TotpSecret::generate()?;
        self.offered
            .insert(user.to_lowercase(), (secret.clone(), now));
        Ok(secret)
    }

    /// Forgets the secret offered to `user`, once they enrolled it.
    pub fn enrolled(&mut self, user: &str) {
        self.offered.remove(&user.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_a_secret_again_until_it_is_enrolled_or_old() {
        let mut pending = PendingEnrollments::default();
        let now = Instant::now();
        let first = pending.offer("alice", now).unwrap();
        assert_eq!(
            pending
                .offer("Alice", now + Duration::from_secs(60))
                .unwrap(),
            first
        );
        assert_ne!(pending.offer("bob", now).unwrap(), first);
        pending.enrolled("alice");
        let second = pending.offer("alice", now).unwrap();
        assert_ne!(second, first);
        assert_ne!(
            pending.offer("alice", now + OFFER_LIFETIME).unwrap(),
            second
        );
    }
}
