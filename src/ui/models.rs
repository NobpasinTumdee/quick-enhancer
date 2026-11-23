//! Models page (catalog browser, install / repair / update / uninstall, ONNX runtime, custom
//! models folder), the on-demand install dialog used by model dropdowns, and the model combo
//! widget itself.

use egui::{Color32, RichText};
use std::sync::mpsc::{Receiver, channel};

use crate::app::{App, Page};
use crate::models::catalog::{Catalog, Group, ModelEntry, RuntimeNeed};
use crate::models::custom::{self, CustomKind, CustomModel};
use crate::models::fit::{self, Fit};
use crate::models::install::{self, InstallSnapshot, ModelState, RuntimeKind};
use crate::net::tools::{CompStatus, Component, Installer, Source, ToolsManifest};
use crate::paths::AppPaths;
use crate::t;
use crate::util::human_bytes;

const OK: Color32 = Color32::from_rgb(110, 200, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERR: Color32 = Color32::from_rgb(235, 100, 100);
const NC: Color32 = Color32::from_rgb(220, 120, 200);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Installed,
    Fits,
}

/// "Install this model?" confirmation (on-demand install from a dropdown, or the page).
#[derive(Clone)]
pub struct InstallAsk {
    pub model_id: String,
    pub repair: bool,
    accept_nc: bool,
}

pub struct ModelsUi {
    pub catalog: Catalog,
    pub tools: ToolsManifest,
    pub snap: InstallSnapshot,
    pub custom: Vec<CustomModel>,
    installer: Option<Installer>,
    /// Model ids being installed by the current run (for row status).
    installing: Vec<String>,
    /// Package ids of the current run.
    run_packages: Vec<String>,
    reported: bool,
    filter: Filter,
    pub ask: Option<InstallAsk>,
    refresh_rx: Option<Receiver<Result<Option<String>, String>>>,
    confirm_uninstall: Option<String>,
}

impl ModelsUi {
    pub fn new(paths: &AppPaths) -> ModelsUi {
        let catalog = Catalog::load(&paths.base);
        let tools = ToolsManifest::embedded();
        let snap = InstallSnapshot::scan(paths, &catalog, &tools);
        ModelsUi {
            custom: custom::scan(&custom::custom_dir(&paths.models)),
            catalog,
            tools,
            snap,
            installer: None,
            installing: Vec::new(),
            run_packages: Vec::new(),
            reported: false,
            filter: Filter::All,
            ask: None,
            refresh_rx: None,
            confirm_uninstall: None,
        }
    }

    pub fn rescan(&mut self, paths: &AppPaths) {
        self.snap = InstallSnapshot::scan(paths, &self.catalog, &self.tools);
        self.custom = custom::scan(&custom::custom_dir(&paths.models));
    }

    pub fn state(&self, m: &ModelEntry) -> ModelState {
        self.snap.state(&self.catalog, &self.tools, m)
    }

    pub fn ready(&self, m: &ModelEntry) -> bool {
        self.snap.ready(&self.catalog, &self.tools, m)
    }

    pub fn running(&self) -> bool {
        self.installer.as_ref().is_some_and(Installer::is_running)
    }

    /// True once after an install run finished (the app then reloads tools and models).
    pub fn take_finished(&mut self, paths: &AppPaths) -> bool {
        match &self.installer {
            Some(i) if !self.reported && i.snapshot().finished => {
                self.reported = true;
                self.rescan(paths);
                true
            }
            _ => false,
        }
    }

    /// Ask before installing `id` (shows the install dialog).
    pub fn request_install(&mut self, id: &str, repair: bool) {
        self.ask = Some(InstallAsk {
            model_id: id.to_string(),
            repair,
            accept_nc: false,
        });
    }

    pub fn plan(&self, m: &ModelEntry, runtime: RuntimeKind, repair: bool) -> Vec<Component> {
        install::install_plan(&self.catalog, &self.tools, &self.snap, m, runtime, repair)
    }

    /// Start downloading `packages` (for `models`, shown as "installing").
    pub fn start(&mut self, paths: &AppPaths, ctx: &egui::Context, packages: Vec<Component>, models: Vec<String>) {
        if packages.is_empty() || self.running() {
            return;
        }
        self.installing = models;
        self.run_packages = packages.iter().map(|p| p.id.clone()).collect();
        self.reported = false;
        let jobs = packages.into_iter().map(|p| (p, Source::Download)).collect();
        self.installer = Some(Installer::start(paths.clone(), jobs, Some(ctx.clone())));
    }

