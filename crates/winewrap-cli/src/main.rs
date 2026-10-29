//! `winewrap`: manage Wine bottles and run Steam on macOS from the terminal.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use winewrap_core::system::{self, Status};
use winewrap_core::wine_builds::{self, InstallProgress, WineFlavor};
use winewrap_core::{
    steam, Bottle, BottleConfig, GraphicsBackend, LaunchSpec, SyncMode, WindowsVersion, WineWrap,
};

#[derive(Parser)]
#[command(
    name = "winewrap",
    version,
    about = "A CrossOver-style front end for Wine on macOS"
)]
struct Cli {
    /// Print the commands that would run instead of running them.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Increase log verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that this machine is ready to run Windows games.
    Doctor,
    /// Manage Wine runtimes.
    #[command(subcommand)]
    Runtime(RuntimeCmd),
    /// Manage bottles (isolated Windows environments).
    #[command(subcommand)]
    Bottle(BottleCmd),
    /// Install, launch and play games with Steam.
    #[command(subcommand)]
    Steam(SteamCmd),
    /// Run a Windows program inside a bottle.
    Run {
        bottle: String,
        /// Path to the .exe (host path), or a program in the bottle's PATH such as `regedit`.
        program: PathBuf,
        /// Arguments passed to the program.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        /// Wait for the program to exit instead of running it in the background.
        #[arg(long)]
        wait: bool,
    },
    /// Open Wine's configuration dialog for a bottle.
    Winecfg { bottle: String },
    /// Stop every Windows program running in a bottle.
    Kill { bottle: String },
}

#[derive(Subcommand)]
enum RuntimeCmd {
    /// List detected and registered Wine runtimes.
    List,
    /// Download and install Wine for you (no Homebrew needed).
    Install {
        /// Release line: stable, devel or staging.
        #[arg(default_value_t = WineFlavor::default())]
        flavor: WineFlavor,
    },
    /// Register a Wine installation (a folder containing bin/wine, or a Wine .app).
    Add { name: String, path: PathBuf },
    /// Unregister a runtime added with `add`.
    Remove { name: String },
    /// Use this runtime for bottles that do not pin one.
    Default { name: String },
}

#[derive(Subcommand)]
enum BottleCmd {
    /// List bottles.
    List,
    /// Create and initialise a new bottle.
    Create(CreateArgs),
    /// Show a bottle's settings.
    Show { name: String },
    /// Change a setting, e.g. `graphics dxvk` or `env.DXVK_HUD fps`.
    Set {
        name: String,
        key: String,
        /// May start with `-`, e.g. `steam_args "-cef-disable-gpu"`.
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Re-apply settings that live in the Wine registry (Windows version, Retina).
    Apply { name: String },
    /// Copy DXVK DLLs into a bottle from an extracted DXVK release.
    InstallDxvk { name: String, dxvk_dir: PathBuf },
    /// Delete a bottle and everything installed in it.
    Delete {
        name: String,
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Args)]
struct CreateArgs {
    name: String,
    /// Wine runtime to use (default: the global default / best detected).
    #[arg(long)]
    runtime: Option<String>,
    #[arg(long, default_value_t = WindowsVersion::default())]
    windows: WindowsVersion,
    #[arg(long, default_value_t = GraphicsBackend::default())]
    graphics: GraphicsBackend,
    #[arg(long, default_value_t = SyncMode::default())]
    sync: SyncMode,
    /// Render at native Retina resolution.
    #[arg(long)]
    retina: bool,
}

#[derive(Subcommand)]
enum SteamCmd {
    /// Download Steam and install it into a bottle (creating the bottle if needed).
    Install {
        #[arg(default_value = "Steam")]
        bottle: String,
    },
    /// Start the Steam client.
    Launch {
        #[arg(default_value = "Steam")]
        bottle: String,
    },
    /// List installed games.
    Games {
        #[arg(default_value = "Steam")]
        bottle: String,
    },
    /// Launch a game by Steam app ID.
    Play {
        app_id: u32,
        #[arg(long, default_value = "Steam")]
        bottle: String,
    },
}

fn main() {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .init();
}

/// Executes [`LaunchSpec`]s, or just prints them in dry-run mode.
struct Runner {
    dry_run: bool,
}

impl Runner {
    fn run(&self, spec: &LaunchSpec) -> Result<()> {
        if self.dry_run {
            println!("{spec}");
            return Ok(());
        }
        tracing::info!("running {spec}");
        spec.run()?;
        Ok(())
    }

