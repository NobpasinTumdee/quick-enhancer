//! Outputs page ("ผลงาน"): everything the app produced (history) plus media found in the
//! output folder, as a grid or list with sort / filter / search and a details panel.

use egui::{Color32, RichText, Vec2};
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::config::{OutputsFilter, OutputsSort};
use crate::history::{EntryKind, OutputRecord, format_date};
use crate::t;
use crate::util::{human_bytes, human_duration};

const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERR: Color32 = Color32::from_rgb(235, 100, 100);

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string())
}

/// Records after filter + search + sort.
pub fn visible(
    records: &[OutputRecord],
    filter: OutputsFilter,
    sort: OutputsSort,
    search: &str,
) -> Vec<OutputRecord> {
    let q = search.trim().to_lowercase();
    let mut v: Vec<OutputRecord> = records
        .iter()
        .filter(|r| match filter {
            OutputsFilter::All => true,
            OutputsFilter::Images => r.kind == EntryKind::Image,
            OutputsFilter::Videos => r.kind == EntryKind::Video,
        })
        .filter(|r| {
            q.is_empty()
                || r.path.to_string_lossy().to_lowercase().contains(&q)
                || r
                    .entry
                    .as_ref()
                    .and_then(|e| e.source.as_ref())
                    .is_some_and(|s| s.to_string_lossy().to_lowercase().contains(&q))
        })
        .cloned()
        .collect();
    match sort {
        OutputsSort::DateDesc => v.sort_by_key(|a| std::cmp::Reverse(a.date)),
        OutputsSort::DateAsc => v.sort_by_key(|a| a.date),
        OutputsSort::NameAsc => v.sort_by_key(|r| file_name(&r.path).to_lowercase()),
        OutputsSort::SizeDesc => v.sort_by_key(|a| std::cmp::Reverse(a.size)),
    }
    v
}

fn sort_label(s: OutputsSort) -> String {
    match s {
        OutputsSort::DateDesc => t!("outputs.sort_newest"),
        OutputsSort::DateAsc => t!("outputs.sort_oldest"),
        OutputsSort::NameAsc => t!("outputs.sort_name"),
        OutputsSort::SizeDesc => t!("outputs.sort_size"),
    }
}

fn filter_label(f: OutputsFilter) -> String {
    match f {
        OutputsFilter::All => t!("outputs.filter_all"),
        OutputsFilter::Images => t!("outputs.filter_images"),
        OutputsFilter::Videos => t!("outputs.filter_videos"),
    }
}

pub fn show_in_explorer(p: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", p.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    if let Some(d) = p.parent() {
        super::open_path(d);
    }
}

enum Act {
    Select(PathBuf),
    Open(PathBuf),
    Reveal(PathBuf),
    Compare(PathBuf, PathBuf),
    AskDelete(PathBuf),
}

