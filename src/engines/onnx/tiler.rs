//! Tiled inference for image-to-image models: split the source into overlapping tiles (static
//! models get exactly `tile × tile` input, padded by reflection at the image edge), run the
//! model on each, and feather-blend the scaled tiles back together. Rows of tiles are blended
//! into a band buffer that is flushed as soon as no later tile can touch it, so memory stays
//! at about one row of output tiles instead of a full float copy of the output.

use image::RgbImage;

/// Tiling parameters for one model run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileSpec {
    /// Input tile edge in pixels.
    pub tile: u32,
    /// The model only accepts exactly `tile × tile` input.
    pub static_tile: bool,
    /// Overlap between neighbouring tiles (input pixels).
    pub overlap: u32,
    pub scale: u32,
    /// Dynamic models: pad tile sizes up to a multiple of this.
    pub multiple: u32,
}

/// One tile: the source region it covers and the padded size fed to the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Model input size (≥ w, h; padded by reflection).
    pub in_w: u32,
    pub in_h: u32,
}

/// Start positions of tiles along one axis of length `n`: every `tile - overlap`, the last one
/// aligned to the end so all tiles (except a single short one) have full length.
pub fn positions(n: u32, tile: u32, overlap: u32) -> Vec<u32> {
    if n <= tile {
        return vec![0];
    }
    let step = tile.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut p = 0;
    while p + tile < n {
        out.push(p);
        p += step;
    }
    out.push(n - tile);
    out.dedup();
    out
}

fn round_up(v: u32, m: u32) -> u32 {
    if m <= 1 { v } else { v.div_ceil(m) * m }
}

/// Tiles covering a `w × h` image, row by row.
pub fn grid(w: u32, h: u32, spec: &TileSpec) -> Vec<Tile> {
    let overlap = spec.overlap.min(spec.tile / 2);
    let xs = positions(w, spec.tile, overlap);
    let ys = positions(h, spec.tile, overlap);
    let mut out = Vec::with_capacity(xs.len() * ys.len());
    for &y in &ys {
        for &x in &xs {
            let tw = spec.tile.min(w);
            let th = spec.tile.min(h);
            let (in_w, in_h) = if spec.static_tile {
                (spec.tile, spec.tile)
            } else {
                (round_up(tw, spec.multiple), round_up(th, spec.multiple))
            };
            out.push(Tile { x, y, w: tw, h: th, in_w, in_h });
        }
    }
    out
}

/// Mirror an index into `0..n` (reflect without repeating the edge: -1 → 1, n → n-2).
pub fn reflect(i: i64, n: u32) -> u32 {
    let n = n as i64;
    if n <= 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let m = i.rem_euclid(period);
    (if m < n { m } else { period - m }) as u32
}

/// Feather weight of output pixel `d` (0-based) along an axis of a tile with output length
/// `len`. `ramp` output pixels fade in at a side that has a neighbour (`lo` / `hi`).
pub fn feather(d: u32, len: u32, ramp: u32, lo: bool, hi: bool) -> f32 {
    let mut w: f32 = 1.0;
    if ramp > 0 {
        if lo {
            w = w.min((d as f32 + 0.5) / ramp as f32);
        }
        if hi {
            w = w.min(((len - d) as f32 - 0.5) / ramp as f32);
        }
    }
    w.clamp(1e-3, 1.0)
}

/// Value mapping between 8-bit pixels and the model's float range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    Unit,
    PlusMinusOne,
}

impl Range {
    pub fn to_model(self, v: u8) -> f32 {
        match self {
            Range::Unit => v as f32 / 255.0,
            Range::PlusMinusOne => v as f32 / 127.5 - 1.0,
        }
    }
    pub fn from_model(self, v: f32) -> f32 {
        let u = match self {
            Range::Unit => v,
            Range::PlusMinusOne => (v + 1.0) * 0.5,
        };
        (u * 255.0).clamp(0.0, 255.0)
    }
}

/// Planar CHW input for one tile (reflect-padded to `in_w × in_h`).
pub fn extract(img: &RgbImage, t: &Tile, range: Range) -> Vec<f32> {
    let plane = (t.in_w * t.in_h) as usize;
    let mut out = vec![0f32; plane * 3];
    for j in 0..t.in_h {
        // Padding beyond the tile region mirrors back into the region.
        let ly = if j < t.h { j } else { reflect(j as i64, t.h) };
        for i in 0..t.in_w {
            let lx = if i < t.w { i } else { reflect(i as i64, t.w) };
            let p = img.get_pixel(t.x + lx, t.y + ly).0;
            let k = (j * t.in_w + i) as usize;
            for c in 0..3 {
                out[c * plane + k] = range.to_model(p[c]);
            }
        }
    }
    out
}

