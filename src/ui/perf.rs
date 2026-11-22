//! Resource estimates and performance controls.

use crate::app::App;
use crate::media::probe::MediaInfo;

/// Extra parts of a page's summary line (estimates).
pub fn summary_extras(_app: &App, _info: &MediaInfo) -> Vec<String> {
    Vec::new()
}

/// Resource section at the bottom of a page's settings.
pub fn page_resources(_app: &mut App, _ui: &mut egui::Ui) {}

/// Performance controls on the Settings page.
pub fn performance_section(app: &mut App, ui: &mut egui::Ui) {
    crate::ui::settings::device(app, ui);
    crate::ui::settings::video(app, ui);
}

/// Problems an imported profile's performance values would cause on this PC.
pub fn machine_warnings(app: &App, m: &crate::config::MachineSettings) -> Vec<String> {
    crate::resources::machine_issues(&crate::resources::SystemInfo::current(app), m)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

/// `m` with every value this PC can't handle replaced by a safe one.
pub fn adjust_to_pc(app: &App, m: &crate::config::MachineSettings) -> crate::config::MachineSettings {
    crate::resources::fix_machine(&crate::resources::SystemInfo::current(app), m)
}
