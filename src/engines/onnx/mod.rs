//! ONNX backend: image-to-image models (super-resolution, 1x restoration) run in-process with
//! ONNX Runtime (DirectML / CUDA / CPU), tiled with feather blending, frames kept in memory.

pub mod face;
pub mod runtime;
pub mod tiler;

use half::f16;
use image::RgbImage;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, TensorElementType, ValueType};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use self::runtime::Ep;
use self::tiler::{Range, TileSpec, run_tiled};
use super::{EngineCtx, EngineError, Upscaler, count_files};
use crate::models::catalog::{self, OnnxSpec, Precision};

/// A loaded session and what it runs on.
pub struct LoadedSession {
    pub session: Mutex<Session>,
    pub ep: Ep,
    pub input: String,
    pub output: String,
    pub half: bool,
    /// Fixed input size from the graph (`Some((w, h))` for static exports).
    pub fixed: Option<(u32, u32)>,
    /// Scalar side inputs (name, shape), e.g. CodeFormer's fidelity weight.
    pub extra_inputs: Vec<(String, Vec<i64>)>,
    pub outputs: Vec<String>,
}

type CacheKey = (PathBuf, Option<u32>);

/// Recently used sessions (loading a model takes 0.5–5 s; previews reuse them).
static CACHE: Mutex<Vec<(CacheKey, Arc<LoadedSession>)>> = Mutex::new(Vec::new());
const CACHE_MAX: usize = 3;

/// Drop cached sessions (frees VRAM, e.g. after a job or when the user switches models).
pub fn clear_cache() {
    if let Ok(mut c) = CACHE.lock() {
        c.clear();
    }
}

fn build(env: &runtime::Loaded, path: &Path, ep: Ep, threads: usize) -> Result<Session, String> {
    use ort::ep;
    let e = |x: &dyn std::fmt::Display| x.to_string();
    let mut b = Session::builder(&env.env)
        .map_err(|x| e(&x))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|x| e(&x))?;
    match ep {
        Ep::DirectMl(dev) => {
            // DirectML needs sequential execution and no memory pattern.
            b = b
                .with_memory_pattern(false)
                .map_err(|x| e(&x))?
                .with_parallel_execution(false)
                .map_err(|x| e(&x))?
                .with_execution_providers([ep::DirectML::default().with_device_id(dev).build().error_on_failure()])
                .map_err(|x| e(&x))?;
        }
        Ep::Cuda(dev) => {
            b = b
                .with_execution_providers([ep::CUDA::default().with_device_id(dev).build().error_on_failure()])
                .map_err(|x| e(&x))?;
        }
        Ep::Cpu => {
            b = b.with_intra_threads(threads.max(1)).map_err(|x| e(&x))?;
        }
    }
    b.commit_from_file(path).map_err(|x| e(&x))
}

type Described = (String, String, bool, Option<(u32, u32)>, Vec<(String, Vec<i64>)>, Vec<String>);

fn describe(s: &Session) -> Described {
    let inp = &s.inputs()[0];
    let (half, fixed) = match inp.dtype() {
        ValueType::Tensor { ty, shape, .. } => {
            let dims: Vec<i64> = shape.iter().copied().collect();
            let fixed = (dims.len() == 4 && dims[2] > 0 && dims[3] > 0).then(|| (dims[3] as u32, dims[2] as u32));
            (*ty == TensorElementType::Float16, fixed)
        }
        _ => (false, None),
    };
    let extra = s
        .inputs()
        .iter()
        .skip(1)
        .map(|o| {
            let dims = match o.dtype() {
                ValueType::Tensor { shape, .. } => shape.iter().copied().collect(),
                _ => Vec::new(),
            };
            (o.name().to_string(), dims)
        })
        .collect();
    let outputs: Vec<String> = s.outputs().iter().map(|o| o.name().to_string()).collect();
    (inp.name().to_string(), outputs[0].clone(), half, fixed, extra, outputs)
}

