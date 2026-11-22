//! Editor page: trim / crop a video, join clips, crop / rotate / flip an image.
//! Exports run on the shared runner (queue) through [`crate::editor::run_edit`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::RgbImage;

use crate::app::App;
use crate::config::{ImageFormat, Studio};
use crate::editor::{Aspect, Clip, CropRect, EditJob, EncodeChoice, ImageOps, TrimMode, fit_aspect, format_time, parse_time};
use crate::media::MediaKind;
use crate::media::ffmpeg::{AudioClip, DecodeSpec};
use crate::media::probe::MediaInfo;
use crate::pipeline::cancel::CancelToken;
use crate::pipeline::job::{JobOutcome, JobSpec};
use crate::preview::player::{PlayerPrefs, SamplePlayer};
use crate::t;

const ACCENT: Color32 = Color32::from_rgb(90, 160, 255);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const STRIP: usize = 12;
/// Longest selection the in-memory player decodes.
const PLAY_MAX_SECS: f64 = 20.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Video,
    Join,
    Image,
}

// ---- background loader ---------------------------------------------------------------------

enum Req {
    Probe { path: PathBuf, purpose: Purpose },
    Strip { seq: u64, path: PathBuf, duration: f64, w: u32, h: u32 },
    Frame { seq: u64, path: PathBuf, t: f64, w: u32, h: u32 },
    Thumb { key: u64, path: PathBuf, w: u32, h: u32 },
    Play { seq: u64, path: PathBuf, start: f64, dur: f64, w: u32, h: u32, fps: f64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Purpose {
    Video,
    Join,
    Image,
}

enum Res {
    Probed(Purpose, Result<MediaInfo, String>),
    Strip(u64, Vec<RgbImage>),
    Frame(u64, f64, RgbImage),
    Thumb(u64, RgbImage),
    Play(u64, Result<(Vec<RgbImage>, Option<AudioClip>, f64), String>),
}

fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let s = (max_w as f64 / w.max(1) as f64).min(max_h as f64 / h.max(1) as f64).min(1.0);
    (((w as f64 * s) as u32).max(2) & !1, ((h as f64 * s) as u32).max(2) & !1)
}

fn spawn_loader(env: Arc<crate::pipeline::job::Env>, ctx: egui::Context) -> (Sender<Req>, Receiver<Res>) {
    let (tx, rx) = unbounded::<Req>();
    let (otx, orx) = unbounded::<Res>();
    let _ = std::thread::Builder::new().name("editor-loader".into()).spawn(move || {
        let cancel = CancelToken::new();
        while let Ok(first) = rx.recv() {
            // Only the newest frame request matters (scrubbing).
            let mut reqs = vec![first];
            reqs.extend(rx.try_iter());
            let last_frame = reqs.iter().rposition(|r| matches!(r, Req::Frame { .. }));
            for (i, r) in reqs.into_iter().enumerate() {
                if matches!(r, Req::Frame { .. }) && Some(i) != last_frame {
                    continue;
                }
                let res = handle(&env, r, &cancel);
                if let Some(res) = res {
                    if otx.send(res).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
        }
    });
    (tx, orx)
}

fn handle(env: &crate::pipeline::job::Env, r: Req, cancel: &CancelToken) -> Option<Res> {
    let ffmpeg = env.tools.ffmpeg.clone();
    match r {
        Req::Probe { path, purpose } => Some(Res::Probed(
            purpose,
            crate::media::probe::probe(env.tools.ffprobe.as_deref(), &path).map_err(|e| e.to_string()),
        )),
        Req::Strip { seq, path, duration, w, h } => {
            let ff = ffmpeg?;
            let frames = (0..STRIP)
                .filter_map(|i| {
                    let t = duration * (i as f64 + 0.5) / STRIP as f64;
                    crate::media::ffmpeg::extract_frame(&ff, &path, t, w, h, true, Some(cancel)).ok()
                })
                .collect();
            Some(Res::Strip(seq, frames))
        }
        Req::Frame { seq, path, t, w, h } => {
            let img = crate::media::ffmpeg::extract_frame(&ffmpeg?, &path, t, w, h, true, Some(cancel)).ok()?;
            Some(Res::Frame(seq, t, img))
        }
        Req::Thumb { key, path, w, h } => {
            let img = crate::media::ffmpeg::extract_frame(&ffmpeg?, &path, 0.0, w, h, true, Some(cancel)).ok()?;
            Some(Res::Thumb(key, img))
        }
        Req::Play { seq, path, start, dur, w, h, fps } => {
            let ff = ffmpeg?;
            let spec = DecodeSpec {
                input: path.clone(),
                segment: Some((start, dur)),
                fps: None,
                width: w,
                height: h,
                scale: true,
            };
            let max = (dur * fps).ceil() as usize + 2;
            let r = crate::media::ffmpeg::decode_clip(&ff, &spec, max, cancel)
                .map_err(|e| e.to_string())
                .map(|frames| {
                    let audio = crate::media::ffmpeg::decode_audio(&ff, &path, Some((start, dur)), cancel).ok().flatten();
                    (frames, audio, fps)
                });
            Some(Res::Play(seq, r))
        }
    }
}

// ---- state ---------------------------------------------------------------------------------

/// Everything undo/redo restores.
#[derive(Clone, Debug, PartialEq, Default)]
struct Snap {
    in_t: f64,
    out_t: f64,
    crop: Option<CropRect>,
    img_crop: Option<CropRect>,
    rotate: u8,
    flip_h: bool,
    flip_v: bool,
    clips: Vec<(u64, Option<(f64, f64)>)>,
}

struct VideoDoc {
    info: MediaInfo,
    seq: u64,
    playhead: f64,
    in_t: f64,
    out_t: f64,
    crop: Option<CropRect>,
    strip: Vec<TextureHandle>,
    frame: Option<(TextureHandle, f64)>,
    requested: f64,
    in_txt: String,
    out_txt: String,
    player: Option<SamplePlayer>,
    loading_play: bool,
}

struct ImageDoc {
    info: MediaInfo,
    tex: Option<TextureHandle>,
    crop: Option<CropRect>,
    rotate: u8,
    flip_h: bool,
    flip_v: bool,
}

struct JoinClip {
    key: u64,
    clip: Clip,
    thumb: Option<TextureHandle>,
    in_txt: String,
    out_txt: String,
}

pub struct EditorState {
    pub mode: Mode,
    video: Option<VideoDoc>,
    image: Option<ImageDoc>,
    clips: Vec<JoinClip>,
    next_key: u64,
    seq: u64,
    pub aspect: Aspect,
    pub trim_mode: TrimMode,
    pub image_format: ImageFormat,
    pub image_quality: u8,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    committed: Snap,
    /// Editor jobs on the runner: id → kind of the output.
    jobs: HashMap<u64, MediaKind>,
    pub last_output: Option<(PathBuf, MediaKind)>,
    loader: Option<(Sender<Req>, Receiver<Res>)>,
    error: Option<String>,
}

impl Default for EditorState {
    fn default() -> Self {
        EditorState {
            mode: Mode::Video,
            video: None,
            image: None,
            clips: Vec::new(),
            next_key: 1,
            seq: 0,
            aspect: Aspect::Free,
            trim_mode: TrimMode::Accurate,
            image_format: ImageFormat::Same,
            image_quality: 92,
            undo: Vec::new(),
            redo: Vec::new(),
            committed: Snap::default(),
            jobs: HashMap::new(),
            last_output: None,
            loader: None,
            error: None,
        }
    }
}

impl EditorState {
    pub fn owns(&self, job: u64) -> bool {
        self.jobs.contains_key(&job)
    }
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    fn snap(&self) -> Snap {
        let v = self.video.as_ref();
        let i = self.image.as_ref();
        Snap {
            in_t: v.map_or(0.0, |v| v.in_t),
            out_t: v.map_or(0.0, |v| v.out_t),
            crop: v.and_then(|v| v.crop),
            img_crop: i.and_then(|i| i.crop),
            rotate: i.map_or(0, |i| i.rotate),
            flip_h: i.is_some_and(|i| i.flip_h),
            flip_v: i.is_some_and(|i| i.flip_v),
            clips: self.clips.iter().map(|c| (c.key, c.clip.trim)).collect(),
        }
    }

    fn restore(&mut self, s: &Snap) {
        if let Some(v) = &mut self.video {
            v.in_t = s.in_t;
            v.out_t = s.out_t;
            v.crop = s.crop;
            v.in_txt = format_time(s.in_t);
            v.out_txt = format_time(s.out_t);
        }
        if let Some(i) = &mut self.image {
            i.crop = s.img_crop;
            i.rotate = s.rotate;
            i.flip_h = s.flip_h;
            i.flip_v = s.flip_v;
        }
        // Clips: keep those still in the snapshot, in its order, with its trims (a clip
        // removed from the list can't be brought back by undo).
        let mut old: HashMap<u64, JoinClip> = self.clips.drain(..).map(|c| (c.key, c)).collect();
        for (k, trim) in &s.clips {
            if let Some(mut c) = old.remove(k) {
                c.clip.trim = *trim;
                let (a, b) = c.clip.span();
                c.in_txt = format_time(a);
                c.out_txt = format_time(b);
                self.clips.push(c);
            }
        }
        self.committed = s.clone();
    }

    /// Record a finished change for undo (coalesces drags and typing).
    fn track(&mut self, ctx: &egui::Context) {
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input();
        let now = self.snap();
        if !busy && now != self.committed {
            let prev = std::mem::replace(&mut self.committed, now);
            self.undo.push(prev);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snap());
            self.restore(&s);
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snap());
            self.restore(&s);
        }
    }

    fn reset_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.committed = self.snap();
    }
}

