//! Resource estimates and validation (pure functions over [`SystemInfo`] so they can be
//! unit-tested with fixed numbers).
//!
//! * [`estimate`] — temp disk per chunk / total, RAM, VRAM, threads and time for one input.
//! * [`validate`] / [`machine_issues`] — `Warn` (allowed but risky) and `Block` (can't start)
//!   issues, each optionally with a safe automatic fix ([`fix_machine`]).

use std::path::PathBuf;

use crate::config::{JobSettings, MachineSettings, TileSize};
use crate::pipeline::plan::{effective_resources, valid_jobs};
use crate::t;

pub const MB: u64 = 1 << 20;
pub const GB: u64 = 1 << 30;
/// Chunks on disk at the same time (the video pipeline's 2-token semaphore).
pub const CHUNKS_ON_DISK: u64 = 2;
/// Conservative PNG size / raw RGB size when nothing was measured yet.
pub const DEFAULT_PNG_RATIO: f64 = 0.6;
/// Largest tile `Auto` / auto-tune will pick (TDR safety, see PLAN.md).
pub const MAX_AUTO_TILE: u32 = 256;

/// What the estimator and the checks know about this PC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemInfo {
    pub cores: usize,
    pub ram_total: u64,
    pub ram_available: u64,
    pub vram_total: Option<u64>,
    pub vram_free: Option<u64>,
    /// Free space on the temp folder's drive.
    pub temp_free: Option<u64>,
    /// `Some(true)` when the temp folder is on a spinning disk.
    pub temp_hdd: Option<bool>,
    pub temp_dir: PathBuf,
    pub refresh_hz: Option<u32>,
    pub gpu: bool,
}

impl SystemInfo {
    /// Gather from the monitor snapshot + the temp drive (a few ms: don't call every frame,
    /// see `App::sysinfo`).
    pub fn current(app: &crate::app::App) -> SystemInfo {
        gather(&app.env, &app.config.machine)
    }
}

pub fn gather(env: &crate::pipeline::job::Env, m: &MachineSettings) -> SystemInfo {
    let snap = env.monitor.as_ref().map(|m| m.snapshot()).unwrap_or_default();
    let temp_dir = crate::pipeline::temp::temp_root(m.temp_dir.as_deref());
    let gpu = crate::devices::resolve(m.compute, &env.gpus);
    let (vram_total, vram_free) = match (&snap.gpu, gpu) {
        (Some(g), _) if g.vram_total > 0 => (Some(g.vram_total), Some(g.vram_total.saturating_sub(g.vram_used))),
        (_, Some(g)) if g.vram_bytes > 0 => (Some(g.vram_bytes), None),
        _ => (None, None),
    };
    let ram_total = if snap.ram_total > 0 { snap.ram_total } else { system_ram().0 };
    let ram_available = if snap.ram_total > 0 {
        snap.ram_total.saturating_sub(snap.ram_used)
    } else {
        system_ram().1
    };
    SystemInfo {
        cores: env.cores,
        ram_total,
        ram_available,
        vram_total,
        vram_free,
        temp_free: crate::pipeline::temp::free_space(&temp_dir),
        temp_hdd: disk_is_hdd(&temp_dir),
        temp_dir,
        refresh_hz: crate::preview::player::refresh_rate_hz(),
        gpu: gpu.is_some(),
    }
}

fn system_ram() -> (u64, u64) {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    match System::new_with_specifics(RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram())) {
        Ok(s) => (s.total_memory(), s.available_memory()),
        Err(_) => (0, 0),
    }
}

/// Whether `path` lives on a spinning disk (`None` = unknown).
pub fn disk_is_hdd(path: &std::path::Path) -> Option<bool> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let canon = std::fs::canonicalize(&p).ok()?;
    let canon = canon.to_string_lossy().trim_start_matches(r"\\?\").to_ascii_lowercase();
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    let d = disks
        .iter()
        .filter(|d| canon.starts_with(&d.mount_point().to_string_lossy().to_ascii_lowercase()))
        .max_by_key(|d| d.mount_point().as_os_str().len())?;
    match d.kind() {
        sysinfo::DiskKind::HDD => Some(true),
        sysinfo::DiskKind::SSD => Some(false),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warn,
    Block,
}

/// Which control an issue belongs to (shown inline next to it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Tile,
    Chunk,
    Ram,
    Threads,
    Jobs,
    TempDir,
    TempSpace,
    Vram,
    Encoder,
    Fps,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub level: Level,
    pub field: Field,
    pub message: String,
    /// "Fix automatically" can resolve it by changing machine settings.
    pub fixable: bool,
}

impl Issue {
    fn warn(field: Field, message: String, fixable: bool) -> Issue {
        Issue { level: Level::Warn, field, message, fixable }
    }
    fn block(field: Field, message: String, fixable: bool) -> Issue {
        Issue { level: Level::Block, field, message, fixable }
    }
}

fn hb(b: u64) -> String {
    crate::util::human_bytes(b)
}

/// Rough VRAM need of Real-ESRGAN for a tile (bytes). Heavy = x4plus-class models;
/// compact = animevideo models. `procs` = ncnn proc threads (`-j _:N:_`).
pub fn esrgan_vram(tile: u32, heavy: bool, procs: u32) -> u64 {
    // realesrgan-ncnn-vulkan itself picks tile 200 for a ~1.9 GB heap with x4plus.
    let per = 1900.0 * MB as f64 / (200.0 * 200.0);
    let f = if heavy { 1.0 } else { 0.35 };
    (300.0 * MB as f64 + tile as f64 * tile as f64 * per * f * procs.max(1) as f64) as u64
}

