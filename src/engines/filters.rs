//! CPU filters on RGB8 frames, parallelised by rows with rayon (run inside the job's pool).
//!
//! * [`cas`] — AMD FidelityFX-style Contrast Adaptive Sharpening.
//! * [`unsharp`] — classic Gaussian unsharp mask with threshold.
//! * [`denoise`] — 5×5 bilateral filter with a luma-difference range kernel. Chosen because it
//!   is edge-preserving, single-pass and O(25·N): ~10–40 ms for a 1080p frame on a desktop CPU,
//!   so it keeps up with video. NL-means would be 10–50× slower for similar strength.

use image::RgbImage;
use rayon::prelude::*;

#[inline]
fn clamp_xy(v: i64, max: u32) -> usize {
    v.clamp(0, max as i64 - 1) as usize
}

/// Contrast Adaptive Sharpening. `strength` 0..=1.
pub fn cas(img: &RgbImage, strength: f32) -> RgbImage {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 || strength <= 0.0 {
        return img.clone();
    }
    let s = strength.clamp(0.0, 1.0);
    // peak = -1 / lerp(8, 5, s)
    let peak = -1.0 / (8.0 - 3.0 * s);
    let src = img.as_raw();
    let stride = w as usize * 3;
    let mut out = vec![0u8; src.len()];
    out.par_chunks_mut(stride).enumerate().for_each(|(y, row)| {
        let yu = clamp_xy(y as i64 - 1, h);
        let yd = clamp_xy(y as i64 + 1, h);
        for x in 0..w as usize {
            let xl = clamp_xy(x as i64 - 1, w);
            let xr = clamp_xy(x as i64 + 1, w);
            for c in 0..3 {
                let px =
                    |xx: usize, yy: usize| src[yy * stride + xx * 3 + c] as f32 * (1.0 / 255.0);
                let b = px(x, yu);
                let d = px(xl, y);
                let e = px(x, y);
                let f = px(xr, y);
                let hh = px(x, yd);
                let mn = b.min(d).min(e).min(f).min(hh);
                let mx = b.max(d).max(e).max(f).max(hh);
                let amp = if mx > 0.0 {
                    (mn.min(1.0 - mx) / mx).clamp(0.0, 1.0).sqrt()
                } else {
                    0.0
                };
                let wgt = amp * peak;
                let v = ((b + d + f + hh) * wgt + e) / (1.0 + 4.0 * wgt);
                row[x * 3 + c] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    });
    RgbImage::from_raw(w, h, out).expect("same size")
}

fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let sigma = sigma.max(0.1);
    let r = (sigma * 3.0).ceil() as i32;
    let mut k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// Separable Gaussian blur (u16 fixed-point intermediate).
pub fn gaussian_blur(img: &RgbImage, sigma: f32) -> RgbImage {
    let (w, h) = img.dimensions();
    let k = gaussian_kernel(sigma);
    let r = (k.len() / 2) as i64;
    let src = img.as_raw();
    let stride = w as usize * 3;
    // Horizontal pass → u16 (value * 256).
    let mut tmp = vec![0u16; src.len()];
    tmp.par_chunks_mut(stride).enumerate().for_each(|(y, row)| {
        let line = &src[y * stride..(y + 1) * stride];
        for x in 0..w as usize {
            for c in 0..3 {
                let mut acc = 0.0f32;
                for (i, kv) in k.iter().enumerate() {
                    let xx = clamp_xy(x as i64 + i as i64 - r, w);
                    acc += line[xx * 3 + c] as f32 * kv;
                }
                row[x * 3 + c] = (acc * 256.0).round().min(65535.0) as u16;
            }
        }
    });
    // Vertical pass → u8.
    let mut out = vec![0u8; src.len()];
    out.par_chunks_mut(stride).enumerate().for_each(|(y, row)| {
        for (i, o) in row.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for (j, kv) in k.iter().enumerate() {
                let yy = clamp_xy(y as i64 + j as i64 - r, h);
                acc += tmp[yy * stride + i] as f32 * kv;
            }
            *o = (acc / 256.0).round().clamp(0.0, 255.0) as u8;
        }
    });
    RgbImage::from_raw(w, h, out).expect("same size")
}

/// Unsharp mask: `out = in + amount * (in - blur)` where `|in - blur| >= threshold`.
pub fn unsharp(img: &RgbImage, radius: f32, amount: f32, threshold: u8) -> RgbImage {
    if amount <= 0.0 {
        return img.clone();
    }
    let blur = gaussian_blur(img, radius);
    let mut out = img.clone();
    let th = threshold as i32;
    out.as_mut()
        .par_iter_mut()
        .zip(blur.as_raw().par_iter())
        .for_each(|(o, &b)| {
            let v = *o as i32;
            let diff = v - b as i32;
            if diff.abs() >= th {
                *o = (v as f32 + amount * diff as f32).round().clamp(0.0, 255.0) as u8;
            }
        });
    out
}

/// Edge-preserving bilateral denoise. `strength` 0..=1 (0 = off).
pub fn denoise(img: &RgbImage, strength: f32) -> RgbImage {
    let (w, h) = img.dimensions();
    if strength <= 0.0 || w < 5 || h < 5 {
        return img.clone();
    }
    let s = strength.clamp(0.0, 1.0);
    let sigma_r = 4.0 + 36.0 * s; // range sigma in 0..255 luma units
    let sigma_s = 1.0 + 1.0 * s; // spatial sigma in pixels
    let range_lut: Vec<f32> = (0..256)
        .map(|d| (-((d * d) as f32) / (2.0 * sigma_r * sigma_r)).exp())
        .collect();
    let mut spatial = [[0f32; 5]; 5];
    for (dy, row) in spatial.iter_mut().enumerate() {
        for (dx, v) in row.iter_mut().enumerate() {
            let (fy, fx) = (dy as f32 - 2.0, dx as f32 - 2.0);
            *v = (-(fx * fx + fy * fy) / (2.0 * sigma_s * sigma_s)).exp();
        }
    }
    let src = img.as_raw();
    let stride = w as usize * 3;
    // Luma plane (BT.601 integer approximation).
    let luma: Vec<u8> = src
        .par_chunks(3)
        .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
        .collect();
    let mut out = vec![0u8; src.len()];
    out.par_chunks_mut(stride).enumerate().for_each(|(y, row)| {
        for x in 0..w as usize {
            let lc = luma[y * w as usize + x] as i32;
            let mut acc = [0f32; 3];
            let mut wsum = 0f32;
            for (dy, srow) in spatial.iter().enumerate() {
                let yy = clamp_xy(y as i64 + dy as i64 - 2, h);
                for (dx, &ws) in srow.iter().enumerate() {
                    let xx = clamp_xy(x as i64 + dx as i64 - 2, w);
                    let l = luma[yy * w as usize + xx] as i32;
                    let wgt = ws * range_lut[(l - lc).unsigned_abs() as usize];
                    let p = &src[yy * stride + xx * 3..yy * stride + xx * 3 + 3];
                    acc[0] += p[0] as f32 * wgt;
                    acc[1] += p[1] as f32 * wgt;
                    acc[2] += p[2] as f32 * wgt;
                    wsum += wgt;
                }
            }
            for c in 0..3 {
                row[x * 3 + c] = (acc[c] / wsum + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    });
    RgbImage::from_raw(w, h, out).expect("same size")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, v: u8) -> RgbImage {
        RgbImage::from_pixel(w, h, image::Rgb([v, v, v]))
    }

    fn step_edge(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, _| {
            let v = if x < w / 2 { 60 } else { 190 };
            image::Rgb([v, v, v])
        })
    }

    #[test]
    fn cas_keeps_flat_and_increases_edge_contrast() {
        let f = flat(16, 16, 120);
        assert_eq!(cas(&f, 1.0), f);
        let e = step_edge(16, 8);
        let s = cas(&e, 1.0);
        // Pixel just left of the edge gets darker, just right gets brighter.
        assert!(s.get_pixel(7, 4)[0] < 60);
        assert!(s.get_pixel(8, 4)[0] > 190);
        // Far from the edge: unchanged.
        assert_eq!(s.get_pixel(2, 4)[0], 60);
        assert_eq!(cas(&e, 0.0), e);
    }

    #[test]
    fn unsharp_overshoots_edges_and_respects_threshold() {
        let e = step_edge(32, 8);
        let s = unsharp(&e, 1.0, 1.0, 0);
        assert!(s.get_pixel(15, 4)[0] < 60);
        assert!(s.get_pixel(16, 4)[0] > 190);
        let f = flat(8, 8, 100);
        assert_eq!(unsharp(&f, 1.0, 2.0, 0), f);
        // Huge threshold → nothing changes.
        assert_eq!(unsharp(&e, 1.0, 1.0, 255), e);
    }

    #[test]
    fn blur_preserves_mean_of_flat() {
        let f = flat(10, 10, 77);
        assert_eq!(gaussian_blur(&f, 2.0), f);
    }

    #[test]
    fn denoise_reduces_noise_keeps_edges() {
        // Deterministic pseudo-noise on a flat field.
        let mut seed = 12345u32;
        let noisy = RgbImage::from_fn(32, 32, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = ((seed >> 16) % 21) as i32 - 10;
            let v = (128 + n) as u8;
            image::Rgb([v, v, v])
        });
        let var = |img: &RgbImage| {
            let px: Vec<f32> = img.pixels().map(|p| p[0] as f32).collect();
            let m = px.iter().sum::<f32>() / px.len() as f32;
            px.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / px.len() as f32
        };
        let d = denoise(&noisy, 0.8);
        assert!(
            var(&d) < var(&noisy) * 0.5,
            "{} vs {}",
            var(&d),
            var(&noisy)
        );
        let e = step_edge(32, 8);
        let de = denoise(&e, 1.0);
        assert!(de.get_pixel(15, 4)[0] < 75);
        assert!(de.get_pixel(16, 4)[0] > 175);
        assert_eq!(denoise(&e, 0.0), e);
    }
}
