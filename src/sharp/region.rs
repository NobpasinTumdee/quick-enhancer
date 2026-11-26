//! Preview regions for "Make sharp": the default box (a detected face, else the centre),
//! clamping, and the settings pair for the Faithful-vs-Creative comparison.

use crate::config::JobSettings;

/// Default preview box side in source pixels (a few seconds faithful, about a minute generative).
pub const DEFAULT_SIDE: u32 = 512;
/// Smallest box the user can draw.
pub const MIN_SIDE: u32 = 64;

/// Clamp `[x, y, w, h]` into a `iw x ih` image (at least 16 px a side, inside the image).
pub fn clamp(r: [u32; 4], iw: u32, ih: u32) -> [u32; 4] {
    let w = r[2].clamp(16.min(iw), iw.max(1));
    let h = r[3].clamp(16.min(ih), ih.max(1));
    let x = r[0].min(iw.saturating_sub(w));
    let y = r[1].min(ih.saturating_sub(h));
    [x, y, w, h]
}

/// A `side`-sized box centred on `(cx, cy)`, clamped into the image.
pub fn around(cx: f32, cy: f32, side: u32, iw: u32, ih: u32) -> [u32; 4] {
    let (w, h) = (side.min(iw), side.min(ih));
    let x = (cx - w as f32 / 2.0).max(0.0) as u32;
    let y = (cy - h as f32 / 2.0).max(0.0) as u32;
    clamp([x, y, w, h], iw, ih)
}

/// Default preview box: around the largest face (`[x, y, w, h]` boxes in source pixels) with
/// room for hair and shoulders, else the image centre.
pub fn default_region(iw: u32, ih: u32, faces: &[[f32; 4]]) -> [u32; 4] {
    match faces.iter().max_by(|a, b| (a[2] * a[3]).total_cmp(&(b[2] * b[3]))) {
        Some(f) => {
            let side = ((f[2].max(f[3]) * 2.2) as u32).clamp(DEFAULT_SIDE / 2, DEFAULT_SIDE * 2);
            around(f[0] + f[2] / 2.0, f[1] + f[3] / 2.0, side, iw, ih)
        }
        None => around(iw as f32 / 2.0, ih as f32 / 2.0, DEFAULT_SIDE, iw, ih),
    }
}

/// Box from a drag between two points (source pixels); a click (tiny drag) centres a default box.
pub fn from_drag(a: (f32, f32), b: (f32, f32), iw: u32, ih: u32) -> [u32; 4] {
    let (x0, x1) = (a.0.min(b.0).max(0.0), a.0.max(b.0).min(iw as f32));
    let (y0, y1) = (a.1.min(b.1).max(0.0), a.1.max(b.1).min(ih as f32));
    if x1 - x0 < MIN_SIDE as f32 && y1 - y0 < MIN_SIDE as f32 {
        return around(b.0, b.1, DEFAULT_SIDE, iw, ih);
    }
    let w = ((x1 - x0) as u32).max(MIN_SIDE);
    let h = ((y1 - y0) as u32).max(MIN_SIDE);
    clamp([x0 as u32, y0 as u32, w, h], iw, ih)
}

/// Preview cache key part for a box.
pub fn key(r: [u32; 4]) -> u64 {
    (r[0] as u64) << 48 ^ (r[1] as u64) << 32 ^ (r[2] as u64) << 16 ^ r[3] as u64
}

/// Settings for the left (Faithful only) and right (Creative) side of the comparison. The right
/// side uses the chosen Creative amount, or 50 % when it is at 0.
pub fn compare_settings(s: &JobSettings) -> (JobSettings, JobSettings) {
    let mut faithful = s.clone();
    faithful.sharp.creative_auto = false;
    faithful.sharp.creative = 0.0;
    let mut creative = s.clone();
    creative.sharp.creative_auto = false;
    let chosen = if s.sharp.creative_auto { 0.0 } else { s.sharp.creative };
    creative.sharp.creative = if chosen > 0.0 { chosen } else { 0.5 };
    (faithful, creative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_boxes_inside() {
        assert_eq!(clamp([900, 900, 300, 300], 1000, 1000), [700, 700, 300, 300]);
        assert_eq!(clamp([0, 0, 5000, 10], 800, 600), [0, 0, 800, 16]);
        assert_eq!(clamp([10, 10, 100, 100], 8, 8), [0, 0, 8, 8]);
    }

    #[test]
    fn default_box_prefers_the_largest_face() {
        let r = default_region(2000, 1500, &[[100.0, 100.0, 50.0, 50.0], [1200.0, 600.0, 200.0, 220.0]]);
        // Centred on the big face (1300, 710), 484 px wide.
        assert!(r[0] < 1300 && r[0] + r[2] > 1300 && r[1] < 710 && r[1] + r[3] > 710, "{r:?}");
        assert_eq!(r[2], 484);
        assert_eq!(default_region(2000, 1000, &[]), [744, 244, 512, 512]);
        assert_eq!(default_region(300, 200, &[]), [0, 0, 300, 200]);
    }

    #[test]
    fn drag_and_click() {
        assert_eq!(from_drag((10.0, 20.0), (210.0, 120.0), 1000, 1000), [10, 20, 200, 100]);
        assert_eq!(from_drag((210.0, 120.0), (10.0, 20.0), 1000, 1000), [10, 20, 200, 100]);
        // A click centres the default box.
        assert_eq!(from_drag((500.0, 500.0), (502.0, 501.0), 2000, 2000), [246, 245, 512, 512]);
    }

    #[test]
    fn compare_pair() {
        let mut s = JobSettings { sharp_on: true, ..Default::default() };
        let (a, b) = compare_settings(&s);
        assert_eq!((a.sharp.creative, b.sharp.creative), (0.0, 0.5));
        assert!(!a.sharp.creative_auto && !b.sharp.creative_auto);
        s.sharp.creative_auto = false;
        s.sharp.creative = 0.8;
        assert_eq!(compare_settings(&s).1.sharp.creative, 0.8);
    }
}
