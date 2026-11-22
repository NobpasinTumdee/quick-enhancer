//! Persisted settings (`config.toml` next to the executable).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::media::encoders::VideoEncoder;

/// Content-type profile. Each maps to a concrete engine/model (see `engines::choose_backend`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Profile {
    #[default]
    Realistic,
    Anime,
    PixelArt,
    General,
    /// No AI at all: denoise / Lanczos resize / sharpen only.
    NoAi,
}

impl Profile {
    pub const ALL: [Profile; 5] = [
        Profile::Realistic,
        Profile::Anime,
        Profile::PixelArt,
        Profile::General,
        Profile::NoAi,
    ];
    pub fn label(self) -> String {
        match self {
            Profile::Realistic => crate::t!("profile.realistic"),
            Profile::Anime => crate::t!("profile.anime"),
            Profile::PixelArt => crate::t!("profile.pixel_art"),
            Profile::General => crate::t!("profile.general"),
            Profile::NoAi => crate::t!("profile.no_ai"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScaleMode {
    X1,
    #[default]
    X2,
    X3,
    X4,
    /// Target short side in pixels (720 / 1080 / 1440 / 2160).
    Target(u32),
}

impl ScaleMode {
    pub const ALL: [ScaleMode; 8] = [
        ScaleMode::X1,
        ScaleMode::X2,
        ScaleMode::X3,
        ScaleMode::X4,
        ScaleMode::Target(720),
        ScaleMode::Target(1080),
        ScaleMode::Target(1440),
        ScaleMode::Target(2160),
    ];
    pub fn label(self) -> String {
        match self {
            ScaleMode::X1 => crate::t!("scale.x1"),
            ScaleMode::X2 => "2x".into(),
            ScaleMode::X3 => "3x".into(),
            ScaleMode::X4 => "4x".into(),
            ScaleMode::Target(2160) => crate::t!("scale.target_4k"),
            ScaleMode::Target(h) => crate::t!("scale.target", h = h),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FrameGenMode {
    #[default]
    Off,
    X2,
    X3,
    X4,
    TargetFps(u32),
}

impl FrameGenMode {
    pub fn label(self) -> String {
        match self {
            FrameGenMode::Off => crate::t!("common.off"),
            FrameGenMode::X2 => "2x".into(),
            FrameGenMode::X3 => "3x".into(),
            FrameGenMode::X4 => "4x".into(),
            FrameGenMode::TargetFps(f) => crate::t!("framegen.target_n", fps = f),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SharpenMode {
    Off,
    #[default]
    Cas,
    Unsharp,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Preset {
    Eco,
    #[default]
    Balanced,
    Max,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Eco, Preset::Balanced, Preset::Max];
    pub fn label(self) -> String {
        match self {
            Preset::Eco => crate::t!("preset.eco"),
            Preset::Balanced => crate::t!("preset.balanced"),
            Preset::Max => crate::t!("preset.max"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Priority {
    BelowNormal,
    #[default]
    Normal,
    AboveNormal,
}

impl Priority {
    pub fn label(self) -> String {
        match self {
            Priority::BelowNormal => crate::t!("priority.below_normal"),
            Priority::Normal => crate::t!("priority.normal"),
            Priority::AboveNormal => crate::t!("priority.above_normal"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ComputeDevice {
    #[default]
    Auto,
    /// Vulkan physical-device index (same as ncnn `-g`).
    Gpu(u32),
    Cpu,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TileSize {
    /// Derived from free VRAM.
    #[default]
    Auto,
    Fixed(u32),
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EncoderChoice {
    #[default]
    Auto,
    Specific(VideoEncoder),
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
    /// Keep the source format when it is one of the above (else PNG).
    Same,
}

impl ImageFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Png | ImageFormat::Same => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Container {
    #[default]
    Mp4,
    Mkv,
}

impl Container {
    pub fn ext(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
        }
    }
    pub fn ffmpeg_format(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "matroska",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OverwritePolicy {
    #[default]
    Ask,
    Skip,
    AutoRename,
    Overwrite,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct OutputSettings {
    /// `None` = next to the source file.
    pub folder: Option<PathBuf>,
    pub overwrite: OverwritePolicy,
    pub image_format: ImageFormat,
    pub image_quality: u8,
    pub container: Container,
    pub encoder: EncoderChoice,
    /// CQ / CRF / QP value. `None` = per-encoder default.
    pub quality: Option<u32>,
    pub keep_partial: bool,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            folder: None,
            overwrite: OverwritePolicy::Ask,
            image_format: ImageFormat::Png,
            image_quality: 92,
            container: Container::Mp4,
            encoder: EncoderChoice::Auto,
            quality: None,
            keep_partial: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct VideoSettings {
    pub chunk_frames: u32,
    pub ram_budget_mb: u32,
    /// `None` = system temp dir.
    pub temp_dir: Option<PathBuf>,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            chunk_frames: 120,
            ram_budget_mb: 2048,
            temp_dir: None,
        }
    }
}

/// Everything that determines how one queue item is processed.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct JobSettings {
    /// Section switches (the settings below are kept when a section is off).
    pub upscale_on: bool,
    pub framegen_on: bool,
    pub filters_on: bool,
    pub profile: Profile,
    pub scale: ScaleMode,
    pub framegen: FrameGenMode,
    pub max_fps: u32,
    /// RIFE model directory name inside `models/rife/`.
    pub rife_model: String,
    pub scene_detect: bool,
    pub scene_threshold: f32,

    pub denoise: f32,
    pub sharpen: SharpenMode,
    pub cas_strength: f32,
    pub unsharp_radius: f32,
    pub unsharp_amount: f32,
    pub unsharp_threshold: u8,

    pub compute: ComputeDevice,
    pub cpu_threads: u32,
    pub preset: Preset,
    pub tile_override: Option<TileSize>,
    pub jobs_override: Option<String>,
    pub filter_threads_override: Option<u32>,
    pub priority_override: Option<Priority>,

    pub output: OutputSettings,
    pub video: VideoSettings,
}

impl JobSettings {
    /// The settings the pipeline actually runs with: sections that are switched off are
    /// neutralised (upscaling off → No AI at 1x, frame gen off → Off, filters off → no
    /// denoise / sharpen). Everything downstream (export, preview, estimates) uses this.
    pub fn effective(&self) -> JobSettings {
        let mut s = self.clone();
        if !s.upscale_on {
            s.profile = Profile::NoAi;
            s.scale = ScaleMode::X1;
        }
        if !s.framegen_on {
            s.framegen = FrameGenMode::Off;
        }
        if !s.filters_on {
            s.denoise = 0.0;
            s.sharpen = SharpenMode::Off;
        }
        s
    }
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            upscale_on: true,
            framegen_on: true,
            filters_on: true,
            profile: Profile::Realistic,
            scale: ScaleMode::X2,
            framegen: FrameGenMode::Off,
            max_fps: 120,
            rife_model: "rife-v4.6".into(),
            scene_detect: true,
            scene_threshold: 0.30,
            denoise: 0.0,
            sharpen: SharpenMode::Off,
            cas_strength: 0.4,
            unsharp_radius: 1.2,
            unsharp_amount: 0.6,
            unsharp_threshold: 2,
            compute: ComputeDevice::Auto,
            cpu_threads: 0,
            preset: Preset::Balanced,
            tile_override: None,
            jobs_override: None,
            filter_threads_override: None,
            priority_override: None,
            output: OutputSettings::default(),
            video: VideoSettings::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    pub settings: JobSettings,
    pub recursive: bool,
    pub auto_preview: bool,
    pub sample_seconds: f32,
    pub show_log: bool,
    /// UI language; `None` until first run picks one from the Windows UI language.
    pub language: Option<crate::i18n::Lang>,
    /// Sample-player audio.
    pub preview_volume: f32,
    pub preview_muted: bool,
    pub preview_loop: bool,
    /// Opt-in update check on startup (asked once on first run).
    pub auto_update_check: bool,
    pub update_check_asked: bool,
    /// Outputs page view options.
    pub outputs: OutputsView,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OutputsSort {
    #[default]
    DateDesc,
    DateAsc,
    NameAsc,
    SizeDesc,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OutputsFilter {
    #[default]
    All,
    Images,
    Videos,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct OutputsView {
    pub grid: bool,
    pub sort: OutputsSort,
    pub filter: OutputsFilter,
}

impl Default for OutputsView {
    fn default() -> Self {
        Self {
            grid: true,
            sort: OutputsSort::DateDesc,
            filter: OutputsFilter::All,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            settings: JobSettings::default(),
            recursive: true,
            auto_preview: true,
            sample_seconds: 3.0,
            show_log: true,
            language: None,
            preview_volume: 0.8,
            preview_muted: false,
            preview_loop: true,
            auto_update_check: false,
            update_check_asked: false,
            outputs: OutputsView::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("config.toml is invalid ({e}); using defaults");
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write-then-rename so a crash never leaves a truncated config.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let mut c = Config::default();
        c.settings.compute = ComputeDevice::Gpu(1);
        c.settings.scale = ScaleMode::Target(1080);
        c.settings.tile_override = Some(TileSize::Fixed(256));
        c.settings.output.encoder = EncoderChoice::Specific(VideoEncoder::HevcNvenc);
        c.settings.framegen = FrameGenMode::TargetFps(60);
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let back: Config = toml::from_str("recursive = false\n").unwrap();
        assert!(!back.recursive);
        assert_eq!(back.settings, JobSettings::default());
    }
}

#[cfg(test)]
mod switch_tests {
    use super::*;
    #[test]
    fn effective_respects_section_switches() {
        let mut s = JobSettings {
            framegen: FrameGenMode::X2,
            denoise: 0.5,
            sharpen: SharpenMode::Cas,
            ..Default::default()
        };
        s.upscale_on = false;
        let e = s.effective();
        assert_eq!((e.profile, e.scale), (Profile::NoAi, ScaleMode::X1));
        assert_eq!(e.framegen, FrameGenMode::X2);
        assert_eq!(e.sharpen, SharpenMode::Cas);
        s.upscale_on = true;
        s.framegen_on = false;
        s.filters_on = false;
        let e = s.effective();
        assert_eq!(e.profile, Profile::Realistic);
        assert_eq!(e.framegen, FrameGenMode::Off);
        assert_eq!((e.denoise, e.sharpen), (0.0, SharpenMode::Off));
        // The stored values are untouched, so switching back on restores them.
        assert_eq!(s.framegen, FrameGenMode::X2);
    }

    #[test]
    fn old_config_without_switches_defaults_on() {
        let s: JobSettings = toml::from_str("denoise = 0.2").unwrap();
        assert!(s.upscale_on && s.framegen_on && s.filters_on);
    }
}
