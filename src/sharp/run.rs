//! Runs the "Make sharp (AI)" stages on one image. Called from `StagePlan::process_still`, so
//! the preview (a region or the whole image) and the export share this code.

use image::RgbImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::analysis::{self, BlurReport};
use super::plan::{self, Resolved, SharpSettings};
use super::seedvr2::{Phase, PhaseTask};
use crate::devices::GpuInfo;
use crate::engines::filters;
use crate::engines::resize::{ResizeKind, resize_rgb};
use crate::engines::{EngineCtx, Upscaler};
use crate::models::RuntimeKind;
use crate::pipeline::cancel::CancelToken;
use crate::pipeline::job::{Env, PipelineError};
use crate::worker::{self, Task, WorkerError};

/// Where and how the generative model runs (in a worker process).
#[derive(Clone, Debug)]
pub struct Generative {
    pub model_dir: PathBuf,
    pub base: PathBuf,
    pub runtime: RuntimeKind,
    pub gpus: Vec<GpuInfo>,
    pub gpu: Option<u32>,
    pub vram_mb: u64,
    pub exe: PathBuf,
}

/// What was done to an image (for the job note and the UI).
#[derive(Clone, Debug, PartialEq)]
pub struct SharpReport {
    pub blur: BlurReport,
    pub resolved: Resolved,
    pub downscale: f32,
    /// The generative model contributed to the result.
    pub generative: bool,
    pub warnings: Vec<String>,
}

pub struct SharpPlan {
    pub settings: SharpSettings,
    /// Output size multiplier (1 = original size, 2, 4).
    pub mult: u32,
    /// Installed deblur models by catalog id.
    deblur: HashMap<String, Arc<dyn Upscaler>>,
    sr: Arc<dyn Upscaler>,
    pub generative: Option<Generative>,
}

fn engine_err(e: crate::engines::EngineError) -> PipelineError {
    if e.is_cancelled() { PipelineError::Cancelled } else { PipelineError::Engine(e) }
}

/// Catalog ids of every model this mode can use, for "Install missing parts".
pub fn model_ids() -> [&'static str; 4] {
    [plan::DEBLUR_MOTION, plan::DEBLUR_DEFOCUS, plan::FAITHFUL_SR, plan::GENERATIVE]
}

impl SharpPlan {
    pub fn build(env: &Env, s: &SharpSettings, mult: u32, gpu: Option<&GpuInfo>) -> Result<SharpPlan, String> {
        let mut deblur: HashMap<String, Arc<dyn Upscaler>> = HashMap::new();
        let mut ids: Vec<String> = vec![plan::DEBLUR_MOTION.into(), plan::DEBLUR_DEFOCUS.into()];
        if !s.deblur_model.is_empty() && s.deblur_model != plan::NONE {
            ids.push(s.deblur_model.clone());
        }
        for id in ids {
            if let Ok(b) = crate::engines::backend_for_model(&id, &env.tools, &env.models) {
                deblur.insert(id, b);
            }
        }
        let sr_id = if s.sr_model.is_empty() { plan::FAITHFUL_SR } else { s.sr_model.as_str() };
        let sr = crate::engines::backend_for_model(sr_id, &env.tools, &env.models)?;
        let generative = if s.creative_auto || s.creative > 0.0 { generative_for(env, gpu) } else { None };
        Ok(SharpPlan { settings: s.clone(), mult: mult.max(1), deblur, sr, generative })
    }

    /// Output size for a source of `w x h`.
    pub fn out_size(&self, w: u32, h: u32) -> (u32, u32) {
        plan::target_size(w, h, self.mult)
    }

