//! Discovery of Wine installations ("runtimes").
//!
//! A runtime is any directory laid out like a Wine install prefix, i.e. one that
//! contains `bin/wine` or `bin/wine64` plus `bin/wineserver`. We look in the
//! places the common macOS distributions install to and let the user register
//! extra ones.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::CustomRuntime;
use crate::error::{Error, Result};

/// Where a runtime came from. Only used for display and to pick sensible defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeKind {
    /// Apple's Game Porting Toolkit (Wine + D3DMetal).
    GamePortingToolkit,
    /// Whisky's bundled Wine (also ships D3DMetal).
    Whisky,
    /// A `Wine *.app` bundle, e.g. Gcenx's wine-crossover / wine-stable casks.
    AppBundle,
    /// Wine installed by Homebrew or another package manager into a prefix.
    System,
    /// A build placed in WineWrap's own `runtimes` directory.
    Managed,
    /// Registered by hand in `config.toml`.
    Custom,
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::GamePortingToolkit => "Game Porting Toolkit",
            Self::Whisky => "Whisky",
            Self::AppBundle => "App bundle",
            Self::System => "System",
            Self::Managed => "Managed",
            Self::Custom => "Custom",
        })
    }
}

/// A usable Wine installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineRuntime {
    /// Unique, user-facing identifier (used in bottle configs).
    pub name: String,
    /// Install prefix: the directory containing `bin/`.
    pub root: PathBuf,
    pub kind: RuntimeKind,
}

impl WineRuntime {
    /// Build a runtime from an install prefix, validating its layout.
    pub fn from_root(
        name: impl Into<String>,
        root: impl Into<PathBuf>,
        kind: RuntimeKind,
    ) -> Result<Self> {
        let root = root.into();
        if Self::find_wine_binary(&root).is_none() {
            return Err(Error::NotAWineInstall(root));
        }
        Ok(Self {
            name: name.into(),
            root,
            kind,
        })
    }

    /// Prefer `wine64` on older builds; since Wine 9 the WoW64 build ships a
    /// single `wine` binary that handles both architectures.
    fn find_wine_binary(root: &Path) -> Option<PathBuf> {
        ["wine64", "wine"]
            .iter()
            .map(|b| root.join("bin").join(b))
            .find(|p| p.is_file())
    }

    pub fn wine(&self) -> PathBuf {
        Self::find_wine_binary(&self.root).unwrap_or_else(|| self.root.join("bin/wine"))
    }

    pub fn wineserver(&self) -> PathBuf {
        self.root.join("bin/wineserver")
    }

    /// Whether this build ships Apple's D3DMetal translation layer.
    pub fn has_d3dmetal(&self) -> bool {
        self.root.join("lib/external/D3DMetal.framework").exists()
            || matches!(self.kind, RuntimeKind::GamePortingToolkit)
    }

    /// Ask the runtime for its version (`wine --version`).
    pub fn version(&self) -> Option<String> {
        let out = Command::new(self.wine()).arg("--version").output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|v| !v.is_empty())
    }
}

/// A place a runtime may be installed, before we know whether it exists.
struct Candidate {
    name: String,
    root: PathBuf,
    kind: RuntimeKind,
}

fn well_known_candidates(home: Option<&Path>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut push = |name: &str, root: PathBuf, kind| {
        out.push(Candidate {
            name: name.to_string(),
            root,
            kind,
        })
    };

    // Apple Game Porting Toolkit (Homebrew x86_64 prefix).
    push(
        "gptk",
        "/usr/local/opt/game-porting-toolkit".into(),
        RuntimeKind::GamePortingToolkit,
    );

    // Whisky's bundled Wine.
    if let Some(home) = home {
        push(
            "whisky",
            home.join("Library/Application Support/com.isaacmarovitz.Whisky/Libraries/Wine"),
            RuntimeKind::Whisky,
        );
    }

    // Gcenx's Homebrew casks install `.app` bundles.
    for (name, app) in [
        ("wine-crossover", "Wine Crossover"),
        ("wine-staging", "Wine Staging"),
        ("wine-devel", "Wine Devel"),
        ("wine-stable", "Wine Stable"),
    ] {
        push(
            name,
            PathBuf::from(format!("/Applications/{app}.app/Contents/Resources/wine")),
            RuntimeKind::AppBundle,
        );
    }

    // Package-manager installs (Homebrew on Apple Silicon / Intel, Linux distros).
    push("homebrew", "/opt/homebrew".into(), RuntimeKind::System);
    push("usr-local", "/usr/local".into(), RuntimeKind::System);
    push("system", "/usr".into(), RuntimeKind::System);
    push(
        "opt-wine-stable",
        "/opt/wine-stable".into(),
        RuntimeKind::System,
    );
    push(
        "opt-wine-staging",
        "/opt/wine-staging".into(),
        RuntimeKind::System,
    );

    out
}

