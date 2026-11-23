//! Does a model fit this PC? Compares the catalog's VRAM needs with the GPU and estimates the
//! run time from the catalog cost (seconds per input megapixel on the reference PC).

use super::catalog::{ModelEntry, RuntimeNeed};
use crate::devices::{GpuInfo, GpuKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Enough VRAM for the recommended tile size.
    Good,
    /// Runs with smaller tiles (slower).
    Tight,
    /// Below the minimum VRAM: likely to fail or crawl.
    TooBig,
    /// Needs an NVIDIA GPU (CUDA) that this PC doesn't have.
    NeedsNvidia,
    /// No usable GPU found.
    NoGpu,
}

impl Fit {
    pub fn label_key(self) -> &'static str {
        match self {
            Fit::Good => "models.fit_good",
            Fit::Tight => "models.fit_tight",
            Fit::TooBig => "models.fit_too_big",
            Fit::NeedsNvidia => "models.fit_needs_nvidia",
            Fit::NoGpu => "models.fit_no_gpu",
        }
    }
    /// Can the user run it at all (possibly slowly)?
    pub fn runnable(self) -> bool {
        matches!(self, Fit::Good | Fit::Tight | Fit::TooBig)
    }
}

pub const NVIDIA_VENDOR: u32 = 0x10de;

/// Fit of `m` on the best GPU in `gpus` (largest VRAM; NVIDIA only for CUDA models).
pub fn fit(m: &ModelEntry, gpus: &[GpuInfo]) -> Fit {
    let usable = |g: &&GpuInfo| g.kind != GpuKind::Cpu;
    let needs_cuda = m.needs_runtime() == Some(RuntimeNeed::Cuda);
    let best = gpus
        .iter()
        .filter(usable)
        .filter(|g| !needs_cuda || g.vendor_id == NVIDIA_VENDOR)
        .max_by_key(|g| g.vram_bytes);
    let Some(g) = best else {
        return if needs_cuda && gpus.iter().any(|g| usable(&g)) {
            Fit::NeedsNvidia
        } else {
            Fit::NoGpu
        };
    };
    fit_vram(m, g.vram_bytes / (1024 * 1024))
}

pub fn fit_vram(m: &ModelEntry, vram_mb: u64) -> Fit {
    if vram_mb >= m.vram_rec_mb as u64 {
        Fit::Good
    } else if vram_mb >= m.vram_min_mb as u64 {
        Fit::Tight
    } else {
        Fit::TooBig
    }
}

/// Estimated AI time in seconds for `megapixels` of input (per frame for video), on the
/// reference PC. `None` when the catalog has no cost (face models: per face).
pub fn estimate_seconds(m: &ModelEntry, megapixels: f64) -> Option<f64> {
    (m.cost.s_per_mp > 0.0).then_some(m.cost.s_per_mp as f64 * megapixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::catalog::Catalog;

    fn gpu(vendor: u32, vram_mb: u64) -> GpuInfo {
        GpuInfo {
            index: 0,
            name: "test".into(),
            vendor_id: vendor,
            kind: GpuKind::Discrete,
            vram_bytes: vram_mb * 1024 * 1024,
            uuid: [0; 16],
            luid: None,
            fp16: true,
        }
    }

    #[test]
    fn fit_levels() {
        let c = Catalog::embedded();
        let dat = c.model("dat2-nomos2-4x").unwrap();
        assert_eq!(fit(dat, &[gpu(0x1002, 8192)]), Fit::Good);
        assert_eq!(fit(dat, &[gpu(0x1002, 2500)]), Fit::Tight);
        assert_eq!(fit(dat, &[gpu(0x8086, 1024)]), Fit::TooBig);
        assert_eq!(fit(dat, &[]), Fit::NoGpu);
        let seed = c.model("seedvr2-3b").unwrap();
        assert_eq!(fit(seed, &[gpu(0x1002, 24000)]), Fit::NeedsNvidia);
        assert_eq!(fit(seed, &[gpu(NVIDIA_VENDOR, 16000)]), Fit::Good);
        assert!(!Fit::NeedsNvidia.runnable() && Fit::TooBig.runnable());
    }

    #[test]
    fn estimates() {
        let c = Catalog::embedded();
        let span = c.model("span-nomosuni-2x").unwrap();
        let t = estimate_seconds(span, 2.0736).unwrap(); // 1080p frame
        assert!((0.5..1.0).contains(&t), "{t}");
        assert!(estimate_seconds(c.model("gfpgan-1.4").unwrap(), 1.0).is_none());
    }
}
