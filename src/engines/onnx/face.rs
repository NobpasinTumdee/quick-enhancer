//! Face restoration: YuNet finds faces and five landmarks, each face is aligned to the FFHQ
//! 512×512 template with a similarity transform, restored (GFPGAN / CodeFormer), and pasted
//! back through the inverse transform with a feathered mask.

use image::RgbImage;
use std::path::PathBuf;
use std::sync::Arc;

use super::{LoadedSession, runtime, session_for, short};
use crate::engines::{EngineCtx, EngineError};

/// Detector input size (the YuNet export is static 640×640).
pub const DET_SIZE: u32 = 640;
/// Aligned face size for the restoration models.
pub const FACE_SIZE: u32 = 512;
/// FFHQ landmarks at 512×512: left eye, right eye, nose, left / right mouth corner (image view).
pub const FFHQ_512: [[f32; 2]; 5] = [
    [192.98, 239.95],
    [318.90, 240.19],
    [256.63, 314.02],
    [201.26, 371.41],
    [313.09, 371.15],
];
const SCORE_MIN: f32 = 0.6;
const NMS_IOU: f32 = 0.3;
const MAX_FACES: usize = 32;
/// Faces smaller than this (pixels, in the processed image) are left alone.
const MIN_FACE: f32 = 24.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// x, y, w, h
    pub bbox: [f32; 4],
    pub kps: [[f32; 2]; 5],
    pub score: f32,
}

/// Raw YuNet outputs for one stride: `cls`, `obj` (n), `bbox` (n×4), `kps` (n×10).
pub struct StrideOut<'a> {
    pub stride: u32,
    pub cls: &'a [f32],
    pub obj: &'a [f32],
    pub bbox: &'a [f32],
    pub kps: &'a [f32],
}

/// Decode YuNet outputs (input `size × size`) into faces in input pixels (before NMS).
pub fn decode(outs: &[StrideOut], size: u32, min_score: f32) -> Vec<Face> {
    let mut faces = Vec::new();
    for o in outs {
        let cols = size / o.stride;
        let s = o.stride as f32;
        for (idx, (&c, &ob)) in o.cls.iter().zip(o.obj).enumerate() {
            let score = (c.clamp(0.0, 1.0) * ob.clamp(0.0, 1.0)).sqrt();
            if score < min_score {
                continue;
            }
            let (col, row) = ((idx as u32 % cols) as f32, (idx as u32 / cols) as f32);
            let b = &o.bbox[idx * 4..idx * 4 + 4];
            let cx = (col + b[0]) * s;
            let cy = (row + b[1]) * s;
            let w = b[2].exp() * s;
            let h = b[3].exp() * s;
            let k = &o.kps[idx * 10..idx * 10 + 10];
            let mut kps = [[0f32; 2]; 5];
            for (n, p) in kps.iter_mut().enumerate() {
                *p = [(k[2 * n] + col) * s, (k[2 * n + 1] + row) * s];
            }
            faces.push(Face {
                bbox: [cx - w / 2.0, cy - h / 2.0, w, h],
                kps,
                score,
            });
        }
    }
    faces
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Greedy non-maximum suppression, best score first.
pub fn nms(mut faces: Vec<Face>, thresh: f32) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Face> = Vec::new();
    for f in faces {
        if keep.iter().all(|k| iou(&k.bbox, &f.bbox) <= thresh) {
            keep.push(f);
        }
    }
    keep
}

/// 2×3 affine matrix `[a, b, c, d, e, f]`: x' = a·x + b·y + c, y' = d·x + e·y + f.
pub type Affine = [f32; 6];