    fn total_disk(&self) -> u64 {
        let mut seen = std::collections::BTreeSet::new();
        self.catalog
            .models
            .iter()
            .flat_map(|m| m.files.iter())
            .filter(|f| seen.insert(f.as_str()))
            .filter_map(|f| std::fs::metadata(self.snap.base.join(f)).ok())
            .map(|md| md.len())
            .sum()
    }

    /// Progress line for the current run (page footer and dialogs).
    pub fn progress_ui(&mut self, ui: &mut egui::Ui) {
        let Some(inst) = &self.installer else { return };
        let snap = inst.snapshot();
        if !snap.running && !snap.finished {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for id in &self.run_packages {
                let name = self
                    .catalog
                    .resolve_package(id, &self.tools)
                    .map(|p| p.name)
                    .unwrap_or_else(|| id.clone());
                match snap.status.get(id) {
                    Some(CompStatus::Downloading(p)) => {
                        let frac = p.total.map(|t| p.done as f32 / t.max(1) as f32).unwrap_or(0.0);
                        ui.label(t!("models.downloading", name = name));
                        ui.add(egui::ProgressBar::new(frac).desired_width(180.0).show_percentage());
                        ui.label(RichText::new(t!("tools.speed", speed = human_bytes(p.speed as u64))).small().weak());
                    }
                    Some(CompStatus::Verifying) => {
                        ui.label(format!("{name}: {}", t!("tools.st_verifying")));
                    }
                    Some(CompStatus::Extracting) => {
                        ui.label(format!("{name}: {}", t!("tools.st_extracting")));
                    }
                    Some(CompStatus::Failed(e)) => {
                        ui.colored_label(ERR, format!("{name}: {}", t!("tools.st_failed"))).on_hover_text(e);
                    }
                    Some(CompStatus::Cancelled) => {
                        ui.colored_label(WARN, format!("{name}: {}", t!("tools.st_cancelled")));
                    }
                    _ => {}
                }
            }
            if snap.running {
                ui.spinner();
                if ui.button(t!("common.cancel")).clicked() {
                    inst.cancel.cancel();
                }
            } else if snap.status.values().any(|s| matches!(s, CompStatus::Failed(_))) {
                ui.colored_label(ERR, t!("models.install_failed"));
            } else if snap.finished && !snap.status.values().any(|s| matches!(s, CompStatus::Cancelled)) {
                ui.colored_label(OK, t!("models.install_done"));
            }
        });
    }

    fn row_status(&self, m: &ModelEntry) -> Option<CompStatus> {
        if !self.installing.contains(&m.id) {
            return None;
        }
        let snap = self.installer.as_ref()?.snapshot();
        if !snap.running {
            return None;
        }
        m.packages.iter().find_map(|p| match snap.status.get(p) {
            Some(s @ (CompStatus::Downloading(_) | CompStatus::Verifying | CompStatus::Extracting)) => Some(s.clone()),
            _ => None,
        }).or(Some(CompStatus::Waiting))
    }
}

/// Stars "★★★☆☆".
fn stars(n: u8) -> String {
    (1..=5).map(|i| if i <= n { '★' } else { '☆' }).collect()
}

fn fit_color(f: Fit) -> Color32 {
    match f {
        Fit::Good => OK,
        Fit::Tight => WARN,
        _ => ERR,
    }
}

/// Download size of a model's own packages.
pub fn model_size(cat: &Catalog, tools: &ToolsManifest, m: &ModelEntry) -> u64 {
    m.packages
        .iter()
        .filter_map(|p| cat.resolve_package(p, tools))
        .map(|p| p.size)
        .sum()
}

