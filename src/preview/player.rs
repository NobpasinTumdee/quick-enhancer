//! Looping side-by-side playback of a processed sample clip (Original vs Enhanced), each
//! side at its own frame rate, with the segment's audio as the master clock.

use egui::{Align2, Color32, FontId, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use image::RgbImage;
use std::collections::VecDeque;
use std::time::Instant;

use super::audio::AudioOut;
use crate::media::ffmpeg::AudioClip;
use crate::t;

/// Which frame of a `fps` stream with `n_frames` frames to show at `clock` seconds.
/// Frames are held (repeated) or skipped (dropped) as needed to follow the clock.
pub fn frame_index(clock: f64, fps: f64, n_frames: usize) -> usize {
    if n_frames == 0 || fps <= 0.0 || !clock.is_finite() {
        return 0;
    }
    (((clock.max(0.0)) * fps + 1e-6).floor() as usize).min(n_frames - 1)
}

/// Clock position for a free-running timer (used when there is no audio).
pub fn timer_clock(elapsed: f64, duration: f64, looping: bool) -> f64 {
    let d = duration.max(1e-3);
    if looping { elapsed.max(0.0) % d } else { elapsed.clamp(0.0, d) }
}

/// Persisted player preferences (live in the app config).
pub struct PlayerPrefs<'a> {
    pub volume: &'a mut f32,
    pub muted: &'a mut bool,
    pub looping: &'a mut bool,
}

struct Side {
    frames: Vec<RgbImage>,
    fps: f64,
    size: (u32, u32),
    tex: Option<(TextureHandle, usize)>,
    /// Times at which a new frame was shown (for the measured display fps).
    shown: VecDeque<Instant>,
}

impl Side {
    fn new(frames: Vec<RgbImage>, fps: f64, size: (u32, u32)) -> Side {
        Side {
            frames,
            fps: fps.max(1.0),
            size,
            tex: None,
            shown: VecDeque::new(),
        }
    }

    fn duration(&self) -> f64 {
        self.frames.len() as f64 / self.fps
    }

    fn show(&mut self, ctx: &egui::Context, idx: usize, name: &str) {
        if self.frames.is_empty() {
            return;
        }
        let idx = idx.min(self.frames.len() - 1);
        if self.tex.as_ref().is_some_and(|(_, i)| *i == idx) {
            return;
        }
        let img = super::to_color_image(&self.frames[idx]);
        match &mut self.tex {
            Some((h, i)) => {
                h.set(img, TextureOptions::LINEAR);
                *i = idx;
            }
            None => self.tex = Some((ctx.load_texture(name, img, TextureOptions::LINEAR), idx)),
        }
        let now = Instant::now();
        self.shown.push_back(now);
        while self
            .shown
            .front()
            .is_some_and(|t| now.duration_since(*t).as_secs_f64() > 1.0)
        {
            self.shown.pop_front();
        }
    }

    fn measured_fps(&self) -> f64 {
        self.shown.len() as f64
    }
}

pub struct SamplePlayer {
    orig: Side,
    enh: Side,
    audio: Option<AudioOut>,
    audio_note: String,
    /// Timer clock (no audio or ¼ speed).
    started: Instant,
    paused_at: Option<f64>,
    pub note: String,
    pub slow: bool,
    /// Show only the "enhanced" side, full size, without labels (Editor playback).
    pub single: bool,
    refresh_hz: Option<u32>,
}