/// Least-squares similarity (rotation, uniform scale, translation; no reflection) mapping
/// `src` points onto `dst` (Umeyama without reflection).
pub fn similarity(src: &[[f32; 2]], dst: &[[f32; 2]]) -> Affine {
    let n = src.len() as f32;
    let mean = |p: &[[f32; 2]]| {
        let (x, y) = p.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
        [x / n, y / n]
    };
    let (ms, md) = (mean(src), mean(dst));
    let (mut sxx, mut num_a, mut num_b) = (0f32, 0f32, 0f32);
    for (p, q) in src.iter().zip(dst) {
        let (px, py) = (p[0] - ms[0], p[1] - ms[1]);
        let (qx, qy) = (q[0] - md[0], q[1] - md[1]);
        sxx += px * px + py * py;
        num_a += px * qx + py * qy;
        num_b += px * qy - py * qx;
    }
    let (a, b) = if sxx > 0.0 { (num_a / sxx, num_b / sxx) } else { (1.0, 0.0) };
    // [a -b; b a]
    let tx = md[0] - (a * ms[0] - b * ms[1]);
    let ty = md[1] - (b * ms[0] + a * ms[1]);
    [a, -b, tx, b, a, ty]
}

pub fn apply(m: &Affine, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
}

pub fn invert(m: &Affine) -> Affine {
    let det = m[0] * m[4] - m[1] * m[3];
    let det = if det.abs() < 1e-12 { 1e-12 } else { det };
    let (a, b, d, e) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    [a, b, -(a * m[2] + b * m[5]), d, e, -(d * m[2] + e * m[5])]
}

fn reflect_f(v: f32, n: u32) -> f32 {
    let max = (n - 1) as f32;
    if n <= 1 {
        return 0.0;
    }
    let period = 2.0 * max;
    let m = v.rem_euclid(period);
    if m <= max { m } else { period - m }
}

/// Bilinear sample of an RGB image (reflecting at the border).
fn sample(img: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = img.dimensions();
    let x = reflect_f(x, w);
    let y = reflect_f(y, h);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |xx, yy| img.get_pixel(xx, yy).0;
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    let mut out = [0f32; 3];
    for i in 0..3 {
        let top = a[i] as f32 * (1.0 - fx) + b[i] as f32 * fx;
        let bot = c[i] as f32 * (1.0 - fx) + d[i] as f32 * fx;
        out[i] = top * (1.0 - fy) + bot * fy;
    }
    out
}

/// Aligned `size × size` crop as planar RGB in 0..1: crop pixel (u, v) samples the image at
/// `inv(m)·(u, v)` where `m` maps image → crop.
pub fn warp_crop(img: &RgbImage, m: &Affine, size: u32) -> Vec<f32> {
    let inv = invert(m);
    let plane = (size * size) as usize;
    let mut out = vec![0f32; plane * 3];
    for v in 0..size {
        for u in 0..size {
            let (x, y) = apply(&inv, u as f32, v as f32);
            let p = sample(img, x, y);
            let k = (v * size + u) as usize;
            for c in 0..3 {
                out[c * plane + k] = p[c] / 255.0;
            }
        }
    }
    out
}

/// Feather mask for pasting a `size` crop back: zero `border` pixels at the edge, then a
/// Gaussian blur with `sigma` (separable).
pub fn paste_mask(size: u32, border: u32, sigma: f32) -> Vec<f32> {
    let n = size as usize;
    let b = border as usize;
    let mut m = vec![0f32; n * n];
    for y in b..n.saturating_sub(b) {
        for x in b..n.saturating_sub(b) {
            m[y * n + x] = 1.0;
        }
    }
    let r = (sigma * 3.0).ceil() as i64;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let ks: f32 = k.iter().sum();
    let blur = |src: &[f32], horiz: bool| {
        let mut dst = vec![0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                let mut acc = 0.0;
                for (j, w) in k.iter().enumerate() {
                    let o = j as i64 - r;
                    let (xx, yy) = if horiz { (x as i64 + o, y as i64) } else { (x as i64, y as i64 + o) };
                    if xx >= 0 && yy >= 0 && (xx as usize) < n && (yy as usize) < n {
                        acc += w * src[yy as usize * n + xx as usize];
                    }
                }
                dst[y * n + x] = acc / ks;
            }
        }
        dst
    };
    let m = blur(&m, true);
    blur(&m, false)
}

