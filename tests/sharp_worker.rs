//! "Make sharp (AI)": the worker-process protocol with the real exe (start, progress, cancel,
//! errors, crashes), and the generative model end to end (ignored: needs the model files and a
//! GPU runtime).
//!
//! `cargo test --test sharp_worker`
//! Real model: set `QE_TEST_BASE` (a tools root with `runtime/dml` or `runtime/cuda`) and
//! `QE_SEEDVR2_DIR` (folder with `dit.onnx`, `vae_enc_first.onnx`, `vae_dec_first.onnx`), then
//! `cargo test --release --test sharp_worker -- --ignored --nocapture`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quick_enhancer::models::RuntimeKind;
use quick_enhancer::pipeline::cancel::CancelToken;
use quick_enhancer::sharp::run::{Generative, run_seedvr2};
use quick_enhancer::worker::{self, SelfTestEnd, Task, WorkerError};

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_QuickEnhancer"))
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("qe-sharp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn self_test(steps: u32, ms: u64, end: SelfTestEnd, cancel: &CancelToken) -> (Result<(), WorkerError>, Vec<f32>) {
    let dir = scratch(&format!("{end:?}-{steps}"));
    let mut seen = Vec::new();
    let r = worker::run(&exe(), &Task::SelfTest { steps, ms, end }, &dir, cancel, &mut |p| seen.push(p));
    // The task file is removed whatever happened.
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "task file left behind");
    let _ = std::fs::remove_dir_all(&dir);
    (r, seen)
}

#[test]
fn worker_reports_progress_and_done() {
    let (r, seen) = self_test(4, 10, SelfTestEnd::Done, &CancelToken::new());
    assert_eq!(r, Ok(()));
    assert_eq!(seen, vec![0.25, 0.5, 0.75, 1.0]);
}

#[test]
fn worker_errors_are_told_apart() {
    let c = CancelToken::new();
    assert!(matches!(self_test(1, 1, SelfTestEnd::Error, &c).0, Err(WorkerError::Failed(m)) if m.contains("self-test error")));
    assert!(matches!(self_test(1, 1, SelfTestEnd::Oom, &c).0, Err(WorkerError::Oom(_))));
}

#[test]
fn worker_crash_becomes_a_clean_error() {
    let c = CancelToken::new();
    match self_test(1, 1, SelfTestEnd::Crash, &c).0 {
        Err(WorkerError::Crashed { code, tail }) => {
            assert_eq!(code, Some(3));
            assert!(tail.contains("self-test crash"), "{tail}");
            let msg = WorkerError::Crashed { code, tail }.to_string();
            assert!(!msg.is_empty());
        }
        other => panic!("{other:?}"),
    }
    match self_test(1, 1, SelfTestEnd::Panic, &c).0 {
        Err(WorkerError::Crashed { code, tail }) => {
            assert_eq!(code, Some(101));
            assert!(tail.contains("self-test panic"), "{tail}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn worker_cancel_kills_the_child_quickly() {
    let c = CancelToken::new();
    let c2 = c.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        c2.cancel();
    });
    let t = Instant::now();
    let (r, seen) = self_test(1000, 50, SelfTestEnd::Done, &c);
    assert_eq!(r, Err(WorkerError::Cancelled));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(seen.len() < 1000);
}

#[test]
fn worker_rejects_a_bad_task_file() {
    let dir = scratch("bad");
    let f = dir.join("task.json");
    std::fs::write(&f, b"{not json").unwrap();
    let out = std::process::Command::new(exe()).arg(worker::ARG).arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("error invalid task file"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mean squared Laplacian (a sharpness measure) of the grey image.
fn lapvar(img: &image::RgbImage) -> f64 {
    let g = image::imageops::grayscale(img);
    let (w, h) = g.dimensions();
    let mut s = 0f64;
    let mut n = 0f64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = g.get_pixel(x, y)[0] as f64;
            let l = g.get_pixel(x - 1, y)[0] as f64 + g.get_pixel(x + 1, y)[0] as f64 + g.get_pixel(x, y - 1)[0] as f64
                + g.get_pixel(x, y + 1)[0] as f64
                - 4.0 * c;
            s += l * l;
            n += 1.0;
        }
    }
    s / n
}

fn mean_abs_diff(a: &image::RgbImage, b: &image::RgbImage) -> f64 {
    a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.as_raw().len() as f64
}

fn generative_env() -> Option<Generative> {
    let base = PathBuf::from(std::env::var_os("QE_TEST_BASE")?);
    let model_dir = PathBuf::from(std::env::var_os("QE_SEEDVR2_DIR")?);
    let runtime = match std::env::var("QE_TEST_RUNTIME").as_deref() {
        Ok("cuda") => RuntimeKind::Cuda,
        _ => RuntimeKind::DirectMl,
    };
    let gpus = quick_enhancer::devices::enumerate().ok()?;
    let gpu = quick_enhancer::devices::auto_select(&gpus)?.clone();
    Some(Generative {
        model_dir,
        base,
        runtime,
        gpu: Some(gpu.index),
        vram_mb: gpu.vram_bytes >> 20,
        gpus,
        exe: exe(),
    })
}

fn test_image() -> PathBuf {
    std::env::var_os("QE_TEST_IMAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("icon-256.png"))
}

#[test]
#[ignore]
fn seedvr2_restores_a_blurry_image() {
    let Some(g) = generative_env() else {
        panic!("set QE_TEST_BASE and QE_SEEDVR2_DIR");
    };
    let src = image::open(test_image()).unwrap().into_rgb8();
    // Sides multiple of 16, at most ~1 MP.
    let mp: f64 = std::env::var("QE_TEST_MP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let k = (mp * 1.0e6 / (src.width() as f64 * src.height() as f64)).sqrt().min(1.0);
    let (w, h) = (((src.width() as f64 * k) as u32) / 16 * 16, ((src.height() as f64 * k) as u32) / 16 * 16);
    let sharp = image::imageops::resize(&src, w, h, image::imageops::FilterType::Lanczos3);
    let blurry = image::imageops::blur(&sharp, 2.0);
    let work = scratch("seedvr2");
    let t = Instant::now();
    let tile = std::env::var("QE_TEST_TILE").ok().and_then(|v| v.parse().ok()).unwrap_or(256);
    let out = run_seedvr2(&g, &blurry, tile, 42, &work, &CancelToken::new()).unwrap_or_else(|(p, e)| panic!("{p:?}: {e}"));
    let secs = t.elapsed().as_secs_f32();
    out.save(work.join("out.png")).unwrap();
    blurry.save(work.join("in.png")).unwrap();
    let (lb, lo, ls) = (lapvar(&blurry), lapvar(&out), lapvar(&sharp));
    // Low frequencies (colours, layout) must stay; fine texture may be redrawn.
    let lo_pass = |i: &image::RgbImage| image::imageops::blur(i, 4.0);
    let (db, dout) = (mean_abs_diff(&lo_pass(&blurry), &lo_pass(&sharp)), mean_abs_diff(&lo_pass(&out), &lo_pass(&sharp)));
    eprintln!(
        "{w}x{h} in {secs:.1}s · sharpness blurry {lb:.0} → out {lo:.0} (sharp {ls:.0}) · error vs sharp {db:.2} → {dout:.2} · {}",
        work.display()
    );
    assert_eq!(out.dimensions(), (w, h));
    assert!(lo > lb * 1.5, "output not sharper: {lb} → {lo}");
    // Same picture (colours and layout kept), not an invented one.
    assert!(dout < 6.0, "output drifted: {db} → {dout}");
}