    /// The whole mode on one image. `faces` restores faces on the result (when enabled).
    pub fn process(
        &self,
        img: RgbImage,
        ctx: &EngineCtx,
        work: &Path,
        faces: Option<&crate::engines::onnx::face::FaceRestorer>,
    ) -> Result<(RgbImage, SharpReport), PipelineError> {
        let cancel = ctx.cancel;
        let (w, h) = img.dimensions();
        let (tw, th) = self.out_size(w, h);
        let blur = analysis::analyse(&img);
        let r = plan::resolve(&self.settings, &blur);
        let mut warnings = Vec::new();
        tracing::info!("make sharp: {blur:?} → {r:?}");
        // The generative model works from the motion-deblurred image, but from the original for
        // other blur (the defocus model's overshoot hurts it; see docs/DEBLUR_EVAL.md).
        let gen_from_deblurred = r.deblur.as_deref() == Some(plan::DEBLUR_MOTION);
        let original = (r.creative > 0.0 && self.generative.is_some() && !gen_from_deblurred).then(|| img.clone());
        // 1. Deblur.
        let img = match &r.deblur {
            Some(id) => match self.deblur.get(id) {
                Some(m) => m.upscale_image(&img, 1, ctx).map_err(engine_err)?,
                None => return Err(PipelineError::Setup(crate::t!("sharp.model_missing", name = id))),
            },
            None => img,
        };
        check(cancel)?;
        // 2. Smart downscale to the effective sharp resolution.
        let f = r
            .downscale_fixed
            .unwrap_or_else(|| plan::downscale_factor(analysis::sharp_factor(&img), r.downscale_cap, r.preset));
        let (dw, dh) = plan::downscaled_size(w, h, f);
        let small = resize_rgb(&img, dw, dh, ResizeKind::Lanczos3);
        // 3. Rebuild detail (faithful), 4. back to the target size.
        let scale = self.sr.native_scales().into_iter().max().unwrap_or(1);
        let faithful = self.sr.upscale_image(&small, scale, ctx).map_err(engine_err)?;
        drop(small);
        let faithful = resize_rgb(&faithful, tw, th, ResizeKind::Lanczos3);
        check(cancel)?;
        // 3b. Generative detail, blended in by the Creative amount.
        let mut generative = false;
        let out = if r.creative > 0.0 {
            match &self.generative {
                Some(g) => {
                    let base = resize_rgb(original.as_ref().unwrap_or(&img), tw, th, ResizeKind::Lanczos3);
                    let gen_img = self.run_generative(g, &base, work, cancel, &mut warnings)?;
                    generative = true;
                    plan::blend(&faithful, &gen_img, r.creative)
                }
                None => {
                    // Auto quietly stays faithful; a Creative amount the user chose gets a note.
                    if !self.settings.creative_auto {
                        warnings.push(crate::t!("sharp.generative_missing"));
                    }
                    faithful
                }
            }
        } else {
            faithful
        };
        drop(img);
        // 5. Faces.
        let out = match (r.faces, faces) {
            (true, Some(fr)) => fr.restore(out, ctx).map_err(engine_err)?,
            _ => out,
        };
        check(cancel)?;
        // 6. Light final sharpen.
        let out = if self.settings.sharpen > 0.0 { filters::cas(&out, self.settings.sharpen) } else { out };
        Ok((out, SharpReport { blur, resolved: r, downscale: f, generative, warnings }))
    }

