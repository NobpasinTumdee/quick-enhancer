//! Presets → stage plan for "Make sharp (AI)" (pure logic, no models loaded here).
//!
//! Stages: deblur (model chosen by blur type) → smart downscale to the effective sharp
//! resolution → rebuild detail (faithful SR, and the generative model when Creative > 0) →
//! target size → faces → light sharpen. The numbers come from `docs/DEBLUR_EVAL.md`.

use serde::{Deserialize, Serialize};

use super::analysis::{BlurKind, BlurReport};

/// Catalog ids of the stage models.
pub const DEBLUR_MOTION: &str = "restormer-motion";
pub const DEBLUR_DEFOCUS: &str = "restormer-defocus";
pub const FAITHFUL_SR: &str = "realplksr-nomos2-4x";
pub const GENERATIVE: &str = "seedvr2-3b";
/// Model id value meaning "skip this stage".
pub const NONE: &str = "none";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Decide from the blur analysis.
    #[default]
    Auto,
    LightBlur,
    StrongBlur,
    /// Camera shake / motion blur.
    Motion,
    OutOfFocus,
    /// Old / low-resolution photo.
    OldPhoto,
    /// Portrait (faces).
    Portrait,
}

impl Preset {
    pub const ALL: [Preset; 7] = [
        Preset::Auto,
        Preset::LightBlur,
        Preset::StrongBlur,
        Preset::Motion,
        Preset::OutOfFocus,
        Preset::OldPhoto,
        Preset::Portrait,
    ];
    pub fn label_key(self) -> &'static str {
        match self {
            Preset::Auto => "sharp.preset_auto",
            Preset::LightBlur => "sharp.preset_light",
            Preset::StrongBlur => "sharp.preset_strong",
            Preset::Motion => "sharp.preset_motion",
            Preset::OutOfFocus => "sharp.preset_focus",
            Preset::OldPhoto => "sharp.preset_old",
            Preset::Portrait => "sharp.preset_portrait",
        }
    }
    pub fn tip_key(self) -> &'static str {
        match self {
            Preset::Auto => "sharp.preset_auto_tip",
            Preset::LightBlur => "sharp.preset_light_tip",
            Preset::StrongBlur => "sharp.preset_strong_tip",
            Preset::Motion => "sharp.preset_motion_tip",
            Preset::OutOfFocus => "sharp.preset_focus_tip",
            Preset::OldPhoto => "sharp.preset_old_tip",
            Preset::Portrait => "sharp.preset_portrait_tip",
        }
    }
    /// The preset `Auto` resolves to for an analysed image.
    pub fn for_report(r: &BlurReport) -> Preset {
        match r.kind {
            BlurKind::Motion => Preset::Motion,
            BlurKind::LowRes => Preset::OldPhoto,
            BlurKind::Strong => Preset::StrongBlur,
            BlurKind::Light | BlurKind::Sharp => Preset::LightBlur,
        }
    }
    /// Suggested Faithful ↔ Creative position (0 = faithful only). From the evaluation: a 50 %
    /// blend wins for defocus and small photos; motion blur is best faithful.
    pub fn default_creative(self) -> f32 {
        match self {
            Preset::StrongBlur | Preset::OutOfFocus | Preset::OldPhoto | Preset::Portrait => 0.5,
            Preset::Auto | Preset::LightBlur | Preset::Motion => 0.0,
        }
    }
}

/// User settings of the mode (stored in `JobSettings::sharp`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SharpSettings {
    pub preset: Preset,
    /// 0 = faithful only; above 0 the generative model runs and is blended in by this amount.
    pub creative: f32,
    /// Use the preset's suggested Creative amount instead of `creative`.
    pub creative_auto: bool,
    /// Output size: 1 = original size, 2 or 4 = enlarged.
    pub scale: u32,
    pub fix_faces: bool,
    pub face_strength: f32,
    /// Final CAS strength (0 = off).
    pub sharpen: f32,
    // ---- Advanced (empty / 0 = automatic) ----
    /// Deblur model id, `""` = by blur type, `"none"` = skip.
    pub deblur_model: String,
    /// Faithful detail model id (`""` = default).
    pub sr_model: String,
    /// Downscale factor (0 = automatic).
    pub downscale: f32,
    /// Tile size for the deblur / generative models (0 = automatic).
    pub tile: u32,
    pub seed: u64,
}

