//! Right-hand settings panels of the Video and Image pages. They edit the page's own
//! `JobSettings` (`Config::settings_for(app.studio)`) and reuse the All-in-one section
//! widgets where they fit; processing is the same engine as everywhere else.

use egui::{Color32, RichText};

use super::settings::{after_edit, enhancement, framegen, hint, output, switched};
use crate::app::App;
use crate::config::{Profile, ScaleMode, SharpenMode, Studio};
use crate::engines::{choose_backend, profile_unavailable};
use crate::media::MediaKind;
use crate::pipeline::plan::{plan_fps, plan_scale};
use crate::t;

const WARN: Color32 = Color32::from_rgb(230, 170, 60);

/// Page header: title, tagline and the Simple / Advanced switch.
fn header(app: &mut App, ui: &mut egui::Ui, title: String, tagline: String) {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let adv = &mut app.config.pages.get_mut(app.studio).advanced;
            ui.selectable_value(adv, true, t!("studio.advanced"))
                .on_hover_text(t!("studio.advanced_tip"));
            ui.selectable_value(adv, false, t!("studio.simple"));
        });
    });
    hint(ui, tagline);
}

pub fn advanced(app: &App) -> bool {
    app.studio == Studio::All || app.config.pages.get(app.studio).advanced
}

/// One-line summary of what the selected input becomes, e.g.
/// `1920×1080 · 30 fps → 3840×2160 · 60 fps · NVENC HEVC`.
pub fn summary_line(app: &App) -> Option<String> {
    let info = app.selected_item()?.info.clone()?;
    let s = app.config.job(app.cur_settings());
    let env = &app.env;
    if s.sharp_on && info.kind == MediaKind::Image {
        let (w, h) = crate::sharp::plan::target_size(info.width, info.height, s.sharp.scale);
        let preset = match (s.sharp.preset, app.sharp_ui.report(app.selected_item()?.id)) {
            (crate::sharp::plan::Preset::Auto, Some(r)) => crate::sharp::plan::Preset::for_report(r),
            (p, _) => p,
        };
        return Some(format!("{}×{} → {w}×{h} · {} · {}", info.width, info.height, t!("sharp.mode"), t!(preset.label_key())));
    }
    let cpu = crate::devices::resolve(s.compute, &env.gpus).is_none();
    let b = choose_backend(s.profile, s.model.as_deref(), info.kind, &env.tools, &env.models, cpu).ok()?;
    let video = info.kind == MediaKind::Video;
    let p = plan_scale(info.width, info.height, s.scale, &b.backend.native_scales(), video);
    let mut parts = Vec::new();
    if video {
        let src_fps = info.fps.unwrap_or(crate::media::Rational::new(30, 1));
        let f = plan_fps(src_fps, s.framegen, s.max_fps);
        parts.push(format!(
            "{}×{} · {} fps → {}×{} · {} fps",
            info.width, info.height, src_fps, p.out_w, p.out_h, f.out_fps
        ));
        let enc = env
            .encoders
            .read()
            .ok()
            .and_then(|a| a.resolve(s.output.encoder))
            .map(|e| e.label().to_string())
            .unwrap_or_else(|| "—".into());
        parts.push(enc);
    } else {
        parts.push(format!("{}×{} → {}×{}", info.width, info.height, p.out_w, p.out_h));
        parts.push(b.backend.label());
    }
    for extra in crate::ui::perf::summary_extras(app, &info) {
        parts.push(extra);
    }
    Some(parts.join(" · "))
}

fn summary(app: &App, ui: &mut egui::Ui) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        match summary_line(app) {
            Some(l) => {
                ui.add(egui::Label::new(RichText::new(l).strong()).wrap());
            }
            None => hint(ui, t!("studio.summary_none")),
        }
    });
}

fn denoise(app: &mut App, ui: &mut egui::Ui) {
    let s = app.config.settings_for_mut(app.studio);
    ui.add(egui::Slider::new(&mut s.denoise, 0.0..=1.0).text(t!("settings.denoise")));
    hint(ui, t!("studio.denoise_hint"));
}