    /// The generative model at the target size (capped by GPU memory), in worker processes;
    /// shrinks tiles / the working size after out-of-memory errors.
    fn run_generative(
        &self,
        g: &Generative,
        base: &RgbImage,
        work: &Path,
        cancel: &CancelToken,
        warnings: &mut Vec<String>,
    ) -> Result<RgbImage, PipelineError> {
        // Free the GPU memory held by this process's sessions before the child starts.
        crate::engines::onnx::clear_cache();
        let (tw, th) = base.dimensions();
        let mut max_mp = plan::generative_max_mp(g.vram_mb);
        let mut tile = plan::generative_tile(g.vram_mb, self.settings.tile);
        loop {
            let (gw, gh) = plan::generative_size(tw, th, max_mp);
            let input = resize_rgb(base, gw, gh, ResizeKind::Lanczos3);
            match run_seedvr2(g, &input, tile, self.settings.seed, work, cancel) {
                Ok(img) => {
                    let img = plan::color_fix(&img, &input, plan::COLOR_FIX_SIGMA);
                    return Ok(resize_rgb(&img, tw, th, ResizeKind::Lanczos3));
                }
                Err((_, WorkerError::Cancelled)) => return Err(PipelineError::Cancelled),
                Err((Phase::Restore, WorkerError::Oom(m))) if max_mp > 0.6 => {
                    tracing::warn!("generative model out of memory at {gw}x{gh}: {m}");
                    max_mp /= 2.0;
                    warnings.push(crate::t!("sharp.oom_smaller", mp = format!("{max_mp:.1}")));
                }
                Err((_, WorkerError::Oom(m))) => match plan::smaller_tile(tile) {
                    Some(t) => {
                        tracing::warn!("generative model out of memory with tile {tile}: {m}");
                        warnings.push(crate::t!("sharp.oom_tile", tile = t));
                        tile = t;
                    }
                    None => return Err(PipelineError::Setup(crate::t!("sharp.oom_final"))),
                },
                Err((_, e)) => return Err(PipelineError::Setup(e.to_string())),
            }
        }
    }
}

fn check(c: &CancelToken) -> Result<(), PipelineError> {
    if c.is_cancelled() { Err(PipelineError::Cancelled) } else { Ok(()) }
}

/// Generative settings when the model and a GPU runtime are there.
pub fn generative_for(env: &Env, gpu: Option<&GpuInfo>) -> Option<Generative> {
    let gpu = gpu?;
    let m = env.models.catalog.model(plan::GENERATIVE)?;
    if !env.models.onnx_runtime || !env.models.catalog_files_present(m) {
        return None;
    }
    let first = m.files.first()?;
    let model_dir = env.models.base.join(first).parent()?.to_path_buf();
    let runtime = crate::engines::onnx::runtime::loaded_kind().unwrap_or_else(crate::engines::onnx::runtime::preferred_kind);
    Some(Generative {
        model_dir,
        base: env.models.base.clone(),
        runtime,
        gpus: env.gpus.clone(),
        gpu: Some(gpu.index),
        vram_mb: gpu.vram_bytes >> 20,
        exe: worker::worker_exe().ok()?,
    })
}

/// Encode → restore → decode, each phase in its own worker process.
pub fn run_seedvr2(
    g: &Generative,
    img: &RgbImage,
    tile: u32,
    seed: u64,
    work: &Path,
    cancel: &CancelToken,
) -> Result<RgbImage, (Phase, WorkerError)> {
    let dir = work.join("generative");
    std::fs::create_dir_all(&dir).map_err(|e| (Phase::Encode, WorkerError::Spawn(e.to_string())))?;
    let png = dir.join("in.png");
    img.save(&png).map_err(|e| (Phase::Encode, WorkerError::Spawn(e.to_string())))?;
    let task = |phase, input: &Path, output: &Path| {
        Task::Seedvr2(PhaseTask {
            phase,
            model_dir: g.model_dir.clone(),
            base: g.base.clone(),
            runtime: g.runtime,
            gpu: g.gpu,
            gpus: g.gpus.clone(),
            tile,
            seed,
            input: input.to_path_buf(),
            output: output.to_path_buf(),
        })
    };
    let steps = [
        (Phase::Encode, png.clone(), dir.join("latent.bin")),
        (Phase::Restore, dir.join("latent.bin"), dir.join("restored.bin")),
        (Phase::Decode, dir.join("restored.bin"), dir.join("out.png")),
    ];
    for (phase, input, output) in &steps {
        let t = std::time::Instant::now();
        worker::run(&g.exe, &task(*phase, input, output), &dir, cancel, &mut |_| {}).map_err(|e| (*phase, e))?;
        tracing::info!("generative {phase:?} done in {:.1}s", t.elapsed().as_secs_f32());
    }
    let out = image::open(dir.join("out.png"))
        .map_err(|e| (Phase::Decode, WorkerError::Failed(e.to_string())))?
        .into_rgb8();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(out)
}
