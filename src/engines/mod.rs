//! Enhancement engines behind traits.
//!
//! * [`Upscaler`] — AI super-resolution (`realesrgan-ncnn-vulkan`, future `ort` backend).
//! * [`Interpolator`] — frame generation (`rife-ncnn-vulkan`).
//! * `filters` / `resize` — pure-Rust CPU stages (rayon).

pub mod filters;
pub mod ort_stub;
pub mod realesrgan_ncnn;
pub mod resize;
pub mod rife_ncnn;

use std::path::Path;
use std::sync::Arc;

use crate::config::{Priority, Profile};
use crate::media::MediaKind;
use crate::paths::{ModelSet, ToolSet};
use crate::pipeline::cancel::CancelToken;
use crate::util::ProcError;

#[derive(thiserror::Error, Debug)]
pub enum EngineError {
    #[error("engine unavailable: {0}")]
    Unavailable(String),
    #[error("{0}")]
    Proc(#[from] ProcError),
    #[error("{engine} produced {got} of {expected} output frames\n{tail}")]
    MissingOutput {
        engine: String,
        got: usize,
        expected: usize,
        tail: String,
    },
    #[error("cancelled")]
    Cancelled,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl EngineError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            EngineError::Cancelled | EngineError::Proc(ProcError::Cancelled)
        )
    }
}

/// Per-invocation engine parameters.
pub struct EngineCtx<'a> {
    pub cancel: &'a CancelToken,
    /// Vulkan device index; `None` = CPU.
    pub gpu: Option<u32>,
    /// ncnn `-j load:proc:save`.
    pub jobs: String,
    /// ncnn tile size (0 = ncnn auto).
    pub tile: u32,
    pub priority: Priority,
    /// Called periodically with the number of output files produced so far.
    pub progress: Option<&'a (dyn Fn(usize) + Sync)>,
}

impl EngineCtx<'_> {
    pub fn gpu_arg(&self) -> String {
        self.gpu
            .map(|g| g.to_string())
            .unwrap_or_else(|| "-1".into())
    }
}

pub trait Upscaler: Send + Sync {
    fn name(&self) -> String;
    /// Scales the model can run natively.
    fn native_scales(&self) -> Vec<u32>;
    /// Upscale every image in `input` into `output` (same file names, PNG).
    fn upscale_dir(
        &self,
        input: &Path,
        output: &Path,
        scale: u32,
        ctx: &EngineCtx,
    ) -> Result<(), EngineError>;
    /// Whether the engine can run on the CPU (`-g -1`).
    fn supports_cpu(&self) -> bool;
}

pub trait Interpolator: Send + Sync {
    fn name(&self) -> String;
    fn supports_multiplier(&self, k: u32) -> bool;
    /// Interpolate the ordered frames in `input` to exactly `out_count` frames in `output`
    /// (`%08d.png`, starting at 1). Output `i` (0-based) sits at time `i * in_count / out_count`.
    fn interpolate_dir(
        &self,
        input: &Path,
        output: &Path,
        out_count: usize,
        uhd: bool,
        ctx: &EngineCtx,
    ) -> Result<(), EngineError>;
}

/// How the upscale stage is performed.
#[derive(Clone)]
pub enum UpscaleBackend {
    Ai(Arc<dyn Upscaler>),
    /// Integer nearest-neighbour (pixel art).
    Nearest,
    /// Lanczos3 (no AI).
    Lanczos,
}

impl UpscaleBackend {
    pub fn native_scales(&self) -> Vec<u32> {
        match self {
            UpscaleBackend::Ai(u) => u.native_scales(),
            _ => vec![],
        }
    }
    pub fn label(&self) -> String {
        match self {
            UpscaleBackend::Ai(u) => u.name(),
            UpscaleBackend::Nearest => crate::t!("engine.nearest"),
            UpscaleBackend::Lanczos => "Lanczos3".into(),
        }
    }
    pub fn is_ai(&self) -> bool {
        matches!(self, UpscaleBackend::Ai(_))
    }
}

pub struct BackendChoice {
    pub backend: UpscaleBackend,
    pub warning: Option<String>,
}

/// Concrete model for a profile, or an explanation of what's missing.
fn ai_model_for(
    profile: Profile,
    kind: MediaKind,
    models: &ModelSet,
) -> Result<Option<(String, Vec<u32>)>, String> {
    let anime_video = || {
        let scales = models.animevideo_scales();
        (!scales.is_empty()).then(|| ("realesr-animevideov3".to_string(), scales))
    };
    let single = |name: &str| models.has(name).then(|| (name.to_string(), vec![4]));
    let need =
        |name: &str| crate::t!("engine.missing_model", name = name);

    match profile {
        Profile::Realistic => single("realesrgan-x4plus")
            .map(Some)
            .ok_or_else(|| need("realesrgan-x4plus")),
        Profile::Anime => match kind {
            MediaKind::Video => anime_video()
                .map(Some)
                .ok_or_else(|| need("realesr-animevideov3-x2/-x3/-x4")),
            MediaKind::Image => single("realesrgan-x4plus-anime")
                .or_else(anime_video)
                .map(Some)
                .ok_or_else(|| need("realesrgan-x4plus-anime")),
        },
        Profile::General => Ok(single("realesr-general-x4v3").or_else(anime_video)),
        Profile::PixelArt | Profile::NoAi => Ok(None),
    }
}

