//! Grafting Apple's D3DMetal (DirectX 12 → Metal) into a Wine build.
//!
//! D3DMetal is the translation layer that makes modern Direct3D 12 games run on
//! Apple Silicon. It ships inside Apple's Game Porting Toolkit, whose license
//! does not allow redistribution, so WineWrap never bundles it. Instead the user
//! installs the Game Porting Toolkit themselves and WineWrap copies the pieces
//! into a Wine build (ours included) — the same thing CrossOver and Whisky do.
//!
//! The payload is small and has a fixed shape:
//!
//! * `external/` — `D3DMetal.framework` plus `libd3dshared.dylib`, the native
//!   engine every DLL dlopens.
//! * `wine/x86_64-windows/` — the PE DLLs a game links against (`d3d12.dll`,
//!   `dxgi.dll`, …). D3DMetal folds `d3d12core` into `d3d12`, so there is no
//!   separate `d3d12core.dll`.
//! * `wine/x86_64-unix/` — one `*.so` per DLL, each a symlink to
//!   `../../external/libd3dshared.dylib`.
//!
//! The graft keeps Wine's own DirectX DLLs and puts D3DMetal's beside them in
//! `lib/d3dmetal/`, with `external/` at `lib/external` so the `../../external`
//! symlinks resolve and no Mach-O rpath or signature has to be touched. Our
//! Wine build (patch 0004) searches `lib/d3dmetal` first when
//! `WINEWRAP_D3DMETAL_DIR` points at it, except for the programs in
//! `WINEWRAP_WINED3D_APPS`: some launchers draw with D3D10 devices that only
//! Wine's own Direct3D can create, while Steam's UI needs D3DMetal's DXGI.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, IoContext, Result};

/// DLLs D3DMetal provides. `d3d12core` is present in newer Game Porting Toolkit
/// releases (older ones fold it into `d3d12`). The GPU-vendor shims (`atidxx64`,
/// `nvapi64`, `nvngx*`) are grafted too when the source has them, since some
/// games probe for them, but only `d3d12`/`d3d12core`/`dxgi` are required.
const D3DMETAL_DLLS: &[&str] = &[
    "d3d12",
    "d3d12core",
    "dxgi",
    "d3d11",
    "atidxx64",
    "nvapi64",
    "nvngx",
    "nvngx-on-metalfx",
];

/// The grafted DLL tree, relative to the runtime's `lib/`.
pub const D3DMETAL_DIR: &str = "d3dmetal";

/// Suffix older grafts gave the built-in DLLs they overwrote.
const BACKUP_SUFFIX: &str = ".pre-d3dmetal";

/// D3DMetal files located inside a Game Porting Toolkit install.
#[derive(Debug, Clone)]
pub struct D3DMetalSource {
    /// Directory holding `D3DMetal.framework` and `libd3dshared.dylib`.
    external: PathBuf,
    /// Directory holding the PE DLLs (`d3d12.dll`, …).
    windows: PathBuf,
}

impl D3DMetalSource {
    /// Locate D3DMetal under a Game Porting Toolkit root: an install directory,
    /// a mounted `Game Porting Toolkit-*.dmg`, or any Wine build that already
    /// ships it. Searches a few levels down for an `external/` directory that
    /// holds `D3DMetal.framework`, then expects the PE DLLs in the sibling
    /// `wine/x86_64-windows/`.
    pub fn locate(root: &Path) -> Result<Self> {
        let external =
            find_external(root, 0).ok_or_else(|| Error::D3DMetalNotFound(root.to_path_buf()))?;
        let windows = external
            .parent()
            .map(|base| base.join("wine/x86_64-windows"))
            .filter(|dir| dir.join("d3d12.dll").is_file())
            .ok_or_else(|| Error::D3DMetalNotFound(root.to_path_buf()))?;
        Ok(Self { external, windows })
    }

    /// Copy D3DMetal into the Wine build rooted at `runtime_root` (the directory
    /// containing `bin/` and `lib/`), replacing any previous graft. Wine's own
    /// DLLs are left alone; a graft from an older WineWrap that overwrote them
    /// is undone first. Afterwards `WineRuntime::has_d3dmetal()` is true for
    /// that build.
    pub fn graft_into(&self, runtime_root: &Path) -> Result<()> {
        remove(runtime_root)?;

        let lib = runtime_root.join("lib");
        let dst_external = lib.join("external");
        let dst_windows = lib.join(D3DMETAL_DIR).join("x86_64-windows");
        let dst_unix = lib.join(D3DMETAL_DIR).join("x86_64-unix");
        for dir in [&dst_windows, &dst_unix] {
            std::fs::create_dir_all(dir).at(dir)?;
        }

        // cp -R preserves the framework's nested symlinks and code signature;
        // it recreates the directory, so it must not exist first.
        copy_tree(&self.external, &dst_external)?;

        for name in D3DMETAL_DLLS {
            let dll = self.windows.join(format!("{name}.dll"));
            if !dll.is_file() {
                continue;
            }
            let dst_dll = dst_windows.join(format!("{name}.dll"));
            std::fs::copy(&dll, &dst_dll).at(&dst_dll)?;

            // The unix stub is just a symlink to the shared engine.
            let link = dst_unix.join(format!("{name}.so"));
            symlink("../../external/libd3dshared.dylib", &link).at(&link)?;
        }
        Ok(())
    }
}

/// Undo a graft in the Wine build at `runtime_root`: drop `lib/external` and
/// `lib/d3dmetal`, and restore any built-in DLLs an older graft overwrote. A
/// build with no graft is left untouched.
pub fn remove(runtime_root: &Path) -> Result<()> {
    let lib = runtime_root.join("lib");
    for dir in [lib.join("external"), lib.join(D3DMETAL_DIR)] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).at(&dir)?;
        }
    }
    for sub in ["wine/x86_64-windows", "wine/x86_64-unix"] {
        let dir = lib.join(sub);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(orig) = name.strip_suffix(BACKUP_SUFFIX) else {
                continue;
            };
            let target = dir.join(orig);
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&path, &target).at(&target)?;
        }
    }
    Ok(())
}

