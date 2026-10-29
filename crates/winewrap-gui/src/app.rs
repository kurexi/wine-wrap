use std::path::{Path, PathBuf};

use eframe::egui;
use winewrap_core::steam::{self, SteamGame};
use winewrap_core::system;
use winewrap_core::wine_builds::{self, InstallProgress, WineFlavor};
use winewrap_core::{
    Bottle, BottleConfig, GraphicsBackend, LaunchSpec, SyncMode, WindowsVersion, WineRuntime,
    WineWrap,
};

use crate::jobs::{Event, Jobs, Reporter};

const MAX_LOG_LINES: usize = 200;

pub struct WineWrapApp {
    ww: Option<WineWrap>,
    bottles: Vec<Bottle>,
    runtimes: Vec<WineRuntime>,
    selected: Option<String>,
    /// Editable copy of the selected bottle's settings.
    draft: Option<BottleConfig>,
    games: Vec<SteamGame>,
    new_bottle: Option<NewBottle>,
    /// Bottle waiting for the user to confirm its deletion.
    delete_bottle: Option<String>,
    jobs: Jobs,
    progress: Option<Option<f32>>,
    log: Vec<String>,
    /// Release line offered by the "Install Wine" button.
    wine_flavor: WineFlavor,
    needs_rosetta: bool,
}

/// State of the "New bottle" dialog.
struct NewBottle {
    config: BottleConfig,
    install_steam: bool,
}

