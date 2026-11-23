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

fn psnr(a: &image::RgbImage, b: &image::RgbImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    let mse = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(&x, &y)| (x as f64 - y as f64).powi(2))
        .sum::<f64>()
        / a.as_raw().len() as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
}

#[test]
#[ignore = "downloads ~60 MB (ONNX Runtime DirectML + models) and needs a DirectX 12 GPU"]
fn onnx_models_on_gpu_match_cpu_and_run_video_in_memory() {
    use quick_enhancer::config::{ComputeDevice, EncoderChoice};
    use quick_enhancer::media::encoders::VideoEncoder;
    use quick_enhancer::models::RuntimeKind;
    use quick_enhancer::pipeline::cancel::PauseFlag;
    use quick_enhancer::pipeline::video_job::{VideoOptions, run_video};

    let project_tools = ToolSet::from_bin_dir(&root().join("bin"));
    let gpus = devices::enumerate().unwrap_or_default();
    if project_tools.ffmpeg.is_none() || gpus.is_empty() {
        eprintln!("SKIPPED: needs bin/ffmpeg and a GPU");
        return;
    }
    let base = root().join("target").join("it-scratch").join("onnx-home");
    let paths = AppPaths::from_base(base.clone());
    install(&paths, &["ort-dml", "vcrt-dml", "onnx-span-nomosuni", "onnx-realplksr-nomos2", "onnx-dejpg"]);
    quick_enhancer::engines::onnx::runtime::configure(&base, RuntimeKind::DirectMl, &gpus);
    let env = Env {
        models: ModelSet::discover(&paths.models),
        gpus: gpus.clone(),
        encoders: Arc::new(RwLock::new(encoders::detect(project_tools.ffmpeg.as_deref()))),
        monitor: None,
        cores: 8,
        tools: project_tools.clone(),
    };
    assert!(env.models.onnx_runtime);
    let dir = base.join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.png");
    image::RgbImage::from_fn(300, 180, |x, y| {
        let v = ((x as f32 / 9.0).sin() * 60.0 + (y as f32 / 7.0).cos() * 60.0 + 128.0) as u8;
        image::Rgb([v, (x % 256) as u8, (y * 2 % 256) as u8])
    })
    .save(&src)
    .unwrap();

    // (model, scale, static tile model?)
    for (model, scale) in [("span-nomosuni-2x", ScaleMode::X2), ("realplksr-nomos2-4x", ScaleMode::X4)] {
        let mut outs = Vec::new();
        for compute in [ComputeDevice::Auto, ComputeDevice::Cpu] {
            let mut s = JobSettings {
                profile: Profile::Realistic,
                model: Some(model.into()),
                scale,
                compute,
                ..Default::default()
            };
            s.output.image_format = ImageFormat::Png;
            s.video.temp_dir = Some(dir.clone());
            let t = std::time::Instant::now();
            let out = dir.join(format!("{model}_{compute:?}.png"));
            let img = run(&env, s, &src, out);
            eprintln!("{model} on {compute:?}: {:?} in {:.2}s", img.dimensions(), t.elapsed().as_secs_f32());
            outs.push(img);
        }
        let f = if model.starts_with("span") { 2 } else { 4 };
        assert_eq!(outs[0].dimensions(), (300 * f, 180 * f));
        let p = psnr(&outs[0], &outs[1]);
        eprintln!("{model}: GPU vs CPU PSNR {p:.1} dB");
        assert!(p > 40.0, "{model}: GPU and CPU outputs differ ({p:.1} dB)");
        let m = mean(&outs[0]);
        assert!((40.0..220.0).contains(&m), "{model}: mean {m}");
    }

    // Video in memory mode with SPAN 2x (no PNG round trip).
    let vid = dir.join("in.mp4");
    let mut c = quick_enhancer::util::new_command(project_tools.ffmpeg.as_ref().unwrap());
    c.args(["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=256x144:rate=24:duration=1", "-pix_fmt", "yuv420p"])
        .arg(&vid);
    quick_enhancer::util::run_capture(c, None, quick_enhancer::config::Priority::Normal).unwrap();
    let info = probe::probe(project_tools.ffprobe.as_deref(), &vid).unwrap();
    let mut s = JobSettings {
        profile: Profile::Realistic,
        model: Some("span-nomosuni-2x".into()),
        scale: ScaleMode::X2,
        ..Default::default()
    };
    s.output.encoder = EncoderChoice::Specific(VideoEncoder::Libx264);
    s.video.temp_dir = Some(dir.clone());
    let out = dir.join("out.mp4");
    let spec = JobSpec { id: 2, info, settings: s, output: out.clone(), edit: None };
    let r = run_video(&env, &spec, &ProgressHandle::default(), &CancelToken::new(), &PauseFlag::default(), &VideoOptions::default())
        .expect("video job");
    assert_eq!((r.out_w, r.out_h), (512, 288));
    assert_eq!(r.frames, 24);
    // onnxruntime.dll binds to the app-local VC++ runtime installed next to it.
    let vc = quick_enhancer::engines::onnx::runtime::vc_runtime_in_use().expect("msvcp140 loaded");
    eprintln!("C++ runtime in use: {vc}");
    assert!(vc.to_ascii_lowercase().contains(r"runtime\dml"), "{vc}");
    let notices = quick_enhancer::engines::onnx::runtime::take_notices();
    eprintln!("notices: {notices:?}");

    // Uninstall SPAN through the Model Manager logic, check the state, then reinstall it.
    use quick_enhancer::models::install::{InstallSnapshot, uninstall};
    use quick_enhancer::models::ModelState;
    let cat = Catalog::embedded();
    let tools = ToolsManifest::embedded();
    let span = cat.model("span-nomosuni-2x").unwrap();
    let snap = InstallSnapshot::scan(&paths, &cat, &tools);
    assert_eq!(snap.state(&cat, &tools, span), ModelState::Installed);
    let removed = uninstall(&paths, &cat, &tools, &snap, span).expect("uninstall");
    eprintln!("uninstalled: {removed:?}");
    assert!(removed.iter().any(|p| p == "onnx-span-nomosuni"), "{removed:?}");
    let snap = InstallSnapshot::scan(&paths, &cat, &tools);
    assert_eq!(snap.state(&cat, &tools, span), ModelState::NotInstalled);
    // The runtime and the other models are untouched.
    assert!(snap.runtime_dml);
    assert_eq!(snap.state(&cat, &tools, cat.model("realplksr-nomos2-4x").unwrap()), ModelState::Installed);
    install(&paths, &["onnx-span-nomosuni"]);
    let snap = InstallSnapshot::scan(&paths, &cat, &tools);
    assert_eq!(snap.state(&cat, &tools, span), ModelState::Installed);
}

