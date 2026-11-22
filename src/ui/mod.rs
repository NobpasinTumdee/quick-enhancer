//! UI panels. Each function draws one region of the window. All user-visible text goes
//! through `t!` (see `locales/*.toml`).

pub mod fonttest;
pub mod outputs;
pub mod perf;
pub mod profiles;
pub mod queue;
pub mod settings;
pub mod settings_page;
pub mod setup;
pub mod studio;
pub mod tools;
pub mod update;
pub mod widgets;

use egui::{Color32, RichText};

use crate::app::{App, Page};
use crate::config::{OverwritePolicy, Studio};
use crate::media::MediaKind;
use crate::preview::canvas::CompareMode;
use crate::t;
use crate::util::{human_bytes, human_duration};

pub fn top_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("top").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Quick Enhancer");
            ui.separator();
            let ctx = ui.ctx().clone();
            for s in Studio::ALL {
                let on = app.page == Page::of(s);
                if ui.selectable_label(on, RichText::new(s.label()).strong()).clicked() {
                    app.switch_studio(s);
                }
            }
            for (p, label) in [
                (Page::Outputs, t!("tabs.outputs")),
                (Page::Settings, t!("tabs.settings")),
            ] {
                if ui.selectable_value(&mut app.page, p, RichText::new(label).strong()).clicked()
                    && p == Page::Outputs
                {
                    app.refresh_outputs(&ctx);
                }
            }
            ui.separator();
            if app.page.studio().is_some() {
                queue_controls(app, ui);
                ui.separator();
            }
            let missing = app.setup.missing().count();
            let txt = if missing == 0 {
                RichText::new(t!("top.setup_ok"))
            } else {
                RichText::new(t!("top.setup_missing", n = missing))
                    .color(Color32::from_rgb(230, 170, 60))
            };
            if ui.button(txt).clicked() {
                if missing == 0 {
                    app.show_setup = true;
                } else {
                    app.tools_ui.open(&app.paths);
                }
            }
            ui.checkbox(&mut app.config.show_log, t!("top.log"));
        });
    });
}

fn queue_controls(app: &mut App, ui: &mut egui::Ui) {
    if ui.button(t!("top.add_files")).clicked() {
        let (exts, filter): (Vec<&str>, String) = match app.studio {
            Studio::All => (
                crate::media::IMAGE_EXTS.iter().chain(crate::media::VIDEO_EXTS.iter()).copied().collect(),
                t!("top.filter_media"),
            ),
            Studio::Video => (crate::media::VIDEO_EXTS.to_vec(), t!("top.filter_videos")),
            Studio::Image => (crate::media::IMAGE_EXTS.to_vec(), t!("top.filter_images")),
        };
        if let Some(files) = rfd::FileDialog::new()
            .add_filter(filter, &exts)
            .add_filter(t!("top.filter_all"), &["*"])
            .pick_files()
        {
            app.add_paths(files);
        }
    }
    if ui.button(t!("top.add_folder")).clicked()
        && let Some(dir) = rfd::FileDialog::new().pick_folder()
    {
        app.add_paths(vec![dir]);
    }
    ui.checkbox(&mut app.config.recursive, t!("top.recursive"));
    ui.separator();
    let busy = app.runner.is_busy();
    let pending = app
        .ws
        .items
        .iter()
        .filter(|i| i.status == crate::app::ItemStatus::Pending)
        .count();
    if ui
        .add_enabled(
            pending > 0,
            egui::Button::new(RichText::new(t!("top.start")).strong()),
        )
        .on_disabled_hover_text(t!("top.start_disabled"))
        .clicked()
    {
        app.start_queue();
    }
    if ui
        .add_enabled(busy, egui::Button::new(t!("top.cancel_current")))
        .clicked()
    {
        app.runner.cancel_current();
    }
    if ui
        .add_enabled(busy, egui::Button::new(t!("top.cancel_all")))
        .clicked()
    {
        app.runner.cancel_all();
    }
    let paused = app.runner.pause.is_paused();
    if ui
        .add_enabled(
            busy,
            egui::Button::new(if paused { t!("top.resume") } else { t!("top.pause") }),
        )
        .on_hover_text(t!("top.pause_tip"))
        .clicked()
    {
        app.runner.pause.set(!paused);
    }
}

pub fn bottom_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::bottom("bottom")
        .resizable(true)
        .default_size(if app.config.show_log { 250.0 } else { 120.0 })
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                widgets::monitor_strip(app, ui);
            });
            ui.separator();
            progress_strip(app, ui);
            if app.config.show_log {
                ui.separator();
                log_view(app, ui);
            }
        });
}