/// Rough VRAM need of RIFE for frames of `w`x`h` (bytes).
pub fn rife_vram(w: u32, h: u32, uhd: bool) -> u64 {
    let per_px = if uhd { 300.0 } else { 600.0 };
    (250.0 * MB as f64 + w as f64 * h as f64 * per_px) as u64
}

/// Largest tile (multiple of 32, ≤ [`MAX_AUTO_TILE`]) that fits in 80 % of free VRAM.
pub fn vram_safe_tile(vram_free: Option<u64>, heavy: bool, procs: u32) -> u32 {
    let Some(free) = vram_free else {
        return 200;
    };
    let budget = free as f64 * 0.8;
    let mut t = MAX_AUTO_TILE;
    while t > 32 && esrgan_vram(t, heavy, procs) as f64 > budget {
        t -= 32;
    }
    t
}

fn procs_of(jobs: &str) -> u32 {
    jobs.split(':').nth(1).and_then(|p| p.parse().ok()).unwrap_or(2)
}

/// Checks on the machine settings alone (used for imported profiles and the Settings page).
pub fn machine_issues(sys: &SystemInfo, m: &MachineSettings) -> Vec<Issue> {
    let mut v = Vec::new();
    let cores = sys.cores.max(1) as u32;
    if let Some(t) = m.filter_threads_override {
        if t > cores {
            v.push(Issue::warn(Field::Threads, t!("res.threads_over", n = t, cores = cores), true));
        } else if t > cores.saturating_sub(1).max(1) {
            v.push(Issue::warn(Field::Threads, t!("res.threads_all", n = t, cores = cores), true));
        }
    }
    if m.cpu_threads > cores {
        v.push(Issue::warn(Field::Threads, t!("res.threads_over", n = m.cpu_threads, cores = cores), true));
    }
    let ram = m.ram_budget_mb as u64 * MB;
    if sys.ram_available > 0 && ram > sys.ram_available {
        v.push(Issue::block(Field::Ram, t!("res.ram_over_avail", budget = hb(ram), avail = hb(sys.ram_available)), true));
    } else if sys.ram_total > 0 && ram as f64 > sys.ram_total as f64 * 0.6 {
        v.push(Issue::warn(Field::Ram, t!("res.ram_high", budget = hb(ram), total = hb(sys.ram_total)), true));
    }
    if let Some(j) = &m.jobs_override
        && !valid_jobs(j)
    {
        v.push(Issue::warn(Field::Jobs, t!("res.jobs_invalid", j = j), true));
    }
    let r = effective_resources(&JobSettings::default().with_machine(m), sys.cores);
    if let TileSize::Fixed(t) = r.tile {
        let safe = vram_safe_tile(sys.vram_free.or(sys.vram_total.map(|t| t * 8 / 10)), true, procs_of(&r.jobs));
        if sys.gpu && t > safe {
            v.push(Issue::warn(Field::Tile, t!("res.tile_high", tile = t, safe = safe), true));
        }
    }
    if let Some(d) = &m.temp_dir
        && (!d.is_absolute() || d.to_string_lossy().trim().is_empty())
    {
        v.push(Issue::block(Field::TempDir, t!("res.temp_invalid", dir = d.display()), true));
    } else if let Some(d) = &m.temp_dir
        && !d.exists()
        && std::fs::create_dir_all(d).is_err()
    {
        v.push(Issue::block(Field::TempDir, t!("res.temp_invalid", dir = d.display()), true));
    }
    if sys.temp_hdd == Some(true) {
        v.push(Issue::warn(Field::TempDir, t!("res.temp_hdd"), false));
    }
    v
}

/// `m` with values that cause issues on this PC replaced by safe ones.
pub fn fix_machine(sys: &SystemInfo, m: &MachineSettings) -> MachineSettings {
    let mut f = m.clone();
    let cores = sys.cores.max(1) as u32;
    let safe_threads = cores.saturating_sub(1).max(1);
    if let Some(t) = f.filter_threads_override.as_mut() {
        *t = (*t).min(safe_threads);
    }
    f.cpu_threads = f.cpu_threads.min(cores);
    let ram = f.ram_budget_mb as u64 * MB;
    let cap = safe_ram_budget(sys);
    if (sys.ram_available > 0 && ram > sys.ram_available) || (sys.ram_total > 0 && ram as f64 > sys.ram_total as f64 * 0.6) {
        f.ram_budget_mb = (cap / MB) as u32;
    }
    if f.jobs_override.as_ref().is_some_and(|j| !valid_jobs(j)) {
        f.jobs_override = None;
    }
    let r = effective_resources(&JobSettings::default().with_machine(&f), sys.cores);
    if let Some(TileSize::Fixed(t)) = f.tile_override {
        let safe = vram_safe_tile(sys.vram_free.or(sys.vram_total.map(|t| t * 8 / 10)), true, procs_of(&r.jobs));
        if sys.gpu && t > safe {
            f.tile_override = Some(TileSize::Fixed(safe));
        }
    }
    if let Some(d) = &f.temp_dir
        && (!d.is_absolute() || (!d.exists() && std::fs::create_dir_all(d).is_err()))
    {
        f.temp_dir = None;
    }
    f
}