/// Blend a restored crop (planar RGB 0..1, `size²`) into `img` through the inverse of `m`.
pub fn paste(img: &mut RgbImage, crop: &[f32], m: &Affine, size: u32, mask: &[f32], strength: f32) {
    let inv = invert(m);
    let (w, h) = img.dimensions();
    let s = size as f32;
    // Bounding box of the crop square in the image.
    let corners = [(0.0, 0.0), (s, 0.0), (0.0, s), (s, s)].map(|(u, v)| apply(&inv, u, v));
    let x0 = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let y0 = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let x1 = (corners.iter().map(|c| c.0).fold(f32::MIN, f32::max).ceil() as i64).clamp(0, w as i64) as u32;
    let y1 = (corners.iter().map(|c| c.1).fold(f32::MIN, f32::max).ceil() as i64).clamp(0, h as i64) as u32;
    let plane = (size * size) as usize;
    let n = size as usize;
    let bil = |buf: &[f32], u: f32, v: f32| -> f32 {
        let (u0, v0) = (u.floor() as usize, v.floor() as usize);
        let (u1, v1) = ((u0 + 1).min(n - 1), (v0 + 1).min(n - 1));
        let (fu, fv) = (u - u0 as f32, v - v0 as f32);
        let t = buf[v0 * n + u0] * (1.0 - fu) + buf[v0 * n + u1] * fu;
        let b = buf[v1 * n + u0] * (1.0 - fu) + buf[v1 * n + u1] * fu;
        t * (1.0 - fv) + b * fv
    };
    for y in y0..y1 {
        for x in x0..x1 {
            let (u, v) = apply(m, x as f32 + 0.0, y as f32 + 0.0);
            if u < 0.0 || v < 0.0 || u > s - 1.0 || v > s - 1.0 {
                continue;
            }
            let a = bil(mask, u, v) * strength;
            if a <= 0.001 {
                continue;
            }
            let p = img.get_pixel_mut(x, y);
            for c in 0..3 {
                let r = (bil(&crop[c * plane..(c + 1) * plane], u, v) * 255.0).clamp(0.0, 255.0);
                p.0[c] = (p.0[c] as f32 * (1.0 - a) + r * a).round() as u8;
            }
        }
    }
}

/// Letterbox `img` into the detector input (BGR, 0..255, planar); returns the scale used.
pub fn detector_input(img: &RgbImage) -> (Vec<f32>, f32) {
    let (w, h) = img.dimensions();
    let r = (DET_SIZE as f32 / w as f32).min(DET_SIZE as f32 / h as f32);
    let (nw, nh) = (((w as f32 * r).round() as u32).max(1), ((h as f32 * r).round() as u32).max(1));
    let small = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
    let plane = (DET_SIZE * DET_SIZE) as usize;
    let mut out = vec![0f32; plane * 3];
    for (x, y, p) in small.enumerate_pixels() {
        let k = (y * DET_SIZE + x) as usize;
        out[k] = p.0[2] as f32;
        out[plane + k] = p.0[1] as f32;
        out[2 * plane + k] = p.0[0] as f32;
    }
    (out, r)
}

/// Face restoration stage.
pub struct FaceRestorer {
    pub name: String,
    detector: PathBuf,
    model: PathBuf,
    strength: f32,
    mask: Vec<f32>,
}

impl FaceRestorer {
    pub fn new(name: String, detector: PathBuf, model: PathBuf, strength: f32) -> FaceRestorer {
        FaceRestorer {
            name,
            detector,
            model,
            strength: strength.clamp(0.0, 1.0),
            mask: paste_mask(FACE_SIZE, 24, 12.0),
        }
    }

    fn sessions(&self, ctx: &EngineCtx) -> Result<(Arc<LoadedSession>, Arc<LoadedSession>), EngineError> {
        let det = session_for(&self.detector, ctx.gpu, 2).map_err(EngineError::Unavailable)?;
        let res = session_for(&self.model, ctx.gpu, 4).map_err(EngineError::Unavailable)?;
        Ok((det, res))
    }

