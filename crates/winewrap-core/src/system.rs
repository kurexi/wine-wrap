//! Host environment checks ("doctor").

use std::path::Path;

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// Whether Rosetta 2 is installed. Wine on macOS is an x86_64 program, so on
/// Apple Silicon it cannot run without it.
pub fn rosetta_installed() -> bool {
    Path::new("/Library/Apple/usr/libexec/oah/libRosettaRuntime").exists()
}

/// Whether this Mac needs Rosetta 2 installed before Wine can run.
pub fn needs_rosetta() -> bool {
    !rosetta_installed()
}

/// Install Rosetta 2. Runs through `osascript` so macOS shows its standard
/// administrator password prompt; works from the GUI without a terminal.
pub fn rosetta_install_command() -> crate::launch::LaunchSpec {
    crate::launch::LaunchSpec::new("/usr/bin/osascript").args([
        "-e",
        "do shell script \"/usr/sbin/softwareupdate --install-rosetta --agree-to-license\" \
         with administrator privileges",
    ])
}

/// Checks that only depend on the host, not on WineWrap's own state.
pub fn host_checks() -> Vec<Check> {
    vec![if rosetta_installed() {
        Check::new("Rosetta 2", Status::Ok, "installed")
    } else {
        Check::new(
            "Rosetta 2",
            Status::Error,
            "not installed; WineWrap can install it, or run \
             `softwareupdate --install-rosetta --agree-to-license`",
        )
    }]
}
