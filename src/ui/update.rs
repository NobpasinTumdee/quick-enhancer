//! "Check for updates" window and the one-time opt-in question. Network access happens
//! only from here (on request or with the opt-in startup check).

use egui::{Color32, RichText};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::app::App;
use crate::net::tools::ProgressSnap;
use crate::net::update::{self, CheckResult, ReleaseManifest};
use crate::pipeline::cancel::CancelToken;
use crate::t;
use crate::util::human_bytes;

const OK: Color32 = Color32::from_rgb(110, 200, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERR: Color32 = Color32::from_rgb(235, 100, 100);

#[derive(Clone, Debug, Default)]
pub enum UpdState {
    #[default]
    Idle,
    Checking,
    UpToDate(String),
    Available(ReleaseManifest),
    ManualRequired(ReleaseManifest),
    Downloading(ReleaseManifest, ProgressSnap),
    Ready(ReleaseManifest, PathBuf),
    Failed(String),
}

/// Deferred UI action (run after the window closure releases its borrows).
type Action = Box<dyn FnOnce(&mut App)>;

#[derive(Default)]
pub struct UpdateUi {
    pub open: bool,
    state: Arc<Mutex<UpdState>>,
    cancel: CancelToken,
    /// Background startup check: only open the window if an update exists.
    silent: bool,
}

impl UpdateUi {
    fn get(&self) -> UpdState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set(state: &Arc<Mutex<UpdState>>, s: UpdState, ctx: &egui::Context) {
        if let Ok(mut g) = state.lock() {
            *g = s;
        }
        ctx.request_repaint();
    }

    pub fn busy(&self) -> bool {
        matches!(self.get(), UpdState::Checking | UpdState::Downloading(..))
    }

    pub fn start_check(&mut self, ctx: &egui::Context, silent: bool) {
        if self.busy() {
            return;
        }
        self.silent = silent;
        if !silent {
            self.open = true;
        }
        let state = self.state.clone();
        Self::set(&state, UpdState::Checking, ctx);
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let s = match update::check() {
                    Ok(CheckResult::UpToDate { latest }) => UpdState::UpToDate(latest),
                    Ok(CheckResult::Available(m)) => UpdState::Available(m),
                    Ok(CheckResult::ManualRequired(m)) => UpdState::ManualRequired(m),
                    Err(e) => {
                        tracing::warn!("update check failed: {e}");
                        UpdState::Failed(e.to_string())
                    }
                };
                Self::set(&state, s, &ctx);
            });
    }

    fn start_download(&mut self, m: ReleaseManifest, ctx: &egui::Context) {
        self.cancel = CancelToken::new();
        let cancel = self.cancel.clone();
        let state = self.state.clone();
        Self::set(&state, UpdState::Downloading(m.clone(), ProgressSnap::default()), ctx);
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("update-download".into())
            .spawn(move || {
                let r = update::download_installer(&m, &cancel, &mut |p| {
                    Self::set(&state, UpdState::Downloading(m.clone(), p.into()), &ctx)
                });
                let s = match r {
                    Ok(path) => UpdState::Ready(m, path),
                    Err(e) => {
                        tracing::error!("update download failed: {e}");
                        UpdState::Failed(e.to_string())
                    }
                };
                Self::set(&state, s, &ctx);
            });
    }
}

fn notes(ui: &mut egui::Ui, m: &ReleaseManifest) {
    let n = m.notes(crate::i18n::lang());
    if !n.trim().is_empty() {
        ui.label(RichText::new(t!("update.notes")).strong());
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| ui.label(n));
    }
    if !m.tools_changes.is_empty() {
        ui.label(
            RichText::new(t!("update.tools_changes", list = m.tools_changes.join(", ")))
                .small()
                .weak(),
        );
    }
}

fn releases_link(ui: &mut egui::Ui) {
    ui.hyperlink_to(t!("update.open_releases"), crate::net::releases_page());
}

