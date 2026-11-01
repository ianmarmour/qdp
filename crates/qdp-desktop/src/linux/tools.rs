//! Running the desktop's command-line tools, as KDE's display and service
//! tools, within a time limit, so that a tool that hangs cannot hold up a
//! session.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How often a running tool is checked on.
const POLL: Duration = Duration::from_millis(50);

/// Runs `program` with `args`, its output discarded, and returns whether it
/// succeeded; it is stopped if it runs longer than `timeout`.
pub(super) fn run(
    program: &str,
    args: impl IntoIterator<Item = String>,
    timeout: Duration,
) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    wait(program, &mut child, timeout)
}

/// Waits for `child`, running `program`, to finish, and returns whether it
/// succeeded; it is stopped if it runs longer than `timeout`.
pub(super) fn wait(program: &str, child: &mut Child, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} failed: {status}")),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} did not finish in time"));
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => return Err(format!("cannot wait for {program}: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool's success and failure are told apart, a missing tool is an
    /// error, and one that runs too long is stopped.
    #[test]
    fn runs_tools_within_a_time_limit() {
        let second = Duration::from_secs(1);
        assert!(run("true", [], second).is_ok());
        assert!(run("false", [], second).is_err());
        assert!(run("qdp-no-such-tool", [], second).is_err());
        let started = Instant::now();
        let slow = run("sleep", ["5".to_owned()], Duration::from_millis(100));
        assert!(slow.is_err_and(|error| error.contains("in time")));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
