//! Profile bar at the top of each studio page: pick / save / save as / rename / duplicate /
//! delete / set default / revert, export and import (`.toml`).

use egui::{Color32, RichText};

use crate::app::App;
use crate::config::{MachineSettings, Studio};
use crate::profiles::{ProfileFile, ProfileKey, builtins_for, safe_file_stem};
use crate::t;

const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERR: Color32 = Color32::from_rgb(235, 100, 100);

/// Open modal on the profile bar.
pub enum ProfileDialog {
    SaveAs { name: String, include_perf: bool, duplicate: bool },
    Rename { old: String, name: String },
    Delete { name: String },
    /// Imported profile with performance values this PC may not handle.
    ImportWarn { name: String, kind: Studio, warnings: Vec<String>, perf: MachineSettings },
}

/// The profile currently loaded on the active page: (key, display name, recipe, perf).
fn loaded(app: &App) -> Option<(ProfileKey, String, crate::config::JobSettings, Option<MachineSettings>)> {
    let st = app.studio;
    let key = ProfileKey::parse(app.config.pages.get(st).last_profile.as_deref()?)?;
    let (name, s, perf) = app.profiles.resolve(st, &key)?;
    Some((key, name, s, perf))
}

/// Current settings differ from the loaded profile.
pub fn is_modified(app: &App) -> bool {
    loaded(app).is_some_and(|(_, _, s, _)| app.cur_settings().recipe() != s.recipe())
}

/// Load a profile into the active page.
pub fn load_profile(app: &mut App, key: &ProfileKey) {
    let st = app.studio;
    let Some((name, recipe, perf)) = app.profiles.resolve(st, key) else {
        app.toast(t!("profiles.err_missing"));
        return;
    };
    app.config.settings_for_mut(st).apply_recipe(&recipe);
    if let Some(p) = perf {
        app.config.machine = p;
        app.sync_monitor_gpu();
        app.toast(t!("profiles.perf_applied", name = &name));
    }
    let meta = app.config.pages.get_mut(st);
    meta.last_profile = Some(key.encode());
    meta.touch_recent(&key.encode());
    if app.config.auto_preview {
        app.mark_preview_dirty();
    }
}

/// Apply each page's default profile (startup).
pub fn apply_defaults(app: &mut App) {
    let cur = app.studio;
    for st in [Studio::All, Studio::Video, Studio::Image] {
        if let Some(key) = app.config.pages.get(st).default_profile.clone().and_then(|k| ProfileKey::parse(&k))
            && app.profiles.resolve(st, &key).is_some()
        {
            app.studio = st;
            load_profile(app, &key);
        }
    }
    app.studio = cur;
}

fn key_label(app: &App, st: Studio, key: &ProfileKey) -> Option<String> {
    let (name, _, _) = app.profiles.resolve(st, key)?;
    let builtin = matches!(key, ProfileKey::Builtin(_));
    let default = app.config.pages.get(st).default_profile.as_deref() == Some(&key.encode());
    Some(format!(
        "{}{}{}",
        if default { "★ " } else { "" },
        name,
        if builtin { format!("  ({})", t!("profiles.builtin")) } else { String::new() }
    ))
}