pub fn models_page(app: &mut App, ui: &mut egui::Ui) {
    // Finish a catalog refresh started earlier.
    if let Some(rx) = &app.models_ui.refresh_rx
        && let Ok(r) = rx.try_recv()
    {
        app.models_ui.refresh_rx = None;
        match r {
            Ok(Some(v)) => {
                app.models_ui.catalog = Catalog::load(&app.paths.base);
                app.models_ui.rescan(&app.paths);
                app.toast(t!("models.catalog_updated", version = v));
            }
            Ok(None) => app.toast(t!("models.catalog_current")),
            Err(e) => app.toast(t!("models.catalog_failed", error = e)),
        }
    }
    egui::Panel::bottom("models-progress").show(ui, |ui| {
        app.models_ui.progress_ui(ui);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(t!("models.title"));
            ui.label(RichText::new(t!("models.intro")).weak());
            toolbar(app, ui);
            ui.separator();
            runtime_section(app, ui);
            for g in Group::SHOWN {
                group_section(app, ui, g);
            }
            custom_section(app, ui);
        });
    });
}

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    let mu = &mut app.models_ui;
    ui.horizontal_wrapped(|ui| {
        ui.label(t!("models.show"));
        ui.radio_value(&mut mu.filter, Filter::All, t!("models.filter_all"));
        ui.radio_value(&mut mu.filter, Filter::Installed, t!("models.filter_installed"));
        ui.radio_value(&mut mu.filter, Filter::Fits, t!("models.filter_fits"));
        ui.separator();
        ui.label(t!("models.disk_used", size = human_bytes(mu.total_disk())));
        if ui.button(t!("models.open_folder")).clicked() {
            let _ = std::fs::create_dir_all(&app.paths.models);
            super::open_path(&app.paths.models);
        }
        if ui.button(t!("models.rescan")).clicked() {
            mu.rescan(&app.paths);
        }
        let busy = mu.refresh_rx.is_some();
        if ui
            .add_enabled(!busy, egui::Button::new(t!("models.check_catalog")))
            .on_hover_text(t!("models.check_catalog_tip", version = &mu.catalog.catalog_version))
            .clicked()
        {
            let (tx, rx) = channel();
            let cat = mu.catalog.clone();
            let base = app.paths.base.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send(cat.refresh(&base).map_err(|e| e.to_string()));
                ctx.request_repaint();
            });
            mu.refresh_rx = Some(rx);
        }
        if busy {
            ui.spinner();
        }
    });
}

fn runtime_section(app: &mut App, ui: &mut egui::Ui) {
    let has_nvidia = app.env.gpus.iter().any(|g| g.vendor_id == fit::NVIDIA_VENDOR);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(t!("models.runtime_title")).strong());
        ui.label(RichText::new(t!("models.runtime_intro")).small().weak());
        let running = app.models_ui.running();
        let mut changed = false;
        for kind in [RuntimeKind::DirectMl, RuntimeKind::Cuda] {
            let have = app.models_ui.snap.has_runtime(kind);
            let size: u64 = kind
                .packages()
                .iter()
                .filter_map(|p| app.models_ui.catalog.package(p))
                .map(|p| p.size)
                .sum();
            ui.horizontal_wrapped(|ui| {
                let (label, tip) = match kind {
                    RuntimeKind::DirectMl => (t!("models.rt_dml"), t!("models.rt_dml_tip")),
                    RuntimeKind::Cuda => (t!("models.rt_cuda"), t!("models.rt_cuda_tip")),
                };
                if ui
                    .radio(app.config.onnx_runtime == kind, label)
                    .on_hover_text(tip)
                    .clicked()
                    && app.config.onnx_runtime != kind
                {
                    app.config.onnx_runtime = kind;
                    changed = true;
                }
                if have {
                    ui.colored_label(OK, t!("models.state_installed"));
                    if ui.add_enabled(!running, egui::Button::new(t!("models.uninstall")).small()).clicked() {
                        match install::uninstall_runtime(&app.paths, &app.models_ui.catalog, &app.models_ui.tools, kind) {
                            Ok(()) => app.models_ui.rescan(&app.paths),
                            Err(e) => app.toast(t!("models.uninstall_failed", error = e)),
                        }
                    }
                } else {
                    ui.label(RichText::new(human_bytes(size)).weak());
                    let allowed = kind == RuntimeKind::DirectMl || has_nvidia;
                    let r = ui.add_enabled(
                        !running && allowed,
                        egui::Button::new(t!("models.install")).small(),
                    );
                    let r = if allowed { r } else { r.on_disabled_hover_text(t!("models.fit_needs_nvidia")) };
                    if r.clicked() {
                        let pkgs: Vec<Component> = kind
                            .packages()
                            .iter()
                            .filter_map(|p| app.models_ui.catalog.package(p).cloned())
                            .collect();
                        let ctx = ui.ctx().clone();
                        app.models_ui.start(&app.paths, &ctx, pkgs, Vec::new());
                    }
                }
            });
        }
        if changed {
            crate::engines::onnx::runtime::configure(&app.paths.base, app.config.onnx_runtime, &app.env.gpus);
            // Only one ONNX Runtime can be loaded per process: a switch after the first ONNX
            // job takes effect on the next start.
            if crate::engines::onnx::runtime::loaded_kind().is_some_and(|k| k != app.config.onnx_runtime) {
                app.toast(t!("models.runtime_restart"));
            }
        }
    });
}