fn loader(app: &mut App) -> Sender<Req> {
    if app.editor.loader.is_none() {
        app.editor.loader = Some(spawn_loader(app.env.clone(), app.egui_ctx.clone()));
    }
    app.editor.loader.as_ref().expect("set").0.clone()
}

/// Files dropped on / opened from the Editor page.
pub fn add_paths(app: &mut App, paths: Vec<PathBuf>) {
    let tx = loader(app);
    let mut videos: Vec<PathBuf> = Vec::new();
    for p in paths {
        match crate::media::detect_kind(&p) {
            Some(MediaKind::Video) => videos.push(p),
            Some(MediaKind::Image) => {
                app.editor.mode = Mode::Image;
                let _ = tx.send(Req::Probe { path: p, purpose: Purpose::Image });
            }
            None => {}
        }
    }
    if videos.len() > 1 || (app.editor.mode == Mode::Join && !videos.is_empty()) {
        app.editor.mode = Mode::Join;
        for p in videos {
            let _ = tx.send(Req::Probe { path: p, purpose: Purpose::Join });
        }
    } else if let Some(p) = videos.pop() {
        app.editor.mode = Mode::Video;
        let _ = tx.send(Req::Probe { path: p, purpose: Purpose::Video });
    }
}

fn tex(ctx: &egui::Context, name: &str, img: &RgbImage) -> TextureHandle {
    ctx.load_texture(name, crate::preview::to_color_image(img), TextureOptions::LINEAR)
}

