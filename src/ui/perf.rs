//! Resource estimates and performance controls: the "Resources" section of each page, the
//! Performance section of the Settings page (Simple / Advanced), benchmark + auto-tune, and
//! the pre-start check.

use egui::{Color32, RichText};

use crate::app::App;
use crate::config::{ComputeDevice, MachineSettings, Preset, Priority, TileSize};
use crate::media::MediaKind;
use crate::media::probe::MediaInfo;
use crate::pipeline::plan::{effective_resources, valid_jobs};
use crate::pipeline::temp::temp_root;
use crate::resources::{self, Estimate, EstimateInput, EstimateMachine, Issue, JobContext, Level};
use crate::t;
use crate::util::human_bytes;

const OK: Color32 = Color32::from_rgb(110, 200, 120);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERR: Color32 = Color32::from_rgb(235, 100, 100);

/// Pending pre-start check result (modal).
pub struct Preflight {
    pub issues: Vec<Issue>,
}

pub fn fmt_secs(s: f64) -> String {
    let s = s.round() as u64;
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min {:02} s", s / 60, s % 60),
        _ => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// PNG ratio to estimate with: measured on real frames > benchmark > default.
pub fn png_ratio(app: &App) -> f64 {
    crate::pipeline::stages::png_ratio()
        .or_else(|| bench(app).map(|b| b.png_ratio))
        .unwrap_or(resources::DEFAULT_PNG_RATIO)
}

pub fn bench(app: &App) -> Option<&resources::BenchResult> {
    let (key, _) = crate::bench::device_key(&app.env, app.config.machine.compute);
    app.bench.get(&key)
}

/// Estimate for `info` with the current page settings.
pub fn estimate_for(app: &App, info: &MediaInfo) -> Option<(EstimateInput, Estimate)> {
    let s = app.config.job(app.cur_settings());
    let i = resources::input_for(&app.env, &s, info)?;
    let m = EstimateMachine::of(&s, app.env.cores, app.sys().vram_free);
    let e = resources::estimate(&i, &m, png_ratio(app), bench(app));
    Some((i, e))
}

fn encoder_problem(app: &App, s: &crate::config::JobSettings, kind: MediaKind) -> Option<String> {
    if kind != MediaKind::Video {
        return None;
    }
    let av = app.env.encoders.read().ok()?.clone();
    if !av.done {
        return None;
    }
    match s.output.encoder {
        crate::config::EncoderChoice::Specific(e) if !av.usable(e) => Some(e.label().into()),
        crate::config::EncoderChoice::Auto if av.auto().is_none() => Some("Auto".into()),
        _ => None,
    }
}

/// All issues for `info` on the current page.
pub fn issues_for(app: &App, info: &MediaInfo) -> Option<(EstimateInput, Estimate, Vec<Issue>)> {
    let (i, e) = estimate_for(app, info)?;
    let s = app.config.job(app.cur_settings());
    let ctx = JobContext {
        encoder_unavailable: encoder_problem(app, &s, info.kind),
        output_dir_invalid: s.output.folder.clone().filter(|d| !d.is_dir() && std::fs::create_dir_all(d).is_err()),
    };
    let v = resources::validate(&i, &e, &app.config.machine, app.sys(), &ctx);
    Some((i, e, v))
}

/// Extra parts of a page's summary line (estimates).
pub fn summary_extras(app: &App, info: &MediaInfo) -> Vec<String> {
    let Some((_, e)) = estimate_for(app, info) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    if e.disk_mode && info.kind == MediaKind::Video {
        v.push(t!("perf.sum_temp", size = human_bytes(e.temp_total)));
    }
    v.push(match e.time_secs {
        Some(s) => t!("perf.sum_time", time = fmt_secs(s)),
        None => t!("perf.sum_time_unknown"),
    });
    v
}

fn issue_line(ui: &mut egui::Ui, i: &Issue) {
    let (c, icon) = match i.level {
        Level::Block => (ERR, "⛔"),
        Level::Warn => (WARN, "⚠"),
    };
    ui.add(egui::Label::new(RichText::new(format!("{icon} {}", i.message)).color(c)).wrap());
}

/// Apply "Fix automatically" for `issues` to the machine settings (+ encoder reset).
pub fn apply_fix(app: &mut App, i: &EstimateInput, issues: &[Issue]) {
    let ratio = png_ratio(app);
    let (m, reset_encoder) = resources::fix_job(i, &app.config.machine, app.sys(), issues, ratio);
    let before = app.config.machine.clone();
    app.config.machine = m;
    if reset_encoder {
        let st = app.studio;
        app.config.settings_for_mut(st).output.encoder = crate::config::EncoderChoice::Auto;
    }
    let changes = describe_changes(app, &before);
    app.toast(if changes.is_empty() { t!("perf.fixed_nothing") } else { t!("perf.fixed", what = changes.join(", ")) });
    app.refresh_sys();
}

fn describe_changes(app: &App, before: &MachineSettings) -> Vec<String> {
    let a = before;
    let b = &app.config.machine;
    let mut v = Vec::new();
    let tile = |m: &MachineSettings| match effective_resources(&crate::config::JobSettings::default().with_machine(m), app.env.cores).tile {
        TileSize::Fixed(t) => t.to_string(),
        TileSize::Auto => "Auto".into(),
    };
    if tile(a) != tile(b) {
        v.push(format!("{} {} → {}", t!("perf.tile_size"), tile(a), tile(b)));
    }
    if a.chunk_frames != b.chunk_frames {
        v.push(format!("{} {} → {}", t!("perf.chunk_size"), a.chunk_frames, b.chunk_frames));
    }
    if a.ram_budget_mb != b.ram_budget_mb {
        v.push(format!("{} {} → {} MB", t!("perf.ram_budget"), a.ram_budget_mb, b.ram_budget_mb));
    }
    if a.filter_threads_override != b.filter_threads_override || a.cpu_threads != b.cpu_threads {
        v.push(t!("perf.filter_threads"));
    }
    if a.jobs_override != b.jobs_override {
        v.push("ncnn -j".into());
    }
    if a.temp_dir != b.temp_dir {
        v.push(t!("perf.temp_folder"));
    }
    v
}

/// Estimate lines + issues for the selected file (shared by pages and Settings).
fn estimate_block(app: &mut App, ui: &mut egui::Ui) {
    let Some(info) = app.selected_item().and_then(|i| i.info.clone()) else {
        crate::ui::settings::hint(ui, t!("perf.select_file"));
        return;
    };
    let Some((i, e, issues)) = issues_for(app, &info) else {
        return;
    };
    let sys = app.sys().clone();
    let text_color = ui.visuals().text_color();
    let color_for = |used: u64, avail: Option<u64>, warn_share: f64| match avail {
        Some(a) if used > a => ERR,
        Some(a) if used as f64 > a as f64 * warn_share => WARN,
        Some(_) => OK,
        None => text_color,
    };
    egui::Grid::new(("est", app.studio.index())).num_columns(2).striped(true).show(ui, |ui| {
        ui.label(t!("perf.est_temp"));
        if e.disk_mode {
            let c = color_for(e.temp_total, sys.temp_free, 0.8);
            ui.colored_label(
                c,
                t!(
                    "perf.est_temp_val",
                    chunk = human_bytes(e.temp_per_chunk),
                    total = human_bytes(e.temp_total),
                    free = sys.temp_free.map(human_bytes).unwrap_or_else(|| "?".into())
                ),
            )
            .on_hover_text(t!("perf.est_temp_tip"));
        } else {
            ui.label(t!("perf.est_in_memory"));
        }
        ui.end_row();
        ui.label(t!("perf.est_ram"));
        ui.colored_label(
            color_for(e.ram, (sys.ram_available > 0).then_some(sys.ram_available), 0.8),
            t!("perf.est_ram_val", need = human_bytes(e.ram), avail = human_bytes(sys.ram_available), total = human_bytes(sys.ram_total)),
        );
        ui.end_row();
        if let Some(v) = e.vram {
            ui.label(t!("perf.est_vram"));
            let txt = match (sys.vram_free, sys.vram_total) {
                (Some(f), Some(t)) => t!("perf.est_vram_val", need = human_bytes(v), free = human_bytes(f), total = human_bytes(t)),
                _ => human_bytes(v),
            };
            ui.colored_label(color_for(v, sys.vram_free, 0.9), txt).on_hover_text(t!("perf.est_vram_tip"));
            ui.end_row();
        }
        ui.label(t!("perf.est_threads"));
        let c = if e.threads >= sys.cores { WARN } else { OK };
        ui.colored_label(c, t!("perf.est_threads_val", n = e.threads, cores = sys.cores));
        ui.end_row();
        ui.label(t!("perf.est_time"));
        match e.time_secs {
            Some(s) => ui.label(t!("perf.est_time_val", time = fmt_secs(s), frames = e.out_frames)),
            None => ui.label(RichText::new(t!("perf.est_time_unknown")).weak()),
        };
        ui.end_row();
    });
    if issues.is_empty() {
        ui.colored_label(OK, t!("perf.all_ok"));
    } else {
        for x in &issues {
            issue_line(ui, x);
        }
        if issues.iter().any(|x| x.fixable) && ui.button(t!("perf.fix_auto")).on_hover_text(t!("perf.fix_auto_tip")).clicked() {
            apply_fix(app, &i, &issues);
        }
    }
}

/// Resource section at the bottom of a page's settings.
pub fn page_resources(app: &mut App, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(RichText::new(t!("perf.resources")).strong())
        .id_salt(("page-res", app.studio.index()))
        .default_open(false)
        .show(ui, |ui| {
            estimate_block(app, ui);
            ui.separator();
            let before = app.cur_and_machine();
            simple_controls(app, ui);
            if crate::ui::studio::advanced(app) {
                advanced_controls(app, ui);
            }
            crate::ui::settings::after_edit(app, before);
        });
}

/// Preset + auto-tune + benchmark buttons.
fn simple_controls(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(t!("perf.preset"));
        for p in Preset::ALL {
            let tip = match p {
                Preset::Eco => t!("perf.preset_eco_tip"),
                Preset::Balanced => t!("perf.preset_balanced_tip"),
                Preset::Max => t!("perf.preset_max_tip"),
            };
            ui.selectable_value(&mut app.config.machine.preset, p, p.label()).on_hover_text(tip);
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button(t!("perf.auto_tune")).on_hover_text(t!("perf.auto_tune_tip")).clicked() {
            auto_tune(app);
        }
        let running = app.bench_run.is_some();
        if ui
            .add_enabled(!running && !app.runner.is_busy(), egui::Button::new(t!("perf.bench")))
            .on_hover_text(t!("perf.bench_tip"))
            .on_disabled_hover_text(t!("perf.bench_busy"))
            .clicked()
        {
            app.start_benchmark();
        }
    });
    bench_status(app, ui);
}

