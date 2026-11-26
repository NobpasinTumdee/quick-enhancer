//! "Make sharp (AI)" end to end through the image job, with the real models (ignored by default).
//!
//! Needs a tools root with the ONNX runtime and the models installed (`runtime/dml`,
//! `models/onnx/restormer-*.onnx`, `realplksr-nomos2-4x-s256.onnx`, `gfpgan-1.4-512.onnx`,
//! `yunet-2023mar.onnx`, and for Creative `models/seedvr2-3b/`), and the evaluation's blurry /
//! sharp pairs (`in/<case>.png`, `gt/<image>.png`, see docs/DEBLUR_EVAL.md):
//!
//! `QE_TEST_BASE=<root> QE_TEST_PAIRS=<eval folder>
//!  cargo test --release --test sharp_real -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quick_enhancer::config::{ImageFormat, JobSettings};
use quick_enhancer::devices;
use quick_enhancer::media::{encoders, probe};
use quick_enhancer::models::RuntimeKind;
use quick_enhancer::paths::{ModelSet, ToolSet};
use quick_enhancer::pipeline::cancel::CancelToken;
use quick_enhancer::pipeline::image_job::run_image;
use quick_enhancer::pipeline::job::{Env, JobOutcome, JobSpec, ProgressHandle};
use quick_enhancer::sharp::plan::Preset;

fn env_from(base: &Path) -> Env {
    // SAFETY: set before any worker starts; this test is the only one in its binary.
    unsafe { std::env::set_var(quick_enhancer::worker::EXE_ENV, env!("CARGO_BIN_EXE_QuickEnhancer")) };
    let gpus = devices::enumerate().unwrap_or_default();
    quick_enhancer::engines::onnx::runtime::configure(base, RuntimeKind::DirectMl, &gpus);
    let tools = ToolSet::from_bin_dir(&base.join("bin"));
    Env {
        models: ModelSet::discover(&base.join("models")),
        gpus,
        encoders: Arc::new(RwLock::new(encoders::detect(None))),
        monitor: None,
        cores: 8,
        tools,
    }
}

/// Mean squared Laplacian (sharpness).
fn lapvar(img: &image::RgbImage) -> f64 {
    let g = image::imageops::grayscale(img);
    let (w, h) = g.dimensions();
    let mut s = 0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let l = g.get_pixel(x - 1, y)[0] as f64 + g.get_pixel(x + 1, y)[0] as f64 + g.get_pixel(x, y - 1)[0] as f64
                + g.get_pixel(x, y + 1)[0] as f64
                - 4.0 * g.get_pixel(x, y)[0] as f64;
            s += l * l;
        }
    }
    s / ((w - 2) * (h - 2)) as f64
}

fn psnr(a: &image::RgbImage, b: &image::RgbImage) -> f64 {
    let mse = a.as_raw().iter().zip(b.as_raw()).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>()
        / a.as_raw().len() as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
}

struct Run {
    img: image::RgbImage,
    note: String,
    secs: f64,
    file: Vec<u8>,
}

fn run(env: &Env, src: &Path, out: PathBuf, preset: Preset, creative: Option<f32>) -> Run {
    let mut s = JobSettings { sharp_on: true, ..Default::default() };
    s.sharp.preset = preset;
    if let Some(c) = creative {
        s.sharp.creative_auto = false;
        s.sharp.creative = c;
    }
    s.output.image_format = ImageFormat::Png;
    let info = probe::probe(None, src).unwrap();
    let spec = JobSpec { id: 7, info, settings: s, output: out.clone(), edit: None };
    let t = std::time::Instant::now();
    let r = run_image(env, &spec, &ProgressHandle::default(), &CancelToken::new()).expect("job ran");
    let JobOutcome::Done { note, .. } = r else { panic!("{r:?}") };
    Run { img: image::open(&out).unwrap().to_rgb8(), note, secs: t.elapsed().as_secs_f64(), file: std::fs::read(&out).unwrap() }
}

#[test]
#[ignore = "needs QE_TEST_BASE (models + ONNX runtime), QE_TEST_PAIRS and a DirectX 12 GPU"]
fn make_sharp_faithful_and_creative() {
    let base = PathBuf::from(std::env::var_os("QE_TEST_BASE").expect("QE_TEST_BASE"));
    let pairs = PathBuf::from(std::env::var_os("QE_TEST_PAIRS").expect("QE_TEST_PAIRS"));
    let env = env_from(&base);
    assert!(env.models.onnx_runtime, "no ONNX runtime under {}", base.display());
    let out_dir = std::env::temp_dir().join(format!("qe-sharp-real-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let creative_cases = std::env::var("QE_TEST_CREATIVE").map(|v| v != "0").unwrap_or(true);
    for (case, gt_name, preset) in [
        ("sign_motion31", "sign", Preset::Auto),
        ("portrait_defocus", "portrait", Preset::Portrait),
        ("landscape_defocus_heavy", "landscape", Preset::Auto),
    ] {
        let src = pairs.join("in").join(format!("{case}.png"));
        let input = image::open(&src).unwrap().to_rgb8();
        let gt = image::open(pairs.join("gt").join(format!("{gt_name}.png"))).unwrap().to_rgb8();
        let mut variants = vec![("faithful", Some(0.0))];
        if creative_cases {
            variants.push(("creative50", Some(0.5)));
        }
        for (name, creative) in variants {
            let r = run(&env, &src, out_dir.join(format!("{case}_{name}.png")), preset, creative);
            let (li, lo, lg) = (lapvar(&input), lapvar(&r.img), lapvar(&gt));
            let (pi, po) = (psnr(&input, &gt), psnr(&r.img, &gt));
            eprintln!(
                "{case} {name}: {:.1}s · sharpness {li:.0} → {lo:.0} (sharp {lg:.0}) · PSNR {pi:.2} → {po:.2} dB · {}",
                r.secs, r.note
            );
            assert_eq!(r.img.dimensions(), input.dimensions(), "{case} {name}: original size");
            assert!(lo > li * 2.0, "{case} {name}: not sharper ({li} → {lo})");
            // Not a different picture: within a few dB of the input's PSNR against the original.
            assert!(po > pi - 4.0, "{case} {name}: PSNR fell {pi} → {po}");
            let generative = creative.unwrap_or(0.0) > 0.0;
            let marked = quick_enhancer::sharp::meta::read_note(&r.file).is_some();
            assert_eq!(marked, generative, "{case} {name}: generative note in the file");
            assert_eq!(r.note.contains(quick_enhancer::sharp::meta::NOTE_MARK), generative, "{}", r.note);
        }
    }
    eprintln!("outputs in {}", out_dir.display());
}
