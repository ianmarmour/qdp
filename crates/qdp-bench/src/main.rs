//! `qdp-bench`: measures how quickly remote desktop protocols show a host's
//! screen and answer a client's input, between two headless GNOME sessions
//! on one machine, so that both ends read the same clock.
//!
//! A probe fills the host's screen, showing in every frame when it drew
//! it, and lighting a marker while a button is held. A viewer shows the
//! host's screen full screen on the client's; the measurer reads the
//! client's screen, and clicks through the client's own input.

#[cfg(target_os = "linux")]
mod cli;
#[cfg(target_os = "linux")]
mod clock;
#[cfg(target_os = "linux")]
mod layout;
#[cfg(target_os = "linux")]
mod measure;
#[cfg(target_os = "linux")]
mod picture;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod report;
#[cfg(target_os = "linux")]
mod results;
#[cfg(target_os = "linux")]
mod scene;
#[cfg(target_os = "linux")]
mod stats;
#[cfg(target_os = "linux")]
mod system;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
use tracing::{Level, error};

#[cfg(target_os = "linux")]
use crate::cli::{Command, USAGE};

/// Runs the command the command line names.
#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .init();
    let result = cli::parse(std::env::args().skip(1)).and_then(|command| match command {
        Command::Probe(scene) => probe::run(scene),
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

/// Says that the benchmark runs only on Linux, where the sessions it
/// measures between do.
#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("qdp-bench runs on Linux, between headless GNOME sessions");
    ExitCode::FAILURE
}