pub fn auto_tune(app: &mut App) {
    let typical = app
        .selected_item()
        .and_then(|i| i.info.clone())
        .filter(|i| i.kind == MediaKind::Video)
        .and_then(|info| estimate_for(app, &info).map(|x| x.0))
        .unwrap_or_default();
    let (m, changes) = resources::auto_tune(app.sys(), bench(app), &app.config.machine, &typical);
    app.config.machine = m;
    app.tune_report = Some(changes);
    app.refresh_sys();
}

fn bench_status(app: &mut App, ui: &mut egui::Ui) {
    if let Some(run) = &app.bench_run {
        let st = run.state();
        ui.horizontal(|ui| {
            ui.add(egui::ProgressBar::new(st.fraction).desired_width(180.0).show_percentage());
            ui.label(RichText::new(&st.step).small());
            if ui.small_button(t!("common.cancel")).clicked() {
                run.cancel.cancel();
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    } else if let Some(b) = bench(app) {
        let best = b.best_tile().map(|t| t.to_string()).unwrap_or_else(|| "—".into());
        crate::ui::settings::hint(
            ui,
            t!(
                "perf.bench_result",
                device = &b.device_name,
                tile = best,
                ms = format!("{:.0}", b.esrgan_ms_per_mp(b.best_tile().unwrap_or(200))),
                png = format!("{:.0}", b.png_write_mb_s),
                enc = b.encode_fps_720p.map(|f| format!("{f:.0}")).unwrap_or_else(|| "—".into())
            ),
        );
    } else {
        crate::ui::settings::hint(ui, t!("perf.bench_none"));
    }
    if let Some(changes) = &app.tune_report {
        let mut close = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            if changes.is_empty() {
                ui.label(t!("perf.tune_nothing"));
            } else {
                ui.label(RichText::new(t!("perf.tune_changed")).strong());
                for c in changes {
                    ui.label(format!("• {}: {} → {}", c.what, c.from, c.to));
                }
            }
            if bench(app).is_none() {
                crate::ui::settings::hint(ui, t!("perf.tune_no_bench"));
            }
            close = ui.small_button(t!("common.close")).clicked();
        });
        if close {
            app.tune_report = None;
        }
    }
}

fn row(ui: &mut egui::Ui, label: String, tip: String, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(label).on_hover_text(&tip);
    ui.horizontal(|ui| {
        add(ui);
    })
    .response
    .on_hover_text(tip);
    ui.end_row();
}

fn field_issue(ui: &mut egui::Ui, issues: &[Issue], f: resources::Field) {
    for i in issues.iter().filter(|i| i.field == f) {
        issue_line(ui, i);
    }
}

/// Every performance control with tooltips (machine settings, global).
fn advanced_controls(app: &mut App, ui: &mut egui::Ui) {
    let env = app.env.clone();
    let cores = env.cores as u32;
    let sys = app.sys().clone();
    let issues = resources::machine_issues(&sys, &app.config.machine);
    let st = app.studio;
    let s = &mut app.config.machine;
    let r = effective_resources(&crate::config::JobSettings::default().with_machine(s), env.cores);
    let auto = t!("perf.auto");
    egui::Grid::new(("perf-adv", st.index())).num_columns(2).show(ui, |ui| {
        // GPU device.
        let auto_name = crate::devices::auto_select(&env.gpus).map(|g| g.name.clone()).unwrap_or_else(|| "CPU".into());
        let auto_label = t!("settings.auto_device", name = &auto_name);
        let cur = match s.compute {
            ComputeDevice::Auto => auto_label.clone(),
            ComputeDevice::Gpu(i) => env.gpus.iter().find(|g| g.index == i).map(|g| g.label()).unwrap_or_else(|| t!("settings.gpu_missing", i = i)),
            ComputeDevice::Cpu => "CPU".into(),
        };
        row(ui, t!("perf.gpu_device"), t!("perf.gpu_device_tip"), |ui| {
            egui::ComboBox::from_id_salt(("perf-gpu", st.index())).selected_text(cur).width(220.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.compute, ComputeDevice::Auto, auto_label);
                for g in &env.gpus {
                    ui.selectable_value(&mut s.compute, ComputeDevice::Gpu(g.index), g.label());
                }
                ui.selectable_value(&mut s.compute, ComputeDevice::Cpu, "CPU");
            });
        });
        // Tile.
        let safe = resources::vram_safe_tile(sys.vram_free, true, 2);
        row(ui, t!("perf.tile_size"), t!("perf.tile_tip", safe = safe), |ui| {
            let mut fixed = matches!(s.tile_override, Some(TileSize::Fixed(_)));
            if ui.radio(!fixed, &auto).clicked() {
                fixed = false;
                s.tile_override = None;
            }
            if ui.radio(fixed, t!("settings.tile_fixed")).clicked() && !fixed {
                s.tile_override = Some(TileSize::Fixed(safe.min(256)));
            }
            if let Some(TileSize::Fixed(v)) = &mut s.tile_override {
                ui.add(egui::DragValue::new(v).range(32..=1024).speed(8));
                *v = (*v / 8 * 8).clamp(32, 1024);
            }
        });
        // ncnn -j.
        row(ui, "ncnn -j".into(), t!("perf.jobs_tip"), |ui| {
            let mut on = s.jobs_override.is_some();
            if ui.checkbox(&mut on, t!("perf.custom")).changed() {
                s.jobs_override = on.then(|| r.jobs.clone());
            }
            match &mut s.jobs_override {
                Some(j) => {
                    ui.add(egui::TextEdit::singleline(j).desired_width(70.0));
                    if !valid_jobs(j) {
                        ui.colored_label(WARN, t!("settings.jobs_format"));
                    }
                }
                None => {
                    ui.label(RichText::new(&r.jobs).weak());
                }
            }
        });
        // Filter threads.
        row(ui, t!("perf.filter_threads"), t!("perf.threads_tip", cores = cores), |ui| {
            let mut on = s.filter_threads_override.is_some();
            if ui.checkbox(&mut on, t!("perf.custom")).changed() {
                s.filter_threads_override = on.then_some(r.filter_threads as u32);
            }
            match &mut s.filter_threads_override {
                Some(t) => {
                    ui.add(egui::DragValue::new(t).range(1..=cores.max(1)));
                }
                None => {
                    ui.label(RichText::new(r.filter_threads.to_string()).weak());
                }
            }
        });
        // Priority.
        row(ui, t!("perf.priority"), t!("perf.priority_tip"), |ui| {
            let mut on = s.priority_override.is_some();
            if ui.checkbox(&mut on, t!("perf.custom")).changed() {
                s.priority_override = on.then_some(r.priority);
            }
            match &mut s.priority_override {
                Some(p) => {
                    for v in [Priority::BelowNormal, Priority::Normal, Priority::AboveNormal] {
                        ui.selectable_value(p, v, v.label());
                    }
                }
                None => {
                    ui.label(RichText::new(r.priority.label()).weak());
                }
            }
        });
        // Chunk / chunks on disk / RAM.
        row(ui, t!("perf.chunk_size"), t!("perf.chunk_tip"), |ui| {
            ui.add(egui::DragValue::new(&mut s.chunk_frames).range(16..=2000).speed(4).suffix(t!("settings.frames_suffix")));
        });
        row(ui, t!("perf.max_chunks"), t!("perf.max_chunks_tip"), |ui| {
            ui.label(RichText::new("2").weak());
        });
        let ram_max = ((sys.ram_total / resources::MB) as u32).max(1024);
        row(ui, t!("perf.ram_budget"), t!("perf.ram_tip"), |ui| {
            ui.add(egui::DragValue::new(&mut s.ram_budget_mb).range(256..=ram_max).speed(64).suffix(" MB"));
        });
        // Temp folder.
        row(ui, t!("perf.temp_folder"), t!("perf.temp_tip"), |ui| {
            ui.label(RichText::new(temp_root(s.temp_dir.as_deref()).display().to_string()).monospace().small());
        });
    });
    ui.horizontal(|ui| {
        if ui.button(t!("settings.choose_temp")).clicked()
            && let Some(d) = rfd::FileDialog::new().pick_folder()
        {
            app.config.machine.temp_dir = Some(d);
            app.refresh_sys();
        }
        if app.config.machine.temp_dir.is_some() && ui.button(t!("settings.system_temp")).clicked() {
            app.config.machine.temp_dir = None;
            app.refresh_sys();
        }
    });
    if let Some(f) = sys.temp_free {
        crate::ui::settings::hint(ui, t!("settings.free_space", size = human_bytes(f)));
    }
    for f in [resources::Field::Tile, resources::Field::Jobs, resources::Field::Threads, resources::Field::Ram, resources::Field::TempDir] {
        field_issue(ui, &issues, f);
    }
    encoder_controls(app, ui);
}

