use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};

/// Global, user-editable settings stored in `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Name of the runtime new bottles use when none is given.
    pub default_runtime: Option<String>,
    /// Wine installations registered by hand, in addition to auto-detected ones.
    #[serde(rename = "runtime")]
    pub runtimes: Vec<CustomRuntime>,
}

/// A Wine installation the user registered explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomRuntime {
    pub name: String,
    /// Directory that contains `bin/wine` (or `bin/wine64`).
    pub path: PathBuf,
}

impl AppConfig {
    /// Load the configuration, returning defaults if the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| Error::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).at(path),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        write_atomic(path, toml::to_string_pretty(self)?.as_bytes())
    }
}

/// Write a file by writing a sibling temp file and renaming it into place, so a
/// crash never leaves a half-written config behind.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).at(&tmp)?;
    std::fs::rename(&tmp, path).at(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig::load(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(cfg, AppConfig::default());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = AppConfig {
            default_runtime: Some("gptk".into()),
            runtimes: vec![CustomRuntime {
                name: "gptk".into(),
                path: "/usr/local/opt/game-porting-toolkit".into(),
            }],
        };
        cfg.save(&path).unwrap();
        assert_eq!(AppConfig::load(&path).unwrap(), cfg);
    }
}
