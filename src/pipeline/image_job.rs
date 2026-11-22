//! Single-image export.

use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};
use std::path::Path;

use super::cancel::CancelToken;
use super::job::{Env, JobOutcome, JobSpec, PipelineError, ProgressHandle, Stage};
use super::stages::StagePlan;
use super::temp::{TempDir, temp_root};
use crate::config::ImageFormat;
use crate::engines::resize::{ResizeKind, resize_gray};

/// Load any supported image as RGB + optional alpha.
pub fn load_rgb_alpha(path: &Path) -> Result<(RgbImage, Option<GrayImage>), PipelineError> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    let alpha = img.color().has_alpha().then(|| {
        let rgba = img.to_rgba8();
        GrayImage::from_raw(
            rgba.width(),
            rgba.height(),
            rgba.pixels().map(|p| p[3]).collect(),
        )
        .expect("size")
    });
    Ok((img.into_rgb8(), alpha))
}

/// Resolve `Same` to a concrete output format.
pub fn resolve_format(fmt: ImageFormat, src: &Path) -> ImageFormat {
    if fmt != ImageFormat::Same {
        return fmt;
    }
    match src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => ImageFormat::Jpeg,
        Some("webp") => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

pub fn save_image(
    rgb: RgbImage,
    alpha: Option<GrayImage>,
    fmt: ImageFormat,
    quality: u8,
    path: &Path,
) -> Result<(), PipelineError> {
    use image::ImageEncoder;
    let file = std::fs::File::create(path)?;
    let w = std::io::BufWriter::new(file);
    let dynimg = match (alpha, fmt) {
        (Some(a), ImageFormat::Png | ImageFormat::Webp) => {
            let mut rgba = RgbaImage::new(rgb.width(), rgb.height());
            for ((o, p), al) in rgba.pixels_mut().zip(rgb.pixels()).zip(a.pixels()) {
                *o = image::Rgba([p[0], p[1], p[2], al[0]]);
            }
            DynamicImage::ImageRgba8(rgba)
        }
        _ => DynamicImage::ImageRgb8(rgb),
    };
    match fmt {
        ImageFormat::Jpeg => {
            image::codecs::jpeg::JpegEncoder::new_with_quality(w, quality.clamp(1, 100))
                .encode_image(&dynimg)?;
        }
        ImageFormat::Webp => {
            // The `image` crate only has a lossless WebP encoder.
            image::codecs::webp::WebPEncoder::new_lossless(w).write_image(
                dynimg.as_bytes(),
                dynimg.width(),
                dynimg.height(),
                dynimg.color().into(),
            )?;
        }
        ImageFormat::Png | ImageFormat::Same => {
            image::codecs::png::PngEncoder::new(w).write_image(
                dynimg.as_bytes(),
                dynimg.width(),
                dynimg.height(),
                dynimg.color().into(),
            )?;
        }
    }
    Ok(())
}

pub fn run_image(
    env: &Env,
    spec: &JobSpec,
    progress: &ProgressHandle,
    cancel: &CancelToken,
) -> Result<JobOutcome, PipelineError> {
    let s = &spec.settings;
    progress.update(|p| p.frames_total = 1);
    let plan = StagePlan::build(env, s, &spec.info)?;
    for w in &plan.warnings {
        tracing::warn!("{}: {w}", spec.info.path.display());
    }
    progress.update(|p| p.device = plan.device_label.clone());
    let root = temp_root(s.video.temp_dir.as_deref());
    let work = TempDir::new(&root, &format!("img{}", spec.id))?;

    progress.stage(Stage::Extract);
    let (rgb, alpha) = load_rgb_alpha(&spec.info.path)?;
    progress.stage(if plan.needs_disk() {
        Stage::Upscale
    } else {
        Stage::Sharpen
    });
    let out = plan.process_still(rgb, work.path(), cancel)?;
    let alpha = alpha.map(|a| resize_gray(&a, out.width(), out.height(), ResizeKind::Lanczos3));

    progress.stage(Stage::Encode);
    let fmt = resolve_format(s.output.image_format, &spec.info.path);
    if let Some(dir) = spec.output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = spec.output.with_extension(format!("part.{}", fmt.ext()));
    let res = save_image(out, alpha, fmt, s.output.image_quality, &part);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if spec.output.exists() {
        std::fs::remove_file(&spec.output)?;
    }
    std::fs::rename(&part, &spec.output)?;
    progress.frames(1);
    let mut note = plan.describe();
    for w in plan.warnings {
        note += &format!(" · ⚠ {w}");
    }
    Ok(JobOutcome::Done {
        output: spec.output.clone(),
        note,
    })
}
