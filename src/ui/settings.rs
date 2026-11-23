//! Right panel: all processing settings (the "current settings" applied to new queue items)
//! plus application settings (language, updates, tools) and About / Paths.

use egui::{Color32, RichText};

use crate::app::App;
use crate::config::{
    ComputeDevice, Container, EncoderChoice, Studio, FrameGenMode, ImageFormat, OverwritePolicy, Preset,
    Priority, Profile, ScaleMode, SharpenMode, TileSize,
};
use crate::engines::{choose_backend, profile_unavailable};
use crate::i18n::Lang;
use crate::media::MediaKind;
use crate::media::encoders::VideoEncoder;
use crate::paths::PathMode;
use crate::pipeline::plan::{effective_resources, plan_fps, plan_scale, resolve_tile, valid_jobs};
use crate::pipeline::temp::{estimate_temp_bytes, free_space, temp_root};
use crate::t;
use crate::util::human_bytes;

const WARN: Color32 = Color32::from_rgb(230, 170, 60);

pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).small().weak());
}

pub fn settings_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::right("settings").resizable(true).default_size(370.0).min_size(300.0).show(ui, |ui| {
        ui.heading(t!("settings.title"));
        hint(ui, t!("settings.intro"));
        crate::ui::profiles::profile_bar(app, ui);
        ui.separator();
        let before = (app.cur_settings().clone(), app.config.machine.clone());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            switched(app, ui, "sec-upscale", t!("settings.upscaling"), t!("settings.upscaling_off"), |s| &mut s.upscale_on, enhancement);
            switched(app, ui, "sec-framegen", t!("settings.framegen"), t!("settings.framegen_off"), |s| &mut s.framegen_on, framegen);
            switched(app, ui, "sec-filters", t!("settings.filters"), t!("settings.filters_off"), |s| &mut s.filters_on, filters);
            device(app, ui);
            output(app, ui);
            video(app, ui);
            ui.add_space(6.0);
            hint(ui, t!("settings.app_moved"));
        });
        after_edit(app, before);
    });
}

/// React to settings edits: re-bind the GPU monitor, schedule a preview refresh.
pub fn after_edit(app: &mut App, before: (crate::config::JobSettings, crate::config::MachineSettings)) {
    if app.config.machine.compute != before.1.compute {
        app.sync_monitor_gpu();
    }
    let after = app.config.job(app.cur_settings());
    let before = before.0.with_machine(&before.1).effective();
    if app.config.auto_preview && crate::app::still_key(&after) != crate::app::still_key(&before) {
        app.mark_preview_dirty();
    }
}

/// Collapsible settings section whose header is an on/off checkbox. When off, the body
/// stays visible (greyed out) so the values are kept for when it is switched back on.
pub fn switch_section(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    off_note: &str,
    on: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id(id);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.checkbox(on, RichText::new(title).strong())
                .on_hover_text(t!("settings.untick_tip"));
        })
        .body(|ui| {
            if !*on {
                ui.colored_label(WARN, t!("settings.off_note", note = off_note));
            }
            ui.add_enabled_ui(*on, body);
        });
}

/// Run a section body with the switch stored in `get(&mut settings)`.
pub fn switched(
    app: &mut App,
    ui: &mut egui::Ui,
    id: &str,
    title: String,
    off_note: String,
    get: fn(&mut crate::config::JobSettings) -> &mut bool,
    body: fn(&mut App, &mut egui::Ui),
) {
    let st = app.studio;
    let mut on = *get(app.config.settings_for_mut(st));
    switch_section(ui, id, &title, &off_note, &mut on, |ui| body(app, ui));
    *get(app.config.settings_for_mut(st)) = on;
}

pub fn selected_kind(app: &App) -> MediaKind {
    app.selected_item()
        .and_then(|i| i.kind())
        .unwrap_or(MediaKind::Image)
}

