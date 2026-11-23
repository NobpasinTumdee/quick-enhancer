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

/// Encoder speed / quality trade-off (mapped to each encoder's preset).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EncoderSpeed {
    Fast,
    #[default]
    Balanced,
    Quality,
}

impl EncoderSpeed {
    pub fn label(self) -> String {
        match self {
            EncoderSpeed::Fast => crate::t!("perf.enc_fast"),
            EncoderSpeed::Balanced => crate::t!("perf.enc_balanced"),
            EncoderSpeed::Quality => crate::t!("perf.enc_quality"),
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
    pub encoder_speed: EncoderSpeed,
    /// `Some` = average-bitrate mode (kbit/s) instead of constant quality.
    pub bitrate_kbps: Option<u32>,
    pub keep_partial: bool,
    /// Appended to the source file name (`photo` → `photo_enhanced.png`).
    pub suffix: String,
}

impl OutputSettings {
    /// Suffix that is safe in a file name (default when empty).
    pub fn safe_suffix(&self) -> String {
        let s: String = self
            .suffix
            .chars()
            .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control())
            .collect();
        if s.trim().is_empty() { "_enhanced".into() } else { s }
    }
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
            encoder_speed: EncoderSpeed::Balanced,
            bitrate_kbps: None,
            keep_partial: false,
            suffix: "_enhanced".into(),
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
    /// Per-stage switches used by the Video / Image pages (All-in-one keeps them on and
    /// uses `filters_on` for both).
    pub denoise_on: bool,
    pub sharpen_on: bool,
    pub profile: Profile,
    /// Upscale model: catalog id or `custom:<name>`. `None` = the profile's default model.
    /// Only used by the AI profiles (not Pixel art / No AI).
    pub model: Option<String>,
    /// 1x restoration before upscaling (JPEG cleanup / denoise), catalog id.
    pub restore_on: bool,
    pub restore_model: String,
    /// Face restoration after upscaling, catalog id; `face_strength` 0..1 blends with the
    /// upscaled face (and is CodeFormer's fidelity weight).
    pub face_on: bool,
    pub face_model: String,
    pub face_strength: f32,
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
    /// Local contrast ("clarity") strength 0..=1, applied after sharpening (0 = off).
    pub clarity: f32,

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
            s.restore_on = false;
            s.face_on = false;
        }
        if matches!(s.profile, Profile::PixelArt | Profile::NoAi) {
            s.model = None;
        }
        if !s.framegen_on {
            s.framegen = FrameGenMode::Off;
        }
        if !s.filters_on {
            s.denoise = 0.0;
            s.sharpen = SharpenMode::Off;
            s.clarity = 0.0;
        }
        if !s.denoise_on {
            s.denoise = 0.0;
        }
        if !s.sharpen_on {
            s.sharpen = SharpenMode::Off;
            s.clarity = 0.0;
        }
        s
    }

    /// The hardware-dependent part of these settings.
    pub fn machine(&self) -> MachineSettings {
        MachineSettings {
            compute: self.compute,
            cpu_threads: self.cpu_threads,
            preset: self.preset,
            tile_override: self.tile_override,
            jobs_override: self.jobs_override.clone(),
            filter_threads_override: self.filter_threads_override,
            priority_override: self.priority_override,
            chunk_frames: self.video.chunk_frames,
            ram_budget_mb: self.video.ram_budget_mb,
            temp_dir: self.video.temp_dir.clone(),
        }
    }

    /// These settings with the machine (performance) values replaced by `m`.
    pub fn with_machine(&self, m: &MachineSettings) -> JobSettings {
        let mut s = self.clone();
        s.compute = m.compute;
        s.cpu_threads = m.cpu_threads;
        s.preset = m.preset;
        s.tile_override = m.tile_override;
        s.jobs_override = m.jobs_override.clone();
        s.filter_threads_override = m.filter_threads_override;
        s.priority_override = m.priority_override;
        s.video = VideoSettings {
            chunk_frames: m.chunk_frames,
            ram_budget_mb: m.ram_budget_mb,
            temp_dir: m.temp_dir.clone(),
        };
        s
    }

    /// The processing recipe only: machine values, output folder and overwrite policy are
    /// normalised (they are not part of a profile and don't make it "modified").
    pub fn recipe(&self) -> JobSettings {
        let d = JobSettings::default();
        let mut r = self.with_machine(&d.machine());
        r.output.folder = None;
        r.output.overwrite = d.output.overwrite;
        r.output.keep_partial = d.output.keep_partial;
        r
    }

    /// Apply a recipe, keeping this page's machine values, output folder and overwrite policy.
    pub fn apply_recipe(&mut self, recipe: &JobSettings) {
        let keep_folder = self.output.folder.clone();
        let keep_ow = self.output.overwrite;
        let keep_partial = self.output.keep_partial;
        let m = self.machine();
        *self = recipe.with_machine(&m);
        self.output.folder = keep_folder;
        self.output.overwrite = keep_ow;
        self.output.keep_partial = keep_partial;
    }

    /// Defaults for the Video page: frame generation 2x + 2x upscale.
    pub fn video_defaults() -> JobSettings {
        JobSettings {
            framegen: FrameGenMode::X2,
            ..Default::default()
        }
    }

    /// Defaults for the Image page: 2x Photo, light CAS.
    pub fn image_defaults() -> JobSettings {
        JobSettings {
            framegen_on: false,
            sharpen: SharpenMode::Cas,
            cas_strength: 0.3,
            denoise: 0.1,
            ..Default::default()
        }
    }
}