impl SamplePlayer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        original: Vec<RgbImage>,
        enhanced: Vec<RgbImage>,
        src_fps: f64,
        out_fps: f64,
        src_size: (u32, u32),
        out_size: (u32, u32),
        audio: Option<AudioClip>,
        note: String,
        prefs: &PlayerPrefs,
    ) -> Self {
        let (audio, audio_note) = match audio {
            None => (None, t!("player.no_audio_track")),
            Some(clip) => match AudioOut::open(clip, *prefs.looping) {
                Ok(a) => {
                    a.set_volume(*prefs.volume, *prefs.muted);
                    a.play();
                    (Some(a), String::new())
                }
                Err(e) => {
                    tracing::warn!("audio output unavailable: {e}");
                    (None, t!("player.no_audio_device"))
                }
            },
        };
        Self {
            orig: Side::new(original, src_fps, src_size),
            enh: Side::new(enhanced, out_fps, out_size),
            audio,
            audio_note,
            started: Instant::now(),
            paused_at: None,
            note,
            slow: false,
            single: false,
            refresh_hz: refresh_rate_hz(),
        }
    }

    fn duration(&self) -> f64 {
        match &self.audio {
            Some(a) if a.duration() > 0.05 => a.duration(),
            _ => self.orig.duration().max(self.enh.duration()).max(0.1),
        }
    }

    fn audio_drives(&self) -> bool {
        self.audio.is_some() && !self.slow
    }

    fn playing(&self) -> bool {
        self.paused_at.is_none()
    }

    fn clock(&self, looping: bool) -> f64 {
        if self.audio_drives() {
            return self.audio.as_ref().map(AudioOut::clock).unwrap_or(0.0);
        }
        let speed = if self.slow { 0.25 } else { 1.0 };
        let t = self
            .paused_at
            .unwrap_or_else(|| self.started.elapsed().as_secs_f64() * speed);
        timer_clock(t, self.duration(), looping)
    }

    fn set_timer(&mut self, t: f64) {
        let speed = if self.slow { 0.25 } else { 1.0 };
        self.started = Instant::now() - std::time::Duration::from_secs_f64((t / speed).max(0.0));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, prefs: PlayerPrefs) {
        let looping = *prefs.looping;
        ui.horizontal_wrapped(|ui| {
            let playing = self.playing();
            if ui
                .button(if playing { t!("player.pause") } else { t!("player.play") })
                .clicked()
            {
                let now = self.clock(looping);
                if playing {
                    self.paused_at = Some(now);
                    if let Some(a) = &self.audio {
                        a.pause();
                    }
                } else {
                    self.paused_at = None;
                    self.set_timer(now);
                    if self.audio_drives()
                        && let Some(a) = &self.audio
                    {
                        a.play();
                    }
                }
            }
            if ui.button(t!("player.restart")).clicked() {
                self.set_timer(0.0);
                if let Some(a) = &self.audio {
                    a.restart();
                }
            }
            if ui.checkbox(prefs.looping, t!("player.loop")).changed()
                && let Some(a) = &self.audio
            {
                a.set_looping(*prefs.looping);
            }
            if ui
                .checkbox(&mut self.slow, t!("player.quarter_speed"))
                .on_hover_text(t!("player.quarter_speed_tip"))
                .changed()
            {
                let now = self.clock(looping);
                self.set_timer(now);
                if let Some(a) = &self.audio {
                    if self.slow || !self.playing() {
                        a.pause();
                    } else {
                        a.play();
                    }
                }
            }
            ui.separator();
            if self.audio.is_some() {
                let icon = if *prefs.muted { "🔇" } else { "🔊" };
                if ui
                    .button(icon)
                    .on_hover_text(if *prefs.muted { t!("player.unmute") } else { t!("player.mute") })
                    .clicked()
                {
                    *prefs.muted = !*prefs.muted;
                }
                let r = ui.add(
                    egui::Slider::new(prefs.volume, 0.0..=1.0)
                        .show_value(false)
                        .text(t!("player.volume")),
                );
                if r.changed() {
                    *prefs.muted = false;
                }
                if let Some(a) = &self.audio {
                    a.set_volume(*prefs.volume, *prefs.muted);
                }
            } else {
                ui.label(egui::RichText::new(&self.audio_note).weak());
            }
        });
        ui.label(egui::RichText::new(&self.note).small().weak());
        if let Some(hz) = self.refresh_hz
            && self.enh.fps > hz as f64 + 0.5
        {
            ui.colored_label(
                Color32::from_rgb(230, 170, 60),
                t!("player.refresh_notice", fps = format!("{:.0}", self.enh.fps), hz = hz),
            );
        }

        let clock = self.clock(looping);
        // Non-looping, timer-driven playback stops at the end.
        if !looping && !self.audio_drives() && clock >= self.duration() && self.playing() {
            self.paused_at = Some(self.duration());
        }
        let io = frame_index(clock, self.orig.fps, self.orig.frames.len());
        let ie = frame_index(clock, self.enh.fps, self.enh.frames.len());
        let ctx = ui.ctx().clone();
        self.orig.show(&ctx, io, "sample-orig");
        self.enh.show(&ctx, ie, "sample-enh");

        let avail = ui.available_size();
        let (rect, _) = ui.allocate_exact_size(avail, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(18));
        let half = Vec2::new(rect.width() / 2.0 - 2.0, rect.height());
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        if self.single {
            if let Some((h, _)) = &self.enh.tex {
                let s = h.size_vec2();
                let scale = (rect.width() / s.x).min(rect.height() / s.y);
                painter.image(h.id(), Rect::from_center_size(rect.center(), s * scale), uv, Color32::WHITE);
            }
            painter.text(
                rect.center_bottom() + Vec2::new(0.0, -8.0),
                Align2::CENTER_BOTTOM,
                format!("{clock:.2}s / {:.2}s", self.duration()),
                FontId::monospace(12.0),
                Color32::LIGHT_GRAY,
            );
            if self.playing() {
                ui.ctx().request_repaint();
            }
            return;
        }
        let sides = [
            (&self.orig, t!("preview.original")),
            (&self.enh, t!("preview.enhanced")),
        ];
        for (i, (side, label)) in sides.iter().enumerate() {
            let cell =
                Rect::from_min_size(rect.min + Vec2::new(i as f32 * (half.x + 4.0), 0.0), half);
            if let Some((h, _)) = &side.tex {
                let s = h.size_vec2();
                let scale = (cell.width() / s.x).min(cell.height() / s.y);
                let r = Rect::from_center_size(cell.center(), s * scale);
                painter.image(h.id(), r, uv, Color32::WHITE);
            }
            let text = t!(
                "player.side_label",
                name = label,
                fps = format_fps(side.fps),
                w = side.size.0,
                h = side.size.1
            );
            overlay(&painter, cell.left_top() + Vec2::new(8.0, 8.0), Align2::LEFT_TOP, &text, 14.0);
            let shown = t!("player.displayed_fps", fps = format!("{:.0}", side.measured_fps()));
            overlay(&painter, cell.left_top() + Vec2::new(8.0, 32.0), Align2::LEFT_TOP, &shown, 12.0);
        }
        painter.text(
            rect.center_bottom() + Vec2::new(0.0, -8.0),
            Align2::CENTER_BOTTOM,
            format!("{clock:.2}s / {:.2}s", self.duration()),
            FontId::monospace(12.0),
            Color32::LIGHT_GRAY,
        );
        if self.playing() {
            ui.ctx().request_repaint();
        }
    }
}

