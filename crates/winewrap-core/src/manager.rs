use std::path::{Path, PathBuf};

use crate::bottle::{self, Bottle, BottleConfig};
use crate::config::{AppConfig, CustomRuntime};
use crate::download;
use crate::error::{Error, IoContext, Result};
use crate::paths::AppPaths;
use crate::runtime::{self, RuntimeKind, WineRuntime};
use crate::steam;
use crate::system::{self, Check, Status};

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

    /// Path of the cached Steam installer, downloading it if needed.
    pub fn steam_installer(&self, progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
        let dest = self.paths.downloads_dir().join(steam::STEAM_SETUP_FILE);
        if !dest.is_file() {
            download::download(steam::STEAM_SETUP_URL, &dest, progress)?;
        }
        Ok(dest)
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
    fn runtime_registration_and_resolution() {
        let (mut ww, d) = setup();
        let root = d.path().join("my-wine");
        crate::runtime::tests::fake_runtime(&root, "wine");
        ww.add_runtime("mine", &root).unwrap();
        ww.set_default_runtime("mine").unwrap();

        // Reload from disk to make sure it persisted.
        let mut ww = WineWrap::with_paths(ww.paths.clone()).unwrap();
        assert_eq!(ww.default_runtime().unwrap().root, root);

        let b = ww.create_bottle(BottleConfig::new("b")).unwrap();
        assert_eq!(ww.runtime_for(&b).unwrap().name, "mine");

        ww.remove_runtime("mine").unwrap();
        assert_eq!(ww.config.default_runtime, None);
        assert!(ww.remove_runtime("mine").is_err());
    }
}