pub fn enhancement(app: &mut App, ui: &mut egui::Ui) {
    let kind = selected_kind(app);
    let env = app.env.clone();
    let sel_info = app.selected_item().and_then(|i| i.info.clone());
    let cpu = crate::devices::resolve(app.config.machine.compute, &env.gpus).is_none();
    let s = app.config.settings_for_mut(app.studio);
    let mut pick = crate::ui::models::ComboPick::None;
    egui::Grid::new("enh").num_columns(2).show(ui, |ui| {
        ui.label(t!("settings.profile"));
        egui::ComboBox::from_id_salt("profile")
            .selected_text(s.profile.label())
            .width(220.0)
            .show_ui(ui, |ui| {
                for p in Profile::ALL {
                    let why = profile_unavailable(p, s.model.as_deref(), kind, &env.tools, &env.models);
                    let r = ui.add_enabled_ui(why.is_none(), |ui| {
                        ui.selectable_value(&mut s.profile, p, p.label())
                    });
                    if let Some(w) = why {
                        r.response.on_disabled_hover_text(w);
                    }
                }
            });
        ui.end_row();
        if crate::engines::uses_model(s.profile) {
            ui.label(t!("settings.model"));
            pick = crate::ui::models::upscale_model_combo(ui, "upmodel", &app.models_ui, &env.models, &mut s.model);
            ui.end_row();
        }
        ui.label(t!("settings.restore"));
        ui.horizontal_wrapped(|ui| {
            let p = restore_picker(ui, "restoremodel", &app.models_ui, &env.models, s);
            if !matches!(p, crate::ui::models::ComboPick::None) {
                pick = p;
            }
        });
        ui.end_row();
        ui.label(t!("settings.faces"));
        ui.horizontal_wrapped(|ui| {
            let p = face_picker(ui, "facemodel", &app.models_ui, s);
            if !matches!(p, crate::ui::models::ComboPick::None) {
                pick = p;
            }
        });
        ui.end_row();
        ui.label(t!("settings.scale"));
        egui::ComboBox::from_id_salt("scale")
            .selected_text(s.scale.label())
            .width(220.0)
            .show_ui(ui, |ui| {
                for m in ScaleMode::ALL {
                    ui.selectable_value(&mut s.scale, m, m.label());
                }
            });
        ui.end_row();
    });
    let model_text = match s.profile {
        Profile::Realistic => t!("settings.model_realistic"),
        Profile::Anime if kind == MediaKind::Video => t!("settings.model_anime_video"),
        Profile::Anime => t!("settings.model_anime_image"),
        Profile::PixelArt => t!("settings.model_pixel"),
        Profile::General => t!("settings.model_general"),
        Profile::NoAi => t!("settings.model_none"),
    };
    if s.model.is_none() || !crate::engines::uses_model(s.profile) {
        hint(ui, model_text);
    }
    if let Some(why) = profile_unavailable(s.profile, s.model.as_deref(), kind, &env.tools, &env.models) {
        ui.colored_label(WARN, format!("⚠ {why}"));
    }
    if let Some(info) = sel_info.filter(|_| s.upscale_on)
        && let Ok(b) = choose_backend(s.profile, s.model.as_deref(), info.kind, &env.tools, &env.models, cpu)
    {
        {
            let p = plan_scale(
                info.width,
                info.height,
                s.scale,
                &b.backend.native_scales(),
                info.kind == MediaKind::Video,
            );
            ui.label(format!(
                "{}x{} → {}x{}  ·  {}",
                info.width, info.height, p.out_w, p.out_h, p.note
            ));
            if let Some(w) = b.warning {
                ui.colored_label(WARN, format!("⚠ {w}"));
            }
        }
    }
    crate::ui::models::handle_pick(app, pick);
}

/// "AI cleanup" checkbox + 1x restore model picker (settings grid and photo studio).
pub fn restore_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    mu: &crate::ui::models::ModelsUi,
    models: &crate::paths::ModelSet,
    s: &mut crate::config::JobSettings,
) -> crate::ui::models::ComboPick {
    ui.checkbox(&mut s.restore_on, "").on_hover_text(t!("settings.restore_hint"));
    ui.add_enabled_ui(s.restore_on, |ui| {
        let extra: Vec<(String, String)> = models
            .custom
            .iter()
            .filter(|c| c.kind == crate::models::CustomKind::Onnx && c.scale == Some(1))
            .map(|c| (c.id.clone(), format!("{} ({})", c.name, t!("settings.model_custom"))))
            .collect();
        crate::ui::models::model_combo(ui, id_salt, mu, crate::models::Group::Restore, &extra, &mut s.restore_model)
    })
    .inner
}