fn group_section(app: &mut App, ui: &mut egui::Ui, g: Group) {
    let ids: Vec<String> = app
        .models_ui
        .catalog
        .in_group(g)
        .filter(|m| match app.models_ui.filter {
            Filter::All => true,
            Filter::Installed => app.models_ui.state(m) != ModelState::NotInstalled,
            Filter::Fits => fit::fit(m, &app.env.gpus) == Fit::Good,
        })
        .map(|m| m.id.clone())
        .collect();
    if ids.is_empty() {
        return;
    }
    egui::CollapsingHeader::new(RichText::new(crate::i18n::tr(g.label_key())).strong().size(16.0))
        .id_salt(("models-group", g))
        .default_open(true)
        .show(ui, |ui| {
            for id in ids {
                model_row(app, ui, &id);
            }
        });
}

fn model_row(app: &mut App, ui: &mut egui::Ui, id: &str) {
    let Some(m) = app.models_ui.catalog.model(id).cloned() else { return };
    let state = app.models_ui.state(&m);
    let fitv = fit::fit(&m, &app.env.gpus);
    let size = model_size(&app.models_ui.catalog, &app.models_ui.tools, &m);
    let running = app.models_ui.running();
    let row_status = app.models_ui.row_status(&m);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(m.name.get()).strong());
            if m.experimental {
                ui.label(RichText::new(t!("models.badge_experimental")).small().color(WARN));
            }
            if m.non_commercial() {
                ui.label(RichText::new(t!("models.badge_nc")).small().strong().color(NC))
                    .on_hover_text(t!("models.nc_tip"));
            }
            let scales: Vec<String> = m.scales.iter().map(|s| format!("{s}x")).collect();
            ui.label(RichText::new(scales.join(" / ")).weak());
        });
        ui.label(m.desc.get());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} {}", t!("models.speed"), stars(m.speed))).small());
            ui.label(RichText::new(format!("{} {}", t!("models.quality"), stars(m.quality))).small());
            ui.colored_label(fit_color(fitv), RichText::new(crate::i18n::tr(fitv.label_key())).small())
                .on_hover_text(t!("models.vram_need", min = m.vram_min_mb, rec = m.vram_rec_mb));
            ui.label(RichText::new(human_bytes(size)).small().weak());
            ui.hyperlink_to(RichText::new(&m.license.name).small(), &m.license.url)
                .on_hover_text(&m.license.attribution);
            if m.needs_runtime() == Some(RuntimeNeed::Cuda) {
                ui.label(RichText::new(t!("models.needs_cuda")).small().color(WARN));
            }
        });
        ui.horizontal(|ui| {
            match &row_status {
                Some(CompStatus::Downloading(p)) => {
                    let frac = p.total.map(|t| p.done as f32 / t.max(1) as f32).unwrap_or(0.0);
                    ui.add(egui::ProgressBar::new(frac).desired_width(200.0).show_percentage());
                }
                Some(_) => {
                    ui.spinner();
                    ui.label(t!("tools.st_waiting"));
                }
                None => {
                    let color = match state {
                        ModelState::Installed => OK,
                        ModelState::NotInstalled => ui.visuals().weak_text_color(),
                        _ => WARN,
                    };
                    ui.colored_label(color, crate::i18n::tr(state.label_key()));
                    if state.is_usable() && !app.models_ui.ready(&m) {
                        ui.colored_label(WARN, RichText::new(t!("models.needs_more")).small());
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = !running && fitv != Fit::NeedsNvidia;
                match state {
                    ModelState::NotInstalled => {
                        if ui.add_enabled(can, egui::Button::new(t!("models.install"))).clicked() {
                            app.models_ui.request_install(&m.id, false);
                        }
                    }
                    ModelState::Partial => {
                        if ui.add_enabled(can, egui::Button::new(t!("models.repair"))).clicked() {
                            app.models_ui.request_install(&m.id, true);
                        }
                    }
                    ModelState::Outdated => {
                        if ui.add_enabled(can, egui::Button::new(t!("models.update"))).clicked() {
                            app.models_ui.request_install(&m.id, true);
                        }
                    }
                    ModelState::Installed => {
                        if !app.models_ui.ready(&m)
                            && ui.add_enabled(can, egui::Button::new(t!("models.install_missing"))).clicked()
                        {
                            app.models_ui.request_install(&m.id, false);
                        }
                    }
                }
                let removable = state != ModelState::NotInstalled
                    && m.packages.iter().any(|p| app.models_ui.catalog.package(p).is_some());
                if removable && ui.add_enabled(!running, egui::Button::new(t!("models.uninstall"))).clicked() {
                    app.models_ui.confirm_uninstall = Some(m.id.clone());
                }
                if state != ModelState::NotInstalled
                    && ui.add_enabled(!running, egui::Button::new(t!("models.reinstall")).small())
                        .on_hover_text(t!("models.reinstall_tip"))
                        .clicked()
                {
                    app.models_ui.request_install(&m.id, true);
                }
            });
        });
    });
}