pub fn update_window(app: &mut App, ctx: &egui::Context) {
    let state = app.update_ui.get();
    if app.update_ui.silent {
        match &state {
            UpdState::Available(_) | UpdState::ManualRequired(_) => {
                app.update_ui.silent = false;
                app.update_ui.open = true;
            }
            UpdState::UpToDate(_) | UpdState::Failed(_) => {
                // Quiet background check: nothing to show.
                app.update_ui.silent = false;
                return;
            }
            _ => {}
        }
    }
    if !app.update_ui.open {
        return;
    }
    let job_running = app.runner.is_busy();
    let mut open = true;
    let mut action: Option<Action> = None;
    egui::Window::new(t!("update.title"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(t!("update.current", version = crate::net::app_version()));
            ui.separator();
            match &state {
                UpdState::Idle => {
                    if ui.button(t!("update.check_now")).clicked() {
                        action = Some(Box::new(|a: &mut App| {
                            let ctx = a.egui_ctx.clone();
                            a.update_ui.start_check(&ctx, false)
                        }));
                    }
                }
                UpdState::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("update.checking"));
                    });
                }
                UpdState::UpToDate(latest) => {
                    ui.colored_label(OK, t!("update.up_to_date", latest = latest));
                    if ui.button(t!("update.check_again")).clicked() {
                        action = Some(Box::new(|a: &mut App| {
                            let ctx = a.egui_ctx.clone();
                            a.update_ui.start_check(&ctx, false)
                        }));
                    }
                }
                UpdState::Available(m) => {
                    ui.label(
                        RichText::new(t!("update.available", version = &m.version))
                            .strong()
                            .size(16.0),
                    );
                    if let Some(sz) = m.installer_size {
                        ui.label(RichText::new(human_bytes(sz)).small().weak());
                    }
                    notes(ui, m);
                    ui.label(RichText::new(t!("update.keeps_data")).small().weak());
                    if job_running {
                        ui.colored_label(WARN, t!("update.refuse_running"));
                    }
                    let m2 = m.clone();
                    if ui
                        .add_enabled(
                            !job_running,
                            egui::Button::new(RichText::new(t!("update.install")).strong()),
                        )
                        .clicked()
                    {
                        action = Some(Box::new(move |a: &mut App| {
                            let ctx = a.egui_ctx.clone();
                            a.update_ui.start_download(m2, &ctx)
                        }));
                    }
                }
                UpdState::ManualRequired(m) => {
                    ui.colored_label(
                        WARN,
                        t!(
                            "update.manual_required",
                            version = &m.version,
                            min = m.min_supported_version.clone().unwrap_or_default()
                        ),
                    );
                    notes(ui, m);
                    releases_link(ui);
                }
                UpdState::Downloading(m, p) => {
                    ui.label(t!("update.downloading", version = &m.version));
                    let frac = p.total.map(|t| p.done as f32 / t.max(1) as f32).unwrap_or(0.0);
                    ui.add(egui::ProgressBar::new(frac).show_percentage());
                    ui.label(
                        RichText::new(t!("tools.speed", speed = human_bytes(p.speed as u64)))
                            .small()
                            .weak(),
                    );
                    if ui.button(t!("common.cancel")).clicked() {
                        app.update_ui.cancel.cancel();
                    }
                }
                UpdState::Ready(m, path) => {
                    if job_running {
                        ui.colored_label(WARN, t!("update.refuse_running"));
                    } else {
                        ui.label(t!("update.ready", version = &m.version));
                    }
                    let path = path.clone();
                    if ui
                        .add_enabled(
                            !job_running,
                            egui::Button::new(RichText::new(t!("update.restart_now")).strong()),
                        )
                        .clicked()
                    {
                        action = Some(Box::new(move |a: &mut App| a.launch_update(&path)));
                    }
                }
                UpdState::Failed(e) => {
                    ui.colored_label(ERR, t!("update.failed"));
                    ui.label(RichText::new(e).small().monospace());
                    releases_link(ui);
                    if ui.button(t!("update.check_again")).clicked() {
                        action = Some(Box::new(|a: &mut App| {
                            let ctx = a.egui_ctx.clone();
                            a.update_ui.start_check(&ctx, false)
                        }));
                    }
                }
            }
            ui.separator();
            ui.checkbox(&mut app.config.auto_update_check, t!("update.auto_check"));
            ui.label(RichText::new(t!("update.privacy")).small().weak());
        });
    if let Some(a) = action {
        a(app);
    }
    if !open {
        app.update_ui.open = false;
    }
}

impl App {

    /// Run the verified installer silently and close the app (the installer relaunches it).
    pub fn launch_update(&mut self, installer: &std::path::Path) {
        if self.runner.is_busy() {
            self.toast(t!("update.refuse_running"));
            return;
        }
        if let Err(e) = self.config.save(&self.paths.config_file) {
            tracing::warn!("saving config failed: {e}");
        }
        match update::launch_installer(installer) {
            Ok(()) => {
                tracing::info!("launched update installer {}", installer.display());
                self.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                tracing::error!("launching installer failed: {e}");
                if let Ok(mut g) = self.update_ui.state.lock() {
                    *g = UpdState::Failed(e.to_string());
                }
            }
        }
    }
}

/// Asked once on first run: check for updates automatically at startup?
pub fn optin_modal(app: &mut App, ctx: &egui::Context) {
    if !app.ask_update_optin || app.tools_ui.open {
        return;
    }
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("update-optin")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading(t!("update.optin_title"));
        ui.label(t!("update.optin_body"));
        ui.label(RichText::new(t!("update.privacy")).small().weak());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(t!("update.optin_yes")).clicked() {
                answer = Some(true);
            }
            if ui.button(t!("update.optin_no")).clicked() {
                answer = Some(false);
            }
        });
    });
    if let Some(yes) = answer {
        app.config.auto_update_check = yes;
        app.config.update_check_asked = true;
        app.ask_update_optin = false;
    }
}
