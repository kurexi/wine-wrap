//! Bottles: isolated Wine prefixes plus the settings used to run them.
//!
//! As in CrossOver, a bottle is just a `WINEPREFIX` directory; its settings live
//! in a `bottle.toml` at the root of that directory, so a bottle can be copied
//! or backed up as a single folder.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::config::write_atomic;
use crate::error::{Error, IoContext, Result};
use crate::launch::LaunchSpec;
use crate::runtime::WineRuntime;

pub const BOTTLE_FILE: &str = "bottle.toml";

/// Which Direct3D implementation games should use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsBackend {
    /// Apple's D3DMetal (Game Porting Toolkit). Best DirectX 11/12 support on
    /// Apple Silicon; requires a GPTK-based runtime.
    #[default]
    D3DMetal,
    /// DXVK (Vulkan via MoltenVK). DLLs must be installed into the bottle.
    Dxvk,
    /// Wine's own OpenGL-based Direct3D.
    WineD3D,
}

/// Wine's synchronisation primitive implementation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Mach-semaphore based sync; fastest on macOS where supported.
    #[default]
    Msync,
    /// eventfd-style sync.
    Esync,
    None,
}

/// The Windows version the prefix reports to applications.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowsVersion {
    #[serde(rename = "win7")]
    Win7,
    #[serde(rename = "win8")]
    Win8,
    #[serde(rename = "win81")]
    Win81,
    #[default]
    #[serde(rename = "win10")]
    Win10,
    #[serde(rename = "win11")]
    Win11,
}

macro_rules! str_enum {
    ($ty:ty { $($variant:ident => $s:literal),+ $(,)? }) => {
        impl $ty {
            pub const ALL: &'static [$ty] = &[$(<$ty>::$variant),+];

            pub fn as_str(&self) -> &'static str {
                match self { $(<$ty>::$variant => $s),+ }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $ty {
            type Err = String;

            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                match s.to_ascii_lowercase().as_str() {
                    $($s => Ok(<$ty>::$variant),)+
                    _ => Err(format!(
                        "expected one of: {}",
                        [$($s),+].join(", ")
                    )),
                }
            }
        }
    };
}

str_enum!(GraphicsBackend { D3DMetal => "d3dmetal", Dxvk => "dxvk", WineD3D => "wined3d" });
str_enum!(SyncMode { Msync => "msync", Esync => "esync", None => "none" });
str_enum!(WindowsVersion { Win7 => "win7", Win8 => "win8", Win81 => "win81", Win10 => "win10", Win11 => "win11" });

/// Per-bottle settings (`bottle.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BottleConfig {
    pub name: String,
    /// Runtime to use; `None` means the global default / best available.
    pub runtime: Option<String>,
    pub windows_version: WindowsVersion,
    pub graphics: GraphicsBackend,
    pub sync: SyncMode,
    /// Render at native Retina resolution instead of scaled.
    pub retina: bool,
    /// Show Apple's Metal performance HUD.
    pub metal_hud: bool,
    /// Make Rosetta 2 advertise AVX to x86_64 code (macOS 15+); some newer
    /// games refuse to start without it.
    pub advertise_avx: bool,
    /// Compile DXVK shaders asynchronously (only meaningful with dxvk-async builds).
    pub dxvk_async: bool,
    /// Extra arguments passed to `steam.exe`.
    pub steam_args: Vec<String>,
    /// Extra environment variables for every process in the bottle.
    pub env: BTreeMap<String, String>,
    /// Extra Windows drives: letter (`d` to `y`) → Mac folder, e.g. a Steam
    /// library on an external disk.
    pub drives: BTreeMap<String, PathBuf>,
}

impl Default for BottleConfig {
    fn default() -> Self {
        Self::new("")
    }
}