/// Hardware-dependent settings shared by every page (not part of a profile recipe).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct MachineSettings {
    pub compute: ComputeDevice,
    pub cpu_threads: u32,
    pub preset: Preset,
    pub tile_override: Option<TileSize>,
    pub jobs_override: Option<String>,
    pub filter_threads_override: Option<u32>,
    pub priority_override: Option<Priority>,
    pub chunk_frames: u32,
    pub ram_budget_mb: u32,
    /// `None` = system temp dir.
    pub temp_dir: Option<PathBuf>,
}

impl Default for MachineSettings {
    fn default() -> Self {
        JobSettings::default().machine()
    }
}

/// A page that owns its own inputs + settings (they all share one job runner).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum Studio {
    #[default]
    All,
    Video,
    Image,
}

impl Studio {
    pub const ALL: [Studio; 3] = [Studio::All, Studio::Video, Studio::Image];
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn label(self) -> String {
        match self {
            Studio::All => crate::t!("tabs.all_in_one"),
            Studio::Video => crate::t!("tabs.video"),
            Studio::Image => crate::t!("tabs.image"),
        }
    }
    /// Whether this page takes files of `kind`.
    pub fn accepts(self, kind: crate::media::MediaKind) -> bool {
        use crate::media::MediaKind;
        match self {
            Studio::All => true,
            Studio::Video => kind == MediaKind::Video,
            Studio::Image => kind == MediaKind::Image,
        }
    }
}

/// Per-page UI state that is remembered (profiles, Simple/Advanced).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct PageMeta {
    pub advanced: bool,
    /// Profile key last loaded on this page (`builtin:<id>` / `user:<name>`).
    pub last_profile: Option<String>,
    pub default_profile: Option<String>,
    /// Most recent first, at most 5.
    pub recent: Vec<String>,
}