impl WineWrapApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            ww: None,
            bottles: Vec::new(),
            runtimes: Vec::new(),
            selected: None,
            draft: None,
            games: Vec::new(),
            new_bottle: None,
            delete_bottle: None,
            jobs: Jobs::new(),
            progress: None,
            log: Vec::new(),
            wine_flavor: WineFlavor::default(),
            needs_rosetta: false,
        };
        match WineWrap::open() {
            Ok(ww) => app.ww = Some(ww),
            Err(e) => app.push_log(format!("Failed to open data directory: {e}")),
        }
        app.reload();
        if app.runtimes.is_empty() {
            app.push_log("No Wine runtime found. Click “Install Wine” to download it.");
        }
        app.selected = app.bottles.first().map(|b| b.name().to_string());
        app.select_changed();
        app
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        tracing::info!("{line}");
        self.log.push(line);
        if self.log.len() > MAX_LOG_LINES {
            self.log.drain(..self.log.len() - MAX_LOG_LINES);
        }
    }

    /// Re-read bottles and runtimes from disk.
    fn reload(&mut self) {
        self.needs_rosetta = system::needs_rosetta();
        let Some(ww) = &self.ww else { return };
        self.runtimes = ww.runtimes();
        match ww.bottles() {
            Ok(b) => self.bottles = b,
            Err(e) => self.push_log(format!("Failed to list bottles: {e}")),
        }
        if let Some(sel) = &self.selected {
            if !self.bottles.iter().any(|b| b.name() == sel) {
                self.selected = None;
            }
        }
        self.select_changed();
    }

    fn select_changed(&mut self) {
        let bottle = self.selected_bottle().cloned();
        self.draft = bottle.as_ref().map(|b| b.config.clone());
        self.games = bottle
            .as_ref()
            .map(steam::installed_games)
            .unwrap_or_default();
    }

    fn selected_bottle(&self) -> Option<&Bottle> {
        let sel = self.selected.as_ref()?;
        self.bottles.iter().find(|b| b.name() == sel)
    }

    fn handle_events(&mut self) {
        for event in self.jobs.poll() {
            match event {
                Event::Log(line) => self.push_log(line),
                Event::Progress(p) => self.progress = Some(p),
                Event::Finished { label, error } => {
                    self.progress = None;
                    match error {
                        None => self.push_log(format!("✔ {label}")),
                        Some(e) => self.push_log(format!("✘ {label}: {e}")),
                    }
                    self.reload();
                }
            }
        }
    }

    // ---- actions --------------------------------------------------------

    /// Start a process in the background, logging its output to a file.
    fn spawn(&mut self, spec: LaunchSpec, label: &str) {
        let Some(ww) = &self.ww else { return };
        match spec.spawn_logged(&ww.paths.logs_dir(), label) {
            Ok((child, log)) => self.push_log(format!(
                "Started {label} (pid {}), log: {}",
                child.id(),
                log.display()
            )),
            Err(e) => self.push_log(format!("Failed to start {label}: {e}")),
        }
    }

    fn with_runtime(&mut self, bottle: &Bottle) -> Option<WineRuntime> {
        let ww = self.ww.as_ref()?;
        match ww.runtime_for(bottle) {
            Ok(rt) => Some(rt),
            Err(e) => {
                self.push_log(format!("{}: {e}", bottle.name()));
                None
            }
        }
    }

    fn create_bottle(&mut self, ctx: &egui::Context, nb: NewBottle) {
        let Some(ww) = self.ww.clone() else { return };
        let name = nb.config.name.clone();
        let install_steam = nb.install_steam;
        // A Steam bottle gets WineWrap's Wine in the job below (downloading
        // it if needed), so it doesn't need any Wine to be installed yet.
        let rt = if install_steam && needs_steam_runtime(&nb.config) {
            None
        } else {
            let rt = match &nb.config.runtime {
                Some(r) => ww.runtime(r),
                None => ww.default_runtime(),
            };
            match rt {
                Ok(rt) => Some(rt),
                Err(e) => return self.push_log(format!("Cannot create {name:?}: {e}")),
            }
        };
        let mut bottle = match ww.create_bottle(nb.config) {
            Ok(b) => b,
            Err(e) => return self.push_log(format!("Cannot create {name:?}: {e}")),
        };
        self.selected = Some(name.clone());
        self.reload();
        self.jobs
            .spawn(ctx, format!("Create bottle {name:?}"), move |r| {
                let result = (|| {
                    let rt = match rt {
                        Some(rt) => rt,
                        None => prepare_steam_runtime(&ww, &mut bottle, r)?,
                    };
                    r.log(format!("Initialising {name:?} with {} …", rt.name));
                    bottle
                        .init_commands(&rt)
                        .iter()
                        .try_for_each(LaunchSpec::run)?;
                    Ok(rt)
                })();
                let rt = match result {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ww.delete_bottle(&name);
                        return Err(e);
                    }
                };
                if install_steam {
                    install_steam_job(&ww, &bottle, &rt, r)?;
                }
                Ok(())
            });
    }

    fn install_steam(&mut self, ctx: &egui::Context, mut bottle: Bottle) {
        let Some(ww) = self.ww.clone() else { return };
        let rt = if needs_steam_runtime(&bottle.config) {
            None
        } else {
            match self.with_runtime(&bottle) {
                Some(rt) => Some(rt),
                None => return,
            }
        };
        self.jobs.spawn(
            ctx,
            format!("Install Steam in {:?}", bottle.name()),
            move |r| {
                let rt = match rt {
                    Some(rt) => rt,
                    None => prepare_steam_runtime(&ww, &mut bottle, r)?,
                };
                install_steam_job(&ww, &bottle, &rt, r)
            },
        );
    }

    /// Start Steam, and the game `app_id` if given. A Steam bottle still on
    /// another Wine moves to WineWrap's build first, which may mean a download.
    fn run_steam(&mut self, ctx: &egui::Context, bottle: &Bottle, app_id: Option<u32>) {
        let label = match app_id {
            Some(id) => format!("{}-{id}", bottle.name()),
            None => format!("{}-steam", bottle.name()),
        };
        let command = move |b: &Bottle, rt: &WineRuntime| match app_id {
            Some(id) => steam::play_command(b, rt, id),
            None => steam::launch_command(b, rt),
        };
        if !needs_steam_runtime(&bottle.config) {
            if let Some(rt) = self.with_runtime(bottle) {
                self.spawn(command(bottle, &rt), &label);
            }
            return;
        }
        let Some(ww) = self.ww.clone() else { return };
        let mut bottle = bottle.clone();
        self.jobs.spawn(
            ctx,
            format!("Start Steam in {:?}", bottle.name()),
            move |r| {
                let rt = prepare_steam_runtime(&ww, &mut bottle, r)?;
                let (child, log) =
                    command(&bottle, &rt).spawn_logged(&ww.paths.logs_dir(), &label)?;
                r.log(format!(
                    "Started {label} (pid {}), log: {}",
                    child.id(),
                    log.display()
                ));
                Ok(())
            },
        );
    }

    fn save_settings(&mut self, ctx: &egui::Context) {
        let (Some(bottle), Some(draft)) = (self.selected_bottle().cloned(), self.draft.clone())
        else {
            return;
        };
        let registry_changed = bottle.config.windows_version != draft.windows_version
            || bottle.config.retina != draft.retina;
        let bottle = Bottle {
            path: bottle.path,
            config: draft,
        };
        if let Err(e) = bottle.save() {
            return self.push_log(format!("Failed to save settings: {e}"));
        }
        self.push_log(format!("Saved settings for {:?}", bottle.name()));
        if registry_changed && bottle.is_initialized() {
            if let Some(rt) = self.with_runtime(&bottle) {
                self.jobs.spawn(
                    ctx,
                    format!("Apply settings to {:?}", bottle.name()),
                    move |_| {
                        bottle
                            .apply_commands(&rt)
                            .iter()
                            .try_for_each(LaunchSpec::run)
                    },
                );
            }
        }
        self.reload();
    }

    fn run_program(&mut self, bottle: &Bottle) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Run Windows program")
            .add_filter("Windows programs", &["exe", "msi", "bat"])
            .pick_file()
        else {
            return;
        };
        let Some(rt) = self.with_runtime(bottle) else {
            return;
        };
        let is_msi = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("msi"));
        let mut spec = LaunchSpec::wine(bottle, &rt);
        spec = if is_msi {
            spec.args(["msiexec", "/i"]).arg(&path)
        } else {
            spec.arg(&path)
        };
        if let Some(dir) = path.parent() {
            spec = spec.cwd(dir);
        }
        let label = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.spawn(spec, &format!("{}-{label}", bottle.name()));
    }

    fn handle_wine_actions(&mut self, ctx: &egui::Context, actions: WineActions) {
        if actions.install {
            self.install_wine(ctx);
        }
        if actions.install_rosetta {
            self.jobs.spawn(ctx, "Install Rosetta 2", |r| {
                r.log("Installing Rosetta 2 (macOS will ask for your password) …");
                system::rosetta_install_command().run()
            });
        }
        if actions.rescan {
            self.reload();
            let n = self.runtimes.len();
            self.push_log(format!("Rescanned: found {n} Wine runtime(s)"));
        }
        if actions.add {
            self.add_runtime_dialog();
        }
    }

    /// Download and install the latest Wine build of the chosen flavor.
    fn install_wine(&mut self, ctx: &egui::Context) {
        let Some(ww) = self.ww.clone() else { return };
        let flavor = self.wine_flavor;
        self.jobs
            .spawn(ctx, format!("Install Wine ({flavor})"), move |r| {
                r.log(format!("Looking up the latest Wine {flavor} build …"));
                let build = wine_builds::latest_build(flavor)?;
                r.log(format!(
                    "Downloading {} ({:.0} MB) …",
                    build.runtime_name(),
                    build.size as f64 / 1_000_000.0
                ));
                let rt = wine_builds::install(&ww.paths, &build, install_progress(r))?;
                r.log(format!("Wine is ready: {}", rt.name));
                Ok(())
            });
    }

    /// Let the user point at a Wine installation that was not auto-detected.
    fn add_runtime_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose a Wine installation (the folder that contains bin/wine)")
            .pick_folder()
        else {
            return;
        };
        let Some(ww) = self.ww.as_mut() else { return };
        match ww.add_runtime_from_path(&path) {
            Ok(rt) => {
                let msg = format!("Added Wine runtime {:?} ({})", rt.name, rt.root.display());
                self.push_log(msg);
                self.reload();
            }
            Err(e) => self.push_log(format!("Could not add {}: {e}", path.display())),
        }
    }

    // ---- UI -------------------------------------------------------------

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Bottles");
        ui.add_space(4.0);
        let mut clicked = None;
        let mut delete = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(ui.available_height() - 40.0)
            .show(ui, |ui| {
                for b in &self.bottles {
                    let selected = self.selected.as_deref() == Some(b.name());
                    let label = if steam::is_installed(b) {
                        format!("🎮 {}", b.name())
                    } else {
                        format!("🍷 {}", b.name())
                    };
                    let response = ui.selectable_label(selected, label);
                    if response.clicked() {
                        clicked = Some(b.name().to_string());
                    }
                    response.context_menu(|ui| {
                        if ui.button("🗑 Delete bottle…").clicked() {
                            delete = Some(b.name().to_string());
                            ui.close();
                        }
                    });
                }
                if self.bottles.is_empty() {
                    ui.weak("No bottles yet.");
                }
            });
        if let Some(name) = clicked {
            self.selected = Some(name);
            self.select_changed();
        }
        if delete.is_some() {
            self.delete_bottle = delete;
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("➕ New bottle").clicked() {
                self.new_bottle = Some(NewBottle {
                    config: BottleConfig::new("Steam"),
                    install_steam: true,
                });
            }
            if ui.button("⟳").on_hover_text("Reload").clicked() {
                self.reload();
            }
        });
    }

    fn bottle_view(&mut self, ui: &mut egui::Ui) {
        let Some(bottle) = self.selected_bottle().cloned() else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading("Welcome to WineWrap");
                ui.label("Create a bottle to install Steam and play your Windows games.");
                ui.add_space(8.0);
                if ui.button("Create a Steam bottle").clicked() {
                    self.new_bottle = Some(NewBottle {
                        config: BottleConfig::new("Steam"),
                        install_steam: true,
                    });
                }
            });
            return;
        };
        let ctx = ui.ctx().clone();
        let busy = self.jobs.is_busy();
        let has_steam = steam::is_installed(&bottle);

        ui.horizontal(|ui| {
            ui.heading(bottle.name());
            ui.weak(bottle.path.display().to_string());
        });
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            if has_steam {
                if ui.button("▶ Launch Steam").clicked() {
                    self.run_steam(&ctx, &bottle, None);
                }
            } else if ui
                .add_enabled(!busy, egui::Button::new("⬇ Install Steam"))
                .clicked()
            {
                self.install_steam(&ctx, bottle.clone());
            }
            if ui.button("Run program…").clicked() {
                self.run_program(&bottle);
            }
            if ui.button("Wine settings").clicked() {
                if let Some(rt) = self.with_runtime(&bottle) {
                    self.spawn(
                        LaunchSpec::wine(&bottle, &rt).arg("winecfg"),
                        &format!("{}-winecfg", bottle.name()),
                    );
                }
            }
            if ui.button("Open C: drive").clicked() {
                open_in_file_manager(&bottle.drive_c());
            }
            if ui
                .button("⏹ Stop all")
                .on_hover_text("Quit every program in this bottle")
                .clicked()
            {
                if let Some(rt) = self.with_runtime(&bottle) {
                    match bottle.kill(&rt) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.push_log(format!("Nothing is running in {}.", bottle.name()))
                        }
                        Err(e) => self.push_log(e.to_string()),
                    }
                }
            }
            if ui
                .button("🗑 Delete…")
                .on_hover_text("Delete this bottle and everything in it")
                .clicked()
            {
                self.delete_bottle = Some(bottle.name().to_string());
            }
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            if has_steam {
                egui::CollapsingHeader::new(format!("Games ({})", self.games.len()))
                    .default_open(true)
                    .show(ui, |ui| self.games(ui, &bottle));
            }
            egui::CollapsingHeader::new("Settings")
                .default_open(!has_steam)
                .show(ui, |ui| self.settings(ui, &bottle));
        });
    }

    fn games(&mut self, ui: &mut egui::Ui, bottle: &Bottle) {
        if self.games.is_empty() {
            ui.weak("No games installed yet. Launch Steam and install some.");
        }
        let mut play = None;
        egui::Grid::new("games")
            .striped(true)
            .num_columns(3)
            .show(ui, |ui| {
                for g in &self.games {
                    ui.label(&g.name);
                    if g.fully_installed {
                        ui.weak(g.app_id.to_string());
                    } else {
                        ui.weak("update pending");
                    }
                    if ui.button("▶ Play").clicked() {
                        play = Some(g.app_id);
                    }
                    ui.end_row();
                }
            });
        if ui.small_button("Refresh").clicked() {
            self.games = steam::installed_games(bottle);
        }
        if let Some(app_id) = play {
            let ctx = ui.ctx().clone();
            self.run_steam(&ctx, bottle, Some(app_id));
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui, bottle: &Bottle) {
        let runtimes = self.runtimes.clone();
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        settings_form(ui, draft, &runtimes);

        ui.horizontal(|ui| {
            ui.label("Steam arguments");
            let mut args = draft.steam_args.join(" ");
            if ui
                .text_edit_singleline(&mut args)
                .on_hover_text("Extra arguments for steam.exe, e.g. -silent")
                .changed()
            {
                draft.steam_args = args.split_whitespace().map(str::to_string).collect();
            }
        });

        ui.add_space(6.0);
        let changed = Some(&bottle.config) != self.draft.as_ref();
        ui.horizontal(|ui| {
            if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
                let ctx = ui.ctx().clone();
                self.save_settings(&ctx);
            }
            if ui
                .add_enabled(changed, egui::Button::new("Revert"))
                .clicked()
            {
                self.draft = Some(bottle.config.clone());
            }
            ui.add_space(24.0);
            if ui.button("🗑 Delete bottle…").clicked() {
                self.delete_bottle = Some(bottle.name().to_string());
            }
        });
    }

    /// Stop everything running in the bottle, then remove it from disk.
    fn delete_bottle_now(&mut self, name: &str) {
        let Some(bottle) = self.bottles.iter().find(|b| b.name() == name).cloned() else {
            return;
        };
        // Without a runtime nothing can be running in it, so just delete.
        let rt = self.ww.as_ref().and_then(|ww| ww.runtime_for(&bottle).ok());
        if let Some(rt) = rt {
            if let Err(e) = bottle.kill(&rt) {
                self.push_log(format!("Could not stop {name:?}: {e}"));
            }
        }
        let Some(ww) = &self.ww else { return };
        match ww.delete_bottle(name) {
            Ok(()) => self.push_log(format!("Deleted {name:?}")),
            Err(e) => self.push_log(format!("Delete failed: {e}")),
        }
        if self.selected.as_deref() == Some(name) {
            self.selected = None;
        }
        self.reload();
        if self.selected.is_none() {
            self.selected = self.bottles.first().map(|b| b.name().to_string());
            self.select_changed();
        }
    }

    fn delete_bottle_dialog(&mut self, ctx: &egui::Context) {
        let Some(name) = self.delete_bottle.clone() else {
            return;
        };
        let Some(bottle) = self.bottles.iter().find(|b| b.name() == name).cloned() else {
            self.delete_bottle = None;
            return;
        };
        let busy = self.jobs.is_busy();
        let mut delete = false;
        let mut cancel = false;

        let modal = egui::Modal::new(egui::Id::unique("delete_bottle")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(format!("Delete “{name}”?"));
            ui.add_space(6.0);
            ui.label(if steam::is_installed(&bottle) {
                "This stops everything running in the bottle, then removes it \
                 with everything installed in it, including Steam and its games."
            } else {
                "This stops everything running in the bottle, then removes it \
                 with everything installed in it."
            });
            ui.colored_label(ui.visuals().warn_fg_color, "This can't be undone.");
            ui.weak(bottle.path.display().to_string());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let button = egui::Button::new(
                    egui::RichText::new("Delete").color(ui.visuals().error_fg_color),
                );
                if ui
                    .add_enabled(!busy, button)
                    .on_disabled_hover_text("Wait for the current task to finish")
                    .clicked()
                {
                    delete = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if delete {
            self.delete_bottle = None;
            self.delete_bottle_now(&name);
        } else if cancel || modal.should_close() {
            self.delete_bottle = None;
        }
    }

    fn new_bottle_dialog(&mut self, ctx: &egui::Context) {
        let Some(nb) = self.new_bottle.as_mut() else {
            return;
        };
        let runtimes = self.runtimes.clone();
        let mut create = false;
        let mut cancel = false;
        let name_ok = winewrap_core::bottle::validate_name(&nb.config.name).is_ok()
            && !self.bottles.iter().any(|b| b.name() == nb.config.name);
        let disabled_reason = if runtimes.is_empty() {
            Some("Install Wine first (see above)")
        } else if !name_ok {
            Some("Choose a unique bottle name")
        } else {
            None
        };
        let mut wine_actions = WineActions::default();
        let mut flavor = self.wine_flavor;
        let busy = self.jobs.is_busy();

        let modal = egui::Modal::new(egui::Id::unique("new_bottle")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("New bottle");
            ui.add_space(6.0);
            if runtimes.is_empty() {
                wine_actions = missing_wine_box(ui, &mut flavor, busy);
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut nb.config.name);
            });
            if !name_ok {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Choose a unique name (letters, digits, spaces, - _ .)",
                );
            }
            settings_form(ui, &mut nb.config, &runtimes);
            ui.checkbox(&mut nb.install_steam, "Install Steam");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(disabled_reason.is_none(), egui::Button::new("Create"))
                    .on_disabled_hover_text(disabled_reason.unwrap_or_default())
                    .clicked()
                {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if let Some(reason) = disabled_reason {
                    ui.weak(reason);
                }
            });
        });
        self.wine_flavor = flavor;
        self.handle_wine_actions(ctx, wine_actions);
        if cancel || modal.should_close() {
            self.new_bottle = None;
        } else if create {
            if let Some(nb) = self.new_bottle.take() {
                self.create_bottle(ctx, nb);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(label) = self.jobs.running.first() {
                ui.spinner();
                ui.label(label);
                if let Some(Some(p)) = self.progress {
                    ui.add(
                        egui::ProgressBar::new(p)
                            .desired_width(160.0)
                            .show_percentage(),
                    );
                }
            } else {
                ui.weak(format!("{} Wine runtime(s)", self.runtimes.len()));
                if let Some(last) = self.log.last() {
                    ui.separator();
                    ui.weak(last);
                }
            }
        });
        egui::CollapsingHeader::new("Activity")
            .id_salt("activity")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.monospace(line);
                        }
                    });
            });
    }
}

