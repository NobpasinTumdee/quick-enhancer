//! Before/after canvas: wipe slider, side-by-side (synced), toggle; zoom around cursor,
//! drag to pan, Fit / 100 %. The original is drawn scaled to the output resolution so both
//! sides align pixel-for-pixel; nearest-neighbour sampling kicks in at high zoom.

use egui::{
    Align2, Color32, ColorImage, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions,
    Vec2,
};
use image::RgbImage;
use std::sync::Arc;

use crate::engines::resize::{ResizeKind, fit_within, resize_rgb};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CompareMode {
    #[default]
    Wipe,
    SideBySide,
    Toggle,
}

struct Tex {
    handle: TextureHandle,
    image: Arc<ColorImage>,
    /// Source pixel size (before any texture-size reduction).
    px: [u32; 2],
    nearest: bool,
}

impl Tex {
    fn new(ctx: &egui::Context, name: &str, img: &RgbImage, max_side: usize) -> Tex {
        let px = [img.width(), img.height()];
        let (w, h) = fit_within(img.width(), img.height(), max_side as u32, max_side as u32);
        let color = if (w, h) != (img.width(), img.height()) {
            super::to_color_image(&resize_rgb(img, w, h, ResizeKind::Lanczos3))
        } else {
            super::to_color_image(img)
        };
        let image = Arc::new(color);
        let handle = ctx.load_texture(name, image.clone(), TextureOptions::LINEAR);
        Tex {
            handle,
            image,
            px,
            nearest: false,
        }
    }

    /// Switch sampling to nearest when one source pixel covers ≥ 2 screen pixels.
    fn update_filter(&mut self, screen_px_per_src_px: f32) {
        let want = screen_px_per_src_px >= 2.0;
        if want != self.nearest {
            self.nearest = want;
            let opts = if want {
                TextureOptions {
                    magnification: egui::TextureFilter::Nearest,
                    ..TextureOptions::LINEAR
                }
            } else {
                TextureOptions::LINEAR
            };
            self.handle.set(self.image.clone(), opts);
        }
    }
}

#[derive(Default)]
pub struct PreviewCanvas {
    pub mode: CompareMode,
    pub show_after: bool,
    zoom: f32,
    /// View centre in output-image pixel coordinates.
    center: Vec2,
    wipe: f32,
    dragging_divider: bool,
    fit_pending: bool,
    before: Option<Tex>,
    after: Option<Tex>,
    /// Output image size (the shared coordinate space).
    size: Vec2,
    /// Captions for the two sides (`None` = Original / Enhanced).
    labels: Option<(String, String)>,
}

impl PreviewCanvas {
    pub fn new() -> Self {
        Self {
            wipe: 0.5,
            show_after: true,
            fit_pending: true,
            zoom: 1.0,
            ..Default::default()
        }
    }

    pub fn has_images(&self) -> bool {
        self.after.is_some()
    }

    pub fn clear(&mut self) {
        self.before = None;
        self.after = None;
    }

    /// Like `set_images`, with captions for the two sides (e.g. Faithful / Creative).
    pub fn set_images_labeled(
        &mut self,
        ctx: &egui::Context,
        before: &RgbImage,
        after: &RgbImage,
        labels: Option<(String, String)>,
    ) {
        self.set_images(ctx, before, after);
        self.labels = labels;
    }

    pub fn set_images(&mut self, ctx: &egui::Context, before: &RgbImage, after: &RgbImage) {
        self.labels = None;
        let max_side = ctx.input(|i| i.max_texture_side).max(2048);
        let new_size = Vec2::new(after.width() as f32, after.height() as f32);
        if new_size != self.size {
            self.fit_pending = true;
        }
        self.size = new_size;
        self.before = Some(Tex::new(ctx, "preview-before", before, max_side));
        self.after = Some(Tex::new(ctx, "preview-after", after, max_side));
    }

    pub fn fit(&mut self) {
        self.fit_pending = true;
    }

    pub fn actual_size(&mut self) {
        self.zoom = 1.0;
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn toggle(&mut self) {
        self.mode = CompareMode::Toggle;
        self.show_after = !self.show_after;
    }

    fn img_rect(&self, view: Rect) -> Rect {
        Rect::from_min_size(
            view.center() - self.center * self.zoom,
            self.size * self.zoom,
        )
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        let (resp, painter) = ui.allocate_painter(avail, Sense::click_and_drag());
        let rect = resp.rect;
        painter.rect_filled(rect, 0.0, Color32::from_gray(18));
        let (Some(_), Some(_)) = (&self.before, &self.after) else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                crate::t!("preview.none_yet"),
                FontId::proportional(16.0),
                Color32::GRAY,
            );
            return;
        };
        let views: Vec<Rect> = match self.mode {
            CompareMode::SideBySide => {
                let half = rect.width() / 2.0;
                vec![
                    Rect::from_min_size(rect.min, Vec2::new(half - 1.0, rect.height())),
                    Rect::from_min_size(
                        rect.min + Vec2::new(half + 1.0, 0.0),
                        Vec2::new(half - 1.0, rect.height()),
                    ),
                ]
            }
            _ => vec![rect],
        };
        if self.fit_pending && self.size.x > 0.0 {
            let v = views[0];
            self.zoom = (v.width() / self.size.x)
                .min(v.height() / self.size.y)
                .max(0.01);
            self.center = self.size / 2.0;
            self.fit_pending = false;
        }

