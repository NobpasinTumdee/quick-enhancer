//! The shared enhancement code path. Image export, video export and every preview call
//! these exact functions, so the preview shows what the export produces.
//!
//! Order: denoise (`pre_filter`) → [interpolate] → upscale (`upscale_dir` / Rust scaling)
//! → exact resize + sharpen (`post_process`).

use image::RgbImage;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::cancel::CancelToken;
use super::job::{Env, PipelineError};
use super::plan::{Resources, ScalePlan, effective_resources, plan_scale, resolve_tile};
use crate::config::{ComputeDevice, JobSettings, Profile, SharpenMode};
use crate::devices;
use crate::engines::filters;
use crate::engines::resize::{ResizeKind, resize_rgb};
use crate::engines::{EngineCtx, EngineError, UpscaleBackend, choose_backend, with_tile_retry};
use crate::media::MediaKind;
use crate::media::probe::MediaInfo;

#[derive(Clone, Debug)]
pub struct SharpenParams {
    pub mode: SharpenMode,
    pub cas_strength: f32,
    pub radius: f32,
    pub amount: f32,
    pub threshold: u8,
}

/// Fully resolved per-job processing plan.
pub struct StagePlan {
    pub kind: MediaKind,
    pub scale: ScalePlan,
    pub backend: UpscaleBackend,
    pub denoise: f32,
    pub sharpen: SharpenParams,
    /// Local contrast strength (0 = off), applied after sharpening.
    pub clarity: f32,
    /// Vulkan index, `None` = CPU.
    pub gpu: Option<u32>,
    pub device_label: String,
    pub resources: Resources,
    /// Current tile size (shrinks after an out-of-memory retry and stays shrunk).
    tile: AtomicU32,
    pub pool: Arc<rayon::ThreadPool>,
    pub warnings: Vec<String>,
}

impl StagePlan {
    pub fn build(env: &Env, s: &JobSettings, info: &MediaInfo) -> Result<StagePlan, PipelineError> {
        let gpu = devices::resolve(s.compute, &env.gpus);
        let cpu_only = gpu.is_none();
        let mut warnings = Vec::new();
        if s.compute == ComputeDevice::Cpu
            && s.profile != Profile::PixelArt
            && s.profile != Profile::NoAi
        {
            warnings
                .push(crate::t!("warn.cpu_slow"));
        } else if cpu_only && s.compute == ComputeDevice::Auto && !env.gpus.is_empty() {
            warnings.push(crate::t!("warn.no_gpu"));
        }
        let choice = choose_backend(s.profile, info.kind, &env.tools, &env.models, cpu_only)
            .map_err(PipelineError::Setup)?;
        if let Some(w) = choice.warning {
            warnings.push(w);
        }
        let even = info.kind == MediaKind::Video;
        let scale = plan_scale(
            info.width,
            info.height,
            s.scale,
            &choice.backend.native_scales(),
            even,
        );
        let resources = effective_resources(s, env.cores);
        let tile = resolve_tile(&resources, env.free_vram());
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(resources.filter_threads)
            .thread_name(|i| format!("filter-{i}"))
            .build()
            .map_err(|e| PipelineError::Setup(format!("thread pool: {e}")))?;
        let device_label = match gpu {
            Some(g) => format!("{} (Vulkan #{})", g.name, g.index),
            None => format!("CPU ({} threads)", resources.filter_threads),
        };
        Ok(StagePlan {
            kind: info.kind,
            scale,
            backend: choice.backend,
            denoise: s.denoise,
            sharpen: SharpenParams {
                mode: s.sharpen,
                cas_strength: s.cas_strength,
                radius: s.unsharp_radius,
                amount: s.unsharp_amount,
                threshold: s.unsharp_threshold,
            },
            clarity: s.clarity,
            gpu: gpu.map(|g| g.index),
            device_label,
            resources,
            tile: AtomicU32::new(tile),
            pool: Arc::new(pool),
            warnings,
        })
    }

    pub fn tile(&self) -> u32 {
        self.tile.load(Ordering::Relaxed)
    }

    /// AI stage needs image files on disk.
    pub fn needs_disk(&self) -> bool {
        self.backend.is_ai() && self.scale.model_scale.is_some()
    }

    /// Size of frames coming out of the AI stage (or source size without AI).
    pub fn model_dims(&self) -> (u32, u32) {
        match (self.backend.is_ai(), self.scale.model_scale) {
            (true, Some(m)) => (self.scale.src_w * m, self.scale.src_h * m),
            _ => (self.scale.src_w, self.scale.src_h),
        }
    }

    fn resize_kind(&self) -> ResizeKind {
        match self.backend {
            UpscaleBackend::Nearest => ResizeKind::Nearest,
            _ => ResizeKind::Lanczos3,
        }
    }

    /// Stage 1: denoise at source resolution.
    pub fn pre_filter(&self, img: RgbImage) -> RgbImage {
        if self.denoise <= 0.0 {
            return img;
        }
        self.pool.install(|| filters::denoise(&img, self.denoise))
    }