fn progress_strip(app: &mut App, ui: &mut egui::Ui) {
    let p = app.runner.progress.get();
    if p.job.is_none() {
        let done = app
            .ws
            .items
            .iter()
            .filter(|i| i.status == crate::app::ItemStatus::Done)
            .count();
        ui.label(
            RichText::new(t!("progress.idle", n = app.ws.items.len(), done = done)).weak(),
        );
        return;
    }
    let overall = if app.queue_total > 0 {
        (app.queue_done as f32 + p.fraction()) / app.queue_total as f32
    } else {
        p.fraction()
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(t!(
            "progress.queue",
            i = (app.queue_done + 1).min(app.queue_total.max(1)),
            n = app.queue_total.max(1)
        ));
        ui.add(
            egui::ProgressBar::new(overall)
                .desired_width(160.0)
                .show_percentage(),
        );
        ui.separator();
        if let Some(origin) = p.job.and_then(|id| app.item(id)).map(|i| i.origin) {
            ui.label(RichText::new(format!("[{}]", origin.label())).weak());
        }
        ui.add(egui::Label::new(RichText::new(&p.file).strong()).truncate());
        ui.add(
            egui::ProgressBar::new(p.fraction())
                .desired_width(220.0)
                .text(t!(
                    "progress.frames",
                    pct = format!("{:.0}", p.fraction() * 100.0),
                    done = p.frames_done,
                    total = p.frames_total
                )),
        );
        if app.runner.pause.is_paused() {
            ui.label(RichText::new(t!("progress.paused")).color(Color32::YELLOW));
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(t!("progress.stage", stage = p.stage.label()));
        ui.separator();
        ui.label(format!("{:.1} FPS", p.fps));
        ui.separator();
        ui.label(t!(
            "progress.eta",
            eta = p.eta_secs.map(human_duration).unwrap_or_else(|| "--".into())
        ));
        ui.separator();
        ui.label(t!("progress.device", device = &p.device));
        if !p.encoder.is_empty() {
            ui.separator();
            ui.label(t!("progress.encoder", encoder = &p.encoder));
        }
        ui.separator();
        ui.label(t!(
            "progress.temp",
            used = human_bytes(p.temp_bytes),
            peak = human_bytes(p.temp_peak)
        ));
        if p.extracted > 0 {
            ui.separator();
            ui.label(t!("progress.decoded", n = p.extracted));
        }
    });
    if !p.detail.is_empty() {
        ui.label(RichText::new(&p.detail).small().weak());
    }
}

fn log_view(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(t!("log.title")).strong());
        if ui.small_button(t!("log.clear")).clicked() {
            app.ui_log.clear();
        }
        if ui.small_button(t!("log.open_folder")).clicked() {
            open_path(&app.paths.logs);
        }
    });
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for l in app.ui_log.snapshot() {
                let color = match l.level {
                    tracing::Level::ERROR => Color32::from_rgb(240, 100, 100),
                    tracing::Level::WARN => Color32::from_rgb(230, 180, 80),
                    _ => ui.visuals().text_color(),
                };
                ui.label(
                    RichText::new(format!("{} {}", l.time, l.text))
                        .monospace()
                        .size(11.5)
                        .color(color),
                );
            }
        });
}

/// Open a file (default app) or folder (Explorer).
pub fn open_path(p: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
}

