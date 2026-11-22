//! Job types, shared runtime environment, progress reporting.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use crate::config::JobSettings;
use crate::devices::GpuInfo;
use crate::engines::EngineError;
use crate::media::encoders::EncoderAvailability;
use crate::media::probe::MediaInfo;
use crate::monitor::Monitor;
use crate::paths::{ModelSet, ToolSet};
use crate::util::ProcError;

pub type JobId = u64;

/// Everything jobs need that doesn't change per job.
pub struct Env {
    pub tools: ToolSet,
    pub models: ModelSet,
    pub gpus: Vec<GpuInfo>,
    pub encoders: Arc<RwLock<EncoderAvailability>>,
    pub monitor: Option<Monitor>,
    pub cores: usize,
}

impl Env {
    pub fn free_vram(&self) -> Option<u64> {
        self.monitor.as_ref().and_then(|m| m.free_vram())
    }
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub id: JobId,
    pub info: MediaInfo,
    pub settings: JobSettings,
    pub output: PathBuf,
    /// Editor job (trim / crop / join / image edit) instead of enhancement.
    pub edit: Option<std::sync::Arc<crate::editor::EditJob>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobOutcome {
    Done { output: PathBuf, note: String },
    Failed(String),
    Cancelled,
}

#[derive(thiserror::Error, Debug)]
pub enum PipelineError {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Setup(String),
    #[error("{0}")]
    Engine(#[from] EngineError),
    #[error("{0}")]
    Proc(#[from] ProcError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
}

impl PipelineError {
    pub fn is_cancelled(&self) -> bool {
        match self {
            PipelineError::Cancelled => true,
            PipelineError::Engine(e) => e.is_cancelled(),
            PipelineError::Proc(ProcError::Cancelled) => true,
            _ => false,
        }
    }

    /// Localized message for the UI: a readable headline plus the technical details
    /// (tool stderr tails stay as the tool printed them).
    pub fn user_message(&self) -> String {
        use crate::t;
        let proc_msg = |e: &ProcError| match e {
            ProcError::Spawn { program, source } => {
                t!("err.spawn", program = program, error = source)
            }
            ProcError::Failed { program, code, tail } => {
                let mut s = t!("err.tool_failed", program = program, code = code);
                if !tail.trim().is_empty() {
                    s = format!("{s}
{tail}");
                }
                s
            }
            ProcError::GpuLost { program, tail } => {
                let s = t!("err.gpu_lost", program = program);
                if tail.trim().is_empty() { s } else { format!("{s}
{tail}") }
            }
            ProcError::Cancelled => t!("err.cancelled"),
            ProcError::Io(e) => t!("err.io", error = e),
        };
        match self {
            PipelineError::Cancelled => t!("err.cancelled"),
            PipelineError::Setup(s) => s.clone(),
            PipelineError::Engine(e) => match e {
                EngineError::Unavailable(s) => t!("err.engine_unavailable", reason = s),
                EngineError::Proc(p) => proc_msg(p),
                EngineError::MissingOutput { engine, got, expected, tail } => {
                    let s = t!("err.missing_output", engine = engine, got = got, expected = expected);
                    if tail.trim().is_empty() { s } else { format!("{s}
{tail}") }
                }
                EngineError::Cancelled => t!("err.cancelled"),
                EngineError::Io(e) => t!("err.io", error = e),
            },
            PipelineError::Proc(p) => proc_msg(p),
            PipelineError::Io(e) => t!("err.io", error = e),
            PipelineError::Image(e) => t!("err.image", error = e),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    Idle,
    Preparing,
    Extract,
    Denoise,
    Interpolate,
    Upscale,
    Sharpen,
    Encode,
    Finalizing,
}

impl Stage {
    pub fn label(self) -> String {
        match self {
            Stage::Idle => crate::t!("stage.idle"),
            Stage::Preparing => crate::t!("stage.preparing"),
            Stage::Extract => crate::t!("stage.extract"),
            Stage::Denoise => crate::t!("stage.denoise"),
            Stage::Interpolate => crate::t!("stage.interpolate"),
            Stage::Upscale => crate::t!("stage.upscale"),
            Stage::Sharpen => crate::t!("stage.sharpen"),
            Stage::Encode => crate::t!("stage.encode"),
            Stage::Finalizing => crate::t!("stage.finalizing"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct JobProgress {
    pub job: Option<JobId>,
    pub file: String,
    pub stage: Stage,
    /// Output frames (video) or items (image) completed.
    pub frames_done: u64,
    pub frames_total: u64,
    /// Source frames extracted so far (video).
    pub extracted: u64,
    pub fps: f32,
    pub eta_secs: Option<f64>,
    pub device: String,
    pub encoder: String,
    pub temp_bytes: u64,
    pub temp_peak: u64,
    pub detail: String,
    pub started: Option<Instant>,
}

impl JobProgress {
    pub fn fraction(&self) -> f32 {
        if self.frames_total == 0 {
            0.0
        } else {
            (self.frames_done as f32 / self.frames_total as f32).clamp(0.0, 1.0)
        }
    }
}

/// Cheap, cloneable progress sink shared with the UI.
#[derive(Clone, Default)]
pub struct ProgressHandle(pub Arc<Mutex<JobProgress>>);

impl ProgressHandle {
    pub fn update(&self, f: impl FnOnce(&mut JobProgress)) {
        if let Ok(mut g) = self.0.lock() {
            f(&mut g);
        }
    }

    pub fn get(&self) -> JobProgress {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn stage(&self, s: Stage) {
        self.update(|p| p.stage = s);
    }

    pub fn start(&self, job: JobId, file: String, total: u64) {
        self.update(|p| {
            *p = JobProgress {
                job: Some(job),
                file,
                frames_total: total,
                stage: Stage::Preparing,
                started: Some(Instant::now()),
                ..Default::default()
            }
        });
    }

    /// Set completed output frames and recompute speed / ETA.
    pub fn frames(&self, done: u64) {
        self.update(|p| {
            p.frames_done = done;
            if let Some(t) = p.started {
                let el = t.elapsed().as_secs_f64();
                if el > 0.5 && done > 0 {
                    let speed = done as f64 / el;
                    p.fps = speed as f32;
                    p.eta_secs = (p.frames_total > done)
                        .then(|| (p.frames_total - done) as f64 / speed)
                        .or(Some(0.0));
                }
            }
        });
    }

    pub fn temp(&self, bytes: u64) {
        self.update(|p| {
            p.temp_bytes = bytes;
            p.temp_peak = p.temp_peak.max(bytes);
        });
    }

    pub fn clear(&self) {
        self.update(|p| *p = JobProgress::default());
    }
}
