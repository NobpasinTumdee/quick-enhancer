//! Resizing with `fast_image_resize` (SIMD, rayon-parallel).

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::{GrayImage, RgbImage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeKind {
    Lanczos3,
    Nearest,
}

fn options(kind: ResizeKind) -> ResizeOptions {
    ResizeOptions::new().resize_alg(match kind {
        ResizeKind::Lanczos3 => ResizeAlg::Convolution(FilterType::Lanczos3),
        ResizeKind::Nearest => ResizeAlg::Nearest,
    })
}

fn resize_raw(
    src: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    pt: PixelType,
    kind: ResizeKind,
) -> Vec<u8> {
    let src_img = ImageRef::new(sw, sh, src, pt).expect("valid source buffer");
    let mut dst = Image::new(dw, dh, pt);
    let mut r = Resizer::new();
    r.resize(&src_img, &mut dst, &options(kind))
        .expect("same pixel type");
    dst.into_vec()
}

pub fn resize_rgb(img: &RgbImage, w: u32, h: u32, kind: ResizeKind) -> RgbImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    let v = resize_raw(
        img.as_raw(),
        img.width(),
        img.height(),
        w,
        h,
        PixelType::U8x3,
        kind,
    );
    RgbImage::from_raw(w, h, v).expect("size")
}

pub fn resize_gray(img: &GrayImage, w: u32, h: u32, kind: ResizeKind) -> GrayImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    let v = resize_raw(
        img.as_raw(),
        img.width(),
        img.height(),
        w,
        h,
        PixelType::U8,
        kind,
    );
    GrayImage::from_raw(w, h, v).expect("size")
}

/// Fit `(w, h)` inside `max_side` keeping aspect ratio (for display textures).
pub fn fit_within(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let s = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    (
        ((w as f64 * s).round() as u32).max(1),
        ((h as f64 * s).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_integer_scale_replicates_pixels() {
        let img = RgbImage::from_fn(2, 2, |x, y| {
            image::Rgb([(x * 100) as u8, (y * 100) as u8, 7])
        });
        let up = resize_rgb(&img, 6, 6, ResizeKind::Nearest);
        for y in 0..6 {
            for x in 0..6 {
                assert_eq!(up.get_pixel(x, y), img.get_pixel(x / 3, y / 3));
            }
        }
    }

    #[test]
    fn lanczos_dims_and_flat_color() {
        let img = RgbImage::from_pixel(10, 6, image::Rgb([50, 100, 150]));
        let r = resize_rgb(&img, 25, 15, ResizeKind::Lanczos3);
        assert_eq!(r.dimensions(), (25, 15));
        assert!(r.pixels().all(|p| p.0 == [50, 100, 150]));
    }

    #[test]
    fn fit() {
        assert_eq!(fit_within(4000, 2000, 1000, 1000), (1000, 500));
        assert_eq!(fit_within(100, 50, 1000, 1000), (100, 50));
    }
}