impl eframe::App for WineWrapApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_events();
        let ctx = ui.ctx().clone();

        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("bottles")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            let busy = self.jobs.is_busy();
            let mut actions = WineActions::default();
            if self.needs_rosetta {
                actions.install_rosetta = rosetta_box(ui, busy);
                ui.add_space(8.0);
            }
            if self.runtimes.is_empty() && self.new_bottle.is_none() {
                let wine = missing_wine_box(ui, &mut self.wine_flavor, busy);
                actions.install = wine.install;
                actions.rescan = wine.rescan;
                actions.add = wine.add;
                ui.add_space(8.0);
            }
            self.handle_wine_actions(&ctx, actions);
            self.bottle_view(ui);
        });

        self.new_bottle_dialog(&ctx);
        self.delete_bottle_dialog(&ctx);
    }
}

/// Buttons pressed in [`missing_wine_box`] / [`rosetta_box`].
#[derive(Default)]
struct WineActions {
    install: bool,
    install_rosetta: bool,
    rescan: bool,
    add: bool,
}

/// Explains that Wine is missing and offers to install it.
fn missing_wine_box(ui: &mut egui::Ui, flavor: &mut WineFlavor, busy: bool) -> WineActions {
    let mut actions = WineActions::default();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            egui::RichText::new("⚠ Wine is not installed").strong(),
        );
        ui.label(
            "WineWrap runs Windows programs with Wine. It can download and install \
             Wine for you (a few hundred MB; no Homebrew or Terminal needed).",
        );
        ui.horizontal(|ui| {
            actions.install = ui
                .add_enabled(!busy, egui::Button::new("⬇ Install Wine"))
                .on_disabled_hover_text("Wait for the current task to finish")
                .clicked();
            egui::ComboBox::from_id_salt("wine_flavor")
                .selected_text(flavor.to_string())
                .show_ui(ui, |ui| {
                    ui.selectable_value(flavor, WineFlavor::Stable, "stable (recommended)");
                    ui.selectable_value(flavor, WineFlavor::Devel, "devel (newer)");
                    ui.selectable_value(
                        flavor,
                        WineFlavor::Staging,
                        "staging (newer + extra patches)",
                    );
                });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.weak("Already have Wine somewhere else?");
            actions.rescan = ui.small_button("⟳ Rescan").clicked();
            actions.add = ui
                .small_button("Add Wine folder…")
                .on_hover_text("Choose a Wine installation folder (the one containing bin/wine)")
                .clicked();
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak("Wine builds come from");
            ui.hyperlink_to("Gcenx/macOS_Wine_builds", wine_builds::BUILDS_URL);
        });
    });
    actions
}