/// Why a profile can't be used right now (UI tooltip), `None` if usable.
pub fn profile_unavailable(
    profile: Profile,
    kind: MediaKind,
    tools: &ToolSet,
    models: &ModelSet,
) -> Option<String> {
    match ai_model_for(profile, kind, models) {
        Err(e) => Some(e),
        Ok(Some(_)) if tools.realesrgan.is_none() => {
            Some(crate::t!("engine.realesrgan_missing"))
        }
        _ => None,
    }
}

/// Pick the upscale backend for a profile.
pub fn choose_backend(
    profile: Profile,
    kind: MediaKind,
    tools: &ToolSet,
    models: &ModelSet,
    cpu_only: bool,
) -> Result<BackendChoice, String> {
    match profile {
        Profile::PixelArt => {
            return Ok(BackendChoice {
                backend: UpscaleBackend::Nearest,
                warning: None,
            });
        }
        Profile::NoAi => {
            return Ok(BackendChoice {
                backend: UpscaleBackend::Lanczos,
                warning: None,
            });
        }
        _ => {}
    }
    let model = ai_model_for(profile, kind, models)?;
    let Some((name, scales)) = model else {
        return Ok(BackendChoice {
            backend: UpscaleBackend::Lanczos,
            warning: Some(crate::t!("engine.no_general_model")),
        });
    };
    let exe = tools
        .realesrgan
        .clone()
        .ok_or_else(|| crate::t!("engine.realesrgan_missing"))?;
    let engine =
        realesrgan_ncnn::RealEsrganNcnn::new(exe, models.realesrgan_dir.clone(), name, scales);
    if cpu_only && !engine.supports_cpu() {
        return Ok(BackendChoice {
            backend: UpscaleBackend::Lanczos,
            warning: Some(
                crate::t!("engine.no_cpu_mode"),
            ),
        });
    }
    Ok(BackendChoice {
        backend: UpscaleBackend::Ai(Arc::new(engine)),
        warning: None,
    })
}

/// Run `f(tile)`; on failure halve the tile size (0 = auto → 256) down to 32 and retry.
/// `out_dir` is emptied before each retry. Returns the tile size that worked.
pub fn with_tile_retry(
    initial: u32,
    out_dir: &Path,
    mut f: impl FnMut(u32) -> Result<(), EngineError>,
) -> Result<u32, EngineError> {
    let mut tile = initial;
    loop {
        match f(tile) {
            Ok(()) => return Ok(tile),
            Err(e) if e.is_cancelled() => return Err(e),
            Err(e @ EngineError::Unavailable(_)) => return Err(e),
            Err(e) => {
                let next = if tile == 0 { 256 } else { tile / 2 };
                if next < 32 {
                    return Err(e);
                }
                tracing::warn!("upscale failed with tile {tile}; retrying with tile {next}: {e}");
                if matches!(e, EngineError::Proc(crate::util::ProcError::GpuLost { .. })) {
                    // The driver needs a moment to recover from a device reset (TDR).
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
                let _ = std::fs::remove_dir_all(out_dir);
                std::fs::create_dir_all(out_dir)?;
                tile = next;
            }
        }
    }
}

/// Number of files in a directory (0 if missing).
pub fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| e.path().is_file()).count())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_retry_halves_until_success() {
        let dir = std::env::temp_dir().join(format!("enh-tile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut tried = vec![];
        let r = with_tile_retry(400, &dir, |t| {
            tried.push(t);
            if t > 100 {
                Err(EngineError::MissingOutput {
                    engine: "e".into(),
                    got: 0,
                    expected: 1,
                    tail: String::new(),
                })
            } else {
                Ok(())
            }
        });
        assert_eq!(r.unwrap(), 100);
        assert_eq!(tried, vec![400, 200, 100]);
        let r = with_tile_retry(64, &dir, |_| {
            Err(EngineError::MissingOutput {
                engine: "e".into(),
                got: 0,
                expected: 1,
                tail: String::new(),
            })
        });
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_fallbacks() {
        let empty = ModelSet::default();
        let tools = ToolSet::default();
        assert!(
            profile_unavailable(Profile::Realistic, MediaKind::Image, &tools, &empty).is_some()
        );
        assert!(profile_unavailable(Profile::PixelArt, MediaKind::Video, &tools, &empty).is_none());
        let c = choose_backend(Profile::General, MediaKind::Video, &tools, &empty, false).unwrap();
        assert!(matches!(c.backend, UpscaleBackend::Lanczos));
        assert!(c.warning.is_some());
    }
}
