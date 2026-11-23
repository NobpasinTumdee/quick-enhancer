//! First-run setup wizard (also the `--install-tools` page run by the installer):
//! 1 language → 2 required tools → 3 starter pack → 4 optional models → 5 summary + install.
//! Offline archives are still handled by the classic tools list ("Manage tools").

use egui::{Color32, RichText};

use crate::app::App;
use crate::i18n::{self, Lang};
use crate::models::catalog::{RUNTIME_CUDA, RUNTIME_DML};
use crate::models::fit::NVIDIA_VENDOR;
use crate::net::tools::Component;
use crate::t;
use crate::util::human_bytes;

const OK: Color32 = Color32::from_rgb(110, 200, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);

/// One selectable line of the wizard.
pub struct WizOpt {
    pub key: &'static str,
    /// Package ids (catalog or `tools.json`).
    pub packages: Vec<&'static str>,
    pub step: usize,
    pub required: bool,
    /// Needs an ONNX runtime (the DirectML runtime is then selected automatically).
    pub needs_onnx: bool,
    pub nvidia_only: bool,
}

pub fn options() -> Vec<WizOpt> {
    let o = |key, packages: Vec<&'static str>, step, required, needs_onnx, nvidia_only| WizOpt {
        key,
        packages,
        step,
        required,
        needs_onnx,
        nvidia_only,
    };
    vec![
        o("ffmpeg", vec!["ffmpeg"], 1, true, false, false),
        o("realesrgan", vec!["realesrgan"], 2, false, false, false),
        o("rife", vec!["rife"], 2, false, false, false),
        o("runtime", RUNTIME_DML.to_vec(), 3, false, false, false),
        o("photo", vec!["onnx-realplksr-nomos2", "onnx-span-nomosuni"], 3, false, true, false),
        o("anime", vec!["realcugan"], 3, false, false, false),
        o("restore", vec!["onnx-dejpg", "onnx-denoise", "onnx-scunet"], 3, false, true, false),
        o("faces", vec!["onnx-gfpgan", "onnx-yunet"], 3, false, true, false),
        o("cuda", RUNTIME_CUDA.to_vec(), 3, false, false, true),
    ]
}

/// Packages of the checked options (plus the DirectML runtime when an ONNX option is checked
/// and no runtime is chosen), without duplicates, in install order.
pub fn selected_packages(opts: &[WizOpt], checked: &dyn Fn(&str) -> bool, have_runtime: bool) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    let onnx = opts.iter().any(|o| o.needs_onnx && checked(o.key));
    let runtime_checked = checked("runtime") || checked("cuda");
    for o in opts {
        let take = checked(o.key) || o.required || (o.key == "runtime" && onnx && !runtime_checked && !have_runtime);
        if take {
            for p in &o.packages {
                if !out.contains(p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[derive(Default)]
pub struct Wizard {
    pub open: bool,
    step: usize,
    checked: std::collections::BTreeMap<&'static str, bool>,
    started: bool,
}

impl Wizard {
    pub fn open(&mut self) {
        self.open = true;
        self.step = 0;
        self.started = false;
        self.checked.clear();
        for k in ["realesrgan", "rife"] {
            self.checked.insert(k, true);
        }
    }
}

const STEPS: usize = 5;

fn resolve(app: &App, id: &str) -> Option<Component> {
    app.models_ui.catalog.resolve_package(id, &app.models_ui.tools)
}

fn present(app: &App, id: &str) -> bool {
    resolve(app, id).is_some_and(|c| c.is_present(&app.paths.base))
}

/// Wizard body. Returns true when the user finished or skipped.
pub fn wizard_body(app: &mut App, ui: &mut egui::Ui) -> bool {
    let mut finished = false;
    let step = app.wizard.step;
    ui.horizontal(|ui| {
        for i in 0..STEPS {
            let txt = RichText::new(format!("{} {}", i + 1, t!(&format!("wizard.step{}", i + 1))));
            ui.label(if i == step { txt.strong().underline() } else { txt.weak() });
            if i + 1 < STEPS {
                ui.label(RichText::new("›").weak());
            }
        }
    });
    ui.separator();
    let opts = options();
    let has_nvidia = app.env.gpus.iter().any(|g| g.vendor_id == NVIDIA_VENDOR);
    match step {
        0 => {
            ui.label(t!("wizard.lang_intro"));
            for l in Lang::ALL {
                let on = app.config.language == Some(l) || (app.config.language.is_none() && i18n::lang() == l);
                if ui.radio(on, l.native_name()).clicked() {
                    app.config.language = Some(l);
                    i18n::set_lang(l);
                }
            }
        }
        1..=3 => {
            ui.label(t!(&format!("wizard.intro{}", step + 1)));
            ui.add_space(4.0);
            for o in opts.iter().filter(|o| o.step == step) {
                let comps: Vec<Component> = o.packages.iter().filter_map(|p| resolve(app, p)).collect();
                let have = o.packages.iter().all(|p| present(app, p));
                let size: u64 = comps.iter().map(|c| c.size).sum();
                let allowed = !o.nvidia_only || has_nvidia;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let mut on = o.required || app.wizard.checked.get(o.key).copied().unwrap_or(false);
                        let enabled = !o.required && !have && allowed && !app.wizard.started;
                        if ui.add_enabled(enabled, egui::Checkbox::new(&mut on, "")).changed() {
                            app.wizard.checked.insert(o.key, on);
                        }
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(t!(&format!("wizard.opt_{}", o.key))).strong());
                                if o.required {
                                    ui.label(RichText::new(t!("tools.required")).small().color(WARN));
                                }
                                if have {
                                    ui.colored_label(OK, RichText::new(t!("models.state_installed")).small());
                                } else {
                                    ui.label(RichText::new(human_bytes(size)).weak());
                                }
                                if !allowed {
                                    ui.label(RichText::new(t!("models.fit_needs_nvidia")).small().color(WARN));
                                }
                            });
                            ui.label(RichText::new(t!(&format!("wizard.opt_{}_desc", o.key))).small().weak());
                            let lic: Vec<String> = comps.iter().map(|c| c.license.clone()).collect();
                            ui.label(RichText::new(format!("{}: {}", t!("tools.license"), lic.join("; "))).small().weak());
                        });
                    });
                });
            }
        }
        _ => {
            let checked = |k: &str| app.wizard.checked.get(k).copied().unwrap_or(false);
            let ids = selected_packages(&opts, &checked, app.models_ui.snap.any_runtime());
            let plan: Vec<Component> = ids
                .iter()
                .filter(|p| !present(app, p))
                .filter_map(|p| resolve(app, p))
                .collect();
            let total: u64 = plan.iter().map(|c| c.size).sum();
            if plan.is_empty() {
                ui.label(t!("wizard.nothing"));
            } else {
                for c in &plan {
                    ui.label(format!("• {} — {}", c.name, human_bytes(c.size)));
                }
                ui.label(RichText::new(t!("models.total_size", size = human_bytes(total))).strong());
                if let Some(free) = crate::pipeline::temp::free_space(&app.paths.base) {
                    ui.label(RichText::new(t!("models.free_space", size = human_bytes(free))).small().weak());
                }
            }
            ui.label(RichText::new(t!("wizard.later")).small().weak());
            ui.label(RichText::new(t!("tools.credits")).small().weak());
            ui.add_space(6.0);
            if !app.wizard.started
                && !plan.is_empty()
                && ui.button(RichText::new(t!("wizard.install", size = human_bytes(total))).strong()).clicked()
            {
                let ctx = ui.ctx().clone();
                let paths = app.paths.clone();
                app.models_ui.start(&paths, &ctx, plan, Vec::new());
                app.wizard.started = true;
            }
            app.models_ui.progress_ui(ui);
        }
    }
    ui.add_space(10.0);
    ui.separator();
    let running = app.models_ui.running();
    ui.horizontal(|ui| {
        if step > 0 && ui.add_enabled(!running, egui::Button::new(t!("wizard.back"))).clicked() {
            app.wizard.step -= 1;
        }
        if step + 1 < STEPS {
            if ui.button(RichText::new(t!("wizard.next")).strong()).clicked() {
                app.wizard.step += 1;
            }
        } else if ui.add_enabled(!running, egui::Button::new(RichText::new(t!("tools.finish")).strong())).clicked() {
            finished = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(!running, egui::Button::new(t!("tools.skip_for_now"))).clicked() {
                finished = true;
            }
            if ui
                .add_enabled(!running, egui::Button::new(t!("wizard.offline")))
                .on_hover_text(t!("tools.offline_tip"))
                .clicked()
            {
                app.tools_ui.open(&app.paths);
            }
        });
    });
    if finished {
        app.config.wizard_done = true;
    }
    finished
}

