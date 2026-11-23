//! Bounded, chunked video pipeline.
//!
//! ```text
//! ffmpeg decode (rgb24 pipe, CFR) ─▶ decoder thread: scene-cut signature, denoise,
//!        PNG chunk dirs (≤ 2 on disk, enforced by a 2-token semaphore)
//!   ─▶ processor (this thread): RIFE (source res) ▶ Real-ESRGAN ▶ load ▶ resize ▶ sharpen
//!   ─▶ bounded channel ─▶ writer thread ─▶ ffmpeg encode (+ audio/subs/metadata mux)
//! ```
//! Without RIFE and AI the frames never touch the disk (in-memory fast path).

use crossbeam_channel::{Receiver, RecvTimeoutError, SendTimeoutError, Sender, bounded};
use image::RgbImage;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::cancel::{CancelToken, PauseFlag};
use super::job::{Env, JobSpec, PipelineError, ProgressHandle, Stage};
use super::plan::{FpsPlan, plan_fps};
use super::scene::{SceneDetector, fill_plan};
use super::stages::{StagePlan, write_png_fast};
use super::temp::{TempDir, dir_size, estimate_temp_bytes, free_space, temp_root};
use crate::config::EncoderChoice;
use crate::engines::rife_ncnn::RifeNcnn;
use crate::engines::{EngineCtx, Interpolator};
use crate::media::Rational;
use crate::media::encoders::VideoEncoder;
use crate::media::ffmpeg::{DecodeSpec, Decoder, EncodeSpec, Encoder, MuxSource};

const MB: u64 = 1 << 20;
const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub struct VideoResult {
    pub output: PathBuf,
    pub out_w: u32,
    pub out_h: u32,
    pub out_fps: Rational,
    pub frames: u64,
    pub encoder: VideoEncoder,
    pub notes: Vec<String>,
}

/// Options that differ between export and the preview sample.
#[derive(Clone, Debug, Default)]
pub struct VideoOptions {
    /// (start, duration) seconds — `Some` for the preview sample.
    pub segment: Option<(f64, f64)>,
}

struct ChunkMsg {
    index: usize,
    dir: PathBuf,
    len: u64,
    overlap: bool,
    /// Chunk-local pair indices `j` with a cut between frame `j` and `j+1`.
    cuts: Vec<u64>,
}

/// `<name>.part.<ext>` next to the final output.
pub fn part_path(output: &Path) -> PathBuf {
    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    output.with_extension(format!("part.{ext}"))
}

