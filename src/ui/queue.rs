//! Left panel: the job queue with thumbnails, metadata and per-item status.

use egui::{Color32, RichText, Vec2};

use crate::app::{App, ItemStatus};
use crate::t;

fn status_color(s: ItemStatus) -> Color32 {
    match s {
        ItemStatus::Done => Color32::from_rgb(110, 200, 110),
        ItemStatus::Failed | ItemStatus::Invalid => Color32::from_rgb(235, 100, 100),
        ItemStatus::Running => Color32::from_rgb(100, 170, 255),
        ItemStatus::Cancelled | ItemStatus::Skipped => Color32::from_rgb(200, 170, 90),
        _ => Color32::GRAY,
    }
}

enum Action {
    Select(u64),
    Remove(u64),
    Move(u64, isize),
    OpenOutput(u64),
}

pub fn queue_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::left("queue").resizable(true).default_size(340.0).min_size(260.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading(t!("queue.title"));
            ui.label(format!("({})", app.items.len()));
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button(t!("queue.apply_all"))
                .on_hover_text(t!("queue.apply_all_tip"))
                .clicked()
            {
                app.apply_settings(false);
            }
            let any_checked = app.items.iter().any(|i| i.checked);
            if ui
                .add_enabled(any_checked, egui::Button::new(t!("queue.apply_checked")).small())
                .on_hover_text(t!("queue.apply_checked_tip"))
                .clicked()
            {
                app.apply_settings(true);
            }
            if ui.small_button(t!("queue.clear_finished")).clicked() {
                app.items.retain(|i| !matches!(i.status, ItemStatus::Done | ItemStatus::Skipped | ItemStatus::Invalid));
                if app.selected_item().is_none() {
                    app.selected = app.items.first().map(|i| i.id);
                }
            }
        });
        ui.separator();
        let mut action = None;
        let current = app.config.settings.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let n = app.items.len();
            for (idx, item) in app.items.iter_mut().enumerate() {
                let selected = app.selected == Some(item.id);
                let frame = egui::Frame::group(ui.style()).fill(if selected {
                    ui.visuals().selection.bg_fill.gamma_multiply(0.35)
                } else {
                    ui.visuals().faint_bg_color
                });
                // The click-to-select sensor is registered *before* the card's children
                // (via the scope's sense), so the buttons inside sit on top of it and get
                // their own clicks. Registering it afterwards covered every button.
                let card = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                    frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut item.checked, "");
                            let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
                            if let Some(t) = &item.thumb {
                                let s = t.size_vec2();
                                let sc = (56.0 / s.x).min(56.0 / s.y);
                                ui.painter().image(
                                    t.id(),
                                    egui::Rect::from_center_size(rect.center(), s * sc),
                                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                            } else {
                                ui.painter().rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
                            }
                            ui.vertical(|ui| {
                                ui.label(RichText::new(item.name()).strong());
                                if let Some(info) = &item.info {
                                    if let Some(len) = info.length_label() {
                                        ui.label(RichText::new(len).color(Color32::from_rgb(140, 200, 255)));
                                    }
                                    ui.label(RichText::new(info.summary()).small());
                                }
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(item.status.label()).color(status_color(item.status)).small());
                                    if let Some(pinned) = &item.settings {
                                        let differs = *pinned != current;
                                        ui.label(
                                            RichText::new(if differs { t!("queue.pinned_differ") } else { t!("queue.pinned") })
                                                .small()
                                                .weak(),
                                        )
                                        .on_hover_text(t!("queue.pinned_tip"));
                                        if ui.small_button("↺").on_hover_text(t!("queue.unpin_tip")).clicked() {
                                            item.settings = None;
                                        }
                                    }
                                });
                            });
                        });
                        if !item.message.is_empty() && item.status != ItemStatus::Pending {
                            let color = if matches!(item.status, ItemStatus::Failed | ItemStatus::Invalid) {
                                Color32::from_rgb(235, 120, 120)
                            } else {
                                ui.visuals().weak_text_color()
                            };
                            let short: String = item.message.lines().next().unwrap_or("").chars().take(160).collect();
                            ui.label(RichText::new(short).small().color(color)).on_hover_ui(|ui| {
                                ui.set_max_width(600.0);
                                ui.label(RichText::new(&item.message).monospace().small());
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(idx > 0, egui::Button::new("▲").small()).clicked() {
                                action = Some(Action::Move(item.id, -1));
                            }
                            if ui.add_enabled(idx + 1 < n, egui::Button::new("▼").small()).clicked() {
                                action = Some(Action::Move(item.id, 1));
                            }
                            if ui
                                .add_enabled(item.status != ItemStatus::Running, egui::Button::new(t!("queue.remove")).small())
                                .clicked()
                            {
                                action = Some(Action::Remove(item.id));
                            }
                            if item.status == ItemStatus::Done && ui.small_button(t!("queue.show_output")).clicked() {
                                action = Some(Action::OpenOutput(item.id));
                            }
                        });
                    });
                });
                if card.response.clicked() && !selected && action.is_none() {
                    action = Some(Action::Select(item.id));
                }
            }
        });
        match action {
            Some(Action::Select(id)) => {
                app.selected = Some(id);
                app.player = None;
                app.scrub_time = 0.0;
                app.preview_note.clear();
                app.preview_error = None;
                app.canvas.clear();
                if app.config.auto_preview {
                    app.mark_preview_dirty();
                }
            }
            Some(Action::Remove(id)) => app.remove_item(id),
            Some(Action::Move(id, d)) => app.move_item(id, d),
            Some(Action::OpenOutput(id)) => {
                if let Some(o) = app.items.iter().find(|i| i.id == id).and_then(|i| i.output.clone())
                    && let Some(dir) = o.parent() {
                        super::open_path(dir);
                    }
            }
            None => {}
        }
    });
}