pub fn outputs_page(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if !app.outputs.loaded_once && !app.outputs.scanning {
        app.refresh_outputs(&ctx);
    }
    let folders = app.outputs_folders();
    let records = visible(
        &app.outputs.records,
        app.config.outputs.filter,
        app.config.outputs.sort,
        &app.outputs.search,
    );
    let selected = app
        .outputs
        .selected
        .as_ref()
        .and_then(|s| app.outputs.records.iter().find(|r| &r.path == s))
        .cloned();
    let mut act: Option<Act> = None;

    if let Some(rec) = &selected {
        egui::Panel::right("outputs-details")
            .resizable(true)
            .default_size(360.0)
            .min_size(280.0)
            .show(ui, |ui| {
                details(app, ui, rec, &mut act);
            });
    }

    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading(t!("outputs.title"));
            ui.label(format!("({})", records.len()));
            ui.separator();
            ui.selectable_value(&mut app.config.outputs.grid, true, t!("outputs.grid"));
            ui.selectable_value(&mut app.config.outputs.grid, false, t!("outputs.list"));
            ui.separator();
            egui::ComboBox::from_id_salt("out-sort")
                .selected_text(sort_label(app.config.outputs.sort))
                .show_ui(ui, |ui| {
                    for s in [
                        OutputsSort::DateDesc,
                        OutputsSort::DateAsc,
                        OutputsSort::NameAsc,
                        OutputsSort::SizeDesc,
                    ] {
                        ui.selectable_value(&mut app.config.outputs.sort, s, sort_label(s));
                    }
                });
            for f in [OutputsFilter::All, OutputsFilter::Images, OutputsFilter::Videos] {
                ui.selectable_value(&mut app.config.outputs.filter, f, filter_label(f));
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut app.outputs.search)
                    .hint_text(t!("outputs.search"))
                    .desired_width(200.0),
            );
            if ui.button(t!("outputs.refresh")).clicked() {
                app.refresh_outputs(&ctx);
            }
            if app.outputs.scanning {
                ui.spinner();
            }
        });
        if folders.is_empty() {
            ui.label(RichText::new(t!("outputs.history_only")).small().color(WARN));
        }
        for f in &folders {
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 120.0).max(80.0);
                let text = t!("outputs.scanning_folder", folder = f.display());
                elided_label(ui, &text, w, |t| RichText::new(t).small().weak());
                if ui.small_button(t!("common.open_folder")).clicked() {
                    super::open_path(f);
                }
            });
        }
        ui.separator();
        if records.is_empty() {
            ui.centered_and_justified(|ui| {
                let msg = if app.outputs.scanning {
                    t!("outputs.loading")
                } else {
                    t!("outputs.empty")
                };
                ui.label(RichText::new(msg).size(16.0).weak());
            });
            return;
        }
        if app.config.outputs.grid {
            grid(app, ui, &records, &mut act);
        } else {
            list(app, ui, &records, &mut act);
        }
    });

    delete_modal(app, &ctx);

    match act {
        Some(Act::Select(p)) => app.outputs.selected = Some(p),
        Some(Act::Open(p)) => super::open_path(&p),
        Some(Act::Reveal(p)) => show_in_explorer(&p),
        Some(Act::Compare(src, out)) => app.compare_with_source(src, out),
        Some(Act::AskDelete(p)) => app.outputs.confirm_delete = Some(p),
        None => {}
    }
}

fn thumb_box(app: &mut App, ui: &mut egui::Ui, r: &OutputRecord, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    if let Some(t) = app.outputs.thumb(r) {
        let s = t.size_vec2();
        let sc = (size / s.x).min(size / s.y);
        ui.painter().image(
            t.id(),
            egui::Rect::from_center_size(rect.center(), s * sc),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        let icon = if !r.exists {
            "⚠"
        } else if r.kind == EntryKind::Video {
            "🎞"
        } else {
            "🖼"
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(size * 0.3),
            ui.visuals().weak_text_color(),
        );
    }
    if r.kind == EntryKind::Video && r.exists {
        let dur = r
            .entry
            .as_ref()
            .and_then(|e| e.duration_secs)
            .or_else(|| {
                app.outputs
                    .info
                    .get(&crate::history::path_key(&r.path))
                    .and_then(|i| i.duration)
            });
        if let Some(d) = dur {
            crate::preview::player::overlay(
                ui.painter(),
                rect.right_bottom() + Vec2::new(-6.0, -6.0),
                egui::Align2::RIGHT_BOTTOM,
                &human_duration(d),
                11.0,
            );
        }
    }
}

/// Thumbnail size inside a grid card.
pub const CARD: f32 = 176.0;
/// Card padding and gap between cards.
const CARD_PAD: f32 = 6.0;
const CARD_GAP: f32 = 10.0;
/// Outer card size (thumb + padding; name + info lines below the thumb).
pub const CARD_W: f32 = CARD + 2.0 * CARD_PAD;
pub const CARD_H: f32 = CARD + 2.0 * CARD_PAD + 40.0;

/// Number of grid columns that fit in `avail` (at least 1).
pub fn grid_columns(avail: f32, card_w: f32, gap: f32) -> usize {
    (((avail + gap) / (card_w + gap)).floor() as usize).max(1)
}

/// Shorten `s` to at most `max` chars by replacing the middle with "…".
pub fn elide_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max < 3 {
        return s.chars().take(max).collect();
    }
    let keep = max - 1;
    let head = keep / 2;
    let tail = keep - head;
    let h: String = s.chars().take(head).collect();
    let t: String = s.chars().skip(n - tail).collect();
    format!("{h}…{t}")
}

