//! A run's results, as `qdp-bench measure` writes them and `qdp-bench
//! report` reads them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What one run of one protocol over one network profile measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// The protocol.
    pub protocol: String,
    /// The network profile.
    pub profile: String,
    /// The round of the benchmark.
    pub round: u32,
    /// The refresh rate of the host's and the client's screens, in hertz.
    #[serde(default = "default_refresh_hz")]
    pub refresh_hz: u32,
    /// How long the run measured, in seconds.
    pub seconds: f64,
    /// The probe's pictures the client's screen showed.
    pub shown_frames: u64,
    /// The screen's frames that brought no new picture.
    pub skipped_frames: u64,
    /// The client's frames whose time could not be read.
    pub unreadable_frames: u64,
    /// The presses and releases whose change never showed.
    pub missed_clicks: u32,
    /// The traffic between the host and the client, in megabits a second.
    pub megabits_per_second: f64,
    /// Each picture's latency, from the probe drawing it to the client's
    /// screen showing it, in milliseconds.
    pub display_latency_ms: Vec<f64>,
    /// The time between new pictures on the client's screen, in
    /// milliseconds.
    pub frame_interval_ms: Vec<f64>,
    /// Each press's and release's latency, from the client's input to the
    /// change on its screen, in milliseconds.
    pub click_latency_ms: Vec<f64>,
    /// Each measured process's CPU use, in percent of one core, by its role.
    pub cpu_percent: BTreeMap<String, f64>,
}

/// The refresh rate of runs that did not record theirs, from before the
/// benchmark ran at any other.
fn default_refresh_hz() -> u32 {
    60
}