/// Blends model outputs into the final image, flushing finished rows.
pub struct Blender {
    out: RgbImage,
    scale: u32,
    ramp: u32,
    /// First output row held in `acc` / `wsum`.
    band_y0: u32,
    acc: Vec<f32>,
    wsum: Vec<f32>,
    out_w: u32,
}

impl Blender {
    pub fn new(src_w: u32, src_h: u32, scale: u32, overlap: u32) -> Blender {
        Blender {
            out: RgbImage::new(src_w * scale, src_h * scale),
            scale,
            ramp: overlap * scale,
            band_y0: 0,
            acc: Vec::new(),
            wsum: Vec::new(),
            out_w: src_w * scale,
        }
    }

    fn band_rows(&self) -> u32 {
        (self.wsum.len() / self.out_w as usize) as u32
    }

    fn ensure_rows(&mut self, end_y: u32) {
        let need = end_y.saturating_sub(self.band_y0);
        if need > self.band_rows() {
            let n = (need * self.out_w) as usize;
            self.wsum.resize(n, 0.0);
            self.acc.resize(n * 3, 0.0);
        }
    }

    /// Add one tile's model output (planar CHW, `in_w*scale × in_h*scale`).
    pub fn add(&mut self, t: &Tile, data: &[f32], range: Range, src_w: u32, src_h: u32) {
        let s = self.scale;
        let (ow, oh) = (t.w * s, t.h * s);
        let (pw, ph) = ((t.in_w * s) as usize, (t.in_h * s) as usize);
        let plane = pw * ph;
        let (ox, oy) = (t.x * s, t.y * s);
        self.ensure_rows(oy + oh);
        let lo_x = t.x > 0;
        let hi_x = t.x + t.w < src_w;
        let lo_y = t.y > 0;
        let hi_y = t.y + t.h < src_h;
        for j in 0..oh {
            let wy = feather(j, oh, self.ramp, lo_y, hi_y);
            let row = (oy + j - self.band_y0) as usize * self.out_w as usize;
            for i in 0..ow {
                let w = wy * feather(i, ow, self.ramp, lo_x, hi_x);
                let k = j as usize * pw + i as usize;
                let d = row + (ox + i) as usize;
                self.wsum[d] += w;
                for c in 0..3 {
                    self.acc[d * 3 + c] += w * range.from_model(data[c * plane + k]);
                }
            }
        }
    }

    /// All output rows above `y` (output pixels) are final: write them out.
    pub fn flush_until(&mut self, y: u32) {
        let y = y.min(self.out.height());
        if y <= self.band_y0 {
            return;
        }
        let rows = (y - self.band_y0).min(self.band_rows());
        let w = self.out_w as usize;
        for r in 0..rows as usize {
            for x in 0..w {
                let d = r * w + x;
                let ws = self.wsum[d].max(1e-6);
                let px = [
                    (self.acc[d * 3] / ws).round() as u8,
                    (self.acc[d * 3 + 1] / ws).round() as u8,
                    (self.acc[d * 3 + 2] / ws).round() as u8,
                ];
                self.out.put_pixel(x as u32, self.band_y0 + r as u32, image::Rgb(px));
            }
        }
        let n = rows as usize * w;
        self.wsum.drain(..n);
        self.acc.drain(..n * 3);
        self.band_y0 += rows;
    }

    pub fn finish(mut self) -> RgbImage {
        let h = self.out.height();
        self.flush_until(h);
        self.out
    }
}