impl Default for SharpSettings {
    fn default() -> Self {
        SharpSettings {
            preset: Preset::Auto,
            creative: 0.0,
            creative_auto: true,
            scale: 1,
            fix_faces: true,
            face_strength: 0.7,
            sharpen: 0.25,
            deblur_model: String::new(),
            sr_model: String::new(),
            downscale: 0.0,
            tile: 0,
            seed: 42,
        }
    }
}

/// Which deblur model a blur type gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deblur {
    Motion,
    Defocus,
}

impl Deblur {
    pub fn model_id(self) -> &'static str {
        match self {
            Deblur::Motion => DEBLUR_MOTION,
            Deblur::Defocus => DEBLUR_DEFOCUS,
        }
    }
}

/// Fully resolved stage choices for one image.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    /// The preset after `Auto` was decided.
    pub preset: Preset,
    /// Deblur model id (`None` = skip).
    pub deblur: Option<String>,
    /// Largest automatic downscale factor.
    pub downscale_cap: f32,
    /// Fixed downscale factor (user override), else measured after deblurring.
    pub downscale_fixed: Option<f32>,
    pub sr_model: String,
    /// Generative blend 0..1 (0 = generative model not used).
    pub creative: f32,
    pub faces: bool,
}

/// Decide the stages for `s` on an image with report `r`.
pub fn resolve(s: &SharpSettings, r: &BlurReport) -> Resolved {
    let preset = if s.preset == Preset::Auto { Preset::for_report(r) } else { s.preset };
    let by_type = match preset {
        Preset::Motion => Some(Deblur::Motion),
        // A small picture is limited by its pixels: SR alone does better than a deblur model.
        Preset::OldPhoto => None,
        Preset::LightBlur | Preset::StrongBlur | Preset::OutOfFocus | Preset::Portrait => {
            // Streaks win even under a defocus preset: the defocus model makes motion worse.
            if r.kind == BlurKind::Motion { Some(Deblur::Motion) } else { Some(Deblur::Defocus) }
        }
        Preset::Auto => unreachable!("resolved above"),
    };
    let deblur = match s.deblur_model.as_str() {
        "" => by_type.map(|d| d.model_id().to_string()),
        NONE => None,
        id => Some(id.to_string()),
    };
    let downscale_cap = match preset {
        // Faces turn waxy past 2.5x; the face restorer rebuilds them instead.
        Preset::Portrait => 2.5,
        Preset::LightBlur => 2.5,
        Preset::OldPhoto => 2.0,
        _ => 4.0,
    };
    Resolved {
        preset,
        deblur,
        downscale_cap,
        downscale_fixed: (s.downscale >= 1.0).then_some(s.downscale.min(8.0)),
        sr_model: if s.sr_model.is_empty() { FAITHFUL_SR.to_string() } else { s.sr_model.clone() },
        creative: if s.creative_auto { preset.default_creative() } else { s.creative.clamp(0.0, 1.0) },
        faces: s.fix_faces,
    }
}

/// Automatic downscale factor from the sharp factor measured after deblurring: never below 1,
/// at most `cap`, snapped down to quarter steps (small differences are noise).
/// Low-resolution sources still get a 2x round trip (the eval's best for upscaled small photos).
pub fn downscale_factor(measured: f32, cap: f32, preset: Preset) -> f32 {
    let f = if preset == Preset::OldPhoto { measured.max(2.0) } else { measured };
    ((f.clamp(1.0, cap.max(1.0)) * 4.0).floor() / 4.0).max(1.0)
}

/// Size after downscaling by `f` (at least 16 px a side).
pub fn downscaled_size(w: u32, h: u32, f: f32) -> (u32, u32) {
    let d = |v: u32| ((v as f32 / f.max(1.0)).round() as u32).max(16).min(v.max(16));
    (d(w), d(h))
}

/// Output size for the target: original size (x1), 2x or 4x of the source.
pub fn target_size(w: u32, h: u32, mult: u32) -> (u32, u32) {
    (w * mult.max(1), h * mult.max(1))
}

