//! Where the gateway keeps its configuration and state by default: on Windows
//! under ProgramData, where its service's account may write; elsewhere where
//! Linux's packages and services keep them.

use std::path::PathBuf;

/// The configuration file's name.
const CONFIG_FILE: &str = "gateway.toml";

/// The gateway's folder in ProgramData, on Windows.
#[cfg(windows)]
const DATA_FOLDER: &str = "qdp-gateway";

/// The state folder's name in the gateway's ProgramData folder, on Windows.
#[cfg(windows)]
const STATE_FOLDER: &str = "state";

/// ProgramData, where a system without the variable keeps it.
#[cfg(windows)]
const DEFAULT_PROGRAM_DATA: &str = "C:\\ProgramData";

/// The configuration directory, elsewhere than on Windows.
#[cfg(not(windows))]
const CONFIG_DIRECTORY: &str = "/etc/qdp-gateway";

/// The state directory, elsewhere than on Windows, which the systemd unit's
/// `StateDirectory=` makes.
#[cfg(not(windows))]
const STATE_DIRECTORY: &str = "/var/lib/qdp-gateway";

/// Returns the gateway's folder in ProgramData, which holds its
/// configuration, state and log, on Windows.
#[cfg(windows)]
pub fn data_folder() -> PathBuf {
    std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(DEFAULT_PROGRAM_DATA), PathBuf::from)
        .join(DATA_FOLDER)
}

/// Returns the configuration file read by default.
pub fn default_config() -> PathBuf {
    #[cfg(windows)]
    {
        data_folder().join(CONFIG_FILE)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(CONFIG_DIRECTORY).join(CONFIG_FILE)
    }
}

/// Returns the state directory used unless the configuration names another.
pub fn default_state_directory() -> PathBuf {
    #[cfg(windows)]
    {
        data_folder().join(STATE_FOLDER)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(STATE_DIRECTORY)
    }
}
