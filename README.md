<p align="center">
  <img src="packaging/icon/winewrap.png" width="160" alt="WineWrap icon">
</p>

<h1 align="center">WineWrap</h1>

<p align="center">
  <b>Play your Windows Steam games on a Mac.</b><br>
  Free and open source. No Windows, no Boot Camp, no CrossOver license.
</p>

<p align="center">
  <a href="https://github.com/hashspace-apps/wine-wrap/releases"><b>⬇ Download for macOS</b></a>
</p>

![Red Dead Redemption running on a Mac through WineWrap](screenshots/red-dead-redemption.png)

<p align="center"><i>Red Dead Redemption on an M4 Mac at 60 FPS, with an Xbox controller.</i></p>

> [!NOTE]
> This project is vibe coded 🤖. It's young: expect rough edges, and please
> report games that work or don't.

## What is it?

WineWrap is a Mac app that runs the Windows version of Steam, so you can
install and play games that only exist for Windows. It sets everything up for
you: no Terminal or Homebrew needed.

![The WineWrap app: a Steam bottle with Grand Theft Auto IV and Red Dead Redemption, each with a Play button](screenshots/winewrap-app.png)

* **One click to Steam.** Click **Install Steam**, sign in, download your games.
* **A Play button for every game.** Your installed Steam games show up in the
  app; start them from there or from Steam.