pub fn preview_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        let Some(item) = app.selected_item() else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(t!("preview.drop_here")).size(18.0).weak());
            });
            return;
        };
        let name = item.name();
        let info = item.info.clone();
        let has_output = item.status == crate::app::ItemStatus::Done
            && item.output.as_ref().is_some_and(|o| o.exists());
        let warn_sdr = info.as_ref().is_some_and(|i| i.needs_sdr_warning());
        let warn_vfr = info.as_ref().is_some_and(|i| i.vfr);

        // Toolbar.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&name).strong());
            ui.separator();
            ui.selectable_value(&mut app.ws.canvas.mode, CompareMode::Wipe, t!("preview.wipe"));
            ui.selectable_value(&mut app.ws.canvas.mode, CompareMode::SideBySide, t!("preview.side_by_side"));
            if ui
                .selectable_label(app.ws.canvas.mode == CompareMode::Toggle, t!("preview.toggle"))
                .clicked()
            {
                app.ws.canvas.toggle();
            }
            ui.separator();
            if ui.button(t!("preview.fit")).clicked() {
                app.ws.canvas.fit();
            }
            if ui.button("100%").clicked() {
                app.ws.canvas.actual_size();
            }
            ui.label(format!("{:.0}%", app.ws.canvas.zoom() * 100.0));
            ui.separator();
            if ui.button(t!("preview.render")).clicked() {
                app.request_still_preview(true);
            }
            ui.checkbox(&mut app.config.auto_preview, t!("preview.auto"));
            if has_output && ui.button(t!("preview.compare_export")).clicked() {
                app.request_compare_output();
            }
            if app.preview.is_busy() {
                ui.spinner();
                let pp = app.preview.progress.get();
                if pp.frames_total > 0 {
                    ui.label(format!("{} {}/{}", pp.stage.label(), pp.frames_done, pp.frames_total));
                } else {
                    ui.label(t!("preview.rendering"));
                }
                if ui.button(t!("common.cancel")).clicked() {
                    app.preview.cancel();
                }
            }
        });

        // Video scrubber + sample.
        if let Some(info) = info.as_ref().filter(|i| i.kind == MediaKind::Video) {
            let dur = info.duration.unwrap_or(0.0).max(0.0);
            let fps = info.fps.map(|f| f.as_f64()).unwrap_or(30.0);
            ui.horizontal(|ui| {
                let mut t = app.ws.scrub_time.min(dur);
                ui.spacing_mut().slider_width = (ui.available_width() - 460.0).max(120.0);
                let r = ui.add(egui::Slider::new(&mut t, 0.0..=dur.max(0.01)).show_value(false));
                if r.changed() {
                    app.ws.scrub_time = t;
                }
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    app.request_still_preview(false);
                }
                ui.monospace(t!(
                    "preview.position",
                    time = human_duration(app.ws.scrub_time),
                    frame = (app.ws.scrub_time * fps).round() as u64
                ));
                ui.add(
                    egui::DragValue::new(&mut app.config.sample_seconds)
                        .range(0.5..=20.0)
                        .speed(0.1)
                        .suffix(t!("common.seconds_suffix")),
                );
                if ui
                    .button(t!("preview.sample"))
                    .on_hover_text(t!("preview.sample_tip"))
                    .clicked()
                {
                    app.request_sample();
                }
                if app.ws.player.is_some() && ui.button(t!("preview.close_sample")).clicked() {
                    app.ws.player = None;
                }
            });
        }
        if warn_sdr {
            ui.colored_label(Color32::from_rgb(230, 170, 60), t!("preview.warn_hdr"));
        }
        if warn_vfr {
            ui.colored_label(Color32::from_rgb(230, 170, 60), t!("preview.warn_vfr"));
        }
        if let Some(e) = &app.ws.preview_error {
            ui.colored_label(Color32::from_rgb(240, 100, 100), t!("preview.failed", error = e));
        } else if !app.ws.preview_note.is_empty() && app.ws.player.is_none() {
            ui.label(
                RichText::new(format!("{} · {:.2}s", app.ws.preview_note, app.ws.preview_secs))
                    .small()
                    .weak(),
            );
        }
        if let Some(p) = app.ws.player.as_mut() {
            let c = &mut app.config;
            p.ui(
                ui,
                crate::preview::player::PlayerPrefs {
                    volume: &mut c.preview_volume,
                    muted: &mut c.preview_muted,
                    looping: &mut c.preview_loop,
                },
            );
        } else {
            app.ws.canvas.ui(ui);
        }
    });
}

pub fn overwrite_window(app: &mut App, ctx: &egui::Context) {
    let Some(p) = app.overwrite_prompt.as_ref() else {
        return;
    };
    let n = p.conflicts.len();
    let list: Vec<String> = p
        .conflicts
        .iter()
        .take(8)
        .map(|c| c.display().to_string())
        .collect();
    let mut choice = None;
    let mut close = false;
    egui::Modal::new(egui::Id::unique("overwrite")).show(ctx, |ui| {
        ui.heading(t!("overwrite.title"));
        ui.label(t!("overwrite.body", n = n));
        for l in &list {
            ui.monospace(l);
        }
        if n > list.len() {
            ui.label("…");
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(t!("overwrite.overwrite")).clicked() {
                choice = Some(OverwritePolicy::Overwrite);
            }
            if ui.button(t!("overwrite.skip")).clicked() {
                choice = Some(OverwritePolicy::Skip);
            }
            if ui.button(t!("overwrite.rename")).clicked() {
                choice = Some(OverwritePolicy::AutoRename);
            }
            if ui.button(t!("common.cancel")).clicked() {
                close = true;
            }
        });
    });
    if choice.is_some() || close {
        app.resolve_overwrite(choice);
    }
}

pub fn toasts(app: &mut App, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::unique("toasts"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 48.0))
        .interactable(false)
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            for (_, msg) in &app.toasts {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(msg);
                });
            }
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

pub fn drop_hint(ctx: &egui::Context) {
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    if !hovering {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("drop"),
    ));
    painter.rect_filled(screen, 0.0, Color32::from_black_alpha(160));
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        t!("preview.drop_to_add"),
        egui::FontId::proportional(28.0),
        Color32::WHITE,
    );
}
