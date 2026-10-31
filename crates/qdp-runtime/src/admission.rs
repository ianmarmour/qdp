//! Which clients a host lets in: at most a few connections that have not
//! authenticated yet, from each address and in total (Section 11.7); no new
//! connections from an address after repeated failed authentications
//! (Section 4.4.1); and one session at a time on the desktop, a newer one
//! replacing the older (Section 2.1).

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use qdp::registry::ErrorCode;
use qdp_quic::QdpConnection;
use tracing::info;

/// How many connections a host lets in before they authenticate, and how
/// many failed authentications it tolerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionLimits {
    /// The most unauthenticated connections from one address.
    pub per_address: usize,
    /// The most unauthenticated connections in total.
    pub total: usize,
    /// The failed authentications from one address within
    /// `failure_window` after which it is refused.
    pub failures_per_address: usize,
    /// How long failed authentications count against an address.
    pub failure_window: Duration,
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            per_address: 4,
            total: 32,
            failures_per_address: 10,
            failure_window: Duration::from_secs(300),
        }
    }
}

/// Why a connection is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Too many connections have not authenticated yet, from the address or
    /// in total.
    TooManyPending,
    /// The address failed to authenticate too often recently.
    TooManyFailures,
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooManyPending => "too many connections are authenticating",
            Self::TooManyFailures => "the address failed to authenticate too often",
        })
    }
}

/// The admission state.
#[derive(Debug, Default)]
struct State {
    /// The limits.
    limits: AdmissionLimits,
    /// The unauthenticated connections from each address.
    pending: HashMap<IpAddr, usize>,
    /// When each address recently failed to authenticate.
    failures: HashMap<IpAddr, Vec<Instant>>,
    /// The session the desktop serves, with the ticket that holds it.
    active: Option<(u64, quinn::Connection)>,
    /// The number of the last ticket.
    last_ticket: u64,
}

impl State {
    /// Forgets failures older than the window.
    fn expire_failures(&mut self, now: Instant) {
        let window = self.limits.failure_window;
        self.failures.retain(|_, times| {
            times.retain(|time| now.saturating_duration_since(*time) < window);
            !times.is_empty()
        });
    }
}

/// The admission of clients to one desktop, shared by its connections.
#[derive(Debug, Clone, Default)]
pub struct Admission {
    /// The state.
    state: Arc<Mutex<State>>,
}

impl Admission {
    /// Returns an admission with `limits`.
    pub fn new(limits: AdmissionLimits) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                limits,
                ..State::default()
            })),
        }
    }

    /// Locks the state.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Returns whether half the places for unauthenticated connections are
    /// taken, when the host should make clients prove their address with a
    /// QUIC Retry before letting them in (Section 11.7).
    pub fn busy(&self) -> bool {
        let state = self.lock();
        let total: usize = state.pending.values().sum();
        total * 2 >= state.limits.total
    }

    /// Lets in a connection from `address`, counting it until it
    /// authenticates, or refuses it.
    pub fn admit(&self, address: IpAddr) -> Result<AdmissionTicket, Refusal> {
        let mut state = self.lock();
        state.expire_failures(Instant::now());
        let failures = state.failures.get(&address).map_or(0, Vec::len);
        if failures >= state.limits.failures_per_address {
            return Err(Refusal::TooManyFailures);
        }
        let from_address = state.pending.get(&address).copied().unwrap_or(0);
        let total: usize = state.pending.values().sum();
        if from_address >= state.limits.per_address || total >= state.limits.total {
            return Err(Refusal::TooManyPending);
        }
        *state.pending.entry(address).or_default() += 1;
        state.last_ticket += 1;
        Ok(AdmissionTicket {
            admission: self.clone(),
            address,
            number: state.last_ticket,
            authenticated: false,
        })
    }
}

/// One admitted connection's place, counted as unauthenticated until it
/// authenticates; dropping it frees the place.
#[derive(Debug)]
pub struct AdmissionTicket {
    /// The admission it belongs to.
    admission: Admission,
    /// The connection's address.
    address: IpAddr,
    /// Identifies the ticket.
    number: u64,
    /// Whether the connection authenticated.
    authenticated: bool,
}

impl AdmissionTicket {
    /// Records a failed authentication attempt.
    pub(crate) fn failed(&self) {
        let mut state = self.admission.lock();
        state
            .failures
            .entry(self.address)
            .or_default()
            .push(Instant::now());
    }

    /// Records that the connection authenticated: it no longer counts as
    /// pending, and its session replaces the one the desktop served, which
    /// is closed with `QDP_SESSION_REPLACED`.
    pub(crate) fn authenticated(&mut self, connection: &QdpConnection) {
        if self.authenticated {
            return;
        }
        self.authenticated = true;
        let mut state = self.admission.lock();
        release(&mut state, self.address);
        let connection = connection.quic().clone();
        if let Some((_, previous)) = state.active.replace((self.number, connection)) {
            info!("a newer session replaces the current one");
            previous.close(
                quinn::VarInt::from_u64(ErrorCode::SESSION_REPLACED.0.into_inner())
                    .unwrap_or(quinn::VarInt::MAX),
                b"a newer session replaced this one",
            );
        }
    }
}

/// Counts one fewer unauthenticated connection from `address`.
fn release(state: &mut State, address: IpAddr) {
    if let Some(count) = state.pending.get_mut(&address) {
        *count -= 1;
        if *count == 0 {
            state.pending.remove(&address);
        }
    }
}

impl Drop for AdmissionTicket {
    fn drop(&mut self) {
        let mut state = self.admission.lock();
        if !self.authenticated {
            release(&mut state, self.address);
        }
        if state
            .active
            .as_ref()
            .is_some_and(|(number, _)| *number == self.number)
        {
            state.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// The limits in the tests.
    const LIMITS: AdmissionLimits = AdmissionLimits {
        per_address: 2,
        total: 3,
        failures_per_address: 2,
        failure_window: Duration::from_secs(60),
    };

    /// Returns an address.
    fn address(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
    }

    /// Pending connections are limited per address and in total, and free
    /// their places when dropped.
    #[test]
    fn limits_pending_connections() {
        let admission = Admission::new(LIMITS);
        let first = admission.admit(address(1)).unwrap();
        let _second = admission.admit(address(1)).unwrap();
        assert_eq!(
            admission.admit(address(1)).unwrap_err(),
            Refusal::TooManyPending
        );
        assert!(admission.busy());
        let _third = admission.admit(address(2)).unwrap();
        assert_eq!(
            admission.admit(address(3)).unwrap_err(),
            Refusal::TooManyPending
        );
        drop(first);
        assert!(admission.admit(address(1)).is_ok());
    }

    /// An address that fails too often is refused until the failures age.
    #[test]
    fn refuses_repeated_failures() {
        let admission = Admission::new(LIMITS);
        let ticket = admission.admit(address(1)).unwrap();
        ticket.failed();
        ticket.failed();
        drop(ticket);
        assert_eq!(
            admission.admit(address(1)).unwrap_err(),
            Refusal::TooManyFailures
        );
        assert!(admission.admit(address(2)).is_ok());
        let mut state = admission.lock();
        state.expire_failures(Instant::now() + LIMITS.failure_window);
        assert!(state.failures.is_empty());
    }
}
