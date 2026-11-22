//! Pure planning math: output size / model scale, frame-generation multiplier,
//! chunking, performance preset mapping and tile size.

use crate::config::{FrameGenMode, JobSettings, Preset, Priority, ScaleMode, TileSize};
use crate::media::Rational;

/// How a frame gets from source size to output size.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalePlan {
    pub src_w: u32,
    pub src_h: u32,
    pub out_w: u32,
    pub out_h: u32,
    /// AI model scale to run (None = no AI upscale stage).
    pub model_scale: Option<u32>,
    /// A final exact resize is needed after the model (or instead of it).
    pub needs_resize: bool,
    /// Human-readable explanation, e.g. "4x model → downscaled to 3x".
    pub note: String,
}

fn round_even(v: f64) -> u32 {
    let r = (v / 2.0).round() as u32 * 2;
    r.max(2)
}

/// Compute output dimensions for a scale mode. Target modes set the *short side*.
pub fn output_dims(src_w: u32, src_h: u32, mode: ScaleMode, even: bool) -> (u32, u32) {
    let (w, h) = match mode {
        ScaleMode::X1 => (src_w as f64, src_h as f64),
        ScaleMode::X2 => (src_w as f64 * 2.0, src_h as f64 * 2.0),
        ScaleMode::X3 => (src_w as f64 * 3.0, src_h as f64 * 3.0),
        ScaleMode::X4 => (src_w as f64 * 4.0, src_h as f64 * 4.0),
        ScaleMode::Target(short) => {
            let s = short as f64 / src_w.min(src_h).max(1) as f64;
            (src_w as f64 * s, src_h as f64 * s)
        }
    };
    if even {
        (round_even(w), round_even(h))
    } else {
        ((w.round() as u32).max(1), (h.round() as u32).max(1))
    }
}

/// Plan the scale stage. `native` = scales the AI model supports (empty = no AI).
pub fn plan_scale(
    src_w: u32,
    src_h: u32,
    mode: ScaleMode,
    native: &[u32],
    even: bool,
) -> ScalePlan {
    let (out_w, out_h) = output_dims(src_w, src_h, mode, even);
    let factor = out_w as f64 / src_w.max(1) as f64;
    let factor_label = |f: f64| {
        if (f - f.round()).abs() < 0.01 {
            format!("{}x", f.round())
        } else {
            format!("{f:.2}x")
        }
    };
    if native.is_empty() {
        let needs_resize = (out_w, out_h) != (src_w, src_h);
        return ScalePlan {
            src_w,
            src_h,
            out_w,
            out_h,
            model_scale: None,
            needs_resize,
            note: if needs_resize {
                format!("{} → {out_w}x{out_h}", factor_label(factor))
            } else {
                crate::t!("plan.no_resize")
            },
        };
    }
    let mut sorted = native.to_vec();
    sorted.sort_unstable();
    // Smallest native scale ≥ requested factor; if none, the largest available.
    let model = sorted
        .iter()
        .copied()
        .find(|&s| s as f64 >= factor - 1e-6)
        .unwrap_or(*sorted.last().expect("non-empty"));
    let model_w = src_w * model;
    let model_h = src_h * model;
    let needs_resize = (model_w, model_h) != (out_w, out_h);
    let note = if !needs_resize {
        crate::t!("plan.model", k = model)
    } else if (model as f64) > factor {
        crate::t!("plan.model_down", k = model, f = factor_label(factor))
    } else {
        crate::t!("plan.model_up", k = model, f = factor_label(factor))
    };
    ScalePlan {
        src_w,
        src_h,
        out_w,
        out_h,
        model_scale: Some(model),
        needs_resize,
        note,
    }
}

/// Frame-generation plan.
#[derive(Clone, Debug, PartialEq)]
pub struct FpsPlan {
    pub multiplier: u32,
    pub src_fps: Rational,
    pub out_fps: Rational,
    pub note: Option<String>,
}