    fn spawn(&self, ww: &WineWrap, spec: &LaunchSpec, label: &str) -> Result<()> {
        if self.dry_run {
            println!("{spec}");
            return Ok(());
        }
        tracing::info!("starting {spec}");
        let (child, log) = spec.spawn_logged(&ww.paths.logs_dir(), label)?;
        println!("Started (pid {}), log: {}", child.id(), log.display());
        Ok(())
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut ww = WineWrap::open().context("failed to open WineWrap data directory")?;
    let runner = Runner {
        dry_run: cli.dry_run,
    };

    match cli.command {
        Command::Doctor => doctor(&ww),
        Command::Runtime(cmd) => runtime(&mut ww, cmd),
        Command::Bottle(cmd) => bottle(&ww, &runner, cmd),
        Command::Steam(cmd) => steam_cmd(&ww, &runner, cmd),
        Command::Run {
            bottle,
            program,
            args,
            wait,
        } => {
            let (b, rt) = open_bottle(&ww, &bottle)?;
            let mut spec = LaunchSpec::wine(&b, &rt).arg(&program).args(args);
            if let Some(dir) = program.parent().filter(|d| d.is_dir()) {
                spec = spec.cwd(dir);
            }
            if wait {
                runner.run(&spec)
            } else {
                let label = program
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "run".into());
                runner.spawn(&ww, &spec, &format!("{bottle}-{label}"))
            }
        }
        Command::Winecfg { bottle } => {
            let (b, rt) = open_bottle(&ww, &bottle)?;
            runner.run(&LaunchSpec::wine(&b, &rt).arg("winecfg"))
        }
        Command::Kill { bottle } => {
            let (b, rt) = open_bottle(&ww, &bottle)?;
            runner.run(&b.kill_command(&rt))
        }
    }
}

fn open_bottle(ww: &WineWrap, name: &str) -> Result<(Bottle, winewrap_core::WineRuntime)> {
    let b = ww.bottle(name)?;
    let rt = ww
        .runtime_for(&b)
        .with_context(|| format!("no usable runtime for bottle {name:?}"))?;
    Ok((b, rt))
}

fn doctor(ww: &WineWrap) -> Result<()> {
    let mut failed = false;
    for check in ww.doctor() {
        let mark = match check.status {
            Status::Ok => "ok  ",
            Status::Warning => "warn",
            Status::Error => {
                failed = true;
                "FAIL"
            }
        };
        println!("[{mark}] {}: {}", check.name, check.detail);
    }
    if failed {
        bail!("some checks failed");
    }
    Ok(())
}

fn runtime(ww: &mut WineWrap, cmd: RuntimeCmd) -> Result<()> {
    match cmd {
        RuntimeCmd::List => {
            let runtimes = ww.runtimes();
            if runtimes.is_empty() {
                println!("No Wine runtimes found. See `winewrap doctor`.");
            }
            let default = ww.default_runtime().ok().map(|r| r.name);
            for rt in runtimes {
                let star = if Some(&rt.name) == default.as_ref() {
                    "*"
                } else {
                    " "
                };
                let version = rt.version().unwrap_or_else(|| "unknown version".into());
                let d3d = if rt.has_d3dmetal() { ", D3DMetal" } else { "" };
                println!(
                    "{star} {:<18} {version} ({}{d3d})\n    {}",
                    rt.name,
                    rt.kind,
                    rt.root.display()
                );
            }
        }
        RuntimeCmd::Install { flavor } => {
            println!("Looking up the latest Wine {flavor} build ...");
            let build = wine_builds::latest_build(flavor)?;
            println!(
                "Installing {} ({:.0} MiB download) ...",
                build.runtime_name(),
                build.size as f64 / 1_048_576.0
            );
            let rt = wine_builds::install(&ww.paths, &build, |p| match p {
                InstallProgress::Downloading { done, total } => progress_bar(done, total),
                InstallProgress::Extracting => eprintln!("\n  Unpacking ..."),
            })?;
            println!("Installed {:?} at {}", rt.name, rt.root.display());
            if system::needs_rosetta() {
                println!(
                    "Wine needs Rosetta 2 on Apple Silicon. Install it with:\n  \
                     softwareupdate --install-rosetta --agree-to-license"
                );
            }
        }
        RuntimeCmd::Add { name, path } => {
            let rt = ww.add_runtime(&name, &path)?;
            println!("Registered runtime {:?} at {}", rt.name, rt.root.display());
        }
        RuntimeCmd::Remove { name } => {
            ww.remove_runtime(&name)?;
            println!("Removed runtime {name:?}");
        }
        RuntimeCmd::Default { name } => {
            ww.set_default_runtime(&name)?;
            println!("Default runtime is now {name:?}");
        }
    }
    Ok(())
}

fn bottle(ww: &WineWrap, runner: &Runner, cmd: BottleCmd) -> Result<()> {
    match cmd {
        BottleCmd::List => {
            let bottles = ww.bottles()?;
            if bottles.is_empty() {
                println!("No bottles yet. Create one with `winewrap bottle create <name>` or `winewrap steam install`.");
            }
            for b in bottles {
                let steam = if steam::is_installed(&b) {
                    "  [Steam]"
                } else {
                    ""
                };
                println!(
                    "{:<20} {} / {}{steam}",
                    b.name(),
                    b.config.graphics,
                    b.config.windows_version
                );
            }
        }
        BottleCmd::Create(args) => {
            let mut cfg = BottleConfig::new(&args.name);
            cfg.runtime = args.runtime;
            cfg.windows_version = args.windows;
            cfg.graphics = args.graphics;
            cfg.sync = args.sync;
            cfg.retina = args.retina;
            create_bottle(ww, runner, cfg)?;
        }
        BottleCmd::Show { name } => {
            let b = ww.bottle(&name)?;
            println!("# {}", b.path.display());
            print!("{}", std::fs::read_to_string(b.config_path())?);
        }
        BottleCmd::Set { name, key, value } => {
            let mut b = ww.bottle(&name)?;
            b.config
                .set(&key, &value)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("invalid value for {key}"))?;
            if let Some(rt) = &b.config.runtime {
                ww.runtime(rt)?;
            }
            b.save()?;
            println!("{name}: {key} = {value}");
            if matches!(key.as_str(), "windows_version" | "retina") {
                println!("Run `winewrap bottle apply {name:?}` to update the Wine registry.");
            }
        }
        BottleCmd::Apply { name } => {
            let (b, rt) = open_bottle(ww, &name)?;
            for spec in b.apply_commands(&rt) {
                runner.run(&spec)?;
            }
        }
        BottleCmd::InstallDxvk { name, dxvk_dir } => {
            let mut b = ww.bottle(&name)?;
            let n = b.install_dxvk(&dxvk_dir)?;
            b.config.graphics = GraphicsBackend::Dxvk;
            b.save()?;
            println!("Copied {n} DXVK DLLs into {name:?} and switched it to the DXVK backend.");
        }
        BottleCmd::Delete { name, yes } => {
            let b = ww.bottle(&name)?;
            if !yes && !confirm(&format!("Delete bottle {name:?} at {}?", b.path.display()))? {
                println!("Cancelled.");
                return Ok(());
            }
            if runner.dry_run {
                println!("rm -rf {}", b.path.display());
            } else {
                ww.delete_bottle(&name)?;
                println!("Deleted {name:?}");
            }
        }
    }
    Ok(())
}

