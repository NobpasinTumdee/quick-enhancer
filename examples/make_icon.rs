//! Regenerates the app icons from a square-ish logo with transparency:
//! `cargo run --release --example make_icon -- assets/logo.png`
//! Writes `assets/icon.ico` (16–256 px; BMP for ≤ 48 px, PNG above) and `assets/icon-256.png`
//! (window icon). Transparent margins are trimmed and the artwork is centred on a square.

use image::{RgbaImage, imageops::FilterType};
use std::io::Write;

const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn square(src: &RgbaImage) -> RgbaImage {
    // Bounding box of visible pixels.
    let (w, h) = src.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in src.enumerate_pixels() {
        if p[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    assert!(x1 >= x0 && y1 >= y0, "logo is fully transparent");
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let side = (bw.max(bh) as f32 * 1.04).ceil() as u32;
    let mut out = RgbaImage::new(side, side);
    let crop = image::imageops::crop_imm(src, x0, y0, bw, bh).to_image();
    image::imageops::overlay(&mut out, &crop, ((side - bw) / 2) as i64, ((side - bh) / 2) as i64);
    println!("visible box {bw}x{bh} at {x0},{y0} → square {side}");
    out
}

fn bmp_entry(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let mut v = Vec::new();
    // BITMAPINFOHEADER, height doubled (XOR + AND mask).
    for x in [40u32, w, h * 2] {
        v.extend(x.to_le_bytes());
    }
    v.extend(1u16.to_le_bytes());
    v.extend(32u16.to_le_bytes());
    v.extend([0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            v.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    let mask_row = w.div_ceil(32) * 4;
    v.extend(std::iter::repeat_n(0u8, (mask_row * h) as usize));
    v
}

fn main() {
    let src = std::env::args().nth(1).unwrap_or_else(|| "assets/logo.png".into());
    let logo = image::open(&src).expect("open logo").to_rgba8();
    let sq = square(&logo);
    let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();
    for s in SIZES {
        let img = image::imageops::resize(&sq, s, s, FilterType::Lanczos3);
        let data = if s <= 48 {
            bmp_entry(&img)
        } else {
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
            png
        };
        if s == 256 {
            img.save("assets/icon-256.png").unwrap();
        }
        entries.push((s, data));
    }
    let mut ico = Vec::new();
    ico.extend([0u8, 0, 1, 0]);
    ico.extend((entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len() as u32;
    for (s, data) in &entries {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        ico.extend([b, b, 0, 0]);
        ico.extend(1u16.to_le_bytes());
        ico.extend(32u16.to_le_bytes());
        ico.extend((data.len() as u32).to_le_bytes());
        ico.extend(offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &entries {
        ico.extend(data);
    }
    std::fs::File::create("assets/icon.ico").unwrap().write_all(&ico).unwrap();
    println!("assets/icon.ico: {} sizes, {} bytes", entries.len(), ico.len());
}