/// Middle-elided label that fits `width` (full text on hover). Clickable.
pub fn elided_label(
    ui: &mut egui::Ui,
    text: &str,
    width: f32,
    rich: impl Fn(String) -> RichText,
) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let ctx = ui.ctx().clone();
    let fits = |t: &str| {
        ctx.fonts_mut(|f| f.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x)
            <= width
    };
    let shown = if fits(text) {
        text.to_string()
    } else {
        // Longest middle-elided string that fits.
        let (mut lo, mut hi) = (3usize, text.chars().count());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if fits(&elide_middle(text, mid)) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        elide_middle(text, lo)
    };
    let elided = shown != text;
    let r = ui.add_sized(
        [width, ui.spacing().interact_size.y],
        egui::Label::new(rich(shown)).truncate().sense(egui::Sense::click()),
    );
    if elided { r.on_hover_text(text) } else { r }
}

fn grid(app: &mut App, ui: &mut egui::Ui, records: &[OutputRecord], act: &mut Option<Act>) {
    let cols = grid_columns(ui.available_width(), CARD_W, CARD_GAP);
    let rows = records.len().div_ceil(cols);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, CARD_H, rows, |ui, range| {
            ui.spacing_mut().item_spacing = Vec2::new(CARD_GAP, CARD_GAP);
            for row in range {
                ui.horizontal(|ui| {
                    for r in records.iter().skip(row * cols).take(cols) {
                        card(app, ui, r, act);
                    }
                });
            }
        });
}

/// One fixed-size grid card. Only cards in visible rows are drawn, so thumbnails are
/// requested lazily for what is on screen.
fn card(app: &mut App, ui: &mut egui::Ui, r: &OutputRecord, act: &mut Option<Act>) {
    let selected = app.outputs.selected.as_ref() == Some(&r.path);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(CARD_W, CARD_H), egui::Sense::click());
    let fill = if selected {
        ui.visuals().selection.bg_fill.gamma_multiply(0.35)
    } else {
        ui.visuals().faint_bg_color
    };
    ui.painter().rect_filled(rect, 6.0, fill);
    if resp.hovered() {
        ui.painter().rect_stroke(
            rect,
            6.0,
            ui.visuals().widgets.hovered.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    let inner = rect.shrink(CARD_PAD);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.y = 3.0;
    thumb_box(app, &mut child, r, CARD);
    let mut name = RichText::new(file_name(&r.path)).strong();
    if !r.exists {
        name = name.strikethrough().color(ERR);
    }
    child.add_sized([CARD, 18.0], egui::Label::new(name).truncate());
    let mut line = if r.exists {
        format!("{} · {}", format_date(r.date), human_bytes(r.size))
    } else {
        format!("{} · {}", format_date(r.date), t!("outputs.missing"))
    };
    if r.entry.as_ref().is_some_and(|e| e.edited) {
        line = format!("✂ {} · {line}", t!("outputs.edited"));
    }
    child.add_sized(
        [CARD, 14.0],
        egui::Label::new(RichText::new(line).small().weak()).truncate(),
    );
    let resp = resp.on_hover_text(r.path.display().to_string());
    if resp.clicked() {
        *act = Some(Act::Select(r.path.clone()));
    }
    if resp.double_clicked() && r.exists {
        *act = Some(Act::Open(r.path.clone()));
    }
}

fn list(app: &mut App, ui: &mut egui::Ui, records: &[OutputRecord], act: &mut Option<Act>) {
    const ROW_H: f32 = 46.0;
    const THUMB: f32 = 40.0;
    const TYPE_W: f32 = 70.0;
    const SIZE_W: f32 = 80.0;
    const DATE_W: f32 = 130.0;
    const GAP: f32 = 10.0;
    // Scrollbar + margins.
    let flex = (ui.available_width() - THUMB - TYPE_W - SIZE_W - DATE_W - GAP * 6.0 - 16.0)
        .max(120.0);
    let name_w = (flex * 0.45).max(60.0);
    let dir_w = (flex - name_w).max(60.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.add_space(THUMB);
        for (t, w) in [
            (t!("outputs.col_name"), name_w),
            (t!("outputs.col_folder"), dir_w),
            (t!("outputs.col_type"), TYPE_W),
            (t!("outputs.col_size"), SIZE_W),
            (t!("outputs.col_date"), DATE_W),
        ] {
            ui.add_sized([w, 18.0], egui::Label::new(RichText::new(t).strong()).truncate());
        }
    });
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, records.len(), |ui, range| {
            for (i, r) in records.iter().enumerate().skip(range.start).take(range.len()) {
                let selected = app.outputs.selected.as_ref() == Some(&r.path);
                let row = ui.horizontal(|ui| {
                    ui.set_height(ROW_H - ui.spacing().item_spacing.y);
                    ui.spacing_mut().item_spacing.x = GAP;
                    thumb_box(app, ui, r, THUMB);
                    let exists = r.exists;
                    let style = move |t: String| {
                        let mut n = RichText::new(t);
                        if selected {
                            n = n.strong();
                        }
                        if !exists {
                            n = n.strikethrough().color(ERR);
                        }
                        n
                    };
                    let mut resps = vec![elided_label(ui, &file_name(&r.path), name_w, style)];
                    let dir = r
                        .path
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default();
                    resps.push(elided_label(ui, &dir, dir_w, |t| RichText::new(t).weak()));
                    let mut kind = match r.kind {
                        EntryKind::Image => t!("outputs.kind_image"),
                        EntryKind::Video => t!("outputs.kind_video"),
                    };
                    if r.entry.as_ref().is_some_and(|e| e.edited) {
                        kind = format!("{kind} ✂");
                    }
                    ui.add_sized([TYPE_W, 18.0], egui::Label::new(kind).truncate());
                    let size = if r.exists { human_bytes(r.size) } else { t!("outputs.missing") };
                    ui.add_sized([SIZE_W, 18.0], egui::Label::new(size).truncate());
                    ui.add_sized([DATE_W, 18.0], egui::Label::new(format_date(r.date)).truncate());
                    resps
                });
                let row_rect = row.response.rect;
                if selected {
                    ui.painter().rect_filled(
                        row_rect,
                        3.0,
                        ui.visuals().selection.bg_fill.gamma_multiply(0.25),
                    );
                }
                let bg = ui.interact(row_rect, ui.scope_id().with(("out-row", i)), egui::Sense::click());
                let clicked = bg.clicked() || row.inner.iter().any(|r| r.clicked());
                let dbl = bg.double_clicked() || row.inner.iter().any(|r| r.double_clicked());
                if clicked {
                    *act = Some(Act::Select(r.path.clone()));
                }
                if dbl && r.exists {
                    *act = Some(Act::Open(r.path.clone()));
                }
            }
        });
}