fn create_bottle(ww: &WineWrap, runner: &Runner, cfg: BottleConfig) -> Result<Bottle> {
    let name = cfg.name.clone();
    // Resolve the runtime before touching the disk so a missing Wine does not
    // leave an empty bottle behind.
    let rt = match &cfg.runtime {
        Some(r) => ww.runtime(r)?,
        None => ww.default_runtime()?,
    };
    if runner.dry_run {
        let b = Bottle {
            path: ww.bottle_path(&name),
            config: cfg,
        };
        for spec in b.init_commands(&rt) {
            runner.run(&spec)?;
        }
        return Ok(b);
    }
    let b = ww.create_bottle(cfg)?;
    println!("Creating bottle {name:?} with {} ...", rt.name);
    for spec in b.init_commands(&rt) {
        if let Err(e) = runner.run(&spec) {
            // Leave nothing half-made behind.
            let _ = ww.delete_bottle(&name);
            return Err(e).context("bottle initialisation failed");
        }
    }
    println!("Bottle {name:?} is ready at {}", b.path.display());
    Ok(b)
}

fn steam_cmd(ww: &WineWrap, runner: &Runner, cmd: SteamCmd) -> Result<()> {
    match cmd {
        SteamCmd::Install { bottle } => {
            let b = match ww.bottle(&bottle) {
                Ok(b) => b,
                Err(winewrap_core::Error::BottleNotFound(_)) => {
                    create_bottle(ww, runner, BottleConfig::new(&bottle))?
                }
                Err(e) => return Err(e.into()),
            };
            if steam::is_installed(&b) {
                println!("Steam is already installed in {bottle:?}.");
                return Ok(());
            }
            let rt = ww.runtime_for(&b)?;
            let setup = if runner.dry_run {
                ww.paths.downloads_dir().join(steam::STEAM_SETUP_FILE)
            } else {
                println!("Downloading Steam installer ...");
                let path = ww.steam_installer(progress_bar)?;
                eprintln!();
                path
            };
            runner.run(&steam::install_command(&b, &rt, &setup))?;
            // Let the prefix settle so all installed files are flushed.
            runner.run(&LaunchSpec::wineserver(&b, &rt).arg("--wait"))?;
            if !runner.dry_run {
                if !steam::is_installed(&b) {
                    bail!("the installer finished but steam.exe was not found");
                }
                println!("Steam installed. Start it with `winewrap steam launch {bottle:?}`.");
            }
        }
        SteamCmd::Launch { bottle } => {
            let (b, rt) = open_bottle(ww, &bottle)?;
            ensure_steam(&b)?;
            runner.spawn(
                ww,
                &steam::launch_command(&b, &rt),
                &format!("{bottle}-steam"),
            )?;
        }
        SteamCmd::Games { bottle } => {
            let b = ww.bottle(&bottle)?;
            ensure_steam(&b)?;
            let games = steam::installed_games(&b);
            if games.is_empty() {
                println!("No games installed yet. Install some from the Steam client.");
            }
            for g in games {
                let state = if g.fully_installed {
                    ""
                } else {
                    "  (update pending)"
                };
                println!("{:>9}  {}{state}", g.app_id, g.name);
            }
        }
        SteamCmd::Play { app_id, bottle } => {
            let (b, rt) = open_bottle(ww, &bottle)?;
            ensure_steam(&b)?;
            runner.spawn(
                ww,
                &steam::play_command(&b, &rt, app_id),
                &format!("{bottle}-{app_id}"),
            )?;
        }
    }
    Ok(())
}