fn video_sharpen(app: &mut App, ui: &mut egui::Ui) {
    let adv = advanced(app);
    let s = app.config.settings_for_mut(app.studio);
    if adv {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.sharpen, SharpenMode::Off, t!("common.off"));
            ui.selectable_value(&mut s.sharpen, SharpenMode::Cas, "CAS");
            ui.selectable_value(&mut s.sharpen, SharpenMode::Unsharp, t!("settings.unsharp"));
        });
    } else if s.sharpen == SharpenMode::Off {
        s.sharpen = SharpenMode::Cas;
    }
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
    if adv {
        ui.add(egui::Slider::new(&mut s.clarity, 0.0..=1.0).text(t!("studio.clarity")))
            .on_hover_text(t!("studio.clarity_tip"));
    }
}

pub fn video_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::right("video-settings")
        .resizable(true)
        .default_size(380.0)
        .min_size(300.0)
        .show(ui, |ui| {
            header(app, ui, t!("studio.video_title"), t!("studio.video_tagline"));
            crate::ui::profiles::profile_bar(app, ui);
            summary(app, ui);
            ui.separator();
            let before = (app.cur_settings().clone(), app.config.machine.clone());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                switched(app, ui, "v-denoise", t!("studio.sec_denoise"), t!("studio.denoise_off"), |s| &mut s.denoise_on, denoise);
                switched(app, ui, "v-framegen", t!("settings.framegen"), t!("settings.framegen_off"), |s| &mut s.framegen_on, framegen);
                switched(app, ui, "v-upscale", t!("studio.sec_upscale"), t!("settings.upscaling_off"), |s| &mut s.upscale_on, enhancement);
                switched(app, ui, "v-sharpen", t!("studio.sec_sharpen"), t!("studio.sharpen_off"), |s| &mut s.sharpen_on, video_sharpen);
                output(app, ui);
                crate::ui::perf::page_resources(app, ui);
            });
            after_edit(app, before);
        });
}

/// Image page profiles (No AI is not offered here: use the All-in-one page for that).
const IMAGE_PROFILES: [Profile; 4] = [Profile::Realistic, Profile::Anime, Profile::PixelArt, Profile::General];

fn image_profile_label(p: Profile) -> String {
    match p {
        Profile::Realistic => t!("studio.profile_photo"),
        other => other.label(),
    }
}

fn image_mode(app: &mut App, ui: &mut egui::Ui) {
    let env = app.env.clone();
    let info = app.selected_item().and_then(|i| i.info.clone());
    let cpu = crate::devices::resolve(app.config.machine.compute, &env.gpus).is_none();
    let s = app.config.settings_for_mut(app.studio);
    s.upscale_on = true;
    if s.profile == Profile::NoAi {
        s.profile = Profile::Realistic;
    }
    let sharp = s.sharp_on;
    let restore = s.scale == ScaleMode::X1 && !sharp;
    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(!restore && !sharp, RichText::new(t!("studio.mode_upscale")).strong())
            .clicked()
        {
            s.sharp_on = false;
            if s.scale == ScaleMode::X1 {
                s.scale = ScaleMode::X2;
            }
        }
        if ui
            .selectable_label(restore, RichText::new(t!("studio.mode_restore")).strong())
            .on_hover_text(t!("studio.mode_restore_tip"))
            .clicked()
        {
            s.sharp_on = false;
            s.scale = ScaleMode::X1;
        }
        if ui
            .selectable_label(sharp, RichText::new(t!("sharp.mode")).strong())
            .on_hover_text(t!("sharp.mode_tip"))
            .clicked()
        {
            s.sharp_on = true;
        }
    });
    if s.sharp_on {
        crate::ui::sharp::sharp_mode(app, ui);
        return;
    }
    if s.scale == ScaleMode::X1 {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(t!("studio.restore_explain")).color(Color32::from_rgb(140, 200, 255)));
        });
    } else {
        ui.horizontal(|ui| {
            ui.label(t!("settings.scale"));
            egui::ComboBox::from_id_salt("img-scale")
                .selected_text(s.scale.label())
                .show_ui(ui, |ui| {
                    for m in ScaleMode::ALL.into_iter().filter(|m| *m != ScaleMode::X1) {
                        ui.selectable_value(&mut s.scale, m, m.label());
                    }
                });
        });
    }
    ui.horizontal(|ui| {
        ui.label(t!("settings.profile"));
        egui::ComboBox::from_id_salt("img-profile")
            .selected_text(image_profile_label(s.profile))
            .width(200.0)
            .show_ui(ui, |ui| {
                for p in IMAGE_PROFILES {
                    let why = profile_unavailable(p, s.model.as_deref(), MediaKind::Image, &env.tools, &env.models);
                    let r = ui.add_enabled_ui(why.is_none(), |ui| {
                        ui.selectable_value(&mut s.profile, p, image_profile_label(p))
                    });
                    if let Some(w) = why {
                        r.response.on_disabled_hover_text(w);
                    }
                }
            });
    });
    let mut pick = crate::ui::models::ComboPick::None;
    if crate::engines::uses_model(s.profile) {
        ui.horizontal(|ui| {
            ui.label(t!("settings.model"));
            pick = crate::ui::models::upscale_model_combo(ui, "img-model", &app.models_ui, &env.models, &mut s.model);
        });
    }
    ui.horizontal(|ui| {
        ui.label(t!("settings.restore"));
        let p = crate::ui::settings::restore_picker(ui, "img-restore", &app.models_ui, &env.models, s);
        if !matches!(p, crate::ui::models::ComboPick::None) {
            pick = p;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(t!("settings.faces"));
        let p = crate::ui::settings::face_picker(ui, "img-face", &app.models_ui, s);
        if !matches!(p, crate::ui::models::ComboPick::None) {
            pick = p;
        }
    });
    if let Some(why) = profile_unavailable(s.profile, s.model.as_deref(), MediaKind::Image, &env.tools, &env.models) {
        ui.colored_label(WARN, format!("⚠ {why}"));
    }
    if let Some(info) = info
        && let Ok(b) = choose_backend(s.profile, s.model.as_deref(), MediaKind::Image, &env.tools, &env.models, cpu)
    {
        let p = plan_scale(info.width, info.height, s.scale, &b.backend.native_scales(), false);
        hint(ui, format!("{}x{} → {}x{} · {}", info.width, info.height, p.out_w, p.out_h, p.note));
    }
    let missing = crate::engines::missing_models(s, MediaKind::Image, &env.tools, &env.models);
    let p = crate::ui::models::missing_row(ui, &app.models_ui, &missing);
    if !matches!(p, crate::ui::models::ComboPick::None) {
        pick = p;
    }
    crate::ui::models::handle_pick(app, pick);
}