    /// Faces in `img` (image pixels).
    pub fn detect(&self, img: &RgbImage, ctx: &EngineCtx) -> Result<Vec<Face>, EngineError> {
        let (det, _) = self.sessions(ctx)?;
        detect_with(&det, img).map_err(|e| EngineError::Unavailable(crate::t!("onnx.run_failed", name = "YuNet", error = short(&e))))
    }

    pub fn restore(&self, img: RgbImage, ctx: &EngineCtx) -> Result<RgbImage, EngineError> {
        if self.strength <= 0.0 {
            return Ok(img);
        }
        let (det, res) = self.sessions(ctx)?;
        let err = |e: String| {
            if runtime::is_oom(&e) {
                super::clear_cache();
                EngineError::Unavailable(crate::t!("onnx.oom_final", name = &self.name))
            } else {
                EngineError::Unavailable(crate::t!("onnx.run_failed", name = &self.name, error = short(&e)))
            }
        };
        let faces = detect_with(&det, &img).map_err(err)?;
        let mut out = img;
        // CodeFormer: fidelity weight (1 = close to the input, 0 = strongest restoration).
        let weight = 1.0 - self.strength;
        let extra = [("weight", weight), ("w", weight), ("fidelity", weight)];
        // CodeFormer blends via its weight; GFPGAN via the paste strength.
        let blend = if res.extra_inputs.is_empty() { self.strength } else { 1.0 };
        for f in faces {
            if ctx.cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let m = similarity(&f.kps, &FFHQ_512);
            let crop = warp_crop(&out, &m, FACE_SIZE);
            let restored = res.run(crop, FACE_SIZE, FACE_SIZE, &extra).map_err(err)?;
            if restored.len() != (3 * FACE_SIZE * FACE_SIZE) as usize {
                return Err(err(format!("unexpected output size {}", restored.len())));
            }
            paste(&mut out, &restored, &m, FACE_SIZE, &self.mask, blend);
        }
        Ok(out)
    }
}

