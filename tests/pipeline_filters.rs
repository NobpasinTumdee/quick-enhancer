//! Integration tests that drive the real pipeline with the real ffmpeg.
//! Skipped (with a message) when `bin/ffmpeg` is missing; the AI test additionally needs
//! the ncnn tools, models and a Vulkan GPU.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quick_enhancer::config::{
    ComputeDevice, EncoderChoice, FrameGenMode, ImageFormat, JobSettings, Profile, ScaleMode,
    SharpenMode,
};
use quick_enhancer::devices;
use quick_enhancer::media::encoders::{self, VideoEncoder};
use quick_enhancer::media::probe;
use quick_enhancer::paths::{ModelSet, ToolSet};
use quick_enhancer::pipeline::cancel::{CancelToken, PauseFlag};
use quick_enhancer::pipeline::image_job::run_image;
use quick_enhancer::pipeline::job::{Env, JobOutcome, JobSpec, ProgressHandle};
use quick_enhancer::pipeline::video_job::{VideoOptions, run_video};
use quick_enhancer::util::{new_command, run_capture};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn tools() -> Option<ToolSet> {
    let t = ToolSet::from_bin_dir(&root().join("bin"));
    if t.ffmpeg.is_none() || t.ffprobe.is_none() {
        eprintln!("SKIPPED: bin/ffmpeg or bin/ffprobe missing");
        return None;
    }
    Some(t)
}

fn make_env(tools: ToolSet) -> Env {
    let enc = encoders::detect(tools.ffmpeg.as_deref());
    Env {
        models: ModelSet::discover(&root().join("models")),
        gpus: devices::enumerate().unwrap_or_default(),
        encoders: Arc::new(RwLock::new(enc)),
        monitor: None,
        cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        tools,
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = root().join("target").join("it-scratch").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg(tools: &ToolSet, args: &[&str]) {
    let mut c = new_command(tools.ffmpeg.as_ref().unwrap());
    c.args(["-hide_banner", "-v", "error", "-y"]).args(args);
    run_capture(c, None, quick_enhancer::config::Priority::Normal).expect("ffmpeg ran");
}

fn base_settings(temp: &Path) -> JobSettings {
    let mut s = JobSettings::default();
    s.output.encoder = EncoderChoice::Specific(VideoEncoder::Libx264);
    s.video.temp_dir = Some(temp.to_path_buf());
    s
}

fn temp_is_clean(temp: &Path) -> bool {
    let root = quick_enhancer::pipeline::temp::temp_root(Some(temp));
    std::fs::read_dir(&root)
        .map(|rd| rd.count() == 0)
        .unwrap_or(true)
}

#[test]
fn video_filters_only_end_to_end() {
    let Some(t) = tools() else { return };
    let dir = scratch("filters");
    let src = dir.join("src.mp4");
    ffmpeg(
        &t,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=30:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
            src.to_str().unwrap(),
        ],
    );
    let env = make_env(t);
    let info = probe::probe(env.tools.ffprobe.as_deref(), &src).unwrap();
    let mut s = base_settings(&dir);
    s.profile = Profile::NoAi;
    s.scale = ScaleMode::X2;
    s.denoise = 0.3;
    s.sharpen = SharpenMode::Cas;
    s.cas_strength = 0.5;
    s.compute = ComputeDevice::Cpu;
    let out = dir.join("out.mp4");
    let spec = JobSpec {
        id: 1,
        info,
        settings: s,
        output: out.clone(),
    };
    let progress = ProgressHandle::default();
    let r = run_video(
        &env,
        &spec,
        &progress,
        &CancelToken::new(),
        &PauseFlag::default(),
        &VideoOptions::default(),
    )
    .expect("pipeline ran");
    assert_eq!((r.out_w, r.out_h), (640, 480));
    let o = probe::probe(env.tools.ffprobe.as_deref(), &out).unwrap();
    assert_eq!((o.width, o.height), (640, 480));
    assert!(
        (o.fps.unwrap().as_f64() - 30.0).abs() < 0.01,
        "fps {:?}",
        o.fps
    );
    let d = o.duration.unwrap();
    assert!((d - 3.0).abs() < 0.15, "duration {d}");
    assert_eq!(o.audio_codecs, vec!["aac"]);
    assert_eq!(r.frames, 90);
    assert!(temp_is_clean(&dir), "temp dir not cleaned");
}

#[test]
fn video_cancel_cleans_up() {
    let Some(t) = tools() else { return };
    let dir = scratch("cancel");
    let src = dir.join("src.mp4");
    ffmpeg(
        &t,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x360:rate=30:duration=20",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            src.to_str().unwrap(),
        ],
    );
    let env = make_env(t);
    let info = probe::probe(env.tools.ffprobe.as_deref(), &src).unwrap();
    let mut s = base_settings(&dir);
    s.profile = Profile::NoAi;
    s.scale = ScaleMode::X4;
    s.denoise = 1.0;
    let out = dir.join("out.mp4");
    let spec = JobSpec {
        id: 2,
        info,
        settings: s,
        output: out.clone(),
    };
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        c2.cancel();
    });
    let r = run_video(
        &env,
        &spec,
        &ProgressHandle::default(),
        &cancel,
        &PauseFlag::default(),
        &VideoOptions::default(),
    );
    assert!(
        matches!(r, Err(ref e) if e.is_cancelled()),
        "expected cancel"
    );
    assert!(!out.exists());
    assert!(
        !quick_enhancer::pipeline::video_job::part_path(&out).exists(),
        "partial output left behind"
    );
    assert!(temp_is_clean(&dir));
}