/// In-app window (first run).
pub fn wizard_window(app: &mut App, ctx: &egui::Context) {
    if !app.wizard.open {
        return;
    }
    let mut done = false;
    egui::Window::new(t!("wizard.title"))
        .collapsible(false)
        .resizable(true)
        .default_width(680.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
                done = wizard_body(app, ui);
            });
        });
    if done {
        app.wizard.open = false;
    }
}

/// Full-window page for `QuickEnhancer.exe --install-tools` (run by the installer).
pub fn wizard_page(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading(t!("tools.setup_heading"));
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if wizard_body(app, ui) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    });
    crate::ui::tools::tools_window(app, &ui.ctx().clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Catalog;
    use crate::net::tools::ToolsManifest;

    #[test]
    fn dynamic_locale_keys_exist() {
        let en = crate::i18n::parse(crate::i18n::EN_SOURCE, "en");
        let th = crate::i18n::parse(crate::i18n::TH_SOURCE, "th");
        let mut keys: Vec<String> = (1..=STEPS).map(|i| format!("wizard.step{i}")).collect();
        keys.extend((2..=4).map(|i| format!("wizard.intro{i}")));
        for o in options() {
            keys.push(format!("wizard.opt_{}", o.key));
            keys.push(format!("wizard.opt_{}_desc", o.key));
        }
        for k in keys {
            assert!(en.contains_key(&k) && th.contains_key(&k), "missing locale key {k}");
        }
    }

    #[test]
    fn every_option_resolves() {
        let cat = Catalog::embedded();
        let tools = ToolsManifest::embedded();
        for o in options() {
            for p in &o.packages {
                assert!(cat.resolve_package(p, &tools).is_some(), "{}: {p}", o.key);
            }
        }
    }

    #[test]
    fn onnx_option_pulls_in_the_runtime() {
        let opts = options();
        let pick = |keys: &'static [&'static str]| move |k: &str| keys.contains(&k);
        let ids = selected_packages(&opts, &pick(&["photo"]), false);
        assert_eq!(ids, ["ffmpeg", "ort-dml", "onnx-realplksr-nomos2", "onnx-span-nomosuni"]);
        // Already have a runtime → not added again.
        let ids = selected_packages(&opts, &pick(&["photo"]), true);
        assert!(!ids.contains(&"ort-dml"));
        // CUDA chosen instead → DirectML not forced.
        let ids = selected_packages(&opts, &pick(&["faces", "cuda"]), false);
        assert!(ids.contains(&"ort-cuda") && !ids.contains(&"ort-dml"));
        // Nothing optional → only the required tool.
        assert_eq!(selected_packages(&opts, &pick(&[]), false), ["ffmpeg"]);
    }
}
