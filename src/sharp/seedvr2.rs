//! SeedVR2-3B (one-step diffusion restoration, ByteDance Seed, Apache-2.0) for single images,
//! on the ONNX graphs by Moonbox (`moonbox-dev/SeedVR2-3B-ONNX`). A port of their numpy
//! `pipeline.py` for one frame:
//!
//! 1. **Encode** (`vae_enc_first.onnx`): the image in [-1, 1] → latent moments; sampled with the
//!    seed and scaled by 0.9152. The causal VAE takes 5 frames: the frame is repeated and only
//!    latent 0 (which depends on frame 0 alone) is kept. Tiled with feathered seams.
//! 2. **Restore** (`dit.onnx`): one DiT step at t = 1000 from noise, conditioned on the latent,
//!    with the two window layouts and RoPE tables built here (`window_inputs`).
//! 3. **Decode** (`vae_dec_first.onnx`, 2 latents in, frame 0 kept), tiled.
//!
//! Each phase runs in its own worker process (`crate::worker`), so its GPU memory is returned to
//! Windows before the next one starts. Tensors pass between phases as small raw files.

use image::RgbImage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::devices::GpuInfo;
use crate::models::RuntimeKind;
use crate::worker::{Reporter, TaskError};

/// VAE scaling factor.
pub const SCALE: f32 = 0.9152;
/// Length of the text embedding baked into the DiT graph.
pub const TXT_LEN: usize = 58;
const WINDOW: (usize, usize, usize) = (4, 3, 3);
/// VAE tile overlap in pixels (8 latents).
const OVERLAP_PX: usize = 64;
/// Smallest VAE tile the out-of-memory retry goes down to.
pub const MIN_TILE: u32 = 128;
/// Frame sides must be multiples of this.
pub const ALIGN: u32 = 16;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Encode,
    Restore,
    Decode,
}

/// One phase, run by `QuickEnhancer.exe --worker`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PhaseTask {
    pub phase: Phase,
    /// Folder with `dit.onnx` (+ `.data`), `vae_enc_first.onnx`, `vae_dec_first.onnx`.
    pub model_dir: PathBuf,
    /// Tools base folder (holds `runtime/<kind>/onnxruntime.dll`).
    pub base: PathBuf,
    pub runtime: RuntimeKind,
    /// GPU (Vulkan index) and the adapter list, so the child maps it to the same device.
    pub gpu: Option<u32>,
    pub gpus: Vec<GpuInfo>,
    /// VAE tile in pixels (0 = whole image).
    pub tile: u32,
    pub seed: u64,
    /// Encode: PNG (sides multiples of 16). Restore / Decode: tensor file of the previous phase.
    pub input: PathBuf,
    /// Encode / Restore: tensor file. Decode: PNG.
    pub output: PathBuf,
}

// ---- deterministic noise ----------------------------------------------------------------------

/// SplitMix64 + Box-Muller: standard normal draws, reproducible for a seed.
pub struct Normal {
    state: u64,
    spare: Option<f32>,
}

impl Normal {
    pub fn new(seed: u64) -> Normal {
        Normal { state: seed ^ 0x9E37_79B9_7F4A_7C15, spare: None }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    pub fn sample(&mut self) -> f32 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let (u1, u2) = (self.uniform(), self.uniform());
        let r = (-2.0 * u1.ln()).sqrt();
        let t = std::f64::consts::TAU * u2;
        self.spare = Some((r * t.sin()) as f32);
        (r * t.cos()) as f32
    }
    pub fn fill(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.sample()).collect()
    }
}

// ---- windows and RoPE (make_720Pwindows_bysize / make_shifted_720Pwindows_bysize) --------------

/// Python's `round` (half to even).
fn round_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && (r as i64) % 2 != 0 { (r - x.signum()) as i64 } else { r as i64 }
}

type Win = (usize, usize, usize, usize, usize, usize);