#[test]
#[ignore = "downloads ~200 MB of ONNX models and needs a DirectX 12 GPU"]
fn onnx_transformers_and_restore_models_run() {
    use quick_enhancer::models::RuntimeKind;
    let project_tools = ToolSet::from_bin_dir(&root().join("bin"));
    let gpus = devices::enumerate().unwrap_or_default();
    if project_tools.ffmpeg.is_none() || gpus.is_empty() {
        eprintln!("SKIPPED: needs bin/ffmpeg and a GPU");
        return;
    }
    let base = root().join("target").join("it-scratch").join("onnx-home");
    let paths = AppPaths::from_base(base.clone());
    install(
        &paths,
        &["ort-dml", "onnx-dat2-nomos2", "onnx-hatl-bhi", "onnx-dejpg", "onnx-denoise", "onnx-scunet"],
    );
    quick_enhancer::engines::onnx::runtime::configure(&base, RuntimeKind::DirectMl, &gpus);
    let env = Env {
        models: ModelSet::discover(&paths.models),
        gpus,
        encoders: Arc::new(RwLock::new(encoders::detect(project_tools.ffmpeg.as_deref()))),
        monitor: None,
        cores: 8,
        tools: project_tools,
    };
    let dir = base.join("work2");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.jpg");
    let clean = image::RgbImage::from_fn(200, 140, |x, y| {
        let v = ((x as f32 / 9.0).sin() * 60.0 + (y as f32 / 7.0).cos() * 60.0 + 128.0) as u8;
        image::Rgb([v, (x % 256) as u8, (y * 2 % 256) as u8])
    });
    // Low-quality JPEG: something for the cleanup models to remove.
    let mut f = std::fs::File::create(&src).unwrap();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, 20).encode_image(&clean).unwrap();
    drop(f);

    // Transformer SR (static tiles, fp16 / fp32).
    for model in ["dat2-nomos2-4x", "hatl-bhi-4x"] {
        let mut s = JobSettings { profile: Profile::Realistic, model: Some(model.into()), scale: ScaleMode::X4, ..Default::default() };
        s.output.image_format = ImageFormat::Png;
        s.video.temp_dir = Some(dir.clone());
        let t = std::time::Instant::now();
        let img = run(&env, s, &src, dir.join(format!("{model}.png")));
        eprintln!("{model}: {:?} in {:.1}s", img.dimensions(), t.elapsed().as_secs_f32());
        assert_eq!(img.dimensions(), (800, 560));
        let m = mean(&img);
        assert!((40.0..220.0).contains(&m), "{model}: mean {m}");
    }

    // 1x cleanup without an upscale model: JPEG block edges must get weaker, colours stay.
    let jpeg = image::open(&src).unwrap().into_rgb8();
    let before = blockiness(&jpeg);
    for model in ["dejpg-1x", "denoise-1x", "scunet-1x"] {
        let mut s = JobSettings {
            profile: Profile::NoAi,
            scale: ScaleMode::X1,
            restore_on: true,
            restore_model: model.into(),
            ..Default::default()
        };
        s.output.image_format = ImageFormat::Png;
        s.video.temp_dir = Some(dir.clone());
        let t = std::time::Instant::now();
        let img = run(&env, s, &src, dir.join(format!("{model}.png")));
        assert_eq!(img.dimensions(), (200, 140));
        let after = blockiness(&img);
        let shift = (mean(&img) - mean(&jpeg)).abs();
        eprintln!(
            "{model}: blockiness {before:.2} -> {after:.2}, mean shift {shift:.1}, {:.1}s",
            t.elapsed().as_secs_f32()
        );
        assert!(after < before, "{model} did not reduce JPEG blocking ({before:.2} -> {after:.2})");
        assert!(shift < 6.0, "{model} shifted brightness by {shift:.1}");
    }
}

