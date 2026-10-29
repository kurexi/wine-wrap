# WineWrap

A CrossOver-style front end for [Wine](https://www.winehq.org/), written in Rust,
for running Steam and Windows games on macOS.

WineWrap does not ship its own Wine. It finds the Wine builds you already have
(Apple's Game Porting Toolkit, Gcenx's `Wine *.app` bundles, Homebrew, Whisky's
Wine) and manages **bottles** for them. A bottle is an isolated Windows environment
(a `WINEPREFIX`) with its own settings. On top of that it can install Steam and
launch your games.

| Feature | |
|---|---|
| Runtime discovery | GPTK, `Wine Crossover/Staging/Devel/Stable.app`, Homebrew, Whisky, custom paths |
| Bottles | create, configure, delete; Windows version, Retina mode |
| Graphics | D3DMetal (GPTK), DXVK (MoltenVK), WineD3D |
| Performance | MSync / ESync, Rosetta AVX advertising, Metal HUD |
| Steam | one-click install, launch, installed-games list, play by app ID |
| Front ends | desktop app (`WineWrap`) and CLI (`winewrap`) |

## Requirements

* macOS 13+ (Apple Silicon or Intel). Linux works for development and with a system Wine.
* **Rosetta 2** on Apple Silicon: `softwareupdate --install-rosetta --agree-to-license`
* A Wine runtime, for example:

  ```bash
  brew install --cask --no-quarantine gcenx/wine/wine-crossover
  ```

  For DirectX 12 games, use a build that includes Apple's D3DMetal, such as the
  [Game Porting Toolkit](https://developer.apple.com/games/game-porting-toolkit/).

## Building

```bash
cargo build --release
```

This produces `target/release/winewrap` (CLI) and `target/release/WineWrap` (GUI).
To build a double-clickable `WineWrap.app` in a `.dmg`, plus a CLI tarball,
run this on a Mac. It writes to `dist/`:

```bash
scripts/package-macos.sh            # universal (Apple Silicon + Intel)
```

```bash
scripts/package-macos.sh native     # current architecture only, faster
```

Prebuilt downloads are attached to each
[GitHub release](https://github.com/hashspace-apps/wine-wrap/releases). Every CI
run also uploads a `winewrap-macos-unsigned` artifact (Actions → CI → a run → Artifacts).
To publish a build as the rolling `dev-unsigned` pre-release instead, run the CI
workflow manually (Actions → CI → Run workflow) with **publish_prerelease** ticked.

### Running an unsigned build

Builds without an Apple Developer ID are *ad-hoc signed*. They run normally, but
macOS Gatekeeper blocks apps downloaded from the internet that Apple has not
notarized. On macOS 15 and later, right-click → Open no longer bypasses this.
After downloading:

1. Unzip the artifact if it came from a CI run, then open `WineWrap-*.dmg` and
   drag **WineWrap** to **Applications**.
2. Remove the quarantine flag once:

   ```bash
   xattr -dr com.apple.quarantine /Applications/WineWrap.app
   ```

   Alternatively, open the app once, then go to **System Settings → Privacy &
   Security** and click **Open Anyway**.
3. Optional: put the bundled CLI on your `PATH`:

   ```bash
   sudo ln -sf /Applications/WineWrap.app/Contents/MacOS/winewrap /usr/local/bin/winewrap
   ```

For the standalone CLI tarball, run `xattr -d com.apple.quarantine ./winewrap` after extracting it.

## Quick start (CLI)

```bash
winewrap doctor                # check Rosetta, Wine runtimes, …
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

## Where things live

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
```

## Troubleshooting

* **Steam window is black or blank**: set `winewrap bottle set Steam steam_args "-cef-disable-gpu"` and relaunch.
* **A game says your CPU lacks AVX**: on macOS 15 or later, enable `advertise_avx`.
* **Nothing happens on launch**: check the newest file in the logs directory, or
  run the command printed by `--dry-run` yourself with `WINEDEBUG=+err` set.

## Project layout

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

## CI/CD

GitHub Actions workflows live in `.github/workflows/`:

| Workflow | Runs on | Does |
|---|---|---|
| `ci.yml` | every push to `master`, every pull request, manual runs | `cargo fmt --check`; clippy on macOS and Linux; tests on macOS arm64, macOS x86_64 and Linux; a minimum Rust version check (1.85 for core/CLI, 1.95 for the GUI); `cargo deny` (licenses, RustSec advisories, crate sources); then builds the universal `.dmg`, uploads it as the `winewrap-macos-unsigned` artifact, and launches the app on a Mac runner as a smoke test |
| `release.yml` | pushing a `v*.*.*` tag | checks that the tag matches `Cargo.toml`, runs CI, builds the macOS `.dmg`, the macOS CLI tarball and a Linux CLI tarball, and publishes a GitHub release with `SHA256SUMS.txt` |

Dependabot (`.github/dependabot.yml`) opens weekly update PRs for crates and actions.

### Cutting a release

1. Bump `version` in the root `Cargo.toml` and run `cargo check` so `Cargo.lock` updates.
2. Commit, then tag and push:

```bash
git tag v0.2.0
```

```bash
git push origin v0.2.0
```

Tags with a suffix such as `v0.2.0-beta.1` are published as pre-releases.

### Code signing (optional)

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