/// Integer multiplier only; target FPS picks the largest `k` with `src·k ≤ min(target, max)`.
pub fn plan_fps(src: Rational, mode: FrameGenMode, max_fps: u32) -> FpsPlan {
    let s = src.as_f64().max(0.001);
    let cap = max_fps.max(1) as f64 + 1e-6;
    let (want, mut note) = match mode {
        FrameGenMode::Off => (1, None),
        FrameGenMode::X2 => (2, None),
        FrameGenMode::X3 => (3, None),
        FrameGenMode::X4 => (4, None),
        FrameGenMode::TargetFps(t) => {
            let target = (t as f64).min(cap);
            let k = ((target + 1e-6) / s).floor().max(1.0) as u32;
            let note = (k > 1 && (s * k as f64 - t as f64).abs() > 0.05)
                .then(|| crate::t!("plan.target_fps", t = t, k = k, fps = format!("{:.3}", s * k as f64)));
            (k, note)
        }
    };
    let mut k = want;
    while k > 1 && s * k as f64 > cap {
        k -= 1;
    }
    if k < want && !matches!(mode, FrameGenMode::TargetFps(_)) {
        note = Some(crate::t!("plan.exceeds_max", want = want, max = max_fps, k = k));
    }
    if k == 1 && !matches!(mode, FrameGenMode::Off) && note.is_none() {
        note = Some(crate::t!("plan.already_at_target", max = max_fps));
    }
    FpsPlan {
        multiplier: k,
        src_fps: src,
        out_fps: src.times(k),
        note,
    }
}

/// One chunk of source frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub index: usize,
    /// First source frame index.
    pub start: u64,
    /// Number of source frames owned by this chunk.
    pub len: u64,
    /// The first frame of the next chunk is appended (for interpolation across the boundary).
    pub overlap: bool,
}

impl Chunk {
    /// Number of frames given to the interpolator (owned + overlap).
    pub fn input_frames(&self) -> u64 {
        self.len + self.overlap as u64
    }
    /// Interpolator output count requested for this chunk.
    pub fn interp_request(&self, k: u32) -> u64 {
        self.input_frames() * k as u64
    }
    /// Output frames this chunk contributes to the final video.
    pub fn output_frames(&self, k: u32) -> u64 {
        self.len * k as u64
    }
}

/// Split `total` frames into chunks of `size`; overlap on all but the last when interpolating.
pub fn plan_chunks(total: u64, size: u32, interpolate: bool) -> Vec<Chunk> {
    let size = size.max(2) as u64;
    let mut v = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while start < total {
        let len = size.min(total - start);
        let last = start + len >= total;
        v.push(Chunk {
            index,
            start,
            len,
            overlap: interpolate && !last,
        });
        start += len;
        index += 1;
    }
    v
}

/// Effective performance resources after preset + overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resources {
    pub tile: TileSize,
    /// Upper bound for an `Auto` tile (Balanced caps at 256).
    pub tile_cap: Option<u32>,
    pub jobs: String,
    pub filter_threads: usize,
    pub priority: Priority,
}

pub fn preset_defaults(preset: Preset, cores: usize) -> Resources {
    let cores = cores.max(1);
    match preset {
        Preset::Eco => Resources {
            tile: TileSize::Fixed(128),
            tile_cap: None,
            jobs: "1:1:1".into(),
            filter_threads: (cores / 4).max(1),
            priority: Priority::BelowNormal,
        },
        Preset::Balanced => Resources {
            tile: TileSize::Auto,
            tile_cap: Some(256),
            jobs: "1:2:2".into(),
            filter_threads: (cores / 2).max(1),
            priority: Priority::Normal,
        },
        Preset::Max => Resources {
            tile: TileSize::Auto,
            tile_cap: None,
            jobs: "2:4:4".into(),
            filter_threads: cores.saturating_sub(1).max(1),
            priority: Priority::AboveNormal,
        },
    }
}

