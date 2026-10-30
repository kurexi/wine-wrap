//! Installing and running the Windows Steam client inside a bottle.

use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

use crate::bottle::{Bottle, BottleConfig};
use crate::config::write_atomic;
use crate::error::{Error, IoContext, Result};
use crate::launch::LaunchSpec;
use crate::runtime::WineRuntime;
use crate::vdf::{self, Vdf};
use crate::wine_builds::{self, WineFlavor};

/// Valve's official Windows installer.
pub const STEAM_SETUP_URL: &str =
    "https://cdn.cloudflare.steamstatic.com/client/installer/SteamSetup.exe";
pub const STEAM_SETUP_FILE: &str = "SteamSetup.exe";

/// Microsoft's Visual C++ 2015-2022 runtime installers (64- and 32-bit), as
/// download URL and cache file name. Most games need the runtime. Steam
/// installs it per game, but under Wine that doesn't always happen, so
/// WineWrap installs it along with Steam.
pub const VC_REDIST: [(&str, &str); 2] = [
    (
        "https://aka.ms/vs/17/release/vc_redist.x64.exe",
        "vc_redist.x64.exe",
    ),
    (
        "https://aka.ms/vs/17/release/vc_redist.x86.exe",
        "vc_redist.x86.exe",
    ),
];

/// Where `SteamSetup.exe` installs to by default.
const STEAM_WIN_DIR: &str = r"C:\Program Files (x86)\Steam";

/// An installed game, read from an `appmanifest_<appid>.acf` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamGame {
    pub app_id: u32,
    pub name: String,
    /// Host path of the game's install directory.
    pub install_dir: PathBuf,
    /// Whether Steam considers the app fully installed (no update pending).
    pub fully_installed: bool,
}

pub fn steam_dir(bottle: &Bottle) -> PathBuf {
    bottle
        .windows_to_host(STEAM_WIN_DIR)
        .expect("STEAM_WIN_DIR is a valid Windows path")
}

pub fn steam_exe(bottle: &Bottle) -> PathBuf {
    steam_dir(bottle).join("steam.exe")
}

pub fn is_installed(bottle: &Bottle) -> bool {
    steam_exe(bottle).is_file()
}

/// Whether a Steam bottle should be moved to WineWrap's own Wine build.
///
/// Under upstream Wine on macOS, Steam's window stays black: Chromium draws it
/// from a helper process, and Wine's macOS driver doesn't show that. That's
/// true for bottles on the default runtime and on Gcenx's builds (what
/// "Install Wine" downloads), as well as on older WineWrap builds. Bottles the
/// user pinned to anything else (GPTK, a custom Wine) are left alone.
pub fn needs_winewrap_runtime(config: &BottleConfig) -> bool {
    let Some(name) = &config.runtime else {
        return true;
    };
    if *name == wine_builds::winewrap_build().runtime_name() {
        return false;
    }
    WineFlavor::ALL
        .iter()
        .any(|f| name.starts_with(&format!("wine-{f}-")))
}

/// Run the Steam installer silently.
pub fn install_command(bottle: &Bottle, runtime: &WineRuntime, setup_exe: &Path) -> LaunchSpec {
    LaunchSpec::wine(bottle, runtime).arg(setup_exe).arg("/S")
}

/// Run a Visual C++ runtime installer silently.
pub fn vc_redist_command(bottle: &Bottle, runtime: &WineRuntime, installer: &Path) -> LaunchSpec {
    LaunchSpec::wine(bottle, runtime)
        .arg(installer)
        .args(["/install", "/quiet", "/norestart"])
}

/// Start the Steam client, with the bottle's extra Steam arguments.
pub fn launch_command(bottle: &Bottle, runtime: &WineRuntime) -> LaunchSpec {
    LaunchSpec::wine(bottle, runtime)
        .arg(steam_exe(bottle))
        .args(&bottle.config.steam_args)
        .cwd(steam_dir(bottle))
}

/// Start (or hand off to a running) Steam client and launch a game.
pub fn play_command(bottle: &Bottle, runtime: &WineRuntime, app_id: u32) -> LaunchSpec {
    launch_command(bottle, runtime).args(["-applaunch".to_string(), app_id.to_string()])
}

/// Steam library folders, as host paths. Always includes the main Steam
/// directory, followed by any extra libraries from `libraryfolders.vdf`.
pub fn library_folders(bottle: &Bottle) -> Vec<PathBuf> {
    let main = steam_dir(bottle);
    let mut folders = vec![main.clone()];

    let vdf_path = main.join("steamapps/libraryfolders.vdf");
    let Ok(text) = std::fs::read_to_string(&vdf_path) else {
        return folders;
    };
    let doc = match vdf::parse(&text) {
        Ok(doc) => doc,
        Err(e) => {
            tracing::warn!("{}: {e}", vdf_path.display());
            return folders;
        }
    };
    let Some(root) = doc.get("libraryfolders") else {
        return folders;
    };
    for (_, entry) in root.entries() {
        // Modern format: "0" { "path" "C:\\..." ... }; legacy: "1" "D:\\...".
        let win_path = entry.get("path").and_then(Vdf::as_str).or(entry.as_str());
        if let Some(host) = win_path.and_then(|p| bottle.windows_to_host(p)) {
            if !folders.contains(&host) {
                folders.push(host);
            }
        }
    }
    folders
}