/// Windows `(t0, t1, h0, h1, w0, w1)` over a `t x h x w` token grid, in the repository's order.
pub fn windows(t: usize, h: usize, w: usize, shifted: bool) -> Vec<Win> {
    let (rt, rh, rw) = WINDOW;
    let scale = ((45.0 * 80.0) / (h as f64 * w as f64)).sqrt();
    let sh_ = round_even(h as f64 * scale) as f64;
    let sw_ = round_even(w as f64 * scale) as f64;
    let wh = (sh_ / rh as f64).ceil() as usize;
    let ww = (sw_ / rw as f64).ceil() as usize;
    let wt = (t.min(30) as f64 / rt as f64).ceil() as usize;
    let (wt, wh, ww) = (wt.max(1), wh.max(1), ww.max(1));
    let mut out = Vec::new();
    if !shifted {
        let (nt, nh, nw) = (t.div_ceil(wt), h.div_ceil(wh), w.div_ceil(ww));
        for iw in 0..nw {
            if (iw * ww).min(w) >= ((iw + 1) * ww).min(w) {
                continue;
            }
            for ih in 0..nh {
                if (ih * wh).min(h) >= ((ih + 1) * wh).min(h) {
                    continue;
                }
                for it in 0..nt {
                    if (it * wt).min(t) >= ((it + 1) * wt).min(t) {
                        continue;
                    }
                    out.push((it * wt, ((it + 1) * wt).min(t), ih * wh, ((ih + 1) * wh).min(h), iw * ww, ((iw + 1) * ww).min(w)));
                }
            }
        }
        return out;
    }
    let st = if wt < t { 0.5 } else { 0.0 };
    let sh = if wh < h { 0.5 } else { 0.0 };
    let sw = if ww < w { 0.5 } else { 0.0 };
    let count = |n: usize, s: f64, win: usize| -> usize {
        if s > 0.0 { ((n as f64 - s) / win as f64).ceil() as usize + 1 } else { 1 }
    };
    let (nt, nh, nw) = (count(t, st, wt), count(h, sh, wh), count(w, sw, ww));
    let rng = |i: usize, s: f64, n: usize, size: usize| -> (usize, usize) {
        let a = ((i as f64 - s) * n as f64).trunc().max(0.0) as usize;
        let b = (((i as f64 - s + 1.0) * n as f64).trunc().max(0.0) as usize).min(size);
        (a, b)
    };
    for iw in 0..nw {
        let (w0, w1) = rng(iw, sw, ww, w);
        if w1 <= w0 {
            continue;
        }
        for ih in 0..nh {
            let (h0, h1) = rng(ih, sh, wh, h);
            if h1 <= h0 {
                continue;
            }
            for it in 0..nt {
                let (t0, t1) = rng(it, st, wt, t);
                if t1 <= t0 {
                    continue;
                }
                out.push((t0, t1, h0, h1, w0, w1));
            }
        }
    }
    out
}

/// The DiT's inputs for one window layout: idx [nw, S], mask [nw, S + txt], inv [L],
/// cos / sin [nw, S, 63].
pub struct WindowInputs {
    pub nw: usize,
    pub s: usize,
    pub idx: Vec<i64>,
    pub mask: Vec<i32>,
    pub inv: Vec<i64>,
    pub cos: Vec<f32>,
    pub sin: Vec<f32>,
}

pub fn window_inputs(t: usize, h: usize, w: usize, shifted: bool) -> WindowInputs {
    let wins = windows(t, h, w, shifted);
    let s = wins.iter().map(|&(t0, t1, h0, h1, w0, w1)| (t1 - t0) * (h1 - h0) * (w1 - w0)).max().unwrap_or(1);
    let nw = wins.len();
    let mut idx = vec![0i64; nw * s];
    let mut mask = vec![0i32; nw * (s + TXT_LEN)];
    let mut inv = vec![0i64; t * h * w];
    let mut pos = vec![0f32; nw * s * 3];
    for k in 0..nw {
        for j in s..s + TXT_LEN {
            mask[k * (s + TXT_LEN) + j] = 1;
        }
    }
    for (k, &(t0, t1, h0, h1, w0, w1)) in wins.iter().enumerate() {
        let mut n = 0;
        for tt in t0..t1 {
            for hh in h0..h1 {
                for ww_ in w0..w1 {
                    let tok = tt * h * w + hh * w + ww_;
                    idx[k * s + n] = tok as i64;
                    mask[k * (s + TXT_LEN) + n] = 1;
                    inv[tok] = (k * s + n) as i64;
                    let p = (k * s + n) * 3;
                    pos[p] = (tt - t0 + TXT_LEN) as f32;
                    pos[p + 1] = (hh - h0) as f32;
                    pos[p + 2] = (ww_ - w0) as f32;
                    n += 1;
                }
            }
        }
    }
    let freqs: Vec<f32> = (0..21).map(|i| 1.0 / 10000f32.powf((2 * i) as f32 / 42.0)).collect();
    let mut cos = vec![0f32; nw * s * 63];
    let mut sin = vec![0f32; nw * s * 63];
    for i in 0..nw * s {
        for a in 0..3 {
            for (j, f) in freqs.iter().enumerate() {
                let ang = pos[i * 3 + a] * f;
                cos[i * 63 + a * 21 + j] = ang.cos();
                sin[i * 63 + a * 21 + j] = ang.sin();
            }
        }
    }
    WindowInputs { nw, s, idx, mask, inv, cos, sin }
}

