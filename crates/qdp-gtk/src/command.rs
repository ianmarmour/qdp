//! Running `qdp`, which does the app's networking and draws its streams, as
//! processes whose lines the app reads on its main loop.

use std::ffi::OsStr;
use std::path::PathBuf;

use gtk::prelude::*;
use gtk::{gio, glib};

/// The name of the `qdp` program.
const QDP_PROGRAM: &str = "qdp";

/// Returns the `qdp` to run: the one installed beside the app, as packages
/// install them, or else the first on the `PATH`.
fn program() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|app| app.with_file_name(QDP_PROGRAM))
        .filter(|beside| beside.is_file())
        .unwrap_or_else(|| PathBuf::from(QDP_PROGRAM))
}

/// Starts `qdp` with `arguments` and `flags`, which say which of its
/// standard streams the app reads or writes.
fn start_with(
    arguments: &[&OsStr],
    flags: gio::SubprocessFlags,
) -> Result<gio::Subprocess, glib::Error> {
    let program = program();
    let mut argv = vec![program.as_os_str()];
    argv.extend_from_slice(arguments);
    gio::Subprocess::newv(&argv, flags)
}

/// Starts `qdp` with `arguments`, its standard input and output piped to
/// the app, and its log on the app's standard error.
pub fn start(arguments: &[&OsStr]) -> Result<gio::Subprocess, glib::Error> {
    start_with_environment(arguments, &[])
}

/// Starts `qdp` as [`start`] does, with the environment variables
/// `environment` besides the app's: for secrets, which a command line would
/// show other users.
pub fn start_with_environment(
    arguments: &[&OsStr],
    environment: &[(&str, &str)],
) -> Result<gio::Subprocess, glib::Error> {
    let launcher = gio::SubprocessLauncher::new(
        gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE,
    );
    for (variable, value) in environment {
        launcher.setenv(variable, value, true);
    }
    let program = program();
    let mut argv = vec![program.as_os_str()];
    argv.extend_from_slice(arguments);
    launcher.spawn(&argv)
}

/// Runs `qdp` with `arguments`, its output dropped, and returns whether it
/// succeeded.
pub async fn run(arguments: &[&OsStr]) -> bool {
    let Ok(process) = start_with(arguments, gio::SubprocessFlags::STDOUT_SILENCE) else {
        return false;
    };
    process.wait_future().await.is_ok() && process.is_successful()
}

/// Calls `line` with each line `process` writes, on the main loop, then
/// `exited` once it has exited.
pub fn read_lines(
    process: &gio::Subprocess,
    mut line: impl FnMut(&str) + 'static,
    exited: impl FnOnce() + 'static,
) {
    let process = process.clone();
    glib::spawn_future_local(async move {
        if let Some(output) = process.stdout_pipe() {
            let lines = gio::DataInputStream::new(&output);
            while let Ok(Some(text)) = lines.read_line_utf8_future(glib::Priority::DEFAULT).await {
                line(&text);
            }
        }
        // A process that cannot be waited on has gone already.
        let _ = process.wait_future().await;
        exited();
    });
}

/// Writes `text` and a line break to `process`'s standard input.
pub fn write_line(process: &gio::Subprocess, text: &str) {
    write_lines(process, &[text]);
}

/// Writes `lines`, each with a line break, to `process`'s standard input,
/// in one write: a stream refuses a write while another is under way.
pub fn write_lines(process: &gio::Subprocess, lines: &[&str]) {
    let Some(input) = process.stdin_pipe() else {
        return;
    };
    let bytes: Vec<u8> = lines
        .iter()
        .flat_map(|line| line.bytes().chain(std::iter::once(b'\n')))
        .collect();
    glib::spawn_future_local(async move {
        // A process that stopped reading has ended, which it reports.
        let _ = input.write_all_future(bytes, glib::Priority::DEFAULT).await;
    });
}
