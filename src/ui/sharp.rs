//! "Make sharp (AI)" on the Image page: what the blur analysis found, presets, the Faithful ↔
//! Creative slider, faces, output size, time / fit estimate, missing parts, the preview region
//! picker and the advanced per-stage settings.

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{Color32, RichText, Sense, Stroke};
use image::RgbImage;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::app::App;
use crate::media::probe::MediaInfo;
use crate::sharp::analysis::BlurReport;
use crate::sharp::plan::{self, Preset};
use crate::sharp::region;
use crate::t;

const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const GOOD: Color32 = Color32::from_rgb(110, 200, 120);
const INFO: Color32 = Color32::from_rgb(140, 200, 255);
const THUMB: u32 = 360;

/// Analysis of one queue item (computed off the UI thread).
pub struct Analysed {
    pub report: BlurReport,
    pub size: (u32, u32),
    pub thumb: RgbImage,
    /// Face boxes in source pixels.
    pub faces: Vec<[f32; 4]>,
}

pub struct SharpUi {
    done: HashMap<u64, Analysed>,
    tex: HashMap<u64, egui::TextureHandle>,
    pending: HashSet<u64>,
    tx: Sender<(u64, Result<Analysed, String>)>,
    rx: Receiver<(u64, Result<Analysed, String>)>,
    errors: HashMap<u64, String>,
    /// Drag start on the region picker (source pixels).
    drag: Option<(f32, f32)>,
}

impl Default for SharpUi {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        SharpUi {
            done: HashMap::new(),
            tex: HashMap::new(),
            pending: HashSet::new(),
            tx,
            rx,
            errors: HashMap::new(),
            drag: None,
        }
    }
}

impl SharpUi {
    /// Default preview box: a detected face, else the centre.
    pub fn default_region(&self, id: u64, info: &MediaInfo) -> [u32; 4] {
        let faces = self.done.get(&id).map(|a| a.faces.as_slice()).unwrap_or(&[]);
        region::default_region(info.width, info.height, faces)
    }

    pub fn report(&self, id: u64) -> Option<&BlurReport> {
        self.done.get(&id).map(|a| &a.report)
    }

    fn pump(&mut self, ctx: &egui::Context) {
        while let Ok((id, r)) = self.rx.try_recv() {
            self.pending.remove(&id);
            match r {
                Ok(a) => {
                    let tex = ctx.load_texture(
                        format!("sharp-thumb-{id}"),
                        crate::preview::to_color_image(&a.thumb),
                        egui::TextureOptions::LINEAR,
                    );
                    self.tex.insert(id, tex);
                    self.done.insert(id, a);
                }
                Err(e) => {
                    self.errors.insert(id, e);
                }
            }
        }
    }

    /// Start analysing `path` in the background (once per item).
    fn ensure(&mut self, id: u64, path: PathBuf, detector: Option<PathBuf>, gpu: Option<u32>, ctx: &egui::Context) {
        if self.done.contains_key(&id) || self.pending.contains(&id) || self.errors.contains_key(&id) {
            return;
        }
        self.pending.insert(id);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("sharp-analysis".into()).spawn(move || {
            let r = analyse_file(&path, detector.as_deref(), gpu);
            let _ = tx.send((id, r));
            ctx.request_repaint();
        });
    }
}