fn pump(app: &mut App, ctx: &egui::Context) {
    let Some((_, rx)) = &app.editor.loader else {
        return;
    };
    let results: Vec<Res> = rx.try_iter().collect();
    let tx = loader(app);
    for r in results {
        let ed = &mut app.editor;
        match r {
            Res::Probed(purpose, Err(e)) => {
                tracing::warn!("editor: probe failed ({purpose:?}): {e}");
                ed.error = Some(t!("editor.err_open", error = e));
            }
            Res::Probed(Purpose::Video, Ok(info)) => {
                ed.error = None;
                ed.seq += 1;
                let d = info.duration.unwrap_or(0.0);
                let (sw, sh) = fit(info.width, info.height, 160, 90);
                let _ = tx.send(Req::Strip { seq: ed.seq, path: info.path.clone(), duration: d, w: sw, h: sh });
                ed.video = Some(VideoDoc {
                    seq: ed.seq,
                    playhead: 0.0,
                    in_t: 0.0,
                    out_t: d,
                    crop: None,
                    strip: Vec::new(),
                    frame: None,
                    requested: -1.0,
                    in_txt: format_time(0.0),
                    out_txt: format_time(d),
                    player: None,
                    loading_play: false,
                    info,
                });
                ed.reset_history();
            }
            Res::Probed(Purpose::Join, Ok(info)) => {
                let key = ed.next_key;
                ed.next_key += 1;
                let (w, h) = fit(info.width, info.height, 96, 54);
                let _ = tx.send(Req::Thumb { key, path: info.path.clone(), w, h });
                let d = info.duration.unwrap_or(0.0);
                ed.clips.push(JoinClip { key, clip: Clip::new(info), thumb: None, in_txt: format_time(0.0), out_txt: format_time(d) });
            }
            Res::Probed(Purpose::Image, Ok(info)) => {
                ed.error = None;
                match crate::pipeline::image_job::load_rgb_alpha(&info.path) {
                    Ok((rgb, _)) => {
                        let (w, h) = fit(rgb.width(), rgb.height(), 2048, 2048);
                        let small = image::imageops::resize(&rgb, w, h, image::imageops::FilterType::Triangle);
                        ed.image = Some(ImageDoc {
                            tex: Some(tex(ctx, "editor-image", &small)),
                            info,
                            crop: None,
                            rotate: 0,
                            flip_h: false,
                            flip_v: false,
                        });
                        ed.reset_history();
                    }
                    Err(e) => ed.error = Some(t!("editor.err_open", error = e.user_message())),
                }
            }
            Res::Strip(seq, frames) => {
                if let Some(v) = ed.video.as_mut().filter(|v| v.seq == seq) {
                    v.strip = frames.iter().enumerate().map(|(i, f)| tex(ctx, &format!("strip{i}"), f)).collect();
                }
            }
            Res::Frame(seq, t, img) => {
                if let Some(v) = ed.video.as_mut().filter(|v| v.seq == seq) {
                    match &mut v.frame {
                        Some((h, ft)) => {
                            h.set(crate::preview::to_color_image(&img), TextureOptions::LINEAR);
                            *ft = t;
                        }
                        None => v.frame = Some((tex(ctx, "editor-frame", &img), t)),
                    }
                }
            }
            Res::Thumb(key, img) => {
                if let Some(c) = ed.clips.iter_mut().find(|c| c.key == key) {
                    c.thumb = Some(tex(ctx, &format!("clip{key}"), &img));
                }
            }
            Res::Play(seq, r) => {
                let prefs = PlayerPrefs {
                    volume: &mut app.config.preview_volume,
                    muted: &mut app.config.preview_muted,
                    looping: &mut app.config.preview_loop,
                };
                if let Some(v) = app.editor.video.as_mut().filter(|v| v.seq == seq) {
                    v.loading_play = false;
                    match r {
                        Ok((frames, audio, fps)) if !frames.is_empty() => {
                            let size = (frames[0].width(), frames[0].height());
                            let mut p = SamplePlayer::new(Vec::new(), frames, fps, fps, size, size, audio, String::new(), &prefs);
                            p.single = true;
                            v.player = Some(p);
                        }
                        Ok(_) => app.editor.error = Some(t!("editor.err_play", error = "no frames")),
                        Err(e) => app.editor.error = Some(t!("editor.err_play", error = e)),
                    }
                }
            }
        }
    }
}

// ---- export ---------------------------------------------------------------------------------

fn encode_choice(app: &App) -> EncodeChoice {
    let o = &app.config.video_settings.output;
    let av = app.env.encoders.read().map(|a| a.clone()).unwrap_or_default();
    EncodeChoice {
        encoder: av.resolve(o.encoder).unwrap_or(crate::media::encoders::VideoEncoder::Libx264),
        quality: o.quality,
        speed: o.encoder_speed,
    }
}

fn out_path(src: &Path, ext: &str) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "edit".into());
    let dir = src.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut p = dir.join(format!("{stem}_edited.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}_edited ({n}).{ext}"));
        n += 1;
    }
    p
}

fn submit(app: &mut App, info: MediaInfo, edit: EditJob, out: PathBuf, kind: MediaKind) {
    let id = app.alloc_id();
    app.editor.jobs.insert(id, kind);
    app.editor.last_output = None;
    app.runner.submit(vec![JobSpec {
        id,
        info,
        settings: Default::default(),
        output: out.clone(),
        edit: Some(Arc::new(edit)),
    }]);
    app.toast(t!("editor.queued", name = out.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
}

/// Runner finished one of the editor's jobs.
pub fn on_finished(app: &mut App, id: u64, outcome: JobOutcome) {
    let Some(kind) = app.editor.jobs.remove(&id) else {
        return;
    };
    match outcome {
        JobOutcome::Done { output, note } => {
            app.record_edit_history(&output, kind, &note);
            app.toast(t!("editor.done", name = output.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
            app.editor.last_output = Some((output, kind));
        }
        JobOutcome::Failed(e) => {
            app.editor.error = Some(t!("editor.err_export", error = e));
        }
        JobOutcome::Cancelled => app.toast(t!("status.cancelled")),
    }
}

// ---- UI ------------------------------------------------------------------------------------

pub fn editor_page(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    pump(app, &ctx);
    keys(app, &ctx);
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading(t!("editor.title"));
            ui.separator();
            for (m, l) in [(Mode::Video, t!("editor.mode_video")), (Mode::Join, t!("editor.mode_join")), (Mode::Image, t!("editor.mode_image"))] {
                ui.selectable_value(&mut app.editor.mode, m, RichText::new(l).strong());
            }
            ui.separator();
            if ui.button(t!("editor.open")).clicked() {
                let (name, exts): (&str, &[&str]) = match app.editor.mode {
                    Mode::Image => ("Images", &crate::media::IMAGE_EXTS),
                    _ => ("Videos", &crate::media::VIDEO_EXTS),
                };
                let dlg = rfd::FileDialog::new().add_filter(name, exts);
                let files = if app.editor.mode == Mode::Join { dlg.pick_files().unwrap_or_default() } else { dlg.pick_file().into_iter().collect() };
                if !files.is_empty() {
                    add_paths(app, files);
                }
            }
            ui.separator();
            let ed = &mut app.editor;
            if ui.add_enabled(!ed.undo.is_empty(), egui::Button::new(t!("editor.undo_short"))).on_hover_text(t!("editor.undo")).clicked() {
                ed.undo();
            }
            if ui.add_enabled(!ed.redo.is_empty(), egui::Button::new(t!("editor.redo_short"))).on_hover_text(t!("editor.redo")).clicked() {
                ed.redo();
            }
        });
        if let Some(e) = app.editor.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(235, 100, 100), e);
                if ui.small_button("✕").clicked() {
                    app.editor.error = None;
                }
            });
        }
        result_bar(app, ui);
        ui.separator();
        match app.editor.mode {
            Mode::Video => video_ui(app, ui),
            Mode::Join => join_ui(app, ui),
            Mode::Image => image_ui(app, ui),
        }
    });
    app.editor.track(&ctx);
}