/// "Face restoration" checkbox, face model picker and strength slider.
pub fn face_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    mu: &crate::ui::models::ModelsUi,
    s: &mut crate::config::JobSettings,
) -> crate::ui::models::ComboPick {
    ui.checkbox(&mut s.face_on, "").on_hover_text(t!("settings.faces_hint"));
    ui.add_enabled_ui(s.face_on, |ui| {
        let p = crate::ui::models::model_combo(ui, id_salt, mu, crate::models::Group::Face, &[], &mut s.face_model);
        ui.add(
            egui::Slider::new(&mut s.face_strength, 0.0..=1.0)
                .text(t!("settings.face_strength"))
                .fixed_decimals(2),
        );
        p
    })
    .inner
}

pub fn framegen(app: &mut App, ui: &mut egui::Ui) {
    let env = app.env.clone();
    let info = app.selected_item().and_then(|i| i.info.clone());
    let adv = crate::ui::studio::advanced(app);
    let s = app.config.settings_for_mut(app.studio);
    let rife_ok = env.tools.rife.is_some() && !env.models.rife.is_empty();
    if !rife_ok {
        ui.colored_label(WARN, t!("settings.rife_needed"));
    }
    let model = env.models.rife_model(&s.rife_model).cloned();
    let arbitrary = model.as_ref().is_some_and(|m| m.arbitrary_count);
    let mut pick = crate::ui::models::ComboPick::None;
    ui.add_enabled_ui(rife_ok, |ui| {
        egui::Grid::new("fg").num_columns(2).show(ui, |ui| {
            ui.label(t!("settings.mode"));
            let is_target = matches!(s.framegen, FrameGenMode::TargetFps(_));
            egui::ComboBox::from_id_salt("fgmode")
                .selected_text(if is_target { t!("framegen.target") } else { s.framegen.label() })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.framegen, FrameGenMode::Off, t!("common.off"));
                    ui.selectable_value(&mut s.framegen, FrameGenMode::X2, "2x");
                    for (m, l) in [(FrameGenMode::X3, "3x"), (FrameGenMode::X4, "4x")] {
                        ui.add_enabled_ui(arbitrary, |ui| ui.selectable_value(&mut s.framegen, m, l))
                            .response
                            .on_disabled_hover_text(t!("settings.rife_2x_only"));
                    }
                    if ui.selectable_label(is_target, t!("framegen.target")).clicked() && !is_target {
                        s.framegen = FrameGenMode::TargetFps(60);
                    }
                });
            ui.end_row();
            if let FrameGenMode::TargetFps(t) = &mut s.framegen {
                ui.label(t!("settings.target"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(t).range(24..=480).suffix(" FPS"));
                    for v in [60, 120] {
                        if ui.small_button(v.to_string()).clicked() {
                            *t = v;
                        }
                    }
                });
                ui.end_row();
            }
            ui.label(t!("settings.max_fps"));
            ui.add(egui::DragValue::new(&mut s.max_fps).range(24..=480).suffix(" FPS"));
            ui.end_row();
            if !adv {
                return;
            }
            ui.label(t!("settings.rife_model"));
            // Installed RIFE folders that the catalog doesn't list (older installs).
            let extra: Vec<(String, String)> = env
                .models
                .rife
                .iter()
                .map(|m| {
                    let label = if m.arbitrary_count {
                        m.name.clone()
                    } else {
                        t!("settings.rife_model_2x", name = &m.name)
                    };
                    (m.name.clone(), label)
                })
                .collect();
            if let Some(m) = &model
                && s.rife_model != m.name
                && !env.models.rife.iter().any(|r| r.name == s.rife_model)
            {
                // The configured model is gone: show what will actually be used.
                s.rife_model = m.name.clone();
            }
            pick = crate::ui::models::model_combo(
                ui,
                "rifemodel",
                &app.models_ui,
                crate::models::Group::Interpolate,
                &extra,
                &mut s.rife_model,
            );
            ui.end_row();
            ui.label(t!("settings.scene_cuts"));
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut s.scene_detect, t!("settings.detect"));
                ui.add_enabled(
                    s.scene_detect,
                    egui::Slider::new(&mut s.scene_threshold, 0.05..=0.9).text(t!("settings.threshold")),
                );
            });
            ui.end_row();
        });
    });
    if let Some(info) = info.filter(|i| i.kind == MediaKind::Video) {
        if let Some(fps) = info.fps.filter(|_| s.framegen_on) {
            let p = plan_fps(fps, s.framegen, s.max_fps);
            ui.label(format!("{fps} FPS → {} FPS ({}x)", p.out_fps, p.multiplier));
            if let Some(n) = p.note {
                hint(ui, n);
            }
            if info.vfr && s.framegen != FrameGenMode::Off {
                ui.colored_label(WARN, t!("settings.vfr_note"));
            }
        }
    } else {
        hint(ui, t!("settings.videos_only"));
    }
    hint(ui, t!("settings.framegen_hint"));
    crate::ui::models::handle_pick(app, pick);
}