impl PageMeta {
    pub fn touch_recent(&mut self, key: &str) {
        self.recent.retain(|k| k != key);
        self.recent.insert(0, key.to_string());
        self.recent.truncate(5);
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Pages {
    pub all: PageMeta,
    pub video: PageMeta,
    pub image: PageMeta,
}

impl Pages {
    pub fn get(&self, s: Studio) -> &PageMeta {
        match s {
            Studio::All => &self.all,
            Studio::Video => &self.video,
            Studio::Image => &self.image,
        }
    }
    pub fn get_mut(&mut self, s: Studio) -> &mut PageMeta {
        match s {
            Studio::All => &mut self.all,
            Studio::Video => &mut self.video,
            Studio::Image => &mut self.image,
        }
    }
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            upscale_on: true,
            framegen_on: true,
            filters_on: true,
            denoise_on: true,
            sharpen_on: true,
            profile: Profile::Realistic,
            scale: ScaleMode::X2,
            framegen: FrameGenMode::Off,
            max_fps: 120,
            rife_model: "rife-v4.26".into(),
            model: None,
            restore_on: false,
            restore_model: "dejpg-1x".into(),
            face_on: false,
            face_model: "gfpgan-1.4".into(),
            face_strength: 0.7,
            scene_detect: true,
            scene_threshold: 0.30,
            denoise: 0.0,
            sharpen: SharpenMode::Off,
            cas_strength: 0.4,
            unsharp_radius: 1.2,
            unsharp_amount: 0.6,
            unsharp_threshold: 2,
            clarity: 0.0,
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
    /// All-in-one page settings (kept under the 2.0 key for compatibility).
    pub settings: JobSettings,
    pub video_settings: JobSettings,
    pub image_settings: JobSettings,
    /// Hardware-dependent values shared by all pages.
    pub machine: MachineSettings,
    pub pages: Pages,
    /// Settings page: show every performance control.
    pub perf_advanced: bool,
    /// Benchmark was offered once after the first tools install.
    pub bench_offered: bool,
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
    /// The first-run setup wizard was completed or skipped.
    pub wizard_done: bool,
    /// ONNX Runtime used for ONNX models (one per process; switching needs a restart).
    pub onnx_runtime: crate::models::RuntimeKind,
    /// Model ids whose non-commercial license the user accepted.
    pub accepted_licenses: Vec<String>,
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
            video_settings: JobSettings::video_defaults(),
            image_settings: JobSettings::image_defaults(),
            machine: MachineSettings::default(),
            pages: Pages::default(),
            perf_advanced: false,
            bench_offered: false,
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
            wizard_done: false,
            onnx_runtime: crate::models::RuntimeKind::DirectMl,
            accepted_licenses: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => match Config::parse(&text) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("config.toml is invalid ({e}); using defaults");
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    /// Parse `config.toml`, migrating 2.0 files (no `[machine]` table: the performance
    /// values are taken from the old `[settings]`).
    pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
        let mut c: Config = toml::from_str(text)?;
        let has_machine = text
            .parse::<toml::Table>()
            .map(|t| t.contains_key("machine"))
            .unwrap_or(true);
        if !has_machine {
            c.machine = c.settings.machine();
        }
        Ok(c)
    }

    pub fn settings_for(&self, s: Studio) -> &JobSettings {
        match s {
            Studio::All => &self.settings,
            Studio::Video => &self.video_settings,
            Studio::Image => &self.image_settings,
        }
    }

    pub fn settings_for_mut(&mut self, s: Studio) -> &mut JobSettings {
        match s {
            Studio::All => &mut self.settings,
            Studio::Video => &mut self.video_settings,
            Studio::Image => &mut self.image_settings,
        }
    }

    /// What a job / preview runs with: machine values applied, switched-off sections
    /// neutralised.
    pub fn job(&self, s: &JobSettings) -> JobSettings {
        s.with_machine(&self.machine).effective()
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
    fn v2_config_migrates_machine_settings() {
        let text = "[settings]\npreset = \"Eco\"\n[settings.video]\nchunk_frames = 64\n";
        let c = Config::parse(text).unwrap();
        assert_eq!(c.machine.preset, Preset::Eco);
        assert_eq!(c.machine.chunk_frames, 64);
        // An explicit [machine] wins.
        let text = "[settings]\npreset = \"Eco\"\n[machine]\npreset = \"Max\"\n";
        assert_eq!(Config::parse(text).unwrap().machine.preset, Preset::Max);
        // New pages get their own defaults.
        assert_eq!(c.video_settings, JobSettings::video_defaults());
    }

    #[test]
    fn recipe_ignores_machine_and_folder() {
        let mut a = JobSettings::default();
        let mut b = a.clone();
        b.preset = Preset::Max;
        b.video.chunk_frames = 999;
        b.output.folder = Some("D:/x".into());
        assert_eq!(a.recipe(), b.recipe());
        b.denoise = 0.5;
        assert_ne!(a.recipe(), b.recipe());
        // apply_recipe keeps the page's own folder + machine values.
        a.output.folder = Some("E:/out".into());
        a.preset = Preset::Eco;
        a.apply_recipe(&b);
        assert_eq!(a.denoise, 0.5);
        assert_eq!(a.output.folder, Some("E:/out".into()));
        assert_eq!(a.preset, Preset::Eco);
    }

    #[test]
    fn suffix_is_sanitised() {
        let mut o = OutputSettings::default();
        assert_eq!(o.safe_suffix(), "_enhanced");
        o.suffix = "_a/b:c".into();
        assert_eq!(o.safe_suffix(), "_abc");
        o.suffix = "  ".into();
        assert_eq!(o.safe_suffix(), "_enhanced");
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
        assert!(s.upscale_on && s.framegen_on && s.filters_on && s.denoise_on && s.sharpen_on);
    }

    #[test]
    fn stage_switches() {
        let s = JobSettings {
            denoise: 0.4,
            sharpen: SharpenMode::Cas,
            clarity: 0.5,
            denoise_on: false,
            ..Default::default()
        };
        let e = s.effective();
        assert_eq!((e.denoise, e.sharpen, e.clarity), (0.0, SharpenMode::Cas, 0.5));
        let e = JobSettings { sharpen_on: false, ..s }.effective();
        assert_eq!((e.sharpen, e.clarity), (SharpenMode::Off, 0.0));
    }
}