pub fn effective_resources(s: &JobSettings, cores: usize) -> Resources {
    let mut r = preset_defaults(s.preset, cores);
    if let Some(t) = s.tile_override {
        r.tile = t;
        r.tile_cap = None;
    }
    if let Some(j) = s.jobs_override.as_ref().filter(|j| valid_jobs(j)) {
        r.jobs = j.clone();
    }
    if let Some(t) = s.filter_threads_override {
        r.filter_threads = (t as usize).clamp(1, cores.max(1));
    }
    if s.compute == crate::config::ComputeDevice::Cpu && s.cpu_threads > 0 {
        r.filter_threads = (s.cpu_threads as usize).clamp(1, cores.max(1));
    }
    if let Some(p) = s.priority_override {
        r.priority = p;
    }
    r
}

/// `load:proc:save` with positive integers.
pub fn valid_jobs(j: &str) -> bool {
    let parts: Vec<_> = j.split(':').collect();
    parts.len() == 3 && parts.iter().all(|p| p.parse::<u32>().is_ok_and(|v| v > 0))
}

/// Tile size from free VRAM (bytes).
pub fn tile_from_vram(free: Option<u64>) -> u32 {
    const GB: u64 = 1 << 30;
    match free {
        None => 256,
        // Capped at 256: larger tiles of the heavy x4plus model can run a single GPU
        // dispatch long enough to trigger a Windows driver reset (TDR / device lost).
        // realesrgan-ncnn-vulkan itself never picks more than 200.
        Some(f) if f >= 3 * GB => 256,
        Some(f) if f >= 3 * GB / 2 => 200,
        Some(f) if f >= 7 * GB / 10 => 128,
        Some(_) => 64,
    }
}

