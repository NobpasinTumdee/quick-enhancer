//! Temp directory management: per-job dirs that clean themselves up, stale-dir cleanup at
//! startup, disk usage and free-space checks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Name of the app's folder inside the temp root.
pub const TEMP_DIR_NAME: &str = "quick-enhancer-tmp";
/// Folder used by v1 ("Enhancer"); stale leftovers there are cleaned up too.
pub const LEGACY_TEMP_DIR_NAME: &str = "enhancer-tmp";

/// `<temp>/enhancer-tmp` (v1 location), for stale cleanup.
pub fn legacy_temp_root(custom: Option<&Path>) -> PathBuf {
    custom
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join(LEGACY_TEMP_DIR_NAME)
}

/// `<temp>/quick-enhancer-tmp`.
pub fn temp_root(custom: Option<&Path>) -> PathBuf {
    custom
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join(TEMP_DIR_NAME)
}

/// A directory removed (recursively) when dropped — on success, error, cancel or panic.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(root: &Path, label: &str) -> std::io::Result<TempDir> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!("{}-{label}-{n}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("could not remove temp dir {}: {e}", self.path.display());
        }
    }
}

/// Remove temp dirs left behind by processes that are no longer running.
pub fn cleanup_stale(root: &Path) -> usize {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(rd) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut sys = System::new().ok();
    let me = std::process::id();
    let mut removed = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let pid: Option<u32> = name.split('-').next().and_then(|p| p.parse().ok());
        let alive = match (pid, sys.as_mut()) {
            (Some(p), _) if p == me => true,
            (Some(p), Some(s)) => {
                let pid = Pid::from_u32(p);
                s.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::nothing(),
                );
                s.process(pid).is_some()
            }
            (Some(_), None) => true, // can't tell; be conservative
            (None, _) => false,
        };
        if !alive && std::fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(
            "removed {removed} stale temp folder(s) from {}",
            root.display()
        );
    }
    removed
}

/// Total size of files under `path`.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

/// Free space on the volume containing `path` (the path or its nearest existing ancestor).
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let canon = strip_verbatim(&std::fs::canonicalize(&p).ok()?);
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    disks
        .iter()
        .filter(|d| {
            let mp = d.mount_point().to_string_lossy().to_ascii_lowercase();
            canon
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with(&mp)
        })
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

/// Rough upper bound of temp disk use for a chunked video job (two chunks on disk).
pub fn estimate_temp_bytes(
    src_w: u32,
    src_h: u32,
    model_w: u32,
    model_h: u32,
    chunk_frames: u32,
    k: u32,
    ai: bool,
) -> u64 {
    // PNG of natural video ≈ 50–60 % of raw RGB; use 0.6 to stay on the safe side.
    let png = |w: u32, h: u32| (w as f64 * h as f64 * 3.0 * 0.6) as u64;
    let n = chunk_frames as u64 + 1;
    let src = png(src_w, src_h) * n;
    let interp = if k > 1 {
        png(src_w, src_h) * n * k as u64
    } else {
        0
    };
    let up = if ai {
        png(model_w, model_h) * n * k.max(1) as u64
    } else {
        0
    };
    // One chunk fully expanded + the next chunk's source frames.
    src + interp + up + src
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_removed_on_drop() {
        let root = std::env::temp_dir().join("enhancer-tmp-test");
        let p;
        {
            let t = TempDir::new(&root, "unit").unwrap();
            p = t.path().to_path_buf();
            std::fs::create_dir_all(p.join("a")).unwrap();
            std::fs::write(p.join("a/x.bin"), [0u8; 100]).unwrap();
            assert_eq!(dir_size(&p), 100);
        }
        assert!(!p.exists());
    }

    #[test]
    fn stale_cleanup_removes_dead_pid_dirs() {
        let root = std::env::temp_dir().join(format!("enhancer-stale-{}", std::process::id()));
        // PID 4294967 is extremely unlikely to exist.
        let dead = root.join("4294967-job-1");
        let mine = root.join(format!("{}-job-1", std::process::id()));
        std::fs::create_dir_all(&dead).unwrap();
        std::fs::create_dir_all(&mine).unwrap();
        cleanup_stale(&root);
        assert!(!dead.exists());
        assert!(mine.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn free_space_of_temp_is_known() {
        assert!(free_space(&std::env::temp_dir()).is_some());
    }

    #[test]
    fn estimate_scales_with_settings() {
        let a = estimate_temp_bytes(1920, 1080, 3840, 2160, 120, 1, true);
        let b = estimate_temp_bytes(1920, 1080, 3840, 2160, 120, 2, true);
        assert!(b > a);
        assert!(estimate_temp_bytes(1920, 1080, 1920, 1080, 120, 1, false) < a);
    }
}