/// Encoder preset / rate mode (per page output settings).
fn encoder_controls(app: &mut App, ui: &mut egui::Ui) {
    let st = app.studio;
    if st == crate::config::Studio::Image {
        return;
    }
    let o = &mut app.config.settings_for_mut(st).output;
    egui::Grid::new(("perf-enc", st.index())).num_columns(2).show(ui, |ui| {
        row(ui, t!("perf.enc_preset"), t!("perf.enc_preset_tip"), |ui| {
            for v in [crate::config::EncoderSpeed::Fast, crate::config::EncoderSpeed::Balanced, crate::config::EncoderSpeed::Quality] {
                ui.selectable_value(&mut o.encoder_speed, v, v.label());
            }
        });
        row(ui, t!("perf.rate_mode"), t!("perf.rate_mode_tip"), |ui| {
            let mut br = o.bitrate_kbps.is_some();
            if ui.radio(!br, t!("perf.rate_quality")).clicked() {
                br = false;
                o.bitrate_kbps = None;
            }
            if ui.radio(br, t!("perf.rate_bitrate")).clicked() && !br {
                o.bitrate_kbps = Some(20000);
            }
            if let Some(b) = &mut o.bitrate_kbps {
                ui.add(egui::DragValue::new(b).range(500..=200_000).speed(100).suffix(" kbit/s"));
            }
        });
    });
}

