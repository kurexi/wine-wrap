//! Installing Wine without Homebrew.
//!
//! Downloads a prebuilt macOS Wine from [Gcenx/macOS_Wine_builds] (the same
//! builds Homebrew's `wine-stable` cask installs), or WineWrap's own build
//! for Steam, and unpacks it into WineWrap's managed runtimes directory,
//! where [`crate::runtime::discover`] picks it up. Files WineWrap downloads
//! itself are not quarantined by macOS, so the runtime works without clearing
//! Gatekeeper flags.
//!
//! [Gcenx/macOS_Wine_builds]: https://github.com/Gcenx/macOS_Wine_builds

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::download;
use crate::error::{Error, IoContext, Result};
use crate::paths::AppPaths;
use crate::runtime::{resolve_root, RuntimeKind, WineRuntime};

pub const BUILDS_URL: &str = "https://github.com/Gcenx/macOS_Wine_builds/releases";
const RELEASES_API: &str =
    "https://api.github.com/repos/Gcenx/macOS_Wine_builds/releases?per_page=30";

/// WineWrap's own build, made by `scripts/build-wine-macos.sh` and published
/// as a release of this repository (with its source, as the LGPL requires).
const WINEWRAP_BUILD_VERSION: &str = "26.3.0";
const WINEWRAP_BUILD_SIZE: u64 = 212_697_632;
const WINEWRAP_RELEASES: &str = "https://github.com/Hashspace-apps/wine-wrap/releases/download";

/// Which Wine release line to install.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum WineFlavor {
    /// The stable release; the safest choice.
    #[default]
    Stable,
    /// The development release: newer, with more recent game fixes.
    Devel,
    /// Development release plus the wine-staging patch set.
    Staging,
    /// WineWrap's own build: CodeWeavers' LGPL CrossOver source plus
    /// WineWrap's patches. Needed for Steam, whose window stays black with
    /// the other builds.
    WineWrap,
}

impl WineFlavor {
    pub const ALL: &'static [WineFlavor] =
        &[Self::Stable, Self::Devel, Self::Staging, Self::WineWrap];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Devel => "devel",
            Self::Staging => "staging",
            Self::WineWrap => "winewrap",
        }
    }
}

impl fmt::Display for WineFlavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WineFlavor {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|f| f.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| "expected one of: stable, devel, staging, winewrap".to_string())
    }
}

/// A downloadable Wine build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineBuild {
    pub flavor: WineFlavor,
    pub version: String,
    pub asset_name: String,
    pub url: String,
    /// Download size in bytes.
    pub size: u64,
}

impl WineBuild {
    /// Name of the runtime once installed, e.g. `wine-stable-10.0`.
    pub fn runtime_name(&self) -> String {
        format!("wine-{}-{}", self.flavor, self.version)
    }
}

/// Progress of [`install`], for progress bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallProgress {
    Downloading { done: u64, total: Option<u64> },
    Extracting,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Pick the newest `flavor` build from a GitHub "list releases" response
/// (which is ordered newest first). Assets are named like
/// `wine-stable-10.0-osx64.tar.xz`.
pub fn pick_build(releases_json: &str, flavor: WineFlavor) -> Result<WineBuild> {
    let releases: Vec<Release> =
        serde_json::from_str(releases_json).map_err(|e| Error::Download {
            url: RELEASES_API.into(),
            message: format!("unexpected response from GitHub: {e}"),
        })?;
    let prefix = format!("wine-{flavor}-");
    releases
        .iter()
        .filter(|r| !r.draft)
        .flat_map(|r| &r.assets)
        .find_map(|a| {
            let rest = a.name.strip_prefix(&prefix)?;
            let version = rest
                .strip_suffix("-osx64.tar.xz")
                .or_else(|| rest.strip_suffix(".tar.xz"))?;
            Some(WineBuild {
                flavor,
                version: version.to_string(),
                asset_name: a.name.clone(),
                url: a.browser_download_url.clone(),
                size: a.size,
            })
        })
        .ok_or_else(|| Error::Download {
            url: BUILDS_URL.into(),
            message: format!("no wine-{flavor} build found in the recent releases"),
        })
}

