//! The measurer: watches the client's screen, where a viewer shows the
//! probe full screen, and reads from each frame when the probe drew it. It
//! also clicks, through the client's own input, and times how long the
//! probe's marker takes to change on the client's screen.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use qdp_desktop::{FrameMemory, ScreenOptions};
use qdp_runtime::{FrameArrivals, FrameSource, InputAction, InputInjector, PointerButton};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::clock::{now_micros, ticks_per_second};
use crate::layout::{self, CLICK, Picture, Pixels};
use crate::results::Run;
use crate::system::{cpu_ticks, interface_bytes};

/// How long the viewer may take to connect and show the probe.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the stream runs before it is measured, for the encoders and
/// the network to settle.
const WARMUP: Duration = Duration::from_secs(3);

/// How long a click may take to show before it counts as missed.
const CLICK_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the button stays held after the marker lit.
const HOLD: Duration = Duration::from_millis(200);

/// The shortest and longest waits between clicks, in milliseconds; each
/// wait is chosen between them, so that clicks fall at any point of the
/// host's and the client's frames.
const CLICK_GAP_MILLIS: (u64, u64) = (400, 900);

/// How long the measurer waits for a frame before looking at its clicks.
const POLL: Duration = Duration::from_millis(20);

/// QDP's input units in a pixel.
const UNITS_PER_PIXEL: f64 = 256.0;

/// Microseconds in a millisecond.
const MICROS_PER_MILLI: f64 = 1_000.0;

/// What `qdp-bench measure` measures, and where it writes the results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasureArgs {
    /// The protocol the viewer streams with.
    pub protocol: String,
    /// The network profile the stream runs over.
    pub profile: String,
    /// The round of the benchmark this run belongs to.
    pub round: u32,
    /// How long to measure.
    pub seconds: u64,
    /// The file to write the run's results to.
    pub out: PathBuf,
    /// The processes whose CPU time to measure, each with its role.
    pub processes: Vec<(String, u32)>,
    /// The network interface whose traffic to count.
    pub interface: String,
}

/// Measures a run and writes its results.
pub fn run(args: &MeasureArgs) -> Result<(), String> {
    let (source, injector) = qdp_desktop::open_gnome(ScreenOptions {
        frames: FrameMemory::System,
        ..ScreenOptions::default()
    })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| error.to_string())?;
    let run = runtime.block_on(measure(args, Screen::watch(source)?, injector))?;
    let text = toml::to_string(&run).map_err(|error| error.to_string())?;
    std::fs::write(&args.out, text)
        .map_err(|error| format!("cannot write {}: {error}", args.out.display()))?;
    info!(out = %args.out.display(), "results written");
    Ok(())
}

/// What a frame of the client's screen showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    /// When it arrived, in microseconds.
    at: u64,
    /// When the probe drew what it shows, if the grid could be read.
    drawn: Option<u64>,
    /// Whether the marker was lit.
    lit: Option<bool>,
}

/// The client's screen, watched frame by frame.
struct Screen {
    /// Its frames.
    source: Box<dyn FrameSource>,
    /// Counts the frames that arrived.
    arrivals: Arc<FrameArrivals>,
    /// Wakes the measurer when one does.
    wake: Arc<Notify>,
    /// The frames read so far.
    read: u64,
    /// The latest frame's luma plane, with its width and height.
    latest: Option<(Vec<u8>, usize, usize)>,
    /// Where the screen last showed the probe.
    probe: Option<Pixels>,
}

impl Screen {
    /// Starts watching `source`'s frames.
    fn watch(mut source: Box<dyn FrameSource>) -> Result<Self, String> {
        let wake = Arc::new(Notify::new());
        let arrivals = Arc::new(FrameArrivals::new(Arc::clone(&wake)));
        if !source.watch_frames(Arc::clone(&arrivals)) {
            return Err("the client's screen does not say when its frames arrive".to_owned());
        }
        Ok(Self {
            source,
            arrivals,
            wake,
            read: 0,
            latest: None,
            probe: None,
        })
    }

