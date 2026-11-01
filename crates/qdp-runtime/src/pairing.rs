//! Pairing (Section 4.4.9): on a host, the policy its sessions share, which
//! runs one exchange at a time, backs off after failures and tells the
//! host's user each code; on a client, the prompt that asks its user for the
//! code.

use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use qdp::auth::pairing::PairingCode;
use qdp_quic::KeyPin;
use thiserror::Error;

/// How long a code is valid after the host shows it.
pub const CODE_LIFETIME: Duration = Duration::from_secs(120);

/// The consecutive failures after which pairing is refused for a while.
const FAILURES_BEFORE_REFUSAL: u32 = 3;

/// How long pairing is refused after the third, fourth and fifth
/// consecutive failures; after each further one, twice the time before.
const REFUSALS: [Duration; 3] = [
    Duration::from_secs(60),
    Duration::from_secs(3 * 60),
    Duration::from_secs(5 * 60),
];

/// The longest refusal, which the doubling stops at: a day.
const LONGEST_REFUSAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Returns how long pairing is refused after `failures` consecutive failed
/// exchanges, if at all.
fn refusal_after(failures: u32) -> Option<Duration> {
    let past = failures.checked_sub(FAILURES_BEFORE_REFUSAL)?;
    let index = usize::try_from(past).unwrap_or(usize::MAX);
    if let Some(refusal) = REFUSALS.get(index) {
        return Some(*refusal);
    }
    let doublings = u32::try_from(index - (REFUSALS.len() - 1)).unwrap_or(u32::MAX);
    let last = REFUSALS[REFUSALS.len() - 1];
    Some(
        2u32.checked_pow(doublings)
            .and_then(|factor| last.checked_mul(factor))
            .map_or(LONGEST_REFUSAL, |refusal| refusal.min(LONGEST_REFUSAL)),
    )
}

/// What the host's user is told about pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingEvent {
    /// A client asks to pair: the host's user is shown the code, which the
    /// client's user types.
    Requested {
        /// The client device's name, as it gave it, unauthenticated.
        device_name: String,
        /// The client's address.
        address: IpAddr,
        /// The code to show.
        code: PairingCode,
    },
    /// The exchange ended, and the code is no longer needed.
    Ended {
        /// The client device's name.
        device_name: String,
        /// How it ended.
        outcome: PairingOutcome,
    },
}

/// How a pairing exchange ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingOutcome {
    /// The client paired: its key is accepted from now on, and the host
    /// keeps it for later runs.
    Paired {
        /// The client's key.
        client_key: KeyPin,
    },
    /// The code the client used was not the one shown, or had expired.
    Failed,
    /// The client left before finishing, without guessing.
    Abandoned,
}

/// Why pairing may not start now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PairingRefusal {
    /// Another client is pairing.
    #[error("another client is pairing")]
    Busy,
    /// Pairing failed too often recently.
    #[error("pairing failed too often; it is refused for {} more seconds", .0.as_secs())]
    BackingOff(Duration),
}

/// The pairing state the sessions share.
#[derive(Debug, Default)]
struct PairingState {
    /// Whether an exchange is in progress.
    busy: bool,
    /// The failed exchanges since the last success.
    failures: u32,
    /// Until when pairing is refused, after failures.
    refused_until: Option<Instant>,
}

/// Receives the host's [`PairingEvent`]s.
type PairingListener = Arc<dyn Fn(PairingEvent) + Send + Sync>;

/// A host's pairing policy, which its sessions share: one exchange at a time,
/// refusals that grow after consecutive failures, and the host's user told of
/// each request and its end.
#[derive(Clone)]
pub struct Pairing {
    /// The host's own key, SPAKE2's identity B.
    host_key: KeyPin,
    /// Where events go.
    listener: PairingListener,
    /// The shared state.
    state: Arc<Mutex<PairingState>>,
}

impl fmt::Debug for Pairing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pairing")
            .field("host_key", &self.host_key)
            .finish_non_exhaustive()
    }
}

impl Pairing {
    /// Returns a policy for a host whose key is `host_key`, telling
    /// `listener` of each request and its end.
    pub fn new(host_key: KeyPin, listener: impl Fn(PairingEvent) + Send + Sync + 'static) -> Self {
        Self {
            host_key,
            listener: Arc::new(listener),
            state: Arc::default(),
        }
    }

    /// Returns the host's key.
    pub(crate) fn host_key(&self) -> KeyPin {
        self.host_key
    }

    /// Tells the listener of `event`.
    pub(crate) fn tell(&self, event: PairingEvent) {
        (self.listener)(event);
    }

    /// Locks the shared state.
    fn state(&self) -> std::sync::MutexGuard<'_, PairingState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts an exchange for `device_name` at `now`, if one may start,
    /// returning the slot it holds until it ends.
    pub(crate) fn begin(
        &self,
        device_name: &str,
        now: Instant,
    ) -> Result<PairingSlot, PairingRefusal> {
        let mut state = self.state();
        if let Some(until) = state.refused_until.filter(|until| now < *until) {
            return Err(PairingRefusal::BackingOff(until - now));
        }
        if state.busy {
            return Err(PairingRefusal::Busy);
        }
        state.busy = true;
        Ok(PairingSlot {
            pairing: self.clone(),
            device_name: device_name.to_owned(),
            ended: false,
        })
    }
}

/// The place of the one exchange in progress, released when it ends or is
/// dropped; an exchange dropped before it ended was abandoned.
pub(crate) struct PairingSlot {
    /// The policy the slot belongs to.
    pairing: Pairing,
    /// The client device's name.
    device_name: String,
    /// Whether the exchange's end was recorded.
    ended: bool,
}