/// WineWrap's own build, the one Steam bottles use.
pub fn winewrap_build() -> WineBuild {
    let version = WINEWRAP_BUILD_VERSION;
    let asset_name = format!("wine-winewrap-{version}-osx64.tar.xz");
    WineBuild {
        flavor: WineFlavor::WineWrap,
        version: version.to_string(),
        url: format!("{WINEWRAP_RELEASES}/wine-winewrap-{version}/{asset_name}"),
        asset_name,
        size: WINEWRAP_BUILD_SIZE,
    }
}

/// The newest build of `flavor`. Asks GitHub, except for WineWrap's own
/// build, which this version of WineWrap pins.
pub fn latest_build(flavor: WineFlavor) -> Result<WineBuild> {
    if flavor == WineFlavor::WineWrap {
        return Ok(winewrap_build());
    }
    let fail = |message: String| Error::Download {
        url: RELEASES_API.into(),
        message,
    };
    let mut request = download::agent()
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json");
    // Anonymous API calls are rate-limited per IP; CI runners share IPs.
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
    }
    let body = request
        .call()
        .map_err(|e| fail(e.to_string()))?
        .body_mut()
        .read_to_string()
        .map_err(|e| fail(e.to_string()))?;
    pick_build(&body, flavor)
}

/// Unpack a `.tar.xz` archive into `dest`.
pub fn extract_tar_xz(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive).at(archive)?;
    let xz = lzma_rust2::XzReader::new(BufReader::new(file), true);
    let mut tar = tar::Archive::new(xz);
    tar.set_preserve_permissions(true);
    // `unpack` refuses entries that would escape `dest` (absolute paths, `..`).
    tar.unpack(dest).at(dest)
}

/// Download (if not cached) and install `build` into the managed runtimes
/// directory, replacing an existing install of the same version.
pub fn install(
    paths: &AppPaths,
    build: &WineBuild,
    mut progress: impl FnMut(InstallProgress),
) -> Result<WineRuntime> {
    let archive = paths.downloads_dir().join(&build.asset_name);
    if !archive.is_file() {
        download::download(&build.url, &archive, |done, total| {
            progress(InstallProgress::Downloading { done, total })
        })?;
    }

    progress(InstallProgress::Extracting);
    let runtimes = paths.runtimes_dir();
    let name = build.runtime_name();
    // Unpack next to the final location (same file system, so the final
    // rename is atomic) under a hidden name that discovery ignores.
    let staging = runtimes.join(format!(".installing-{name}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).at(&staging)?;
    }
    std::fs::create_dir_all(&staging).at(&staging)?;
    let result = extract_tar_xz(&archive, &staging).and_then(|()| {
        let root = find_install(&staging).ok_or_else(|| Error::NotAWineInstall(staging.clone()))?;
        let dest = runtimes.join(&name);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).at(&dest)?;
        }
        std::fs::rename(&root, &dest).at(&dest)?;
        Ok(dest)
    });
    let _ = std::fs::remove_dir_all(&staging);
    let dest = match result {
        Ok(dest) => dest,
        Err(e) => {
            // A corrupt download would fail the same way every time.
            let _ = std::fs::remove_file(&archive);
            return Err(e);
        }
    };
    // The archive is only needed again for a reinstall; don't keep ~200 MB around.
    let _ = std::fs::remove_file(&archive);
    WineRuntime::from_root(name, dest, RuntimeKind::Managed)
}

