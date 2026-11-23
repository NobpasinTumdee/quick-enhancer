//! Tools manager: download / repair / offline-install the external tools listed in the
//! embedded `tools.json`. Also the whole window when started with `--install-tools`.

use egui::{Color32, RichText};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::app::App;
use crate::net::tools::{
    CompStatus, Component, InstalledTools, Installer, Source, ToolsManifest, find_archives_in,
    outdated,
};
use crate::paths::{AppPaths, ModelSet, ToolSet};
use crate::t;
use crate::util::human_bytes;

const OK: Color32 = Color32::from_rgb(110, 200, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERR: Color32 = Color32::from_rgb(235, 100, 100);

#[derive(Default)]
pub struct ToolsUi {
    pub open: bool,
    components: Vec<Component>,
    selected: BTreeMap<String, bool>,
    /// Components already present on disk.
    present: BTreeMap<String, bool>,
    installed: InstalledTools,
    offline: BTreeMap<String, PathBuf>,
    installer: Option<Installer>,
    reported: bool,
    licenses_ok: bool,
}

fn is_present(id: &str, tools: &ToolSet, models: &ModelSet) -> bool {
    match id {
        "ffmpeg" => tools.ffmpeg.is_some() && tools.ffprobe.is_some(),
        "realesrgan" => tools.realesrgan.is_some() && models.has("realesrgan-x4plus"),
        "rife" => tools.rife.is_some() && !models.rife.is_empty(),
        _ => false,
    }
}

impl ToolsUi {
    /// (Re)load the manifest and what is installed, and show the window.
    pub fn open(&mut self, paths: &AppPaths) {
        let tools = ToolSet::discover(paths);
        let models = ModelSet::discover(&paths.models);
        self.components = ToolsManifest::embedded().for_platform();
        self.installed = InstalledTools::load(&paths.tools_state_file);
        let old = outdated(&self.components, &self.installed);
        self.present.clear();
        self.selected.clear();
        for c in &self.components {
            let have = is_present(&c.id, &tools, &models);
            self.present.insert(c.id.clone(), have);
            self.selected
                .insert(c.id.clone(), !have || old.contains(&c.id));
        }
        self.open = true;
    }

    pub fn running(&self) -> bool {
        self.installer.as_ref().is_some_and(Installer::is_running)
    }

    /// True once after an install run finished (the app then reloads its tools).
    pub fn take_finished(&mut self, paths: &AppPaths) -> bool {
        match &self.installer {
            Some(i) if !self.reported && i.snapshot().finished => {
                self.reported = true;
                self.refresh_presence(paths);
                true
            }
            _ => false,
        }
    }

    fn start(&mut self, paths: &AppPaths, ids: &[String], ctx: &egui::Context) {
        let jobs: Vec<(Component, Source)> = self
            .components
            .iter()
            .filter(|c| ids.contains(&c.id))
            .map(|c| {
                let src = match self.offline.get(&c.id) {
                    Some(p) => Source::LocalFile(p.clone()),
                    None => Source::Download,
                };
                (c.clone(), src)
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.reported = false;
        self.installer = Some(Installer::start(paths.clone(), jobs, Some(ctx.clone())));
    }

    fn add_offline(&mut self, files: Vec<PathBuf>) -> Vec<String> {
        let mut unknown = Vec::new();
        for f in files {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            match self
                .components
                .iter()
                .find(|c| c.file_name().to_lowercase() == name)
            {
                Some(c) => {
                    self.offline.insert(c.id.clone(), f.clone());
                    self.selected.insert(c.id.clone(), true);
                }
                None => unknown.push(name),
            }
        }
        unknown
    }

    /// The window body (shared by the window and the `--install-tools` page).
    fn body(&mut self, ui: &mut egui::Ui, paths: &AppPaths, toasts: &mut Vec<String>) {
        let state = self.installer.as_ref().map(Installer::snapshot);
        let running = state.as_ref().is_some_and(|s| s.running);
        ui.label(t!("tools.intro"));
        ui.label(
            RichText::new(t!("tools.location", path = paths.base.display()))
                .small()
                .weak(),
        );
        ui.add_space(6.0);
        for c in &self.components {
            let sel = self.selected.entry(c.id.clone()).or_insert(false);
            let have = self.present.get(&c.id).copied().unwrap_or(false);
            if c.required && !have {
                *sel = true;
            }
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!running && !(c.required && !have), |ui| ui.checkbox(sel, ""));
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&c.name).strong());
                            ui.label(RichText::new(&c.version).weak());
                            if c.required {
                                ui.label(RichText::new(t!("tools.required")).small().color(WARN));
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("{}: {}", t!("tools.license"), c.license))
                                    .small()
                                    .weak(),
                            );
                            if !c.homepage.is_empty() {
                                ui.hyperlink_to(RichText::new(t!("tools.homepage")).small(), &c.homepage);
                            }
                        });
                        if let Some(p) = self.offline.get(&c.id) {
                            ui.label(
                                RichText::new(t!("tools.from_file", file = p.display()))
                                    .small()
                                    .weak(),
                            );
                        }
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} {}", t!("tools.size"), human_bytes(c.size))).small());
                            ui.separator();
                            let st = state.as_ref().and_then(|s| s.status.get(&c.id).cloned());
                            status_cell(ui, st, have, self.installed.components.get(&c.id), &c.version);
                        });
                    });
                });
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new(t!("tools.credits")).small().weak());
        ui.checkbox(&mut self.licenses_ok, t!("tools.accept_licenses"));
        ui.add_space(4.0);
        let chosen: Vec<String> = self
            .selected
            .iter()
            .filter(|(_, v)| **v)
            .map(|(k, _)| k.clone())
            .collect();
        let total: u64 = self
            .components
            .iter()
            .filter(|c| chosen.contains(&c.id) && !self.offline.contains_key(&c.id))
            .map(|c| c.size)
            .sum();
        ui.horizontal_wrapped(|ui| {
            let can = !running && !chosen.is_empty() && self.licenses_ok;
            if ui
                .add_enabled(
                    can,
                    egui::Button::new(
                        RichText::new(t!("tools.install_selected", size = human_bytes(total)))
                            .strong(),
                    ),
                )
                .on_disabled_hover_text(t!("tools.install_disabled"))
                .clicked()
            {
                self.start(paths, &chosen, ui.ctx());
            }
            if ui
                .add_enabled(!running, egui::Button::new(t!("tools.offline_files")))
                .on_hover_text(t!("tools.offline_tip"))
                .clicked()
                && let Some(files) = rfd::FileDialog::new()
                    .add_filter("zip", &["zip"])
                    .pick_files()
            {
                let unknown = self.add_offline(files);
                if !unknown.is_empty() {
                    toasts.push(t!("tools.offline_unknown", names = unknown.join(", ")));
                }
            }
            if ui
                .add_enabled(!running, egui::Button::new(t!("tools.offline_folder")))
                .clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                let found = find_archives_in(&dir, &self.components);
                if found.is_empty() {
                    toasts.push(t!("tools.offline_none"));
                }
                for (id, p) in found {
                    self.offline.insert(id.clone(), p);
                    self.selected.insert(id, true);
                }
            }
            if !self.offline.is_empty()
                && ui
                    .add_enabled(!running, egui::Button::new(t!("tools.offline_clear")))
                    .clicked()
            {
                self.offline.clear();
            }
            if running {
                if ui.button(t!("common.cancel")).clicked()
                    && let Some(i) = &self.installer
                {
                    i.cancel.cancel();
                }
                ui.spinner();
            }
            let failed: Vec<String> = state
                .as_ref()
                .map(|s| {
                    s.status
                        .iter()
                        .filter(|(_, st)| matches!(st, CompStatus::Failed(_) | CompStatus::Cancelled))
                        .map(|(k, _)| k.clone())
                        .collect()
                })
                .unwrap_or_default();
            if !running && !failed.is_empty() && ui.button(t!("tools.retry_failed")).clicked() {
                self.start(paths, &failed, ui.ctx());
            }
        });
        if let Some(s) = &state
            && s.finished
        {
            let failed = s
                .status
                .values()
                .any(|st| matches!(st, CompStatus::Failed(_)));
            if failed {
                ui.colored_label(ERR, t!("tools.some_failed"));
            } else {
                ui.colored_label(OK, t!("tools.all_done"));
            }
        }
    }

    /// Refresh presence after an install run.
    fn refresh_presence(&mut self, paths: &AppPaths) {
        let tools = ToolSet::discover(paths);
        let models = ModelSet::discover(&paths.models);
        self.installed = InstalledTools::load(&paths.tools_state_file);
        let old = outdated(&self.components, &self.installed);
        for c in &self.components {
            let have = is_present(&c.id, &tools, &models);
            self.present.insert(c.id.clone(), have);
            // Keep only what still needs installing selected (failed items stay selected).
            self.selected
                .insert(c.id.clone(), !have || old.contains(&c.id));
        }
        self.offline.clear();
    }
}

