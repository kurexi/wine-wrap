use std::path::{Path, PathBuf};

use crate::bottle::{self, Bottle, BottleConfig};
use crate::config::{AppConfig, CustomRuntime};
use crate::download;
use crate::error::{Error, IoContext, Result};
use crate::paths::AppPaths;
use crate::runtime::{self, RuntimeKind, WineRuntime};
use crate::steam;
use crate::system::{self, Check, Status};
use crate::wine_builds::{self, InstallProgress};

/// Entry point to the library: owns the paths and global configuration and
/// ties runtimes and bottles together. Front ends hold one of these.
#[derive(Debug, Clone)]
pub struct WineWrap {
    pub paths: AppPaths,
    pub config: AppConfig,
}

impl WineWrap {
    /// Open using the standard platform directories.
    pub fn open() -> Result<Self> {
        Self::with_paths(AppPaths::from_env()?)
    }

    pub fn with_paths(paths: AppPaths) -> Result<Self> {
        paths.ensure()?;
        let config = AppConfig::load(&paths.config_file())?;
        Ok(Self { paths, config })
    }

    pub fn save_config(&self) -> Result<()> {
        self.config.save(&self.paths.config_file())
    }

    // ---- runtimes -------------------------------------------------------

    pub fn runtimes(&self) -> Vec<WineRuntime> {
        runtime::discover(&self.config.runtimes, &self.paths.runtimes_dir())
    }