impl fmt::Debug for PairingSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingSlot")
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

impl PairingSlot {
    /// Records how the exchange ended at `now`, telling the listener.
    pub(crate) fn end(mut self, outcome: PairingOutcome, now: Instant) {
        {
            let mut state = self.pairing.state();
            match outcome {
                PairingOutcome::Paired { .. } => {
                    state.failures = 0;
                    state.refused_until = None;
                }
                PairingOutcome::Failed => {
                    state.failures = state.failures.saturating_add(1);
                    state.refused_until =
                        refusal_after(state.failures).map(|refusal| now + refusal);
                }
                PairingOutcome::Abandoned => {}
            }
        }
        self.finish(outcome);
    }

    /// Releases the slot and tells the listener the exchange ended.
    fn finish(&mut self, outcome: PairingOutcome) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.pairing.state().busy = false;
        self.pairing.tell(PairingEvent::Ended {
            device_name: std::mem::take(&mut self.device_name),
            outcome,
        });
    }
}

impl Drop for PairingSlot {
    fn drop(&mut self) {
        self.finish(PairingOutcome::Abandoned);
    }
}

/// The future of a code the user typed, or `None` if they declined.
pub type CodeFuture = Pin<Box<dyn Future<Output = Option<String>> + Send>>;

/// Asks a client's user for the code the host shows, and tells the app the
/// host's key once pairing succeeded, to pin it (Section 4.4.9).
#[derive(Clone)]
pub struct PairingPrompt {
    /// Asks for the code.
    ask: Arc<dyn Fn() -> CodeFuture + Send + Sync>,
    /// Takes the paired host's key.
    paired: Arc<dyn Fn(KeyPin) + Send + Sync>,
}

impl fmt::Debug for PairingPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingPrompt")
            .finish_non_exhaustive()
    }
}

impl PairingPrompt {
    /// Returns a prompt that asks the user with `ask` and gives `paired` the
    /// host's key once the host proved it showed the code.
    pub fn new(
        ask: impl Fn() -> CodeFuture + Send + Sync + 'static,
        paired: impl Fn(KeyPin) + Send + Sync + 'static,
    ) -> Self {
        Self {
            ask: Arc::new(ask),
            paired: Arc::new(paired),
        }
    }

    /// Asks the user for the code.
    pub(crate) fn ask(&self) -> CodeFuture {
        (self.ask)()
    }

    /// Tells the app the host it paired with.
    pub(crate) fn paired(&self, host_key: KeyPin) {
        (self.paired)(host_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refusals start after three failures at one minute, then three, then
    /// five, and double from there up to a day.
    #[test]
    fn backs_off_after_failures() {
        let minutes = |failures| refusal_after(failures).map(|refusal| refusal.as_secs() / 60);
        assert_eq!(minutes(0), None);
        assert_eq!(minutes(2), None);
        assert_eq!(minutes(3), Some(1));
        assert_eq!(minutes(4), Some(3));
        assert_eq!(minutes(5), Some(5));
        assert_eq!(minutes(6), Some(10));
        assert_eq!(minutes(7), Some(20));
        assert_eq!(minutes(40), Some(24 * 60));
        assert_eq!(minutes(u32::MAX), Some(24 * 60));
    }

    /// Returns a policy whose events are kept in a list.
    fn pairing() -> (Pairing, Arc<Mutex<Vec<PairingEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&events);
        let pairing = Pairing::new(KeyPin([1; 32]), move |event| {
            kept.lock().unwrap().push(event);
        });
        (pairing, events)
    }

    /// One exchange runs at a time; a dropped one was abandoned and frees
    /// the slot without counting as a failure.
    #[test]
    fn runs_one_exchange_at_a_time() {
        let (pairing, events) = pairing();
        let now = Instant::now();
        let slot = pairing.begin("Thor", now).unwrap();
        assert_eq!(
            pairing.begin("Tablet", now).unwrap_err(),
            PairingRefusal::Busy
        );
        drop(slot);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [PairingEvent::Ended {
                device_name: "Thor".to_owned(),
                outcome: PairingOutcome::Abandoned
            }]
        );
        for _ in 0..5 {
            pairing
                .begin("Thor", now)
                .unwrap()
                .end(PairingOutcome::Abandoned, now);
        }
        assert!(pairing.begin("Thor", now).is_ok());
    }

    /// Three failures in a row refuse pairing for a minute, a fourth for
    /// three; a success clears the count.
    #[test]
    fn refuses_after_consecutive_failures() {
        let (pairing, _) = pairing();
        let mut now = Instant::now();
        for _ in 0..3 {
            pairing
                .begin("Thor", now)
                .unwrap()
                .end(PairingOutcome::Failed, now);
        }
        assert_eq!(
            pairing.begin("Thor", now).unwrap_err(),
            PairingRefusal::BackingOff(Duration::from_secs(60))
        );
        now += Duration::from_secs(60);
        pairing
            .begin("Thor", now)
            .unwrap()
            .end(PairingOutcome::Failed, now);
        assert_eq!(
            pairing
                .begin("Thor", now + Duration::from_secs(60))
                .unwrap_err(),
            PairingRefusal::BackingOff(Duration::from_secs(120))
        );
        now += Duration::from_secs(3 * 60);
        let paired = PairingOutcome::Paired {
            client_key: KeyPin([2; 32]),
        };
        pairing.begin("Thor", now).unwrap().end(paired, now);
        pairing
            .begin("Thor", now)
            .unwrap()
            .end(PairingOutcome::Failed, now);
        assert!(pairing.begin("Thor", now).is_ok());
    }
}