/// Load (or reuse) a session for `path` on `gpu` (Vulkan index), falling back along the EP chain.
pub fn session_for(path: &Path, gpu: Option<u32>, threads: usize) -> Result<Arc<LoadedSession>, String> {
    let key = (path.to_path_buf(), gpu);
    if let Ok(c) = CACHE.lock()
        && let Some((_, s)) = c.iter().find(|(k, _)| *k == key)
    {
        return Ok(s.clone());
    }
    let env = runtime::environment()?;
    let info = runtime::gpu_info(gpu);
    let chain = runtime::ep_chain(env.kind, info.as_ref(), &runtime::gpus());
    let mut last = String::new();
    for (i, ep) in chain.iter().enumerate() {
        let t = std::time::Instant::now();
        match build(env, path, *ep, threads) {
            Ok(s) => {
                tracing::info!(
                    "ONNX session {} on {} in {:.1}s",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    ep.label(),
                    t.elapsed().as_secs_f32()
                );
                if i > 0 {
                    runtime::notice(crate::t!(
                        "onnx.ep_fallback",
                        from = chain[0].label(),
                        to = ep.label(),
                        error = short(&last)
                    ));
                }
                let (input, output, half, fixed, extra_inputs, outputs) = describe(&s);
                let ls = Arc::new(LoadedSession {
                    session: Mutex::new(s),
                    ep: *ep,
                    input,
                    output,
                    half,
                    fixed,
                    extra_inputs,
                    outputs,
                });
                if let Ok(mut c) = CACHE.lock() {
                    c.retain(|(k, _)| *k != key);
                    c.insert(0, (key, ls.clone()));
                    c.truncate(CACHE_MAX);
                }
                return Ok(ls);
            }
            Err(e) => {
                tracing::warn!("ONNX session on {} failed: {e}", ep.label());
                last = e;
            }
        }
    }
    Err(crate::t!("onnx.session_failed", name = path.display(), error = short(&last)))
}

pub(crate) fn short(s: &str) -> String {
    let s = s.lines().next().unwrap_or(s);
    if s.chars().count() > 240 {
        format!("{}…", s.chars().take(240).collect::<String>())
    } else {
        s.to_string()
    }
}

impl LoadedSession {
    /// Run one NCHW tile (`extra` = scalar inputs such as CodeFormer's fidelity weight) and
    /// return the main output.
    pub fn run(&self, data: Vec<f32>, w: u32, h: u32, extra: &[(&str, f32)]) -> Result<Vec<f32>, String> {
        let names = [self.output.clone()];
        Ok(self.run_outputs(data, w, h, extra, &names)?.remove(0).1)
    }

    /// Run and return every output (e.g. the face detector's per-stride heads).
    pub fn run_all(&self, data: Vec<f32>, w: u32, h: u32) -> Result<Vec<(String, Vec<f32>)>, String> {
        let names = self.outputs.clone();
        self.run_outputs(data, w, h, &[], &names)
    }

    fn run_outputs(
        &self,
        data: Vec<f32>,
        w: u32,
        h: u32,
        extra: &[(&str, f32)],
        names: &[String],
    ) -> Result<Vec<(String, Vec<f32>)>, String> {
        let shape = [1usize, 3, h as usize, w as usize];
        let mut s = self.session.lock().map_err(|e| e.to_string())?;
        let e = |x: ort::Error| x.to_string();
        let mut inputs: Vec<(String, ort::value::DynValue)> = Vec::with_capacity(1 + extra.len());
        if self.half {
            let v: Vec<f16> = data.into_iter().map(f16::from_f32).collect();
            inputs.push((self.input.clone(), Tensor::from_array((shape, v)).map_err(e)?.into_dyn()));
        } else {
            inputs.push((self.input.clone(), Tensor::from_array((shape, data)).map_err(e)?.into_dyn()));
        }
        for (name, dims) in &self.extra_inputs {
            let v = extra.iter().find(|(n, _)| n == name).map(|(_, v)| *v).unwrap_or(0.5);
            let shape: Vec<usize> = dims.iter().map(|&d| d.max(1) as usize).collect();
            let t = if self.half {
                Tensor::from_array((shape, vec![f16::from_f32(v)])).map_err(e)?.into_dyn()
            } else {
                Tensor::from_array((shape, vec![v])).map_err(e)?.into_dyn()
            };
            inputs.push((name.clone(), t));
        }
        let out = s.run(inputs).map_err(e)?;
        let mut res = Vec::with_capacity(names.len());
        for n in names {
            let o = &out[n.as_str()];
            let v = match o.try_extract_tensor::<f32>() {
                Ok((_, v)) => v.to_vec(),
                Err(_) => {
                    let (_, v) = o.try_extract_tensor::<f16>().map_err(e)?;
                    v.iter().map(|x| x.to_f32()).collect()
                }
            };
            res.push((n.clone(), v));
        }
        Ok(res)
    }
}

/// An ONNX image-to-image model.
pub struct OnnxUpscaler {
    name: String,
    path: PathBuf,
    scale: u32,
    tile: u32,
    static_tile: bool,
    range: Range,
}

/// Overlap between tiles (input pixels).
pub fn overlap_for(tile: u32) -> u32 {
    (tile / 16).clamp(8, 32)
}

impl OnnxUpscaler {
    pub fn from_catalog(name: String, base: &Path, spec: &OnnxSpec, scale: u32) -> OnnxUpscaler {
        let _ = spec.precision == Precision::Fp16; // the session reports its own input type
        OnnxUpscaler {
            name,
            path: base.join(&spec.file),
            scale,
            tile: spec.tile,
            static_tile: spec.static_tile,
            range: match spec.range {
                catalog::Range::Unit => Range::Unit,
                catalog::Range::PlusMinusOne => Range::PlusMinusOne,
            },
        }
    }