    pub fn runtime(&self, name: &str) -> Result<WineRuntime> {
        self.runtimes()
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| Error::RuntimeNotFound(name.to_string()))
    }

    /// The runtime to use when nothing is pinned: the configured default, or
    /// else the first discovered one.
    pub fn default_runtime(&self) -> Result<WineRuntime> {
        match &self.config.default_runtime {
            Some(name) => self.runtime(name),
            None => self.runtimes().into_iter().next().ok_or(Error::NoRuntime),
        }
    }

    /// Register a Wine installation by path. Returns the validated runtime.
    pub fn add_runtime(&mut self, name: &str, path: &Path) -> Result<WineRuntime> {
        let rt = WineRuntime::from_root(name, path, RuntimeKind::Custom)?;
        self.config.runtimes.retain(|r| r.name != name);
        self.config.runtimes.push(CustomRuntime {
            name: name.to_string(),
            path: path.to_path_buf(),
        });
        self.save_config()?;
        Ok(rt)
    }

    /// Register a Wine installation the user picked, naming it after the
    /// folder or `.app` (with a numeric suffix if that name is taken).
    pub fn add_runtime_from_path(&mut self, path: &Path) -> Result<WineRuntime> {
        let base = path
            .file_stem()
            .map(|s| runtime::slug(&s.to_string_lossy()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "wine".into());
        let taken: Vec<String> = self.runtimes().into_iter().map(|r| r.name).collect();
        let name = std::iter::once(base.clone())
            .chain((2..).map(|i| format!("{base}-{i}")))
            .find(|n| !taken.contains(n))
            .expect("an unused name always exists");
        self.add_runtime(&name, path)
    }

    pub fn remove_runtime(&mut self, name: &str) -> Result<()> {
        let before = self.config.runtimes.len();
        self.config.runtimes.retain(|r| r.name != name);
        if before == self.config.runtimes.len() {
            return Err(Error::RuntimeNotFound(name.to_string()));
        }
        if self.config.default_runtime.as_deref() == Some(name) {
            self.config.default_runtime = None;
        }
        self.save_config()
    }

    pub fn set_default_runtime(&mut self, name: &str) -> Result<()> {
        self.runtime(name)?;
        self.config.default_runtime = Some(name.to_string());
        self.save_config()
    }

    /// Graft Apple's D3DMetal (DirectX 12 → Metal) from a Game Porting Toolkit
    /// install into a Wine runtime, so its bottles can use the D3DMetal graphics
    /// backend. `gptk` is an install directory or a mounted
    /// `Game Porting Toolkit-*.dmg`. Returns the runtime, now reporting
    /// `has_d3dmetal()`.
    pub fn install_d3dmetal(&self, runtime: &str, gptk: &Path) -> Result<WineRuntime> {
        let rt = self.runtime(runtime)?;
        crate::d3dmetal::D3DMetalSource::locate(gptk)?.graft_into(&rt.root)?;
        self.runtime(runtime)
    }

    /// Remove a previously grafted D3DMetal from a runtime, restoring its
    /// original built-in DirectX DLLs.
    pub fn remove_d3dmetal(&self, runtime: &str) -> Result<()> {
        let rt = self.runtime(runtime)?;
        crate::d3dmetal::remove(&rt.root)
    }

    /// The runtime a bottle should run with.
    pub fn runtime_for(&self, bottle: &Bottle) -> Result<WineRuntime> {
        match &bottle.config.runtime {
            Some(name) => self.runtime(name),
            None => self.default_runtime(),
        }
    }

    // ---- bottles --------------------------------------------------------

    pub fn bottle_path(&self, name: &str) -> PathBuf {
        self.paths.bottles_dir().join(name)
    }

    pub fn bottles(&self) -> Result<Vec<Bottle>> {
        bottle::list(&self.paths.bottles_dir())
    }

    pub fn bottle(&self, name: &str) -> Result<Bottle> {
        bottle::validate_name(name)?;
        let path = self.bottle_path(name);
        if !path.join(bottle::BOTTLE_FILE).is_file() {
            return Err(Error::BottleNotFound(name.to_string()));
        }
        Bottle::load(path)
    }

    /// Create the bottle directory and its settings file. The prefix itself
    /// is populated by running [`Bottle::init_commands`].
    pub fn create_bottle(&self, config: BottleConfig) -> Result<Bottle> {
        bottle::validate_name(&config.name)?;
        let path = self.bottle_path(&config.name);
        if path.exists() {
            return Err(Error::BottleExists(config.name));
        }
        if let Some(rt) = &config.runtime {
            self.runtime(rt)?;
        }
        let bottle = Bottle { path, config };
        bottle.save()?;
        Ok(bottle)
    }

    pub fn delete_bottle(&self, name: &str) -> Result<()> {
        let bottle = self.bottle(name)?;
        std::fs::remove_dir_all(&bottle.path).at(&bottle.path)
    }

    // ---- steam ----------------------------------------------------------

    /// Point a Steam bottle's settings at WineWrap's own Wine build,
    /// downloading it first if needed, when [`steam::needs_winewrap_runtime`]
    /// says so. Returns whether `config` changed; the caller saves it.
    pub fn use_steam_runtime(
        &self,
        config: &mut BottleConfig,
        progress: impl FnMut(InstallProgress),
    ) -> Result<bool> {
        if !steam::needs_winewrap_runtime(config) {
            return Ok(false);
        }
        let build = wine_builds::winewrap_build();
        let name = build.runtime_name();
        if self.runtime(&name).is_err() {
            wine_builds::install(&self.paths, &build, progress)?;
        }
        config.runtime = Some(name);
        Ok(true)
    }

    /// Path of the cached Steam installer, downloading it if needed.
    pub fn steam_installer(&self, progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
        let dest = self.paths.downloads_dir().join(steam::STEAM_SETUP_FILE);
        if !dest.is_file() {
            download::download(steam::STEAM_SETUP_URL, &dest, progress)?;
        }
        Ok(dest)
    }

    /// Install Microsoft's Visual C++ runtime (64- and 32-bit) into a bottle,
    /// downloading the installers if they aren't cached.
    pub fn install_vc_runtime(
        &self,
        bottle: &Bottle,
        runtime: &WineRuntime,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<()> {
        for (url, file) in steam::VC_REDIST {
            let installer = self.paths.downloads_dir().join(file);
            if !installer.is_file() {
                download::download(url, &installer, &mut progress)?;
            }
            let spec = steam::vc_redist_command(bottle, runtime, &installer);
            let status = spec.status()?;
            // 1638: a newer version is already installed; 3010: reboot needed.
            if !matches!(status.code(), Some(0 | 1638 | 3010)) {
                return Err(spec.command_error(format!("exited with {status}")));
            }
        }
        Ok(())
    }

    // ---- diagnostics ----------------------------------------------------

    pub fn doctor(&self) -> Vec<Check> {
        let mut checks = system::host_checks();
        let runtimes = self.runtimes();
        checks.push(if runtimes.is_empty() {
            Check {
                name: "Wine runtime",
                status: Status::Error,
                detail: Error::NoRuntime.to_string(),
            }
        } else {
            let names: Vec<_> = runtimes.iter().map(|r| r.name.as_str()).collect();
            Check {
                name: "Wine runtime",
                status: Status::Ok,
                detail: format!("found {}", names.join(", ")),
            }
        });
        if !runtimes.is_empty() && !runtimes.iter().any(WineRuntime::has_d3dmetal) {
            checks.push(Check {
                name: "D3DMetal",
                status: Status::Warning,
                detail: "no runtime with D3DMetal found; DirectX 12 games will not run. \
                         Install Apple's Game Porting Toolkit or use the DXVK/WineD3D backends"
                    .into(),
            });
        }
        checks.push(Check {
            name: "Data directory",
            status: Status::Ok,
            detail: self.paths.data_dir.display().to_string(),
        });
        checks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (WineWrap, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ww = WineWrap::with_paths(AppPaths::rooted(dir.path())).unwrap();
        (ww, dir)
    }

    #[test]
    fn bottle_lifecycle() {
        let (ww, _d) = setup();
        let b = ww.create_bottle(BottleConfig::new("Steam")).unwrap();
        assert!(b.config_path().is_file());
        assert!(matches!(
            ww.create_bottle(BottleConfig::new("Steam")),
            Err(Error::BottleExists(_))
        ));
        assert_eq!(ww.bottles().unwrap().len(), 1);
        assert_eq!(ww.bottle("Steam").unwrap(), b);
        ww.delete_bottle("Steam").unwrap();
        assert!(matches!(ww.bottle("Steam"), Err(Error::BottleNotFound(_))));
        assert!(matches!(
            ww.bottle("../x"),
            Err(Error::InvalidBottleName(_))
        ));
    }

    #[test]
    fn steam_bottles_use_the_installed_winewrap_build() {
        let (ww, _d) = setup();
        let name = wine_builds::winewrap_build().runtime_name();
        // Already installed, so nothing is downloaded.
        crate::runtime::tests::fake_runtime(&ww.paths.runtimes_dir().join(&name), "wine");

        let mut cfg = BottleConfig::new("Steam");
        cfg.runtime = Some("wine-stable-11.0".into());
        let changed = ww.use_steam_runtime(&mut cfg, |_| {}).unwrap();
        assert!(changed);
        assert_eq!(cfg.runtime.as_deref(), Some(name.as_str()));
        // Already moved: nothing more to do.
        assert!(!ww.use_steam_runtime(&mut cfg, |_| {}).unwrap());

        let mut custom = BottleConfig::new("Steam");
        custom.runtime = Some("gptk".into());
        assert!(!ww.use_steam_runtime(&mut custom, |_| {}).unwrap());
        assert_eq!(custom.runtime.as_deref(), Some("gptk"));
    }

    #[test]
    fn create_rejects_unknown_runtime() {
        let (ww, _d) = setup();
        let mut cfg = BottleConfig::new("x");
        cfg.runtime = Some("does-not-exist".into());
        assert!(matches!(
            ww.create_bottle(cfg),
            Err(Error::RuntimeNotFound(_))
        ));
    }

    #[test]
    fn add_runtime_from_path_names_uniquely() {
        let (mut ww, d) = setup();
        let a = d.path().join("a/My Wine.app");
        let b = d.path().join("b/My Wine.app");
        crate::runtime::tests::fake_runtime(&a.join("Contents/Resources/wine"), "wine");
        crate::runtime::tests::fake_runtime(&b.join("Contents/Resources/wine"), "wine");
        assert_eq!(ww.add_runtime_from_path(&a).unwrap().name, "my-wine");
        assert_eq!(ww.add_runtime_from_path(&b).unwrap().name, "my-wine-2");
        assert!(ww.add_runtime_from_path(d.path()).is_err());
        assert_eq!(ww.config.runtimes.len(), 2);
    }

    #[test]
    fn runtime_registration_and_resolution() {
        let (mut ww, d) = setup();
        let root = crate::runtime::tests::symlinked_dir(d.path()).join("my-wine");
        crate::runtime::tests::fake_runtime(&root, "wine");
        ww.add_runtime("mine", &root).unwrap();
        ww.set_default_runtime("mine").unwrap();

        // Reload from disk to make sure it persisted.
        let mut ww = WineWrap::with_paths(ww.paths.clone()).unwrap();
        // Roots are resolved through symlinks (macOS: /var -> /private/var).
        assert_eq!(
            ww.default_runtime().unwrap().root,
            std::fs::canonicalize(&root).unwrap()
        );

        let b = ww.create_bottle(BottleConfig::new("b")).unwrap();
        assert_eq!(ww.runtime_for(&b).unwrap().name, "mine");

        ww.remove_runtime("mine").unwrap();
        assert_eq!(ww.config.default_runtime, None);
        assert!(ww.remove_runtime("mine").is_err());
    }
}
