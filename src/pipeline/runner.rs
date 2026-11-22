//! Queue runner: one background thread that executes jobs sequentially.
//! A failed job never stops the queue.

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::cancel::{CancelToken, PauseFlag};
use super::image_job::run_image;
use super::job::{Env, JobId, JobOutcome, JobSpec, ProgressHandle, Stage};
use super::video_job::{VideoOptions, run_video};
use crate::media::MediaKind;

pub enum RunnerEvent {
    Started(JobId),
    Finished(JobId, JobOutcome),
    QueueIdle,
}

#[derive(Clone)]
pub struct Runner {
    tx: Sender<Vec<JobSpec>>,
    pub events: Receiver<RunnerEvent>,
    current: Arc<Mutex<Option<CancelToken>>>,
    cancel_all: Arc<AtomicBool>,
    pub pause: PauseFlag,
    pub progress: ProgressHandle,
    busy: Arc<AtomicBool>,
}

impl Runner {
    pub fn start(env: Arc<Env>, repaint: Option<egui::Context>) -> Runner {
        let (tx, rx) = unbounded::<Vec<JobSpec>>();
        let (ev_tx, ev_rx) = unbounded();
        let r = Runner {
            tx,
            events: ev_rx,
            current: Arc::new(Mutex::new(None)),
            cancel_all: Arc::new(AtomicBool::new(false)),
            pause: PauseFlag::default(),
            progress: ProgressHandle::default(),
            busy: Arc::new(AtomicBool::new(false)),
        };
        let r2 = r.clone();
        let _ = std::thread::Builder::new()
            .name("job-runner".into())
            .spawn(move || r2.run(env, rx, ev_tx, repaint));
        r
    }

    /// Queue jobs (processed after anything already queued).
    pub fn submit(&self, jobs: Vec<JobSpec>) {
        self.cancel_all.store(false, Ordering::SeqCst);
        self.busy.store(true, Ordering::SeqCst);
        let _ = self.tx.send(jobs);
    }

    pub fn cancel_current(&self) {
        if let Ok(g) = self.current.lock()
            && let Some(t) = g.as_ref()
        {
            t.cancel();
        }
        self.pause.set(false);
    }

    pub fn cancel_all(&self) {
        self.cancel_all.store(true, Ordering::SeqCst);
        self.cancel_current();
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    fn run(
        self,
        env: Arc<Env>,
        rx: Receiver<Vec<JobSpec>>,
        ev: Sender<RunnerEvent>,
        repaint: Option<egui::Context>,
    ) {
        let notify = |e: RunnerEvent| {
            let _ = ev.send(e);
            if let Some(c) = &repaint {
                c.request_repaint();
            }
        };
        while let Ok(batch) = rx.recv() {
            for spec in batch {
                if self.cancel_all.load(Ordering::SeqCst) {
                    notify(RunnerEvent::Finished(spec.id, JobOutcome::Cancelled));
                    continue;
                }
                let token = CancelToken::new();
                if let Ok(mut g) = self.current.lock() {
                    *g = Some(token.clone());
                }
                notify(RunnerEvent::Started(spec.id));
                let name = spec
                    .info
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.progress
                    .start(spec.id, name, spec.info.frames.unwrap_or(1));
                tracing::info!("job {} started: {}", spec.id, spec.info.path.display());
                let outcome = self.run_one(&env, &spec, &token);
                match &outcome {
                    JobOutcome::Done { output, .. } => {
                        tracing::info!("job {} done → {}", spec.id, output.display())
                    }
                    JobOutcome::Failed(e) => tracing::error!("job {} failed: {e}", spec.id),
                    JobOutcome::Cancelled => tracing::info!("job {} cancelled", spec.id),
                }
                if let Ok(mut g) = self.current.lock() {
                    *g = None;
                }
                self.progress.stage(Stage::Idle);
                notify(RunnerEvent::Finished(spec.id, outcome));
            }
            if rx.is_empty() {
                self.busy.store(false, Ordering::SeqCst);
                self.progress.clear();
                notify(RunnerEvent::QueueIdle);
            }
        }
    }

    fn run_one(&self, env: &Env, spec: &JobSpec, token: &CancelToken) -> JobOutcome {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match spec.info.kind {
            _ if spec.edit.is_some() => crate::editor::run_edit(
                env,
                spec.edit.as_deref().expect("checked"),
                &spec.output,
                &self.progress,
                token,
            ),
            MediaKind::Image => run_image(env, spec, &self.progress, token),
            MediaKind::Video => run_video(
                env,
                spec,
                &self.progress,
                token,
                &self.pause,
                &VideoOptions::default(),
            )
            .map(|r| JobOutcome::Done {
                output: r.output,
                note: {
                    let mut n = format!(
                        "{}x{} @ {} fps · {} frames · {}",
                        r.out_w,
                        r.out_h,
                        r.out_fps,
                        r.frames,
                        r.encoder.label()
                    );
                    for w in r.notes {
                        n += &format!(" · ⚠ {w}");
                    }
                    n
                },
            }),
        }));
        match res {
            Ok(Ok(o)) => o,
            Ok(Err(e)) if e.is_cancelled() || token.is_cancelled() => JobOutcome::Cancelled,
            Ok(Err(e)) => JobOutcome::Failed(e.user_message()),
            Err(_) => JobOutcome::Failed(crate::t!("err.panic")),
        }
    }
}