// ---- tiling --------------------------------------------------------------------------------------

/// Tile start positions of size `t` with at least `o` overlap covering `n`.
pub fn tile_starts(n: usize, t: usize, o: usize) -> Vec<usize> {
    if t == 0 || n <= t {
        return vec![0];
    }
    let step = (t - o).max(1);
    let mut v: Vec<usize> = (0..n - t).step_by(step).collect();
    v.push(n - t);
    v.dedup();
    v
}

/// Blend weights of one tile: linear ramps over the overlap on interior edges, 1 on borders.
pub fn edge_weights(th: usize, tw: usize, o: usize, top: bool, bottom: bool, left: bool, right: bool) -> Vec<f32> {
    let ramp = |n: usize, lo: bool, hi: bool| -> Vec<f32> {
        let mut v = vec![1f32; n];
        let o = o.min(n);
        for i in 0..o {
            let r = (i as f32 + 0.5) / o as f32;
            if !lo {
                v[i] = v[i].min(r);
            }
            if !hi {
                v[n - 1 - i] = v[n - 1 - i].min(r);
            }
        }
        v
    };
    let wy = ramp(th, top, bottom);
    let wx = ramp(tw, left, right);
    let mut out = Vec::with_capacity(th * tw);
    for y in &wy {
        for x in &wx {
            out.push(y * x);
        }
    }
    out
}

// ---- tensor files between phases -----------------------------------------------------------------

pub fn write_tensor(path: &Path, dims: &[usize], data: &[f32]) -> std::io::Result<()> {
    let mut b = Vec::with_capacity(4 + dims.len() * 4 + data.len() * 4);
    b.extend_from_slice(&(dims.len() as u32).to_le_bytes());
    for d in dims {
        b.extend_from_slice(&(*d as u32).to_le_bytes());
    }
    for v in data {
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, b)
}

pub fn read_tensor(path: &Path) -> std::io::Result<(Vec<usize>, Vec<f32>)> {
    let b = std::fs::read(path)?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "bad tensor file");
    let u = |i: usize| -> Option<u32> { b.get(i * 4..i * 4 + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())) };
    let nd = u(0).ok_or_else(bad)? as usize;
    let dims: Vec<usize> = (0..nd).map(|i| u(1 + i).map(|v| v as usize)).collect::<Option<_>>().ok_or_else(bad)?;
    let n: usize = dims.iter().product();
    let off = 4 * (1 + nd);
    if b.len() != off + 4 * n {
        return Err(bad());
    }
    let data = b[off..].as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    Ok((dims, data))
}

// ---- phases ----------------------------------------------------------------------------------------

fn session(t: &PhaseTask, name: &str) -> Result<std::sync::Arc<crate::engines::onnx::LoadedSession>, TaskError> {
    crate::engines::onnx::runtime::configure(&t.base, t.runtime, &t.gpus);
    crate::engines::onnx::session_lean(&t.model_dir.join(name), t.gpu).map_err(TaskError::from_message)
}

fn err(e: String) -> TaskError {
    TaskError::from_message(e)
}

/// Run one phase in this (worker) process.
pub fn run_phase(t: &PhaseTask, rep: &Reporter) -> Result<(), TaskError> {
    match t.phase {
        Phase::Encode => encode(t, rep),
        Phase::Restore => restore(t, rep),
        Phase::Decode => decode(t, rep),
    }
}