        // --- input --------------------------------------------------------------------
        let hover = resp.hover_pos();
        if let Some(cursor) = hover {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (scroll * 0.0015).exp() * pinch;
            if (factor - 1.0).abs() > 1e-4 {
                let view = views
                    .iter()
                    .find(|v| v.contains(cursor))
                    .copied()
                    .unwrap_or(rect);
                let img_pt = self.center + (cursor - view.center()) / self.zoom;
                self.zoom = (self.zoom * factor).clamp(0.02, 64.0);
                self.center = img_pt - (cursor - view.center()) / self.zoom;
            }
        }
        let divider_x = rect.left() + self.wipe * rect.width();
        if resp.drag_started() {
            self.dragging_divider = self.mode == CompareMode::Wipe
                && resp
                    .interact_pointer_pos()
                    .is_some_and(|p| (p.x - divider_x).abs() < 10.0);
        }
        if resp.dragged() {
            if self.dragging_divider {
                if let Some(p) = resp.interact_pointer_pos() {
                    self.wipe = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                }
            } else {
                self.center -= resp.drag_delta() / self.zoom;
            }
        }
        if resp.drag_stopped() {
            self.dragging_divider = false;
        }
        if resp.double_clicked() {
            self.fit_pending = true;
        }
        if hover.is_some_and(|p| self.mode == CompareMode::Wipe && (p.x - divider_x).abs() < 10.0)
            || self.dragging_divider
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        } else if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }

        // --- texture filtering ----------------------------------------------------------
        let zoom = self.zoom;
        let size = self.size;
        if let Some(t) = self.after.as_mut() {
            t.update_filter(zoom * size.x / t.px[0] as f32);
        }
        if let Some(t) = self.before.as_mut() {
            t.update_filter(zoom * size.x / t.px[0] as f32);
        }
        let res = |t: &Option<Tex>| t.as_ref().map(|t| format!("{}x{}", t.px[0], t.px[1])).unwrap_or_default();
        let (orig_name, enh_name) = self
            .labels
            .clone()
            .unwrap_or_else(|| (crate::t!("preview.original"), crate::t!("preview.enhanced")));
        let orig_label = format!("{orig_name} · {}", res(&self.before));
        let enh_label = format!("{enh_name} · {}", res(&self.after));
        let before = self.before.as_ref().expect("checked").handle.id();
        let after = self.after.as_ref().expect("checked").handle.id();
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let label = |p: &egui::Painter, pos: Pos2, align: Align2, text: &str| {
            let galley_rect = p.text(pos, align, text, FontId::proportional(13.0), Color32::WHITE);
            p.rect_filled(galley_rect.expand(3.0), 3.0, Color32::from_black_alpha(140));
            p.text(pos, align, text, FontId::proportional(13.0), Color32::WHITE);
        };

        // --- drawing --------------------------------------------------------------------
        match self.mode {
            CompareMode::Wipe => {
                let ir = self.img_rect(rect);
                let left = Rect::from_min_max(rect.min, Pos2::new(divider_x, rect.max.y));
                let right = Rect::from_min_max(Pos2::new(divider_x, rect.min.y), rect.max);
                painter
                    .with_clip_rect(left)
                    .image(before, ir, uv, Color32::WHITE);
                painter
                    .with_clip_rect(right)
                    .image(after, ir, uv, Color32::WHITE);
                painter.line_segment(
                    [
                        Pos2::new(divider_x, rect.top()),
                        Pos2::new(divider_x, rect.bottom()),
                    ],
                    Stroke::new(2.0, Color32::WHITE),
                );
                painter.circle_filled(Pos2::new(divider_x, rect.center().y), 7.0, Color32::WHITE);
                label(
                    &painter,
                    rect.left_top() + Vec2::new(8.0, 8.0),
                    Align2::LEFT_TOP,
                    &orig_label,
                );
                label(
                    &painter,
                    rect.right_top() + Vec2::new(-8.0, 8.0),
                    Align2::RIGHT_TOP,
                    &enh_label,
                );
            }
            CompareMode::SideBySide => {
                for (i, v) in views.iter().enumerate() {
                    let ir = self.img_rect(*v);
                    let tex = if i == 0 { before } else { after };
                    painter
                        .with_clip_rect(*v)
                        .image(tex, ir, uv, Color32::WHITE);
                    label(
                        &painter,
                        v.left_top() + Vec2::new(8.0, 8.0),
                        Align2::LEFT_TOP,
                        if i == 0 { &orig_label } else { &enh_label },
                    );
                }
                painter.line_segment(
                    [
                        Pos2::new(rect.center().x, rect.top()),
                        Pos2::new(rect.center().x, rect.bottom()),
                    ],
                    Stroke::new(2.0, Color32::from_gray(60)),
                );
            }
            CompareMode::Toggle => {
                let ir = self.img_rect(rect);
                let tex = if self.show_after { after } else { before };
                painter
                    .with_clip_rect(rect)
                    .image(tex, ir, uv, Color32::WHITE);
                label(
                    &painter,
                    rect.left_top() + Vec2::new(8.0, 8.0),
                    Align2::LEFT_TOP,
                    &if self.show_after {
                        format!("{enh_label} — {}", crate::t!("preview.space_toggle"))
                    } else {
                        format!("{orig_label} — {}", crate::t!("preview.space_toggle"))
                    },
                );
            }
        }
        label(
            &painter,
            rect.left_bottom() + Vec2::new(8.0, -8.0),
            Align2::LEFT_BOTTOM,
            &format!(
                "{:.0}%  ·  {}x{}",
                self.zoom * 100.0,
                size.x as u32,
                size.y as u32
            ),
        );
    }
}
