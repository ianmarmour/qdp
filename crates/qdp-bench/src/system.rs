//! What the kernel counts for processes and network interfaces.

use std::path::Path;

/// The field of `/proc/PID/stat`, after the command's name, holding the
/// process's user time; its system time follows.
const USER_TIME_FIELD: usize = 11;

/// Returns the CPU time the process `pid` used so far, its threads'
/// included, in clock ticks, or `None` if it is gone.
pub fn cpu_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_cpu_ticks(&stat)
}

/// Returns the user and system time in a `/proc/PID/stat` line.
fn parse_cpu_ticks(stat: &str) -> Option<u64> {
    // The command's name, in parentheses, may itself hold spaces and
    // parentheses, so the fields are counted from its last one.
    let (_, fields) = stat.rsplit_once(')')?;
    let mut fields = fields.split_whitespace().skip(USER_TIME_FIELD);
    let user: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some(user + system)
}

/// Returns the bytes the network interface `name` sent so far, or `None`
/// if there is no such interface.
pub fn interface_bytes(name: &str) -> Option<u64> {
    let path = Path::new("/sys/class/net")
        .join(name)
        .join("statistics/tx_bytes");
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user and system times are read past a name with spaces.
    #[test]
    fn reads_cpu_time() {
        let stat = "42 (a (b) c) S 1 42 42 0 -1 4194560 100 0 0 0 250 30 0 0 20 0 4";
        assert_eq!(parse_cpu_ticks(stat), Some(280));
    }

    /// This process's own CPU time can be read.
    #[test]
    fn reads_this_process() {
        assert!(cpu_ticks(std::process::id()).is_some());
    }

    /// The loopback interface's traffic can be read.
    #[test]
    fn reads_loopback_traffic() {
        assert!(interface_bytes("lo").is_some());
    }
}