/// Size the generative model works at: the target, capped at `max_mp` megapixels (larger
/// targets are enlarged afterwards), rounded to multiples of 16.
pub fn generative_size(w: u32, h: u32, max_mp: f64) -> (u32, u32) {
    let mp = w as f64 * h as f64 / 1e6;
    let k = if mp > max_mp { (max_mp / mp).sqrt() } else { 1.0 };
    let a = |v: u32| (((v as f64 * k) / 16.0).round() as u32).max(1) * 16;
    (a(w), a(h))
}

/// Largest working size of the generative model (megapixels), by GPU memory.
pub fn generative_max_mp(vram_mb: u64) -> f64 {
    // Measured on DirectML (docs/DEBLUR_EVAL.md): 2 MP peaks at +5.9 GB VRAM / +7.2 GB commit.
    if vram_mb <= 7_000 { 1.0 } else { 2.1 }
}

/// Tile for the generative VAE by GPU memory (the decoder is the peak).
pub fn generative_tile(vram_mb: u64, user: u32) -> u32 {
    if user >= super::seedvr2::MIN_TILE {
        return user / 16 * 16;
    }
    // The VAE decoder is the peak; 384 px tiles ran out of memory on DirectML at 2 MP.
    if vram_mb <= 7_000 { 192 } else { 256 }
}

/// Rough processing time in seconds (DirectML on an RTX 5060 Ti class GPU; docs/DEBLUR_EVAL.md):
/// faithful about 5 s per source megapixel; the generative model about 68 s + 23 s per working
/// megapixel (its working size is capped at `max_mp`), about 40 % less on CUDA.
pub fn estimate_secs(src_mp: f64, mult: u32, creative: f32, max_mp: f64, cuda: bool) -> f64 {
    let out_mp = src_mp * (mult.max(1) * mult.max(1)) as f64;
    let faithful = 2.0 + 5.0 * src_mp + 0.3 * out_mp;
    let generative = if creative > 0.0 {
        let s = 68.0 + 23.0 * out_mp.min(max_mp);
        if cuda { s * 0.6 } else { s }
    } else {
        0.0
    };
    faithful + generative
}

/// Whether the settings suit this PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcFit {
    Fits,
    /// Works, but close other programs (little free memory).
    Tight,
    /// The generative model needs more GPU memory than this PC has.
    TooHeavy,
}

impl PcFit {
    pub fn label_key(self) -> &'static str {
        match self {
            PcFit::Fits => "sharp.fit_ok",
            PcFit::Tight => "sharp.fit_tight",
            PcFit::TooHeavy => "sharp.fit_heavy",
        }
    }
}

/// `vram_mb` / `ram_free_mb`: GPU memory and free RAM. The generative model peaked at +5.9 GB
/// VRAM and +7.2 GB commit at 2 MP.
pub fn pc_fit(creative: f32, gpu: bool, vram_mb: u64, ram_free_mb: u64) -> PcFit {
    if creative <= 0.0 {
        return if ram_free_mb < 2_000 { PcFit::Tight } else { PcFit::Fits };
    }
    if !gpu || vram_mb < 6_000 {
        PcFit::TooHeavy
    } else if ram_free_mb < 7_500 || vram_mb < 8_000 {
        PcFit::Tight
    } else {
        PcFit::Fits
    }
}

/// Next smaller tile after an out-of-memory error, `None` when already at the minimum.
pub fn smaller_tile(tile: u32) -> Option<u32> {
    let min = super::seedvr2::MIN_TILE;
    if tile <= min {
        return None;
    }
    let next = if tile == 0 { 512 } else { (tile * 2 / 3) / 16 * 16 };
    Some(next.max(min))
}

/// Colour fix of a generative result: keep its detail, take the low frequencies (colour, tone)
/// from `reference` — `out - blur(out) + blur(reference)` (like SeedVR2's wavelet colour fix).
pub fn color_fix(out: &image::RgbImage, reference: &image::RgbImage, sigma: f32) -> image::RgbImage {
    debug_assert_eq!(out.dimensions(), reference.dimensions());
    let lo_out = crate::engines::filters::gaussian_blur(out, sigma);
    let lo_ref = crate::engines::filters::gaussian_blur(reference, sigma);
    let mut res = out.clone();
    for (i, o) in res.as_mut().iter_mut().enumerate() {
        let v = out.as_raw()[i] as i32 - lo_out.as_raw()[i] as i32 + lo_ref.as_raw()[i] as i32;
        *o = v.clamp(0, 255) as u8;
    }
    res
}