fn status_cell(
    ui: &mut egui::Ui,
    st: Option<CompStatus>,
    have: bool,
    installed: Option<&String>,
    version: &str,
) {
    match st {
        Some(CompStatus::Waiting) => {
            ui.label(t!("tools.st_waiting"));
        }
        Some(CompStatus::Downloading(p)) => {
            ui.vertical(|ui| {
                let frac = p.total.map(|t| p.done as f32 / t.max(1) as f32).unwrap_or(0.0);
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(200.0)
                        .text(format!(
                            "{} / {}",
                            human_bytes(p.done),
                            p.total.map(human_bytes).unwrap_or_else(|| "?".into())
                        )),
                );
                ui.label(
                    RichText::new(t!("tools.speed", speed = human_bytes(p.speed as u64)))
                        .small()
                        .weak(),
                );
            });
        }
        Some(CompStatus::Verifying) => {
            ui.label(t!("tools.st_verifying"));
        }
        Some(CompStatus::Extracting) => {
            ui.label(t!("tools.st_extracting"));
        }
        Some(CompStatus::Done) => {
            ui.colored_label(OK, t!("tools.st_done"));
        }
        Some(CompStatus::Skipped) => {
            ui.label(t!("tools.st_skipped"));
        }
        Some(CompStatus::Cancelled) => {
            ui.colored_label(WARN, t!("tools.st_cancelled"));
        }
        Some(CompStatus::Failed(e)) => {
            ui.colored_label(ERR, t!("tools.st_failed"))
                .on_hover_text(e.clone());
            let short: String = e.chars().take(90).collect();
            ui.label(RichText::new(short).small().color(ERR));
        }
        None => match (have, installed) {
            (true, Some(v)) if v != version => {
                ui.colored_label(WARN, t!("tools.st_update", old = v, new = version));
            }
            (true, _) => {
                ui.colored_label(OK, t!("tools.st_installed"));
            }
            (false, _) => {
                ui.colored_label(WARN, t!("tools.st_missing"));
            }
        },
    }
}

pub fn tools_window(app: &mut App, ctx: &egui::Context) {
    if !app.tools_ui.open {
        return;
    }
    let mut open = true;
    let mut toasts = Vec::new();
    let paths = app.paths.clone();
    egui::Window::new(t!("tools.title"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(820.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            app.tools_ui.body(ui, &paths, &mut toasts);
        });
    for t in toasts {
        app.toast(t);
    }
    if !open {
        app.tools_ui.open = false;
    }
}
