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
    /// Build a runtime from a path to a Wine installation, validating it.
    ///
    /// `path` may be the install prefix (the directory containing `bin/wine`),
    /// its `bin/` directory, the `wine` binary itself, or a `.app` bundle such
    /// as `/Applications/Wine Stable.app`; see [`resolve_root`].
    pub fn from_root(
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        kind: RuntimeKind,
    ) -> Result<Self> {
        let path = path.into();
        let root = resolve_root(&path).ok_or(Error::NotAWineInstall(path))?;
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

/// Find the install prefix for something the user (or a search) points at.
///
/// Accepts the prefix itself, its `bin/` directory, a `wine`/`wine64` binary,
/// or a macOS `.app` bundle that embeds Wine (`Contents/Resources/wine` or
/// `Contents/SharedSupport/wine`). Symlinked binaries, such as the ones
/// Homebrew puts in `/opt/homebrew/bin`, are followed to the real install so
/// the same Wine is not listed twice.
pub fn resolve_root(path: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if path.is_file() {
        candidates.extend(path.parent().and_then(Path::parent).map(Path::to_path_buf));
    } else {
        candidates.push(path.to_path_buf());
        candidates.push(path.join("Contents/Resources/wine"));
        candidates.push(path.join("Contents/SharedSupport/wine"));
        if path.file_name().is_some_and(|n| n == "bin") {
            candidates.extend(path.parent().map(Path::to_path_buf));
        }
    }
    candidates.into_iter().find_map(|root| {
        let wine = WineRuntime::find_wine_binary(&root)?;
        // Follow a symlinked binary back to the prefix it really lives in.
        let real = std::fs::canonicalize(&wine)
            .ok()
            .and_then(|p| Some(p.parent()?.parent()?.to_path_buf()))
            .filter(|r| WineRuntime::find_wine_binary(r).is_some());
        Some(real.unwrap_or(root))
    })
}

/// A place a runtime may be installed, before we know whether it exists.
struct Candidate {
    name: String,
    root: PathBuf,
    kind: RuntimeKind,
}

/// Turn an app name like "Wine Crossover" into a runtime name like "wine-crossover".
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Every `.app` in `dirs` (normally `/Applications` and `~/Applications`). Only
/// the ones that really embed Wine survive [`WineRuntime::from_root`], so this
/// picks up any Wine bundle whatever it is called (Gcenx's `Wine Stable.app`,
/// `Wine Crossover.app`, `Game Porting Toolkit.app`, …).
fn app_bundle_candidates(dirs: &[PathBuf]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut apps: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
            .collect();
        apps.sort();
        for app in apps {
            let stem = app
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let kind = if stem.contains("Game Porting Toolkit") {
                RuntimeKind::GamePortingToolkit
            } else {
                RuntimeKind::AppBundle
            };
            out.push(Candidate {
                name: slug(&stem),
                root: app,
                kind,
            });
        }
    }
    out
}

/// GPTK and Whisky, which live in fixed places outside `/Applications`.
fn toolkit_candidates(home: Option<&Path>) -> Vec<Candidate> {
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

    out
}

/// Package-manager installs (Homebrew on Apple Silicon / Intel, Linux
/// distros). Homebrew casks symlink into these, which resolve_root follows.
fn prefix_candidates() -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut push = |name: &str, root: PathBuf, kind| {
        out.push(Candidate {
            name: name.to_string(),
            root,
            kind,
        })
    };
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
        // Hidden entries are in-progress installs (see wine_builds::install).
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| Candidate {
            name: e.file_name().to_string_lossy().into_owned(),
            root: e.path(),
            kind: RuntimeKind::Managed,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Directories searched for `.app` bundles that embed Wine.
pub fn app_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    dirs.extend(home.map(|h| h.join("Applications")));
    dirs
}

/// Discover all available runtimes.
///
/// Order matters: it is the order of preference when a bottle does not pin a
/// runtime. Custom runtimes come first, then managed ones, then GPTK/Whisky,
/// then `.app` bundles, then package-manager prefixes.
pub fn discover(custom: &[CustomRuntime], managed_dir: &Path) -> Vec<WineRuntime> {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    collect(
        custom
            .iter()
            .map(|c| Candidate {
                name: c.name.clone(),
                root: c.path.clone(),
                kind: RuntimeKind::Custom,
            })
            .chain(managed_candidates(managed_dir))
            .chain(toolkit_candidates(home.as_deref()))
            .chain(app_bundle_candidates(&app_dirs(home.as_deref())))
            .chain(prefix_candidates()),
    )
}