    /// Stage 3: exact resize to the output size (Rust scaling for non-AI profiles, or the
    /// down/up-scale after a mismatched model scale), then sharpen.
    pub fn post_process(&self, img: RgbImage) -> RgbImage {
        self.pool.install(|| {
            let (w, h) = (self.scale.out_w, self.scale.out_h);
            let img = if img.dimensions() != (w, h) {
                resize_rgb(&img, w, h, self.resize_kind())
            } else {
                img
            };
            let img = match self.sharpen.mode {
                SharpenMode::Off => img,
                SharpenMode::Cas => filters::cas(&img, self.sharpen.cas_strength),
                SharpenMode::Unsharp => filters::unsharp(
                    &img,
                    self.sharpen.radius,
                    self.sharpen.amount,
                    self.sharpen.threshold,
                ),
            };
            if self.clarity > 0.0 {
                filters::clarity(&img, self.clarity)
            } else {
                img
            }
        })
    }

    /// Stage 2 (AI): upscale every PNG in `input` into `output`, with tile-size retry.
    pub fn upscale_dir(
        &self,
        input: &Path,
        output: &Path,
        cancel: &CancelToken,
        progress: Option<&(dyn Fn(usize) + Sync)>,
    ) -> Result<(), PipelineError> {
        let (UpscaleBackend::Ai(engine), Some(scale)) = (&self.backend, self.scale.model_scale)
        else {
            return Ok(());
        };
        std::fs::create_dir_all(output)?;
        let used = with_tile_retry(self.tile(), output, |tile| {
            let ctx = EngineCtx {
                cancel,
                gpu: self.gpu,
                jobs: self.resources.jobs.clone(),
                tile,
                priority: self.resources.priority,
                progress,
            };
            engine.upscale_dir(input, output, scale, &ctx)
        })
        .map_err(|e| {
            if e.is_cancelled() {
                PipelineError::Cancelled
            } else {
                PipelineError::Engine(e)
            }
        })?;
        if used != self.tile() {
            tracing::warn!("tile size reduced to {used} for the rest of this job");
            self.tile.store(used, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Full still-image path (image export and all still previews).
    pub fn process_still(
        &self,
        img: RgbImage,
        work: &Path,
        cancel: &CancelToken,
    ) -> Result<RgbImage, PipelineError> {
        let img = self.pre_filter(img);
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let img = if self.needs_disk() {
            let in_dir = work.join("in");
            let out_dir = work.join("out");
            std::fs::create_dir_all(&in_dir)?;
            let _ = std::fs::remove_dir_all(&out_dir);
            write_png_fast(&img, &in_dir.join("00000001.png"))?;
            drop(img);
            self.upscale_dir(&in_dir, &out_dir, cancel, None)?;
            let out = image::open(out_dir.join("00000001.png"))
                .map_err(|e| {
                    PipelineError::Engine(EngineError::MissingOutput {
                        engine: self.backend.label(),
                        got: 0,
                        expected: 1,
                        tail: e.to_string(),
                    })
                })?
                .into_rgb8();
            let _ = std::fs::remove_dir_all(&in_dir);
            let _ = std::fs::remove_dir_all(&out_dir);
            out
        } else {
            img
        };
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        Ok(self.post_process(img))
    }

    pub fn describe(&self) -> String {
        let mut parts = vec![format!(
            "{}x{} → {}x{} · {}",
            self.scale.src_w,
            self.scale.src_h,
            self.scale.out_w,
            self.scale.out_h,
            self.backend.label()
        )];
        if self.scale.needs_resize || self.scale.model_scale.is_some() {
            parts.push(self.scale.note.clone());
        }
        parts.join(" · ")
    }
}

/// Measured PNG size / raw RGB size of real frames (EMA, in 1/10000), 0 = nothing yet.
static PNG_RATIO: AtomicU32 = AtomicU32::new(0);

/// PNG compression ratio measured on real frames written so far (feeds the temp estimate).
pub fn png_ratio() -> Option<f64> {
    match PNG_RATIO.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v as f64 / 10000.0),
    }
}

/// Record one written frame. Pure function of the old value for testability.
pub fn ema_ratio(old: u32, bytes: u64, pixels: u64) -> u32 {
    let r = ((bytes as f64 / (pixels.max(1) as f64 * 3.0)) * 10000.0).clamp(100.0, 15000.0) as u32;
    if old == 0 { r } else { (old as u64 * 9 / 10 + r as u64 / 10) as u32 }
}

/// PNG with fast compression (temp files for the ncnn tools). Calibrates [`png_ratio`].
pub fn write_png_fast(img: &RgbImage, path: &Path) -> Result<(), PipelineError> {
    write_png_uncalibrated(img, path)?;
    if let Ok(m) = std::fs::metadata(path) {
        let px = img.width() as u64 * img.height() as u64;
        let _ = PNG_RATIO.try_update(Ordering::Relaxed, Ordering::Relaxed, |old| Some(ema_ratio(old, m.len(), px)));
    }
    Ok(())
}

/// [`write_png_fast`] without touching the calibration (synthetic benchmark frames).
pub fn write_png_uncalibrated(img: &RgbImage, path: &Path) -> Result<(), PipelineError> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let f = std::fs::File::create(path)?;
    let w = std::io::BufWriter::with_capacity(1 << 20, f);
    PngEncoder::new_with_quality(w, CompressionType::Fast, FilterType::Sub).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_ratio_ema() {
        assert_eq!(super::ema_ratio(0, 300, 100), 10000);
        assert_eq!(super::ema_ratio(10000, 150, 100), 9500);
    }
}
