//! Before / After preview: background renderer, zoom/pan/wipe canvas, sample-clip player.

pub mod audio;
pub mod canvas;
pub mod player;
pub mod worker;

use egui::ColorImage;
use image::RgbImage;

pub fn to_color_image(img: &RgbImage) -> ColorImage {
    ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw())
}