/// Depth-limited search for an `external/` directory that holds
/// `D3DMetal.framework`.
fn find_external(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    if dir.join("D3DMetal.framework").is_dir() && dir.join("libd3dshared.dylib").exists() {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir() && !e.path().is_symlink())
        .find_map(|e| find_external(&e.path(), depth + 1))
}

/// Recursively copy a directory with `cp -R`, which faithfully reproduces a
/// framework bundle's nested symlinks and code signature. `dst` must not exist.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    let describe = || format!("cp -R {} {}", src.display(), dst.display());
    let status = Command::new("/bin/cp")
        .arg("-R")
        .arg(src)
        .arg(dst)
        .status()
        .map_err(|e| Error::Command {
            command: describe(),
            message: e.to_string(),
        })?;
    if !status.success() {
        return Err(Error::Command {
            command: describe(),
            message: format!("exited with {status}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake Game Porting Toolkit payload under `root`.
    fn fake_gptk(root: &Path) {
        let external = root.join("lib/external");
        let framework = external.join("D3DMetal.framework/Versions/A");
        std::fs::create_dir_all(&framework).unwrap();
        std::fs::write(framework.join("D3DMetal"), b"engine").unwrap();
        symlink(
            "Versions/A/D3DMetal",
            external.join("D3DMetal.framework/D3DMetal"),
        )
        .unwrap();
        std::fs::write(external.join("libd3dshared.dylib"), b"shared").unwrap();

        let windows = root.join("lib/wine/x86_64-windows");
        std::fs::create_dir_all(&windows).unwrap();
        for name in ["d3d12", "dxgi", "d3d11"] {
            std::fs::write(windows.join(format!("{name}.dll")), b"pe").unwrap();
        }
    }

    #[test]
    fn locate_finds_payload_a_few_levels_down() {
        let dir = tempfile::tempdir().unwrap();
        fake_gptk(dir.path());
        let src = D3DMetalSource::locate(dir.path()).unwrap();
        assert!(src.external.join("D3DMetal.framework").is_dir());
        assert!(src.windows.join("d3d12.dll").is_file());
    }

    #[test]
    fn locate_errors_without_d3dmetal() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            D3DMetalSource::locate(dir.path()),
            Err(Error::D3DMetalNotFound(_))
        ));
    }

    #[test]
    fn graft_then_remove_round_trips() {
        let src_dir = tempfile::tempdir().unwrap();
        fake_gptk(src_dir.path());
        let src = D3DMetalSource::locate(src_dir.path()).unwrap();

        // A target build that already ships a vkd3d d3d12.dll + unix stub.
        let rt = tempfile::tempdir().unwrap();
        let win = rt.path().join("lib/wine/x86_64-windows");
        let unix = rt.path().join("lib/wine/x86_64-unix");
        std::fs::create_dir_all(&win).unwrap();
        std::fs::create_dir_all(&unix).unwrap();
        std::fs::write(win.join("d3d12.dll"), b"vkd3d").unwrap();
        std::fs::write(unix.join("d3d12.so"), b"vkd3d-unix").unwrap();

        src.graft_into(rt.path()).unwrap();
        let d3dmetal = rt.path().join("lib").join(D3DMETAL_DIR);
        assert!(rt.path().join("lib/external/D3DMetal.framework").is_dir());
        assert_eq!(
            std::fs::read(d3dmetal.join("x86_64-windows/d3d12.dll")).unwrap(),
            b"pe"
        );
        let link = d3dmetal.join("x86_64-unix/d3d12.so");
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            Path::new("../../external/libd3dshared.dylib")
        );
        assert_eq!(std::fs::read(&link).unwrap(), b"shared");
        // Wine's own DLLs stay in place for programs that keep wined3d.
        assert_eq!(std::fs::read(win.join("d3d12.dll")).unwrap(), b"vkd3d");
        assert_eq!(std::fs::read(unix.join("d3d12.so")).unwrap(), b"vkd3d-unix");

        // Grafting again replaces the previous graft.
        src.graft_into(rt.path()).unwrap();

        remove(rt.path()).unwrap();
        assert!(!rt.path().join("lib/external").exists());
        assert!(!d3dmetal.exists());
        assert_eq!(std::fs::read(win.join("d3d12.dll")).unwrap(), b"vkd3d");
    }

    #[test]
    fn graft_undoes_an_overwriting_graft() {
        let src_dir = tempfile::tempdir().unwrap();
        fake_gptk(src_dir.path());
        let src = D3DMetalSource::locate(src_dir.path()).unwrap();

        // Layout left by older WineWrap: D3DMetal over Wine's DLL, original
        // kept as *.pre-d3dmetal.
        let rt = tempfile::tempdir().unwrap();
        let win = rt.path().join("lib/wine/x86_64-windows");
        std::fs::create_dir_all(&win).unwrap();
        std::fs::write(win.join("dxgi.dll"), b"pe").unwrap();
        std::fs::write(win.join("dxgi.dll.pre-d3dmetal"), b"wine").unwrap();

        src.graft_into(rt.path()).unwrap();
        assert_eq!(std::fs::read(win.join("dxgi.dll")).unwrap(), b"wine");
        assert!(!win.join("dxgi.dll.pre-d3dmetal").exists());
        assert!(rt
            .path()
            .join("lib")
            .join(D3DMETAL_DIR)
            .join("x86_64-windows/dxgi.dll")
            .is_file());
    }
}
