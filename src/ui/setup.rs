//! First-run / setup panel: lists exactly which tools and models are missing and where they go.

use egui::{Color32, RichText};

use crate::app::App;
use crate::t;

pub fn setup_window(app: &mut App, ctx: &egui::Context) {
    if !app.show_setup {
        return;
    }
    let mut open = true;
    egui::Window::new(t!("setup.title"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(720.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(t!("setup.base_folder", path = app.paths.base.display()));
            ui.label(t!("setup.where"));
            if let Some(e) = &app.vulkan_error {
                ui.colored_label(Color32::from_rgb(230, 170, 60), t!("setup.vulkan", error = e));
            }
            ui.separator();
            egui::Grid::new("setup-grid").striped(true).num_columns(4).show(ui, |ui| {
                ui.label(RichText::new("").strong());
                ui.label(RichText::new(t!("setup.item")).strong());
                ui.label(RichText::new(t!("setup.needed_for")).strong());
                ui.label(RichText::new(t!("setup.expected")).strong());
                ui.end_row();
                for it in &app.setup.items {
                    if it.present {
                        ui.colored_label(Color32::from_rgb(110, 200, 110), "✔");
                    } else if it.required {
                        ui.colored_label(Color32::from_rgb(235, 100, 100), "✖");
                    } else {
                        ui.colored_label(Color32::from_rgb(230, 170, 60), "–");
                    }
                    ui.label(&it.what);
                    ui.label(crate::i18n::tr(it.needed_for));
                    let rel = it.expected.strip_prefix(&app.paths.base).unwrap_or(&it.expected);
                    ui.monospace(rel.display().to_string()).on_hover_text(it.expected.display().to_string());
                    ui.end_row();
                }
            });
            ui.separator();
            if app.setup.all_present() {
                ui.colored_label(Color32::from_rgb(110, 200, 110), t!("setup.all_ok"));
            } else {
                ui.label(t!("setup.missing_note"));
            }
            ui.horizontal(|ui| {
                if ui.button(t!("setup.open_base")).clicked() {
                    super::open_path(&app.paths.base);
                }
                if ui.button(t!("setup.download_tools")).clicked() {
                    app.tools_ui.open(&app.paths);
                }
                if ui.button(t!("setup.recheck")).clicked() {
                    let ctx = ui.ctx().clone();
                    app.reload_tools(&ctx);
                }
            });
        });
    if !open {
        app.show_setup = false;
    }
}