/// Make each of the bottle's game folders (`BottleConfig::drives`) a Steam
/// library, so games can be installed there. Adds them to
/// `steamapps/libraryfolders.vdf`, which Steam reads when it starts. Folders
/// that are missing (disk not connected) are skipped, and so is everything
/// until Steam has run once and written that file. Returns the Windows paths
/// added.
pub fn register_libraries(bottle: &Bottle) -> Result<Vec<String>> {
    let vdf_path = steam_dir(bottle).join("steamapps/libraryfolders.vdf");
    let Ok(text) = std::fs::read_to_string(&vdf_path) else {
        return Ok(Vec::new());
    };
    let parse_error = |message: String| Error::Parse {
        path: vdf_path.clone(),
        message,
    };
    let mut doc = vdf::parse(&text).map_err(|e| parse_error(e.to_string()))?;
    let Vdf::Obj(top) = &mut doc else {
        unreachable!("parse returns an object");
    };
    let Some((_, Vdf::Obj(folders))) = top
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case("libraryfolders"))
    else {
        return Err(parse_error("no \"libraryfolders\" section".into()));
    };
    let known: Vec<PathBuf> = folders
        .iter()
        .filter_map(|(_, f)| f.get("path").and_then(Vdf::as_str))
        .filter_map(|p| bottle.windows_to_host(p))
        .collect();

    let mut added = Vec::new();
    for (letter, folder) in &bottle.config.drives {
        let win_path = format!("{}:\\", letter.to_uppercase());
        let registered = bottle
            .windows_to_host(&win_path)
            .is_some_and(|host| known.contains(&host));
        if registered || !folder.is_dir() {
            continue;
        }
        let content_id = library_content_id(folder)?;
        let key = folders
            .iter()
            .filter_map(|(k, _)| k.parse::<u32>().ok())
            .max()
            .map_or(0, |n| n + 1);
        let s = |v: &str| Vdf::Str(v.to_string());
        let entry = [
            ("path", s(&win_path)),
            ("label", s("")),
            ("contentid", s(&content_id)),
            ("totalsize", s("0")),
            ("update_clean_bytes_tally", s("0")),
            ("time_last_update_verified", s("0")),
            ("apps", Vdf::Obj(Vec::new())),
        ];
        folders.push((
            key.to_string(),
            Vdf::Obj(entry.into_iter().map(|(k, v)| (k.to_string(), v)).collect()),
        ));
        added.push(win_path);
    }
    if !added.is_empty() {
        write_atomic(&vdf_path, vdf::write(&doc).as_bytes())?;
    }
    Ok(added)
}

/// The content ID from a library folder's own `libraryfolder.vdf`. If the
/// folder isn't a Steam library yet, set it up as Steam does: a new ID in
/// `libraryfolder.vdf`, and an empty `steamapps/`.
fn library_content_id(folder: &Path) -> Result<String> {
    let file = folder.join("libraryfolder.vdf");
    let existing = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| vdf::parse(&text).ok())
        .and_then(|doc| {
            doc.at(&["libraryfolder", "contentid"])
                .and_then(Vdf::as_str)
                .map(str::to_string)
        });
    if let Some(id) = existing {
        return Ok(id);
    }
    // Steam's IDs are random non-negative 64-bit numbers.
    let id = (std::hash::RandomState::new().hash_one(folder) >> 1).to_string();
    let steamapps = folder.join("steamapps");
    std::fs::create_dir_all(&steamapps).at(&steamapps)?;
    let s = |v: &str| Vdf::Str(v.to_string());
    let doc = Vdf::Obj(vec![(
        "libraryfolder".into(),
        Vdf::Obj(vec![("contentid".into(), s(&id)), ("label".into(), s(""))]),
    )]);
    write_atomic(&file, vdf::write(&doc).as_bytes())?;
    Ok(id)
}

/// Parse one app manifest.
pub fn parse_manifest(text: &str, library: &Path) -> Option<SteamGame> {
    let doc = vdf::parse(text).ok()?;
    let state = doc.get("AppState")?;
    let field = |k| state.get(k).and_then(Vdf::as_str);
    let app_id = field("appid")?.parse().ok()?;
    let install_dir = library.join("steamapps/common").join(field("installdir")?);
    // StateFlags bit 2 (value 4) = StateFullyInstalled.
    let flags: u32 = field("StateFlags")
        .and_then(|f| f.parse().ok())
        .unwrap_or(0);
    Some(SteamGame {
        app_id,
        name: field("name").unwrap_or("Unknown").to_string(),
        install_dir,
        fully_installed: flags & 4 != 0,
    })
}