fn custom_section(app: &mut App, ui: &mut egui::Ui) {
    let dir = custom::custom_dir(&app.paths.models);
    egui::CollapsingHeader::new(RichText::new(t!("models.group_custom")).strong().size(16.0))
        .id_salt("models-custom")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(RichText::new(t!("models.custom_intro", path = dir.display())).small().weak());
            ui.horizontal_wrapped(|ui| {
                if ui.button(t!("models.custom_add")).clicked()
                    && let Some(files) = rfd::FileDialog::new()
                        .add_filter(t!("models.custom_filter"), &["onnx", "param", "bin"])
                        .pick_files()
                {
                    let _ = std::fs::create_dir_all(&dir);
                    let mut n = 0;
                    for f in files {
                        if let Some(name) = f.file_name()
                            && std::fs::copy(&f, dir.join(name)).is_ok()
                        {
                            n += 1;
                        }
                    }
                    app.models_ui.rescan(&app.paths);
                    app.toast(t!("models.custom_added", n = n));
                }
                if ui.button(t!("models.open_folder")).clicked() {
                    let _ = std::fs::create_dir_all(&dir);
                    super::open_path(&dir);
                }
                if ui.button(t!("models.rescan")).clicked() {
                    app.models_ui.rescan(&app.paths);
                }
            });
            if app.models_ui.custom.is_empty() {
                ui.label(RichText::new(t!("models.custom_none")).weak());
            }
            for c in &app.models_ui.custom {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&c.name).strong());
                    ui.label(match c.kind {
                        CustomKind::Ncnn => "ncnn",
                        CustomKind::Onnx => "ONNX",
                    });
                    match c.scale {
                        Some(s) => ui.label(format!("{s}x")),
                        None => ui.label(RichText::new(t!("models.custom_scale_unknown")).small().color(WARN)),
                    };
                    if c.kind == CustomKind::Onnx && !app.models_ui.snap.any_runtime() {
                        ui.label(RichText::new(t!("models.needs_runtime")).small().color(WARN));
                    }
                });
            }
        });
}

/// The install confirmation and uninstall confirmation dialogs.
pub fn models_dialogs(app: &mut App, ctx: &egui::Context) {
    install_dialog(app, ctx);
    uninstall_dialog(app, ctx);
}

