//! Scene-cut detection and interpolation slot filling.
//!
//! The detector compares a 64×36 luma thumbnail of consecutive frames using the mean
//! absolute difference plus a 16-bin histogram distance. When a cut is found between
//! frame `j` and `j+1`, no frames are interpolated across it: the generated slots are
//! filled with frame `j` instead (see [`slot_source`]).

use image::RgbImage;

const SIG_W: usize = 64;
const SIG_H: usize = 36;
const BINS: usize = 16;

#[derive(Clone, Debug)]
pub struct Signature {
    luma: Vec<u8>,
    hist: [u32; BINS],
}

/// Downscaled luma thumbnail (box-sampled) + histogram.
pub fn signature(img: &RgbImage) -> Signature {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let mut luma = Vec::with_capacity(SIG_W * SIG_H);
    let mut hist = [0u32; BINS];
    for sy in 0..SIG_H {
        let y0 = sy * h / SIG_H;
        let y1 = ((sy + 1) * h / SIG_H).max(y0 + 1).min(h);
        for sx in 0..SIG_W {
            let x0 = sx * w / SIG_W;
            let x1 = ((sx + 1) * w / SIG_W).max(x0 + 1).min(w);
            // Sample a sparse 4x4 grid inside the cell (fast for large frames).
            let mut acc = 0u32;
            let mut n = 0u32;
            let sy_step = ((y1 - y0) / 4).max(1);
            let sx_step = ((x1 - x0) / 4).max(1);
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let i = (y * w + x) * 3;
                    acc += (raw[i] as u32 * 77 + raw[i + 1] as u32 * 150 + raw[i + 2] as u32 * 29)
                        >> 8;
                    n += 1;
                    x += sx_step;
                }
                y += sy_step;
            }
            let l = (acc / n.max(1)) as u8;
            luma.push(l);
            hist[l as usize * BINS / 256] += 1;
        }
    }
    Signature { luma, hist }
}

/// Difference score in 0..=1 (≈0 for identical, high for hard cuts).
pub fn score(a: &Signature, b: &Signature) -> f32 {
    let n = a.luma.len().max(1) as f32;
    let mad: f32 = a
        .luma
        .iter()
        .zip(&b.luma)
        .map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs() as f32)
        .sum::<f32>()
        / n
        / 255.0;
    // 1-D earth mover's distance between the histograms (robust to small brightness drift
    // that moves mass into a neighbouring bin, unlike a bin-wise L1 distance).
    let mut ca = 0f32;
    let mut cb = 0f32;
    let mut emd = 0f32;
    for (&x, &y) in a.hist.iter().zip(&b.hist) {
        ca += x as f32 / n;
        cb += y as f32 / n;
        emd += (ca - cb).abs();
    }
    emd /= BINS as f32;
    0.5 * (mad * 2.5).min(1.0) + 0.5 * (emd * 4.0).min(1.0)
}

/// Streaming detector: feed frames in order.
pub struct SceneDetector {
    threshold: f32,
    prev: Option<Signature>,
}

impl SceneDetector {
    pub fn new(threshold: f32) -> SceneDetector {
        SceneDetector {
            threshold,
            prev: None,
        }
    }

    /// Returns `true` if there is a cut between the previous frame and this one.
    pub fn push(&mut self, img: &RgbImage) -> bool {
        let sig = signature(img);
        let cut = self
            .prev
            .as_ref()
            .is_some_and(|p| score(p, &sig) > self.threshold);
        self.prev = Some(sig);
        cut
    }
}

/// Where an output slot's frame comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotSource {
    /// Interpolator output index (0-based) — real frame when `slot % k == 0`, generated otherwise.
    Interp(u64),
    /// Duplicate of interpolator output index (the frame before a cut).
    Dup(u64),
}

impl SlotSource {
    pub fn index(self) -> u64 {
        match self {
            SlotSource::Interp(i) | SlotSource::Dup(i) => i,
        }
    }
}

/// For chunk-local output slot `slot` with multiplier `k`: generated slots between frame `j`
/// and `j+1` (where `cut_after(j)`) are filled with frame `j` (= interpolator output `j*k`).
pub fn slot_source(slot: u64, k: u32, cut_after: impl Fn(u64) -> bool) -> SlotSource {
    let k = k.max(1) as u64;
    let j = slot / k;
    let m = slot % k;
    if m != 0 && cut_after(j) {
        SlotSource::Dup(j * k)
    } else {
        SlotSource::Interp(slot)
    }
}

/// Plan all slots of a chunk producing `n_frames * k` outputs; `cuts` are local pair indices.
pub fn fill_plan(n_frames: u64, k: u32, cuts: &[u64]) -> Vec<SlotSource> {
    (0..n_frames * k.max(1) as u64)
        .map(|s| slot_source(s, k, |j| cuts.contains(&j)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(seed: u8, shift: u32) -> RgbImage {
        RgbImage::from_fn(320, 180, |x, y| {
            let v = ((x + shift) / 4 + y / 3) as u8;
            image::Rgb([v.wrapping_add(seed), v, v / 2])
        })
    }

    fn flat(v: u8) -> RgbImage {
        RgbImage::from_pixel(320, 180, image::Rgb([v, v, v]))
    }

    #[test]
    fn detects_hard_cut_not_motion() {
        let mut d = SceneDetector::new(0.30);
        assert!(!d.push(&gradient(0, 0))); // first frame: never a cut
        assert!(!d.push(&gradient(0, 4))); // small pan
        assert!(!d.push(&gradient(0, 8)));
        assert!(d.push(&flat(240))); // hard cut to a bright flat frame
        assert!(!d.push(&flat(238))); // same scene
        assert!(d.push(&flat(10))); // cut to black
    }

    #[test]
    fn identical_frames_score_zero() {
        let a = signature(&gradient(3, 0));
        assert!(score(&a, &a) < 1e-6);
        let b = signature(&flat(0));
        let c = signature(&flat(255));
        assert!(score(&b, &c) > 0.9);
    }

    #[test]
    fn slot_filling_duplicates_before_cut() {
        // 3 frames, 2x, cut between frame 1 and 2.
        let plan = fill_plan(3, 2, &[1]);
        assert_eq!(
            plan,
            vec![
                SlotSource::Interp(0),
                SlotSource::Interp(1),
                SlotSource::Interp(2),
                SlotSource::Dup(2), // would interpolate 1→2 across the cut
                SlotSource::Interp(4),
                SlotSource::Interp(5),
            ]
        );
        // 4x, cut after frame 0.
        let plan = fill_plan(2, 4, &[0]);
        assert_eq!(
            &plan[..4],
            &[
                SlotSource::Interp(0),
                SlotSource::Dup(0),
                SlotSource::Dup(0),
                SlotSource::Dup(0)
            ]
        );
        assert_eq!(plan[4], SlotSource::Interp(4));
        // No cuts: identity.
        assert!(
            fill_plan(5, 3, &[])
                .iter()
                .enumerate()
                .all(|(i, s)| *s == SlotSource::Interp(i as u64))
        );
        // k = 1: never duplicates.
        assert_eq!(
            fill_plan(2, 1, &[0]),
            vec![SlotSource::Interp(0), SlotSource::Interp(1)]
        );
    }
}