pub fn filters(app: &mut App, ui: &mut egui::Ui) {
    let s = app.config.settings_for_mut(app.studio);
    ui.add(egui::Slider::new(&mut s.denoise, 0.0..=1.0).text(t!("settings.denoise")));
    ui.horizontal(|ui| {
        ui.label(t!("settings.sharpen"));
        ui.selectable_value(&mut s.sharpen, SharpenMode::Off, t!("common.off"));
        ui.selectable_value(&mut s.sharpen, SharpenMode::Cas, "CAS");
        ui.selectable_value(&mut s.sharpen, SharpenMode::Unsharp, t!("settings.unsharp"));
    });
    match s.sharpen {
        SharpenMode::Cas => {
            ui.add(egui::Slider::new(&mut s.cas_strength, 0.0..=1.0).text(t!("settings.cas_strength")));
        }
        SharpenMode::Unsharp => {
            ui.add(egui::Slider::new(&mut s.unsharp_radius, 0.3..=5.0).text(t!("settings.radius")));
            ui.add(egui::Slider::new(&mut s.unsharp_amount, 0.0..=3.0).text(t!("settings.amount")));
            ui.add(egui::Slider::new(&mut s.unsharp_threshold, 0..=64).text(t!("settings.threshold")));
        }
        SharpenMode::Off => {}
    }
    hint(ui, t!("settings.filters_hint"));
}

pub fn device(app: &mut App, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(RichText::new(t!("settings.device")).strong())
        .id_salt("sec-device")
        .default_open(true)
        .show(ui, |ui| {
        let env = app.env.clone();
        let free_vram = env.free_vram();
        let s = &mut app.config.machine;
        let auto_name = crate::devices::auto_select(&env.gpus).map(|g| g.name.clone()).unwrap_or_else(|| "CPU".into());
        let auto_label = t!("settings.auto_device", name = &auto_name);
        let current = match s.compute {
            ComputeDevice::Auto => auto_label.clone(),
            ComputeDevice::Gpu(i) => env
                .gpus
                .iter()
                .find(|g| g.index == i)
                .map(|g| g.label())
                .unwrap_or_else(|| t!("settings.gpu_missing", i = i)),
            ComputeDevice::Cpu => "CPU".into(),
        };
        egui::Grid::new("dev").num_columns(2).show(ui, |ui| {
            ui.label(t!("settings.compute"));
            egui::ComboBox::from_id_salt("compute").selected_text(current).width(240.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.compute, ComputeDevice::Auto, auto_label.clone());
                for g in &env.gpus {
                    ui.selectable_value(&mut s.compute, ComputeDevice::Gpu(g.index), format!("{} · {:?}", g.label(), g.kind));
                }
                ui.selectable_value(&mut s.compute, ComputeDevice::Cpu, "CPU");
            });
            ui.end_row();
            if s.compute == ComputeDevice::Cpu {
                ui.label(t!("settings.cpu_threads"));
                let preset = t!("settings.preset_word");
                ui.add(
                    egui::DragValue::new(&mut s.cpu_threads)
                        .range(0..=env.cores as u32)
                        .custom_formatter(move |v, _| if v == 0.0 { preset.clone() } else { format!("{v}") }),
                );
                ui.end_row();
            }
            ui.label(t!("settings.preset"));
            ui.horizontal(|ui| {
                for p in Preset::ALL {
                    ui.selectable_value(&mut s.preset, p, p.label());
                }
            });
            ui.end_row();
        });
        if s.compute == ComputeDevice::Cpu {
            ui.colored_label(WARN, t!("settings.cpu_warning"));
        }
        if let Some(g) = crate::devices::resolve(s.compute, &env.gpus) {
            hint(
                ui,
                t!(
                    "settings.gpu_info",
                    vendor = g.vendor(),
                    name = &g.name,
                    fp16 = if g.fp16 { t!("settings.supported") } else { t!("settings.not_supported") }
                ),
            );
        } else if env.gpus.is_empty() {
            ui.colored_label(WARN, t!("settings.no_vulkan"));
        }

        let r = effective_resources(&crate::config::JobSettings::default().with_machine(s), env.cores);
        let tile = resolve_tile(&r, free_vram);
        ui.label(t!(
            "settings.effective",
            tile = tile,
            jobs = &r.jobs,
            threads = r.filter_threads,
            priority = r.priority.label()
        ));
        egui::CollapsingHeader::new(t!("settings.overrides")).id_salt("overrides").show(ui, |ui| {
            let mut tile_ov = s.tile_override.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut tile_ov, t!("settings.tile_size")).changed() {
                    s.tile_override = tile_ov.then_some(TileSize::Auto);
                }
                if let Some(t) = &mut s.tile_override {
                    let mut fixed = matches!(t, TileSize::Fixed(_));
                    if ui.radio(!fixed, t!("settings.tile_auto")).clicked() {
                        fixed = false;
                        *t = TileSize::Auto;
                    }
                    if ui.radio(fixed, t!("settings.tile_fixed")).clicked() && !fixed {
                        *t = TileSize::Fixed(256);
                    }
                    if let TileSize::Fixed(v) = t {
                        ui.add(egui::DragValue::new(v).range(32..=2048).speed(8));
                    }
                }
            });
            let mut jobs_ov = s.jobs_override.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut jobs_ov, "ncnn -j").changed() {
                    s.jobs_override = jobs_ov.then(|| r.jobs.clone());
                }
                if let Some(j) = &mut s.jobs_override {
                    ui.add(egui::TextEdit::singleline(j).desired_width(80.0));
                    if !valid_jobs(j) {
                        ui.colored_label(WARN, t!("settings.jobs_format"));
                    }
                }
            });
            let mut thr_ov = s.filter_threads_override.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut thr_ov, t!("settings.filter_threads")).changed() {
                    s.filter_threads_override = thr_ov.then_some(r.filter_threads as u32);
                }
                if let Some(t) = &mut s.filter_threads_override {
                    ui.add(egui::DragValue::new(t).range(1..=env.cores as u32));
                }
            });
            let mut pr_ov = s.priority_override.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut pr_ov, t!("settings.priority")).changed() {
                    s.priority_override = pr_ov.then_some(r.priority);
                }
                if let Some(p) = &mut s.priority_override {
                    for v in [Priority::BelowNormal, Priority::Normal, Priority::AboveNormal] {
                        ui.selectable_value(p, v, v.label());
                    }
                }
            });
            hint(ui, t!("settings.presets_hint"));
        });
    });
}