/// Every runtime directory in `dir` (WineWrap's managed runtimes folder).
fn managed_candidates(dir: &Path) -> Vec<Candidate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| Candidate {
            name: e.file_name().to_string_lossy().into_owned(),
            root: e.path(),
            kind: RuntimeKind::Managed,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Discover all available runtimes.
///
/// Order matters: it is the order of preference when a bottle does not pin a
/// runtime. Custom runtimes come first, then managed ones, then well-known
/// locations (D3DMetal-capable builds first, since they run the most games).
pub fn discover(custom: &[CustomRuntime], managed_dir: &Path) -> Vec<WineRuntime> {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());

    let candidates = custom
        .iter()
        .map(|c| Candidate {
            name: c.name.clone(),
            root: c.path.clone(),
            kind: RuntimeKind::Custom,
        })
        .chain(managed_candidates(managed_dir))
        .chain(well_known_candidates(home.as_deref()));

    let mut found: Vec<WineRuntime> = Vec::new();
    for c in candidates {
        let Ok(rt) = WineRuntime::from_root(c.name, c.root, c.kind) else {
            continue;
        };
        // The same install can be reachable twice (e.g. a custom entry pointing
        // at a well-known path); keep the first, and keep names unique.
        let canonical = std::fs::canonicalize(&rt.root).unwrap_or_else(|_| rt.root.clone());
        let duplicate = found.iter().any(|f| {
            f.name == rt.name
                || std::fs::canonicalize(&f.root).unwrap_or_else(|_| f.root.clone()) == canonical
        });
        if !duplicate {
            found.push(rt);
        }
    }
    found
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Create a fake Wine install prefix for tests.
    pub(crate) fn fake_runtime(root: &Path, binary: &str) {
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join(binary), "").unwrap();
        std::fs::write(root.join("bin/wineserver"), "").unwrap();
    }

    #[test]
    fn rejects_non_wine_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            WineRuntime::from_root("x", dir.path(), RuntimeKind::Custom),
            Err(Error::NotAWineInstall(_))
        ));
    }

    #[test]
    fn prefers_wine64_when_present() {
        let dir = tempfile::tempdir().unwrap();
        fake_runtime(dir.path(), "wine");
        fake_runtime(dir.path(), "wine64");
        let rt = WineRuntime::from_root("x", dir.path(), RuntimeKind::Custom).unwrap();
        assert!(rt.wine().ends_with("bin/wine64"));
    }

    #[test]
    fn detects_d3dmetal() {
        let dir = tempfile::tempdir().unwrap();
        fake_runtime(dir.path(), "wine64");
        let rt = WineRuntime::from_root("x", dir.path(), RuntimeKind::Custom).unwrap();
        assert!(!rt.has_d3dmetal());
        std::fs::create_dir_all(dir.path().join("lib/external/D3DMetal.framework")).unwrap();
        assert!(rt.has_d3dmetal());
    }

    #[test]
    fn discovers_custom_and_managed_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let managed = dir.path().join("runtimes");
        fake_runtime(&managed.join("wine-10"), "wine");
        std::fs::create_dir_all(managed.join("empty")).unwrap();

        let custom = vec![
            CustomRuntime {
                name: "mine".into(),
                path: managed.join("wine-10"),
            },
            CustomRuntime {
                name: "broken".into(),
                path: dir.path().join("missing"),
            },
        ];
        let found = discover(&custom, &managed);
        let ours: Vec<_> = found
            .iter()
            .filter(|r| r.root.starts_with(dir.path()))
            .collect();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].name, "mine");
        assert_eq!(ours[0].kind, RuntimeKind::Custom);
    }
}
