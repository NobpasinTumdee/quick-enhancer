//! Audio for the sample player. A custom looping rodio source counts the samples it hands
//! to the device; that count is the **master clock** both video sides follow. Any failure
//! (no device, no audio stream) leaves the player in "No audio" mode — never a crash.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::media::ffmpeg::AudioClip;

/// Shared state between the audio callback and the UI.
struct Shared {
    /// Interleaved-sample position inside the clip.
    pos: AtomicU64,
    looping: AtomicBool,
    ended: AtomicBool,
}

struct LoopSource {
    clip: AudioClip,
    shared: Arc<Shared>,
}

impl Iterator for LoopSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let len = self.clip.samples.len() as u64;
        if len == 0 {
            return None;
        }
        let mut p = self.shared.pos.load(Ordering::Relaxed);
        if p >= len {
            if self.shared.looping.load(Ordering::Relaxed) {
                p = 0;
            } else {
                self.shared.ended.store(true, Ordering::Relaxed);
                // Keep the stream alive with silence so the player can be restarted.
                return Some(0.0);
            }
        }
        self.shared.pos.store(p + 1, Ordering::Relaxed);
        Some(self.clip.samples[p as usize])
    }
}

impl rodio::Source for LoopSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(self.clip.channels.max(1)).expect("non-zero")
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(self.clip.sample_rate.max(1)).expect("non-zero")
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub struct AudioOut {
    _sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    shared: Arc<Shared>,
    channels: u64,
    rate: f64,
    duration: f64,
}

impl AudioOut {
    /// Open the default output device and start (paused) playback of `clip`.
    pub fn open(clip: AudioClip, looping: bool) -> Result<AudioOut, String> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        sink.log_on_drop(false);
        let player = rodio::Player::connect_new(sink.mixer());
        player.pause();
        let shared = Arc::new(Shared {
            pos: AtomicU64::new(0),
            looping: AtomicBool::new(looping),
            ended: AtomicBool::new(false),
        });
        let channels = clip.channels.max(1) as u64;
        let rate = clip.sample_rate.max(1) as f64;
        let duration = clip.duration();
        player.append(LoopSource {
            clip,
            shared: shared.clone(),
        });
        Ok(AudioOut {
            _sink: sink,
            player,
            shared,
            channels,
            rate,
            duration,
        })
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Current audio position in seconds (0..duration).
    pub fn clock(&self) -> f64 {
        let p = self.shared.pos.load(Ordering::Relaxed) / self.channels;
        (p as f64 / self.rate).min(self.duration)
    }

    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    pub fn play(&self) {
        if self.ended() {
            self.restart();
        }
        self.player.play();
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn restart(&self) {
        self.shared.pos.store(0, Ordering::Relaxed);
        self.shared.ended.store(false, Ordering::Relaxed);
    }

    pub fn set_looping(&self, on: bool) {
        self.shared.looping.store(on, Ordering::Relaxed);
    }

    /// `volume` 0..1; muted = 0.
    pub fn set_volume(&self, volume: f32, muted: bool) {
        self.player
            .set_volume(if muted { 0.0 } else { volume.clamp(0.0, 1.0) });
    }
}
