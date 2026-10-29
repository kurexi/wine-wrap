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

/// Checks that only depend on the host, not on WineWrap's own state.
pub fn host_checks() -> Vec<Check> {
    let mut checks = Vec::new();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    checks.push(if os == "macos" {
        Check::new("Operating system", Status::Ok, format!("macOS ({arch})"))
    } else {
        Check::new(
            "Operating system",
            Status::Warning,
            format!("{os} ({arch}): WineWrap targets macOS, other platforms are best-effort"),
        )
    });

    if os == "macos" && arch == "aarch64" {
        checks.push(if rosetta_installed() {
            Check::new("Rosetta 2", Status::Ok, "installed")
        } else {
            Check::new(
                "Rosetta 2",
                Status::Error,
                "not installed; run `softwareupdate --install-rosetta --agree-to-license`",
            )
        });
    }
    checks
}
