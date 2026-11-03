//! The README's Usage section is `qdp-gateway help`, word for word, so that the
//! two never drift apart.

use std::env;
use std::fs;
use std::process::Command;

/// The path of the built `qdp-gateway` binary.
const BINARY: &str = env!("CARGO_BIN_EXE_qdp-gateway");

/// The crate's README.
const README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");

/// What opens the README's usage block.
const BLOCK_START: &str = "## Usage\n\n```text\n";

/// What closes it.
const BLOCK_END: &str = "\n```\n";

/// The environment variable that has the test rewrite the README rather than
/// fail when it differs.
const UPDATE_VARIABLE: &str = "QDP_UPDATE_USAGE";

/// The README's Usage section matches the help the command prints.
#[test]
fn readme_usage_matches_help() {
    let output = Command::new(BINARY).arg("help").output().unwrap();
    assert!(output.status.success(), "`qdp-gateway help` failed");
    let help = String::from_utf8(output.stdout).unwrap();
    let help = help.trim_end();

    let readme = fs::read_to_string(README).unwrap().replace("\r\n", "\n");
    let start = readme
        .find(BLOCK_START)
        .expect("the README has a Usage section with a text block")
        + BLOCK_START.len();
    let end = start
        + readme[start..]
            .find(BLOCK_END)
            .expect("the README's usage block is closed");
    if &readme[start..end] == help {
        return;
    }
    if env::var_os(UPDATE_VARIABLE).is_some() {
        fs::write(
            README,
            format!("{}{help}{}", &readme[..start], &readme[end..]),
        )
        .unwrap();
        return;
    }
    panic!(
        "the README's Usage section differs from `qdp-gateway help`: run \
         `{UPDATE_VARIABLE}=1 cargo test -p qdp-gateway --test usage` to update it"
    );
}