fn install_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(mut ask) = app.models_ui.ask.clone() else { return };
    let Some(m) = app.models_ui.catalog.model(&ask.model_id).cloned() else {
        app.models_ui.ask = None;
        return;
    };
    let plan = app.models_ui.plan(&m, app.config.onnx_runtime, ask.repair);
    let fitv = fit::fit(&m, &app.env.gpus);
    let mut close = false;
    let mut go = false;
    egui::Window::new(t!("models.install_title", name = m.name.get()))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.label(m.desc.get());
            ui.label(
                RichText::new(format!(
                    "{} {} · {} {} · {}",
                    t!("models.speed"),
                    stars(m.speed),
                    t!("models.quality"),
                    stars(m.quality),
                    crate::i18n::tr(fitv.label_key())
                ))
                .small(),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{}:", t!("tools.license"))).small());
                ui.hyperlink_to(RichText::new(&m.license.name).small(), &m.license.url);
                ui.label(RichText::new(&m.license.attribution).small().weak());
            });
            ui.separator();
            if plan.is_empty() {
                ui.label(t!("models.nothing_to_download"));
            } else {
                ui.label(t!("models.will_download"));
                for p in &plan {
                    ui.label(RichText::new(format!("• {} — {}", p.name, human_bytes(p.size))).small());
                }
                let total = install::plan_size(&plan);
                let free = crate::pipeline::temp::free_space(&app.paths.base);
                ui.label(RichText::new(t!("models.total_size", size = human_bytes(total))).strong());
                if let Some(free) = free {
                    // Download + extraction need roughly twice the size for a moment.
                    let col = if free < total.saturating_mul(2) { ERR } else { ui.visuals().weak_text_color() };
                    ui.colored_label(col, RichText::new(t!("models.free_space", size = human_bytes(free))).small());
                }
            }
            if m.experimental {
                ui.colored_label(WARN, t!("models.experimental_note"));
            }
            if fitv == Fit::TooBig {
                ui.colored_label(WARN, t!("models.too_big_note"));
            }
            let accepted = app.config.accepted_licenses.contains(&m.id);
            if m.non_commercial() && !accepted {
                ui.separator();
                ui.colored_label(NC, RichText::new(t!("models.nc_title")).strong());
                ui.label(t!("models.nc_body", license = &m.license.name));
                ui.checkbox(&mut ask.accept_nc, t!("models.nc_accept"));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok = !app.models_ui.running()
                    && fitv != Fit::NeedsNvidia
                    && (!m.non_commercial() || accepted || ask.accept_nc);
                let label = if ask.repair { t!("models.repair") } else { t!("models.install") };
                if ui.add_enabled(ok, egui::Button::new(RichText::new(label).strong())).clicked() {
                    go = true;
                }
                if ui.button(t!("common.cancel")).clicked() {
                    close = true;
                }
            });
            if app.models_ui.running() {
                ui.label(RichText::new(t!("models.busy")).small().color(WARN));
            }
        });
    if go {
        if m.non_commercial() && !app.config.accepted_licenses.contains(&m.id) {
            app.config.accepted_licenses.push(m.id.clone());
        }
        let ctx2 = ctx.clone();
        if plan.is_empty() {
            app.models_ui.rescan(&app.paths);
        } else {
            app.models_ui.start(&app.paths, &ctx2, plan, vec![m.id.clone()]);
        }
        app.models_ui.ask = None;
    } else if close {
        app.models_ui.ask = None;
    } else {
        app.models_ui.ask = Some(ask);
    }
}

fn uninstall_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(id) = app.models_ui.confirm_uninstall.clone() else { return };
    let Some(m) = app.models_ui.catalog.model(&id).cloned() else {
        app.models_ui.confirm_uninstall = None;
        return;
    };
    let mut done = false;
    egui::Window::new(t!("models.uninstall_title", name = m.name.get()))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(t!("models.uninstall_body"));
            if app.runner.is_busy() {
                ui.colored_label(WARN, t!("models.uninstall_busy"));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!app.runner.is_busy(), egui::Button::new(t!("models.uninstall")))
                    .clicked()
                {
                    let mu = &app.models_ui;
                    match install::uninstall(&app.paths, &mu.catalog, &mu.tools, &mu.snap, &m) {
                        Ok(removed) if removed.is_empty() => app.toast(t!("models.uninstall_shared")),
                        Ok(_) => app.toast(t!("models.uninstalled", name = m.name.get())),
                        Err(e) => app.toast(t!("models.uninstall_failed", error = e)),
                    }
                    app.models_ui.rescan(&app.paths);
                    app.reload_tools(ctx);
                    done = true;
                }
                if ui.button(t!("common.cancel")).clicked() {
                    done = true;
                }
            });
        });
    if done {
        app.models_ui.confirm_uninstall = None;
    }
}

