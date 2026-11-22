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
mod tests {
    use super::*;

    pub fn pc() -> SystemInfo {
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
