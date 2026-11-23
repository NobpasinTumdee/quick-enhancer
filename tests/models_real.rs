//! Real downloads + real runs of catalog models (ignored by default: needs the network,
//! a Vulkan GPU and the Real-ESRGAN tool in `bin/`).
//!
//! `cargo test --test models_real -- --ignored --nocapture`

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use quick_enhancer::config::{ImageFormat, JobSettings, Profile, ScaleMode};
use quick_enhancer::devices;
use quick_enhancer::media::{encoders, probe};
use quick_enhancer::models::Catalog;
use quick_enhancer::net::tools::{Source, ToolsManifest, install_one};
use quick_enhancer::paths::{AppPaths, ModelSet, ToolSet};
use quick_enhancer::pipeline::cancel::CancelToken;
use quick_enhancer::pipeline::image_job::run_image;
use quick_enhancer::pipeline::job::{Env, JobOutcome, JobSpec, ProgressHandle};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Install catalog packages into a scratch tools root (downloads are reused between runs).
fn install(paths: &AppPaths, ids: &[&str]) {
    let cat = Catalog::embedded();
    let tools = ToolsManifest::embedded();
    for id in ids {
        let comp = cat.resolve_package(id, &tools).expect("package in catalog");
        if comp.is_present(&paths.base) {
            continue;
        }
        eprintln!("installing {id} ({} bytes)…", comp.size);
        install_one(paths, &comp, &Source::Download, &CancelToken::new(), &mut |_| {}).expect("install");
        assert!(comp.is_present(&paths.base), "{id} files present after install");
    }
}

fn run(env: &Env, settings: JobSettings, src: &std::path::Path, out: PathBuf) -> image::RgbImage {
    let info = probe::probe(env.tools.ffprobe.as_deref(), src).unwrap();
    let spec = JobSpec {
        id: 1,
        info,
        settings,
        output: out.clone(),
        edit: None,
    };
    let r = run_image(env, &spec, &ProgressHandle::default(), &CancelToken::new()).expect("job ran");
    assert!(matches!(r, JobOutcome::Done { .. }), "{r:?}");
    image::open(&out).unwrap().to_rgb8()
}

/// Mean brightness: catches black / blank output from a broken model.
fn mean(img: &image::RgbImage) -> f64 {
    img.pixels().map(|p| p.0.iter().map(|&c| c as f64).sum::<f64>() / 3.0).sum::<f64>()
        / (img.width() * img.height()) as f64
}

#[test]
#[ignore = "downloads ~80 MB and needs a Vulkan GPU and bin/realesrgan-ncnn-vulkan"]
fn realcugan_and_ncnn_custom_models() {
    let project_tools = ToolSet::from_bin_dir(&root().join("bin"));
    if project_tools.realesrgan.is_none() || devices::enumerate().unwrap_or_default().is_empty() {
        eprintln!("SKIPPED: needs bin/realesrgan-ncnn-vulkan and a Vulkan GPU");
        return;
    }
    let base = root().join("target").join("it-scratch").join("models-home");
    let paths = AppPaths::from_base(base.clone());
    install(&paths, &["realcugan", "esrgan-nomos2-ncnn"]);
    // Custom model: the Nomos2 ncnn files copied into models/custom under a community name.
    let custom = paths.models.join("custom");
    std::fs::create_dir_all(&custom).unwrap();
    for ext in ["param", "bin"] {
        std::fs::copy(
            paths.models.join("realesrgan").join(format!("4xNomos2_otf_esrgan.{ext}")),
            custom.join(format!("4x_MyNomos.{ext}")),
        )
        .unwrap();
    }
    let mut tools = ToolSet::from_bin_dir(&paths.bin);
    tools.realesrgan = project_tools.realesrgan.clone();
    tools.ffprobe = project_tools.ffprobe.clone();
    tools.ffmpeg = project_tools.ffmpeg.clone();
    assert!(tools.realcugan.is_some());
    let env = Env {
        models: ModelSet::discover(&paths.models),
        gpus: devices::enumerate().unwrap_or_default(),
        encoders: Arc::new(RwLock::new(encoders::detect(tools.ffmpeg.as_deref()))),
        monitor: None,
        cores: 4,
        tools,
    };
    let dir = base.join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.png");
    image::RgbImage::from_fn(96, 64, |x, y| {
        let line = (x / 8 + y / 8) % 2 == 0;
        if line { image::Rgb([30, 30, 40]) } else { image::Rgb([240, 200, 120]) }
    })
    .save(&src)
    .unwrap();

    for (profile, model, scale, w) in [
        (Profile::Anime, "realcugan-se", ScaleMode::X2, 192),
        (Profile::Anime, "realcugan-pro", ScaleMode::X3, 288),
        (Profile::Realistic, "esrgan-nomos2-4x", ScaleMode::X4, 384),
        (Profile::Realistic, "custom:4x_MyNomos", ScaleMode::X4, 384),
    ] {
        let mut s = JobSettings {
            profile,
            model: Some(model.into()),
            scale,
            ..Default::default()
        };
        s.output.image_format = ImageFormat::Png;
        s.video.temp_dir = Some(dir.clone());
        let out = dir.join(format!("out_{}.png", model.replace(':', "_")));
        let img = run(&env, s, &src, out);
        assert_eq!(img.dimensions(), (w, w * 2 / 3), "{model}");
        let m = mean(&img);
        assert!((60.0..220.0).contains(&m), "{model}: mean {m} looks blank");
        eprintln!("{model}: {}x{} mean {m:.0} ok", img.width(), img.height());
    }
}