fn keys(app: &mut App, ctx: &egui::Context) {
    let (undo, redo) = ctx.input_mut(|i| {
        let redo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
        let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
        (undo, redo)
    });
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    if undo {
        app.editor.undo();
    }
    if redo {
        app.editor.redo();
    }
    if app.editor.mode != Mode::Video {
        return;
    }
    let Some(v) = app.editor.video.as_mut() else {
        return;
    };
    let step = 1.0 / v.info.fps.map(|f| f.as_f64()).unwrap_or(30.0);
    let d = v.info.duration.unwrap_or(0.0);
    ctx.input(|i| {
        if i.key_pressed(egui::Key::I) {
            v.in_t = v.playhead.min(v.out_t);
            v.in_txt = format_time(v.in_t);
        }
        if i.key_pressed(egui::Key::O) {
            v.out_t = v.playhead.max(v.in_t);
            v.out_txt = format_time(v.out_t);
        }
        if i.key_pressed(egui::Key::ArrowLeft) {
            v.playhead = (v.playhead - step).max(0.0);
        }
        if i.key_pressed(egui::Key::ArrowRight) {
            v.playhead = (v.playhead + step).min(d);
        }
    });
}

fn result_bar(app: &mut App, ui: &mut egui::Ui) {
    if app.editor.busy() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(t!("editor.exporting"));
        });
    }
    let Some((out, kind)) = app.editor.last_output.clone() else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("✔ {}", out.display())).color(Color32::from_rgb(110, 200, 120)));
        let (studio, label) = match kind {
            MediaKind::Video => (Studio::Video, t!("editor.send_video")),
            MediaKind::Image => (Studio::Image, t!("editor.send_image")),
        };
        if ui.button(label).on_hover_text(t!("editor.send_tip")).clicked() {
            app.switch_studio(studio);
            app.add_paths(vec![out.clone()]);
        }
        if ui.button(t!("editor.show_outputs")).clicked() {
            app.page = crate::app::Page::Outputs;
            let ctx = ui.ctx().clone();
            app.refresh_outputs(&ctx);
        }
    });
}

fn drop_hint(ui: &mut egui::Ui, text: String) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(text).size(16.0).weak());
    });
}

