//! `qdp-bench`: measures how quickly remote desktop protocols show a host's
//! screen and answer a client's input, between two headless GNOME sessions
//! on one machine, so that both ends read the same clock.
//!
//! A probe fills the host's screen, showing in every frame when it drew
//! it, and lighting a marker while a button is held. A viewer shows the
//! host's screen full screen on the client's; the measurer reads the
//! client's screen, and clicks through the client's own input.

mod clock;
mod layout;
mod measure;
mod picture;
mod probe;
mod report;
mod results;
mod stats;
mod system;

use std::path::PathBuf;
use std::process::ExitCode;

use tracing::{Level, error};

use crate::measure::MeasureArgs;

/// The help text.
const USAGE: &str = "\
qdp-bench: measure remote desktop latency between headless GNOME sessions

Usage:
  qdp-bench probe
  qdp-bench measure --protocol NAME --profile NAME --out FILE [options]
  qdp-bench report DIRECTORY
  qdp-bench help

probe shows the probe full screen, for the host's session.

measure watches the client's session, where a viewer shows the probe full
screen, and writes what it measured to FILE:
  --protocol NAME      The protocol the viewer streams with
  --profile NAME       The network profile the stream runs over
  --round N            The round of the benchmark this run is (default 1)
  --refresh HZ         The refresh rate of the host's and the client's
                       screens, which every new picture is counted against
                       (default 60)
  --seconds N          How long to measure (default 30)
  --process ROLE=PID   Measure the CPU time of process PID as ROLE
                       (repeatable)
  --interface NAME     The network interface whose traffic to count
                       (default lo)
  --out FILE           Where to write the results

report summarises every run's results in DIRECTORY as Markdown.
";

/// What `qdp-bench` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Show the probe.
    Probe,
    /// Measure a run.
    Measure(MeasureArgs),
    /// Report on the runs in a directory.
    Report(PathBuf),
    /// Print the help text.
    Help,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .init();
    let result = parse(std::env::args().skip(1)).and_then(|command| match command {
        Command::Probe => probe::run(),
        Command::Measure(args) => measure::run(&args),
        Command::Report(directory) => report::report(&directory).map(|report| print!("{report}")),
        Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            error!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Parses the command line, without the program's name.
fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Command, String> {
    match arguments.next().as_deref() {
        Some("probe") => Ok(Command::Probe),
        Some("measure") => parse_measure(arguments).map(Command::Measure),
        Some("report") => {
            let directory = arguments.next().ok_or("report needs a DIRECTORY")?;
            Ok(Command::Report(PathBuf::from(directory)))
        }
        Some("help" | "--help" | "-h") | None => Ok(Command::Help),
        Some(other) => Err(format!("unknown command {other}; see qdp-bench help")),
    }
}

/// Parses the options of `qdp-bench measure`.
fn parse_measure(mut arguments: impl Iterator<Item = String>) -> Result<MeasureArgs, String> {
    let mut protocol = None;
    let mut profile = None;
    let mut out = None;
    let mut args = MeasureArgs {
        protocol: String::new(),
        profile: String::new(),
        round: 1,
        refresh_hz: 60,
        seconds: 30,
        out: PathBuf::new(),
        processes: Vec::new(),
        interface: "lo".to_owned(),
    };
    while let Some(option) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{option} needs a value"))
        };
        match option.as_str() {
            "--protocol" => protocol = Some(value()?),
            "--profile" => profile = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--round" => args.round = number(&option, &value()?)?,
            "--refresh" => args.refresh_hz = number(&option, &value()?)?,
            "--seconds" => args.seconds = number(&option, &value()?)?,
            "--interface" => args.interface = value()?,
            "--process" => {
                let process = value()?;
                let (role, pid) = process
                    .split_once('=')
                    .ok_or_else(|| format!("--process takes ROLE=PID, not {process}"))?;
                args.processes
                    .push((role.to_owned(), number(&option, pid)?));
            }
            other => return Err(format!("unknown option {other}; see qdp-bench help")),
        }
    }
    args.protocol = protocol.ok_or("measure needs --protocol")?;
    args.profile = profile.ok_or("measure needs --profile")?;
    args.out = out.ok_or("measure needs --out")?;
    Ok(args)
}

/// Parses the number `value` of `option`.
fn number<T: std::str::FromStr>(option: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{option} takes a number, not {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a command line.
    fn parse_line(line: &str) -> Result<Command, String> {
        parse(line.split_whitespace().map(str::to_owned))
    }

    /// Each command and its options parse.
    #[test]
    fn parses_commands() {
        assert_eq!(parse_line("probe"), Ok(Command::Probe));
        assert_eq!(parse_line(""), Ok(Command::Help));
        assert_eq!(
            parse_line("report results"),
            Ok(Command::Report(PathBuf::from("results")))
        );
        let Ok(Command::Measure(args)) = parse_line(
            "measure --protocol qdp --profile lan --round 2 --refresh 120 --seconds 5 \
             --process server=10 --process viewer=11 --out run.toml",
        ) else {
            panic!("expected measure");
        };
        assert_eq!(args.protocol, "qdp");
        assert_eq!(args.profile, "lan");
        assert_eq!(args.round, 2);
        assert_eq!(args.refresh_hz, 120);
        assert_eq!(args.seconds, 5);
        assert_eq!(
            args.processes,
            [("server".to_owned(), 10), ("viewer".to_owned(), 11)]
        );
        assert_eq!(args.interface, "lo");
        assert_eq!(args.out, PathBuf::from("run.toml"));
    }

    /// Missing and malformed options are refused.
    #[test]
    fn refuses_bad_options() {
        assert!(parse_line("measure --profile lan --out x").is_err());
        assert!(parse_line("measure --protocol qdp --profile lan --out x --round two").is_err());
        assert!(parse_line("measure --protocol qdp --profile lan --out x --process 12").is_err());
        assert!(parse_line("report").is_err());
        assert!(parse_line("bench").is_err());
    }
}
