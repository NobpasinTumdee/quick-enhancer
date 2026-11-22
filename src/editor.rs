//! Editor page logic: trim / crop / join for videos, crop / rotate / flip for images.
//!
//! Everything here builds ffmpeg argument lists (pure, unit-tested) or runs one edit job on
//! the shared runner thread ([`run_edit`]), so edits are queued, cancellable and reported
//! like any other job.

use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageDecoder};

use crate::config::{EncoderSpeed, ImageFormat, Priority};
use crate::media::Rational;
use crate::media::encoders::VideoEncoder;
use crate::media::probe::MediaInfo;
use crate::pipeline::cancel::CancelToken;
use crate::pipeline::job::{Env, JobOutcome, PipelineError, ProgressHandle, Stage};
use crate::t;
use crate::util::{new_command, run_capture_tick};

// ---- time ------------------------------------------------------------------------------

/// `hh:mm:ss.mmm`, `mm:ss.mmm`, `ss.mmm` (also `,` as decimal separator) → seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, p) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        let v: f64 = if last {
            p.parse().ok()?
        } else {
            p.parse::<u32>().ok()? as f64
        };
        if v < 0.0 || !v.is_finite() || (i > 0 && v >= 60.0) {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// Seconds → `hh:mm:ss.mmm`.
pub fn format_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

// ---- crop ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl CropRect {
    pub fn full(w: u32, h: u32) -> CropRect {
        CropRect { x: 0, y: 0, w, h }
    }

    pub fn is_full(&self, w: u32, h: u32) -> bool {
        self.x == 0 && self.y == 0 && self.w >= w && self.h >= h
    }

    /// Clamped inside `max_w`x`max_h` with even position and size (yuv420 needs even
    /// dimensions), at least 16x16.
    pub fn normalized(self, max_w: u32, max_h: u32) -> CropRect {
        let even = |v: u32| v & !1;
        let max_w = even(max_w).max(2);
        let max_h = even(max_h).max(2);
        let w = even(self.w.clamp(16.min(max_w), max_w));
        let h = even(self.h.clamp(16.min(max_h), max_h));
        let x = even(self.x.min(max_w - w));
        let y = even(self.y.min(max_h - h));
        CropRect { x, y, w, h }
    }

    /// ffmpeg `crop` filter.
    pub fn filter(&self) -> String {
        format!("crop={}:{}:{}:{}", self.w, self.h, self.x, self.y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Aspect {
    #[default]
    Free,
    Original,
    R16x9,
    R9x16,
    R1x1,
    R4x3,
    R21x9,
}

impl Aspect {
    pub const ALL: [Aspect; 7] = [
        Aspect::Free,
        Aspect::Original,
        Aspect::R16x9,
        Aspect::R9x16,
        Aspect::R1x1,
        Aspect::R4x3,
        Aspect::R21x9,
    ];

    pub fn label(self) -> String {
        match self {
            Aspect::Free => t!("editor.aspect_free"),
            Aspect::Original => t!("editor.aspect_original"),
            Aspect::R16x9 => "16:9".into(),
            Aspect::R9x16 => "9:16".into(),
            Aspect::R1x1 => "1:1".into(),
            Aspect::R4x3 => "4:3".into(),
            Aspect::R21x9 => "21:9".into(),
        }
    }

    /// Width / height, `None` = free.
    pub fn ratio(self, src_w: u32, src_h: u32) -> Option<f64> {
        Some(match self {
            Aspect::Free => return None,
            Aspect::Original => src_w as f64 / src_h.max(1) as f64,
            Aspect::R16x9 => 16.0 / 9.0,
            Aspect::R9x16 => 9.0 / 16.0,
            Aspect::R1x1 => 1.0,
            Aspect::R4x3 => 4.0 / 3.0,
            Aspect::R21x9 => 21.0 / 9.0,
        })
    }
}

/// Shrink `r` around its centre to `ratio` (w/h), then normalise.
pub fn fit_aspect(r: CropRect, ratio: f64, max_w: u32, max_h: u32) -> CropRect {
    let (cx, cy) = (r.x as f64 + r.w as f64 / 2.0, r.y as f64 + r.h as f64 / 2.0);
    let (mut w, mut h) = (r.w as f64, r.h as f64);
    if w / h.max(1.0) > ratio {
        w = h * ratio;
    } else {
        h = w / ratio;
    }
    let x = (cx - w / 2.0).max(0.0);
    let y = (cy - h / 2.0).max(0.0);
    CropRect { x: x as u32, y: y as u32, w: w.round() as u32, h: h.round() as u32 }.normalized(max_w, max_h)
}

// ---- video jobs --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrimMode {
    /// Cut at keyframes, no re-encode (instant, lossless, start may be a bit early).
    Fast,
    /// Re-encode: frame-exact.
    #[default]
    Accurate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub info: MediaInfo,
    /// (in, out) seconds; `None` = whole clip.
    pub trim: Option<(f64, f64)>,
}

impl Clip {
    pub fn new(info: MediaInfo) -> Clip {
        Clip { info, trim: None }
    }
    pub fn duration(&self) -> f64 {
        self.info.duration.unwrap_or(0.0)
    }
    /// Effective (start, end).
    pub fn span(&self) -> (f64, f64) {
        let d = self.duration();
        match self.trim {
            Some((a, b)) => (a.clamp(0.0, d), b.clamp(a.max(0.0), d.max(a))),
            None => (0.0, d),
        }
    }
    pub fn len(&self) -> f64 {
        let (a, b) = self.span();
        (b - a).max(0.0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() <= 0.0
    }
    fn trimmed(&self) -> bool {
        let (a, b) = self.span();
        a > 0.0005 || b < self.duration() - 0.0005
    }
    fn has_audio(&self) -> bool {
        !self.info.audio_codecs.is_empty()
    }
}

/// Encoder choice for re-encoding edits.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodeChoice {
    pub encoder: VideoEncoder,
    pub quality: Option<u32>,
    pub speed: EncoderSpeed,
}

impl EncodeChoice {
    fn args(&self) -> Vec<String> {
        self.encoder.args_with(self.quality, self.speed, None)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageOps {
    pub crop: Option<CropRect>,
    /// Clockwise quarter turns (0..=3).
    pub rotate: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    pub format: ImageFormat,
    pub quality: u8,
}

impl Default for ImageOps {
    fn default() -> Self {
        ImageOps { crop: None, rotate: 0, flip_h: false, flip_v: false, format: ImageFormat::Same, quality: 92 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditJob {
    Video { clip: Box<Clip>, crop: Option<CropRect>, mode: TrimMode, enc: EncodeChoice },
    Join { clips: Vec<Clip>, enc: EncodeChoice },
    Image { input: PathBuf, ops: ImageOps },
}

impl EditJob {
    /// Output duration (seconds) for progress.
    pub fn duration(&self) -> f64 {
        match self {
            EditJob::Video { clip, .. } => clip.len(),
            EditJob::Join { clips, .. } => clips.iter().map(Clip::len).sum(),
            EditJob::Image { .. } => 0.0,
        }
    }
    pub fn source(&self) -> Option<&Path> {
        match self {
            EditJob::Video { clip, .. } => Some(&clip.info.path),
            EditJob::Join { clips, .. } => clips.first().map(|c| c.info.path.as_path()),
            EditJob::Image { input, .. } => Some(input),
        }
    }
    /// Short description for history / toasts.
    pub fn describe(&self) -> String {
        match self {
            EditJob::Video { clip, crop, mode, .. } => {
                let mut v = Vec::new();
                if clip.trimmed() {
                    let (a, b) = clip.span();
                    v.push(t!(
                        "editor.desc_trim",
                        from = format_time(a),
                        to = format_time(b),
                        mode = if *mode == TrimMode::Fast && crop.is_none() { t!("editor.fast") } else { t!("editor.accurate") }
                    ));
                }
                if let Some(c) = crop {
                    v.push(t!("editor.desc_crop", w = c.w, h = c.h));
                }
                v.join(" · ")
            }
            EditJob::Join { clips, .. } => t!("editor.desc_join", n = clips.len()),
            EditJob::Image { ops, .. } => {
                let mut v = Vec::new();
                if let Some(c) = ops.crop {
                    v.push(t!("editor.desc_crop", w = c.w, h = c.h));
                }
                if ops.rotate % 4 != 0 {
                    v.push(t!("editor.desc_rotate", deg = ops.rotate as u32 % 4 * 90));
                }
                if ops.flip_h {
                    v.push(t!("editor.flip_h"));
                }
                if ops.flip_v {
                    v.push(t!("editor.flip_v"));
                }
                v.join(" · ")
            }
        }
    }
}

/// Container extension for an edit of `input`: stream copy keeps the source container,
/// re-encodes go to MP4 unless the source is MKV.
pub fn output_ext(input: &Path, reencode: bool) -> String {
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    if !reencode {
        return ext;
    }
    if ext == "mkv" { "mkv".into() } else { "mp4".into() }
}

fn common_args(progress: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-v", "error", "-stats_period", "0.25"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.push("-progress".into());
    a.push(progress.display().to_string());
    a
}

fn ts(v: f64) -> String {
    format!("{v:.3}")
}

/// Trim and/or crop one video.
pub fn video_args(clip: &Clip, crop: Option<CropRect>, mode: TrimMode, enc: &EncodeChoice, out: &Path, progress: &Path) -> Vec<String> {
    let mut a = common_args(progress);
    let (start, end) = clip.span();
    let copy = mode == TrimMode::Fast && crop.is_none();
    if start > 0.0 {
        a.extend(["-ss".into(), ts(start)]);
    }
    a.push("-i".into());
    a.push(clip.info.path.display().to_string());
    if clip.trimmed() {
        a.extend(["-t".into(), ts(end - start)]);
    }
    a.extend(["-map".into(), "0:v:0".into(), "-map".into(), "0:a?".into()]);
    if copy {
        a.extend(["-c".into(), "copy".into(), "-avoid_negative_ts".into(), "make_zero".into()]);
    } else {
        if let Some(c) = crop {
            a.extend(["-vf".into(), c.filter()]);
        }
        a.extend(enc.args());
        a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }
    if out.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(out.display().to_string());
    a
}

/// Clips can be joined losslessly (concat demuxer + stream copy).
pub fn concat_compatible(clips: &[Clip]) -> bool {
    let Some(first) = clips.first() else {
        return false;
    };
    let key = |c: &Clip| {
        (
            c.info.video_codec.clone(),
            c.info.width,
            c.info.height,
            c.info.fps,
            c.info.pix_fmt.clone(),
            c.info.audio_codecs.first().cloned(),
            c.info.audio_layout.clone(),
            c.info.path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()),
        )
    };
    let k = key(first);
    clips.iter().all(|c| key(c) == k)
}

fn concat_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

/// `ffconcat` list with per-clip in/out points.
pub fn concat_list(clips: &[Clip]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    for c in clips {
        s += &format!("file {}\n", concat_quote(&c.info.path));
        if c.trimmed() {
            let (a, b) = c.span();
            if a > 0.0 {
                s += &format!("inpoint {}\n", ts(a));
            }
            s += &format!("outpoint {}\n", ts(b));
        }
    }
    s
}

/// Lossless join through the concat demuxer.
pub fn concat_copy_args(list: &Path, out: &Path, progress: &Path) -> Vec<String> {
    let mut a = common_args(progress);
    a.extend(["-f", "concat", "-safe", "0", "-i"].iter().map(|s| s.to_string()));
    a.push(list.display().to_string());
    a.extend(["-map", "0:v:0", "-map", "0:a?", "-c", "copy"].iter().map(|s| s.to_string()));
    if out.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(out.display().to_string());
    a
}

/// `filter_complex` for a re-encoded join: every clip scaled + letterboxed to `w`x`h` at
/// `fps`, audio resampled to 48 kHz stereo, silence for clips without audio.
/// `silent_inputs[i]` = input index of the `anullsrc` for clip `i` (when it has no audio).
pub fn concat_filter(clips: &[Clip], w: u32, h: u32, fps: Rational, silent_inputs: &[Option<usize>]) -> (String, bool) {
    let audio = clips.iter().any(Clip::has_audio);
    let mut f = String::new();
    let mut tail = String::new();
    for (i, c) in clips.iter().enumerate() {
        f += &format!(
            "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={}/{},format=yuv420p[v{i}];",
            fps.num, fps.den
        );
        tail += &format!("[v{i}]");
        if audio {
            let src = match silent_inputs.get(i).copied().flatten() {
                Some(k) if !c.has_audio() => format!("[{k}:a]"),
                _ => format!("[{i}:a:0]"),
            };
            f += &format!("{src}aresample=48000,aformat=channel_layouts=stereo[a{i}];");
            tail += &format!("[a{i}]");
        }
    }
    f += &format!("{tail}concat=n={}:v=1:a={}[v]{}", clips.len(), u8::from(audio), if audio { "[a]" } else { "" });
    (f, audio)
}

/// Re-encoded join to the first clip's size and frame rate.
pub fn concat_reencode_args(clips: &[Clip], enc: &EncodeChoice, out: &Path, progress: &Path) -> Vec<String> {
    let mut a = common_args(progress);
    let first = &clips[0].info;
    let (w, h) = (first.width & !1, first.height & !1);
    let fps = first.fps.unwrap_or(Rational::new(30, 1));
    for c in clips {
        let (s, e) = c.span();
        if s > 0.0 {
            a.extend(["-ss".into(), ts(s)]);
        }
        if c.trimmed() {
            a.extend(["-t".into(), ts(e - s)]);
        }
        a.push("-i".into());
        a.push(c.info.path.display().to_string());
    }
    let any_audio = clips.iter().any(Clip::has_audio);
    let mut silent = vec![None; clips.len()];
    let mut next = clips.len();
    if any_audio {
        for (i, c) in clips.iter().enumerate() {
            if !c.has_audio() {
                a.extend(["-f", "lavfi", "-t"].iter().map(|s| s.to_string()));
                a.push(ts(c.len().max(0.04)));
                a.push("-i".into());
                a.push("anullsrc=channel_layout=stereo:sample_rate=48000".into());
                silent[i] = Some(next);
                next += 1;
            }
        }
    }
    let (filter, audio) = concat_filter(clips, w, h, fps, &silent);
    a.push("-filter_complex".into());
    a.push(filter);
    a.extend(["-map".into(), "[v]".into()]);
    if audio {
        a.extend(["-map".into(), "[a]".into()]);
    }
    a.extend(enc.args());
    if audio {
        a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }
    if out.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.push(out.display().to_string());
    a
}

// ---- images ------------------------------------------------------------------------------

/// Apply rotate → flip → crop (the crop rectangle is in the rotated/flipped image, which
/// is what the Editor shows).
pub fn apply_image_ops(img: DynamicImage, ops: &ImageOps) -> DynamicImage {
    let mut img = match ops.rotate % 4 {
        1 => img.rotate90(),
        2 => img.rotate180(),
        3 => img.rotate270(),
        _ => img,
    };
    if ops.flip_h {
        img = img.fliph();
    }
    if ops.flip_v {
        img = img.flipv();
    }
    if let Some(c) = ops.crop {
        let c = c.normalized(img.width(), img.height());
        img = img.crop_imm(c.x, c.y, c.w, c.h);
    }
    img
}

/// Size after rotation.
pub fn rotated_size(w: u32, h: u32, rotate: u8) -> (u32, u32) {
    if rotate % 2 == 1 { (h, w) } else { (w, h) }
}

fn edit_image(input: &Path, ops: &ImageOps, out: &Path) -> Result<(), PipelineError> {
    use image::ImageEncoder;
    let reader = image::ImageReader::open(input)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let icc = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    // EXIF is kept only when it has no rotation of its own (that's applied to the pixels
    // below, and a stale orientation tag would rotate the result twice).
    let exif = decoder
        .exif_metadata()
        .ok()
        .flatten()
        .filter(|_| orientation == image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let img = apply_image_ops(img, ops);
    let fmt = crate::pipeline::image_job::resolve_format(ops.format, input);
    let w = std::io::BufWriter::new(std::fs::File::create(out)?);
    match fmt {
        ImageFormat::Jpeg => {
            let mut e = image::codecs::jpeg::JpegEncoder::new_with_quality(w, ops.quality.clamp(1, 100));
            if let Some(p) = icc {
                let _ = e.set_icc_profile(p);
            }
            if let Some(x) = exif {
                let _ = e.set_exif_metadata(x);
            }
            e.write_image(img.to_rgb8().as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)?;
        }
        ImageFormat::Webp => {
            let img = if img.color().has_alpha() { DynamicImage::ImageRgba8(img.to_rgba8()) } else { DynamicImage::ImageRgb8(img.to_rgb8()) };
            let mut e = image::codecs::webp::WebPEncoder::new_lossless(w);
            if let Some(p) = icc {
                let _ = e.set_icc_profile(p);
            }
            if let Some(x) = exif {
                let _ = e.set_exif_metadata(x);
            }
            e.write_image(img.as_bytes(), img.width(), img.height(), img.color().into())?;
        }
        ImageFormat::Png | ImageFormat::Same => {
            let img = match img.color() {
                image::ColorType::Rgb8 | image::ColorType::Rgba8 | image::ColorType::Rgb16 | image::ColorType::Rgba16 | image::ColorType::L8 | image::ColorType::La8 | image::ColorType::L16 | image::ColorType::La16 => img,
                c if c.has_alpha() => DynamicImage::ImageRgba8(img.to_rgba8()),
                _ => DynamicImage::ImageRgb8(img.to_rgb8()),
            };
            let mut e = image::codecs::png::PngEncoder::new(w);
            if let Some(p) = icc {
                let _ = e.set_icc_profile(p);
            }
            if let Some(x) = exif {
                let _ = e.set_exif_metadata(x);
            }
            e.write_image(img.as_bytes(), img.width(), img.height(), img.color().into())?;
        }
    }
    Ok(())
}

// ---- running ------------------------------------------------------------------------------

/// Seconds of output written so far, from an ffmpeg `-progress` file.
pub fn progress_secs(text: &str) -> Option<f64> {
    text.lines()
        .rev()
        .find_map(|l| l.strip_prefix("out_time_us=").or_else(|| l.strip_prefix("out_time_ms=")))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|us| us.max(0) as f64 / 1e6)
}

/// Run one edit job (called on the runner thread).
pub fn run_edit(env: &Env, edit: &EditJob, out: &Path, progress: &ProgressHandle, cancel: &CancelToken) -> Result<JobOutcome, PipelineError> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let res = run_edit_inner(env, edit, out, progress, cancel);
    if res.is_err() {
        let _ = std::fs::remove_file(out);
    }
    res
}

fn run_edit_inner(env: &Env, edit: &EditJob, out: &Path, progress: &ProgressHandle, cancel: &CancelToken) -> Result<JobOutcome, PipelineError> {
    if let EditJob::Image { input, ops } = edit {
        progress.stage(Stage::Finalizing);
        edit_image(input, ops, out)?;
        return Ok(JobOutcome::Done { output: out.to_path_buf(), note: edit.describe() });
    }
    let ffmpeg = env.tools.ffmpeg.clone().ok_or_else(|| PipelineError::Setup(t!("err.ffmpeg_missing")))?;
    let work = crate::pipeline::temp::TempDir::new(&crate::pipeline::temp::temp_root(None), "edit")?;
    let prog_file = work.path().join("progress.txt");
    let (args, mode_note) = match edit {
        EditJob::Video { clip, crop, mode, enc } => {
            let copy = *mode == TrimMode::Fast && crop.is_none();
            (video_args(clip, *crop, *mode, enc, out, &prog_file), if copy { t!("editor.note_copy") } else { enc.encoder.label().to_string() })
        }
        EditJob::Join { clips, enc } => {
            if concat_compatible(clips) {
                let list = work.path().join("list.ffconcat");
                std::fs::write(&list, concat_list(clips))?;
                (concat_copy_args(&list, out, &prog_file), t!("editor.note_copy"))
            } else {
                (concat_reencode_args(clips, enc, out, &prog_file), enc.encoder.label().to_string())
            }
        }
        EditJob::Image { .. } => unreachable!(),
    };
    tracing::info!("edit: ffmpeg {}", args.join(" "));
    let total = edit.duration().max(0.001);
    let fps = edit.source().map(|_| match edit {
        EditJob::Video { clip, .. } => clip.info.fps,
        EditJob::Join { clips, .. } => clips[0].info.fps,
        _ => None,
    });
    let fps = fps.flatten().map(|f| f.as_f64()).unwrap_or(30.0);
    let total_frames = (total * fps).round().max(1.0) as u64;
    progress.update(|p| {
        p.stage = Stage::Encode;
        p.frames_total = total_frames;
        p.encoder = mode_note.clone();
    });
    let mut cmd = new_command(&ffmpeg);
    cmd.args(&args);
    let started = std::time::Instant::now();
    let mut tick = || {
        if let Some(s) = std::fs::read_to_string(&prog_file).ok().as_deref().and_then(progress_secs) {
            let done = ((s / total).clamp(0.0, 1.0) * total_frames as f64) as u64;
            let el = started.elapsed().as_secs_f64();
            progress.update(|p| {
                p.frames_done = done;
                p.fps = (done as f64 / el.max(0.01)) as f32;
                p.eta_secs = (s > 0.1).then(|| el * (total - s).max(0.0) / s);
            });
        }
    };
    run_capture_tick(cmd, Some(cancel), Priority::Normal, &mut tick)?;
    if !out.is_file() {
        return Err(PipelineError::Setup(t!("editor.err_no_output")));
    }
    let note = format!("{} · {}", edit.describe(), mode_note);
    Ok(JobOutcome::Done { output: out.to_path_buf(), note })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaKind;

    fn info(path: &str, w: u32, h: u32, fps: (u64, u64), audio: bool) -> MediaInfo {
        MediaInfo {
            path: path.into(),
            kind: MediaKind::Video,
            width: w,
            height: h,
            fps: Some(Rational::new(fps.0, fps.1)),
            vfr: false,
            duration: Some(10.0),
            frames: Some(300),
            video_codec: Some("h264".into()),
            audio_codecs: if audio { vec!["aac".into()] } else { vec![] },
            audio_layout: audio.then(|| "2@48000".into()),
            subtitle_codecs: vec![],
            pix_fmt: Some("yuv420p".into()),
            bit_depth: 8,
            hdr: false,
            has_alpha: false,
            file_size: 1,
            image_format: None,
        }
    }

    fn enc() -> EncodeChoice {
        EncodeChoice { encoder: VideoEncoder::Libx264, quality: Some(18), speed: EncoderSpeed::Balanced }
    }

    #[test]
    fn time_parse_and_format() {
        assert_eq!(parse_time("01:02:03.250"), Some(3723.25));
        assert_eq!(parse_time("2:03,5"), Some(123.5));
        assert_eq!(parse_time("42"), Some(42.0));
        assert_eq!(parse_time("1:75"), None);
        assert_eq!(parse_time("a:b"), None);
        assert_eq!(parse_time(""), None);
        assert_eq!(format_time(3723.25), "01:02:03.250");
        assert_eq!(format_time(0.0005), "00:00:00.001");
        for t in [0.0, 1.5, 59.999, 61.0, 7322.123] {
            assert!((parse_time(&format_time(t)).unwrap() - t).abs() < 0.0006);
        }
    }

    #[test]
    fn crop_is_even_and_inside() {
        let c = CropRect { x: 101, y: 51, w: 333, h: 999 }.normalized(1920, 1080);
        assert_eq!((c.w % 2, c.h % 2, c.x % 2, c.y % 2), (0, 0, 0, 0));
        assert!(c.x + c.w <= 1920 && c.y + c.h <= 1080);
        assert_eq!((c.w, c.h, c.x, c.y), (332, 998, 100, 50));
        let c = CropRect { x: 0, y: 500, w: 100, h: 2000 }.normalized(1920, 1080);
        assert_eq!((c.h, c.y), (1080, 0));
        let c = CropRect { x: 5000, y: 0, w: 3, h: 3 }.normalized(1919, 1079);
        assert_eq!((c.w, c.h), (16, 16));
        assert_eq!(c.x, 1918 - 16);
        let f = fit_aspect(CropRect::full(1920, 1080), 1.0, 1920, 1080);
        assert_eq!((f.w, f.h, f.x, f.y), (1080, 1080, 420, 0));
        let f = fit_aspect(CropRect::full(1920, 1080), 9.0 / 16.0, 1920, 1080);
        assert_eq!(f.h, 1080);
        assert_eq!(f.w, 608);
        assert_eq!(c.filter().split('=').next(), Some("crop"));
    }

    #[test]
    fn trim_fast_and_accurate_args() {
        let mut clip = Clip::new(info("in.mp4", 1920, 1080, (30, 1), true));
        clip.trim = Some((2.5, 7.0));
        let fast = video_args(&clip, None, TrimMode::Fast, &enc(), Path::new("out.mp4"), Path::new("p.txt")).join(" ");
        assert!(fast.contains("-ss 2.500 -i in.mp4 -t 4.500"), "{fast}");
        assert!(fast.contains("-c copy -avoid_negative_ts make_zero"), "{fast}");
        assert!(!fast.contains("libx264"));
        let acc = video_args(&clip, None, TrimMode::Accurate, &enc(), Path::new("out.mp4"), Path::new("p.txt")).join(" ");
        assert!(acc.contains("-c:v libx264") && acc.contains("-c:a aac") && !acc.contains("-c copy"), "{acc}");
        // Crop always re-encodes, even in fast mode.
        let crop = Some(CropRect { x: 0, y: 0, w: 1280, h: 720 });
        let c = video_args(&clip, crop, TrimMode::Fast, &enc(), Path::new("out.mkv"), Path::new("p.txt")).join(" ");
        assert!(c.contains("-vf crop=1280:720:0:0") && c.contains("libx264") && !c.contains("faststart"), "{c}");
        // Untrimmed: no -ss / -t.
        let whole = Clip::new(info("in.mp4", 1920, 1080, (30, 1), true));
        let w = video_args(&whole, crop, TrimMode::Accurate, &enc(), Path::new("o.mp4"), Path::new("p")).join(" ");
        assert!(!w.contains("-ss") && !w.contains(" -t "), "{w}");
        assert_eq!(output_ext(Path::new("a.MOV"), false), "mov");
        assert_eq!(output_ext(Path::new("a.mov"), true), "mp4");
        assert_eq!(output_ext(Path::new("a.mkv"), true), "mkv");
    }

    #[test]
    fn concat_compatibility_and_filter_graph() {
        let a = Clip::new(info("a.mp4", 1920, 1080, (30, 1), true));
        let b = Clip::new(info("b.mp4", 1920, 1080, (30, 1), true));
        assert!(concat_compatible(&[a.clone(), b.clone()]));
        let c = Clip::new(info("c.mp4", 1280, 720, (30, 1), true));
        assert!(!concat_compatible(&[a.clone(), c.clone()]));
        let d = Clip::new(info("d.mp4", 1920, 1080, (30000, 1001), true));
        assert!(!concat_compatible(&[a.clone(), d]));
        let silent = Clip::new(info("s.mp4", 1920, 1080, (30, 1), false));
        assert!(!concat_compatible(&[a.clone(), silent.clone()]));

        let mut a2 = a.clone();
        a2.trim = Some((1.0, 4.0));
        let list = concat_list(&[a2.clone(), Clip::new(info("it's.mp4", 1920, 1080, (30, 1), true))]);
        assert!(list.contains("file 'a.mp4'\ninpoint 1.000\noutpoint 4.000\n"), "{list}");
        assert!(list.contains(r"file 'it'\''s.mp4'"), "{list}");

        let clips = [a2, c, silent];
        let args = concat_reencode_args(&clips, &enc(), Path::new("out.mp4"), Path::new("p"));
        let j = args.join(" ");
        assert!(j.contains("-ss 1.000 -t 3.000 -i a.mp4"), "{j}");
        assert!(j.contains("-f lavfi -t 10.000 -i anullsrc=channel_layout=stereo:sample_rate=48000"), "{j}");
        let fc = &args[args.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        assert!(fc.contains("[1:v:0]scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080"), "{fc}");
        assert!(fc.contains("fps=30/1"), "{fc}");
        assert!(fc.contains("[3:a]aresample=48000"), "silent clip uses anullsrc input 3: {fc}");
        assert!(fc.ends_with("[v0][a0][v1][a1][v2][a2]concat=n=3:v=1:a=1[v][a]"), "{fc}");
        // No audio anywhere → video-only concat.
        let (fc, audio) = concat_filter(&[Clip::new(info("x.mp4", 640, 480, (25, 1), false))], 640, 480, Rational::new(25, 1), &[None]);
        assert!(!audio && fc.ends_with("concat=n=1:v=1:a=0[v]"), "{fc}");
    }

    #[test]
    fn progress_file_parsing() {
        assert_eq!(progress_secs("frame=10\nout_time_us=2500000\nprogress=continue\n"), Some(2.5));
        assert_eq!(progress_secs("out_time_us=1000000\nout_time_us=3000000\n"), Some(3.0));
        assert_eq!(progress_secs("frame=1\n"), None);
    }

    #[test]
    fn image_ops() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(40, 20, |x, y| image::Rgb([x as u8, y as u8, 0])));
        // Rotated 90° + flipped: 20x40, pixel (x, y) = source (y, x); crop y 10..30.
        let ops = ImageOps { crop: Some(CropRect { x: 0, y: 10, w: 20, h: 20 }), rotate: 1, flip_h: true, ..Default::default() };
        let out = apply_image_ops(img, &ops).to_rgb8();
        assert_eq!(out.dimensions(), (20, 20));
        assert_eq!(out.get_pixel(0, 0).0, [10, 0, 0]);
        assert_eq!(rotated_size(40, 20, 1), (20, 40));
        assert_eq!(rotated_size(40, 20, 2), (40, 20));
    }
}