fn detect_with(det: &LoadedSession, img: &RgbImage) -> Result<Vec<Face>, String> {
    let (input, r) = detector_input(img);
    let outs = det.run_all(input, DET_SIZE, DET_SIZE)?;
    let get = |name: &str| -> Result<&[f32], String> {
        outs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_slice())
            .ok_or_else(|| format!("detector output {name} missing"))
    };
    let mut strides = Vec::new();
    for s in [8u32, 16, 32] {
        strides.push(StrideOut {
            stride: s,
            cls: get(&format!("cls_{s}"))?,
            obj: get(&format!("obj_{s}"))?,
            bbox: get(&format!("bbox_{s}"))?,
            kps: get(&format!("kps_{s}"))?,
        });
    }
    let mut faces = nms(decode(&strides, DET_SIZE, SCORE_MIN), NMS_IOU);
    faces.truncate(MAX_FACES);
    let inv = 1.0 / r;
    for f in &mut faces {
        for v in &mut f.bbox {
            *v *= inv;
        }
        for p in &mut f.kps {
            p[0] *= inv;
            p[1] *= inv;
        }
    }
    faces.retain(|f| f.bbox[2].min(f.bbox[3]) >= MIN_FACE);
    Ok(faces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_recovers_a_known_transform() {
        // Rotate 30°, scale 1.7, translate (40, -12).
        let (c, s) = (30f32.to_radians().cos() * 1.7, 30f32.to_radians().sin() * 1.7);
        let m: Affine = [c, -s, 40.0, s, c, -12.0];
        let dst: Vec<[f32; 2]> = FFHQ_512.iter().map(|p| {
            let (x, y) = apply(&m, p[0], p[1]);
            [x, y]
        }).collect();
        let est = similarity(&FFHQ_512, &dst);
        for (a, b) in est.iter().zip(m.iter()) {
            assert!((a - b).abs() < 1e-2, "{est:?} vs {m:?}");
        }
        // Inverse round trip.
        let inv = invert(&est);
        let (x, y) = apply(&inv, dst[2][0], dst[2][1]);
        assert!((x - FFHQ_512[2][0]).abs() < 1e-2 && (y - FFHQ_512[2][1]).abs() < 1e-2);
    }

    #[test]
    fn decode_and_nms() {
        // 640 input, stride 32 → 20×20 grid; one strong anchor at col 5, row 3.
        let n = 400;
        let mut cls = vec![0f32; n];
        let mut obj = vec![0f32; n];
        let mut bbox = vec![0f32; n * 4];
        let mut kps = vec![0f32; n * 10];
        let idx = 3 * 20 + 5;
        cls[idx] = 0.9;
        obj[idx] = 0.9;
        bbox[idx * 4..idx * 4 + 4].copy_from_slice(&[0.5, 0.5, 2f32.ln(), 3f32.ln()]);
        kps[idx * 10] = 0.25;
        kps[idx * 10 + 1] = 0.75;
        // A weaker duplicate next to it and a below-threshold one.
        cls[idx + 1] = 0.8;
        obj[idx + 1] = 0.8;
        bbox[(idx + 1) * 4..(idx + 1) * 4 + 4].copy_from_slice(&[-0.5, 0.5, 2f32.ln(), 3f32.ln()]);
        cls[0] = 0.3;
        obj[0] = 0.3;
        let out = decode(
            &[StrideOut { stride: 32, cls: &cls, obj: &obj, bbox: &bbox, kps: &kps }],
            640,
            SCORE_MIN,
        );
        assert_eq!(out.len(), 2);
        let f = &nms(out, NMS_IOU)[0];
        // centre (5.5*32, 3.5*32) = (176, 112), size 64×96
        assert!((f.bbox[0] - 144.0).abs() < 1e-3 && (f.bbox[1] - 64.0).abs() < 1e-3);
        assert!((f.bbox[2] - 64.0).abs() < 1e-3 && (f.bbox[3] - 96.0).abs() < 1e-3);
        assert_eq!(f.kps[0], [5.25 * 32.0, 3.75 * 32.0]);
        assert!((f.score - 0.9).abs() < 1e-5);
    }

    #[test]
    fn mask_is_zero_at_the_edge_and_one_inside() {
        let m = paste_mask(128, 12, 6.0);
        assert!(m[0] < 1e-3);
        assert!(m[64 * 128 + 64] > 0.999);
        assert!(m[64 * 128 + 12] > 0.3 && m[64 * 128 + 12] < 0.7);
    }

    #[test]
    fn warp_then_paste_identity_keeps_the_image() {
        let img = RgbImage::from_fn(200, 160, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 77]));
        // Crop the region around (100, 80) at scale 2 and paste it back unchanged.
        let m: Affine = [2.0, 0.0, -2.0 * 40.0, 0.0, 2.0, -2.0 * 30.0];
        let crop = warp_crop(&img, &m, 128);
        let mut out = img.clone();
        let mask = paste_mask(128, 8, 4.0);
        paste(&mut out, &crop, &m, 128, &mask, 1.0);
        for (x, y, p) in out.enumerate_pixels() {
            let q = img.get_pixel(x, y);
            for c in 0..3 {
                assert!((p.0[c] as i32 - q.0[c] as i32).abs() <= 1, "{x},{y}: {p:?} vs {q:?}");
            }
        }
    }

    #[test]
    fn detector_input_is_bgr_letterboxed() {
        let img = RgbImage::from_pixel(1280, 640, image::Rgb([10, 20, 30]));
        let (v, r) = detector_input(&img);
        assert!((r - 0.5).abs() < 1e-6);
        let plane = (DET_SIZE * DET_SIZE) as usize;
        assert_eq!((v[0], v[plane], v[2 * plane]), (30.0, 20.0, 10.0));
        // Below the 320-row image: padding.
        assert_eq!(v[(400 * DET_SIZE) as usize], 0.0);
    }
}