    /// Waits up to `timeout` for the next frame, and returns what it shows.
    async fn next(&mut self, timeout: Duration) -> Result<Option<Seen>, String> {
        if self.arrivals.count() == self.read
            && tokio::time::timeout(timeout, self.wake.notified())
                .await
                .is_err()
        {
            return Ok(None);
        }
        self.read = self.arrivals.count();
        let at = now_micros();
        let captured = self.source.capture().map_err(|error| error.to_string())?;
        let Some(frame) = captured.in_memory() else {
            return Err("the client's screen frames are not in memory".to_owned());
        };
        let (width, height) = (frame.width(), frame.height());
        let picture = Picture {
            luma: frame.y(),
            blue: frame.u(),
            red: frame.v(),
            width,
            height,
        };
        let probe = layout::locate(picture);
        if probe.is_some() {
            self.probe = probe;
        }
        let seen = Seen {
            at,
            drawn: probe.and_then(|probe| layout::read_time(picture, probe)),
            lit: probe.and_then(|probe| layout::marker_lit(picture, probe)),
        };
        if seen.drawn.is_none() {
            self.latest = Some((frame.y().to_vec(), width, height));
        }
        Ok(Some(seen))
    }

    /// Writes the latest frame that did not show the probe to `path`, as a
    /// greyscale PGM picture, for finding out what the viewer showed
    /// instead.
    fn save_latest(&self, path: &Path) {
        let Some((luma, width, height)) = &self.latest else {
            return;
        };
        let mut picture = format!("P5\n{width} {height}\n255\n").into_bytes();
        picture.extend_from_slice(luma);
        match std::fs::write(path, picture) {
            Ok(()) => info!(path = %path.display(), "the client's screen was saved"),
            Err(error) => warn!(%error, "the client's screen cannot be saved"),
        }
    }
}

/// Where a click is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Click {
    /// Waiting to press the button at this time.
    Waiting(u64),
    /// The button was pressed at this time; waiting for the marker to light.
    Pressed(u64),
    /// Holding the button until this time.
    Holding(u64),
    /// The button was released at this time; waiting for the marker to go
    /// dark.
    Released(u64),
}

/// Measures the stream: frames' latency and pacing, and clicks' latency.
async fn measure(
    args: &MeasureArgs,
    mut screen: Screen,
    mut injector: Box<dyn InputInjector>,
) -> Result<Run, String> {
    if let Err(error) = wait_for_probe(&mut screen).await {
        screen.save_latest(&args.out.with_extension("pgm"));
        return Err(error);
    }
    // The pointer moves twice before the clicks, so that the viewer, and the
    // window under the host's pointer, know where it is.
    let mut clicks = Clicks::default();
    for _ in 0..2 {
        clicks.point(screen.probe, &mut *injector)?;
        let warmed = now_micros() + micros(WARMUP) / 2;
        while now_micros() < warmed {
            screen.next(POLL).await?;
        }
    }

    let ticks = ticks_per_second();
    let cpu_before = cpu(&args.processes);
    let bytes_before = interface_bytes(&args.interface);
    let start = now_micros();
    let end = start + args.seconds * 1_000_000;
    let mut frames = Frames::default();
    let mut click = Click::Waiting(start + click_gap(0));
    while now_micros() < end {
        let seen = screen.next(POLL).await?;
        if let Some(seen) = seen {
            frames.add(seen);
        }
        click = clicks.advance(click, seen, now_micros(), screen.probe, &mut *injector)?;
    }
    let elapsed = (now_micros() - start) as f64 / 1_000_000.0;
    if matches!(click, Click::Pressed(_) | Click::Holding(_)) {
        injector.inject(&InputAction::ButtonUp(PointerButton::Left))?;
    }

    let cpu_after = cpu(&args.processes);
    let cpu_percent = cpu_before
        .iter()
        .filter_map(|(role, before)| {
            let after = cpu_after.get(role)?;
            let seconds = after.saturating_sub(*before) as f64 / ticks as f64;
            Some((role.clone(), seconds / elapsed * 100.0))
        })
        .collect();
    let megabits_per_second = match (bytes_before, interface_bytes(&args.interface)) {
        (Some(before), Some(after)) => after.saturating_sub(before) as f64 * 8.0 / elapsed / 1e6,
        _ => 0.0,
    };
    info!(
        frames = frames.latencies.len(),
        clicks = clicks.latencies.len(),
        missed = clicks.missed,
        "measured"
    );
    Ok(Run {
        protocol: args.protocol.clone(),
        profile: args.profile.clone(),
        round: args.round,
        seconds: elapsed,
        shown_frames: frames.latencies.len() as u64,
        skipped_frames: frames.skipped(),
        unreadable_frames: frames.unreadable,
        missed_clicks: clicks.missed,
        megabits_per_second,
        display_latency_ms: frames.latencies,
        frame_interval_ms: frames.intervals,
        click_latency_ms: clicks.latencies,
        cpu_percent,
    })
}