pub fn profile_bar(app: &mut App, ui: &mut egui::Ui) {
    let st = app.studio;
    let current = loaded(app);
    let modified = is_modified(app);
    let mut pick: Option<ProfileKey> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(t!("profiles.label"));
        let text = match &current {
            Some((_, name, _, _)) => format!("{name}{}", if modified { " *" } else { "" }),
            None => t!("profiles.none"),
        };
        egui::ComboBox::from_id_salt(("profile-pick", st.index()))
            .selected_text(RichText::new(text).strong())
            .width(200.0)
            .show_ui(ui, |ui| {
                let recent: Vec<ProfileKey> = app
                    .config
                    .pages
                    .get(st)
                    .recent
                    .iter()
                    .filter_map(|k| ProfileKey::parse(k))
                    .filter(|k| app.profiles.resolve(st, k).is_some())
                    .collect();
                if !recent.is_empty() {
                    ui.label(RichText::new(t!("profiles.recent")).small().weak());
                    for k in &recent {
                        if let Some(l) = key_label(app, st, k)
                            && ui.selectable_label(false, l).clicked()
                        {
                            pick = Some(k.clone());
                        }
                    }
                    ui.separator();
                }
                ui.label(RichText::new(t!("profiles.builtin_list")).small().weak());
                for b in builtins_for(st) {
                    let k = ProfileKey::Builtin(b.id.into());
                    let on = current.as_ref().is_some_and(|c| c.0 == k);
                    if let Some(l) = key_label(app, st, &k)
                        && ui.selectable_label(on, l).clicked()
                    {
                        pick = Some(k);
                    }
                }
                let users: Vec<String> = app.profiles.user_for(st).map(|u| u.data.name.clone()).collect();
                ui.separator();
                ui.label(RichText::new(t!("profiles.user_list")).small().weak());
                if users.is_empty() {
                    ui.label(RichText::new(t!("profiles.user_none")).small().weak());
                }
                for n in users {
                    let k = ProfileKey::User(n);
                    let on = current.as_ref().is_some_and(|c| c.0 == k);
                    if let Some(l) = key_label(app, st, &k)
                        && ui.selectable_label(on, l).clicked()
                    {
                        pick = Some(k);
                    }
                }
            });
        if modified {
            ui.label(RichText::new(t!("profiles.modified")).color(WARN).small())
                .on_hover_text(t!("profiles.modified_tip"));
        }
    });
    if let Some(k) = pick {
        load_profile(app, &k);
    }

    let user_name = match &current {
        Some((ProfileKey::User(n), ..)) => Some(n.clone()),
        _ => None,
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let can_save = user_name.is_some() && modified;
        if ui
            .add_enabled(can_save, egui::Button::new(t!("profiles.save")).small())
            .on_hover_text(t!("profiles.save_tip"))
            .on_disabled_hover_text(t!("profiles.save_disabled"))
            .clicked()
            && let Some(n) = &user_name
        {
            let perf = app.profiles.find_user(st, n).and_then(|u| u.data.performance.as_ref()).map(|_| app.config.machine.clone());
            let s = app.cur_settings().clone();
            match app.profiles.update(st, n, &s, perf) {
                Ok(()) => app.toast(t!("profiles.saved", name = n)),
                Err(e) => app.toast(t!("profiles.err_write", error = e)),
            }
        }
        if ui.small_button(t!("profiles.save_as")).clicked() {
            let base = current.as_ref().map(|c| c.1.clone()).unwrap_or_else(|| t!("profiles.new_name"));
            app.profile_dialog = Some(ProfileDialog::SaveAs {
                name: app.profiles.unique_name(st, &base),
                include_perf: false,
                duplicate: false,
            });
        }
        if ui
            .add_enabled(current.is_some(), egui::Button::new(t!("profiles.duplicate")).small())
            .on_hover_text(t!("profiles.duplicate_tip"))
            .clicked()
            && let Some(c) = &current
        {
            app.profile_dialog = Some(ProfileDialog::SaveAs {
                name: app.profiles.unique_name(st, &c.1),
                include_perf: c.3.is_some(),
                duplicate: true,
            });
        }
        if ui
            .add_enabled(user_name.is_some(), egui::Button::new(t!("profiles.rename")).small())
            .on_disabled_hover_text(t!("profiles.builtin_readonly"))
            .clicked()
            && let Some(n) = &user_name
        {
            app.profile_dialog = Some(ProfileDialog::Rename { old: n.clone(), name: n.clone() });
        }
        if ui
            .add_enabled(user_name.is_some(), egui::Button::new(RichText::new(t!("profiles.delete")).color(ERR)).small())
            .on_disabled_hover_text(t!("profiles.builtin_readonly"))
            .clicked()
            && let Some(n) = &user_name
        {
            app.profile_dialog = Some(ProfileDialog::Delete { name: n.clone() });
        }
        if let Some(c) = &current {
            let key = c.0.encode();
            let is_default = app.config.pages.get(st).default_profile.as_deref() == Some(&key);
            let label = if is_default { t!("profiles.unset_default") } else { t!("profiles.set_default") };
            if ui.small_button(label).on_hover_text(t!("profiles.default_tip")).clicked() {
                app.config.pages.get_mut(st).default_profile = (!is_default).then_some(key);
            }
        }
        if modified && ui.small_button(t!("profiles.revert")).clicked()
            && let Some(c) = &current
        {
            let k = c.0.clone();
            load_profile(app, &k);
        }
        ui.separator();
        if ui.small_button(t!("profiles.export")).on_hover_text(t!("profiles.export_tip")).clicked() {
            export(app);
        }
        if ui.small_button(t!("profiles.import")).clicked() {
            import(app);
        }
    });
    dialog(app, ui.ctx());
}