fn analyse_file(path: &std::path::Path, detector: Option<&std::path::Path>, gpu: Option<u32>) -> Result<Analysed, String> {
    let img = crate::pipeline::image_job::load_rgb_alpha(path).map_err(|e| e.to_string())?.0;
    let report = crate::sharp::analysis::analyse(&img);
    let (w, h) = img.dimensions();
    let (tw, th) = crate::engines::resize::fit_within(w, h, THUMB, THUMB);
    let thumb = crate::engines::resize::resize_rgb(&img, tw, th, crate::engines::resize::ResizeKind::Lanczos3);
    let faces = match detector {
        Some(det) => {
            let (dw, dh) = crate::engines::resize::fit_within(w, h, 1280, 1280);
            let small = crate::engines::resize::resize_rgb(&img, dw, dh, crate::engines::resize::ResizeKind::Lanczos3);
            let k = w as f32 / dw as f32;
            crate::engines::onnx::face::detect_faces(det, &small, gpu)
                .map(|v| v.into_iter().map(|f| [f.bbox[0] * k, f.bbox[1] * k, f.bbox[2] * k, f.bbox[3] * k]).collect())
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    Ok(Analysed { report, size: (w, h), thumb, faces })
}

/// Catalog ids the mode needs with the current settings that are not installed.
pub fn missing_parts(app: &App) -> Vec<String> {
    let s = app.config.job(app.cur_settings());
    let models = &app.env.models;
    let cat = &models.catalog;
    let mut ids: Vec<&str> = vec![plan::DEBLUR_MOTION, plan::DEBLUR_DEFOCUS];
    ids.push(if s.sharp.sr_model.is_empty() { plan::FAITHFUL_SR } else { s.sharp.sr_model.as_str() });
    let wants_creative = !s.sharp.creative_auto && s.sharp.creative > 0.0;
    if wants_creative {
        ids.push(plan::GENERATIVE);
    }
    if s.sharp.fix_faces {
        ids.push(if cat.model(&s.face_model).is_some() { s.face_model.as_str() } else { "gfpgan-1.4" });
        ids.push("yunet");
    }
    ids.into_iter()
        .filter(|id| cat.model(id).is_some_and(|m| !models.catalog_files_present(m)))
        .map(str::to_string)
        .collect()
}

fn generative_installed(app: &App) -> bool {
    let m = &app.env.models;
    m.catalog.model(plan::GENERATIVE).is_some_and(|e| m.catalog_files_present(e))
}

/// The mode's settings panel (inside the Image page's "Mode" section).
pub fn sharp_mode(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.sharp_ui.pump(&ctx);
    let item = app.selected_item().and_then(|i| Some((i.id, i.info.clone()?)));
    let gpu = crate::devices::resolve(app.config.machine.compute, &app.env.gpus).cloned();
    if let Some((id, info)) = &item {
        let det = app
            .env
            .models
            .catalog
            .model("yunet")
            .filter(|m| app.env.models.onnx_runtime && app.env.models.catalog_files_present(m))
            .and_then(|m| m.onnx.as_ref().map(|o| app.env.models.base.join(&o.file)));
        app.sharp_ui.ensure(*id, info.path.clone(), det, gpu.as_ref().map(|g| g.index), &ctx);
    }
    let report = item.as_ref().and_then(|(id, _)| app.sharp_ui.report(*id).copied());

    // ---- What the analysis found ----
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        match (&item, report) {
            (Some((_, info)), Some(r)) => {
                ui.label(RichText::new(t!(r.describe_key())).strong().color(INFO));
                let suggested = Preset::for_report(&r);
                let (ew, eh) = plan::downscaled_size(info.width, info.height, r.sharp_factor.max(1.0));
                ui.label(
                    RichText::new(t!(
                        "sharp.analysis_detail",
                        preset = t!(suggested.label_key()),
                        w = ew,
                        h = eh
                    ))
                    .small(),
                );
            }
            (Some((id, _)), None) => {
                if let Some(e) = app.sharp_ui.errors.get(id) {
                    ui.colored_label(WARN, format!("⚠ {e}"));
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("sharp.analysing"));
                    });
                }
            }
            (None, _) => {
                ui.label(RichText::new(t!("sharp.pick_image")).weak());
            }
        }
    });

    let gen_ok = generative_installed(app);
    let adv = crate::ui::studio::advanced(app);
    let sys = app.sys().clone();
    let s = app.config.settings_for_mut(app.studio);
    let before = s.sharp.clone();

    // ---- Presets ----
    ui.label(RichText::new(t!("sharp.preset")).strong());
    ui.horizontal_wrapped(|ui| {
        for p in Preset::ALL {
            ui.selectable_value(&mut s.sharp.preset, p, t!(p.label_key())).on_hover_text(t!(p.tip_key()));
        }
    });
    let resolved_preset = match (s.sharp.preset, report) {
        (Preset::Auto, Some(r)) => Preset::for_report(&r),
        (p, _) => p,
    };

    // ---- Faithful ↔ Creative ----
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(t!("sharp.faithful")).small());
        let mut v = if s.sharp.creative_auto { resolved_preset.default_creative() } else { s.sharp.creative };
        let r = ui
            .add_enabled(gen_ok, egui::Slider::new(&mut v, 0.0..=1.0).show_value(false))
            .on_hover_text(t!("sharp.creative_tip"))
            .on_disabled_hover_text(t!("sharp.creative_needs_model"));
        if r.changed() {
            s.sharp.creative = v;
            s.sharp.creative_auto = false;
        }
        ui.label(RichText::new(t!("sharp.creative")).small());
        let pct = format!("{:.0}%", v * 100.0);
        ui.label(if s.sharp.creative_auto { t!("sharp.auto_value", v = pct) } else { pct });
        if !s.sharp.creative_auto && ui.small_button(t!("sharp.reset_auto")).clicked() {
            s.sharp.creative_auto = true;
        }
    });
    let creative = if !gen_ok {
        0.0
    } else if s.sharp.creative_auto {
        resolved_preset.default_creative()
    } else {
        s.sharp.creative
    };
    if creative > 0.0 {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(WARN, format!("⚠ {}", t!("sharp.generative_notice")));
        });
    } else if !gen_ok {
        ui.label(RichText::new(t!("sharp.creative_needs_model")).small().weak());
    }

    // ---- Faces, size ----
    ui.checkbox(&mut s.sharp.fix_faces, t!("sharp.fix_faces")).on_hover_text(t!("sharp.fix_faces_tip"));
    ui.horizontal(|ui| {
        ui.label(t!("sharp.output_size"));
        ui.selectable_value(&mut s.sharp.scale, 1, t!("sharp.size_original"));
        ui.selectable_value(&mut s.sharp.scale, 2, "2x");
        ui.selectable_value(&mut s.sharp.scale, 4, "4x");
    });

    // ---- Estimate ----
    if let Some((_, info)) = &item {
        let mp = info.width as f64 * info.height as f64 / 1e6;
        let vram_mb = gpu.as_ref().map(|g| g.vram_bytes >> 20).unwrap_or(0);
        let cuda = crate::engines::onnx::runtime::loaded_kind() == Some(crate::models::RuntimeKind::Cuda);
        let secs = plan::estimate_secs(mp, s.sharp.scale, creative, plan::generative_max_mp(vram_mb), cuda);
        let fit = plan::pc_fit(creative, gpu.is_some(), vram_mb, sys.ram_available >> 20);
        ui.horizontal_wrapped(|ui| {
            ui.label(t!("sharp.estimate", time = crate::util::human_duration(secs)));
            let col = match fit {
                plan::PcFit::Fits => GOOD,
                plan::PcFit::Tight => WARN,
                plan::PcFit::TooHeavy => Color32::from_rgb(230, 90, 80),
            };
            ui.colored_label(col, t!(fit.label_key()));
        });
    }

    // ---- Advanced ----
    if adv {
        egui::CollapsingHeader::new(t!("sharp.advanced")).id_salt("sharp-adv").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(t!("sharp.adv_deblur")).on_hover_text(t!("sharp.adv_deblur_tip"));
                let label = |v: &str| match v {
                    "" => t!("sharp.deblur_auto"),
                    plan::DEBLUR_MOTION => t!("sharp.deblur_motion"),
                    plan::DEBLUR_DEFOCUS => t!("sharp.deblur_defocus"),
                    plan::NONE => t!("sharp.deblur_none"),
                    other => other.to_string(),
                };
                egui::ComboBox::from_id_salt("sharp-deblur").selected_text(label(&s.sharp.deblur_model)).show_ui(ui, |ui| {
                    for v in ["", plan::DEBLUR_MOTION, plan::DEBLUR_DEFOCUS, plan::NONE] {
                        ui.selectable_value(&mut s.sharp.deblur_model, v.to_string(), label(v));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(t!("sharp.adv_downscale")).on_hover_text(t!("sharp.adv_downscale_tip"));
                let mut auto = s.sharp.downscale < 1.0;
                if ui.checkbox(&mut auto, t!("sharp.auto")).changed() {
                    s.sharp.downscale = if auto { 0.0 } else { 2.0 };
                }
                if !auto {
                    ui.add(egui::Slider::new(&mut s.sharp.downscale, 1.0..=6.0).step_by(0.25).suffix("x"));
                }
            });
            ui.horizontal(|ui| {
                ui.label(t!("sharp.adv_tile")).on_hover_text(t!("sharp.adv_tile_tip"));
                let mut auto = s.sharp.tile == 0;
                if ui.checkbox(&mut auto, t!("sharp.auto")).changed() {
                    s.sharp.tile = if auto { 0 } else { 256 };
                }
                if !auto {
                    ui.add(egui::Slider::new(&mut s.sharp.tile, crate::sharp::seedvr2::MIN_TILE..=512).step_by(16.0));
                }
            });
            ui.horizontal(|ui| {
                ui.label(t!("sharp.adv_seed")).on_hover_text(t!("sharp.adv_seed_tip"));
                ui.add(egui::DragValue::new(&mut s.sharp.seed));
            });
            ui.add(egui::Slider::new(&mut s.sharp.face_strength, 0.0..=1.0).text(t!("sharp.adv_face_strength")))
                .on_hover_text(t!("sharp.adv_face_strength_tip"));
            ui.add(egui::Slider::new(&mut s.sharp.sharpen, 0.0..=1.0).text(t!("sharp.adv_sharpen")))
                .on_hover_text(t!("sharp.adv_sharpen_tip"));
        });
    }
    let changed = s.sharp != before;

    // ---- Detail model (advanced) ----
    let mut pick = crate::ui::models::ComboPick::None;
    if adv {
        let mut sr = (!app.config.settings_for(app.studio).sharp.sr_model.is_empty())
            .then(|| app.config.settings_for(app.studio).sharp.sr_model.clone());
        ui.horizontal(|ui| {
            ui.label(t!("sharp.adv_sr")).on_hover_text(t!("sharp.adv_sr_tip"));
            pick = crate::ui::models::upscale_model_combo(ui, "sharp-sr", &app.models_ui, &app.env.models, &mut sr);
        });
        let s = app.config.settings_for_mut(app.studio);
        s.sharp.sr_model = sr.unwrap_or_default();
    }

    // ---- Missing parts ----
    let missing = missing_parts(app);
    if !missing.is_empty() {
        let size: u64 = missing
            .iter()
            .filter_map(|id| app.models_ui.catalog.model(id))
            .flat_map(|m| m.packages.iter())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(|p| app.models_ui.catalog.package(p))
            .map(|p| p.size)
            .sum();
        ui.horizontal_wrapped(|ui| {
            let names: Vec<String> = missing
                .iter()
                .map(|id| app.models_ui.catalog.model(id).map(|m| m.name.get().to_string()).unwrap_or_else(|| id.clone()))
                .collect();
            ui.colored_label(WARN, format!("⚠ {}", t!("models.missing_for_job", names = names.join(", "))));
            if app.models_ui.running() {
                ui.spinner();
            } else if ui.button(t!("sharp.install_parts", size = crate::util::human_bytes(size))).clicked() {
                app.models_ui.request_install_many(&missing);
            }
        });
    }
    if !gen_ok && missing.iter().all(|m| m != plan::GENERATIVE) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t!("sharp.get_creative")).small());
            if ui.small_button(t!("sharp.get_creative_btn")).clicked() && !app.models_ui.running() {
                app.models_ui.request_install(plan::GENERATIVE, false);
            }
        });
    }
    crate::ui::models::handle_pick(app, pick);

    // ---- Preview region ----
    if let Some((id, info)) = item {
        ui.separator();
        ui.label(RichText::new(t!("sharp.region")).strong()).on_hover_text(t!("sharp.region_tip"));
        region_picker(app, ui, id, &info);
        ui.horizontal_wrapped(|ui| {
            if ui.button(t!("sharp.preview_region")).on_hover_text(t!("sharp.preview_region_tip")).clicked() {
                app.request_sharp_preview(false, false);
            }
            if ui
                .add_enabled(gen_ok, egui::Button::new(t!("sharp.compare")))
                .on_hover_text(t!("sharp.compare_tip"))
                .on_disabled_hover_text(t!("sharp.creative_needs_model"))
                .clicked()
            {
                app.request_sharp_preview(false, true);
            }
            if ui.button(t!("sharp.full_image")).on_hover_text(t!("sharp.full_image_tip")).clicked() {
                app.request_sharp_preview(true, false);
            }
        });
    }
    if changed {
        app.mark_preview_dirty();
    }
}