impl BottleConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            runtime: None,
            windows_version: WindowsVersion::default(),
            graphics: GraphicsBackend::default(),
            sync: SyncMode::default(),
            retina: false,
            metal_hud: false,
            advertise_avx: false,
            dxvk_async: false,
            steam_args: Vec::new(),
            env: BTreeMap::new(),
            drives: BTreeMap::new(),
        }
    }

    /// Set a setting from its `bottle.toml` key and a string value; used by
    /// `winewrap bottle set`.
    pub fn set(&mut self, key: &str, value: &str) -> std::result::Result<(), String> {
        fn flag(v: &str) -> std::result::Result<bool, String> {
            match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                _ => Err(format!("expected true/false, got {v:?}")),
            }
        }
        match key {
            "runtime" => self.runtime = (!value.is_empty()).then(|| value.to_string()),
            "windows_version" => self.windows_version = value.parse()?,
            "graphics" => self.graphics = value.parse()?,
            "sync" => self.sync = value.parse()?,
            "retina" => self.retina = flag(value)?,
            "metal_hud" => self.metal_hud = flag(value)?,
            "advertise_avx" => self.advertise_avx = flag(value)?,
            "dxvk_async" => self.dxvk_async = flag(value)?,
            "steam_args" => {
                self.steam_args = value.split_whitespace().map(str::to_string).collect()
            }
            _ => {
                if let Some(var) = key.strip_prefix("env.") {
                    if value.is_empty() {
                        self.env.remove(var);
                    } else {
                        self.env.insert(var.to_string(), value.to_string());
                    }
                } else if let Some(letter) = key.strip_prefix("drive.") {
                    let letter = drive_letter(letter)?;
                    if value.is_empty() {
                        self.drives.remove(&letter);
                    } else {
                        self.drives.insert(letter, PathBuf::from(value));
                    }
                } else {
                    return Err(format!(
                        "unknown setting {key:?}; known: runtime, windows_version, graphics, sync, \
                         retina, metal_hud, advertise_avx, dxvk_async, steam_args, env.<VAR>, \
                         drive.<letter>"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A bottle on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bottle {
    /// The `WINEPREFIX` directory.
    pub path: PathBuf,
    pub config: BottleConfig,
}

impl Bottle {
    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn config_path(&self) -> PathBuf {
        self.path.join(BOTTLE_FILE)
    }

    pub fn drive_c(&self) -> PathBuf {
        self.path.join("drive_c")
    }

    /// Whether `wineboot` has populated the prefix.
    pub fn is_initialized(&self) -> bool {
        self.path.join("system.reg").is_file()
    }

    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = path.join(BOTTLE_FILE);
        let text = std::fs::read_to_string(&file).at(&file)?;
        let mut config: BottleConfig = toml::from_str(&text).map_err(|e| Error::Parse {
            path: file,
            message: e.to_string(),
        })?;
        if config.name.is_empty() {
            config.name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        Ok(Self { path, config })
    }

    /// Write `bottle.toml` and update the prefix's drive links to match.
    pub fn save(&self) -> Result<()> {
        let previous = Self::load(&self.path)
            .map(|b| b.config.drives)
            .unwrap_or_default();
        std::fs::create_dir_all(&self.path).at(&self.path)?;
        write_atomic(
            &self.config_path(),
            toml::to_string_pretty(&self.config)?.as_bytes(),
        )?;
        self.sync_drives(&previous)
    }

    /// Point `dosdevices/<letter>:` at each configured drive, and remove the
    /// links of drives in `previous` that are no longer configured. Only
    /// symlinks are touched. Does nothing before `wineboot` has created
    /// `dosdevices`.
    pub fn sync_drives(&self, previous: &BTreeMap<String, PathBuf>) -> Result<()> {
        let dir = self.path.join("dosdevices");
        if !dir.is_dir() {
            return Ok(());
        }
        let link = |letter: &str| dir.join(format!("{letter}:"));
        for letter in previous.keys() {
            let path = link(letter);
            if !self.config.drives.contains_key(letter) && path.is_symlink() {
                std::fs::remove_file(&path).at(&path)?;
            }
        }
        for (letter, target) in &self.config.drives {
            if drive_letter(letter).is_err() {
                tracing::warn!("{}: ignoring invalid drive {letter:?}", self.name());
                continue;
            }
            let path = link(letter);
            if std::fs::read_link(&path).is_ok_and(|t| t == *target) {
                continue;
            }
            if path.is_symlink() {
                std::fs::remove_file(&path).at(&path)?;
            } else if path.exists() {
                tracing::warn!("{}: not replacing {}", self.name(), path.display());
                continue;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &path).at(&path)?;
        }
        Ok(())
    }

    /// The first drive letter that is neither configured nor already used by
    /// Wine in this prefix.
    pub fn free_drive_letter(&self) -> Option<String> {
        ('d'..='y').map(String::from).find(|l| {
            !self.config.drives.contains_key(l)
                && !self.path.join("dosdevices").join(format!("{l}:")).exists()
        })
    }

    /// Convert a Windows path inside this bottle (e.g. `C:\Games\x.exe`) to a
    /// host path, following the prefix's `dosdevices` drive links.
    pub fn windows_to_host(&self, win_path: &str) -> Option<PathBuf> {
        let (drive, rest) = win_path.split_once(':')?;
        if drive.len() != 1 || !drive.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let drive = drive.to_ascii_lowercase();
        let base = if drive == "c" {
            self.drive_c()
        } else {
            self.path.join("dosdevices").join(format!("{drive}:"))
        };
        Some(
            rest.split(['\\', '/'])
                .filter(|s| !s.is_empty())
                .fold(base, |p, s| p.join(s)),
        )
    }

    /// Commands that turn an empty directory into a configured prefix.
    pub fn init_commands(&self, runtime: &WineRuntime) -> Vec<LaunchSpec> {
        let mut cmds = vec![LaunchSpec::wine(self, runtime).args(["wineboot", "--init"])];
        cmds.extend(self.apply_commands(runtime));
        cmds
    }

    /// Commands that push the bottle's settings into the prefix registry.
    /// Safe to re-run after changing settings.
    pub fn apply_commands(&self, runtime: &WineRuntime) -> Vec<LaunchSpec> {
        let wine = || LaunchSpec::wine(self, runtime);
        vec![
            wine().args(["winecfg", "-v", self.config.windows_version.as_str()]),
            wine().args([
                "reg",
                "add",
                r"HKCU\Software\Wine\Mac Driver",
                "/v",
                "RetinaMode",
                "/t",
                "REG_SZ",
                "/d",
                if self.config.retina { "y" } else { "n" },
                "/f",
            ]),
            LaunchSpec::wineserver(self, runtime).arg("--wait"),
        ]
    }

    /// Install DXVK from an extracted release (a directory containing `x64/`
    /// and optionally `x32/` folders of DLLs, as shipped by DXVK and
    /// DXVK-macOS). Returns the number of DLLs copied.
    pub fn install_dxvk(&self, dxvk_dir: &Path) -> Result<usize> {
        let mut copied = 0;
        for (src, dst) in [("x64", "windows/system32"), ("x32", "windows/syswow64")] {
            let src = dxvk_dir.join(src);
            let dst = self.drive_c().join(dst);
            let Ok(entries) = std::fs::read_dir(&src) else {
                continue;
            };
            std::fs::create_dir_all(&dst).at(&dst)?;
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dll = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("dll"));
                if is_dll {
                    let target = dst.join(entry.file_name());
                    std::fs::copy(&path, &target).at(&target)?;
                    copied += 1;
                }
            }
        }
        if copied == 0 {
            return Err(Error::Parse {
                path: dxvk_dir.to_path_buf(),
                message: "no DLLs found in x64/ or x32/; point at an extracted DXVK release".into(),
            });
        }
        Ok(copied)
    }

    /// Stop every Wine process running in this bottle.
    pub fn kill_command(&self, runtime: &WineRuntime) -> LaunchSpec {
        LaunchSpec::wineserver(self, runtime).arg("--kill")
    }

    /// Run [`Self::kill_command`]. Returns `false` if nothing was running:
    /// `wineserver --kill` exits with 1 when the prefix has no server.
    pub fn kill(&self, runtime: &WineRuntime) -> Result<bool> {
        let spec = self.kill_command(runtime);
        let status = spec.status()?;
        match status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(spec.command_error(format!("exited with {status}"))),
        }
    }
}

/// Normalise a drive letter for [`BottleConfig::drives`] (`"D:"` → `"d"`).
/// Only D: to Y: are allowed: Wine uses C: for the bottle and Z: for the Mac's
/// whole file system.
pub fn drive_letter(s: &str) -> std::result::Result<String, String> {
    match s.trim_end_matches(':').to_ascii_lowercase().as_bytes() {
        [c @ b'd'..=b'y'] => Ok(char::from(*c).to_string()),
        _ => Err(format!("drive letter must be D to Y, got {s:?}")),
    }
}

/// Bottle names become directory names, so keep them filesystem-friendly.
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.trim().is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidBottleName(name.to_string()))
    }
}

