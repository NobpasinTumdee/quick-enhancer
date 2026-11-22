//! Editor jobs against the real ffmpeg: trim (fast / accurate), crop, join (lossless and
//! re-encoded with a silent clip), image crop/rotate. Skipped when `bin/ffmpeg` is missing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quick_enhancer::config::{EncoderSpeed, ImageFormat};
use quick_enhancer::editor::{Clip, CropRect, EditJob, EncodeChoice, ImageOps, TrimMode, run_edit};
use quick_enhancer::media::encoders::{self, VideoEncoder};
use quick_enhancer::media::probe;
use quick_enhancer::paths::{ModelSet, ToolSet};
use quick_enhancer::pipeline::cancel::CancelToken;
use quick_enhancer::pipeline::job::{Env, JobOutcome, ProgressHandle};
use quick_enhancer::util::{new_command, run_capture};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn env() -> Option<Env> {
    let tools = ToolSet::from_bin_dir(&root().join("bin"));
    if tools.ffmpeg.is_none() || tools.ffprobe.is_none() {
        eprintln!("SKIPPED: bin/ffmpeg or bin/ffprobe missing");
        return None;
    }
    Some(Env {
        encoders: Arc::new(RwLock::new(encoders::detect(tools.ffmpeg.as_deref()))),
        models: ModelSet::discover(&root().join("models")),
        gpus: Vec::new(),
        monitor: None,
        cores: 4,
        tools,
    })
}

fn make(env: &Env, out: &Path, size: &str, rate: u32, secs: u32, audio: bool) {
    let mut cmd = new_command(env.tools.ffmpeg.as_ref().unwrap());
    cmd.args(["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i"]);
    cmd.arg(format!("testsrc=size={size}:rate={rate}:duration={secs}"));
    if audio {
        cmd.args(["-f", "lavfi", "-i"]).arg(format!("sine=frequency=440:sample_rate=48000:duration={secs}"));
        cmd.args(["-ac", "2", "-c:a", "aac"]);
    }
    // Keyframe every second so fast trims have something to snap to.
    cmd.args(["-c:v", "libx264", "-g", &rate.to_string(), "-pix_fmt", "yuv420p", "-shortest"]);
    cmd.arg(out);
    run_capture(cmd, None, Default::default()).expect("make test video");
}

fn info(env: &Env, p: &Path) -> quick_enhancer::media::probe::MediaInfo {
    probe::probe(env.tools.ffprobe.as_deref(), p).expect("probe")
}

fn enc() -> EncodeChoice {
    EncodeChoice { encoder: VideoEncoder::Libx264, quality: Some(23), speed: EncoderSpeed::Fast }
}

fn run(env: &Env, job: EditJob, out: &Path) -> quick_enhancer::media::probe::MediaInfo {
    let r = run_edit(env, &job, out, &ProgressHandle::default(), &CancelToken::new()).expect("edit");
    assert!(matches!(r, JobOutcome::Done { .. }), "{r:?}");
    info(env, out)
}

#[test]
fn trim_crop_and_join() {
    let Some(env) = env() else { return };
    let dir = std::env::temp_dir().join("qe-editor-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.mp4");
    let b = dir.join("b.mp4");
    let c = dir.join("c.mp4");
    make(&env, &a, "320x240", 30, 6, true);
    make(&env, &b, "320x240", 30, 4, true);
    make(&env, &c, "640x360", 25, 3, false);

    // Accurate trim 1.5–4.0 s + crop.
    let mut clip = Clip::new(info(&env, &a));
    clip.trim = Some((1.5, 4.0));
    let out = dir.join("acc.mp4");
    let crop = Some(CropRect { x: 10, y: 10, w: 200, h: 120 });
    let i = run(&env, EditJob::Video { clip: Box::new(clip.clone()), crop, mode: TrimMode::Accurate, enc: enc() }, &out);
    assert_eq!((i.width, i.height), (200, 120));
    let d = i.duration.unwrap();
    assert!((d - 2.5).abs() < 0.15, "accurate duration {d}");
    assert!(!i.audio_codecs.is_empty());

    // Fast trim: stream copy, same size, roughly the selection (keyframe snapped).
    let out = dir.join("fast.mp4");
    let i = run(&env, EditJob::Video { clip: Box::new(clip), crop: None, mode: TrimMode::Fast, enc: enc() }, &out);
    assert_eq!((i.width, i.height), (320, 240));
    let d = i.duration.unwrap();
    assert!(d > 2.0 && d < 4.2, "fast duration {d}");

    // Lossless join of matching clips.
    let clips = vec![Clip::new(info(&env, &a)), Clip::new(info(&env, &b))];
    assert!(quick_enhancer::editor::concat_compatible(&clips));
    let out = dir.join("join_copy.mp4");
    let i = run(&env, EditJob::Join { clips, enc: enc() }, &out);
    let d = i.duration.unwrap();
    assert!((d - 10.0).abs() < 0.3, "copy join {d}");

    // Re-encoded join: different size/fps, one clip without audio, one trimmed.
    let mut first = Clip::new(info(&env, &a));
    first.trim = Some((0.0, 2.0));
    let clips = vec![first, Clip::new(info(&env, &c)), Clip::new(info(&env, &b))];
    assert!(!quick_enhancer::editor::concat_compatible(&clips));
    let out = dir.join("join_enc.mp4");
    let i = run(&env, EditJob::Join { clips, enc: enc() }, &out);
    assert_eq!((i.width, i.height), (320, 240));
    assert_eq!(i.fps.map(|f| f.as_f64().round() as u32), Some(30));
    assert!(!i.audio_codecs.is_empty());
    let d = i.duration.unwrap();
    assert!((d - 9.0).abs() < 0.3, "re-encoded join {d}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn image_rotate_crop_keeps_format() {
    let Some(env) = env() else { return };
    let dir = std::env::temp_dir().join("qe-editor-img-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.jpg");
    image::RgbImage::from_fn(400, 200, |x, y| image::Rgb([(x / 2) as u8, y as u8, 80])).save(&src).unwrap();
    let out = dir.join("out.jpg");
    let ops = ImageOps { crop: Some(CropRect { x: 0, y: 0, w: 100, h: 300 }), rotate: 1, format: ImageFormat::Same, ..Default::default() };
    let r = run_edit(&env, &EditJob::Image { input: src, ops }, &out, &ProgressHandle::default(), &CancelToken::new()).unwrap();
    assert!(matches!(r, JobOutcome::Done { .. }));
    let img = image::open(&out).unwrap();
    assert_eq!((img.width(), img.height()), (100, 300));
    assert_eq!(image::ImageFormat::from_path(&out).unwrap(), image::ImageFormat::Jpeg);
    let _ = std::fs::remove_dir_all(&dir);
}
