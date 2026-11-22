//! Settings page: performance (Simple / Advanced, benchmark, auto-tune), application
//! settings (language, updates, tools) and About / Paths.

use egui::RichText;

use super::settings::{about, application, hint};
use crate::app::App;
use crate::t;

pub fn settings_page(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(820.0_f32.min(ui.available_width()));
            ui.heading(t!("tabs.settings"));
            hint(ui, t!("settings_page.intro"));
            ui.separator();
            let before = (app.cur_settings().clone(), app.config.machine.clone());
            egui::CollapsingHeader::new(RichText::new(t!("settings_page.performance")).strong())
                .id_salt("sp-perf")
                .default_open(true)
                .show(ui, |ui| crate::ui::perf::performance_section(app, ui));
            super::settings::after_edit(app, before);
            application(app, ui);
            about(app, ui);
        });
    });
}