fn encode(t: &PhaseTask, rep: &Reporter) -> Result<(), TaskError> {
    let img = image::open(&t.input).map_err(|e| TaskError::Failed(e.to_string()))?.into_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w % ALIGN as usize != 0 || h % ALIGN as usize != 0 {
        return Err(TaskError::Failed(format!("image size {w}x{h} is not a multiple of {ALIGN}")));
    }
    let s = session(t, "vae_enc_first.onnx")?;
    rep.log(&format!("encode {w}x{h} on {}", s.ep.label()));
    let (lh, lw) = (h / 8, w / 8);
    let mut acc = vec![0f32; 32 * lh * lw];
    let mut wsum = vec![0f32; lh * lw];
    let tile = if t.tile == 0 { usize::MAX } else { t.tile as usize };
    let ys = tile_starts(h, tile.min(h), OVERLAP_PX);
    let xs = tile_starts(w, tile.min(w), OVERLAP_PX);
    let total = ys.len() * xs.len();
    let mut done = 0;
    for &y0 in &ys {
        for &x0 in &xs {
            let (th, tw) = (tile.min(h), tile.min(w));
            // [1, 3, 5, th, tw]: the tile repeated over 5 frames, in [-1, 1].
            let mut x = vec![0f32; 3 * 5 * th * tw];
            for c in 0..3 {
                for yy in 0..th {
                    for xx in 0..tw {
                        let v = img.get_pixel((x0 + xx) as u32, (y0 + yy) as u32)[c] as f32 / 127.5 - 1.0;
                        for f in 0..5 {
                            x[((c * 5 + f) * th + yy) * tw + xx] = v;
                        }
                    }
                }
            }
            let out = s.run_named(vec![("x".into(), crate::engines::onnx::Input::F32(vec![1, 3, 5, th, tw], x))], &["y"]).map_err(err)?;
            let (dims, m) = &out[0];
            // y [1, 32, n, th/8, tw/8]: latent 0 only.
            let (n, mh, mw) = (dims[2], dims[3], dims[4]);
            let wgt = edge_weights(mh, mw, OVERLAP_PX / 8, y0 == 0, y0 + th == h, x0 == 0, x0 + tw == w);
            for c in 0..32 {
                for yy in 0..mh {
                    for xx in 0..mw {
                        let v = m[((c * n) * mh + yy) * mw + xx];
                        let o = (y0 / 8 + yy) * lw + x0 / 8 + xx;
                        acc[c * lh * lw + o] += v * wgt[yy * mw + xx];
                        if c == 0 {
                            wsum[o] += wgt[yy * mw + xx];
                        }
                    }
                }
            }
            done += 1;
            rep.progress(done as f32 / total as f32);
        }
    }
    for c in 0..32 {
        for i in 0..lh * lw {
            acc[c * lh * lw + i] /= wsum[i].max(1e-6);
        }
    }
    // Sample the posterior, scale, to [h, w, 16]; then the initial noise.
    let mut rng = Normal::new(t.seed);
    let post = rng.fill(16 * lh * lw);
    let mut lat = vec![0f32; lh * lw * 16];
    for c in 0..16 {
        for i in 0..lh * lw {
            let mean = acc[c * lh * lw + i];
            let logvar = acc[(16 + c) * lh * lw + i].clamp(-30.0, 20.0);
            lat[i * 16 + c] = (mean + (0.5 * logvar).exp() * post[c * lh * lw + i]) * SCALE;
        }
    }
    let noise = rng.fill(lh * lw * 16);
    let mut both = lat;
    both.extend_from_slice(&noise);
    write_tensor(&t.output, &[2, lh, lw, 16], &both).map_err(|e| TaskError::Failed(e.to_string()))
}

