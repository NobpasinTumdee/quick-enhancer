//! Enhancement engines behind traits.
//!
//! * [`Upscaler`] — AI super-resolution (ncnn CLI tools, or ONNX models in-process).
//! * [`Interpolator`] — frame generation (`rife-ncnn-vulkan`).
//! * `filters` / `resize` — pure-Rust CPU stages (rayon).

pub mod filters;
pub mod onnx;
pub mod realcugan_ncnn;
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
    /// Works on in-memory images ([`Upscaler::upscale_image`]) without PNG files on disk.
    fn in_memory(&self) -> bool {
        false
    }
    /// Upscale one image in memory (only when [`Upscaler::in_memory`]).
    fn upscale_image(
        &self,
        _img: &image::RgbImage,
        _scale: u32,
        _ctx: &EngineCtx,
    ) -> Result<image::RgbImage, EngineError> {
        Err(EngineError::Unavailable(format!("{} works on files only", self.name())))
    }
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
    /// AI that runs on in-memory frames (ONNX).
    pub fn in_memory(&self) -> bool {
        matches!(self, UpscaleBackend::Ai(u) if u.in_memory())
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

/// Why a profile (with an optional explicit model) can't be used right now (UI tooltip),
/// `None` if usable.
pub fn profile_unavailable(
    profile: Profile,
    model: Option<&str>,
    kind: MediaKind,
    tools: &ToolSet,
    models: &ModelSet,
) -> Option<String> {
    if let Some(id) = model.filter(|_| uses_model(profile)) {
        return backend_for_model(id, tools, models).err();
    }
    match ai_model_for(profile, kind, models) {
        Err(e) => Some(e),
        Ok(Some(_)) if tools.realesrgan.is_none() => {
            Some(crate::t!("engine.realesrgan_missing"))
        }
        _ => None,
    }
}

/// Does the profile run an AI model (so an explicit model choice applies)?
pub fn uses_model(profile: Profile) -> bool {
    !matches!(profile, Profile::PixelArt | Profile::NoAi)
}

/// Upscaler for an explicit model id (catalog id or `custom:<name>`), or why it can't run.
pub fn backend_for_model(
    id: &str,
    tools: &ToolSet,
    models: &ModelSet,
) -> Result<Arc<dyn Upscaler>, String> {
    use crate::models::{Backend, CustomKind};
    let realesrgan = || {
        tools
            .realesrgan
            .clone()
            .ok_or_else(|| crate::t!("engine.realesrgan_missing"))
    };
    if crate::models::custom::is_custom_id(id) {
        let c = models
            .custom_model(id)
            .ok_or_else(|| crate::t!("engine.custom_missing", name = id))?;
        return match c.kind {
            CustomKind::Ncnn => {
                let dir = c.path.parent().map(Path::to_path_buf).unwrap_or_default();
                Ok(Arc::new(realesrgan_ncnn::RealEsrganNcnn::new(
                    realesrgan()?,
                    dir,
                    c.name.clone(),
                    vec![c.scale.unwrap_or(4)],
                )))
            }
            CustomKind::Onnx => {
                if !models.onnx_runtime {
                    return Err(crate::t!("engine.onnx_unavailable"));
                }
                Ok(Arc::new(onnx::OnnxUpscaler::from_custom(
                    c.name.clone(),
                    c.path.clone(),
                    c.scale,
                    None,
                )?))
            }
        };
    }
    let m = models
        .catalog
        .model(id)
        .ok_or_else(|| crate::t!("engine.unknown_model", name = id))?;
    if !models.catalog_files_present(m) {
        return Err(crate::t!("engine.model_not_installed", name = m.name.get()));
    }
    let ncnn_name = m.ncnn_name.clone().unwrap_or_default();
    match m.backend {
        Backend::NcnnRealesrgan => {
            let scales = if ncnn_name == "realesr-animevideov3" {
                models.animevideo_scales()
            } else {
                m.scales.clone()
            };
            Ok(Arc::new(realesrgan_ncnn::RealEsrganNcnn::new(
                realesrgan()?,
                models.realesrgan_dir.clone(),
                ncnn_name,
                scales,
            )))
        }
        Backend::NcnnRealcugan => {
            let exe = tools
                .realcugan
                .clone()
                .ok_or_else(|| crate::t!("engine.model_not_installed", name = m.name.get()))?;
            let dir = models.base.join("models").join("realcugan").join(&ncnn_name);
            Ok(Arc::new(realcugan_ncnn::RealCuganNcnn::new(exe, dir, m.scales.clone(), -1)))
        }
        Backend::Onnx => {
            if !models.onnx_runtime {
                return Err(crate::t!("engine.onnx_unavailable"));
            }
            let spec = m
                .onnx
                .as_ref()
                .ok_or_else(|| crate::t!("engine.not_an_upscaler", name = m.name.get()))?;
            if m.group != crate::models::Group::Upscale && m.group != crate::models::Group::Restore {
                return Err(crate::t!("engine.not_an_upscaler", name = m.name.get()));
            }
            Ok(Arc::new(onnx::OnnxUpscaler::from_catalog(
                m.name.get().to_string(),
                &models.base,
                spec,
                m.max_scale(),
            )))
        }
        Backend::NcnnRife => Err(crate::t!("engine.not_an_upscaler", name = m.name.get())),
    }
}

/// Pick the upscale backend for a profile, or for an explicit `model` (AI profiles only).
pub fn choose_backend(
    profile: Profile,
    model: Option<&str>,
    kind: MediaKind,
    tools: &ToolSet,
    models: &ModelSet,
    cpu_only: bool,
) -> Result<BackendChoice, String> {
    if let Some(id) = model.filter(|_| uses_model(profile)) {
        let engine = backend_for_model(id, tools, models)?;
        if cpu_only && !engine.supports_cpu() {
            return Ok(BackendChoice {
                backend: UpscaleBackend::Lanczos,
                warning: Some(crate::t!("engine.no_cpu_mode")),
            });
        }
        return Ok(BackendChoice {
            backend: UpscaleBackend::Ai(engine),
            warning: None,
        });
    }
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
            profile_unavailable(Profile::Realistic, None, MediaKind::Image, &tools, &empty).is_some()
        );
        assert!(profile_unavailable(Profile::PixelArt, None, MediaKind::Video, &tools, &empty).is_none());
        let c = choose_backend(Profile::General, None, MediaKind::Video, &tools, &empty, false).unwrap();
        assert!(matches!(c.backend, UpscaleBackend::Lanczos));
        assert!(c.warning.is_some());
    }

    #[test]
    fn explicit_models_resolve_or_explain() {
        let d = std::env::temp_dir().join(format!("qe-eng-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let touch = |rel: &str| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        let exe = |stem: &str| format!("bin/{stem}{}", std::env::consts::EXE_SUFFIX);
        let paths = crate::paths::AppPaths::from_base(d.clone());
        let models = ModelSet::discover(&paths.models);
        let tools = ToolSet::from_bin_dir(&paths.bin);
        // Not installed: a clear message, and the profile reports it too.
        let e = backend_for_model("realcugan-se", &tools, &models).err().unwrap();
        assert!(e.contains("Real-CUGAN"), "{e}");
        assert!(profile_unavailable(Profile::Anime, Some("realcugan-se"), MediaKind::Video, &tools, &models).is_some());
        // Pixel art ignores the model.
        assert!(profile_unavailable(Profile::PixelArt, Some("realcugan-se"), MediaKind::Video, &tools, &models).is_none());
        assert!(backend_for_model("nope", &tools, &models).is_err());
        assert!(backend_for_model("custom:missing", &tools, &models).is_err());
        // Installed Real-CUGAN: its engine, which can run on the CPU.
        touch(&exe("realcugan-ncnn-vulkan"));
        touch("bin/realcugan-ncnn-vulkan.exe");
        touch("models/realcugan/models-se/up2x-conservative.param");
        let models = ModelSet::discover(&paths.models);
        let tools = ToolSet::from_bin_dir(&paths.bin);
        let c = choose_backend(Profile::Anime, Some("realcugan-se"), MediaKind::Video, &tools, &models, true).unwrap();
        assert!(c.backend.label().contains("Real-CUGAN"));
        assert_eq!(c.backend.native_scales(), vec![2, 3, 4]);
        // Custom ncnn model: Real-ESRGAN engine with the scale from the file name.
        touch("models/custom/4x_Remacri.param");
        touch("models/custom/4x_Remacri.bin");
        touch(&exe("realesrgan-ncnn-vulkan"));
        let models = ModelSet::discover(&paths.models);
        let tools = ToolSet::from_bin_dir(&paths.bin);
        let u = backend_for_model("custom:4x_Remacri", &tools, &models).unwrap();
        assert_eq!(u.native_scales(), vec![4]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