* **DirectX 12 games** work with Apple's D3DMetal (from Apple's free Game
  Porting Toolkit; see [DirectX 12 games](#directx-12-games)).
* **Controllers** work, e.g. an Xbox Wireless Controller.
* **Big libraries on an external SSD.** Add a folder and Steam can install
  games there.
* **FPS counter.** Turn on the Metal performance HUD in a bottle's settings.
* **Bottles** keep things separate: each one is its own little Windows PC, so
  a broken game can't mess up the others.

Under the hood it uses [Wine](https://www.winehq.org/), the same open-source
technology inside CrossOver, plus fixes of its own so Steam's window and
Rockstar games work.

## Get started

You need a Mac with **macOS 13 or later**. Apple Silicon (M1–M4) and Intel both work.

1. **Download** `WineWrap-*.dmg` from the
   [latest release](https://github.com/hashspace-apps/wine-wrap/releases),
   open it and drag **WineWrap** to **Applications**.
2. **Let it open.** WineWrap isn't signed by Apple yet, so macOS blocks it the
   first time. Open it once, then go to **System Settings → Privacy &
   Security** and click **Open Anyway**. (Or run
   `xattr -dr com.apple.quarantine /Applications/WineWrap.app` in Terminal.)
3. **Install Steam.** Click **⬇ Install Steam**. WineWrap downloads Wine and
   Steam for you; on Apple Silicon it also offers to install Rosetta 2 if it's
   missing.
4. **Play.** Click **▶ Launch Steam**, sign in, install a game, and hit
   **▶ Play**.

If Steam can't find your other computers for LAN game transfers, check that
WineWrap and Wine are allowed under **System Settings → Privacy & Security →
Local Network**.

## Games that work

Tested on an M4 Mac.

| Game | Works? | Notes |
| --- | --- | --- |
| Red Dead Redemption | ✅ Yes | 60 FPS. Needs D3DMetal. Starts through the Rockstar Games Launcher; Social Club sign-in and Xbox controller work. |
| Phantom Blade Zero | ❌ No | Its Denuvo DRM crashes before the game shows anything (it fails in CrossOver too). |

Tried another game? Open an issue and tell us how it went.

## DirectX 12 games

Newer games (Unreal Engine 5 and the like) need **D3DMetal**, Apple's
DirectX 12 translator. Apple doesn't allow anyone to share it, so you download
it yourself, once:

1. Download the
   [Game Porting Toolkit](https://developer.apple.com/games/game-porting-toolkit/)
   (free with an Apple ID).
2. Open the `.dmg`, then open the **Evaluation environment for Windows games**
   disk image inside it.
3. In WineWrap, click **Add D3DMetal…** in the bottle and pick that disk.
   You can eject the disk afterwards.

## Troubleshooting

* **The game says your CPU lacks AVX**: open the bottle's **Settings** and turn
  on **Advertise AVX to games** (macOS 15 or later).
* **The Steam window is black**: the bottle isn't using WineWrap's own Wine
  build. Installing or launching Steam from the app switches it automatically.
* **Nothing happens when I click Play**: open **Activity** at the bottom of
  the window, or check the newest file in
  `~/Library/Application Support/com.wine-wrap.WineWrap/logs/`.

---

# For power users and developers

Everything below is the technical side: the command-line tool, where files
live, building from source and how releases are made.

### Requirements

* macOS 13+ (Apple Silicon or Intel). Linux works for development and with a system Wine.
* **Rosetta 2** on Apple Silicon. The app offers to install it if it's missing.
* **Wine.** If none is found, click **Install Wine** in the app, or run
  `winewrap runtime install`. This downloads a prebuilt macOS Wine from
  [Gcenx/macOS_Wine_builds](https://github.com/Gcenx/macOS_Wine_builds) (the same
  builds Homebrew's `wine-stable` cask uses) into WineWrap's own folder.

  For DirectX 12 games, you need Apple's D3DMetal (DirectX 12 → Metal). Its
  license forbids redistribution, so WineWrap doesn't bundle it: download the
  [Game Porting Toolkit](https://developer.apple.com/games/game-porting-toolkit/)
  yourself and graft D3DMetal into a runtime (see
  [DirectX 12 with D3DMetal](#directx-12-with-d3dmetal)).

### Building

```bash
cargo build --release
```

This produces `target/release/winewrap` (CLI) and `target/release/winewrap-gui` (GUI).
To build a double-clickable `WineWrap.app` in a `.dmg`, plus a CLI tarball,
run this on a Mac. It writes to `dist/`:

```bash
scripts/package-macos.sh            # universal (Apple Silicon + Intel)
```

```bash
scripts/package-macos.sh native     # current architecture only, faster
```

Prebuilt downloads are attached to each
[GitHub release](https://github.com/hashspace-apps/wine-wrap/releases). The latest
build of `master` is always available as a `dev-<run>` pre-release on the same
page.
To publish one from another branch, run the CI workflow manually (Actions → CI →
Run workflow). Builds are shared as release files, not Actions artifacts, so they
don't use the account's artifact storage.

#### Running an unsigned build

Builds without an Apple Developer ID are *ad-hoc signed*. They run normally, but
macOS Gatekeeper blocks apps downloaded from the internet that Apple has not
notarized. On macOS 15 and later, right-click → Open no longer bypasses this.
After downloading:

1. Download `WineWrap-*.dmg` from the release, open it and
   drag **WineWrap** to **Applications**.
2. Remove the quarantine flag once:

   ```bash
   xattr -dr com.apple.quarantine /Applications/WineWrap.app
   ```

   Alternatively, open the app once, then go to **System Settings → Privacy &
   Security** and click **Open Anyway**.
3. Optional: put the bundled CLI on your `PATH`:

   ```bash
   sudo ln -sf /Applications/WineWrap.app/Contents/Helpers/winewrap /usr/local/bin/winewrap
   ```

For the standalone CLI tarball, run `xattr -d com.apple.quarantine ./winewrap` after extracting it.

### Quick start (CLI)

```bash
winewrap doctor                # check Rosetta, Wine runtimes, …
winewrap runtime install       # download Wine (stable; or: devel, staging)
winewrap steam install         # creates a "Steam" bottle and installs Steam
winewrap steam launch          # sign in and install games
winewrap steam games           # list installed games with their app IDs
winewrap steam play 620        # launch a game by app ID
```

Other commands:

```bash
winewrap runtime list
winewrap runtime add gptk /usr/local/opt/game-porting-toolkit
winewrap runtime default gptk

winewrap bottle create Games --graphics dxvk --windows win10 --retina
winewrap bottle set Games metal_hud true
winewrap bottle set Games env.DXVK_HUD fps
winewrap bottle install-dxvk Games ~/Downloads/dxvk-macOS-async-v1.10.3
winewrap bottle show Games
winewrap run Games ~/Downloads/setup.exe
winewrap winecfg Games
winewrap kill Games
```

Add `--dry-run` to any command to print the Wine command lines it would run
instead of running them. Add `-v` or `-vv` for more logging.

### Where things live

| What | macOS location |
|---|---|
| Settings | `~/Library/Application Support/com.wine-wrap.WineWrap/config.toml` |
| Bottles | `~/Library/Application Support/com.wine-wrap.WineWrap/bottles/<name>/` |
| Managed runtimes | `~/Library/Application Support/com.wine-wrap.WineWrap/runtimes/<name>/` |
| Logs | `~/Library/Application Support/com.wine-wrap.WineWrap/logs/` |
| Downloads | `~/Library/Caches/com.wine-wrap.WineWrap/downloads/` |

Set `WINEWRAP_HOME=/some/dir` to keep everything under one directory, for
example on an external drive.

Each bottle is a normal Wine prefix with a `bottle.toml` next to `drive_c`:

```toml
name = "Steam"
windows_version = "win10"
graphics = "d3dmetal"   # d3dmetal | dxvk | wined3d
sync = "msync"          # msync | esync | none
retina = false
metal_hud = false
advertise_avx = false
dxvk_async = false
steam_args = []

[env]
# Extra environment variables for every process in the bottle.

[drives]
# Game folders outside the bottle, as Windows drive letters (D: to Y:):
# d = "/Volumes/SSD/SteamLibrary"
```

#### Installing games on another disk

In the app, open the bottle's settings, click **Game folders → Add folder…**,
pick a folder (e.g. `/Volumes/SSD/SteamLibrary`) and Save. Or:

```bash
winewrap bottle set Steam drive.d /Volumes/SSD/SteamLibrary
```

Windows programs see the folder as a drive, and in a Steam bottle it becomes a
Steam library the next time Steam starts, so it shows up in Steam's install
dialog. The disk should be formatted APFS or Mac OS Extended: macOS can't
write to NTFS, and exFAT works but is slower. While the disk is unplugged,
Steam shows the games on it as not installed.

### DirectX 12 with D3DMetal

Modern DirectX 12 games (Unreal Engine 5 and similar) need Apple's D3DMetal to
translate Direct3D 12 to Metal. WineWrap's own Wine build ships vkd3d over
MoltenVK, which only reaches Direct3D feature level 11_0 — enough to create a
device, but not to run feature-level-12 games. D3DMetal reaches feature level
12.

Apple's license doesn't allow redistributing D3DMetal, so you supply it. In the
app, a bottle that uses D3DMetal shows a **D3DMetal is not added yet** box until
its runtime has it: mount the Game Porting Toolkit image as in step 1 below,
click **Add D3DMetal…** and choose the mounted disk. The bottle's settings offer
to remove it again. From the command line:

1. Download the
   [Game Porting Toolkit](https://developer.apple.com/games/game-porting-toolkit/)
   (free with an Apple ID). Open the `.dmg`, then open the
   **Evaluation environment for Windows games** image inside it to mount it. You
   don't have to install anything else — the graft copies what it needs.
2. Graft D3DMetal into the runtime a bottle uses (for Steam bottles that's the
   managed `wine-winewrap-*` build), pointing at the mounted volume:

   ```bash
   winewrap runtime install-d3dmetal wine-winewrap-26.3.0-4 "/Volumes/Evaluation environment for Windows games 4.0 beta 3"
   ```

   You can eject the images afterwards: everything D3DMetal needs is copied into
   the build.

3. Switch the bottle's graphics backend to D3DMetal:

   ```bash
   winewrap bottle set Steam graphics d3dmetal
   ```

This copies `D3DMetal.framework`, `libd3dshared.dylib` and the D3DMetal
`d3d12`/`dxgi` DLLs into the build, next to (not over) Wine's own. Games get
D3DMetal; a few programs it can't serve, such as the Rockstar Games Launcher,
keep Wine's Direct3D. To undo the graft, run
`winewrap runtime remove-d3dmetal <runtime>`.

### More troubleshooting

* **Steam window is black**: a known limitation of upstream Wine on macOS
  (tested with 11.0 and 11.20). Steam's UI renders, but Chromium draws it from a
  helper process into a window another process owns, and Wine's macOS driver
  doesn't show that. Steam arguments such as `-cef-disable-gpu` don't help.
  WineWrap's own Wine build fixes it, and Steam bottles switch to it
  automatically (downloading it the first time); see
  [Building Wine for Steam](#building-wine-for-steam).
* **A game says your CPU lacks AVX**: on macOS 15 or later, enable `advertise_avx`.
* **Nothing happens on launch**: check the newest file in the logs directory, or
  run the command printed by `--dry-run` yourself with `WINEDEBUG=+err` set.

### Building Wine for Steam

`scripts/build-wine-macos.sh` builds an x86_64 macOS Wine from CodeWeavers'
LGPL [CrossOver source](https://www.codeweavers.com/crossover/source) (Wine 11.0
base) plus the patches in `scripts/wine-patches/`, which make Steam's UI show.
Wine's Windows-side DLLs are compiled with a MinGW GCC toolchain that the script
also builds: with clang (llvm-mingw), Steam never connects to its servers.
It needs Xcode (and Rosetta on Apple Silicon), but no Homebrew or MacPorts:
every dependency is downloaded with a pinned checksum and built into the work
directory. A first build takes about an hour; the toolchain is reused after that.

Installing Steam, launching it, or playing a game moves a Steam bottle onto this
build, downloaded from this repository's releases (`wine-winewrap-<version>`).
Bottles pinned to GPTK or a custom Wine are left alone.

```bash
scripts/build-wine-macos.sh        # writes target/wine-build/wine-winewrap-*-osx64.tar.xz
```

To use a local build instead, extract it and register it as a runtime for your Steam bottle:

```bash
tar xJf target/wine-build/wine-winewrap-*-osx64.tar.xz -C ~/Applications
winewrap runtime add winewrap-cx ~/Applications/"Wine WineWrap.app"
winewrap bottle set Steam runtime winewrap-cx
```

### Project layout

```
crates/
  winewrap-core/   library: runtimes, bottles, launch environment, Steam, VDF parser
  winewrap-cli/    `winewrap` command-line tool (clap)
  winewrap-gui/    `WineWrap` desktop app (egui/eframe)
```

The core library expresses every operation as a `LaunchSpec` (program,
arguments, environment and working directory) before running it. That keeps
the logic unit-testable without Wine installed, and both front ends share it.

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

### CI/CD

GitHub Actions workflows live in `.github/workflows/`:

| Workflow | Runs on | Does |
|---|---|---|
| `ci.yml` | every push to `master`, every pull request, manual runs | `cargo fmt --check`; clippy on macOS and Linux; tests on macOS arm64, macOS x86_64 and Linux; a minimum Rust version check (1.88 for core/CLI, 1.95 for the GUI); `cargo deny` (licenses, RustSec advisories, crate sources); then builds the universal `.dmg`, launches it on a Mac runner as a smoke test, and (on `master` or manual runs) publishes it as a `dev-<run>` pre-release, deleting the previous one |
| `release.yml` | pushing a `v*.*.*` tag | checks that the tag matches `Cargo.toml`, runs CI, builds the macOS `.dmg`, the macOS CLI tarball and a Linux CLI tarball, uploads them to a draft GitHub release and publishes it with `SHA256SUMS.txt` |

Dependabot (`.github/dependabot.yml`) opens weekly update PRs for crates and actions.

#### Cutting a release

1. Bump `version` in the root `Cargo.toml` and run `cargo check` so `Cargo.lock` updates.
2. Commit, then tag and push:

```bash
git tag v0.2.0
```

```bash
git push origin v0.2.0
```

Tags with a suffix such as `v0.2.0-beta.1` are published as pre-releases.

If the Wine build changed (a new patch in `scripts/wine-patches/`, say), publish
it first, because the app downloads it from its own release:

1. Bump `VERSION` in `scripts/build-wine-macos.sh` and `WINEWRAP_BUILD_VERSION`
   in `crates/winewrap-core/src/wine_builds.rs`, and update
   `scripts/wine-release-notes.md`.
2. Once that's merged, tag it and push the tag. The Wine build workflow builds
   it on a macOS runner (a couple of hours) and publishes the build together with
   the CrossOver source and patches:

```bash
git tag wine-winewrap-26.3.0-4
```

```bash
git push origin wine-winewrap-26.3.0-4
```

#### Code signing (optional)

Without signing secrets, builds are ad-hoc signed (see
[Running an unsigned build](#running-an-unsigned-build)). To ship a signed and
notarized app that opens without warnings, add these repository secrets:

| Secret | Value |
|---|---|
| `MACOS_CERTIFICATE_P12` | base64 of your *Developer ID Application* certificate exported as `.p12` |
| `MACOS_CERTIFICATE_PASSWORD` | password of that `.p12` |
| `MACOS_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | for notarization; use an app-specific password |

## License

MIT. See [LICENSE](LICENSE).