fn export(app: &mut App) {
    let st = app.studio;
    let (name, perf) = match loaded(app) {
        Some((_, name, _, perf)) => (name, perf),
        None => (t!("profiles.new_name"), None),
    };
    let data = ProfileFile::new(&name, st, app.cur_settings(), perf);
    let Some(path) = rfd::FileDialog::new()
        .add_filter("TOML", &["toml"])
        .set_file_name(format!("{}.toml", safe_file_stem(&name)))
        .save_file()
    else {
        return;
    };
    match data.to_toml().and_then(|t| std::fs::write(&path, t).map_err(|e| e.to_string())) {
        Ok(()) => app.toast(t!("profiles.exported", path = path.display())),
        Err(e) => app.toast(t!("profiles.err_write", error = e)),
    }
}

fn import(app: &mut App) {
    let Some(path) = rfd::FileDialog::new().add_filter("TOML", &["toml"]).pick_file() else {
        return;
    };
    let data = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| ProfileFile::from_toml(&t));
    let mut data = match data {
        Ok(d) => d,
        Err(e) => {
            app.toast(t!("profiles.err_import", error = e));
            return;
        }
    };
    if data.name.is_empty() {
        data.name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    }
    data.name = app.profiles.unique_name(data.kind, &data.name);
    if data.schema_version > crate::profiles::SCHEMA_VERSION {
        app.toast(t!("profiles.newer_schema"));
    }
    let kind = data.kind;
    let name = data.name.clone();
    let perf = data.performance.clone();
    if let Err(e) = app.profiles.create(data) {
        app.toast(t!("profiles.err_import", error = e));
        return;
    }
    app.toast(t!("profiles.imported", name = &name, page = kind.label()));
    if let Some(p) = perf {
        let warnings = crate::ui::perf::machine_warnings(app, &p);
        if !warnings.is_empty() {
            app.profile_dialog = Some(ProfileDialog::ImportWarn { name, kind, warnings, perf: p });
        }
    }
}