#[test]
fn image_with_alpha_no_ai() {
    let dir = scratch("image");
    let src = dir.join("a.png");
    let img = image::RgbaImage::from_fn(50, 30, |x, y| {
        image::Rgba([
            (x * 5) as u8,
            (y * 8) as u8,
            99,
            if x < 25 { 255 } else { 0 },
        ])
    });
    img.save(&src).unwrap();
    let env = make_env(ToolSet::default());
    let info = probe::probe(None, &src).unwrap();
    assert!(info.has_alpha);
    let mut s = base_settings(&dir);
    s.profile = Profile::PixelArt;
    s.scale = ScaleMode::X3;
    s.output.image_format = ImageFormat::Png;
    let out = dir.join("a_enhanced.png");
    let spec = JobSpec {
        id: 3,
        info,
        settings: s,
        output: out.clone(),
    };
    let r = run_image(&env, &spec, &ProgressHandle::default(), &CancelToken::new()).unwrap();
    assert!(matches!(r, JobOutcome::Done { .. }));
    let o = image::open(&out).unwrap().to_rgba8();
    assert_eq!(o.dimensions(), (150, 90));
    // Nearest-neighbour: exact pixel replication, alpha kept.
    assert_eq!(o.get_pixel(4, 4), img.get_pixel(1, 1));
    assert_eq!(o.get_pixel(140, 10)[3], 0);
    assert!(temp_is_clean(&dir));
}

#[test]
fn video_ai_upscale_and_framegen_with_scene_cut() {
    let Some(t) = tools() else { return };
    let env = make_env(t);
    if env.tools.realesrgan.is_none()
        || env.tools.rife.is_none()
        || env.models.rife.is_empty()
        || env.models.animevideo_scales().is_empty()
    {
        eprintln!("SKIPPED: ncnn tools/models missing");
        return;
    }
    if devices::auto_select(&env.gpus).is_none() {
        eprintln!("SKIPPED: no Vulkan GPU");
        return;
    }
    let dir = scratch("ai");
    let src = dir.join("cut.mp4");
    // 1 s of testsrc then 1 s of smptebars → one hard cut at frame 12.
    ffmpeg(
        &t_ffmpeg(&env),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x96:rate=12:duration=1",
            "-f",
            "lavfi",
            "-i",
            "smptebars=size=160x96:rate=12:duration=1",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1[v]",
            "-map",
            "[v]",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            src.to_str().unwrap(),
        ],
    );
    let info = probe::probe(env.tools.ffprobe.as_deref(), &src).unwrap();
    let mut s = base_settings(&dir);
    s.profile = Profile::Anime;
    s.scale = ScaleMode::X2;
    s.framegen = FrameGenMode::X2;
    s.video.chunk_frames = 8;
    s.sharpen = SharpenMode::Cas;
    let out = dir.join("out.mkv");
    s.output.container = quick_enhancer::config::Container::Mkv;
    let spec = JobSpec {
        id: 4,
        info,
        settings: s,
        output: out.clone(),
    };
    let r = run_video(
        &env,
        &spec,
        &ProgressHandle::default(),
        &CancelToken::new(),
        &PauseFlag::default(),
        &VideoOptions::default(),
    )
    .expect("AI pipeline ran");
    assert_eq!((r.out_w, r.out_h), (320, 192));
    assert_eq!(r.frames, 48);
    let o = probe::probe(env.tools.ffprobe.as_deref(), &out).unwrap();
    assert!((o.fps.unwrap().as_f64() - 24.0).abs() < 0.01);
    assert!(temp_is_clean(&dir));
}