/// Mean horizontal step across 8-pixel JPEG block edges divided by the mean step inside
/// blocks (≈1 for a clean image, >1 with blocking).
fn blockiness(img: &image::RgbImage) -> f64 {
    let (mut edge, mut ne, mut inner, mut ni) = (0.0, 0.0, 0.0, 0.0);
    for y in 0..img.height() {
        for x in 0..img.width() - 1 {
            let a = img.get_pixel(x, y).0;
            let b = img.get_pixel(x + 1, y).0;
            let d: f64 = (0..3).map(|c| (a[c] as f64 - b[c] as f64).abs()).sum();
            if x % 8 == 7 {
                edge += d;
                ne += 1.0;
            } else {
                inner += d;
                ni += 1.0;
            }
        }
    }
    (edge / ne) / (inner / ni).max(1e-6)
}

#[test]
#[ignore = "downloads ~700 MB of face models, needs a DirectX 12 GPU and .feasibility/ reference images"]
fn face_detection_and_restoration_match_the_reference() {
    use quick_enhancer::config::Priority;
    use quick_enhancer::engines::EngineCtx;
    use quick_enhancer::models::RuntimeKind;
    let feas = root().join(".feasibility");
    let project_tools = ToolSet::from_bin_dir(&root().join("bin"));
    let gpus = devices::enumerate().unwrap_or_default();
    if project_tools.ffmpeg.is_none() || gpus.is_empty() || !feas.join("ref").join("gfpgan.png").is_file() {
        eprintln!("SKIPPED: needs bin/ffmpeg, a GPU and the .feasibility reference images");
        return;
    }
    let base = root().join("target").join("it-scratch").join("onnx-home");
    let paths = AppPaths::from_base(base.clone());
    install(&paths, &["ort-dml", "vcrt-dml", "onnx-yunet", "onnx-gfpgan", "onnx-codeformer"]);
    quick_enhancer::engines::onnx::runtime::configure(&base, RuntimeKind::DirectMl, &gpus);
    let env = Env {
        models: ModelSet::discover(&paths.models),
        gpus: gpus.clone(),
        encoders: Arc::new(RwLock::new(encoders::detect(project_tools.ffmpeg.as_deref()))),
        monitor: None,
        cores: 8,
        tools: project_tools,
    };

    // Detection vs OpenCV FaceDetectorYN on the same model (.feasibility/conv/yunet_ref.py).
    let fr = quick_enhancer::engines::face_backend("gfpgan-1.4", 1.0, &env.models).expect("face backend");
    let cancel = CancelToken::new();
    let ctx = EngineCtx {
        cancel: &cancel,
        gpu: Some(gpus[0].index),
        jobs: "1:4:1".into(),
        tile: 0,
        priority: Priority::Normal,
        progress: None,
    };
    // (file, bbox x y w h, left eye)
    for (file, bbox, eye) in [
        ("astronaut_512.png", [178.2, 62.9, 89.9, 113.3], [200.1, 101.8]),
        ("frame_1080p.png", [376.3, 128.7, 194.6, 242.8], [431.0, 212.6]),
    ] {
        let img = image::open(feas.join("inputs").join(file)).unwrap().into_rgb8();
        let faces = fr.detect(&img, &ctx).expect("detect");
        eprintln!("{file}: {faces:?}");
        assert_eq!(faces.len(), 1, "{file}");
        let f = &faces[0];
        let tol = bbox[2] * 0.06;
        for k in 0..4 {
            assert!((f.bbox[k] - bbox[k]).abs() < tol, "{file}: bbox {:?} vs {bbox:?}", f.bbox);
        }
        assert!((f.kps[0][0] - eye[0]).abs() < tol && (f.kps[0][1] - eye[1]).abs() < tol, "{file}: eye {:?}", f.kps[0]);
    }

    // Restoration through the pipeline vs the Python prototype (.feasibility/conv/face_test.py).
    let dir = base.join("work3");
    std::fs::create_dir_all(&dir).unwrap();
    let src = feas.join("inputs").join("face_lq_up512.png");
    let lq = image::open(&src).unwrap().into_rgb8();
    for (model, strength, reference) in [("gfpgan-1.4", 1.0, "gfpgan.png"), ("codeformer", 0.7, "codeformer.png")] {
        let mut s = JobSettings {
            profile: Profile::NoAi,
            scale: ScaleMode::X1,
            face_on: true,
            face_model: model.into(),
            face_strength: strength,
            ..Default::default()
        };
        s.output.image_format = ImageFormat::Png;
        s.video.temp_dir = Some(dir.clone());
        let t = std::time::Instant::now();
        let out = run(&env, s, &src, dir.join(format!("{model}.png")));
        let r = image::open(feas.join("ref").join(reference)).unwrap().into_rgb8();
        // Face region: generative models (CodeFormer) don't reproduce pixels exactly, so check
        // that the face got much sharper and keeps the reference's colours.
        let face = |i: &image::RgbImage| image::imageops::crop_imm(i, 150, 40, 140, 160).to_image();
        let (fo, fr, fl) = (face(&out), face(&r), face(&lq));
        let (so, sr, sl) = (sharpness(&fo), sharpness(&fr), sharpness(&fl));
        let shift = (mean(&fo) - mean(&fr)).abs();
        eprintln!(
            "{model}: sharpness {sl:.2} -> {so:.2} (reference {sr:.2}), colour shift {shift:.1}, {:.1}s",
            t.elapsed().as_secs_f32()
        );
        assert!(so > sl * 1.5 && so > sr * 0.6, "{model}: face not restored ({sl:.2} -> {so:.2})");
        assert!(shift < 6.0, "{model}: colour shift {shift:.1}");
        // Outside the face the image is untouched.
        let corner = |i: &image::RgbImage| image::imageops::crop_imm(i, 0, 400, 100, 100).to_image();
        assert!(psnr(&corner(&out), &corner(&lq)) > 45.0, "{model} changed the background");
    }
}

