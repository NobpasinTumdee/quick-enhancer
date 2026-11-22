//! Thai rendering test window (Settings → Application → "Thai font test", or
//! `QuickEnhancer.exe --font-test`). Shows the tricky combinations — stacked tone marks,
//! upper/lower vowels, SARA AM — at several sizes so broken shaping is obvious.

use egui::RichText;

use crate::app::App;
use crate::t;

pub const SAMPLES: [&str; 5] = ["ที่", "กิ่ง", "น้ำ", "ปู่", "ญี่ปุ่น"];

const SENTENCES: [&str; 3] = [
    "ภาษาไทยแสดงผลถูกต้อง: ที่ กิ่ง น้ำ ปู่ ญี่ปุ่น ฤๅษี ฦๅ ปั้น ฝั่ง ซื่อสัตย์",
    "เพิ่มความละเอียดวิดีโอด้วย GPU · VRAM 8 GB · 60 FPS · CRF 18 · NVENC",
    "Mixed: Upscale 4x → ภาพคมชัดขึ้น (Tile 256)",
];

pub fn font_test_window(app: &mut App, ctx: &egui::Context) {
    if !app.show_font_test {
        return;
    }
    let mut open = true;
    egui::Window::new(t!("fonttest.title"))
        .open(&mut open)
        .resizable(true)
        .default_width(640.0)
        .show(ctx, |ui| {
            ui.label(RichText::new(t!("fonttest.hint")).small().weak());
            ui.separator();
            for size in [12.0, 14.0, 18.0, 24.0, 36.0] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{size:>2.0}px")).monospace().weak());
                    for s in SAMPLES {
                        ui.label(RichText::new(s).size(size));
                        ui.add_space(size * 0.4);
                    }
                });
            }
            ui.separator();
            for s in SENTENCES {
                ui.label(RichText::new(s).size(16.0));
            }
            ui.label(RichText::new(SENTENCES[0]).monospace().size(14.0));
            ui.separator();
            ui.label(RichText::new(t!("fonttest.credit")).small().weak());
        });
    if !open {
        app.show_font_test = false;
    }
}