/// Image `size` fitted into `area`.
fn fit_rect(area: Rect, size: Vec2) -> Rect {
    let s = (area.width() / size.x.max(1.0)).min(area.height() / size.y.max(1.0));
    Rect::from_center_size(area.center(), size * s)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Grip {
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
    New,
}

/// Crop rectangle editor over an image drawn in `r` (screen) for a `w`x`h` source.
fn crop_overlay(ui: &mut egui::Ui, id: egui::Id, r: Rect, w: u32, h: u32, crop: &mut CropRect, ratio: Option<f64>) {
    let sx = r.width() / w as f32;
    let sy = r.height() / h as f32;
    let to_screen = |c: &CropRect| Rect::from_min_size(r.min + Vec2::new(c.x as f32 * sx, c.y as f32 * sy), Vec2::new(c.w as f32 * sx, c.h as f32 * sy));
    let resp = ui.interact(r, id, Sense::drag());
    let cr = to_screen(crop);
    let painter = ui.painter_at(r);
    // Dim outside.
    let dim = Color32::from_black_alpha(140);
    painter.rect_filled(Rect::from_min_max(r.min, Pos2::new(r.max.x, cr.min.y)), 0.0, dim);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.min.x, cr.max.y), r.max), 0.0, dim);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.min.x, cr.min.y), Pos2::new(cr.min.x, cr.max.y)), 0.0, dim);
    painter.rect_filled(Rect::from_min_max(Pos2::new(cr.max.x, cr.min.y), Pos2::new(r.max.x, cr.max.y)), 0.0, dim);
    painter.rect_stroke(cr, 0.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Middle);
    for x in [1.0 / 3.0, 2.0 / 3.0] {
        painter.line_segment([Pos2::new(cr.min.x + cr.width() * x, cr.min.y), Pos2::new(cr.min.x + cr.width() * x, cr.max.y)], Stroke::new(0.5, Color32::from_white_alpha(80)));
        painter.line_segment([Pos2::new(cr.min.x, cr.min.y + cr.height() * x), Pos2::new(cr.max.x, cr.min.y + cr.height() * x)], Stroke::new(0.5, Color32::from_white_alpha(80)));
    }
    let handles = [
        (Grip::NW, cr.left_top()),
        (Grip::NE, cr.right_top()),
        (Grip::SW, cr.left_bottom()),
        (Grip::SE, cr.right_bottom()),
        (Grip::N, cr.center_top()),
        (Grip::S, cr.center_bottom()),
        (Grip::W, cr.left_center()),
        (Grip::E, cr.right_center()),
    ];
    for (_, p) in handles {
        painter.rect_filled(Rect::from_center_size(p, Vec2::splat(9.0)), 1.0, ACCENT);
    }
    let grip_id = id.with("grip");
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let g = handles
            .iter()
            .find(|(_, h)| h.distance(p) < 10.0)
            .map(|(g, _)| *g)
            .unwrap_or(if cr.contains(p) { Grip::Move } else { Grip::New });
        ui.data_mut(|d| d.insert_temp(grip_id, (g, *crop, p)));
    }
    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.dragged())
        && let Some((g, start, origin)) = ui.data(|d| d.get_temp::<(Grip, CropRect, Pos2)>(grip_id))
    {
        let dx = ((p.x - origin.x) / sx) as i64;
        let dy = ((p.y - origin.y) / sy) as i64;
        let (mut x0, mut y0, mut x1, mut y1) = (start.x as i64, start.y as i64, (start.x + start.w) as i64, (start.y + start.h) as i64);
        match g {
            Grip::Move => {
                let nx = (x0 + dx).clamp(0, w as i64 - start.w as i64);
                let ny = (y0 + dy).clamp(0, h as i64 - start.h as i64);
                x1 += nx - x0;
                y1 += ny - y0;
                x0 = nx;
                y0 = ny;
            }
            Grip::New => {
                let ox = ((origin.x - r.min.x) / sx) as i64;
                let oy = ((origin.y - r.min.y) / sy) as i64;
                let px = ((p.x - r.min.x) / sx) as i64;
                let py = ((p.y - r.min.y) / sy) as i64;
                (x0, x1) = (ox.min(px), ox.max(px));
                (y0, y1) = (oy.min(py), oy.max(py));
            }
            _ => {
                let name = format!("{g:?}");
                if name.contains('W') {
                    x0 += dx;
                }
                if name.contains('E') {
                    x1 += dx;
                }
                if name.contains('N') {
                    y0 += dy;
                }
                if name.contains('S') {
                    y1 += dy;
                }
            }
        }
        let (x0, x1) = (x0.min(x1).clamp(0, w as i64), x0.max(x1).clamp(0, w as i64));
        let (y0, y1) = (y0.min(y1).clamp(0, h as i64), y0.max(y1).clamp(0, h as i64));
        let mut c = CropRect { x: x0 as u32, y: y0 as u32, w: (x1 - x0) as u32, h: (y1 - y0) as u32 };
        if let Some(ratio) = ratio
            && g != Grip::Move
        {
            // Keep the ratio, anchored at the side/corner opposite the grip.
            let (cw, ch) = (c.w as f64, c.h as f64);
            let (nw, nh) = if matches!(g, Grip::N | Grip::S) { (ch * ratio, ch) } else { (cw, cw / ratio) };
            let (nw, nh) = {
                let s = (w as f64 / nw).min(h as f64 / nh).min(1.0);
                (nw * s, nh * s)
            };
            let name = format!("{g:?}");
            let nx = if name.contains('W') { c.x as f64 + cw - nw } else { c.x as f64 };
            let ny = if name.contains('N') { c.y as f64 + ch - nh } else { c.y as f64 };
            c = CropRect { x: nx.max(0.0) as u32, y: ny.max(0.0) as u32, w: nw as u32, h: nh as u32 };
        }
        *crop = c.normalized(w, h);
    }
}

fn crop_controls(ui: &mut egui::Ui, crop: &mut Option<CropRect>, aspect: &mut Aspect, w: u32, h: u32) {
    ui.horizontal_wrapped(|ui| {
        let mut on = crop.is_some();
        if ui.checkbox(&mut on, RichText::new(t!("editor.crop")).strong()).changed() {
            *crop = on.then(|| match aspect.ratio(w, h) {
                Some(r) => fit_aspect(CropRect::full(w, h), r, w, h),
                None => CropRect::full(w, h).normalized(w, h),
            });
        }
        let before = *aspect;
        egui::ComboBox::from_id_salt("crop-aspect").selected_text(aspect.label()).width(110.0).show_ui(ui, |ui| {
            for a in Aspect::ALL {
                ui.selectable_value(aspect, a, a.label());
            }
        });
        if *aspect != before
            && let (Some(c), Some(r)) = (crop.as_mut(), aspect.ratio(w, h))
        {
            *c = fit_aspect(CropRect::full(w, h), r, w, h);
        }
        if let Some(c) = crop.as_mut() {
            let mut v = *c;
            ui.label("x");
            ui.add(egui::DragValue::new(&mut v.x).range(0..=w));
            ui.label("y");
            ui.add(egui::DragValue::new(&mut v.y).range(0..=h));
            ui.label("w");
            ui.add(egui::DragValue::new(&mut v.w).range(16..=w));
            ui.label("h");
            ui.add(egui::DragValue::new(&mut v.h).range(16..=h));
            if v != *c {
                *c = v.normalized(w, h);
            }
            if ui.small_button(t!("editor.crop_reset")).clicked() {
                *c = CropRect::full(w, h).normalized(w, h);
            }
            ui.label(RichText::new(t!("editor.crop_even")).small().weak());
        }
    });
}

fn time_field(ui: &mut egui::Ui, txt: &mut String, value: &mut f64, lo: f64, hi: f64) {
    let r = ui.add(egui::TextEdit::singleline(txt).desired_width(96.0).font(egui::TextStyle::Monospace));
    if r.lost_focus() {
        if let Some(t) = parse_time(txt) {
            *value = t.clamp(lo, hi);
        }
        *txt = format_time(*value);
    } else if !r.has_focus() {
        let f = format_time(*value);
        if *txt != f {
            *txt = f;
        }
    }
    r.on_hover_text(t!("editor.time_tip"));
}

