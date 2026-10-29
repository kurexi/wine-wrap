use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::{Error, IoContext, Result};

/// Environment variable that relocates every WineWrap directory under one root.
/// Useful for tests, portable installs and keeping bottles on an external disk.
pub const HOME_ENV: &str = "WINEWRAP_HOME";

/// The directories WineWrap reads from and writes to.
///
/// On macOS these follow Apple's conventions, e.g.
/// `~/Library/Application Support/com.wine-wrap.WineWrap` for data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    /// Resolve the standard platform directories, honouring [`HOME_ENV`].
    pub fn from_env() -> Result<Self> {
        if let Some(root) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::rooted(PathBuf::from(root)));
        }
        let dirs = ProjectDirs::from("com", "wine-wrap", "WineWrap").ok_or(Error::NoHomeDir)?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    /// Put every directory below a single root.
    pub fn rooted(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn bottles_dir(&self) -> PathBuf {
        self.data_dir.join("bottles")
    }

    /// Wine builds that WineWrap manages itself (drop a build here and it is
    /// picked up automatically).
    pub fn runtimes_dir(&self) -> PathBuf {
        self.data_dir.join("runtimes")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.cache_dir.join("downloads")
    }

    /// Create all directories that WineWrap expects to exist.
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.bottles_dir(),
            &self.runtimes_dir(),
            &self.logs_dir(),
            &self.downloads_dir(),
        ] {
            std::fs::create_dir_all(dir).at(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_layout() {
        let paths = AppPaths::rooted("/x");
        assert_eq!(paths.config_file(), PathBuf::from("/x/config/config.toml"));
        assert_eq!(paths.bottles_dir(), PathBuf::from("/x/data/bottles"));
        assert_eq!(paths.downloads_dir(), PathBuf::from("/x/cache/downloads"));
    }
}