pub fn output(app: &mut App, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(RichText::new(t!("settings.output")).strong())
        .id_salt("sec-output")
        .default_open(true)
        .show(ui, |ui| {
        let env = app.env.clone();
        let st = app.studio;
        let s = &mut app.config.settings_for_mut(st).output;
        ui.horizontal(|ui| {
            ui.label(t!("settings.folder"));
            let txt = s.folder.as_ref().map(|f| f.display().to_string()).unwrap_or_else(|| t!("settings.next_to_source"));
            ui.label(RichText::new(txt).monospace());
        });
        ui.horizontal(|ui| {
            if ui.button(t!("settings.choose")).clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder() {
                    s.folder = Some(d);
                }
            if s.folder.is_some() && ui.button(t!("settings.use_next_to_source")).clicked() {
                s.folder = None;
            }
        });
        let policies = [
            (OverwritePolicy::Ask, t!("settings.ow_ask")),
            (OverwritePolicy::Skip, t!("settings.ow_skip")),
            (OverwritePolicy::AutoRename, t!("settings.ow_rename")),
            (OverwritePolicy::Overwrite, t!("settings.ow_overwrite")),
        ];
        egui::Grid::new("out").num_columns(2).show(ui, |ui| {
            ui.label(t!("settings.if_exists"));
            let cur = policies.iter().find(|(v, _)| *v == s.overwrite).map(|(_, l)| l.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt("overwrite").selected_text(cur).show_ui(ui, |ui| {
                for (v, l) in &policies {
                    ui.selectable_value(&mut s.overwrite, *v, l);
                }
            });
            ui.end_row();
            ui.label(t!("settings.suffix"));
            ui.add(egui::TextEdit::singleline(&mut s.suffix).desired_width(120.0).hint_text("_enhanced"))
                .on_hover_text(t!("settings.suffix_tip"));
            ui.end_row();
            if st != Studio::Video {
            ui.label(t!("settings.image_format"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.image_format, ImageFormat::Same, t!("settings.same"));
                for (v, l) in [(ImageFormat::Png, "PNG"), (ImageFormat::Jpeg, "JPG"), (ImageFormat::Webp, "WebP")] {
                    ui.selectable_value(&mut s.image_format, v, l);
                }
            });
            ui.end_row();
            ui.label(t!("settings.jpg_quality"));
            ui.add(egui::Slider::new(&mut s.image_quality, 50..=100));
            ui.end_row();
            }
            if st != Studio::Image {
            ui.label(t!("settings.container"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.container, Container::Mp4, "MP4");
                ui.selectable_value(&mut s.container, Container::Mkv, "MKV");
            });
            ui.end_row();
            ui.label(t!("settings.encoder"));
            let av = env.encoders.read().map(|a| a.clone()).unwrap_or_default();
            let auto_label = match av.auto() {
                Some(e) => t!("settings.auto_device", name = e.label()),
                None if !av.done => t!("settings.encoder_detecting"),
                None => t!("settings.encoder_none"),
            };
            let sel = match s.encoder {
                EncoderChoice::Auto => auto_label.clone(),
                EncoderChoice::Specific(e) => e.label().into(),
            };
            egui::ComboBox::from_id_salt("encoder").selected_text(sel).width(200.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.encoder, EncoderChoice::Auto, auto_label);
                for e in VideoEncoder::ALL {
                    let ok = av.usable(e);
                    let r = ui.add_enabled_ui(ok, |ui| ui.selectable_value(&mut s.encoder, EncoderChoice::Specific(e), e.label()));
                    if !ok {
                        r.response.on_disabled_hover_text(av.reason(e));
                    }
                }
            });
            ui.end_row();
            let enc = av.resolve(s.encoder).unwrap_or(VideoEncoder::Libx264);
            let (def, lo, hi) = enc.quality_range();
            ui.label(enc.quality_name());
            ui.horizontal(|ui| {
                let mut custom = s.quality.is_some();
                if ui.checkbox(&mut custom, t!("settings.custom")).changed() {
                    s.quality = custom.then_some(def);
                }
                match &mut s.quality {
                    Some(q) => {
                        *q = (*q).clamp(lo, hi);
                        ui.add(egui::DragValue::new(q).range(lo..=hi));
                    }
                    None => {
                        ui.label(RichText::new(t!("settings.default_value", v = def)).weak());
                    }
                }
            });
            ui.end_row();
            }
        });
        if s.image_format == ImageFormat::Webp && st != Studio::Video {
            hint(ui, t!("settings.webp_hint"));
        }
        if st != Studio::Image {
            ui.checkbox(&mut s.keep_partial, t!("settings.keep_partial"));
            hint(ui, t!("settings.audio_hint"));
        }
    });
}

