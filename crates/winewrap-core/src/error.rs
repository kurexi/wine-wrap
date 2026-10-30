use std::path::PathBuf;

/// Errors produced by the WineWrap core library.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse {path}: {message}")]
    Parse { path: PathBuf, message: String },

    #[error("failed to serialize configuration: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("could not determine a home directory for this user")]
    NoHomeDir,

    #[error("invalid bottle name {0:?}: use letters, digits, spaces, '-', '_' or '.', and do not start with '.'")]
    InvalidBottleName(String),

    #[error("bottle {0:?} already exists")]
    BottleExists(String),

    #[error("bottle {0:?} not found")]
    BottleNotFound(String),

    #[error("Wine runtime {0:?} not found")]
    RuntimeNotFound(String),

    #[error("no Wine runtime found; install one (e.g. `brew install --cask gcenx/wine/wine-crossover`, or a build from https://github.com/Gcenx/macOS_Wine_builds/releases into /Applications) or register one with `winewrap runtime add <name> <path>`")]
    NoRuntime,

    #[error("{0} is not a Wine installation (no bin/wine or bin/wine64 found)")]
    NotAWineInstall(PathBuf),

    #[error("no D3DMetal found under {0}; install Apple's Game Porting Toolkit (or mount its disk image) and point at it")]
    D3DMetalNotFound(PathBuf),

    #[error("Steam is not installed in bottle {0:?}")]
    SteamNotInstalled(String),

    #[error("download of {url} failed: {message}")]
    Download { url: String, message: String },

    #[error("command `{command}` failed: {message}")]
    Command { command: String, message: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Small extension trait to attach a path to `std::io::Error`s.
pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
