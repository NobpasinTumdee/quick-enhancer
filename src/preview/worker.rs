//! Background preview rendering. Latest request wins; the running one is cancelled
//! (child processes killed) when a new request arrives.

use crossbeam_channel::{Receiver, Sender, unbounded};
use image::RgbImage;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::JobSettings;
use crate::engines::resize::fit_within;
use crate::media::MediaKind;
use crate::media::ffmpeg::{DecodeSpec, decode_clip, extract_frame};
use crate::media::probe::{self, MediaInfo};
use crate::pipeline::cancel::{CancelToken, PauseFlag};
use crate::pipeline::image_job::load_rgb_alpha;
use crate::pipeline::job::{Env, JobSpec, PipelineError, ProgressHandle};
use crate::pipeline::stages::StagePlan;
use crate::pipeline::temp::{TempDir, temp_root};
use crate::pipeline::video_job::{VideoOptions, run_video};

/// Memory cap for decoded sample-clip frames (both sides together).
const SAMPLE_BUDGET: u64 = 640 << 20;

#[derive(Clone, Debug)]
pub enum PreviewKind {
    /// Single image, or one video frame at `time`.
    Still { time: f64 },
    /// Process a short clip with all settings (incl. frame generation) and play it.
    Sample { start: f64, dur: f64 },
    /// Compare the source with an exported output file.
    CompareOutput { output: PathBuf, time: f64 },
}

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub item: u64,
    pub info: MediaInfo,
    pub settings: JobSettings,
    pub kind: PreviewKind,
}

pub enum PreviewResult {
    Still {
        item: u64,
        before: RgbImage,
        after: RgbImage,
        note: String,
        secs: f64,
    },
    Sample {
        item: u64,
        original: Vec<RgbImage>,
        enhanced: Vec<RgbImage>,
        src_fps: f64,
        out_fps: f64,
        src_size: (u32, u32),
        out_size: (u32, u32),
        audio: Option<crate::media::ffmpeg::AudioClip>,
        note: String,
    },
    Error {
        item: u64,
        msg: String,
    },
}

#[derive(Clone)]
pub struct PreviewWorker {
    tx: Sender<(PreviewRequest, CancelToken)>,
    pub results: Receiver<PreviewResult>,
    current: Arc<Mutex<Option<CancelToken>>>,
    busy: Arc<AtomicBool>,
    pub progress: ProgressHandle,
}

impl PreviewWorker {
    pub fn start(env: Arc<Env>, repaint: Option<egui::Context>) -> PreviewWorker {
        let (tx, rx) = unbounded::<(PreviewRequest, CancelToken)>();
        let (res_tx, res_rx) = unbounded();
        let w = PreviewWorker {
            tx,
            results: res_rx,
            current: Arc::new(Mutex::new(None)),
            busy: Arc::new(AtomicBool::new(false)),
            progress: ProgressHandle::default(),
        };
        let w2 = w.clone();
        let _ = std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || w2.run(env, rx, res_tx, repaint));
        w
    }

    pub fn request(&self, req: PreviewRequest) {
        self.cancel();
        let token = CancelToken::new();
        if let Ok(mut g) = self.current.lock() {
            *g = Some(token.clone());
        }
        self.busy.store(true, Ordering::SeqCst);
        let _ = self.tx.send((req, token));
    }

    pub fn cancel(&self) {
        if let Ok(g) = self.current.lock()
            && let Some(t) = g.as_ref()
        {
            t.cancel();
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    fn run(
        self,
        env: Arc<Env>,
        rx: Receiver<(PreviewRequest, CancelToken)>,
        tx: Sender<PreviewResult>,
        repaint: Option<egui::Context>,
    ) {
        while let Ok(mut job) = rx.recv() {
            // Only the most recent request matters.
            while let Ok(newer) = rx.try_recv() {
                job = newer;
            }
            let (req, token) = job;
            if token.is_cancelled() {
                if rx.is_empty() {
                    self.busy.store(false, Ordering::SeqCst);
                }
                continue;
            }
            let item = req.item;
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render(&env, &req, &token, &self.progress)
            }));
            let out = match res {
                Ok(Ok(r)) => Some(r),
                Ok(Err(e)) if e.is_cancelled() || token.is_cancelled() => None,
                Ok(Err(e)) => Some(PreviewResult::Error {
                    item,
                    msg: e.user_message(),
                }),
                Err(_) => Some(PreviewResult::Error {
                    item,
                    msg: crate::t!("err.preview_crashed"),
                }),
            };
            if let Some(r) = out
                && !token.is_cancelled()
            {
                let _ = tx.send(r);
            }
            if rx.is_empty() {
                self.busy.store(false, Ordering::SeqCst);
            }
            if let Some(c) = &repaint {
                c.request_repaint();
            }
        }
    }
}

fn source_still(
    env: &Env,
    info: &MediaInfo,
    time: f64,
    cancel: &CancelToken,
) -> Result<RgbImage, PipelineError> {
    match info.kind {
        MediaKind::Image => Ok(load_rgb_alpha(&info.path)?.0),
        MediaKind::Video => {
            let ffmpeg = env
                .tools
                .ffmpeg
                .as_ref()
                .ok_or_else(|| PipelineError::Setup(crate::t!("err.ffmpeg_missing")))?;
            Ok(extract_frame(
                ffmpeg,
                &info.path,
                time,
                info.width,
                info.height,
                false,
                Some(cancel),
            )?)
        }
    }
}