/// What the user picked in a [`model_combo`].
pub enum ComboPick {
    None,
    /// An installed model was selected (already written to `selected`).
    Selected,
    /// A model that isn't installed: ask to install it.
    Install(String),
    /// "Get more models…": open the Models page.
    OpenPage,
}

/// Model dropdown: installed models first; models that aren't installed are greyed out with
/// ⬇ and their size, and picking one asks to install it. `extra` are installed entries that
/// are not in the catalog (old RIFE models, custom models): `(id, label)`.
pub fn model_combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    mu: &ModelsUi,
    group: Group,
    extra: &[(String, String)],
    selected: &mut String,
) -> ComboPick {
    let mut pick = ComboPick::None;
    let entries: Vec<&ModelEntry> = mu.catalog.in_group(group).collect();
    let label_of = |id: &str| -> String {
        entries
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.name.get().to_string())
            .or_else(|| extra.iter().find(|(e, _)| e == id).map(|(_, l)| l.clone()))
            .unwrap_or_else(|| id.to_string())
    };
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(label_of(selected))
        .width(240.0)
        .show_ui(ui, |ui| {
            let (ready, missing): (Vec<&&ModelEntry>, Vec<&&ModelEntry>) =
                entries.iter().partition(|m| mu.ready(m));
            for (id, label) in extra {
                if entries.iter().any(|m| &m.id == id) {
                    continue;
                }
                if ui.selectable_label(selected == id, label).clicked() {
                    *selected = id.clone();
                    pick = ComboPick::Selected;
                }
            }
            for m in ready {
                if ui.selectable_label(*selected == m.id, m.name.get()).clicked() {
                    *selected = m.id.clone();
                    pick = ComboPick::Selected;
                }
            }
            if !missing.is_empty() {
                ui.separator();
            }
            for m in missing {
                let size = human_bytes(model_size(&mu.catalog, &mu.tools, m));
                let text = RichText::new(format!("⬇ {}  ({size})", m.name.get())).weak();
                if ui
                    .selectable_label(false, text)
                    .on_hover_text(t!("models.click_to_install"))
                    .clicked()
                {
                    pick = ComboPick::Install(m.id.clone());
                }
            }
            ui.separator();
            if ui.selectable_label(false, t!("models.get_more")).clicked() {
                pick = ComboPick::OpenPage;
            }
        });
    pick
}

/// Upscale model dropdown: "Automatic (profile default)", custom models, then the catalog.
/// `selected` is `None` for automatic.
pub fn upscale_model_combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    mu: &ModelsUi,
    models: &crate::paths::ModelSet,
    selected: &mut Option<String>,
) -> ComboPick {
    let mut extra = vec![(String::new(), t!("settings.model_auto"))];
    extra.extend(
        models
            .custom
            .iter()
            .map(|c| (c.id.clone(), t!("settings.model_custom", name = &c.name))),
    );
    let mut cur = selected.clone().unwrap_or_default();
    let pick = model_combo(ui, id_salt, mu, Group::Upscale, &extra, &mut cur);
    if matches!(pick, ComboPick::Selected) {
        *selected = (!cur.is_empty()).then_some(cur);
    }
    pick
}

/// Apply a [`ComboPick`] that needs the app (install dialog / page switch).
/// Warning + "Install missing model" button for catalog models the settings need.
pub fn missing_row(ui: &mut egui::Ui, mu: &ModelsUi, ids: &[String]) -> ComboPick {
    let mut pick = ComboPick::None;
    if ids.is_empty() {
        return pick;
    }
    let names: Vec<String> = ids
        .iter()
        .map(|id| mu.catalog.model(id).map(|m| m.name.get().to_string()).unwrap_or_else(|| id.clone()))
        .collect();
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(WARN, format!("⚠ {}", t!("models.missing_for_job", names = names.join(", "))));
        if mu.running() {
            ui.spinner();
        } else if ui.button(t!("models.install_missing")).clicked() {
            pick = ComboPick::Install(ids[0].clone());
        }
    });
    pick
}

pub fn handle_pick(app: &mut App, pick: ComboPick) {
    match pick {
        ComboPick::Install(id) => app.models_ui.request_install(&id, false),
        ComboPick::OpenPage => app.page = Page::Models,
        ComboPick::None | ComboPick::Selected => {}
    }
}