/// Explains that Rosetta 2 is missing (Apple Silicon) and offers to install it.
fn rosetta_box(ui: &mut egui::Ui, busy: bool) -> bool {
    let mut install = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            egui::RichText::new("⚠ Rosetta 2 is not installed").strong(),
        );
        ui.label(
            "Wine is an Intel (x86_64) program, so on Apple Silicon Macs it needs \
             Apple's Rosetta 2. macOS will ask for your password to install it.",
        );
        install = ui
            .add_enabled(!busy, egui::Button::new("Install Rosetta 2"))
            .clicked();
    });
    install
}

/// Shared settings widgets for the new-bottle dialog and the settings panel.
fn settings_form(ui: &mut egui::Ui, cfg: &mut BottleConfig, runtimes: &[WineRuntime]) {
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Wine runtime");
            let current = cfg.runtime.clone().unwrap_or_else(|| "Automatic".into());
            egui::ComboBox::from_id_salt("runtime")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.runtime, None, "Automatic");
                    for rt in runtimes {
                        let label = if rt.has_d3dmetal() {
                            format!("{} ({}, D3DMetal)", rt.name, rt.kind)
                        } else {
                            format!("{} ({})", rt.name, rt.kind)
                        };
                        ui.selectable_value(&mut cfg.runtime, Some(rt.name.clone()), label);
                    }
                });
            ui.end_row();

            ui.label("Windows version");
            enum_combo(ui, "winver", &mut cfg.windows_version, WindowsVersion::ALL);
            ui.end_row();

            ui.label("Graphics");
            enum_combo(ui, "graphics", &mut cfg.graphics, GraphicsBackend::ALL);
            ui.end_row();

            ui.label("Synchronisation");
            enum_combo(ui, "sync", &mut cfg.sync, SyncMode::ALL);
            ui.end_row();

            ui.label("Options");
            ui.vertical(|ui| {
                ui.checkbox(&mut cfg.retina, "Retina mode");
                ui.checkbox(&mut cfg.metal_hud, "Metal performance HUD");
                ui.checkbox(&mut cfg.advertise_avx, "Advertise AVX to games (macOS 15+)");
                if cfg.graphics == GraphicsBackend::Dxvk {
                    ui.checkbox(&mut cfg.dxvk_async, "DXVK async shader compilation");
                }
            });
            ui.end_row();
        });
}