/// Low-pass size of the colour fix (working pixels).
pub const COLOR_FIX_SIGMA: f32 = 8.0;

/// Linear blend `a·(1-t) + b·t` of two same-size images.
pub fn blend(a: &image::RgbImage, b: &image::RgbImage, t: f32) -> image::RgbImage {
    debug_assert_eq!(a.dimensions(), b.dimensions());
    let t = t.clamp(0.0, 1.0);
    let mut out = a.clone();
    for (o, (pa, pb)) in out.as_mut().iter_mut().zip(a.as_raw().iter().zip(b.as_raw().iter())) {
        *o = (*pa as f32 * (1.0 - t) + *pb as f32 * t).round() as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(kind: BlurKind) -> BlurReport {
        BlurReport { sharp_factor: 3.0, motion: 0.1, streak_deg: 0.0, megapixels: 2.0, kind }
    }

    #[test]
    fn auto_routes_by_blur_type() {
        let s = SharpSettings::default();
        let m = resolve(&s, &report(BlurKind::Motion));
        assert_eq!((m.preset, m.deblur.as_deref()), (Preset::Motion, Some(DEBLUR_MOTION)));
        let d = resolve(&s, &report(BlurKind::Strong));
        assert_eq!((d.preset, d.deblur.as_deref()), (Preset::StrongBlur, Some(DEBLUR_DEFOCUS)));
        let l = resolve(&s, &report(BlurKind::LowRes));
        assert_eq!((l.preset, l.deblur), (Preset::OldPhoto, None));
        assert_eq!(resolve(&s, &report(BlurKind::Sharp)).preset, Preset::LightBlur);
    }

    #[test]
    fn presets_map_to_stages() {
        let mut s = SharpSettings { preset: Preset::Portrait, ..Default::default() };
        let p = resolve(&s, &report(BlurKind::Strong));
        assert_eq!(p.deblur.as_deref(), Some(DEBLUR_DEFOCUS));
        assert_eq!(p.downscale_cap, 2.5);
        assert!(p.faces);
        // Motion streaks override a defocus preset.
        s.preset = Preset::OutOfFocus;
        assert_eq!(resolve(&s, &report(BlurKind::Motion)).deblur.as_deref(), Some(DEBLUR_MOTION));
        s.preset = Preset::Motion;
        assert_eq!(resolve(&s, &report(BlurKind::Light)).deblur.as_deref(), Some(DEBLUR_MOTION));
        // Advanced overrides.
        s.deblur_model = NONE.into();
        s.downscale = 1.5;
        s.sr_model = "dat2-nomos2-4x".into();
        s.creative = 3.0;
        s.creative_auto = false;
        let o = resolve(&s, &report(BlurKind::Light));
        assert_eq!(o.deblur, None);
        assert_eq!(o.downscale_fixed, Some(1.5));
        assert_eq!(o.sr_model, "dat2-nomos2-4x");
        assert_eq!(o.creative, 1.0);
        for p in Preset::ALL {
            assert!(!p.label_key().is_empty() && !p.tip_key().is_empty());
        }
    }

    #[test]
    fn creative_follows_the_preset_until_set() {
        let s = SharpSettings::default();
        assert_eq!(resolve(&s, &report(BlurKind::Motion)).creative, 0.0);
        assert_eq!(resolve(&s, &report(BlurKind::Strong)).creative, 0.5);
        let s = SharpSettings { creative_auto: false, creative: 0.8, ..Default::default() };
        assert_eq!(resolve(&s, &report(BlurKind::Motion)).creative, 0.8);
    }

    #[test]
    fn downscale_math() {
        assert_eq!(downscale_factor(0.8, 4.0, Preset::StrongBlur), 1.0);
        assert_eq!(downscale_factor(2.9, 4.0, Preset::StrongBlur), 2.75);
        assert_eq!(downscale_factor(6.0, 4.0, Preset::StrongBlur), 4.0);
        assert_eq!(downscale_factor(4.0, 2.5, Preset::Portrait), 2.5);
        assert_eq!(downscale_factor(1.2, 2.0, Preset::OldPhoto), 2.0);
        assert_eq!(downscaled_size(1920, 1080, 2.0), (960, 540));
        assert_eq!(downscaled_size(1920, 1080, 1.0), (1920, 1080));
        assert_eq!(downscaled_size(40, 20, 4.0), (16, 16));
        assert_eq!(target_size(800, 600, 2), (1600, 1200));
        assert_eq!(target_size(800, 600, 0), (800, 600));
    }

    #[test]
    fn generative_size_is_aligned_and_capped() {
        assert_eq!(generative_size(1920, 1080, 4.0), (1920, 1088));
        let (w, h) = generative_size(4000, 3000, 2.0);
        assert!(w % 16 == 0 && h % 16 == 0);
        assert!((w as f64 * h as f64 / 1e6) < 2.1, "{w}x{h}");
        assert!((w as f64 / h as f64 - 4.0 / 3.0).abs() < 0.02);
    }

    #[test]
    fn tiles_shrink_on_oom_down_to_the_minimum() {
        assert_eq!(generative_tile(16_000, 0), 256);
        assert_eq!(generative_tile(6_000, 0), 192);
        assert_eq!(generative_tile(6_000, 300), 288);
        assert_eq!(generative_max_mp(16_000), 2.1);
        let mut t = 384;
        let mut steps = 0;
        while let Some(n) = smaller_tile(t) {
            assert!(n < t && n % 16 == 0);
            t = n;
            steps += 1;
        }
        assert_eq!(t, super::super::seedvr2::MIN_TILE);
        assert!(steps <= 4);
    }

    #[test]
    fn color_fix_takes_tone_from_the_reference_and_keeps_detail() {
        // Output: fine checker on a warm tint; reference: flat neutral grey.
        let out = image::RgbImage::from_fn(64, 64, |x, y| {
            let d = if (x + y) % 2 == 0 { 30 } else { 0 };
            image::Rgb([150 + d, 120 + d, 100 + d])
        });
        let reference = image::RgbImage::from_pixel(64, 64, image::Rgb([128, 128, 128]));
        let f = color_fix(&out, &reference, 4.0);
        let mean = |c: usize| f.pixels().map(|p| p[c] as f32).sum::<f32>() / (64.0 * 64.0);
        for c in 0..3 {
            assert!((mean(c) - 128.0).abs() < 2.0, "channel {c}: {}", mean(c));
        }
        // The checker survives.
        let (a, b) = (f.get_pixel(32, 32)[0] as i32, f.get_pixel(33, 32)[0] as i32);
        assert!((a - b).abs() > 20, "{a} {b}");
    }

    #[test]
    fn estimates_and_fit() {
        let f = estimate_secs(2.0, 1, 0.0, 2.1, false);
        assert!((10.0..15.0).contains(&f), "{f}");
        let g = estimate_secs(1.0, 1, 0.5, 2.1, false);
        assert!((90.0..110.0).contains(&g), "{g}"); // measured 91 s + faithful
        assert!(estimate_secs(1.0, 1, 0.5, 2.1, true) < g);
        // 4x output: the generative part is capped at its working size.
        assert!(estimate_secs(1.0, 4, 0.5, 2.1, false) < 160.0);
        assert_eq!(pc_fit(0.0, false, 0, 8_000), PcFit::Fits);
        assert_eq!(pc_fit(0.5, false, 0, 8_000), PcFit::TooHeavy);
        assert_eq!(pc_fit(0.5, true, 4_000, 16_000), PcFit::TooHeavy);
        assert_eq!(pc_fit(0.5, true, 16_000, 4_000), PcFit::Tight);
        assert_eq!(pc_fit(0.5, true, 16_000, 12_000), PcFit::Fits);
    }

    #[test]
    fn blend_endpoints() {
        let a = image::RgbImage::from_pixel(2, 2, image::Rgb([0, 100, 200]));
        let b = image::RgbImage::from_pixel(2, 2, image::Rgb([200, 100, 0]));
        assert_eq!(blend(&a, &b, 0.0), a);
        assert_eq!(blend(&a, &b, 1.0), b);
        assert_eq!(blend(&a, &b, 0.5).get_pixel(0, 0).0, [100, 100, 100]);
    }
}