fn row(ui: &mut egui::Ui, k: String, v: impl Into<String>) {
    ui.label(RichText::new(k).weak());
    ui.add(egui::Label::new(v.into()).wrap());
    ui.end_row();
}

fn details(app: &mut App, ui: &mut egui::Ui, r: &OutputRecord, act: &mut Option<Act>) {
    ui.heading(t!("outputs.details"));
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.vertical_centered(|ui| thumb_box(app, ui, r, 300.0_f32.min(ui.available_width())));
        ui.add_space(6.0);
        ui.label(RichText::new(file_name(&r.path)).strong());
        if !r.exists {
            ui.colored_label(ERR, t!("outputs.missing_long"));
        }
        let info = app
            .outputs
            .info
            .get(&crate::history::path_key(&r.path))
            .cloned();
        egui::Grid::new("out-details").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            row(ui, t!("outputs.path"), r.path.display().to_string());
            if r.exists {
                row(ui, t!("outputs.col_size"), human_bytes(r.size));
            }
            row(ui, t!("outputs.col_date"), format_date(r.date));
            let e = r.entry.as_ref();
            let (w, h) = info
                .as_ref()
                .map(|i| (i.width, i.height))
                .or_else(|| e.map(|e| (e.width, e.height)))
                .unwrap_or((0, 0));
            if w > 0 {
                row(ui, t!("outputs.resolution"), format!("{w}x{h}"));
            }
            if let Some(d) = info.as_ref().and_then(|i| i.duration).or(e.and_then(|e| e.duration_secs)) {
                row(ui, t!("outputs.duration"), human_duration(d));
            }
            if let Some(f) = info.as_ref().and_then(|i| i.fps) {
                row(ui, "FPS".into(), f.to_string());
            }
            if let Some(e) = e {
                if let Some(s) = &e.source {
                    row(ui, t!("outputs.source"), s.display().to_string());
                }
                row(ui, t!("outputs.profile"), e.profile.clone());
                row(ui, t!("outputs.scale"), e.scale.clone());
                if let Some(f) = &e.fps {
                    row(ui, t!("outputs.framerate"), f.clone());
                }
                if let Some(enc) = &e.encoder {
                    row(ui, t!("outputs.encoder"), enc.clone());
                }
                if e.processing_secs > 0.0 {
                    row(ui, t!("outputs.processing_time"), human_duration(e.processing_secs));
                }
            } else {
                row(ui, t!("outputs.origin"), t!("outputs.found_in_folder"));
            }
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(r.exists, egui::Button::new(t!("outputs.open"))).clicked() {
                *act = Some(Act::Open(r.path.clone()));
            }
            if ui.add_enabled(r.exists, egui::Button::new(t!("outputs.reveal"))).clicked() {
                *act = Some(Act::Reveal(r.path.clone()));
            }
            let source = r.entry.as_ref().and_then(|e| e.source.clone());
            let can_cmp = r.exists && source.as_ref().is_some_and(|s| s.exists());
            let b = ui
                .add_enabled(can_cmp, egui::Button::new(t!("outputs.compare")))
                .on_disabled_hover_text(t!("outputs.compare_disabled"));
            if b.clicked()
                && let Some(s) = source
            {
                *act = Some(Act::Compare(s, r.path.clone()));
            }
            let del = if r.exists { t!("outputs.delete") } else { t!("outputs.remove_entry") };
            if ui.button(RichText::new(del).color(ERR)).clicked() {
                *act = Some(Act::AskDelete(r.path.clone()));
            }
        });
    });
}