/// The Wine install inside an unpacked archive: normally a `Wine *.app`
/// bundle at the top level, or the directory itself if it is a Wine prefix.
fn find_install(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    entries
        .into_iter()
        .find(|p| resolve_root(p).is_some())
        .or_else(|| resolve_root(dir).map(|_| dir.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASES: &str = r#"[
      {"tag_name": "11.0-rc1", "draft": true, "assets": [
        {"name": "wine-stable-11.0-osx64.tar.xz", "browser_download_url": "https://example/draft", "size": 1}
      ]},
      {"tag_name": "10.18", "draft": false, "assets": [
        {"name": "wine-devel-10.18-osx64.tar.xz", "browser_download_url": "https://example/devel", "size": 200},
        {"name": "wine-staging-10.18-osx64.tar.xz", "browser_download_url": "https://example/staging", "size": 210}
      ]},
      {"tag_name": "10.0", "draft": false, "assets": [
        {"name": "wine-stable-10.0-osx64.tar.xz", "browser_download_url": "https://example/stable", "size": 190},
        {"name": "wine-stable-10.0-osx64.tar.xz.sha256", "browser_download_url": "https://example/sum", "size": 64}
      ]}
    ]"#;

    #[test]
    fn picks_newest_matching_asset() {
        let stable = pick_build(RELEASES, WineFlavor::Stable).unwrap();
        assert_eq!(stable.version, "10.0");
        assert_eq!(stable.url, "https://example/stable");
        assert_eq!(stable.runtime_name(), "wine-stable-10.0");

        let devel = pick_build(RELEASES, WineFlavor::Devel).unwrap();
        assert_eq!(devel.version, "10.18");
        assert_eq!(devel.size, 200);

        assert!(pick_build("[]", WineFlavor::Staging).is_err());
        assert!(pick_build("{\"message\": \"rate limited\"}", WineFlavor::Stable).is_err());
    }

    #[test]
    fn flavors_parse() {
        assert_eq!("Staging".parse::<WineFlavor>(), Ok(WineFlavor::Staging));
        assert_eq!("winewrap".parse::<WineFlavor>(), Ok(WineFlavor::WineWrap));
        assert!("beta".parse::<WineFlavor>().is_err());
    }

    #[test]
    fn winewrap_build_is_pinned() {
        let build = latest_build(WineFlavor::WineWrap).unwrap();
        assert_eq!(build, winewrap_build());
        assert_eq!(build.runtime_name(), "wine-winewrap-26.3.0");
        assert_eq!(
            build.url,
            "https://github.com/Hashspace-apps/wine-wrap/releases/download/\
             wine-winewrap-26.3.0/wine-winewrap-26.3.0-osx64.tar.xz"
        );
    }

    /// Build a `.tar.xz` shaped like Gcenx's: `Wine Stable.app/Contents/Resources/wine/bin/…`.
    fn fake_archive(path: &Path) {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, mode) in [
            ("Wine Stable.app/Contents/Resources/wine/bin/wine", 0o755),
            (
                "Wine Stable.app/Contents/Resources/wine/bin/wineserver",
                0o755,
            ),
            ("Wine Stable.app/Contents/Info.plist", 0o644),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(mode);
            header.set_cksum();
            tar.append_data(&mut header, name, std::io::empty())
                .unwrap();
        }
        let data = tar.into_inner().unwrap();
        let mut xz = lzma_rust2::XzWriter::new(
            File::create(path).unwrap(),
            lzma_rust2::XzOptions::with_preset(1),
        )
        .unwrap();
        std::io::Write::write_all(&mut xz, &data).unwrap();
        xz.finish().unwrap();
    }

    #[test]
    fn installs_from_cached_archive() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted(dir.path());
        paths.ensure().unwrap();
        let build = WineBuild {
            flavor: WineFlavor::Stable,
            version: "10.0".into(),
            asset_name: "wine-stable-10.0-osx64.tar.xz".into(),
            // Never fetched: the archive is already in the download cache.
            url: "https://invalid.example/never".into(),
            size: 0,
        };
        fake_archive(&paths.downloads_dir().join(&build.asset_name));

        let mut stages = Vec::new();
        let rt = install(&paths, &build, |p| stages.push(p)).unwrap();
        assert_eq!(stages, [InstallProgress::Extracting]);
        assert_eq!(rt.name, "wine-stable-10.0");
        assert!(rt.wine().is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(rt.wine()).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "wine must stay executable");
        }
        // Discovered as a managed runtime; no leftovers.
        let found = crate::runtime::discover(&[], &paths.runtimes_dir());
        assert!(found
            .iter()
            .any(|r| r.name == "wine-stable-10.0" && r.kind == RuntimeKind::Managed));
        let leftovers: Vec<_> = std::fs::read_dir(paths.runtimes_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(leftovers, ["wine-stable-10.0"]);
        assert!(!paths.downloads_dir().join(&build.asset_name).exists());
    }

    #[test]
    fn corrupt_archive_is_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted(dir.path());
        paths.ensure().unwrap();
        let build = WineBuild {
            flavor: WineFlavor::Stable,
            version: "10.0".into(),
            asset_name: "wine-stable-10.0-osx64.tar.xz".into(),
            url: "https://invalid.example/never".into(),
            size: 0,
        };
        let archive = paths.downloads_dir().join(&build.asset_name);
        std::fs::write(&archive, b"not an xz file").unwrap();
        assert!(install(&paths, &build, |_| {}).is_err());
        assert!(!archive.exists());
        assert_eq!(std::fs::read_dir(paths.runtimes_dir()).unwrap().count(), 0);
    }
}