fn enum_combo<T>(ui: &mut egui::Ui, id: &str, value: &mut T, all: &[T])
where
    T: Copy + PartialEq + std::fmt::Display,
{
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.to_string())
        .show_ui(ui, |ui| {
            for v in all {
                ui.selectable_value(value, *v, v.to_string());
            }
        });
}

/// Whether starting Steam in a bottle with this config first switches it to
/// WineWrap's Wine build (see `WineWrap::use_steam_runtime`).
fn needs_steam_runtime(config: &BottleConfig) -> bool {
    cfg!(target_os = "macos") && steam::needs_winewrap_runtime(config)
}

/// Move a Steam bottle to WineWrap's Wine build, downloading it if needed, and
/// return the runtime the bottle now uses.
fn prepare_steam_runtime(
    ww: &WineWrap,
    bottle: &mut Bottle,
    r: &Reporter,
) -> winewrap_core::Result<WineRuntime> {
    let build = wine_builds::winewrap_build();
    if needs_steam_runtime(&bottle.config) && ww.runtime(&build.runtime_name()).is_err() {
        r.log(format!(
            "Downloading WineWrap's Wine for Steam ({:.0} MB) …",
            build.size as f64 / 1_000_000.0
        ));
    }
    if ww.use_steam_runtime(&mut bottle.config, install_progress(r))? {
        bottle.save()?;
        r.log(format!(
            "{:?} now uses {}: Steam's window stays black with other Wine builds.",
            bottle.name(),
            build.runtime_name()
        ));
    }
    r.progress(None);
    ww.runtime_for(bottle)
}