pub fn video(app: &mut App, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(RichText::new(t!("settings.video_pipeline")).strong())
        .id_salt("sec-video")
        .default_open(false)
        .show(ui, |ui| {
        let info = app.selected_item().and_then(|i| i.info.clone());
        let env = app.env.clone();
        let st = app.config.job(app.cur_settings());
        let s = &mut app.config.machine;
        egui::Grid::new("vid").num_columns(2).show(ui, |ui| {
            ui.label(t!("settings.chunk_size"));
            ui.add(egui::DragValue::new(&mut s.chunk_frames).range(16..=2000).suffix(t!("settings.frames_suffix")));
            ui.end_row();
            ui.label(t!("settings.ram_budget"));
            ui.add(egui::DragValue::new(&mut s.ram_budget_mb).range(256..=65536).speed(64).suffix(" MB"));
            ui.end_row();
            ui.label(t!("settings.temp_folder"));
            ui.label(RichText::new(temp_root(s.temp_dir.as_deref()).display().to_string()).monospace().small());
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button(t!("settings.choose_temp")).clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder() {
                    s.temp_dir = Some(d);
                }
            if s.temp_dir.is_some() && ui.button(t!("settings.system_temp")).clicked() {
                s.temp_dir = None;
            }
        });
        let root = temp_root(s.temp_dir.as_deref());
        let free = free_space(&root);
        if let Some(info) = info.filter(|i| i.kind == MediaKind::Video) {
            let cpu = crate::devices::resolve(st.compute, &env.gpus).is_none();
            if let Ok(b) = choose_backend(st.profile, st.model.as_deref(), info.kind, &env.tools, &env.models, cpu) {
                let p = plan_scale(info.width, info.height, st.scale, &b.backend.native_scales(), true);
                let k = info.fps.map(|f| plan_fps(f, st.framegen, st.max_fps).multiplier).unwrap_or(1);
                let ai = b.backend.is_ai() && p.model_scale.is_some();
                let (mw, mh) = p.model_scale.filter(|_| ai).map(|m| (info.width * m, info.height * m)).unwrap_or((info.width, info.height));
                if ai || k > 1 {
                    let cap = estimate_temp_bytes(info.width, info.height, mw, mh, s.chunk_frames, k, ai);
                    let line = t!("settings.temp_cap", size = human_bytes(cap));
                    match free {
                        Some(f) if f < cap + (512 << 20) => {
                            ui.colored_label(WARN, t!("settings.temp_low", line = &line, free = human_bytes(f)));
                        }
                        _ => {
                            ui.label(line);
                        }
                    }
                } else {
                    ui.label(t!("settings.in_memory"));
                }
            }
        }
        if let Some(f) = free {
            hint(ui, t!("settings.free_space", size = human_bytes(f)));
        }
        hint(ui, t!("settings.chunks_hint"));
    });
}