/// RAM budget that is safe here: ≤ 50 % of available and ≤ 25 % of total, 512 MB..16 GB,
/// rounded down to 256 MB.
pub fn safe_ram_budget(sys: &SystemInfo) -> u64 {
    if sys.ram_total == 0 {
        return 2048 * MB;
    }
    let v = (sys.ram_available / 2).min(sys.ram_total / 4).clamp(512 * MB, 16 * GB);
    v / (256 * MB) * (256 * MB)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn pc() -> SystemInfo {
        SystemInfo {
            cores: 16,
            ram_total: 32 * GB,
            ram_available: 20 * GB,
            vram_total: Some(16 * GB),
            vram_free: Some(14 * GB),
            temp_free: Some(200 * GB),
            temp_hdd: Some(false),
            temp_dir: std::env::temp_dir(),
            refresh_hz: Some(60),
            gpu: true,
        }
    }

    #[test]
    fn vram_tile_math() {
        assert_eq!(vram_safe_tile(Some(14 * GB), true, 2), 256);
        let small = vram_safe_tile(Some(2 * GB), true, 2);
        assert!((32..200).contains(&small), "{small}");
        assert!(vram_safe_tile(Some(2 * GB), false, 2) > small);
        assert!(esrgan_vram(256, true, 1) > esrgan_vram(128, true, 1));
        assert!(rife_vram(1920, 1080, true) < rife_vram(1920, 1080, false));
    }

    #[test]
    fn machine_checks_and_fixes() {
        let sys = pc();
        let ok = MachineSettings::default();
        assert!(machine_issues(&sys, &ok).is_empty(), "{:?}", machine_issues(&sys, &ok));
        let bad = MachineSettings {
            filter_threads_override: Some(64),
            ram_budget_mb: 24 * 1024,
            tile_override: Some(TileSize::Fixed(1024)),
            jobs_override: Some("x".into()),
            ..Default::default()
        };
        let issues = machine_issues(&sys, &bad);
        let has = |f: Field, l: Level| issues.iter().any(|i| i.field == f && i.level == l);
        assert!(has(Field::Threads, Level::Warn));
        assert!(has(Field::Ram, Level::Block)); // 24 GB > 20 GB available
        assert!(has(Field::Tile, Level::Warn));
        assert!(has(Field::Jobs, Level::Warn));
        let fixed = fix_machine(&sys, &bad);
        assert!(machine_issues(&sys, &fixed).is_empty(), "{:?}", machine_issues(&sys, &fixed));
        assert_eq!(fixed.filter_threads_override, Some(15));
        assert_eq!(fixed.ram_budget_mb, 8192); // min(10 GB, 8 GB)
        // RAM over 60 % of total but available → warning only.
        let mut s2 = sys.clone();
        s2.ram_available = 30 * GB;
        let m = MachineSettings { ram_budget_mb: 20 * 1024, ..Default::default() };
        let i = machine_issues(&s2, &m);
        assert!(i.iter().any(|i| i.field == Field::Ram && i.level == Level::Warn));
    }

    #[test]
    fn hdd_temp_is_a_warning() {
        let mut sys = pc();
        sys.temp_hdd = Some(true);
        let i = machine_issues(&sys, &MachineSettings::default());
        assert!(i.iter().any(|i| i.field == Field::TempDir && i.level == Level::Warn));
    }
}

// ---- estimator ---------------------------------------------------------------------------

/// Everything the estimator needs about one input + recipe (built by [`input_for`]).
#[derive(Clone, Debug, PartialEq)]
pub struct EstimateInput {
    pub video: bool,
    pub src_w: u32,
    pub src_h: u32,
    /// Source frames to process (1 for images).
    pub frames: u64,
    /// Frame-generation multiplier (1 = off).
    pub k: u32,
    /// AI upscale stage runs (frames go to disk).
    pub ai: bool,
    /// x4plus-class (heavy) model vs. compact animevideo model.
    pub heavy: bool,
    pub model_w: u32,
    pub model_h: u32,
    pub out_w: u32,
    pub out_h: u32,
    pub out_fps: f64,
    /// The AI stage runs on in-memory frames (ONNX): no PNG folders for it.
    pub in_memory: bool,
    /// AI cost relative to realesrgan-x4plus (the benchmark model) per source megapixel.
    pub ai_factor: f64,
    /// VRAM of the AI stage from the catalog (ONNX); `None` = ncnn tile estimate.
    pub ai_vram: Option<u64>,
    /// AI cleanup cost relative to realesrgan-x4plus (0 = off).
    pub restore_factor: f64,
    /// Face restoration: seconds per frame on the reference PC (about one face; 0 = off).
    pub face_s: f64,
    /// VRAM of the cleanup / face sessions that stay loaded next to the upscaler.
    pub extra_vram: u64,
}

impl Default for EstimateInput {
    fn default() -> Self {
        EstimateInput {
            video: true,
            src_w: 1920,
            src_h: 1080,
            frames: 1800,
            k: 2,
            ai: true,
            heavy: true,
            model_w: 7680,
            model_h: 4320,
            out_w: 3840,
            out_h: 2160,
            out_fps: 60.0,
            in_memory: false,
            ai_factor: 1.0,
            ai_vram: None,
            restore_factor: 0.0,
            face_s: 0.0,
            extra_vram: 0,
        }
    }
}

/// Catalog cost of the benchmark model (realesrgan-x4plus) on the reference PC, s / MP.
pub const X4PLUS_S_PER_MP: f64 = 6.0;

/// Effective performance values the estimate is computed for.
#[derive(Clone, Debug, PartialEq)]
pub struct EstimateMachine {
    pub chunk_frames: u32,
    pub ram_budget: u64,
    pub filter_threads: usize,
    pub tile: u32,
    pub procs: u32,
}

