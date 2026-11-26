//! Blur analysis for "Make sharp (AI)": how much detail the image really has and whether the
//! blur looks one-directional (camera shake). Pure Rust on a grey copy of at most 2048 px.
//!
//! * **Sharp factor**: the largest factor the image can be shrunk by and enlarged again (area
//!   down, bilinear up) while losing less than 10 % of its gradient energy. A sharp photo gives
//!   1; a photo whose detail would fit in half the pixels gives 2. It is also the "effective
//!   sharp resolution" used for the smart downscale.
//! * **Motion**: gradient energy along 8 directions at full resolution, divided by the same at a
//!   coarse scale (Gaussian, sigma 3). Content that is itself directional (stripes, text) shows up
//!   at both scales and mostly cancels; motion blur removes fine detail along one direction only.
//!   The score is `1 - min/max` over the directions; the minimum gives the streak direction.

use image::RgbImage;

/// Above this motion score the blur is treated as camera shake.
pub const MOTION_THRESHOLD: f32 = 0.35;
/// Below this many megapixels a photo counts as small / low-resolution.
pub const SMALL_MP: f64 = 0.6;
const ANALYSIS_MAX_SIDE: u32 = 2048;
const LOSS_TOLERANCE: f64 = 0.10;
const FACTORS: [f32; 11] = [1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0, 5.0, 6.0, 8.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurKind {
    /// Little or no blur.
    Sharp,
    /// Slightly soft (sharp factor up to 2.5).
    Light,
    /// Clearly out of focus / heavily blurred.
    Strong,
    /// One-directional blur (camera shake, moving subject).
    Motion,
    /// Small picture: detail is limited by the pixel count rather than by blur.
    LowRes,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurReport {
    /// Effective sharp-resolution factor (>= 1, in source pixels).
    pub sharp_factor: f32,
    /// 0..1, one-directional blur score.
    pub motion: f32,
    /// Direction of the blur streak in degrees (0 = horizontal, 90 = vertical), image coordinates.
    pub streak_deg: f32,
    pub megapixels: f64,
    pub kind: BlurKind,
}

impl BlurReport {
    /// Locale key of the plain-words description.
    pub fn describe_key(&self) -> &'static str {
        match self.kind {
            BlurKind::Sharp => "sharp.looks_sharp",
            BlurKind::Light => "sharp.looks_light",
            BlurKind::Strong => "sharp.looks_strong",
            BlurKind::Motion => "sharp.looks_motion",
            BlurKind::LowRes => "sharp.looks_lowres",
        }
    }
}

/// Grey image as f32 0..1.
#[derive(Clone, Debug)]
struct Gray {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Gray {
    fn at(&self, x: usize, y: usize) -> f32 {
        self.px[y * self.w + x]
    }
}

fn to_gray(img: &RgbImage) -> Gray {
    let px = img
        .pixels()
        .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
        .collect();
    Gray { w: img.width() as usize, h: img.height() as usize, px }
}

/// Area-weighted resampling matrix rows for shrinking `n` samples to `m` (m <= n): for each
/// output sample, the (first input index, weights) it averages.
fn area_rows(n: usize, m: usize) -> Vec<(usize, Vec<f32>)> {
    (0..m)
        .map(|i| {
            let a = i as f64 * n as f64 / m as f64;
            let b = (i + 1) as f64 * n as f64 / m as f64;
            let j0 = a.floor() as usize;
            let j1 = (b.ceil() as usize).min(n);
            let mut w: Vec<f32> = (j0..j1).map(|j| (b.min(j as f64 + 1.0) - a.max(j as f64)) as f32).collect();
            let s: f32 = w.iter().sum();
            w.iter_mut().for_each(|v| *v /= s.max(1e-9));
            (j0, w)
        })
        .collect()
}

/// Area (box) downscale by `f` >= 1.
fn box_down(g: &Gray, f: f32) -> Gray {
    let w = ((g.w as f32 / f).round() as usize).max(1);
    let h = ((g.h as f32 / f).round() as usize).max(1);
    let rx = area_rows(g.w, w);
    let ry = area_rows(g.h, h);
    let mut tmp = vec![0f32; w * g.h];
    for y in 0..g.h {
        let row = &g.px[y * g.w..(y + 1) * g.w];
        for (x, (j0, ws)) in rx.iter().enumerate() {
            tmp[y * w + x] = ws.iter().enumerate().map(|(k, wk)| row[j0 + k] * wk).sum();
        }
    }
    let mut px = vec![0f32; w * h];
    for (y, (j0, ws)) in ry.iter().enumerate() {
        for x in 0..w {
            px[y * w + x] = ws.iter().enumerate().map(|(k, wk)| tmp[(j0 + k) * w + x] * wk).sum();
        }
    }
    Gray { w, h, px }
}

/// Bilinear sample positions for enlarging `n` samples to `m`.
fn lin_taps(n: usize, m: usize) -> Vec<(usize, usize, f32)> {
    (0..m)
        .map(|i| {
            let x = ((i as f64 + 0.5) * n as f64 / m as f64 - 0.5).clamp(0.0, (n - 1) as f64);
            let i0 = x.floor() as usize;
            let i1 = (i0 + 1).min(n - 1);
            (i0, i1, (x - i0 as f64) as f32)
        })
        .collect()
}

fn lin_up(s: &Gray, w: usize, h: usize) -> Gray {
    let tx = lin_taps(s.w, w);
    let ty = lin_taps(s.h, h);
    let mut tmp = vec![0f32; w * s.h];
    for y in 0..s.h {
        for (x, &(a, b, t)) in tx.iter().enumerate() {
            tmp[y * w + x] = s.at(a, y) * (1.0 - t) + s.at(b, y) * t;
        }
    }
    let mut px = vec![0f32; w * h];
    for (y, &(a, b, t)) in ty.iter().enumerate() {
        for x in 0..w {
            px[y * w + x] = tmp[a * w + x] * (1.0 - t) + tmp[b * w + x] * t;
        }
    }
    Gray { w, h, px }
}

/// Central-difference gradients (0 on the border).
fn grads(g: &Gray) -> (Vec<f32>, Vec<f32>) {
    let (w, h) = (g.w, g.h);
    let mut gx = vec![0f32; w * h];
    let mut gy = vec![0f32; w * h];
    for y in 0..h {
        for x in 1..w.saturating_sub(1) {
            gx[y * w + x] = (g.px[y * w + x + 1] - g.px[y * w + x - 1]) * 0.5;
        }
    }
    for y in 1..h.saturating_sub(1) {
        for x in 0..w {
            gy[y * w + x] = (g.px[(y + 1) * w + x] - g.px[(y - 1) * w + x]) * 0.5;
        }
    }
    (gx, gy)
}

/// Fraction of gradient energy lost by an area-down / bilinear-up round trip at factor `f`.
fn detail_loss(g: &Gray, gx: &[f32], gy: &[f32], f: f32) -> f64 {
    let r = lin_up(&box_down(g, f), g.w, g.h);
    let (rx, ry) = grads(&r);
    let mut e = 1e-9f64;
    let mut d = 0f64;
    for i in 0..gx.len() {
        e += (gx[i] * gx[i] + gy[i] * gy[i]) as f64;
        let (a, b) = (gx[i] - rx[i], gy[i] - ry[i]);
        d += (a * a + b * b) as f64;
    }
    d / e
}

fn sharp_factor_of(g: &Gray) -> f32 {
    let (gx, gy) = grads(g);
    let mut best = 1.0;
    for f in FACTORS {
        if (g.w as f32 / f) < 16.0 || (g.h as f32 / f) < 16.0 {
            break;
        }
        if detail_loss(g, &gx, &gy, f) < LOSS_TOLERANCE {
            best = f;
        } else {
            break;
        }
    }
    best
}

fn blur1d(g: &Gray, sigma: f32, vertical: bool) -> Gray {
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-0.5 * (i as f32 / sigma).powi(2)).exp()).collect();
    let s: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / s).collect();
    let (w, h) = (g.w as isize, g.h as isize);
    let reflect = |i: isize, n: isize| -> usize {
        let mut i = i;
        if i < 0 {
            i = -i;
        }
        if i >= n {
            i = 2 * n - 2 - i;
        }
        i.clamp(0, n - 1) as usize
    };
    let mut px = vec![0f32; g.px.len()];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0f32;
            for (t, kv) in k.iter().enumerate() {
                let o = t as isize - r;
                acc += kv
                    * if vertical {
                        g.at(x as usize, reflect(y + o, h))
                    } else {
                        g.at(reflect(x + o, w), y as usize)
                    };
            }
            px[(y * w + x) as usize] = acc;
        }
    }
    Gray { w: g.w, h: g.h, px }
}