fn ensure_steam(b: &Bottle) -> Result<()> {
    if !steam::is_installed(b) {
        return Err(winewrap_core::Error::SteamNotInstalled(
            b.name().to_string(),
        ))
        .context(format!("run `winewrap steam install {:?}` first", b.name()));
    }
    Ok(())
}

fn progress_bar(done: u64, total: Option<u64>) {
    let mb = |b: u64| b as f64 / 1_048_576.0;
    match total {
        Some(t) if t > 0 => eprint!(
            "\r  {:5.1} / {:.1} MiB ({:3}%)",
            mb(done),
            mb(t),
            done * 100 / t
        ),
        _ => eprint!("\r  {:5.1} MiB", mb(done)),
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_common_commands() {
        let cli = Cli::try_parse_from(["winewrap", "--dry-run", "steam", "play", "620"]).unwrap();
        assert!(cli.dry_run);
        assert!(matches!(
            cli.command,
            Command::Steam(SteamCmd::Play { app_id: 620, ref bottle }) if bottle == "Steam"
        ));

        let cli = Cli::try_parse_from([
            "winewrap",
            "bottle",
            "create",
            "Games",
            "--graphics",
            "dxvk",
            "--windows",
            "win11",
        ])
        .unwrap();
        let Command::Bottle(BottleCmd::Create(args)) = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(args.graphics, GraphicsBackend::Dxvk);
        assert_eq!(args.windows, WindowsVersion::Win11);

        assert!(
            Cli::try_parse_from(["winewrap", "bottle", "create", "x", "--graphics", "opengl"])
                .is_err()
        );

        let cli = Cli::try_parse_from([
            "winewrap",
            "bottle",
            "set",
            "Steam",
            "steam_args",
            "-cef-disable-gpu -cef-disable-gpu-compositing",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Bottle(BottleCmd::Set { ref value, .. }) if value.starts_with("-cef")
        ));
    }
}