/// Every game installed in any library of the bottle, sorted by name.
/// Steam's own redistributable package (app 228980) is omitted.
pub fn installed_games(bottle: &Bottle) -> Vec<SteamGame> {
    const STEAMWORKS_REDIST: u32 = 228980;
    let mut games = Vec::new();
    for library in library_folders(bottle) {
        let Ok(entries) = std::fs::read_dir(library.join("steamapps")) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            match parse_manifest(&text, &library) {
                Some(g) if g.app_id != STEAMWORKS_REDIST => games.push(g),
                Some(_) => {}
                None => tracing::warn!("could not parse {}", entry.path().display()),
            }
        }
    }
    games.sort_by_key(|g| (g.name.to_lowercase(), g.app_id));
    games.dedup_by_key(|g| g.app_id);
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(app_id: u32, name: &str, flags: u32) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\t\"name\"\t\t\"{name}\"\n\
             \t\"StateFlags\"\t\t\"{flags}\"\n\t\"installdir\"\t\t\"{name}\"\n}}\n"
        )
    }

    #[test]
    fn steam_bottles_move_to_winewrap_wine() {
        let with = |runtime: Option<&str>| {
            let mut c = BottleConfig::new("Steam");
            c.runtime = runtime.map(str::to_string);
            needs_winewrap_runtime(&c)
        };
        assert!(with(None));
        assert!(with(Some("wine-stable-11.0_1")));
        assert!(with(Some("wine-devel-11.20")));
        assert!(with(Some("wine-staging-11.20")));
        assert!(with(Some("wine-winewrap-0.1.0")));
        assert!(with(Some("wine-winewrap-26.3.0")));
        assert!(with(Some("wine-winewrap-26.3.0-2")));
        assert!(with(Some("wine-winewrap-26.3.0-3")));
        assert!(!with(Some(&wine_builds::winewrap_build().runtime_name())));
        assert!(!with(Some("gptk")));
        assert!(!with(Some("my-wine")));
    }

    #[cfg(unix)]
    #[test]
    fn game_folders_become_steam_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let ssd = dir.path().join("SSD/SteamLibrary");
        std::fs::create_dir_all(&ssd).unwrap();
        let mut bottle = Bottle {
            path: dir.path().join("b"),
            config: BottleConfig::new("Steam"),
        };
        std::fs::create_dir_all(bottle.path.join("dosdevices")).unwrap();
        let apps = steam_dir(&bottle).join("steamapps");
        std::fs::create_dir_all(&apps).unwrap();
        let vdf_path = apps.join("libraryfolders.vdf");
        std::fs::write(
            &vdf_path,
            "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n}\n",
        )
        .unwrap();

        bottle.config.drives.insert("d".into(), ssd.clone());
        bottle
            .config
            .drives
            .insert("e".into(), dir.path().join("unplugged"));
        bottle.save().unwrap();
        assert_eq!(register_libraries(&bottle).unwrap(), [r"D:\"]);
        // Already registered: nothing to do.
        assert!(register_libraries(&bottle).unwrap().is_empty());

        let doc = vdf::parse(&std::fs::read_to_string(&vdf_path).unwrap()).unwrap();
        let entry = doc.at(&["libraryfolders", "1"]).unwrap();
        assert_eq!(entry.get("path").and_then(Vdf::as_str), Some(r"D:\"));
        let id = entry.get("contentid").and_then(Vdf::as_str).unwrap();
        assert_eq!(library_content_id(&ssd).unwrap(), id);
        assert!(ssd.join("steamapps").is_dir());
        assert_eq!(
            library_folders(&bottle)[1],
            bottle.path.join("dosdevices/d:")
        );
    }

    #[test]
    fn finds_games_across_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let bottle = Bottle {
            path: dir.path().to_path_buf(),
            config: BottleConfig::new("Steam"),
        };
        let main = steam_dir(&bottle);
        let apps = main.join("steamapps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("appmanifest_620.acf"),
            manifest(620, "Portal 2", 4),
        )
        .unwrap();
        std::fs::write(
            apps.join("appmanifest_228980.acf"),
            manifest(228980, "Steamworks Common Redistributables", 4),
        )
        .unwrap();
        std::fs::write(apps.join("appmanifest_bad.acf"), "garbage {").unwrap();

        // Second library on D:, reached through dosdevices.
        let lib2 = dir.path().join("ExternalLibrary");
        std::fs::create_dir_all(lib2.join("steamapps")).unwrap();
        std::fs::write(
            lib2.join("steamapps/appmanifest_70.acf"),
            manifest(70, "Half-Life", 1026),
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("dosdevices")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&lib2, dir.path().join("dosdevices/d:")).unwrap();

        std::fs::write(
            apps.join("libraryfolders.vdf"),
            "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\
             \t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\\"\n\t}\n}\n",
        )
        .unwrap();

        let libs = library_folders(&bottle);
        assert_eq!(libs, vec![main.clone(), dir.path().join("dosdevices/d:")]);

        let games = installed_games(&bottle);
        let names: Vec<_> = games
            .iter()
            .map(|g| (g.app_id, g.name.as_str(), g.fully_installed))
            .collect();
        #[cfg(unix)]
        assert_eq!(
            names,
            vec![(70, "Half-Life", false), (620, "Portal 2", true)]
        );
        assert_eq!(
            games.iter().find(|g| g.app_id == 620).unwrap().install_dir,
            main.join("steamapps/common/Portal 2")
        );
    }
}