fn wait_for_encoders(env: &Env, cancel: &CancelToken) -> Result<(), PipelineError> {
    for _ in 0..600 {
        if env.encoders.read().map(|a| a.done).unwrap_or(true) {
            return Ok(());
        }
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

fn resolve_encoder(env: &Env, choice: EncoderChoice) -> Result<VideoEncoder, PipelineError> {
    let av = env
        .encoders
        .read()
        .map_err(|_| PipelineError::Setup("encoder state poisoned".into()))?;
    match choice {
        EncoderChoice::Auto => av.auto().ok_or_else(|| {
            PipelineError::Setup(crate::t!("err.no_encoder"))
        }),
        EncoderChoice::Specific(e) => {
            if av.done && !av.usable(e) {
                Err(PipelineError::Setup(format!(
                    "{} is unavailable: {}",
                    e.label(),
                    av.reason(e)
                )))
            } else {
                Ok(e)
            }
        }
    }
}

fn build_interpolator(
    env: &Env,
    spec: &JobSpec,
    fps: &FpsPlan,
) -> Result<Option<RifeNcnn>, PipelineError> {
    if fps.multiplier <= 1 {
        return Ok(None);
    }
    let exe = env.tools.rife.clone().ok_or_else(|| {
        PipelineError::Setup(crate::t!("err.rife_missing"))
    })?;
    let model = env
        .models
        .rife_model(&spec.settings.rife_model)
        .ok_or_else(|| PipelineError::Setup(crate::t!("err.no_rife_model")))?;
    let r = RifeNcnn::new(
        exe,
        model.path.clone(),
        model.name.clone(),
        model.arbitrary_count,
    );
    if !r.supports_multiplier(fps.multiplier) {
        return Err(PipelineError::Setup(format!(
            "RIFE model {} only supports 2x; pick a rife-v4.x model for {}x",
            model.name, fps.multiplier
        )));
    }
    Ok(Some(r))
}

/// Send with periodic stop checks. Returns false if stopped or the receiver is gone.
fn send_checked<T>(tx: &Sender<T>, mut v: T, stop: &CancelToken) -> bool {
    loop {
        match tx.send_timeout(v, POLL) {
            Ok(()) => return true,
            Err(SendTimeoutError::Timeout(back)) => {
                if stop.is_cancelled() {
                    return false;
                }
                v = back;
            }
            Err(SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}

fn recv_checked<T>(rx: &Receiver<T>, stop: &CancelToken) -> Option<T> {
    loop {
        match rx.recv_timeout(POLL) {
            Ok(v) => return Some(v),
            Err(RecvTimeoutError::Timeout) => {
                if stop.is_cancelled() {
                    return None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn frame_name(i: u64) -> String {
    format!("{:08}.png", i + 1)
}

#[allow(clippy::too_many_arguments)]
pub fn run_video(
    env: &Env,
    spec: &JobSpec,
    progress: &ProgressHandle,
    cancel: &CancelToken,
    pause: &PauseFlag,
    opts: &VideoOptions,
) -> Result<VideoResult, PipelineError> {
    let s = &spec.settings;
    let info = &spec.info;
    let ffmpeg = env
        .tools
        .ffmpeg
        .clone()
        .ok_or_else(|| PipelineError::Setup(crate::t!("err.ffmpeg_missing")))?;
    let src_fps = info.fps.unwrap_or(Rational::new(30, 1));
    let fps = plan_fps(src_fps, s.framegen, s.max_fps);
    let k = fps.multiplier;
    let interp = build_interpolator(env, spec, &fps)?;
    let plan = Arc::new(StagePlan::build(env, s, info)?);
    wait_for_encoders(env, cancel)?;
    let encoder = resolve_encoder(env, s.output.encoder)?;

    let mut notes: Vec<String> = plan.warnings.clone();
    if let Some(n) = &fps.note {
        notes.push(n.clone());
    }
    if info.vfr {
        notes.push(crate::t!("note.vfr_converted", fps = src_fps));
    }
    if info.needs_sdr_warning() {
        notes.push("10-bit/HDR source processed as 8-bit SDR".into());
    }
    for n in &notes {
        tracing::warn!("{}: {n}", info.path.display());
    }

    let total_src = match opts.segment {
        Some((_, d)) => (d * src_fps.as_f64()).round() as u64,
        None => info.frames.unwrap_or(0),
    };
    let (out_w, out_h) = (plan.scale.out_w, plan.scale.out_h);
    progress.update(|p| {
        p.frames_total = total_src * k as u64;
        p.device = plan.device_label.clone();
        p.encoder = encoder.label().into();
        p.detail = format!("{} · {src_fps} → {} fps", plan.describe(), fps.out_fps);
    });
    tracing::info!(
        "video {}: {} · {src_fps} → {} fps · encoder {}",
        info.path.display(),
        plan.describe(),
        fps.out_fps,
        encoder.ffmpeg_name()
    );

    let root = temp_root(s.video.temp_dir.as_deref());
    let work = TempDir::new(&root, &format!("vid{}", spec.id))?;
    let disk_mode = plan.needs_disk() || interp.is_some();
    let chunk_frames = s.video.chunk_frames.max(8);
    let mut chunk_need = 0u64;
    if disk_mode {
        let (mw, mh) = plan.model_dims();
        let need = estimate_temp_bytes(
            info.width,
            info.height,
            mw,
            mh,
            chunk_frames,
            k,
            plan.needs_disk(),
        );
        // One fully expanded chunk (without the next chunk's source frames).
        let src_only = estimate_temp_bytes(info.width, info.height, 0, 0, chunk_frames, 1, false) / 2;
        chunk_need = need.saturating_sub(src_only);
        if let Some(free) = free_space(work.path())
            && free < need + 512 * MB
        {
            return Err(PipelineError::Setup(crate::t!(
                "err.temp_space",
                need = crate::util::human_bytes(need),
                free = crate::util::human_bytes(free),
                dir = root.display()
            )));
        }
    }

    // ---- encoder ---------------------------------------------------------------------
    if let Some(dir) = spec.output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(&spec.output);
    let enc_spec = EncodeSpec {
        width: out_w,
        height: out_h,
        fps: fps.out_fps,
        encoder,
        quality: s.output.quality,
        speed: s.output.encoder_speed,
        bitrate_kbps: s.output.bitrate_kbps,
        container: s.output.container,
        output: part.clone(),
        mux: Some(MuxSource {
            path: info.path.clone(),
            segment: opts.segment,
            audio_codecs: info.audio_codecs.clone(),
            subtitle_codecs: info.subtitle_codecs.clone(),
            include_subtitles: opts.segment.is_none(),
        }),
    };
    let priority = plan.resources.priority;
    let enc = Encoder::spawn(&ffmpeg, &enc_spec, priority)?;

    // ---- stop propagation (user cancel or internal failure) --------------------------
    let stop = CancelToken::new();
    let finished = Arc::new(AtomicBool::new(false));
    {
        let (user, stop, finished) = (cancel.clone(), stop.clone(), finished.clone());
        std::thread::spawn(move || {
            while !finished.load(Ordering::Relaxed) {
                if user.is_cancelled() {
                    stop.cancel();
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }

    // ---- bounded queues sized from the RAM budget --------------------------------------
    let budget = s.video.ram_budget_mb.max(256) as u64 * MB;
    let out_bytes = (out_w as u64 * out_h as u64 * 3).max(1);
    let src_bytes = (info.width as u64 * info.height as u64 * 3).max(1);
    let writer_cap = (budget / 4 / out_bytes).clamp(2, 32) as usize;
    let batch =
        (budget / 4 / out_bytes).clamp(1, plan.resources.filter_threads.max(2) as u64) as usize;
    let raw_cap = (budget / 4 / src_bytes).clamp(2, 32) as usize;

    // ---- writer thread -----------------------------------------------------------------
    let (frame_tx, frame_rx) = bounded::<RgbImage>(writer_cap);
    let writer = {
        let stop = stop.clone();
        let progress = progress.clone();
        std::thread::Builder::new()
            .name("encode-writer".into())
            .spawn(move || {
                let mut enc = enc;
                for f in frame_rx {
                    if let Err(e) = enc.write_frame(&f) {
                        stop.cancel();
                        return (enc, Some(e));
                    }
                    progress.frames(enc.frames_written);
                }
                (enc, None)
            })?
    };

    // ---- decoder thread ----------------------------------------------------------------
    let dspec = DecodeSpec {
        input: info.path.clone(),
        segment: opts.segment,
        fps: Some(src_fps),
        width: info.width,
        height: info.height,
        scale: false,
    };
    let (chunk_tx, chunk_rx) = bounded::<ChunkMsg>(2);
    let (raw_tx, raw_rx) = bounded::<RgbImage>(raw_cap);
    let (tok_tx, tok_rx) = bounded::<()>(2);
    tok_tx.send(()).ok();
    tok_tx.send(()).ok();
    let decoder = {
        let stop = stop.clone();
        let plan = plan.clone();
        let progress = progress.clone();
        let work_dir = work.path().to_path_buf();
        let ffmpeg = ffmpeg.clone();
        let scene = (k > 1 && s.scene_detect).then_some(s.scene_threshold);
        std::thread::Builder::new().name("decode".into()).spawn(
            move || -> Result<u64, PipelineError> {
                let r = decode_loop(
                    &ffmpeg,
                    &dspec,
                    priority,
                    &plan,
                    &progress,
                    &stop,
                    disk_mode,
                    k,
                    chunk_frames as u64,
                    scene,
                    &work_dir,
                    &chunk_tx,
                    &raw_tx,
                    &tok_rx,
                    chunk_need,
                );
                if r.is_err() {
                    stop.cancel();
                }
                r
            },
        )?
    };

    // ---- processor (this thread) -------------------------------------------------------
    let proc_result: Result<(), PipelineError> = if disk_mode {
        process_chunks(
            &plan,
            interp.as_ref(),
            k,
            &chunk_rx,
            &tok_tx,
            &frame_tx,
            &stop,
            pause,
            progress,
            work.path(),
            batch,
            info.width.min(info.height) >= 1440,
        )
    } else {
        process_memory(&plan, &raw_rx, &frame_tx, &stop, pause, progress, batch)
    };
    if proc_result.is_err() {
        stop.cancel();
    }
    drop(chunk_rx);
    drop(raw_rx);
    let dec_result = decoder
        .join()
        .unwrap_or_else(|_| Err(PipelineError::Setup("decoder thread panicked".into())));
    drop(frame_tx);
    let (enc, write_err) = writer
        .join()
        .map_err(|_| PipelineError::Setup("writer thread panicked".into()))?;
    finished.store(true, Ordering::Relaxed);

    let fail = |enc: Encoder, err: PipelineError| -> PipelineError {
        let tail = enc.abort();
        if !tail.is_empty() {
            tracing::debug!("encoder stderr after abort: {tail}");
        }
        discard_partial(&part, &spec.output, s.output.keep_partial);
        err
    };

    if cancel.is_cancelled() {
        return Err(fail(enc, PipelineError::Cancelled));
    }
    if let Some(werr) = write_err {
        // The encoder died; its stderr says why.
        let e = match enc.finish() {
            Err(e) => PipelineError::Proc(e),
            Ok(()) => PipelineError::Io(werr),
        };
        discard_partial(&part, &spec.output, s.output.keep_partial);
        return Err(e);
    }
    let first_real = |r: Result<(), PipelineError>| r.err().filter(|e| !e.is_cancelled());
    if let Some(e) = first_real(proc_result) {
        return Err(fail(enc, e));
    }
    let decoded = match dec_result {
        Ok(n) => n,
        Err(e) => return Err(fail(enc, e)),
    };
    progress.stage(Stage::Finalizing);
    let frames = enc.frames_written;
    if let Err(e) = enc.finish() {
        discard_partial(&part, &spec.output, s.output.keep_partial);
        return Err(PipelineError::Proc(e));
    }
    if decoded == 0 {
        let _ = std::fs::remove_file(&part);
        return Err(PipelineError::Setup(crate::t!("err.zero_frames")));
    }
    if spec.output.exists() {
        std::fs::remove_file(&spec.output)?;
    }
    std::fs::rename(&part, &spec.output)?;
    progress.update(|p| {
        p.frames_total = frames;
        p.frames_done = frames;
    });
    tracing::info!(
        "wrote {} ({frames} frames, {} decoded)",
        spec.output.display(),
        decoded
    );
    Ok(VideoResult {
        output: spec.output.clone(),
        out_w,
        out_h,
        out_fps: fps.out_fps,
        frames,
        encoder,
        notes,
    })
}

fn discard_partial(part: &Path, output: &Path, keep: bool) {
    if !part.exists() {
        return;
    }
    if keep {
        let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
        let kept = output.with_extension(format!("partial.{ext}"));
        if std::fs::rename(part, &kept).is_ok() {
            tracing::info!("kept partial output {}", kept.display());
            return;
        }
    }
    let _ = std::fs::remove_file(part);
}

#[allow(clippy::too_many_arguments)]
fn decode_loop(
    ffmpeg: &Path,
    dspec: &DecodeSpec,
    priority: crate::config::Priority,
    plan: &StagePlan,
    progress: &ProgressHandle,
    stop: &CancelToken,
    disk_mode: bool,
    k: u32,
    chunk_frames: u64,
    scene_threshold: Option<f32>,
    work: &Path,
    chunk_tx: &Sender<ChunkMsg>,
    raw_tx: &Sender<RgbImage>,
    tok_rx: &Receiver<()>,
    chunk_need: u64,
) -> Result<u64, PipelineError> {
    let mut dec = Decoder::spawn(ffmpeg, dspec, priority)?;
    let mut detector = scene_threshold.map(SceneDetector::new);
    let mut g: u64 = 0;

    struct Building {
        index: usize,
        dir: PathBuf,
        start: u64,
        count: u64,
        cuts: Vec<u64>,
    }
    let mut cur: Option<Building> = None;
    let mut next_index = 0usize;
    let mut pending: Vec<(PathBuf, RgbImage)> = Vec::new();
    let flush_batch = plan.resources.filter_threads.max(2);
    let flush = |pending: &mut Vec<(PathBuf, RgbImage)>| -> Result<(), PipelineError> {
        if pending.is_empty() {
            return Ok(());
        }
        let items = std::mem::take(pending);
        plan.pool.install(|| {
            items
                .par_iter()
                .try_for_each(|(p, img)| write_png_fast(img, p))
        })
    };

    loop {
        if stop.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let frame = match dec.read_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                if stop.is_cancelled() {
                    return Err(PipelineError::Cancelled);
                }
                return Err(PipelineError::Io(e));
            }
        };
        let is_cut = detector.as_mut().is_some_and(|d| d.push(&frame));
        let frame = plan.pre_stage(frame, stop)?;
        progress.update(|p| p.extracted = g + 1);

        if !disk_mode {
            if !send_checked(raw_tx, frame, stop) {
                return Err(PipelineError::Cancelled);
            }
            g += 1;
            continue;
        }

        // Chunk full → this frame is the overlap of the current chunk and starts the next.
        if let Some(b) = cur.as_mut().filter(|b| b.count == chunk_frames) {
            let mut b_cuts = std::mem::take(&mut b.cuts);
            if k > 1 {
                pending.push((b.dir.join(frame_name(b.count)), frame.clone()));
                if is_cut {
                    b_cuts.push(b.count - 1);
                }
            }
            flush(&mut pending)?;
            let b = cur.take().expect("checked");
            let msg = ChunkMsg {
                index: b.index,
                dir: b.dir.parent().expect("chunk/src").to_path_buf(),
                len: b.count,
                overlap: k > 1,
                cuts: b_cuts,
            };
            if !send_checked(chunk_tx, msg, stop) {
                return Err(PipelineError::Cancelled);
            }
        } else if is_cut && let Some(b) = cur.as_mut() {
            b.cuts.push(g - b.start - 1);
        }
        if cur.is_none() {
            // Wait for a free disk slot (≤ 2 chunks on disk).
            if recv_checked(tok_rx, stop).is_none() {
                return Err(PipelineError::Cancelled);
            }
            wait_for_space(work, chunk_need, progress, stop)?;
            let dir = work.join(format!("chunk{next_index:05}")).join("src");
            std::fs::create_dir_all(&dir)?;
            cur = Some(Building {
                index: next_index,
                dir,
                start: g,
                count: 0,
                cuts: Vec::new(),
            });
            next_index += 1;
        }
        let b = cur.as_mut().expect("set above");
        pending.push((b.dir.join(frame_name(b.count)), frame));
        b.count += 1;
        if pending.len() >= flush_batch {
            flush(&mut pending)?;
        }
        g += 1;
    }
    flush(&mut pending)?;
    if let Some(b) = cur.take().filter(|b| b.count > 0) {
        let msg = ChunkMsg {
            index: b.index,
            dir: b.dir.parent().expect("chunk/src").to_path_buf(),
            len: b.count,
            overlap: false,
            cuts: b.cuts,
        };
        if !send_checked(chunk_tx, msg, stop) {
            return Err(PipelineError::Cancelled);
        }
    }
    let tail = dec.stderr_tail();
    dec.finish().map_err(|e| {
        tracing::warn!("decoder: {e}\n{tail}");
        PipelineError::Proc(e)
    })?;
    Ok(g)
}

/// Pause (don't fail) while the temp drive has less free space than the next chunk needs.
fn wait_for_space(work: &Path, need: u64, progress: &ProgressHandle, stop: &CancelToken) -> Result<(), PipelineError> {
    if need == 0 {
        return Ok(());
    }
    let mut paused = false;
    while let Some(free) = free_space(work)
        && free < need + 256 * MB
    {
        if stop.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        if !paused {
            tracing::warn!("temp space low ({free} B free, {need} B needed): pausing");
            paused = true;
        }
        let msg = crate::t!(
            "status.paused_disk",
            free = crate::util::human_bytes(free),
            need = crate::util::human_bytes(need)
        );
        progress.update(|p| p.detail = msg.clone());
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    if paused {
        tracing::info!("temp space available again: resuming");
        progress.update(|p| p.detail = String::new());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_chunks(
    plan: &StagePlan,
    interp: Option<&RifeNcnn>,
    k: u32,
    chunk_rx: &Receiver<ChunkMsg>,
    tok_tx: &Sender<()>,
    frame_tx: &Sender<RgbImage>,
    stop: &CancelToken,
    pause: &PauseFlag,
    progress: &ProgressHandle,
    work: &Path,
    batch: usize,
    uhd: bool,
) -> Result<(), PipelineError> {
    while let Some(msg) = recv_checked(chunk_rx, stop) {
        pause.wait_while_paused(stop);
        if stop.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let n_out = msg.len * k as u64;
        let mut frames_dir = msg.dir.join("src");
        progress.update(|p| p.detail = format!("chunk {} ({} frames)", msg.index + 1, msg.len));

        let inputs = msg.len + msg.overlap as u64;
        if interp.is_some() && inputs < 2 {
            // A 1-frame tail chunk (e.g. 243 frames with 121-frame chunks) has nothing to
            // interpolate towards, and rife-ncnn-vulkan crashes on a single input image.
            // Repeat the frame for every output slot instead.
            let out = msg.dir.join("interp");
            std::fs::create_dir_all(&out)?;
            let first = frames_dir.join(frame_name(0));
            for i in 0..n_out {
                std::fs::copy(&first, out.join(frame_name(i)))?;
            }
            let _ = std::fs::remove_dir_all(&frames_dir);
            frames_dir = out;
        } else if let Some(r) = interp {
            progress.stage(Stage::Interpolate);
            let req = inputs * k as u64;
            let out = msg.dir.join("interp");
            let cb = |n: usize| {
                progress.update(|p| {
                    p.detail = format!("chunk {}: interpolated {n}/{req}", msg.index + 1)
                })
            };
            let ctx = EngineCtx {
                cancel: stop,
                gpu: plan.gpu,
                jobs: plan.resources.jobs.clone(),
                tile: 0,
                priority: plan.resources.priority,
                progress: Some(&cb),
            };
            r.interpolate_dir(&frames_dir, &out, req as usize, uhd, &ctx)
                .map_err(|e| {
                    if e.is_cancelled() {
                        PipelineError::Cancelled
                    } else {
                        PipelineError::Engine(e)
                    }
                })?;
            progress.temp(dir_size(work));
            let _ = std::fs::remove_dir_all(&frames_dir);
            // Drop the overlap tail and the generated frames that cross a scene cut.
            for i in n_out..req {
                let _ = std::fs::remove_file(out.join(frame_name(i)));
            }
            for (slot, src) in fill_plan(msg.len, k, &msg.cuts).iter().enumerate() {
                if src.index() != slot as u64 {
                    let _ = std::fs::remove_file(out.join(frame_name(slot as u64)));
                }
            }
            if !msg.cuts.is_empty() {
                tracing::info!(
                    "chunk {}: {} scene cut(s), no interpolation across them",
                    msg.index + 1,
                    msg.cuts.len()
                );
            }
            frames_dir = out;
        }

        if plan.needs_disk() {
            progress.stage(Stage::Upscale);
            let up = msg.dir.join("up");
            let total = crate::engines::count_files(&frames_dir);
            let cb = |n: usize| {
                progress
                    .update(|p| p.detail = format!("chunk {}: upscaled {n}/{total}", msg.index + 1))
            };
            plan.upscale_dir(&frames_dir, &up, stop, Some(&cb))?;
            progress.temp(dir_size(work));
            let _ = std::fs::remove_dir_all(&frames_dir);
            frames_dir = up;
        }

        progress.stage(Stage::Sharpen);
        let slots = fill_plan(msg.len, k, &msg.cuts);
        for group in slots.chunks(batch.max(1)) {
            if stop.is_cancelled() {
                return Err(PipelineError::Cancelled);
            }
            let imgs: Vec<Result<RgbImage, PipelineError>> = plan.pool.install(|| {
                group
                    .par_iter()
                    .map(|src| {
                        let p = frames_dir.join(frame_name(src.index()));
                        let img = image::open(&p)
                            .map_err(|e| {
                                PipelineError::Setup(format!("reading {}: {e}", p.display()))
                            })?
                            .into_rgb8();
                        // In-memory AI (ONNX) after frame generation; sessions serialize.
                        let img = plan.ai_mem(img, stop)?;
                        Ok(plan.post_process(img))
                    })
                    .collect()
            });
            progress.stage(Stage::Encode);
            for img in imgs {
                if !send_checked(frame_tx, img?, stop) {
                    return Err(PipelineError::Cancelled);
                }
            }
        }
        progress.temp(dir_size(work));
        let _ = std::fs::remove_dir_all(&msg.dir);
        progress.temp(dir_size(work));
        // Return the disk slot.
        let _ = tok_tx.send(());
    }
    if stop.is_cancelled() {
        return Err(PipelineError::Cancelled);
    }
    Ok(())
}

fn process_memory(
    plan: &StagePlan,
    raw_rx: &Receiver<RgbImage>,
    frame_tx: &Sender<RgbImage>,
    stop: &CancelToken,
    pause: &PauseFlag,
    progress: &ProgressHandle,
    batch: usize,
) -> Result<(), PipelineError> {
    progress.stage(Stage::Sharpen);
    let mut group = Vec::with_capacity(batch);
    let mut done = false;
    while !done {
        group.clear();
        while group.len() < batch.max(1) {
            match recv_checked(raw_rx, stop) {
                Some(f) => group.push(f),
                None => {
                    done = true;
                    break;
                }
            }
        }
        pause.wait_while_paused(stop);
        if stop.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        // In-memory AI (ONNX) runs frame by frame on the GPU; the CPU filters in parallel.
        if plan.ai_in_memory() {
            progress.stage(Stage::Upscale);
            let mut up = Vec::with_capacity(group.len());
            for f in group.drain(..) {
                up.push(plan.ai_mem(f, stop)?);
            }
            group = up;
            progress.stage(Stage::Sharpen);
        }
        let out: Vec<RgbImage> = plan
            .pool
            .install(|| group.par_drain(..).map(|f| plan.post_process(f)).collect());
        progress.stage(Stage::Encode);
        for f in out {
            if !send_checked(frame_tx, f, stop) {
                return Err(PipelineError::Cancelled);
            }
        }
    }
    Ok(())
}