fn format_fps(f: f64) -> String {
    if (f - f.round()).abs() < 0.01 {
        format!("{f:.0}")
    } else {
        format!("{f:.2}")
    }
}

/// Text with a dark backing box, readable on any image.
pub fn overlay(p: &egui::Painter, pos: Pos2, align: Align2, text: &str, size: f32) {
    let font = FontId::proportional(size);
    let galley = p.layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let r = align.anchor_size(pos, galley.size());
    p.rect_filled(r.expand(4.0), 4.0, Color32::from_black_alpha(160));
    p.galley(r.min, galley, Color32::WHITE);
}

/// Refresh rate of the primary display (Windows), cached.
pub fn refresh_rate_hz() -> Option<u32> {
    use std::sync::OnceLock;
    static HZ: OnceLock<Option<u32>> = OnceLock::new();
    *HZ.get_or_init(|| {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
            let mut dm = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            if EnumDisplaySettingsW(windows::core::PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut dm)
                .as_bool()
                && dm.dmDisplayFrequency > 1
            {
                return Some(dm.dmDisplayFrequency);
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_follow_the_clock() {
        // 30 fps original, 60 fps enhanced, 2 s clip.
        assert_eq!(frame_index(0.0, 30.0, 60), 0);
        assert_eq!(frame_index(0.5, 30.0, 60), 15);
        assert_eq!(frame_index(0.5, 60.0, 120), 30);
        // Exactly on a frame boundary (float noise must not drop a frame).
        assert_eq!(frame_index(1.0 / 3.0, 30.0, 60), 10);
        // Past the end → hold the last frame; negative / NaN → first.
        assert_eq!(frame_index(5.0, 30.0, 60), 59);
        assert_eq!(frame_index(-1.0, 30.0, 60), 0);
        assert_eq!(frame_index(f64::NAN, 30.0, 60), 0);
        assert_eq!(frame_index(1.0, 30.0, 0), 0);
        // Audio clock jumps ahead (UI stalled) → frames are skipped, not queued.
        assert_eq!(frame_index(1.9, 60.0, 120), 114);
    }

    #[test]
    fn timer_clock_loops_or_clamps() {
        assert!((timer_clock(2.5, 2.0, true) - 0.5).abs() < 1e-9);
        assert_eq!(timer_clock(2.5, 2.0, false), 2.0);
        assert_eq!(timer_clock(-1.0, 2.0, true), 0.0);
    }
}