fn video_ui(app: &mut App, ui: &mut egui::Ui) {
    let tx = loader(app);
    let aspect_ptr = app.editor.aspect;
    let Some(v) = app.editor.video.as_mut() else {
        drop_hint(ui, t!("editor.drop_video"));
        return;
    };
    let d = v.info.duration.unwrap_or(0.0).max(0.001);
    let fps = v.info.fps.map(|f| f.as_f64()).unwrap_or(30.0);
    let (w, h) = (v.info.width, v.info.height);
    ui.label(RichText::new(format!(
        "{} · {}×{} · {} fps · {}",
        v.info.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        w,
        h,
        v.info.fps.map(|f| f.to_string()).unwrap_or_else(|| "?".into()),
        format_time(d)
    )).weak());

    // Viewer.
    let avail = ui.available_size();
    let view_h = (avail.y - 210.0).clamp(160.0, 900.0);
    let (area, _) = ui.allocate_exact_size(Vec2::new(avail.x, view_h), Sense::hover());
    ui.painter().rect_filled(area, 4.0, Color32::from_gray(18));
    if let Some(p) = v.player.as_mut() {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
        p.ui(
            &mut child,
            PlayerPrefs { volume: &mut app.config.preview_volume, muted: &mut app.config.preview_muted, looping: &mut app.config.preview_loop },
        );
    } else {
        // Fetch the frame under the playhead (latest request wins).
        if (v.requested - v.playhead).abs() > 1e-4 {
            let (fw, fh) = fit(w, h, (area.width() as u32).clamp(320, 1600), (area.height() as u32).clamp(180, 900));
            let _ = tx.send(Req::Frame { seq: v.seq, path: v.info.path.clone(), t: v.playhead, w: fw, h: fh });
            v.requested = v.playhead;
        }
        if let Some((t, _)) = &v.frame {
            let r = fit_rect(area, Vec2::new(w as f32, h as f32));
            ui.painter().image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            if let Some(c) = v.crop.as_mut() {
                crop_overlay(ui, ui.scope_id().with("vcrop"), r, w, h, c, aspect_ptr.ratio(w, h));
            }
        } else {
            ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, t!("editor.loading"), egui::FontId::proportional(14.0), Color32::GRAY);
        }
    }

    // Timeline with thumbnail strip.
    let (tl, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), Sense::click_and_drag());
    let p = ui.painter_at(tl);
    p.rect_filled(tl, 3.0, Color32::from_gray(30));
    if !v.strip.is_empty() {
        let cw = tl.width() / v.strip.len() as f32;
        for (i, t) in v.strip.iter().enumerate() {
            let cell = Rect::from_min_size(tl.min + Vec2::new(i as f32 * cw, 0.0), Vec2::new(cw, tl.height()));
            let r = fit_rect(cell, t.size_vec2());
            p.image(t.id(), Rect::from_center_size(cell.center(), Vec2::new(cell.width(), r.height().max(cell.height()))), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }
    let x_of = |t: f64| tl.min.x + (t / d) as f32 * tl.width();
    let dim = Color32::from_black_alpha(170);
    p.rect_filled(Rect::from_min_max(tl.min, Pos2::new(x_of(v.in_t), tl.max.y)), 0.0, dim);
    p.rect_filled(Rect::from_min_max(Pos2::new(x_of(v.out_t), tl.min.y), tl.max), 0.0, dim);
    p.rect_stroke(Rect::from_min_max(Pos2::new(x_of(v.in_t), tl.min.y), Pos2::new(x_of(v.out_t), tl.max.y)), 2.0, Stroke::new(2.0, Color32::from_rgb(250, 200, 60)), egui::StrokeKind::Inside);
    let px = x_of(v.playhead);
    p.line_segment([Pos2::new(px, tl.min.y - 2.0), Pos2::new(px, tl.max.y + 2.0)], Stroke::new(2.0, Color32::WHITE));
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let t = ((pos.x - tl.min.x) / tl.width()).clamp(0.0, 1.0) as f64 * d;
        // Snap to frames.
        v.playhead = (t * fps).round() / fps;
        v.player = None;
    }
    resp.on_hover_text(t!("editor.timeline_tip"));

    // Transport + In/Out.
    ui.horizontal_wrapped(|ui| {
        let step = 1.0 / fps;
        if ui.button("⏮").on_hover_text(t!("editor.to_in")).clicked() {
            v.playhead = v.in_t;
        }
        if ui.button("◀").on_hover_text(t!("editor.frame_back")).clicked() {
            v.playhead = (v.playhead - step).max(0.0);
        }
        ui.label(RichText::new(format_time(v.playhead)).monospace());
        if ui.button("▶").on_hover_text(t!("editor.frame_fwd")).clicked() {
            v.playhead = (v.playhead + step).min(d);
        }
        if ui.button("⏭").on_hover_text(t!("editor.to_out")).clicked() {
            v.playhead = v.out_t;
        }
        ui.separator();
        if ui.button(t!("editor.set_in")).on_hover_text(t!("editor.set_in_tip")).clicked() {
            v.in_t = v.playhead.min(v.out_t);
        }
        let out_t = v.out_t;
        time_field(ui, &mut v.in_txt, &mut v.in_t, 0.0, out_t);
        if ui.button(t!("editor.set_out")).on_hover_text(t!("editor.set_out_tip")).clicked() {
            v.out_t = v.playhead.max(v.in_t);
        }
        let in_t = v.in_t;
        time_field(ui, &mut v.out_txt, &mut v.out_t, in_t, d);
        ui.label(t!("editor.selection", len = format_time(v.out_t - v.in_t)));
        ui.separator();
        if v.player.is_some() {
            if ui.button(t!("editor.stop")).clicked() {
                v.player = None;
            }
        } else if v.loading_play {
            ui.spinner();
        } else if ui.button(t!("editor.play_sel")).on_hover_text(t!("editor.play_sel_tip", n = PLAY_MAX_SECS as u32)).clicked() {
            let dur = (v.out_t - v.in_t).clamp(0.1, PLAY_MAX_SECS);
            let (pw, ph) = fit(w, h, 960, 540);
            let _ = tx.send(Req::Play { seq: v.seq, path: v.info.path.clone(), start: v.in_t, dur, w: pw, h: ph, fps });
            v.loading_play = true;
        }
    });
    ui.horizontal_wrapped(|ui| {
        let mut aspect = app.editor.aspect;
        crop_controls(ui, &mut v.crop, &mut aspect, w, h);
        app.editor.aspect = aspect;
    });
    let v = app.editor.video.as_ref().expect("checked");
    let trimmed = v.in_t > 0.0005 || v.out_t < d - 0.0005;
    let crop = v.crop.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.label(t!("editor.trim_mode"));
        ui.add_enabled_ui(!crop, |ui| {
            ui.radio_value(&mut app.editor.trim_mode, TrimMode::Fast, t!("editor.fast")).on_hover_text(t!("editor.fast_tip"));
        })
        .response
        .on_disabled_hover_text(t!("editor.crop_reencodes"));
        ui.radio_value(&mut app.editor.trim_mode, TrimMode::Accurate, t!("editor.accurate")).on_hover_text(t!("editor.accurate_tip"));
        ui.separator();
        let can = (trimmed || crop) && !app.editor.busy();
        if ui
            .add_enabled(can, egui::Button::new(RichText::new(t!("editor.export")).strong()))
            .on_disabled_hover_text(t!("editor.nothing_to_export"))
            .clicked()
        {
            let v = app.editor.video.as_ref().expect("checked");
            let mut clip = Clip::new(v.info.clone());
            clip.trim = trimmed.then_some((v.in_t, v.out_t));
            let mode = if crop { TrimMode::Accurate } else { app.editor.trim_mode };
            let out = out_path(&v.info.path, &crate::editor::output_ext(&v.info.path, mode == TrimMode::Accurate));
            let edit = EditJob::Video { clip: Box::new(clip), crop: v.crop, mode, enc: encode_choice(app) };
            let info = v.info.clone();
            submit(app, info, edit, out, MediaKind::Video);
        }
        ui.label(RichText::new(t!("editor.out_hint")).small().weak());
    });
}

