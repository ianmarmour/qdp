//! The report: every run's results in a directory, summarised as a table
//! for each refresh rate and network profile, a row for each protocol.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use crate::results::Run;
use crate::stats::{percentile, standard_deviation};

/// The percentile reported beside each median.
const TAIL_PERCENT: f64 = 95.0;

/// Returns the report on the runs whose results are in `directory`, as
/// Markdown.
pub fn report(directory: &Path) -> Result<String, String> {
    let mut runs = Vec::new();
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let run: Run = toml::from_str(&text)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            runs.push(run);
        }
    }
    if runs.is_empty() {
        return Err(format!("no results in {}", directory.display()));
    }
    Ok(render(&runs))
}

/// The runs of one protocol over one profile, together.
#[derive(Debug, Default)]
struct Summary {
    /// The runs.
    runs: usize,
    /// The seconds measured.
    seconds: f64,
    /// The pictures shown.
    shown: u64,
    /// The pictures skipped.
    skipped: u64,
    /// The clicks that never showed.
    missed: u32,
    /// The traffic, weighted by each run's length, in megabit seconds.
    megabits: f64,
    /// The pictures' latencies.
    display: Vec<f64>,
    /// The intervals between pictures.
    intervals: Vec<f64>,
    /// The clicks' latencies.
    clicks: Vec<f64>,
    /// The CPU each role used, weighted by each run's length.
    cpu: BTreeMap<String, f64>,
}

impl Summary {
    /// Adds a run.
    fn add(&mut self, run: &Run) {
        self.runs += 1;
        self.seconds += run.seconds;
        self.shown += run.shown_frames;
        self.skipped += run.skipped_frames;
        self.missed += run.missed_clicks;
        self.megabits += run.megabits_per_second * run.seconds;
        self.display.extend(&run.display_latency_ms);
        self.intervals.extend(&run.frame_interval_ms);
        self.clicks.extend(&run.click_latency_ms);
        for (role, percent) in &run.cpu_percent {
            *self.cpu.entry(role.clone()).or_default() += percent * run.seconds;
        }
    }
}

/// The runs of each protocol, by protocol.
type Protocols<'a> = BTreeMap<&'a str, Summary>;

/// Returns the report on `runs`.
fn render(runs: &[Run]) -> String {
    let mut sections: BTreeMap<u32, BTreeMap<&str, Protocols<'_>>> = BTreeMap::new();
    for run in runs {
        sections
            .entry(run.refresh_hz)
            .or_default()
            .entry(&run.profile)
            .or_default()
            .entry(&run.protocol)
            .or_default()
            .add(run);
    }
    let roles: Vec<String> = runs
        .iter()
        .flat_map(|run| run.cpu_percent.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut out = String::from("# Benchmark results\n");
    for (refresh_hz, profiles) in &sections {
        let _ = write!(out, "\n## {refresh_hz} Hz\n");
        for (profile, protocols) in profiles {
            let _ = write!(out, "\n### {profile}\n\n");
            table(&mut out, protocols, &roles);
        }
    }
    out.push_str(
        "\nLatencies are medians, with the 95th percentile in parentheses. \
         The frame time spread is the standard deviation of the time between \
         new pictures.\n",
    );
    out
}

/// Writes to `out` the table of `protocols`, with a CPU column for each of
/// `roles`.
fn table(out: &mut String, protocols: &Protocols<'_>, roles: &[String]) {
    out.push_str(
        "| Protocol | Runs | Click to photon (ms) | Display latency (ms) \
         | Frame rate (fps) | Frame time spread (ms) | Skipped frames \
         | Missed clicks | Bandwidth (Mbit/s) |",
    );
    for role in roles {
        let _ = write!(out, " {role} CPU (%) |");
    }
    out.push_str("\n|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    out.push_str(&"---:|".repeat(roles.len()));
    out.push('\n');
    for (protocol, summary) in protocols {
        let seconds = summary.seconds.max(f64::MIN_POSITIVE);
        let frames = (summary.shown + summary.skipped).max(1);
        let _ = write!(
            out,
            "| {protocol} | {} | {} | {} | {:.1} | {} | {:.1}% | {} | {:.1} |",
            summary.runs,
            median_and_tail(&summary.clicks),
            median_and_tail(&summary.display),
            summary.shown as f64 / seconds,
            optional(standard_deviation(&summary.intervals)),
            summary.skipped as f64 * 100.0 / frames as f64,
            summary.missed,
            summary.megabits / seconds,
        );
        for role in roles {
            let cpu = summary.cpu.get(role).map(|cpu| cpu / seconds);
            let _ = write!(out, " {} |", optional(cpu));
        }
        out.push('\n');
    }
}

/// Returns the median of `samples` with their tail percentile, or a dash
/// if there are none.
fn median_and_tail(samples: &[f64]) -> String {
    match (percentile(samples, 50.0), percentile(samples, TAIL_PERCENT)) {
        (Some(median), Some(tail)) => format!("{median:.1} ({tail:.1})"),
        _ => "–".to_owned(),
    }
}

/// Returns `value` to one decimal place, or a dash if there is none.
fn optional(value: Option<f64>) -> String {
    value.map_or_else(|| "–".to_owned(), |value| format!("{value:.1}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a run of `protocol` over `profile`.
    fn run(protocol: &str, profile: &str, click: f64) -> Run {
        Run {
            protocol: protocol.to_owned(),
            profile: profile.to_owned(),
            round: 1,
            refresh_hz: 60,
            seconds: 10.0,
            shown_frames: 590,
            skipped_frames: 10,
            unreadable_frames: 0,
            missed_clicks: 0,
            megabits_per_second: 12.0,
            display_latency_ms: vec![20.0, 22.0, 30.0],
            frame_interval_ms: vec![16.0, 17.0],
            click_latency_ms: vec![click],
            cpu_percent: BTreeMap::from([("server".to_owned(), 50.0)]),
        }
    }

    /// Each refresh rate and profile gets a table, with a row for each
    /// protocol, whose runs are summarised together.
    #[test]
    fn summarises_each_profile() {
        let fast = Run {
            refresh_hz: 120,
            ..run("qdp", "lan", 5.0)
        };
        let report = render(&[
            run("qdp", "lan", 10.0),
            run("qdp", "lan", 20.0),
            run("vnc", "lan", 40.0),
            run("qdp", "wan", 30.0),
            fast,
        ]);
        let sixty = report.find("## 60 Hz").expect("a 60 Hz section");
        let hundred_twenty = report.find("## 120 Hz").expect("a 120 Hz section");
        assert!(sixty < hundred_twenty, "{report}");
        assert_eq!(report.matches("### lan").count(), 2, "{report}");
        assert_eq!(report.matches("### wan").count(), 1, "{report}");
        assert!(report.contains("| qdp | 1 | 5.0 (5.0) |"), "{report}");
        assert!(
            report.contains(
                "| qdp | 2 | 10.0 (20.0) | 22.0 (30.0) | 59.0 | 0.6 | 1.7% | 0 | 12.0 | 50.0 |"
            ),
            "{report}"
        );
        assert!(report.contains("| vnc | 1 | 40.0 (40.0) |"), "{report}");
    }
}