/// Report a Wine download to the progress bar and log.
fn install_progress(r: &Reporter) -> impl FnMut(InstallProgress) + '_ {
    move |p| match p {
        InstallProgress::Downloading { done, total } => {
            r.progress(total.filter(|&t| t > 0).map(|t| done as f32 / t as f32))
        }
        InstallProgress::Extracting => {
            r.progress(None);
            r.log("Unpacking Wine …");
        }
    }
}

/// Download (if needed) and run the Steam installer.
fn install_steam_job(
    ww: &WineWrap,
    bottle: &Bottle,
    rt: &WineRuntime,
    r: &Reporter,
) -> winewrap_core::Result<()> {
    r.log("Downloading Steam installer …");
    let setup: PathBuf = ww.steam_installer(|done, total| {
        r.progress(total.filter(|&t| t > 0).map(|t| done as f32 / t as f32));
    })?;
    r.progress(None);
    r.log("Running Steam installer …");
    steam::install_command(bottle, rt, &setup).run()?;
    LaunchSpec::wineserver(bottle, rt).arg("--wait").run()?;
    if !steam::is_installed(bottle) {
        return Err(winewrap_core::Error::SteamNotInstalled(
            bottle.name().to_string(),
        ));
    }
    r.log("Steam installed. Click “Launch Steam” to sign in.");
    Ok(())
}

fn open_in_file_manager(path: &Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(path).spawn() {
        tracing::warn!("could not open {}: {e}", path.display());
    }
}