fn restore(t: &PhaseTask, rep: &Reporter) -> Result<(), TaskError> {
    let (dims, data) = read_tensor(&t.input).map_err(|e| TaskError::Failed(e.to_string()))?;
    let (lh, lw) = (dims[1], dims[2]);
    let n = lh * lw * 16;
    let (lat, noise) = data.split_at(n);
    let mut vid = vec![0f32; lh * lw * 33];
    for i in 0..lh * lw {
        vid[i * 33..i * 33 + 16].copy_from_slice(&noise[i * 16..i * 16 + 16]);
        vid[i * 33 + 16..i * 33 + 32].copy_from_slice(&lat[i * 16..i * 16 + 16]);
        vid[i * 33 + 32] = 1.0;
    }
    let s = session(t, "dit.onnx")?;
    rep.log(&format!("restore {}x{} latents on {}", lw, lh, s.ep.label()));
    rep.progress(0.1);
    use crate::engines::onnx::Input;
    let mut inputs = vec![("vid".to_string(), Input::F32(vec![1, lh, lw, 33], vid))];
    for m in 0..2 {
        let wi = window_inputs(1, lh / 2, lw / 2, m == 1);
        inputs.push((format!("idx{m}"), Input::I64(vec![wi.nw, wi.s], wi.idx)));
        inputs.push((format!("mask{m}"), Input::I32(vec![wi.nw, wi.s + TXT_LEN], wi.mask)));
        inputs.push((format!("inv{m}"), Input::I64(vec![wi.inv.len()], wi.inv)));
        inputs.push((format!("cos{m}"), Input::F32(vec![wi.nw, wi.s, 63], wi.cos)));
        inputs.push((format!("sin{m}"), Input::F32(vec![wi.nw, wi.s, 63], wi.sin)));
    }
    let out = s.run_named(inputs, &["v"]).map_err(err)?;
    let v = &out[0].1;
    // x0 = noise - v; z = x0 / SCALE as [16, h, w].
    let mut z = vec![0f32; 16 * lh * lw];
    for i in 0..lh * lw {
        for c in 0..16 {
            z[c * lh * lw + i] = (noise[i * 16 + c] - v[i * 16 + c]) / SCALE;
        }
    }
    rep.progress(1.0);
    write_tensor(&t.output, &[16, lh, lw], &z).map_err(|e| TaskError::Failed(e.to_string()))
}