impl EstimateMachine {
    pub fn of(s: &JobSettings, cores: usize, vram_free: Option<u64>) -> EstimateMachine {
        let r = effective_resources(s, cores);
        EstimateMachine {
            chunk_frames: s.video.chunk_frames.max(8),
            ram_budget: s.video.ram_budget_mb.max(256) as u64 * MB,
            filter_threads: r.filter_threads,
            tile: crate::pipeline::plan::resolve_tile(&r, vram_free),
            procs: procs_of(&r.jobs),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Estimate {
    /// Frames go through temp PNG folders.
    pub disk_mode: bool,
    pub temp_per_chunk: u64,
    /// Peak temp use (2 chunks on disk).
    pub temp_total: u64,
    pub ram: u64,
    pub vram: Option<u64>,
    pub threads: usize,
    pub out_frames: u64,
    pub chunks: u64,
    /// `None` = no benchmark yet.
    pub time_secs: Option<f64>,
}

/// PNG bytes for a frame (`ratio` = PNG size / raw RGB size).
pub fn png_bytes(w: u32, h: u32, ratio: f64) -> u64 {
    (w as f64 * h as f64 * 3.0 * ratio) as u64
}

/// Same queue sizing as `video_job::run_video`.
fn queue_caps(budget: u64, src_bytes: u64, out_bytes: u64, threads: usize) -> (u64, u64, u64) {
    let writer = (budget / 4 / out_bytes.max(1)).clamp(2, 32);
    let batch = (budget / 4 / out_bytes.max(1)).clamp(1, threads.max(2) as u64);
    let raw = (budget / 4 / src_bytes.max(1)).clamp(2, 32);
    (writer, batch, raw)
}

pub fn estimate(i: &EstimateInput, m: &EstimateMachine, png_ratio: f64, bench: Option<&BenchResult>) -> Estimate {
    let src_bytes = i.src_w as u64 * i.src_h as u64 * 3;
    let model_bytes = i.model_w as u64 * i.model_h as u64 * 3;
    let out_bytes = i.out_w as u64 * i.out_h as u64 * 3;
    let k = i.k.max(1) as u64;
    let mut e = Estimate {
        threads: m.filter_threads,
        out_frames: i.frames.max(1) * k,
        ..Default::default()
    };
    let ai_disk = i.ai && !i.in_memory;
    if i.video {
        e.disk_mode = ai_disk || k > 1;
        let n = (m.chunk_frames as u64).min(i.frames.max(1)) + 1;
        e.chunks = i.frames.max(1).div_ceil(m.chunk_frames.max(1) as u64);
        if e.disk_mode {
            let src_png = png_bytes(i.src_w, i.src_h, png_ratio);
            let interp = if k > 1 { src_png * n * k } else { 0 };
            let up = if ai_disk { png_bytes(i.model_w, i.model_h, png_ratio) * n * k } else { 0 };
            e.temp_per_chunk = src_png * n + interp + up;
            e.temp_total = e.temp_per_chunk * CHUNKS_ON_DISK.min(e.chunks.max(1));
        }
        let (writer, batch, raw) = queue_caps(m.ram_budget, src_bytes, out_bytes, m.filter_threads);
        e.ram = writer * out_bytes + raw * src_bytes + batch * (model_bytes + out_bytes) + 400 * MB;
    } else {
        e.disk_mode = ai_disk;
        e.chunks = 1;
        if ai_disk {
            e.temp_per_chunk = png_bytes(i.src_w, i.src_h, png_ratio) + png_bytes(i.model_w, i.model_h, png_ratio);
            e.temp_total = e.temp_per_chunk;
        }
        e.ram = src_bytes * 2 + model_bytes * 2 + out_bytes * 2 + 300 * MB;
    }
    let up = i
        .ai
        .then(|| i.ai_vram.unwrap_or_else(|| esrgan_vram(m.tile, i.heavy, m.procs)) + i.extra_vram)
        .or((i.extra_vram > 0).then_some(i.extra_vram));
    let rife = (k > 1).then(|| rife_vram(i.src_w, i.src_h, i.src_w.min(i.src_h) >= 1440));
    e.vram = match (up, rife) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    e.time_secs = bench.map(|b| estimate_time(i, m, &e, b));
    e
}

fn estimate_time(i: &EstimateInput, m: &EstimateMachine, e: &Estimate, b: &BenchResult) -> f64 {
    let src_mp = i.src_w as f64 * i.src_h as f64 / 1e6;
    let out_mp = i.out_w as f64 * i.out_h as f64 / 1e6;
    let frames = e.out_frames as f64;
    let mut t = 0.0;
    // This PC's speed relative to the reference PC, from the x4plus benchmark.
    let x4 = b.esrgan_ms_per_mp(m.tile) / 1000.0;
    if i.ai {
        t += frames * src_mp * x4 * i.ai_factor;
        if !i.in_memory {
            t += b.esrgan_startup_s * e.chunks as f64;
        }
    }
    // Cleanup runs on source frames, faces once per output frame of the AI stage.
    let src_frames = i.frames.max(1) as f64;
    t += src_frames * src_mp * x4 * i.restore_factor;
    t += frames * i.face_s * x4 / X4PLUS_S_PER_MP;
    if i.k > 1 {
        t += frames * src_mp * b.rife_ms_per_mp.unwrap_or(40.0) / 1000.0;
        t += b.esrgan_startup_s * e.chunks as f64;
    }
    // CPU stages run in parallel with the GPU; count the part that exceeds it.
    let cpu = frames * out_mp * b.filter_ms_per_mp / 1000.0 * (16.0 / m.filter_threads.max(1) as f64).max(1.0) / 4.0;
    if i.video {
        let enc = b.encode_fps_720p.unwrap_or(120.0) * (0.92 / out_mp.max(0.05));
        let png = if e.disk_mode {
            e.temp_per_chunk as f64 * e.chunks as f64 / (b.png_write_mb_s.max(20.0) * MB as f64)
        } else {
            0.0
        };
        t = t.max(frames / enc.max(1.0)) + png + cpu * 0.3;
    } else {
        t += cpu;
    }
    t.max(0.5)
}

// ---- validation --------------------------------------------------------------------------

/// Extra context for job-level checks.
#[derive(Clone, Debug, Default)]
pub struct JobContext {
    /// The chosen encoder is not usable (video only).
    pub encoder_unavailable: Option<String>,
    /// Output folder can't be created.
    pub output_dir_invalid: Option<PathBuf>,
}

/// All issues for one job (machine checks included).
pub fn validate(i: &EstimateInput, e: &Estimate, m: &MachineSettings, sys: &SystemInfo, ctx: &JobContext) -> Vec<Issue> {
    let mut v = machine_issues(sys, m);
    if let Some(free) = sys.temp_free
        && e.temp_total > 0
    {
        if e.temp_total > free {
            v.push(Issue::block(Field::TempSpace, t!("res.temp_over", need = hb(e.temp_total), free = hb(free)), true));
        } else if e.temp_total as f64 > free as f64 * 0.8 {
            v.push(Issue::warn(Field::TempSpace, t!("res.temp_high", need = hb(e.temp_total), free = hb(free)), true));
        }
    }
    if sys.ram_available > 0 && e.ram > sys.ram_available {
        v.push(Issue::block(Field::Ram, t!("res.ram_est_over", need = hb(e.ram), avail = hb(sys.ram_available)), true));
    }
    if let (Some(need), Some(free)) = (e.vram, sys.vram_free)
        && need > free
    {
        v.push(Issue::warn(Field::Vram, t!("res.vram_high", need = hb(need), free = hb(free)), true));
    }
    if let Some(enc) = &ctx.encoder_unavailable {
        v.push(Issue::block(Field::Encoder, t!("res.encoder_unavailable", enc = enc), true));
    }
    if let Some(d) = &ctx.output_dir_invalid {
        v.push(Issue::block(Field::TempDir, t!("res.output_invalid", dir = d.display()), false));
    }
    if i.video
        && let Some(hz) = sys.refresh_hz
        && i.out_fps > hz as f64 + 0.5
    {
        v.push(Issue::warn(Field::Fps, t!("res.fps_over_refresh", fps = format!("{:.0}", i.out_fps), hz = hz), false));
    }
    v.sort_by_key(|i| std::cmp::Reverse(i.level));
    v.dedup_by(|a, b| a.field == b.field && a.message == b.message);
    v
}

/// Largest chunk size whose two chunks use at most `share` of the free temp space.
pub fn chunk_that_fits(i: &EstimateInput, free: u64, png_ratio: f64, share: f64) -> Option<u32> {
    let k = i.k.max(1) as u64;
    let src_png = png_bytes(i.src_w, i.src_h, png_ratio);
    let per_frame = src_png
        + if k > 1 { src_png * k } else { 0 }
        + if i.ai { png_bytes(i.model_w, i.model_h, png_ratio) * k } else { 0 };
    if per_frame == 0 {
        return None;
    }
    let n = (free as f64 * share / CHUNKS_ON_DISK as f64 / per_frame as f64).floor() as i64 - 1;
    (n >= 16).then(|| (n.min(2000) as u32) / 8 * 8)
}

/// "Fix automatically": safe machine values + a chunk size that fits the temp drive.
/// Returns the new machine settings and whether the encoder must be reset to Auto.
pub fn fix_job(i: &EstimateInput, m: &MachineSettings, sys: &SystemInfo, issues: &[Issue], png_ratio: f64) -> (MachineSettings, bool) {
    let mut f = fix_machine(sys, m);
    let disk = issues.iter().any(|x| x.field == Field::TempSpace);
    if disk
        && let Some(free) = sys.temp_free
        && let Some(c) = chunk_that_fits(i, free, png_ratio, 0.5)
    {
        f.chunk_frames = f.chunk_frames.min(c);
    }
    if issues.iter().any(|x| x.field == Field::Vram) {
        let safe = vram_safe_tile(sys.vram_free, i.heavy, 1);
        f.tile_override = Some(TileSize::Fixed(safe.min(128)));
        f.jobs_override = Some("1:1:2".into());
    }
    if issues.iter().any(|x| x.field == Field::Ram && x.level == Level::Block) {
        f.ram_budget_mb = f.ram_budget_mb.min((safe_ram_budget(sys) / MB) as u32);
    }
    (f, issues.iter().any(|x| x.field == Field::Encoder))
}

// ---- benchmark results + auto-tune ------------------------------------------------------

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
pub struct TileBench {
    pub tile: u32,
    pub ok: bool,
    /// Real-ESRGAN (heavy model, 4x) milliseconds per source megapixel.
    pub ms_per_mp: f64,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BenchResult {
    /// Identifies the device the numbers belong to.
    pub device_key: String,
    pub device_name: String,
    pub created: i64,
    pub app_version: String,
    pub tiles: Vec<TileBench>,
    pub esrgan_startup_s: f64,
    /// RIFE ms per source megapixel per generated output frame.
    pub rife_ms_per_mp: Option<f64>,
    /// Denoise + CAS + resize, ms per output megapixel (all filter threads).
    pub filter_ms_per_mp: f64,
    pub png_write_mb_s: f64,
    pub png_ratio: f64,
    pub encoder: Option<String>,
    pub encode_fps_720p: Option<f64>,
}

impl Default for BenchResult {
    fn default() -> Self {
        BenchResult {
            device_key: String::new(),
            device_name: String::new(),
            created: 0,
            app_version: env!("CARGO_PKG_VERSION").into(),
            tiles: Vec::new(),
            esrgan_startup_s: 1.5,
            rife_ms_per_mp: None,
            filter_ms_per_mp: 30.0,
            png_write_mb_s: 200.0,
            png_ratio: DEFAULT_PNG_RATIO,
            encoder: None,
            encode_fps_720p: None,
        }
    }
}

impl BenchResult {
    /// ms/MP at `tile` (nearest measured tile; slow default when nothing was measured).
    pub fn esrgan_ms_per_mp(&self, tile: u32) -> f64 {
        self.tiles
            .iter()
            .filter(|t| t.ok && t.ms_per_mp > 0.0)
            .min_by_key(|t| t.tile.abs_diff(tile))
            .map(|t| t.ms_per_mp)
            .unwrap_or(2500.0)
    }
    /// Fastest tile that worked.
    pub fn best_tile(&self) -> Option<u32> {
        self.tiles
            .iter()
            .filter(|t| t.ok && t.ms_per_mp > 0.0)
            .min_by(|a, b| a.ms_per_mp.total_cmp(&b.ms_per_mp))
            .map(|t| t.tile)
    }
}

/// One line of "what auto-tune changed".
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub what: String,
    pub from: String,
    pub to: String,
}

/// Choose tile / chunk / RAM / threads / `-j` for this PC. `typical` = the input to tune for
/// (selected video, or 1080p 2x + frame generation). Returns the new settings + changes.
pub fn auto_tune(sys: &SystemInfo, bench: Option<&BenchResult>, m: &MachineSettings, typical: &EstimateInput) -> (MachineSettings, Vec<Change>) {
    let mut f = m.clone();
    let cores = sys.cores.max(1) as u32;
    // -j: proc threads double VRAM use; 2 only with plenty of free VRAM.
    let vfree = sys.vram_free.or(sys.vram_total.map(|t| t * 8 / 10));
    let load = if cores >= 8 { 2 } else { 1 };
    let save = if cores >= 8 { 4 } else { 2 };
    let mut tile = bench.and_then(|b| b.best_tile()).unwrap_or(200).min(MAX_AUTO_TILE);
    let procs = if vfree.is_some_and(|v| esrgan_vram(tile, true, 2) as f64 <= v as f64 * 0.6) { 2 } else { 1 };
    tile = tile.min(vram_safe_tile(vfree, true, procs));
    f.tile_override = Some(TileSize::Fixed(tile));
    f.jobs_override = Some(format!("{load}:{procs}:{save}"));
    // ~75 % of the cores keeps the PC responsive while the GPU is the bottleneck anyway.
    f.filter_threads_override = Some((cores * 3 / 4).clamp(1, cores.saturating_sub(1).max(1)));
    f.ram_budget_mb = (safe_ram_budget(sys) / MB) as u32;
    // Chunk: ≥ ~25 s of GPU work per chunk (amortises ncnn start-up), limited by disk.
    let src_mp = typical.src_w as f64 * typical.src_h as f64 / 1e6;
    let k = typical.k.max(1) as f64;
    let per_src_frame_s = match bench {
        Some(b) => {
            let up = if typical.ai { k * src_mp * b.esrgan_ms_per_mp(tile) * typical.ai_factor / 1000.0 } else { 0.0 };
            let rife = if typical.k > 1 { k * src_mp * b.rife_ms_per_mp.unwrap_or(40.0) / 1000.0 } else { 0.0 };
            (up + rife).max(0.005)
        }
        None => 0.2,
    };
    let mut chunk = (25.0 / per_src_frame_s).ceil().clamp(32.0, 1500.0) as u32;
    let ratio = bench.map(|b| b.png_ratio).unwrap_or(DEFAULT_PNG_RATIO);
    if let Some(free) = sys.temp_free {
        chunk = match chunk_that_fits(typical, free, ratio, 0.4) {
            Some(c) => chunk.min(c),
            // Not even 16 frames fit: smallest chunk (the pre-start check will block).
            None => 16,
        };
    }
    f.chunk_frames = (chunk / 8 * 8).max(16);
    let mut changes = Vec::new();
    let mut note = |what: String, a: String, b: String| {
        if a != b {
            changes.push(Change { what, from: a, to: b });
        }
    };
    let r0 = effective_resources(&JobSettings::default().with_machine(m), sys.cores);
    let r1 = effective_resources(&JobSettings::default().with_machine(&f), sys.cores);
    let tile_str = |r: &crate::pipeline::plan::Resources| match r.tile {
        TileSize::Fixed(t) => t.to_string(),
        TileSize::Auto => "Auto".into(),
    };
    note(t!("perf.tile_size"), tile_str(&r0), tile_str(&r1));
    note("ncnn -j".into(), r0.jobs.clone(), r1.jobs.clone());
    note(t!("perf.filter_threads"), r0.filter_threads.to_string(), r1.filter_threads.to_string());
    note(t!("perf.ram_budget"), format!("{} MB", m.ram_budget_mb), format!("{} MB", f.ram_budget_mb));
    note(t!("perf.chunk_size"), m.chunk_frames.to_string(), f.chunk_frames.to_string());
    (f, changes)
}

#[cfg(test)]
mod estimate_tests {
    use super::tests::pc;
    use super::*;

    fn input_1080p(k: u32, ai: bool) -> EstimateInput {
        EstimateInput {
            video: true,
            src_w: 1920,
            src_h: 1080,
            frames: 3000,
            k,
            ai,
            heavy: true,
            model_w: 7680,
            model_h: 4320,
            out_w: 3840,
            out_h: 2160,
            out_fps: 30.0 * k as f64,
            ..Default::default()
        }
    }

    fn mach() -> EstimateMachine {
        EstimateMachine { chunk_frames: 120, ram_budget: 2048 * MB, filter_threads: 8, tile: 200, procs: 2 }
    }

    #[test]
    fn temp_math_with_fixed_numbers() {
        let i = input_1080p(2, true);
        let e = estimate(&i, &mach(), 0.5, None);
        // n = 121 frames; src 1920*1080*3*0.5 = 3_110_400 B
        let src = 3_110_400u64;
        let up = (7680u64 * 4320 * 3) / 2;
        assert_eq!(e.temp_per_chunk, src * 121 + src * 121 * 2 + up * 121 * 2);
        assert_eq!(e.temp_total, e.temp_per_chunk * 2);
        assert_eq!(e.chunks, 25);
        assert_eq!(e.out_frames, 6000);
        assert!(e.time_secs.is_none());
        // No AI, no frame gen → in memory.
        let e = estimate(&input_1080p(1, false), &mach(), 0.5, None);
        assert!(!e.disk_mode);
        assert_eq!(e.temp_total, 0);
        // RAM: writer = 512 MB / 24.9 MB → 21 frames, batch 8, raw 32 (capped).
        let out = 3840u64 * 2160 * 3;
        let src_b = 1920u64 * 1080 * 3;
        let model = 7680u64 * 4320 * 3;
        assert_eq!(e.ram, 21 * out + 32 * src_b + 8 * (model + out) + 400 * MB);
    }

    #[test]
    fn validation_levels() {
        let sys = pc();
        let i = input_1080p(2, true);
        let mut e = estimate(&i, &mach(), 0.5, None);
        let m = MachineSettings::default();
        let ctx = JobContext::default();
        assert!(validate(&i, &e, &m, &sys, &ctx).iter().all(|x| x.level != Level::Block));
        // 60 fps > 60 Hz? no; 120 fps > 60 Hz → warning.
        let mut i2 = i.clone();
        i2.out_fps = 120.0;
        assert!(validate(&i2, &e, &m, &sys, &ctx).iter().any(|x| x.field == Field::Fps));
        // Temp over 80 % of free → warn; over free → block.
        e.temp_total = 170 * GB;
        assert!(validate(&i, &e, &m, &sys, &ctx).iter().any(|x| x.field == Field::TempSpace && x.level == Level::Warn));
        e.temp_total = 250 * GB;
        let issues = validate(&i, &e, &m, &sys, &ctx);
        assert_eq!(issues[0].level, Level::Block);
        let (fixed, _) = fix_job(&i, &m, &sys, &issues, 0.5);
        let e2 = estimate(&i, &EstimateMachine { chunk_frames: fixed.chunk_frames, ..mach() }, 0.5, None);
        assert!(e2.temp_total <= 100 * GB, "{}", e2.temp_total);
        // Encoder unavailable → blocking + encoder reset requested.
        let ctx = JobContext { encoder_unavailable: Some("NVENC HEVC".into()), ..Default::default() };
        let issues = validate(&i, &estimate(&i, &mach(), 0.5, None), &m, &sys, &ctx);
        assert!(issues.iter().any(|x| x.field == Field::Encoder && x.level == Level::Block));
        assert!(fix_job(&i, &m, &sys, &issues, 0.5).1);
    }

    #[test]
    fn time_from_benchmark() {
        let b = BenchResult {
            tiles: vec![
                TileBench { tile: 128, ok: true, ms_per_mp: 900.0 },
                TileBench { tile: 256, ok: true, ms_per_mp: 700.0 },
            ],
            rife_ms_per_mp: Some(20.0),
            encode_fps_720p: Some(300.0),
            ..Default::default()
        };
        assert_eq!(b.best_tile(), Some(256));
        assert_eq!(b.esrgan_ms_per_mp(200), 700.0);
        let i = input_1080p(2, true);
        let t = estimate(&i, &mach(), 0.5, Some(&b)).time_secs.unwrap();
        // 6000 frames × 2.07 MP × 0.7 s ≈ 8700 s of upscaling alone.
        assert!(t > 8000.0 && t < 20000.0, "{t}");
        // A model twice as heavy (catalog cost) takes about twice as long on this PC.
        let t2 = estimate(&EstimateInput { ai_factor: 2.0, ..i.clone() }, &mach(), 0.5, Some(&b)).time_secs.unwrap();
        assert!(t2 > t * 1.6, "{t} -> {t2}");
        // Cleanup and faces add time; ONNX in memory needs no upscale PNGs.
        let i3 = EstimateInput { restore_factor: 0.2, face_s: 0.1, ..i.clone() };
        assert!(estimate(&i3, &mach(), 0.5, Some(&b)).time_secs.unwrap() > t);
    }

    #[test]
    fn onnx_in_memory_estimate() {
        let mem = EstimateInput {
            k: 1,
            in_memory: true,
            ai_factor: 0.35 / X4PLUS_S_PER_MP,
            ai_vram: Some(250 * MB),
            extra_vram: 1500 * MB,
            ..input_1080p(1, true)
        };
        let e = estimate(&mem, &mach(), 0.5, None);
        assert!(!e.disk_mode);
        assert_eq!(e.temp_total, 0);
        assert_eq!(e.vram, Some(1750 * MB));
        // With frame generation the source frames go to disk, but no upscaled PNGs.
        let e2 = estimate(&EstimateInput { k: 2, ..mem.clone() }, &mach(), 0.5, None);
        let src = 3_110_400u64;
        assert!(e2.disk_mode);
        assert_eq!(e2.temp_per_chunk, src * 121 + src * 121 * 2);
    }

    #[test]
    fn auto_tune_is_sensible_for_a_16gb_gpu() {
        let sys = pc();
        let b = BenchResult {
            tiles: vec![
                TileBench { tile: 128, ok: true, ms_per_mp: 900.0 },
                TileBench { tile: 192, ok: true, ms_per_mp: 800.0 },
                TileBench { tile: 256, ok: true, ms_per_mp: 760.0 },
            ],
            rife_ms_per_mp: Some(15.0),
            ..Default::default()
        };
        let i = input_1080p(2, true);
        let (m, changes) = auto_tune(&sys, Some(&b), &MachineSettings::default(), &i);
        assert_eq!(m.tile_override, Some(TileSize::Fixed(256)));
        assert_eq!(m.jobs_override.as_deref(), Some("2:2:4"));
        assert_eq!(m.filter_threads_override, Some(12));
        assert_eq!(m.ram_budget_mb, 8192);
        // 25 s / (2 × 2.07 MP × 0.775 s/MP) ≈ 8 → clamped to 32.
        assert_eq!(m.chunk_frames, 32);
        assert!(!changes.is_empty());
        assert!(machine_issues(&sys, &m).is_empty(), "{:?}", machine_issues(&sys, &m));
        // Small disk limits the chunk size.
        let mut s2 = sys.clone();
        s2.temp_free = Some(2 * GB);
        let small = input_1080p(1, false);
        let (m2, _) = auto_tune(&s2, None, &MachineSettings::default(), &EstimateInput { ai: true, ..small });
        assert_eq!(m2.chunk_frames, 16);
    }
}

/// Build the estimator input for `info` with `s` (the job settings, machine applied).
pub fn input_for(env: &crate::pipeline::job::Env, s: &JobSettings, info: &crate::media::probe::MediaInfo) -> Option<EstimateInput> {
    use crate::media::MediaKind;
    let cpu = crate::devices::resolve(s.compute, &env.gpus).is_none();
    let b = crate::engines::choose_backend(s.profile, s.model.as_deref(), info.kind, &env.tools, &env.models, cpu).ok()?;
    let video = info.kind == MediaKind::Video;
    let p = crate::pipeline::plan::plan_scale(info.width, info.height, s.scale, &b.backend.native_scales(), video);
    let ai = b.backend.is_ai() && p.model_scale.is_some();
    let (model_w, model_h) = match p.model_scale.filter(|_| ai) {
        Some(m) => (info.width * m, info.height * m),
        None => (info.width, info.height),
    };
    let src_fps = info.fps.unwrap_or(crate::media::Rational::new(30, 1));
    let f = crate::pipeline::plan::plan_fps(src_fps, s.framegen, s.max_fps);
    let k = if video { f.multiplier } else { 1 };
    Some(EstimateInput {
        video,
        src_w: info.width,
        src_h: info.height,
        frames: if video { info.frames.unwrap_or(1) } else { 1 },
        k,
        ai,
        heavy: !b.backend.label().to_ascii_lowercase().contains("animevideo"),
        model_w,
        model_h,
        out_w: p.out_w,
        out_h: p.out_h,
        out_fps: if video { f.out_fps.as_f64() } else { 0.0 },
        ..catalog_costs(env, s, info.kind, b.backend.in_memory())
    })
}

/// Catalog-based cost of the AI stages of `s` (relative to the x4plus benchmark model).
fn catalog_costs(
    env: &crate::pipeline::job::Env,
    s: &JobSettings,
    kind: crate::media::MediaKind,
    in_memory: bool,
) -> EstimateInput {
    let cat = &env.models.catalog;
    let entry = |id: &str| cat.model(id);
    let rel = |id: &str| entry(id).map(|m| m.cost.s_per_mp as f64 / X4PLUS_S_PER_MP);
    let vram = |id: &str| entry(id).map(|m| m.cost.vram_mb as u64 * MB);
    let up_id = crate::engines::active_model_id(s.profile, s.model.as_deref(), kind, &env.models);
    let mut e = EstimateInput {
        in_memory,
        ai_factor: up_id.as_deref().and_then(rel).unwrap_or(1.0),
        ai_vram: up_id.as_deref().filter(|_| in_memory).and_then(vram),
        ..Default::default()
    };
    if s.restore_on {
        e.restore_factor = rel(&s.restore_model).unwrap_or(0.25);
        e.extra_vram += vram(&s.restore_model).unwrap_or(300 * MB);
    }
    if s.face_on {
        // ~50 ms per face for GFPGAN on the reference PC; heavier models scale with VRAM.
        let face_vram = vram(&s.face_model).unwrap_or(1500 * MB);
        e.face_s = 0.05 * face_vram as f64 / (1500 * MB) as f64 + 0.01;
        e.extra_vram += face_vram + vram("yunet").unwrap_or(50 * MB);
    }
    e
}