/// All bottles under `dir`, sorted by name. Directories without a valid
/// `bottle.toml` are skipped with a warning.
pub fn list(dir: &Path) -> Result<Vec<Bottle>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).at(dir),
    };
    let mut bottles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.join(BOTTLE_FILE).is_file() {
            continue;
        }
        match Bottle::load(&path) {
            Ok(b) => bottles.push(b),
            Err(e) => tracing::warn!("skipping bottle at {}: {e}", path.display()),
        }
    }
    bottles.sort_by_key(|b| b.name().to_lowercase());
    Ok(bottles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for good in ["Steam", "My Games", "gptk-2.0", "Ünïcode_ok"] {
            validate_name(good).unwrap();
        }
        for bad in ["", "  ", ".hidden", "a/b", "..", "x\0y", "a:b"] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn save_load_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = BottleConfig::new("Steam");
        cfg.graphics = GraphicsBackend::Dxvk;
        cfg.env.insert("FOO".into(), "bar".into());
        let bottle = Bottle {
            path: dir.path().join("Steam"),
            config: cfg,
        };
        bottle.save().unwrap();
        std::fs::create_dir_all(dir.path().join("not-a-bottle")).unwrap();

        let all = list(dir.path()).unwrap();
        assert_eq!(all, vec![bottle]);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Old");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(BOTTLE_FILE), "graphics = \"wined3d\"\n").unwrap();
        let b = Bottle::load(&path).unwrap();
        assert_eq!(b.name(), "Old");
        assert_eq!(b.config.graphics, GraphicsBackend::WineD3D);
        assert_eq!(b.config.sync, SyncMode::Msync);
    }

    #[test]
    fn set_values() {
        let mut cfg = BottleConfig::new("x");
        cfg.set("graphics", "DXVK").unwrap();
        cfg.set("retina", "on").unwrap();
        cfg.set("windows_version", "win11").unwrap();
        cfg.set("steam_args", "-cef-disable-gpu  -silent").unwrap();
        cfg.set("env.DXVK_HUD", "fps").unwrap();
        assert_eq!(cfg.graphics, GraphicsBackend::Dxvk);
        assert!(cfg.retina);
        assert_eq!(cfg.windows_version, WindowsVersion::Win11);
        assert_eq!(cfg.steam_args, ["-cef-disable-gpu", "-silent"]);
        assert_eq!(cfg.env["DXVK_HUD"], "fps");
        cfg.set("env.DXVK_HUD", "").unwrap();
        assert!(cfg.env.is_empty());
        assert!(cfg.set("graphics", "vulkan").is_err());
        assert!(cfg.set("nope", "1").is_err());
        cfg.set("drive.E:", "/Volumes/SSD/SteamLibrary").unwrap();
        assert_eq!(cfg.drives["e"], PathBuf::from("/Volumes/SSD/SteamLibrary"));
        cfg.set("drive.e", "").unwrap();
        assert!(cfg.drives.is_empty());
        for bad in ["c", "z", "dd", "1", ""] {
            assert!(cfg.set(&format!("drive.{bad}"), "/x").is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn drives_become_dosdevices_links() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("SSD/SteamLibrary");
        std::fs::create_dir_all(&ext).unwrap();
        let mut b = Bottle {
            path: dir.path().join("b"),
            config: BottleConfig::new("b"),
        };
        let dosdevices = b.path.join("dosdevices");
        std::fs::create_dir_all(&dosdevices).unwrap();
        std::os::unix::fs::symlink("/", dosdevices.join("z:")).unwrap();
        // A drive Wine set up itself is left alone and isn't offered again.
        std::os::unix::fs::symlink("/tmp", dosdevices.join("d:")).unwrap();
        assert_eq!(b.free_drive_letter().as_deref(), Some("e"));

        b.config.drives.insert("e".into(), ext.clone());
        b.save().unwrap();
        assert_eq!(std::fs::read_link(dosdevices.join("e:")).unwrap(), ext);
        assert_eq!(
            b.windows_to_host(r"E:\steamapps"),
            Some(dosdevices.join("e:/steamapps"))
        );
        assert_eq!(b.free_drive_letter().as_deref(), Some("f"));

        // Moving the drive updates the link; removing it deletes the link.
        b.config.drives.insert("e".into(), dir.path().join("SSD"));
        b.save().unwrap();
        assert_eq!(
            std::fs::read_link(dosdevices.join("e:")).unwrap(),
            dir.path().join("SSD")
        );
        b.config.drives.clear();
        b.save().unwrap();
        assert!(!dosdevices.join("e:").is_symlink());
        assert!(dosdevices.join("d:").is_symlink() && dosdevices.join("z:").is_symlink());
    }

    #[test]
    fn dxvk_install() {
        let dir = tempfile::tempdir().unwrap();
        let dxvk = dir.path().join("dxvk-2.4");
        std::fs::create_dir_all(dxvk.join("x64")).unwrap();
        std::fs::create_dir_all(dxvk.join("x32")).unwrap();
        for d in [
            "x64/d3d11.dll",
            "x64/dxgi.dll",
            "x32/d3d11.dll",
            "x64/README.txt",
        ] {
            std::fs::write(dxvk.join(d), "").unwrap();
        }
        let b = Bottle {
            path: dir.path().join("b"),
            config: BottleConfig::new("b"),
        };
        assert_eq!(b.install_dxvk(&dxvk).unwrap(), 3);
        assert!(b.drive_c().join("windows/system32/dxgi.dll").is_file());
        assert!(b.drive_c().join("windows/syswow64/d3d11.dll").is_file());
        assert!(b.install_dxvk(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn kill_treats_exit_1_as_nothing_running() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("rt");
        crate::runtime::tests::fake_runtime(&root, "wine");
        let rt = WineRuntime::from_root("rt", &root, crate::runtime::RuntimeKind::Custom).unwrap();
        let b = Bottle {
            path: dir.path().join("b"),
            config: BottleConfig::new("b"),
        };
        let wineserver = root.join("bin/wineserver");
        for (code, expected) in [(0, Some(true)), (1, Some(false)), (2, None)] {
            std::fs::write(&wineserver, format!("#!/bin/sh\nexit {code}\n")).unwrap();
            std::fs::set_permissions(&wineserver, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(b.kill(&rt).ok(), expected, "exit {code}");
        }
    }

    #[test]
    fn windows_paths() {
        let b = Bottle {
            path: "/b".into(),
            config: BottleConfig::new("b"),
        };
        assert_eq!(
            b.windows_to_host(r"C:\Program Files (x86)\Steam"),
            Some(PathBuf::from("/b/drive_c/Program Files (x86)/Steam"))
        );
        assert_eq!(
            b.windows_to_host(r"D:\\SteamLibrary"),
            Some(PathBuf::from("/b/dosdevices/d:/SteamLibrary"))
        );
        assert_eq!(b.windows_to_host("relative\\path"), None);
    }
}