fn decode(t: &PhaseTask, rep: &Reporter) -> Result<(), TaskError> {
    let (dims, z) = read_tensor(&t.input).map_err(|e| TaskError::Failed(e.to_string()))?;
    let (lh, lw) = (dims[1], dims[2]);
    let (h, w) = (lh * 8, lw * 8);
    let s = session(t, "vae_dec_first.onnx")?;
    rep.log(&format!("decode {w}x{h} on {}", s.ep.label()));
    let lt = if t.tile == 0 { usize::MAX } else { (t.tile as usize / 8).max(8) };
    let o = OVERLAP_PX / 8;
    let ys = tile_starts(lh, lt.min(lh), o);
    let xs = tile_starts(lw, lt.min(lw), o);
    let mut acc = vec![0f32; 3 * h * w];
    let mut wsum = vec![0f32; h * w];
    let total = ys.len() * xs.len();
    let mut done = 0;
    for &y0 in &ys {
        for &x0 in &xs {
            let (th, tw) = (lt.min(lh), lt.min(lw));
            // [1, 16, 2, th, tw]: the latent tile repeated twice.
            let mut x = vec![0f32; 16 * 2 * th * tw];
            for c in 0..16 {
                for yy in 0..th {
                    for xx in 0..tw {
                        let v = z[(c * lh + y0 + yy) * lw + x0 + xx];
                        for f in 0..2 {
                            x[((c * 2 + f) * th + yy) * tw + xx] = v;
                        }
                    }
                }
            }
            let out = s.run_named(vec![("x".into(), crate::engines::onnx::Input::F32(vec![1, 16, 2, th, tw], x))], &["y"]).map_err(err)?;
            let (d, y) = &out[0];
            let (n, ph, pw) = (d[2], d[3], d[4]);
            let wgt = edge_weights(ph, pw, o * 8, y0 == 0, y0 + th == lh, x0 == 0, x0 + tw == lw);
            for c in 0..3 {
                for yy in 0..ph {
                    for xx in 0..pw {
                        let v = y[((c * n) * ph + yy) * pw + xx];
                        let oi = (y0 * 8 + yy) * w + x0 * 8 + xx;
                        acc[c * h * w + oi] += v * wgt[yy * pw + xx];
                        if c == 0 {
                            wsum[oi] += wgt[yy * pw + xx];
                        }
                    }
                }
            }
            done += 1;
            rep.progress(done as f32 / total as f32);
        }
    }
    let img = RgbImage::from_fn(w as u32, h as u32, |x, y| {
        let i = y as usize * w + x as usize;
        let px = |c: usize| (((acc[c * h * w + i] / wsum[i].max(1e-6)).clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0).round() as u8;
        image::Rgb([px(0), px(1), px(2)])
    });
    img.save(&t.output).map_err(|e| TaskError::Failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_rounding() {
        assert_eq!(round_even(2.5), 2);
        assert_eq!(round_even(3.5), 4);
        assert_eq!(round_even(2.4), 2);
        assert_eq!(round_even(-2.5), -2);
    }

    #[test]
    fn windows_cover_every_token_once() {
        for &(h, w) in &[(36, 64), (45, 80), (60, 34), (17, 23)] {
            for shifted in [false, true] {
                let wi = window_inputs(1, h, w, shifted);
                let mut seen = vec![0u8; h * w];
                for k in 0..wi.nw {
                    for j in 0..wi.s {
                        if wi.mask[k * (wi.s + TXT_LEN) + j] == 1 {
                            seen[wi.idx[k * wi.s + j] as usize] += 1;
                        }
                    }
                }
                assert!(seen.iter().all(|&c| c == 1), "{h}x{w} shifted={shifted}");
                // inv points back at each token's slot.
                for tok in 0..h * w {
                    assert_eq!(wi.idx[wi.inv[tok] as usize] as usize, tok);
                }
            }
        }
    }

    #[test]
    fn window_layout_matches_the_reference_for_720p() {
        // 1280x720 → latents 160x90 → tokens 80x45: the repository's 720p layout is 3x3 windows of
        // 27x15 tokens (unshifted); the shifted one adds half windows (4x4).
        let a = windows(1, 45, 80, false);
        assert_eq!(a.len(), 9);
        assert_eq!(a[0], (0, 1, 0, 15, 0, 27));
        let b = windows(1, 45, 80, true);
        assert_eq!(b.len(), 16);
        assert_eq!(b[0], (0, 1, 0, 7, 0, 13));
    }

    #[test]
    fn windows_match_the_python_pipeline() {
        // (h, w, shifted, count, first, last) from Moonbox's pipeline.py `windows`.
        let cases = [
            (36, 64, false, 9, (0, 1, 0, 15, 0, 27), (0, 1, 30, 36, 54, 64)),
            (36, 64, true, 9, (0, 1, 0, 7, 0, 13), (0, 1, 22, 36, 40, 64)),
            (45, 80, true, 16, (0, 1, 0, 7, 0, 13), (0, 1, 37, 45, 67, 80)),
            (60, 34, false, 9, (0, 1, 0, 27, 0, 15), (0, 1, 54, 60, 30, 34)),
            (60, 34, true, 9, (0, 1, 0, 13, 0, 7), (0, 1, 40, 60, 22, 34)),
            (17, 23, true, 1, (0, 1, 0, 17, 0, 23), (0, 1, 0, 17, 0, 23)),
            (68, 120, false, 25, (0, 1, 0, 15, 0, 27), (0, 1, 60, 68, 108, 120)),
            (68, 120, true, 30, (0, 1, 0, 7, 0, 13), (0, 1, 67, 68, 94, 120)),
        ];
        for (h, w, s, n, first, last) in cases {
            let v = windows(1, h, w, s);
            assert_eq!((v.len(), v[0], *v.last().unwrap()), (n, first, last), "{h}x{w} shifted={s}");
        }
    }

    #[test]
    fn tiles_cover_with_overlap_and_weights_sum_to_one_inside() {
        assert_eq!(tile_starts(100, 200, 8), vec![0]);
        let s = tile_starts(1000, 512, 64);
        assert_eq!(s.first(), Some(&0));
        assert_eq!(s.last(), Some(&(1000 - 512)));
        for p in s.windows(2) {
            assert!(p[0] + 512 >= p[1] + 64, "{s:?}");
        }
        let w = edge_weights(8, 8, 4, true, false, true, true);
        assert_eq!(w[0], 1.0);
        assert!(w[7 * 8] < 0.2);
    }

    #[test]
    fn tensor_files_round_trip() {
        let p = std::env::temp_dir().join(format!("qe-tensor-{}.bin", std::process::id()));
        write_tensor(&p, &[2, 3], &[1.0, -2.0, 3.5, 0.0, 5.0, 6.0]).unwrap();
        let (d, v) = read_tensor(&p).unwrap();
        assert_eq!(d, vec![2, 3]);
        assert_eq!(v[2], 3.5);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn noise_is_deterministic_and_standard() {
        let a = Normal::new(7).fill(20000);
        assert_eq!(a[..5], Normal::new(7).fill(5)[..]);
        let mean = a.iter().sum::<f32>() / a.len() as f32;
        let var = a.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / a.len() as f32;
        assert!(mean.abs() < 0.03 && (var - 1.0).abs() < 0.05, "{mean} {var}");
    }
}