fn render(
    env: &Env,
    req: &PreviewRequest,
    cancel: &CancelToken,
    progress: &ProgressHandle,
) -> Result<PreviewResult, PipelineError> {
    let t0 = Instant::now();
    let info = &req.info;
    match &req.kind {
        PreviewKind::Still { time } => {
            let before = source_still(env, info, *time, cancel)?;
            let plan = StagePlan::build(env, &req.settings, info)?;
            let root = temp_root(req.settings.video.temp_dir.as_deref());
            let work = TempDir::new(&root, "preview")?;
            let after = plan.process_still(before.clone(), work.path(), cancel)?;
            let mut note = plan.describe();
            for w in &plan.warnings {
                note += &format!(" · ⚠ {w}");
            }
            Ok(PreviewResult::Still {
                item: req.item,
                before,
                after,
                note,
                secs: t0.elapsed().as_secs_f64(),
            })
        }
        PreviewKind::CompareOutput { output, time } => {
            let before = source_still(env, info, *time, cancel)?;
            let out_info = probe::probe(env.tools.ffprobe.as_deref(), output)
                .map_err(|e| PipelineError::Setup(crate::t!("err.cannot_read_output", e = e)))?;
            let after = source_still(env, &out_info, *time, cancel)?;
            Ok(PreviewResult::Still {
                item: req.item,
                before,
                after,
                note: crate::t!(
                    "preview.exported_file",
                    path = output.display(),
                    info = out_info.summary()
                ),
                secs: t0.elapsed().as_secs_f64(),
            })
        }
        PreviewKind::Sample { start, dur } => {
            let ffmpeg = env
                .tools
                .ffmpeg
                .clone()
                .ok_or_else(|| PipelineError::Setup(crate::t!("err.ffmpeg_missing")))?;
            let root = temp_root(req.settings.video.temp_dir.as_deref());
            let work = TempDir::new(&root, "sample")?;
            let ext = req.settings.output.container.ext();
            let spec = JobSpec {
                id: u64::MAX,
                info: info.clone(),
                settings: req.settings.clone(),
                output: work.path().join(format!("sample.{ext}")),
            };
            let dur = dur.min(info.duration.map_or(*dur, |d| (d - start).max(0.1)));
            progress.start(0, "sample".into(), 0);
            let r = run_video(
                env,
                &spec,
                progress,
                cancel,
                &PauseFlag::default(),
                &VideoOptions {
                    segment: Some((*start, dur)),
                },
            )?;
            let src_fps = info.fps.map(|f| f.as_f64()).unwrap_or(30.0);
            let out_fps = r.out_fps.as_f64();
            // Display size: fit to 960x540, then shrink further to respect the memory cap.
            let (mut dw, mut dh) = fit_within(r.out_w, r.out_h, 960, 540);
            let n = (dur * (src_fps + out_fps)).ceil() as u64 + 4;
            while dw > 160 && dw as u64 * dh as u64 * 3 * n > SAMPLE_BUDGET {
                dw = dw * 4 / 5;
                dh = dh * 4 / 5;
            }
            let (dw, dh) = (dw.max(2) & !1, dh.max(2) & !1);
            let max_o = (dur * src_fps).ceil() as usize + 2;
            let max_e = (dur * out_fps).ceil() as usize + 2;
            let original = decode_clip(
                &ffmpeg,
                &DecodeSpec {
                    input: info.path.clone(),
                    segment: Some((*start, dur)),
                    fps: info.fps,
                    width: dw,
                    height: dh,
                    scale: true,
                },
                max_o,
                cancel,
            )?;
            let enhanced = decode_clip(
                &ffmpeg,
                &DecodeSpec {
                    input: r.output.clone(),
                    segment: None,
                    fps: None,
                    width: dw,
                    height: dh,
                    scale: true,
                },
                max_e,
                cancel,
            )?;
            // The segment's audio (from the source: frame generation keeps the duration).
            let audio = if info.audio_codecs.is_empty() {
                None
            } else {
                crate::media::ffmpeg::decode_audio(&ffmpeg, &info.path, Some((*start, dur)), cancel)
                    .unwrap_or_else(|e| {
                        tracing::warn!("sample audio decode failed: {e}");
                        None
                    })
            };
            let mut note = crate::t!(
                "preview.sample_note",
                secs = format!("{dur:.1}"),
                src = format!("{}x{} @ {:.3} fps", info.width, info.height, src_fps),
                out = format!("{}x{} @ {} fps", r.out_w, r.out_h, r.out_fps),
                time = format!("{:.1}", t0.elapsed().as_secs_f64())
            );
            for w in r.notes {
                note += &format!(" · ⚠ {w}");
            }
            Ok(PreviewResult::Sample {
                item: req.item,
                original,
                enhanced,
                src_fps,
                out_fps,
                src_size: (info.width, info.height),
                out_size: (r.out_w, r.out_h),
                audio,
                note,
            })
        }
    }
}