/// Performance section of the Settings page.
pub fn performance_section(app: &mut App, ui: &mut egui::Ui) {
    let before = app.cur_and_machine();
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.config.perf_advanced, false, t!("studio.simple"));
        ui.selectable_value(&mut app.config.perf_advanced, true, t!("studio.advanced"));
    });
    crate::ui::settings::hint(ui, t!("perf.global_hint"));
    simple_controls(app, ui);
    if app.config.perf_advanced {
        ui.separator();
        advanced_controls(app, ui);
    } else {
        let issues = resources::machine_issues(app.sys(), &app.config.machine);
        for i in &issues {
            issue_line(ui, i);
        }
    }
    ui.separator();
    ui.label(RichText::new(t!("perf.selected_estimate")).strong());
    estimate_block(app, ui);
    crate::ui::settings::after_edit(app, before);
}

/// Problems an imported profile's performance values would cause on this PC.
pub fn machine_warnings(app: &App, m: &MachineSettings) -> Vec<String> {
    resources::machine_issues(app.sys(), m).into_iter().map(|i| i.message).collect()
}

/// `m` with every value this PC can't handle replaced by a safe one.
pub fn adjust_to_pc(app: &App, m: &MachineSettings) -> MachineSettings {
    resources::fix_machine(app.sys(), m)
}