/// Run `model` over all tiles of `img`. `model(tile_input_chw, in_w, in_h)` returns the planar
/// output (`in_w*scale × in_h*scale`). `on_tile(done, total)` reports progress.
pub fn run_tiled(
    img: &RgbImage,
    spec: &TileSpec,
    range: Range,
    mut model: impl FnMut(Vec<f32>, u32, u32) -> Result<Vec<f32>, String>,
    mut on_tile: impl FnMut(usize, usize) -> bool,
) -> Result<RgbImage, String> {
    let (w, h) = img.dimensions();
    let tiles = grid(w, h, spec);
    let overlap = spec.overlap.min(spec.tile / 2);
    let mut b = Blender::new(w, h, spec.scale, overlap);
    let total = tiles.len();
    for (n, t) in tiles.iter().enumerate() {
        let input = extract(img, t, range);
        let out = model(input, t.in_w, t.in_h)?;
        let expect = (t.in_w * spec.scale * t.in_h * spec.scale * 3) as usize;
        if out.len() != expect {
            return Err(format!(
                "model output has {} values, expected {expect} ({}x{} x{} tile)",
                out.len(),
                t.in_w,
                t.in_h,
                spec.scale
            ));
        }
        b.add(t, &out, range, w, h);
        // When the next tile starts a new row, rows above its top are final.
        if let Some(next) = tiles.get(n + 1)
            && next.y != t.y
        {
            b.flush_until(next.y * spec.scale);
        }
        if !on_tile(n + 1, total) {
            return Err("cancelled".into());
        }
    }
    Ok(b.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_cover_and_align() {
        assert_eq!(positions(100, 256, 16), vec![0]);
        assert_eq!(positions(256, 256, 16), vec![0]);
        assert_eq!(positions(300, 256, 16), vec![0, 44]);
        let p = positions(1000, 256, 32);
        assert_eq!(*p.last().unwrap(), 1000 - 256);
        for w in p.windows(2) {
            assert!(w[1] - w[0] <= 256 - 32, "gap too large: {w:?}");
        }
    }

    #[test]
    fn grid_static_and_dynamic() {
        let st = TileSpec { tile: 256, static_tile: true, overlap: 16, scale: 4, multiple: 1 };
        let g = grid(100, 50, &st);
        assert_eq!(g, vec![Tile { x: 0, y: 0, w: 100, h: 50, in_w: 256, in_h: 256 }]);
        let dy = TileSpec { static_tile: false, multiple: 8, ..st };
        let g = grid(100, 50, &dy);
        assert_eq!((g[0].in_w, g[0].in_h), (104, 56));
        let g = grid(600, 300, &dy);
        assert_eq!(g.len(), 3 * 2);
        assert!(g.iter().all(|t| t.x + t.w <= 600 && t.y + t.h <= 300));
    }

    #[test]
    fn reflect_indices() {
        assert_eq!(reflect(-1, 5), 1);
        assert_eq!(reflect(5, 5), 3);
        assert_eq!(reflect(6, 5), 2);
        assert_eq!(reflect(3, 5), 3);
        assert_eq!(reflect(7, 1), 0);
    }

    #[test]
    fn feather_weights() {
        // No neighbours → full weight everywhere.
        assert_eq!(feather(0, 64, 16, false, false), 1.0);
        // Ramps up from the low side and down to the high side.
        assert!(feather(0, 64, 16, true, false) < 0.1);
        assert_eq!(feather(20, 64, 16, true, true), 1.0);
        assert!(feather(63, 64, 16, false, true) < 0.1);
        // Two tiles overlapping by the ramp: weights sum to ~1 across the seam.
        for d in 0..16 {
            let a = feather(48 + d, 64, 16, false, true);
            let b = feather(d, 64, 16, true, false);
            assert!((a + b - 1.0).abs() < 0.07, "{d}: {a}+{b}");
        }
    }

    fn test_image(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7 % 256) as u8, (y * 3 % 256) as u8, ((x + y) % 256) as u8]))
    }

    /// Nearest-neighbour "model": the tiled result must equal a plain nearest upscale exactly,
    /// whatever the tiling, padding and blending.
    fn nearest_model(scale: u32) -> impl FnMut(Vec<f32>, u32, u32) -> Result<Vec<f32>, String> {
        move |input, w, h| {
            let (ow, oh) = (w * scale, h * scale);
            let plane_in = (w * h) as usize;
            let mut out = vec![0f32; (ow * oh * 3) as usize];
            for c in 0..3 {
                for y in 0..oh {
                    for x in 0..ow {
                        let v = input[c * plane_in + ((y / scale) * w + x / scale) as usize];
                        out[c * (ow * oh) as usize + (y * ow + x) as usize] = v;
                    }
                }
            }
            Ok(out)
        }
    }

    #[test]
    fn tiled_nearest_matches_reference() {
        let img = test_image(150, 97);
        for (spec, range) in [
            (TileSpec { tile: 64, static_tile: true, overlap: 8, scale: 2, multiple: 1 }, Range::Unit),
            (TileSpec { tile: 48, static_tile: false, overlap: 6, scale: 4, multiple: 8 }, Range::PlusMinusOne),
            (TileSpec { tile: 512, static_tile: true, overlap: 16, scale: 1, multiple: 1 }, Range::Unit),
        ] {
            let mut calls = 0;
            let out = run_tiled(&img, &spec, range, nearest_model(spec.scale), |_, _| {
                calls += 1;
                true
            })
            .unwrap();
            assert_eq!(out.dimensions(), (150 * spec.scale, 97 * spec.scale));
            assert_eq!(calls, grid(150, 97, &spec).len());
            for (x, y, p) in out.enumerate_pixels() {
                assert_eq!(*p, *img.get_pixel(x / spec.scale, y / spec.scale), "{spec:?} at {x},{y}");
            }
        }
    }

    #[test]
    fn wrong_output_size_and_cancel_are_errors() {
        let img = test_image(40, 40);
        let spec = TileSpec { tile: 32, static_tile: true, overlap: 4, scale: 2, multiple: 1 };
        assert!(run_tiled(&img, &spec, Range::Unit, |_, _, _| Ok(vec![0.0; 10]), |_, _| true).is_err());
        let r = run_tiled(&img, &spec, Range::Unit, nearest_model(2), |_, _| false);
        assert_eq!(r.unwrap_err(), "cancelled");
    }
}