/// Waits until the client's screen shows the probe running: two pictures it
/// drew at different times, as a frozen picture would not.
async fn wait_for_probe(screen: &mut Screen) -> Result<(), String> {
    let deadline = now_micros() + micros(STARTUP_TIMEOUT);
    let mut first = None;
    while now_micros() < deadline {
        let Some(Seen {
            drawn: Some(drawn), ..
        }) = screen.next(POLL).await?
        else {
            continue;
        };
        match first {
            None => first = Some(drawn),
            Some(first) if drawn != first => {
                if let Some(probe) = screen.probe {
                    info!(
                        x = probe.x,
                        y = probe.y,
                        width = probe.width,
                        height = probe.height,
                        "the viewer shows the probe"
                    );
                }
                return Ok(());
            }
            Some(_) => {}
        }
    }
    Err("the viewer did not show the probe running in time".to_owned())
}

/// The frames measured.
#[derive(Debug, Default)]
struct Frames {
    /// The latest time a frame showed the probe drew, and when it arrived.
    last: Option<(u64, u64)>,
    /// Each new picture's latency, from when the probe drew it to when the
    /// client's screen showed it, in milliseconds.
    latencies: Vec<f64>,
    /// The time between new pictures arriving, in milliseconds.
    intervals: Vec<f64>,
    /// The time between the probe drawing successive pictures that arrived,
    /// in milliseconds.
    drawn_gaps: Vec<f64>,
    /// The frames whose grid could not be read.
    unreadable: u64,
}

impl Frames {
    /// Counts a frame of the client's screen.
    fn add(&mut self, seen: Seen) {
        let Some(drawn) = seen.drawn else {
            self.unreadable += 1;
            return;
        };
        if let Some((last_drawn, last_at)) = self.last {
            if drawn <= last_drawn {
                // The same picture, as when only the pointer moved.
                return;
            }
            self.intervals.push(millis(seen.at - last_at));
            self.drawn_gaps.push(millis(drawn - last_drawn));
        }
        self.latencies.push(millis(seen.at.saturating_sub(drawn)));
        self.last = Some((drawn, seen.at));
    }

    /// Returns how many of the probe's pictures never arrived: those the
    /// gaps between the probe's pictures that did arrive leave out, with the
    /// probe drawing a picture each typical gap.
    fn skipped(&self) -> u64 {
        let Some(period) = crate::stats::percentile(&self.drawn_gaps, 50.0) else {
            return 0;
        };
        self.drawn_gaps
            .iter()
            .map(|gap| ((gap / period).round() as u64).saturating_sub(1))
            .sum()
    }
}

/// The clicks measured.
#[derive(Debug, Default)]
struct Clicks {
    /// Each press's and release's latency, from the input to the marker
    /// changing on the client's screen, in milliseconds.
    latencies: Vec<f64>,
    /// The presses and releases that never showed.
    missed: u32,
    /// The clicks made so far.
    made: u64,
    /// The times the pointer was moved to where the clicks go.
    moves: u64,
}

impl Clicks {
    /// Moves the pointer to where the clicks go on the `probe` the screen
    /// shows. It moves by a pixel each time, so that the window there sees
    /// motion, and the pointer, if it lost it.
    fn point(
        &mut self,
        probe: Option<Pixels>,
        injector: &mut dyn InputInjector,
    ) -> Result<(), String> {
        let Some(probe) = probe else {
            return Ok(());
        };
        let (x, y) = probe.point(CLICK);
        let nudge = (self.moves % 2) as f64;
        self.moves += 1;
        injector.inject(&InputAction::PointerMoveTo {
            x: ((x + nudge) * UNITS_PER_PIXEL) as i32,
            y: (y * UNITS_PER_PIXEL) as i32,
        })
    }

