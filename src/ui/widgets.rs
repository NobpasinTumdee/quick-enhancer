//! Small reusable widgets: sparkline gauges for the resource monitor.

use egui::{Color32, Pos2, RichText, Sense, Stroke, Vec2};
use std::collections::VecDeque;

use crate::app::App;
use crate::monitor::HISTORY;
use crate::t;
use crate::util::human_bytes;

/// 60-second sparkline (values 0..=100).
pub fn sparkline(ui: &mut egui::Ui, hist: &VecDeque<f32>, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(120.0, 28.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    if hist.len() >= 2 {
        let n = HISTORY as f32 - 1.0;
        let offset = HISTORY - hist.len();
        let pts: Vec<Pos2> = hist
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = rect.left() + (i + offset) as f32 / n * rect.width();
                let y = rect.bottom() - v.clamp(0.0, 100.0) / 100.0 * (rect.height() - 2.0) - 1.0;
                Pos2::new(x, y)
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
    }
}

fn gauge(ui: &mut egui::Ui, title: &str, value: String, hist: &VecDeque<f32>, color: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(title).small().weak());
        ui.label(RichText::new(value).monospace());
        sparkline(ui, hist, color);
    });
}

pub fn monitor_strip(app: &mut App, ui: &mut egui::Ui) {
    let s = app.monitor.snapshot();
    gauge(
        ui,
        "CPU",
        format!("{:5.1}%", s.cpu),
        &s.cpu_hist,
        Color32::from_rgb(90, 170, 250),
    );
    ui.separator();
    gauge(
        ui,
        "RAM",
        format!("{} / {}", human_bytes(s.ram_used), human_bytes(s.ram_total)),
        &s.ram_hist,
        Color32::from_rgb(120, 210, 120),
    );
    ui.separator();
    match &s.gpu {
        Some(g) => {
            gauge(
                ui,
                &format!(
                    "GPU · {} ({})",
                    s.gpu_name.clone().unwrap_or_default(),
                    g.provider
                ),
                format!("{:5.1}%", g.util),
                &s.gpu_hist,
                Color32::from_rgb(240, 150, 70),
            );
            ui.separator();
            gauge(
                ui,
                "VRAM",
                format!(
                    "{} / {}",
                    human_bytes(g.vram_used),
                    human_bytes(g.vram_total)
                ),
                &s.vram_hist,
                Color32::from_rgb(200, 120, 230),
            );
        }
        None => {
            ui.vertical(|ui| {
                ui.label(RichText::new("GPU").small().weak());
                let why = match &s.gpu_name {
                    Some(n) => t!("monitor.no_stats", gpu = n),
                    None => t!("monitor.no_gpu"),
                };
                ui.label(RichText::new(why).weak());
            });
        }
    }
}