/// Mean absolute Laplacian (detail / sharpness).
fn sharpness(img: &image::RgbImage) -> f64 {
    let g = image::imageops::grayscale(img);
    let (w, h) = g.dimensions();
    let mut acc = 0.0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = g.get_pixel(x, y).0[0] as f64 * 4.0;
            let n = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                .iter()
                .map(|&(xx, yy)| g.get_pixel(xx, yy).0[0] as f64)
                .sum::<f64>();
            acc += (c - n).abs();
        }
    }
    acc / ((w - 2) * (h - 2)) as f64
}

#[test]
#[ignore = "downloads the RIFE package (~43 MB) and needs a Vulkan GPU"]
fn rife_tntwise_models_interpolate() {
    use quick_enhancer::config::{EncoderChoice, FrameGenMode};
    use quick_enhancer::media::encoders::VideoEncoder;
    use quick_enhancer::pipeline::cancel::PauseFlag;
    use quick_enhancer::pipeline::video_job::{VideoOptions, run_video};

    let project_tools = ToolSet::from_bin_dir(&root().join("bin"));
    let gpus = devices::enumerate().unwrap_or_default();
    if project_tools.ffmpeg.is_none() || gpus.is_empty() {
        eprintln!("SKIPPED: needs bin/ffmpeg and a Vulkan GPU");
        return;
    }
    let base = root().join("target").join("it-scratch").join("rife-home");
    let paths = AppPaths::from_base(base.clone());
    install(&paths, &["rife"]);
    let mut tools = project_tools.clone();
    tools.rife = ToolSet::from_bin_dir(&paths.bin).rife;
    assert!(tools.rife.is_some(), "rife exe installed");
    let env = Env {
        models: ModelSet::discover(&paths.models),
        gpus,
        encoders: Arc::new(RwLock::new(encoders::detect(project_tools.ffmpeg.as_deref()))),
        monitor: None,
        cores: 8,
        tools,
    };
    let names: Vec<&str> = env.models.rife.iter().map(|m| m.name.as_str()).collect();
    eprintln!("rife models: {names:?}");
    let dir = base.join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.mp4");
    let mut c = quick_enhancer::util::new_command(project_tools.ffmpeg.as_ref().unwrap());
    c.args(["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=256x144:rate=12:duration=1", "-pix_fmt", "yuv420p"])
        .arg(&src);
    quick_enhancer::util::run_capture(c, None, quick_enhancer::config::Priority::Normal).unwrap();
    let info = probe::probe(project_tools.ffprobe.as_deref(), &src).unwrap();
    for model in ["rife-v4.26", "rife-v4.25-lite", "rife-v4.6", "rife-v4"] {
        assert!(names.contains(&model), "{model} installed");
        let mut s = JobSettings {
            profile: Profile::NoAi,
            scale: ScaleMode::X1,
            framegen_on: true,
            framegen: FrameGenMode::X2,
            rife_model: model.into(),
            ..Default::default()
        };
        s.output.encoder = EncoderChoice::Specific(VideoEncoder::Libx264);
        s.video.temp_dir = Some(dir.clone());
        let out = dir.join(format!("{model}.mp4"));
        let spec = JobSpec { id: 3, info: info.clone(), settings: s, output: out.clone(), edit: None };
        let t = std::time::Instant::now();
        let r = run_video(&env, &spec, &ProgressHandle::default(), &CancelToken::new(), &PauseFlag::default(), &VideoOptions::default())
            .unwrap_or_else(|e| panic!("{model}: {e}"));
        eprintln!("{model}: {} frames in {:.1}s", r.frames, t.elapsed().as_secs_f32());
        assert_eq!(r.frames, 24, "{model}");
        let o = probe::probe(project_tools.ffprobe.as_deref(), &out).unwrap();
        assert!((o.fps.unwrap().as_f64() - 24.0).abs() < 0.01, "{model}: fps {:?}", o.fps);
    }
}