/// (motion score 0..1, streak direction in degrees).
fn motion_of(g: &Gray) -> (f32, f32) {
    let c = blur1d(&blur1d(g, 3.0, false), 3.0, true);
    let (fx, fy) = grads(g);
    let (cx, cy) = grads(&c);
    let mut ratios = [0f64; 8];
    for (k, r) in ratios.iter_mut().enumerate() {
        let t = (k as f32 * 22.5).to_radians();
        let (ct, st) = (t.cos(), t.sin());
        let mut fine = 0f64;
        let mut coarse = 0f64;
        for i in 0..fx.len() {
            let d = ct * fx[i] + st * fy[i];
            let dc = ct * cx[i] + st * cy[i];
            fine += (d * d) as f64;
            coarse += (dc * dc) as f64;
        }
        *r = fine / coarse.max(1e-12);
    }
    let (imin, min) = ratios.iter().enumerate().fold((0, f64::MAX), |a, (i, &v)| if v < a.1 { (i, v) } else { a });
    let max = ratios.iter().cloned().fold(0f64, f64::max);
    ((1.0 - min / max.max(1e-12)) as f32, imin as f32 * 22.5)
}

/// Classify from the measured numbers (pure, tested on its own).
pub fn classify(sharp_factor: f32, motion: f32, megapixels: f64) -> BlurKind {
    if motion >= MOTION_THRESHOLD {
        BlurKind::Motion
    } else if megapixels < SMALL_MP {
        BlurKind::LowRes
    } else if sharp_factor <= 1.25 {
        BlurKind::Sharp
    } else if sharp_factor <= 2.5 {
        BlurKind::Light
    } else {
        BlurKind::Strong
    }
}