pub fn application(app: &mut App, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(RichText::new(t!("settings.application")).strong())
        .id_salt("sec-app")
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("app-grid").num_columns(2).show(ui, |ui| {
                ui.label(t!("settings.language"));
                let cur = crate::i18n::lang();
                egui::ComboBox::from_id_salt("lang")
                    .selected_text(cur.native_name())
                    .show_ui(ui, |ui| {
                        for l in [Lang::Th, Lang::En] {
                            if ui.selectable_label(cur == l, l.native_name()).clicked() {
                                crate::i18n::set_lang(l);
                                app.config.language = Some(l);
                            }
                        }
                    });
                ui.end_row();
            });
            ui.horizontal_wrapped(|ui| {
                let ctx = ui.ctx().clone();
                if ui.button(t!("settings.check_updates")).clicked() {
                    app.update_ui.start_check(&ctx, false);
                }
                ui.checkbox(&mut app.config.auto_update_check, t!("update.auto_check"))
                    .on_hover_text(t!("update.privacy"));
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button(t!("settings.manage_tools")).clicked() {
                    app.tools_ui.open(&app.paths);
                }
                if ui.button(t!("settings.setup_check")).clicked() {
                    app.show_setup = true;
                }
                if ui.button(t!("fonttest.title")).clicked() {
                    app.show_font_test = true;
                }
            });
        });
}

fn path_row(ui: &mut egui::Ui, label: String, p: &std::path::Path) {
    ui.label(label);
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(p.display().to_string()).monospace().small()).truncate())
            .on_hover_text(p.display().to_string());
        if ui.small_button(t!("common.open_folder")).clicked() {
            let _ = std::fs::create_dir_all(p);
            super::open_path(p);
        }
    });
    ui.end_row();
}

pub fn about(app: &mut App, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(RichText::new(t!("settings.about")).strong())
        .id_salt("sec-about")
        .default_open(false)
        .show(ui, |ui| {
            ui.label(RichText::new(format!("Quick Enhancer {}", crate::net::app_version())).strong());
            let mode = match app.paths.mode {
                PathMode::Installed => t!("settings.mode_installed"),
                PathMode::Portable => t!("settings.mode_portable"),
            };
            ui.label(t!("settings.data_mode", mode = mode));
            let p = app.paths.clone();
            egui::Grid::new("paths-grid").num_columns(2).show(ui, |ui| {
                path_row(ui, t!("settings.path_data"), &p.data);
                path_row(ui, t!("settings.path_tools"), &p.bin);
                path_row(ui, t!("settings.path_models"), &p.models);
                path_row(ui, t!("settings.path_logs"), &p.logs);
                path_row(ui, t!("settings.path_thumbs"), &p.thumbs);
                path_row(ui, t!("settings.path_program"), &p.exe_dir);
            });
            hint(ui, t!("settings.about_licenses"));
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to(t!("settings.releases_page"), crate::net::releases_page());
                let lic = p.exe_dir.join("licenses");
                if lic.is_dir() && ui.small_button(t!("settings.open_licenses")).clicked() {
                    super::open_path(&lic);
                }
            });
        });
}