    /// Moves `click` on at `now`, given the frame `seen` if one arrived,
    /// clicking where the screen last showed the `probe`.
    fn advance(
        &mut self,
        click: Click,
        seen: Option<Seen>,
        now: u64,
        probe: Option<Pixels>,
        injector: &mut dyn InputInjector,
    ) -> Result<Click, String> {
        let lit = seen.and_then(|seen| seen.lit.map(|lit| (seen.at, lit)));
        Ok(match click {
            Click::Waiting(at) if now >= at => {
                self.point(probe, injector)?;
                let pressed = now_micros();
                injector.inject(&InputAction::ButtonDown(PointerButton::Left))?;
                Click::Pressed(pressed)
            }
            Click::Pressed(pressed) => match lit {
                Some((at, true)) => {
                    self.latencies.push(millis(at.saturating_sub(pressed)));
                    Click::Holding(at + micros(HOLD))
                }
                _ if now >= pressed + micros(CLICK_TIMEOUT) => {
                    // A release would show nothing to time.
                    self.missed += 1;
                    injector.inject(&InputAction::ButtonUp(PointerButton::Left))?;
                    self.next(now)
                }
                _ => click,
            },
            Click::Holding(until) if now >= until => {
                let released = now_micros();
                injector.inject(&InputAction::ButtonUp(PointerButton::Left))?;
                Click::Released(released)
            }
            Click::Released(released) => match lit {
                Some((at, false)) => {
                    self.latencies.push(millis(at.saturating_sub(released)));
                    self.next(at)
                }
                _ if now >= released + micros(CLICK_TIMEOUT) => {
                    self.missed += 1;
                    self.next(now)
                }
                _ => click,
            },
            Click::Waiting(_) | Click::Holding(_) => click,
        })
    }

    /// Returns the next click, waiting from `now`.
    fn next(&mut self, now: u64) -> Click {
        self.made += 1;
        Click::Waiting(now + click_gap(self.made))
    }
}

/// Returns the wait before click `index`, in microseconds: spread between
/// the shortest and longest waits, the same for every run.
fn click_gap(index: u64) -> u64 {
    let (shortest, longest) = CLICK_GAP_MILLIS;
    // A golden-ratio sequence spreads the waits evenly over their range.
    let fraction = (index as f64 * 0.618_034).fract();
    (shortest as f64 + fraction * (longest - shortest) as f64) as u64 * 1_000
}

/// Returns each process's CPU time so far, in clock ticks.
fn cpu(processes: &[(String, u32)]) -> BTreeMap<String, u64> {
    processes
        .iter()
        .filter_map(|(role, pid)| Some((role.clone(), cpu_ticks(*pid)?)))
        .collect()
}

/// Returns `duration` in microseconds.
fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Returns `micros` microseconds in milliseconds.
fn millis(micros: u64) -> f64 {
    micros as f64 / MICROS_PER_MILLI
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a frame that arrived at `at`, showing a picture drawn at
    /// `drawn`.
    fn seen(at: u64, drawn: u64) -> Seen {
        Seen {
            at,
            drawn: Some(drawn),
            lit: Some(false),
        }
    }

    /// Each new picture counts once, with its latency and the interval
    /// since the last.
    #[test]
    fn counts_new_pictures_once() {
        let mut frames = Frames::default();
        frames.add(seen(10_000, 4_000));
        frames.add(seen(12_000, 4_000));
        frames.add(seen(26_000, 20_000));
        assert_eq!(frames.latencies, [6.0, 6.0]);
        assert_eq!(frames.intervals, [16.0]);
    }

    /// Pictures the probe drew between those that arrived count as
    /// skipped.
    #[test]
    fn counts_skipped_pictures() {
        let mut frames = Frames::default();
        for drawn in [0, 16_000, 32_000, 80_000, 96_000] {
            frames.add(seen(drawn + 5_000, drawn));
        }
        assert_eq!(frames.skipped(), 2);
    }

    /// Unreadable frames are counted apart.
    #[test]
    fn counts_unreadable_frames() {
        let mut frames = Frames::default();
        frames.add(Seen {
            at: 1,
            drawn: None,
            lit: None,
        });
        assert_eq!(frames.unreadable, 1);
        assert!(frames.latencies.is_empty());
    }

    /// The waits between clicks stay in their range.
    #[test]
    fn spreads_clicks() {
        let (shortest, longest) = CLICK_GAP_MILLIS;
        for index in 0..100 {
            let gap = click_gap(index) / 1_000;
            assert!((shortest..=longest).contains(&gap), "{gap}");
        }
    }
}
