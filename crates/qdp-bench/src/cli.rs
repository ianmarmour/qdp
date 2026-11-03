//! The command line: the help text, and the commands and options parsed
//! from it.

use std::path::PathBuf;

use crate::measure::MeasureArgs;
use crate::scene::Scene;

/// The help text.
pub const USAGE: &str = "\
qdp-bench: measure remote desktop latency between headless GNOME sessions

Usage:
  qdp-bench probe [--scene NAME]
  qdp-bench measure --protocol NAME --profile NAME --out FILE [options]
  qdp-bench report DIRECTORY
  qdp-bench help

probe shows the probe full screen, for the host's session, with the scene
NAME around it: low (the default), where only the probe changes; scroll,
where a page scrolls in a window; or full, where the whole screen pans.

measure watches the client's session, where a viewer shows the probe full
screen, and writes what it measured to FILE:
  --protocol NAME      The protocol the viewer streams with
  --profile NAME       The network profile the stream runs over
  --scene NAME         The scene the probe shows (default low)
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
pub enum Command {
    /// Show the probe, with a scene.
    Probe(Scene),
    /// Measure a run.
    Measure(MeasureArgs),
    /// Report on the runs in a directory.
    Report(PathBuf),
    /// Print the help text.
    Help,
}

/// Parses the command line, without the program's name.
pub fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Command, String> {
    match arguments.next().as_deref() {
        Some("probe") => parse_probe(arguments).map(Command::Probe),
        Some("measure") => parse_measure(arguments).map(Command::Measure),
        Some("report") => {
            let directory = arguments.next().ok_or("report needs a DIRECTORY")?;
            Ok(Command::Report(PathBuf::from(directory)))
        }
        Some("help" | "--help" | "-h") | None => Ok(Command::Help),
        Some(other) => Err(format!("unknown command {other}; see qdp-bench help")),
    }
}

/// Parses the options of `qdp-bench probe`.
fn parse_probe(mut arguments: impl Iterator<Item = String>) -> Result<Scene, String> {
    let mut scene = Scene::default();
    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--scene" => {
                let name = arguments.next().ok_or("--scene needs a value")?;
                scene = Scene::from_name(&name)?;
            }
            other => return Err(format!("unknown option {other}; see qdp-bench help")),
        }
    }
    Ok(scene)
}

/// Parses the options of `qdp-bench measure`.
fn parse_measure(mut arguments: impl Iterator<Item = String>) -> Result<MeasureArgs, String> {
    let mut protocol = None;
    let mut profile = None;
    let mut out = None;
    let mut args = MeasureArgs {
        protocol: String::new(),
        profile: String::new(),
        scene: Scene::default(),
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
            "--scene" => args.scene = Scene::from_name(&value()?)?,
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
        assert_eq!(parse_line("probe"), Ok(Command::Probe(Scene::Low)));
        assert_eq!(
            parse_line("probe --scene full"),
            Ok(Command::Probe(Scene::Full))
        );
        assert_eq!(parse_line(""), Ok(Command::Help));
        assert_eq!(
            parse_line("report results"),
            Ok(Command::Report(PathBuf::from("results")))
        );
        let Ok(Command::Measure(args)) = parse_line(
            "measure --protocol qdp --profile lan --scene scroll --round 2 --refresh 120 --seconds 5 \
             --process server=10 --process viewer=11 --out run.toml",
        ) else {
            panic!("expected measure");
        };
        assert_eq!(args.protocol, "qdp");
        assert_eq!(args.profile, "lan");
        assert_eq!(args.scene, Scene::Scroll);
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
        assert!(parse_line("probe --scene busy").is_err());
        assert!(parse_line("bench").is_err());
    }
}