fn delete_modal(app: &mut App, ctx: &egui::Context) {
    let Some(path) = app.outputs.confirm_delete.clone() else {
        return;
    };
    let exists = path.exists();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("outputs-delete")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(if exists { t!("outputs.delete_title") } else { t!("outputs.remove_title") });
        ui.label(RichText::new(path.display().to_string()).monospace());
        ui.label(if exists { t!("outputs.delete_body") } else { t!("outputs.remove_body") });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let label = if exists { t!("outputs.delete_confirm") } else { t!("outputs.remove_entry") };
            if ui.button(RichText::new(label).color(ERR)).clicked() {
                answer = Some(true);
            }
            if ui.button(t!("common.cancel")).clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            app.outputs.confirm_delete = None;
            match crate::outputs::delete_output(&path, &app.paths.history_file) {
                Ok(()) => {
                    app.outputs.forget(&path);
                    app.toast(t!("outputs.deleted", name = file_name(&path)));
                }
                Err(e) => app.toast(t!("outputs.delete_failed", error = e)),
            }
        }
        Some(false) => app.outputs.confirm_delete = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_wrap_to_width() {
        assert_eq!(grid_columns(100.0, CARD_W, 10.0), 1);
        assert_eq!(grid_columns(0.0, CARD_W, 10.0), 1);
        assert_eq!(grid_columns(CARD_W * 2.0 + 10.0, CARD_W, 10.0), 2);
        assert_eq!(grid_columns(CARD_W * 2.0 + 9.0, CARD_W, 10.0), 1);
        // ~800 px window minus margins.
        assert_eq!(grid_columns(780.0, CARD_W, 10.0), 3);
    }

    #[test]
    fn middle_elision() {
        assert_eq!(elide_middle("short", 10), "short");
        let e = elide_middle("C:/very/long/path/to/some/file.mp4", 12);
        assert_eq!(e.chars().count(), 12);
        assert!(e.starts_with("C:/v") && e.ends_with(".mp4") && e.contains('…'));
        // Thai (multi-byte) is cut on char boundaries.
        let th = elide_middle("วิดีโอที่ปรับปรุงแล้วยาวมาก.mp4", 9);
        assert_eq!(th.chars().count(), 9);
    }
}