/// Thumbnail of the source with the preview box; drag to draw a new box, click to move it.
fn region_picker(app: &mut App, ui: &mut egui::Ui, id: u64, info: &MediaInfo) {
    let Some(tex) = app.sharp_ui.tex.get(&id).cloned() else {
        return;
    };
    let (iw, ih) = (info.width.max(1) as f32, info.height.max(1) as f32);
    let max_w = ui.available_width().min(THUMB as f32);
    let k = (max_w / iw).min(THUMB as f32 / ih);
    let size = egui::vec2(iw * k, ih * k);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    let to_src = |pos: egui::Pos2| ((pos.x - rect.min.x) / k, (pos.y - rect.min.y) / k);
    let mut r = app.sharp_region(id, info);
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        app.sharp_ui.drag = Some(to_src(pos));
    }
    let mut commit = None;
    if let (Some(a), Some(pos)) = (app.sharp_ui.drag, resp.interact_pointer_pos())
        && resp.dragged()
    {
        r = region::from_drag(a, to_src(pos), info.width, info.height);
    }
    if resp.drag_stopped()
        && let (Some(a), Some(pos)) = (app.sharp_ui.drag.take(), resp.interact_pointer_pos())
    {
        commit = Some(region::from_drag(a, to_src(pos), info.width, info.height));
    } else if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let (cx, cy) = to_src(pos);
        commit = Some(region::around(cx, cy, r[2].max(r[3]), info.width, info.height));
    }
    if let Some(c) = commit {
        r = c;
        app.ws.sharp_regions.insert(id, c);
        app.mark_preview_dirty();
    }
    let box_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(r[0] as f32 * k, r[1] as f32 * k),
        egui::vec2(r[2] as f32 * k, r[3] as f32 * k),
    );
    p.rect_filled(rect, 0.0, Color32::from_black_alpha(40));
    p.rect_stroke(box_rect, 0.0, Stroke::new(2.0, Color32::from_rgb(255, 210, 60)), egui::StrokeKind::Outside);
    ui.label(RichText::new(t!("sharp.region_size", w = r[2], h = r[3])).small().weak());
}