/// Validate candidates, dropping missing ones and duplicates. The same install
/// can be reachable twice (e.g. a Homebrew symlink and the app bundle it points
/// at); the first one wins, and names stay unique.
fn collect(candidates: impl Iterator<Item = Candidate>) -> Vec<WineRuntime> {
    let mut found: Vec<WineRuntime> = Vec::new();
    for c in candidates {
        let Ok(rt) = WineRuntime::from_root(c.name, c.root, c.kind) else {
            continue;
        };
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

    /// A directory reached through a symlink, like macOS temp dirs
    /// (`/var` -> `/private/var`), so tests on Linux see the same paths.
    pub(crate) fn symlinked_dir(base: &Path) -> PathBuf {
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        #[cfg(unix)]
        {
            let link = base.join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            link
        }
        #[cfg(not(unix))]
        real
    }

    /// Create a fake Wine install prefix for tests.
    pub(crate) fn fake_runtime(root: &Path, binary: &str) {
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join(binary), "").unwrap();
        std::fs::write(root.join("bin/wineserver"), "").unwrap();
    }

    #[test]
    fn resolves_app_bundles_bin_dirs_and_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Wine Stable.app");
        let prefix = app.join("Contents/Resources/wine");
        fake_runtime(&prefix, "wine");
        let canonical = std::fs::canonicalize(&prefix).unwrap();
        for input in [
            app.clone(),
            prefix.clone(),
            prefix.join("bin"),
            prefix.join("bin/wine"),
        ] {
            let got = resolve_root(&input).map(|p| std::fs::canonicalize(p).unwrap());
            assert_eq!(got.as_ref(), Some(&canonical), "input {}", input.display());
        }
        assert_eq!(resolve_root(dir.path()), None);
    }

    #[cfg(unix)]
    #[test]
    fn follows_homebrew_style_symlinks_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Applications/Wine Crossover.app");
        let prefix = app.join("Contents/Resources/wine");
        fake_runtime(&prefix, "wine");
        // A Homebrew-like prefix whose bin/ holds symlinks into the app.
        let brew = dir.path().join("homebrew");
        std::fs::create_dir_all(brew.join("bin")).unwrap();
        for bin in ["wine", "wineserver"] {
            std::os::unix::fs::symlink(prefix.join("bin").join(bin), brew.join("bin").join(bin))
                .unwrap();
        }
        assert_eq!(
            resolve_root(&brew).map(|p| std::fs::canonicalize(p).unwrap()),
            Some(std::fs::canonicalize(&prefix).unwrap())
        );

        let found = collect(
            app_bundle_candidates(&[dir.path().join("Applications")])
                .into_iter()
                .chain([Candidate {
                    name: "homebrew".into(),
                    root: brew,
                    kind: RuntimeKind::System,
                }]),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "wine-crossover");
        assert_eq!(found[0].kind, RuntimeKind::AppBundle);
    }

    #[test]
    fn app_scan_ignores_apps_without_wine() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Safari.app/Contents")).unwrap();
        fake_runtime(
            &dir.path()
                .join("Game Porting Toolkit.app/Contents/Resources/wine"),
            "wine64",
        );
        let found = collect(app_bundle_candidates(&[dir.path().to_path_buf()]).into_iter());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "game-porting-toolkit");
        assert_eq!(found[0].kind, RuntimeKind::GamePortingToolkit);
        assert!(found[0].has_d3dmetal());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Wine Crossover"), "wine-crossover");
        assert_eq!(
            slug("  Game Porting Toolkit 2.1 "),
            "game-porting-toolkit-2-1"
        );
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
        let base = symlinked_dir(dir.path());
        let managed = base.join("runtimes");
        fake_runtime(&managed.join("wine-10"), "wine");
        std::fs::create_dir_all(managed.join("empty")).unwrap();

        let custom = vec![
            CustomRuntime {
                name: "mine".into(),
                path: managed.join("wine-10"),
            },
            CustomRuntime {
                name: "broken".into(),
                path: base.join("missing"),
            },
        ];
        let found = discover(&custom, &managed);
        let ours: Vec<_> = found
            .iter()
            // Roots are resolved through symlinks.
            .filter(|r| {
                r.root
                    .starts_with(std::fs::canonicalize(dir.path()).unwrap())
            })
            .collect();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].name, "mine");
        assert_eq!(ours[0].kind, RuntimeKind::Custom);
    }
}