fn dialog(app: &mut App, ctx: &egui::Context) {
    let Some(mut d) = app.profile_dialog.take() else {
        return;
    };
    let st = app.studio;
    let mut close = false;
    egui::Modal::new(egui::Id::unique("profile-dialog")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        match &mut d {
            ProfileDialog::SaveAs { name, include_perf, duplicate } => {
                ui.heading(if *duplicate { t!("profiles.duplicate") } else { t!("profiles.save_as") });
                ui.label(t!("profiles.name"));
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
                r.request_focus();
                ui.checkbox(include_perf, t!("profiles.include_perf"))
                    .on_hover_text(t!("profiles.include_perf_tip"));
                let check = app.profiles.check_name(st, name, None);
                if let Err(e) = &check {
                    ui.colored_label(ERR, e.message());
                }
                ui.horizontal(|ui| {
                    let ok = ui.add_enabled(check.is_ok(), egui::Button::new(t!("profiles.save"))).clicked()
                        || (check.is_ok() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if ok {
                        let settings = if *duplicate {
                            loaded(app).map(|c| c.2).unwrap_or_else(|| app.cur_settings().clone())
                        } else {
                            app.cur_settings().clone()
                        };
                        let perf = include_perf.then(|| app.config.machine.clone());
                        let data = ProfileFile::new(name, st, &settings, perf);
                        let key = ProfileKey::User(data.name.clone());
                        match app.profiles.create(data) {
                            Ok(()) => {
                                if !*duplicate {
                                    let meta = app.config.pages.get_mut(st);
                                    meta.last_profile = Some(key.encode());
                                    meta.touch_recent(&key.encode());
                                }
                                app.toast(t!("profiles.saved", name = name.trim()));
                                close = true;
                            }
                            Err(e) => app.toast(t!("profiles.err_write", error = e)),
                        }
                    }
                    if ui.button(t!("common.cancel")).clicked() {
                        close = true;
                    }
                });
            }
            ProfileDialog::Rename { old, name } => {
                ui.heading(t!("profiles.rename"));
                ui.add(egui::TextEdit::singleline(name).desired_width(320.0)).request_focus();
                let check = app.profiles.check_name(st, name, Some(old));
                if let Err(e) = &check {
                    ui.colored_label(ERR, e.message());
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(check.is_ok(), egui::Button::new(t!("profiles.rename"))).clicked() {
                        match app.profiles.rename(st, old, name) {
                            Ok(()) => {
                                let old_key = ProfileKey::User(old.clone()).encode();
                                let new_key = ProfileKey::User(name.trim().to_string()).encode();
                                let meta = app.config.pages.get_mut(st);
                                for k in meta.last_profile.iter_mut().chain(meta.default_profile.iter_mut()).chain(meta.recent.iter_mut()) {
                                    if *k == old_key {
                                        *k = new_key.clone();
                                    }
                                }
                                close = true;
                            }
                            Err(e) => app.toast(t!("profiles.err_write", error = e)),
                        }
                    }
                    if ui.button(t!("common.cancel")).clicked() {
                        close = true;
                    }
                });
            }
            ProfileDialog::Delete { name } => {
                ui.heading(t!("profiles.delete_title"));
                ui.label(t!("profiles.delete_body", name = &*name));
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(t!("profiles.delete")).color(ERR)).clicked() {
                        match app.profiles.delete(st, name) {
                            Ok(()) => {
                                let key = ProfileKey::User(name.clone()).encode();
                                let meta = app.config.pages.get_mut(st);
                                meta.recent.retain(|k| *k != key);
                                if meta.last_profile.as_deref() == Some(&key) {
                                    meta.last_profile = None;
                                }
                                if meta.default_profile.as_deref() == Some(&key) {
                                    meta.default_profile = None;
                                }
                                close = true;
                            }
                            Err(e) => app.toast(t!("profiles.err_write", error = e)),
                        }
                    }
                    if ui.button(t!("common.cancel")).clicked() {
                        close = true;
                    }
                });
            }
            ProfileDialog::ImportWarn { name, kind, warnings, perf } => {
                ui.heading(t!("profiles.import_warn_title"));
                ui.label(t!("profiles.import_warn_body", name = &*name));
                for w in warnings.iter() {
                    ui.colored_label(WARN, format!("⚠ {w}"));
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("profiles.adjust_pc")).on_hover_text(t!("profiles.adjust_pc_tip")).clicked() {
                        let fixed = crate::ui::perf::adjust_to_pc(app, perf);
                        let s = app.profiles.find_user(*kind, name).map(|u| u.data.settings.clone());
                        if let Some(s) = s {
                            let _ = app.profiles.update(*kind, name, &s, Some(fixed));
                        }
                        app.toast(t!("profiles.adjusted", name = &*name));
                        close = true;
                    }
                    if ui.button(t!("profiles.keep_as_is")).clicked() {
                        close = true;
                    }
                });
            }
        }
    });
    if !close {
        app.profile_dialog = Some(d);
    }
}
