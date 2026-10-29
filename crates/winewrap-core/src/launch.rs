//! Describing and running processes inside a bottle.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::bottle::{Bottle, GraphicsBackend, SyncMode};
use crate::error::{Error, IoContext, Result};
use crate::runtime::WineRuntime;

/// DLLs that DXVK replaces.
const DXVK_DLLS: &str = "dxgi,d3d9,d3d10core,d3d11";
/// Microsoft's Visual C++ 2015-2022 runtime.
const VC_RUNTIME_DLLS: &str = "concrt140,msvcp140,msvcp140_1,msvcp140_2,\
    msvcp140_atomic_wait,msvcp140_codecvt_ids,vcomp140,vcruntime140,vcruntime140_1";

/// A fully-resolved command line: program, arguments, environment and working
/// directory. Building one has no side effects, so it can be inspected,
/// printed (`--dry-run`) or unit-tested before being executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl LaunchSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Build a `wine <program> <args...>` command for `bottle` using `runtime`,
    /// with the bottle's settings translated into environment variables.
    pub fn wine(bottle: &Bottle, runtime: &WineRuntime) -> Self {
        Self {
            program: runtime.wine(),
            args: Vec::new(),
            env: wine_env(bottle, runtime),
            cwd: None,
        }
    }

    /// `wineserver` for `bottle` (used to wait for or kill a prefix).
    pub fn wineserver(bottle: &Bottle, runtime: &WineRuntime) -> Self {
        Self {
            program: runtime.wineserver(),
            args: Vec::new(),
            env: wine_env(bottle, runtime),
            cwd: None,
        }
    }

    pub fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).envs(&self.env);
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }

    /// Run to completion and return the exit status; only failing to start
    /// is an error.
    pub fn status(&self) -> Result<ExitStatus> {
        self.to_command()
            .status()
            .map_err(|e| self.command_error(e.to_string()))
    }

    /// Run to completion, failing on a non-zero exit status.
    pub fn run(&self) -> Result<()> {
        let status = self.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(self.command_error(format!("exited with {status}")))
        }
    }

    /// Start in the background with stdout/stderr redirected to a log file in
    /// `logs_dir`. Returns the child and the log path.
    pub fn spawn_logged(&self, logs_dir: &Path, label: &str) -> Result<(Child, PathBuf)> {
        std::fs::create_dir_all(logs_dir).at(logs_dir)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let safe_label: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let log_path = logs_dir.join(format!("{safe_label}-{stamp}.log"));
        let log = File::create(&log_path).at(&log_path)?;
        let log_err = log.try_clone().at(&log_path)?;
        let child = self
            .to_command()
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .spawn()
            .map_err(|e| self.command_error(e.to_string()))?;
        Ok((child, log_path))
    }

    pub(crate) fn command_error(&self, message: String) -> Error {
        Error::Command {
            command: self.to_string(),
            message,
        }
    }
}

impl fmt::Display for LaunchSpec {
    /// Renders as a shell-pasteable command line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(cwd) = &self.cwd {
            write!(f, "cd {} && ", shell_quote(&cwd.to_string_lossy()))?;
        }
        for (k, v) in &self.env {
            write!(f, "{k}={} ", shell_quote(v))?;
        }
        write!(f, "{}", shell_quote(&self.program.to_string_lossy()))?;
        for a in &self.args {
            write!(f, " {}", shell_quote(&a.to_string_lossy()))?;
        }
        Ok(())
    }
}

fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Translate a bottle's settings into Wine environment variables.
pub fn wine_env(bottle: &Bottle, runtime: &WineRuntime) -> BTreeMap<String, String> {
    let cfg = &bottle.config;
    let mut env = BTreeMap::new();
    env.insert(
        "WINEPREFIX".into(),
        bottle.path.to_string_lossy().into_owned(),
    );
    env.insert("WINEDEBUG".into(), "-all".into());

    match cfg.sync {
        SyncMode::Msync => {
            env.insert("WINEMSYNC".into(), "1".into());
        }
        SyncMode::Esync => {
            env.insert("WINEESYNC".into(), "1".into());
        }
        SyncMode::None => {}
    }

    let mut overrides: Vec<String> = Vec::new();
    match cfg.graphics {
        GraphicsBackend::Dxvk => {
            overrides.push(format!("{DXVK_DLLS}=n,b"));
            if cfg.dxvk_async {
                env.insert("DXVK_ASYNC".into(), "1".into());
            }
        }
        // D3DMetal is Wine's built-in Direct3D on GPTK-based runtimes, so it
        // just needs the builtins to win over any native DLLs a game ships.
        GraphicsBackend::D3DMetal if runtime.has_d3dmetal() => {
            overrides.push(format!("{DXVK_DLLS},d3d12=b"));
        }
        GraphicsBackend::D3DMetal | GraphicsBackend::WineD3D => {}
    }
    // Use Microsoft's Visual C++ runtime once it's installed (Steam installs
    // it for most games): some games check its DLLs and reject Wine's own,
    // e.g. Phantom Blade Zero's launcher. Where it isn't installed, Wine
    // falls back to its builtins.
    overrides.push(format!("{VC_RUNTIME_DLLS}=n,b"));

    if cfg.metal_hud {
        env.insert("MTL_HUD_ENABLED".into(), "1".into());
    }
    if cfg.advertise_avx {
        env.insert("ROSETTA_ADVERTISE_AVX".into(), "1".into());
    }

    // User variables win, except that WINEDLLOVERRIDES is merged so a user
    // override does not silently drop the graphics backend's overrides.
    for (k, v) in &cfg.env {
        if k == "WINEDLLOVERRIDES" {
            overrides.push(v.clone());
        } else {
            env.insert(k.clone(), v.clone());
        }
    }
    env.insert("WINEDLLOVERRIDES".into(), overrides.join(";"));
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bottle::BottleConfig;
    use crate::runtime::RuntimeKind;

    fn fixture(cfg: BottleConfig, kind: RuntimeKind) -> (Bottle, WineRuntime, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        crate::runtime::tests::fake_runtime(&dir.path().join("rt"), "wine64");
        let rt = WineRuntime::from_root("rt", dir.path().join("rt"), kind).unwrap();
        let bottle = Bottle {
            path: dir.path().join("bottles/Steam"),
            config: cfg,
        };
        (bottle, rt, dir)
    }

    #[test]
    fn default_env() {
        let (bottle, rt, _d) = fixture(BottleConfig::new("Steam"), RuntimeKind::GamePortingToolkit);
        let env = wine_env(&bottle, &rt);
        assert_eq!(env["WINEPREFIX"], bottle.path.to_string_lossy());
        assert_eq!(env["WINEMSYNC"], "1");
        assert_eq!(
            env["WINEDLLOVERRIDES"],
            format!("dxgi,d3d9,d3d10core,d3d11,d3d12=b;{VC_RUNTIME_DLLS}=n,b")
        );
        assert!(!env.contains_key("MTL_HUD_ENABLED"));
    }

    #[test]
    fn dxvk_and_user_overrides_merge() {
        let mut cfg = BottleConfig::new("Steam");
        cfg.graphics = GraphicsBackend::Dxvk;
        cfg.dxvk_async = true;
        cfg.sync = SyncMode::Esync;
        cfg.metal_hud = true;
        cfg.env
            .insert("WINEDLLOVERRIDES".into(), "winemenubuilder.exe=d".into());
        cfg.env.insert("WINEDEBUG".into(), "+seh".into());
        let (bottle, rt, _d) = fixture(cfg, RuntimeKind::AppBundle);
        let env = wine_env(&bottle, &rt);
        assert_eq!(
            env["WINEDLLOVERRIDES"],
            format!("dxgi,d3d9,d3d10core,d3d11=n,b;{VC_RUNTIME_DLLS}=n,b;winemenubuilder.exe=d")
        );
        assert_eq!(env["DXVK_ASYNC"], "1");
        assert_eq!(env["WINEESYNC"], "1");
        assert_eq!(env["MTL_HUD_ENABLED"], "1");
        assert_eq!(env["WINEDEBUG"], "+seh");
        assert!(!env.contains_key("WINEMSYNC"));
    }

    #[test]
    fn d3dmetal_without_support_only_prefers_native_vc_runtime() {
        let (bottle, rt, _d) = fixture(BottleConfig::new("Steam"), RuntimeKind::AppBundle);
        assert_eq!(
            wine_env(&bottle, &rt)["WINEDLLOVERRIDES"],
            format!("{VC_RUNTIME_DLLS}=n,b")
        );
    }

    #[test]
    fn display_is_shell_quoted() {
        let mut spec = LaunchSpec::new("/opt/wine/bin/wine")
            .arg("C:\\Program Files (x86)\\Steam\\steam.exe")
            .arg("-silent");
        spec.env
            .insert("WINEPREFIX".into(), "/Users/me/My Bottles/Steam".into());
        assert_eq!(
            spec.to_string(),
            r"WINEPREFIX='/Users/me/My Bottles/Steam' /opt/wine/bin/wine 'C:\Program Files (x86)\Steam\steam.exe' -silent"
        );
    }
}