fn image_clarity(app: &mut App, ui: &mut egui::Ui) {
    let adv = advanced(app);
    let s = app.config.settings_for_mut(app.studio);
    s.filters_on = true;
    ui.horizontal(|ui| {
        ui.label(t!("settings.sharpen"));
        ui.selectable_value(&mut s.sharpen, SharpenMode::Off, t!("common.off"));
        ui.selectable_value(&mut s.sharpen, SharpenMode::Cas, "CAS");
        if adv {
            ui.selectable_value(&mut s.sharpen, SharpenMode::Unsharp, t!("settings.unsharp"));
        } else if s.sharpen == SharpenMode::Unsharp {
            s.sharpen = SharpenMode::Cas;
        }
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
    ui.add(egui::Slider::new(&mut s.clarity, 0.0..=1.0).text(t!("studio.clarity")))
        .on_hover_text(t!("studio.clarity_tip"));
    ui.add(egui::Slider::new(&mut s.denoise, 0.0..=1.0).text(t!("studio.light_denoise")))
        .on_hover_text(t!("studio.light_denoise_tip"));
    hint(ui, t!("studio.clarity_hint"));
}

pub fn image_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::right("image-settings")
        .resizable(true)
        .default_size(380.0)
        .min_size(300.0)
        .show(ui, |ui| {
            header(app, ui, t!("studio.image_title"), t!("studio.image_tagline"));
            crate::ui::profiles::profile_bar(app, ui);
            summary(app, ui);
            ui.separator();
            let before = (app.cur_settings().clone(), app.config.machine.clone());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::CollapsingHeader::new(RichText::new(t!("studio.sec_mode")).strong())
                    .id_salt("i-mode")
                    .default_open(true)
                    .show(ui, |ui| image_mode(app, ui));
                // "Make sharp" has its own light sharpen step.
                if !app.cur_settings().sharp_on {
                    egui::CollapsingHeader::new(RichText::new(t!("studio.sec_clarity")).strong())
                        .id_salt("i-clarity")
                        .default_open(true)
                        .show(ui, |ui| image_clarity(app, ui));
                }
                output(app, ui);
                crate::ui::perf::page_resources(app, ui);
            });
            after_edit(app, before);
        });
}