    /// User model from the custom folder: 0..1 range assumed (the chaiNNer / spandrel export
    /// convention); static input size and the scale are read from the model (scale by a small
    /// probe run when the file name doesn't say).
    pub fn from_custom(name: String, path: PathBuf, scale: Option<u32>, gpu: Option<u32>) -> Result<OnnxUpscaler, String> {
        let s = session_for(&path, gpu, 4)?;
        let (tile, static_tile) = match s.fixed {
            Some((w, h)) => (w.min(h), true),
            None => (256, false),
        };
        let scale = match scale {
            Some(v) => v,
            None => {
                let n = if static_tile { tile } else { 32 };
                let out = s.run(vec![0.5; (3 * n * n) as usize], n, n, &[])?;
                let ratio = ((out.len() / 3) as f64 / (n * n) as f64).sqrt().round() as u32;
                if !(1..=8).contains(&ratio) {
                    return Err(format!("could not detect the scale of {name}"));
                }
                ratio
            }
        };
        Ok(OnnxUpscaler { name, path, scale, tile, static_tile, range: Range::Unit })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Tiled run with out-of-memory handling: dynamic models retry with half the tile size
    /// (down to 64); static ones (fixed tile) can't shrink and report a clear error.
    pub fn run_image(&self, img: &RgbImage, ctx: &EngineCtx) -> Result<RgbImage, EngineError> {
        let threads = ctx.jobs.split(':').nth(1).and_then(|v| v.parse().ok()).unwrap_or(4);
        let mut tile = if !self.static_tile && ctx.tile >= 64 {
            self.tile.min(ctx.tile)
        } else {
            self.tile
        };
        loop {
            let s = session_for(&self.path, ctx.gpu, threads).map_err(EngineError::Unavailable)?;
            if self.static_tile
                && let Some((w, _)) = s.fixed
            {
                tile = w;
            }
            let spec = TileSpec {
                tile,
                static_tile: self.static_tile,
                overlap: overlap_for(tile),
                scale: self.scale,
                multiple: 8,
            };
            let r = run_tiled(
                img,
                &spec,
                self.range,
                |data, w, h| s.run(data, w, h, &[]),
                |_, _| !ctx.cancel.is_cancelled(),
            );
            match r {
                Ok(out) => return Ok(out),
                Err(_) if ctx.cancel.is_cancelled() => return Err(EngineError::Cancelled),
                Err(e) if runtime::is_oom(&e) => {
                    // A removed / exhausted DirectML device needs a fresh session either way.
                    drop(s);
                    clear_cache();
                    if self.static_tile || tile <= 64 {
                        return Err(EngineError::Unavailable(crate::t!("onnx.oom_final", name = &self.name)));
                    }
                    let next = (tile / 2).max(64);
                    runtime::notice(crate::t!("onnx.oom_retry", from = tile, to = next));
                    tile = next;
                }
                Err(e) => {
                    return Err(EngineError::Unavailable(crate::t!(
                        "onnx.run_failed",
                        name = &self.name,
                        error = short(&e)
                    )));
                }
            }
        }
    }
}

impl Upscaler for OnnxUpscaler {
    fn name(&self) -> String {
        format!("{} (ONNX)", self.name)
    }

    fn native_scales(&self) -> Vec<u32> {
        vec![self.scale]
    }

    fn supports_cpu(&self) -> bool {
        true
    }

    fn in_memory(&self) -> bool {
        true
    }

    fn upscale_image(&self, img: &RgbImage, _scale: u32, ctx: &EngineCtx) -> Result<RgbImage, EngineError> {
        self.run_image(img, ctx)
    }

    /// Directory mode (used when frames are on disk anyway, e.g. after frame generation).
    fn upscale_dir(&self, input: &Path, output: &Path, scale: u32, ctx: &EngineCtx) -> Result<(), EngineError> {
        std::fs::create_dir_all(output)?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(input)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for f in files {
            if ctx.cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let img = image::open(&f)
                .map_err(|e| EngineError::Unavailable(format!("{}: {e}", f.display())))?
                .into_rgb8();
            let out = self.upscale_image(&img, scale, ctx)?;
            let name = f.with_extension("png");
            let dest = output.join(name.file_name().unwrap_or_default());
            crate::pipeline::stages::write_png_uncalibrated(&out, &dest)
                .map_err(|e| EngineError::Unavailable(e.to_string()))?;
            if let Some(p) = ctx.progress {
                p(count_files(output));
            }
        }
        Ok(())
    }
}
