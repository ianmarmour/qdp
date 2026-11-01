//! Limits on failed sign-ins (Sections 4.4.1 and 13.4): per user name, so that
//! attempts through the gateway stay below the directory's lockout threshold
//! and cannot lock users out of the organization's other systems, and per
//! source address, so that one address cannot try many names.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::config::LimitsConfig;

/// The failures of one key within the window, oldest first.
type Failures = VecDeque<Instant>;

/// Counts failed sign-ins.
#[derive(Debug)]
pub struct FailureLimiter {
    /// Failures per user name, lowercase.
    users: HashMap<String, Failures>,
    /// Failures per source address.
    addresses: HashMap<IpAddr, Failures>,
    /// Failures for one user name allowed within the window.
    user_limit: usize,
    /// Failures from one address allowed within the window.
    address_limit: usize,
    /// The window.
    window: Duration,
}

impl FailureLimiter {
    /// Returns a limiter with `limits`.
    pub fn new(limits: &LimitsConfig) -> Self {
        Self {
            users: HashMap::new(),
            addresses: HashMap::new(),
            user_limit: usize::try_from(limits.user_failures).unwrap_or(usize::MAX),
            address_limit: usize::try_from(limits.address_failures).unwrap_or(usize::MAX),
            window: limits.window(),
        }
    }

    /// Returns whether `address` may try again at `now`.
    pub fn address_allowed(&mut self, address: IpAddr, now: Instant) -> bool {
        allowed(
            &mut self.addresses,
            &address,
            self.address_limit,
            self.window,
            now,
        )
    }

    /// Returns whether `user` may be tried again at `now`.
    pub fn user_allowed(&mut self, user: &str, now: Instant) -> bool {
        let user = user.to_lowercase();
        allowed(&mut self.users, &user, self.user_limit, self.window, now)
    }

    /// Notes a failure from `address`, for `user` if it is known.
    pub fn record_failure(&mut self, user: Option<&str>, address: IpAddr, now: Instant) {
        self.addresses.entry(address).or_default().push_back(now);
        if let Some(user) = user {
            self.users
                .entry(user.to_lowercase())
                .or_default()
                .push_back(now);
        }
    }

    /// Forgets `user`'s failures once they sign in.
    pub fn record_success(&mut self, user: &str) {
        self.users.remove(&user.to_lowercase());
    }
}

/// Drops `key`'s failures older than `window` and returns whether fewer than
/// `limit` remain.
fn allowed<K: Eq + Hash + Clone>(
    failures: &mut HashMap<K, Failures>,
    key: &K,
    limit: usize,
    window: Duration,
    now: Instant,
) -> bool {
    let Some(times) = failures.get_mut(key) else {
        return true;
    };
    while times
        .front()
        .is_some_and(|&time| now.saturating_duration_since(time) >= window)
    {
        times.pop_front();
    }
    if times.is_empty() {
        failures.remove(key);
        return true;
    }
    times.len() < limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_a_user_and_an_address_until_the_window_passes() {
        let limits = LimitsConfig {
            user_failures: 2,
            address_failures: 3,
            window_minutes: 15,
        };
        let mut limiter = FailureLimiter::new(&limits);
        let start = Instant::now();
        let address: IpAddr = "192.0.2.7".parse().unwrap();
        limiter.record_failure(Some("Alice"), address, start);
        assert!(limiter.user_allowed("alice", start));
        limiter.record_failure(Some("alice"), address, start);
        assert!(!limiter.user_allowed("ALICE", start));
        assert!(limiter.address_allowed(address, start));
        limiter.record_failure(None, address, start);
        assert!(!limiter.address_allowed(address, start));
        let later = start + limits.window();
        assert!(limiter.user_allowed("alice", later));
        assert!(limiter.address_allowed(address, later));
        limiter.record_failure(Some("bob"), address, later);
        limiter.record_success("Bob");
        assert!(limiter.user_allowed("bob", later));
    }
}