pub fn resolve_tile(r: &Resources, free_vram: Option<u64>) -> u32 {
    match r.tile {
        TileSize::Fixed(t) => t.max(32),
        TileSize::Auto => {
            let t = tile_from_vram(free_vram);
            r.tile_cap.map_or(t, |c| t.min(c))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_math() {
        let p = plan_scale(640, 360, ScaleMode::X2, &[4], true);
        assert_eq!((p.out_w, p.out_h), (1280, 720));
        assert_eq!(p.model_scale, Some(4));
        assert!(p.needs_resize);
        assert!(p.note.contains("4x model → downscaled to 2x"), "{}", p.note);

        let p = plan_scale(640, 360, ScaleMode::X3, &[2, 3, 4], true);
        assert_eq!(p.model_scale, Some(3));
        assert!(!p.needs_resize);

        let p = plan_scale(640, 360, ScaleMode::X3, &[4], false);
        assert!(p.note.contains("downscaled to 3x"));

        // 1x with AI = restore at smallest native scale, then back down.
        let p = plan_scale(640, 360, ScaleMode::X1, &[2, 4], true);
        assert_eq!(p.model_scale, Some(2));
        assert_eq!((p.out_w, p.out_h), (640, 360));

        // No AI.
        let p = plan_scale(640, 360, ScaleMode::X1, &[], true);
        assert_eq!(p.model_scale, None);
        assert!(!p.needs_resize);
    }

    #[test]
    fn target_resolution_math() {
        // 16:9 to 1080p.
        assert_eq!(
            output_dims(1280, 720, ScaleMode::Target(1080), true),
            (1920, 1080)
        );
        // Portrait: short side is the width.
        assert_eq!(
            output_dims(720, 1280, ScaleMode::Target(1080), true),
            (1080, 1920)
        );
        // Odd aspect → even dims for video.
        let (w, h) = output_dims(853, 480, ScaleMode::Target(720), true);
        assert_eq!((w % 2, h % 2), (0, 0));
        assert_eq!(h, 720);
        assert_eq!(w, 1280);
        // 4K from 480p needs 4.5x → largest model then Lanczos up.
        let p = plan_scale(854, 480, ScaleMode::Target(2160), &[4], true);
        assert_eq!(p.model_scale, Some(4));
        assert!(p.note.contains("Lanczos up"), "{}", p.note);
        // Images keep exact (non-even) rounding.
        assert_eq!(output_dims(101, 51, ScaleMode::X2, false), (202, 102));
    }

    #[test]
    fn fps_math() {
        let p = plan_fps(Rational::new(30, 1), FrameGenMode::X2, 120);
        assert_eq!((p.multiplier, p.out_fps), (2, Rational::new(60, 1)));
        let p = plan_fps(Rational::new(60, 1), FrameGenMode::X4, 120);
        assert_eq!(p.multiplier, 2);
        assert!(p.note.is_some());
        let p = plan_fps(Rational::new(24, 1), FrameGenMode::TargetFps(60), 120);
        assert_eq!(p.multiplier, 2);
        assert_eq!(p.out_fps, Rational::new(48, 1));
        let p = plan_fps(Rational::new(30000, 1001), FrameGenMode::TargetFps(60), 120);
        assert_eq!(p.multiplier, 2);
        assert_eq!(p.out_fps, Rational::new(60000, 1001));
        let p = plan_fps(Rational::new(30, 1), FrameGenMode::TargetFps(120), 120);
        assert_eq!(p.multiplier, 4);
        let p = plan_fps(Rational::new(30, 1), FrameGenMode::Off, 120);
        assert_eq!(p.multiplier, 1);
        assert!(p.note.is_none());
    }

    #[test]
    fn chunk_planner() {
        let c = plan_chunks(250, 120, true);
        assert_eq!(c.len(), 3);
        assert_eq!((c[0].start, c[0].len, c[0].overlap), (0, 120, true));
        assert_eq!((c[1].start, c[1].len, c[1].overlap), (120, 120, true));
        assert_eq!((c[2].start, c[2].len, c[2].overlap), (240, 10, false));
        assert_eq!(c.iter().map(|c| c.len).sum::<u64>(), 250);
        assert_eq!(c[0].interp_request(2), 242);
        assert_eq!(c[0].output_frames(2), 240);
        assert_eq!(c[2].interp_request(2), 20);
        let c = plan_chunks(240, 120, false);
        assert_eq!(c.len(), 2);
        assert!(c.iter().all(|c| !c.overlap));
        assert!(plan_chunks(0, 120, true).is_empty());
    }

    #[test]
    fn preset_mapping() {
        let eco = preset_defaults(Preset::Eco, 16);
        assert_eq!(eco.tile, TileSize::Fixed(128));
        assert_eq!(eco.jobs, "1:1:1");
        assert_eq!(eco.filter_threads, 4);
        assert_eq!(eco.priority, Priority::BelowNormal);
        let bal = preset_defaults(Preset::Balanced, 16);
        assert_eq!((bal.tile, bal.tile_cap), (TileSize::Auto, Some(256)));
        assert_eq!(bal.jobs, "1:2:2");
        assert_eq!(bal.filter_threads, 8);
        let max = preset_defaults(Preset::Max, 16);
        assert_eq!(max.jobs, "2:4:4");
        assert_eq!(max.filter_threads, 15);
        assert_eq!(max.priority, Priority::AboveNormal);
        assert_eq!(preset_defaults(Preset::Eco, 1).filter_threads, 1);

        let s = JobSettings {
            preset: Preset::Eco,
            tile_override: Some(TileSize::Fixed(64)),
            jobs_override: Some("bad".into()),
            filter_threads_override: Some(100),
            priority_override: Some(Priority::AboveNormal),
            ..Default::default()
        };
        let r = effective_resources(&s, 8);
        assert_eq!(r.tile, TileSize::Fixed(64));
        assert_eq!(r.jobs, "1:1:1"); // invalid override ignored
        assert_eq!(r.filter_threads, 8);
        assert_eq!(r.priority, Priority::AboveNormal);
    }

    #[test]
    fn tile_resolution() {
        const GB: u64 = 1 << 30;
        let bal = preset_defaults(Preset::Balanced, 8);
        assert_eq!(resolve_tile(&bal, Some(12 * GB)), 256);
        let max = preset_defaults(Preset::Max, 8);
        assert_eq!(resolve_tile(&max, Some(12 * GB)), 256);
        assert_eq!(resolve_tile(&max, Some(2 * GB)), 200);
        assert_eq!(resolve_tile(&max, Some(GB)), 128);
        assert_eq!(resolve_tile(&max, None), 256);
        assert!(valid_jobs("1:2:2"));
        assert!(!valid_jobs("1:2"));
        assert!(!valid_jobs("0:2:2"));
    }
}