fn join_ui(app: &mut App, ui: &mut egui::Ui) {
    if app.editor.clips.is_empty() {
        drop_hint(ui, t!("editor.drop_join"));
        return;
    }
    let compatible = crate::editor::concat_compatible(&app.editor.clips.iter().map(|c| c.clip.clone()).collect::<Vec<_>>());
    if compatible {
        ui.colored_label(Color32::from_rgb(110, 200, 120), t!("editor.join_lossless"));
    } else {
        ui.colored_label(WARN, t!("editor.join_reencode")).on_hover_text(t!("editor.join_reencode_tip"));
    }
    let mut remove = None;
    let mut moved: Option<(usize, usize)> = None;
    egui::ScrollArea::vertical().max_height(ui.available_height() - 50.0).auto_shrink([false, true]).show(ui, |ui| {
        for (i, c) in app.editor.clips.iter_mut().enumerate() {
            let row_id = ui.scope_id().with(("join-row", c.key));
            let frame = egui::Frame::group(ui.style()).inner_margin(4.0);
            let (_, dropped) = ui.dnd_drop_zone::<usize, _>(frame, |ui| {
                ui.horizontal(|ui| {
                    ui.dnd_drag_source(row_id, i, |ui| {
                        ui.label(RichText::new("☰").size(18.0)).on_hover_text(t!("editor.drag_tip"));
                    });
                    let (tr, _) = ui.allocate_exact_size(Vec2::new(96.0, 54.0), Sense::hover());
                    ui.painter().rect_filled(tr, 2.0, Color32::from_gray(25));
                    if let Some(t) = &c.thumb {
                        ui.painter().image(t.id(), fit_rect(tr, t.size_vec2()), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                    ui.vertical(|ui| {
                        let name = c.clip.info.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        crate::ui::outputs::elided_label(ui, &name, (ui.available_width() - 40.0).max(80.0), |s| RichText::new(s).strong());
                        let i = &c.clip.info;
                        ui.label(RichText::new(format!(
                            "{}×{} · {} fps · {} · {}",
                            i.width,
                            i.height,
                            i.fps.map(|f| f.to_string()).unwrap_or_else(|| "?".into()),
                            i.video_codec.clone().unwrap_or_default(),
                            if i.audio_codecs.is_empty() { t!("editor.no_audio") } else { i.audio_codecs.join(",") }
                        )).small().weak());
                        ui.horizontal(|ui| {
                            let d = c.clip.duration();
                            let (mut a, mut b) = c.clip.span();
                            ui.label(t!("editor.in"));
                            time_field(ui, &mut c.in_txt, &mut a, 0.0, b);
                            ui.label(t!("editor.out"));
                            time_field(ui, &mut c.out_txt, &mut b, a, d);
                            let full = a <= 0.0005 && b >= d - 0.0005;
                            c.clip.trim = (!full).then_some((a, b));
                            ui.label(RichText::new(format_time(c.clip.len())).small());
                        });
                    });
                    if ui.small_button("✕").on_hover_text(t!("editor.remove")).clicked() {
                        remove = Some(i);
                    }
                });
            });
            if let Some(from) = dropped {
                moved = Some((*from, i));
            }
        }
    });
    if let Some(i) = remove {
        app.editor.clips.remove(i);
    }
    if let Some((from, to)) = moved
        && from != to
        && from < app.editor.clips.len()
    {
        let c = app.editor.clips.remove(from);
        app.editor.clips.insert(to.min(app.editor.clips.len()), c);
    }
    ui.horizontal_wrapped(|ui| {
        let total: f64 = app.editor.clips.iter().map(|c| c.clip.len()).sum();
        ui.label(t!("editor.join_total", n = app.editor.clips.len(), len = format_time(total)));
        let can = app.editor.clips.len() >= 2 && !app.editor.busy();
        if ui
            .add_enabled(can, egui::Button::new(RichText::new(t!("editor.export_join")).strong()))
            .on_disabled_hover_text(t!("editor.join_need_two"))
            .clicked()
        {
            let clips: Vec<Clip> = app.editor.clips.iter().map(|c| c.clip.clone()).collect();
            let first = clips[0].info.clone();
            let ext = crate::editor::output_ext(&first.path, !compatible);
            let out = out_path(&first.path, &ext).with_file_name(format!(
                "{}_joined.{ext}",
                first.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            ));
            let out = if out.exists() { out_path(&out, &ext) } else { out };
            let edit = EditJob::Join { clips, enc: encode_choice(app) };
            submit(app, first, edit, out, MediaKind::Video);
        }
        if ui.button(t!("editor.clear")).clicked() {
            app.editor.clips.clear();
        }
    });
}

fn image_ui(app: &mut App, ui: &mut egui::Ui) {
    let aspect = app.editor.aspect;
    let Some(im) = app.editor.image.as_mut() else {
        drop_hint(ui, t!("editor.drop_image"));
        return;
    };
    let (w, h) = (im.info.width, im.info.height);
    ui.horizontal_wrapped(|ui| {
        if ui.button("⟲ 90°").on_hover_text(t!("editor.rotate_left")).clicked() {
            im.rotate = (im.rotate + 3) % 4;
            im.crop = None;
        }
        if ui.button("⟳ 90°").on_hover_text(t!("editor.rotate_right")).clicked() {
            im.rotate = (im.rotate + 1) % 4;
            im.crop = None;
        }
        if ui.button("180°").clicked() {
            im.rotate = (im.rotate + 2) % 4;
            im.crop = None;
        }
        ui.toggle_value(&mut im.flip_h, t!("editor.flip_h"));
        ui.toggle_value(&mut im.flip_v, t!("editor.flip_v"));
        ui.separator();
        ui.label(RichText::new(format!("{w}×{h}")).weak());
    });
    // Crop works on the rotated/flipped image (what the user sees).
    let (rw, rh) = crate::editor::rotated_size(w, h, im.rotate);
    let mut a = aspect;
    crop_controls(ui, &mut im.crop, &mut a, rw, rh);
    let avail = ui.available_size();
    let (area, _) = ui.allocate_exact_size(Vec2::new(avail.x, (avail.y - 40.0).max(120.0)), Sense::hover());
    ui.painter().rect_filled(area, 4.0, Color32::from_gray(18));
    if let Some(t) = &im.tex {
        let r = fit_rect(area, Vec2::new(rw as f32, rh as f32));
        // Rotation/flip through UV + mesh corners.
        let uv = [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)];
        // Source corners TL, TR, BR, BL land on screen corners rotated clockwise.
        let mut corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
        corners.rotate_left(im.rotate as usize % 4);
        for p in corners.iter_mut() {
            if im.flip_h {
                p.x = r.min.x + r.max.x - p.x;
            }
            if im.flip_v {
                p.y = r.min.y + r.max.y - p.y;
            }
        }
        let mut mesh = egui::Mesh::with_texture(t.id());
        for (p, u) in corners.iter().zip(uv) {
            mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv: u, color: Color32::WHITE });
        }
        mesh.indices = vec![0, 1, 2, 0, 2, 3];
        ui.painter().add(egui::Shape::mesh(mesh));
        if let Some(c) = im.crop.as_mut() {
            crop_overlay(ui, ui.scope_id().with("icrop"), r, rw, rh, c, a.ratio(rw, rh));
        }
    }
    app.editor.aspect = a;
    ui.horizontal_wrapped(|ui| {
        let ed = &mut app.editor;
        ui.label(t!("settings.image_format"));
        ui.selectable_value(&mut ed.image_format, ImageFormat::Same, t!("editor.format_original"));
        for (v, l) in [(ImageFormat::Png, "PNG"), (ImageFormat::Jpeg, "JPG"), (ImageFormat::Webp, "WebP")] {
            ui.selectable_value(&mut ed.image_format, v, l);
        }
        if crate::pipeline::image_job::resolve_format(ed.image_format, &ed.image.as_ref().expect("checked").info.path) == ImageFormat::Jpeg {
            ui.add(egui::Slider::new(&mut ed.image_quality, 50..=100).text(t!("settings.jpg_quality")));
        }
        ui.separator();
        let im = ed.image.as_ref().expect("checked");
        let changed = im.crop.is_some_and(|c| !c.is_full(rw, rh)) || im.rotate != 0 || im.flip_h || im.flip_v || ed.image_format != ImageFormat::Same;
        if ui
            .add_enabled(changed && !ed.busy(), egui::Button::new(RichText::new(t!("editor.export")).strong()))
            .on_disabled_hover_text(t!("editor.nothing_to_export"))
            .clicked()
        {
            // The crop was drawn on the rotated view: apply rotation/flip first, then crop.
            let ops = ImageOps {
                crop: None,
                rotate: im.rotate,
                flip_h: im.flip_h,
                flip_v: im.flip_v,
                format: ed.image_format,
                quality: ed.image_quality,
            };
            let info = im.info.clone();
            let fmt = crate::pipeline::image_job::resolve_format(ops.format, &info.path);
            let out = out_path(&info.path, fmt.ext());
            let crop = im.crop.filter(|c| !c.is_full(rw, rh));
            let edit = EditJob::Image { input: info.path.clone(), ops: ImageOps { crop: None, ..ops.clone() } };
            let edit = match crop {
                None => edit,
                Some(c) => EditJob::Image { input: info.path.clone(), ops: ImageOps { crop: Some(c), ..ops } },
            };
            submit(app, info, edit, out, MediaKind::Image);
        }
        ui.label(RichText::new(t!("editor.meta_hint")).small().weak());
    });
}