/// Analyse an image (at most 2048 px on the long side are used; factors are in source pixels).
pub fn analyse(img: &RgbImage) -> BlurReport {
    let (w, h) = img.dimensions();
    let long = w.max(h).max(1);
    let (g, scale) = if long > ANALYSIS_MAX_SIDE {
        let s = long as f32 / ANALYSIS_MAX_SIDE as f32;
        (box_down(&to_gray(img), s), s)
    } else {
        (to_gray(img), 1.0)
    };
    let sharp_factor = sharp_factor_of(&g) * scale;
    let (motion, streak_deg) = motion_of(&g);
    let megapixels = w as f64 * h as f64 / 1e6;
    BlurReport { sharp_factor, motion, streak_deg, megapixels, kind: classify(sharp_factor, motion, megapixels) }
}

/// Sharp factor only (used again after deblurring to pick the downscale factor).
pub fn sharp_factor(img: &RgbImage) -> f32 {
    let (w, h) = img.dimensions();
    let long = w.max(h).max(1);
    if long > ANALYSIS_MAX_SIDE {
        let s = long as f32 / ANALYSIS_MAX_SIDE as f32;
        sharp_factor_of(&box_down(&to_gray(img), s)) * s
    } else {
        sharp_factor_of(&to_gray(img))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic textured test picture: edges, stripes in several directions and noise-like
    /// detail (no external files).
    pub fn texture(w: u32, h: u32) -> RgbImage {
        let mut seed = 12345u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 1000) as f32 / 1000.0
        };
        let blobs: Vec<(f32, f32, f32, f32)> = (0..60).map(|_| (rnd() * w as f32, rnd() * h as f32, 4.0 + rnd() * 30.0, rnd())).collect();
        RgbImage::from_fn(w, h, |x, y| {
            let (xf, yf) = (x as f32, y as f32);
            let mut v = 0.5 + 0.12 * ((xf * 0.31).sin() * (yf * 0.23).cos()) + 0.08 * ((xf + yf) * 0.45).sin();
            for &(bx, by, r, c) in &blobs {
                if (xf - bx).powi(2) + (yf - by).powi(2) < r * r {
                    v = 0.15 + 0.7 * c;
                }
            }
            let n = ((x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663)) % 97) as f32 / 97.0 - 0.5;
            let v = ((v + 0.06 * n).clamp(0.0, 1.0) * 255.0) as u8;
            image::Rgb([v, v.saturating_add(10), v.saturating_sub(10)])
        })
    }

    fn gauss(img: &RgbImage, s: f32) -> RgbImage {
        crate::engines::filters::gaussian_blur(img, s)
    }

    /// Linear motion blur of `len` px along the x axis (box average), rotated by `vertical`.
    pub fn motion(img: &RgbImage, len: u32, vertical: bool) -> RgbImage {
        let (w, h) = img.dimensions();
        let r = len as i64 / 2;
        RgbImage::from_fn(w, h, |x, y| {
            let mut acc = [0u32; 3];
            let mut n = 0;
            for o in -r..=r {
                let (xx, yy) = if vertical {
                    (x as i64, (y as i64 + o).clamp(0, h as i64 - 1))
                } else {
                    ((x as i64 + o).clamp(0, w as i64 - 1), y as i64)
                };
                let p = img.get_pixel(xx as u32, yy as u32);
                for c in 0..3 {
                    acc[c] += p[c] as u32;
                }
                n += 1;
            }
            image::Rgb([(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8])
        })
    }

    #[test]
    fn blur_amount_orders_light_medium_heavy() {
        let sharp = texture(640, 480);
        let f0 = sharp_factor(&sharp);
        let f1 = sharp_factor(&gauss(&sharp, 1.0));
        let f2 = sharp_factor(&gauss(&sharp, 2.5));
        let f3 = sharp_factor(&gauss(&sharp, 5.0));
        assert!(f0 <= 1.25, "sharp texture {f0}");
        assert!(f0 < f1 && f1 < f2 && f2 < f3, "{f0} {f1} {f2} {f3}");
    }

    #[test]
    fn motion_blur_is_told_apart_from_defocus() {
        let sharp = texture(640, 480);
        let m = analyse(&motion(&sharp, 21, false));
        let d = analyse(&gauss(&sharp, 3.0));
        assert_eq!(m.kind, BlurKind::Motion, "{m:?}");
        assert!(m.motion > d.motion + 0.2, "{m:?} vs {d:?}");
        // Horizontal streak: detail is lost along x (0 degrees).
        assert!(m.streak_deg < 23.0 || m.streak_deg > 157.0, "{m:?}");
        let v = analyse(&motion(&sharp, 21, true));
        assert!((v.streak_deg - 90.0).abs() < 23.0, "{v:?}");
        assert_ne!(d.kind, BlurKind::Motion, "{d:?}");
    }

    #[test]
    fn classify_rules() {
        assert_eq!(classify(1.0, 0.1, 2.0), BlurKind::Sharp);
        assert_eq!(classify(2.0, 0.1, 2.0), BlurKind::Light);
        assert_eq!(classify(4.0, 0.1, 2.0), BlurKind::Strong);
        assert_eq!(classify(1.0, 0.5, 2.0), BlurKind::Motion);
        assert_eq!(classify(3.0, 0.1, 0.3), BlurKind::LowRes);
    }

    #[test]
    fn large_images_are_analysed_on_a_bounded_copy() {
        let big = gauss(&texture(2600, 400), 3.0);
        let r = analyse(&big);
        assert!(r.sharp_factor >= 2.0, "{r:?}");
        assert!((r.megapixels - 1.04).abs() < 0.01);
    }
}