fn t_ffmpeg(env: &Env) -> ToolSet {
    env.tools.clone()
}

/// Regression: a 1-frame tail chunk (17 frames, chunks of 8 → 8+8+1) used to crash
/// rife-ncnn-vulkan, which cannot interpolate from a single input image.
#[test]
fn framegen_single_frame_tail_chunk() {
    let Some(t) = tools() else { return };
    let env = make_env(t);
    if env.tools.rife.is_none() || env.models.rife.is_empty() {
        eprintln!("SKIPPED: rife missing");
        return;
    }
    if devices::auto_select(&env.gpus).is_none() {
        eprintln!("SKIPPED: no Vulkan GPU");
        return;
    }
    let dir = scratch("tail");
    let src = dir.join("tail.mp4");
    ffmpeg(
        &env.tools,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=96x160:rate=17:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            src.to_str().unwrap(),
        ],
    );
    let info = probe::probe(env.tools.ffprobe.as_deref(), &src).unwrap();
    let mut s = base_settings(&dir);
    s.profile = Profile::NoAi;
    s.scale = ScaleMode::X1;
    s.framegen = FrameGenMode::X2;
    s.video.chunk_frames = 8;
    let out = dir.join("out.mp4");
    let spec = JobSpec {
        id: 5,
        info,
        settings: s,
        output: out,
    };
    let r = run_video(
        &env,
        &spec,
        &ProgressHandle::default(),
        &CancelToken::new(),
        &PauseFlag::default(),
        &VideoOptions::default(),
    )
    .expect("tail chunk handled");
    assert_eq!(r.frames, 34);
    assert!(temp_is_clean(&dir));
}

/// Sample-preview audio: segment audio decodes to 48 kHz stereo PCM of the right length,
/// and a clip without audio reports `None` (the player then shows "No audio").
#[test]
fn sample_audio_decode() {
    let Some(tools) = tools() else { return };
    let dir = scratch("audio");
    let with = dir.join("with_audio.mp4");
    let without = dir.join("no_audio.mp4");
    ffmpeg(
        &tools,
        &[
            "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=24:duration=3",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
            "-c:v", "libx264", "-c:a", "aac", "-shortest", with.to_str().unwrap(),
        ],
    );
    ffmpeg(
        &tools,
        &[
            "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=24:duration=2",
            "-c:v", "libx264", without.to_str().unwrap(),
        ],
    );
    let ff = tools.ffmpeg.as_ref().unwrap();
    let cancel = CancelToken::new();
    let clip = quick_enhancer::media::ffmpeg::decode_audio(ff, &with, Some((0.5, 1.5)), &cancel)
        .unwrap()
        .expect("has audio");
    assert_eq!((clip.sample_rate, clip.channels), (48_000, 2));
    assert!((clip.duration() - 1.5).abs() < 0.05, "duration {}", clip.duration());
    let peak = clip.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "sine should be audible, peak {peak}");
    let none = quick_enhancer::media::ffmpeg::decode_audio(ff, &without, None, &cancel).unwrap();
    assert!(none.is_none());
}