/// Modal shown when the pre-start check finds problems.
pub fn preflight_window(app: &mut App, ctx: &egui::Context) {
    let Some(p) = app.preflight.take() else {
        return;
    };
    let blocking = p.issues.iter().any(|i| i.level == Level::Block);
    let fixable = p.issues.iter().any(|i| i.fixable);
    let mut keep = true;
    egui::Modal::new(egui::Id::unique("preflight")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        ui.heading(if blocking { t!("perf.pre_block_title") } else { t!("perf.pre_warn_title") });
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            for i in &p.issues {
                issue_line(ui, i);
            }
        });
        ui.horizontal(|ui| {
            if fixable && ui.button(t!("perf.fix_auto")).on_hover_text(t!("perf.fix_auto_tip")).clicked() {
                keep = false;
                app.fix_preflight(&p.issues);
            }
            if !blocking && ui.button(t!("perf.start_anyway")).clicked() {
                keep = false;
                app.start_queue_checked();
            }
            if ui.button(t!("common.cancel")).clicked() {
                keep = false;
            }
        });
    });
    if keep {
        app.preflight = Some(p);
    }
}

/// "Benchmark this PC?" once after the first tools install.
pub fn bench_offer_window(app: &mut App, ctx: &egui::Context) {
    if !app.bench_offer {
        return;
    }
    egui::Modal::new(egui::Id::unique("bench-offer")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading(t!("perf.offer_title"));
        ui.label(t!("perf.offer_body"));
        ui.horizontal(|ui| {
            if ui.button(t!("perf.offer_run")).clicked() {
                app.bench_offer = false;
                app.config.bench_offered = true;
                app.start_benchmark();
                app.page = crate::app::Page::Settings;
            }
            if ui.button(t!("perf.offer_later")).clicked() {
                app.bench_offer = false;
                app.config.bench_offered = true;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn seconds_format() {
        assert_eq!(super::fmt_secs(42.0), "42 s");
        assert_eq!(super::fmt_secs(125.0), "2 min 05 s");
        assert_eq!(super::fmt_secs(3725.0), "1 h 02 min");
    }
}
