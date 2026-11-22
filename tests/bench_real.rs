//! Runs the real benchmark + auto-tune on this PC and prints the results.
//! `cargo test --release --test bench_real -- --ignored --nocapture`

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use quick_enhancer::bench;
use quick_enhancer::config::MachineSettings;
use quick_enhancer::devices;
use quick_enhancer::media::encoders;
use quick_enhancer::paths::{AppPaths, ModelSet, ToolSet};
use quick_enhancer::pipeline::cancel::CancelToken;
use quick_enhancer::pipeline::job::Env;
use quick_enhancer::resources::{self, EstimateInput};

#[test]
#[ignore = "needs the ncnn tools, models and a Vulkan GPU; takes up to a minute"]
fn benchmark_and_auto_tune_this_pc() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let local = ToolSet::from_bin_dir(&root.join("bin"));
    let paths = AppPaths::resolve();
    let (tools, models) = if local.realesrgan.is_some() {
        (local, ModelSet::discover(&root.join("models")))
    } else {
        (ToolSet::discover(&paths), ModelSet::discover(&paths.models))
    };
    if tools.realesrgan.is_none() {
        eprintln!("SKIPPED: realesrgan-ncnn-vulkan not installed");
        return;
    }
    let enc = encoders::detect(tools.ffmpeg.as_deref());
    let env = Env {
        tools,
        models,
        gpus: devices::enumerate().unwrap_or_default(),
        encoders: Arc::new(RwLock::new(enc)),
        monitor: None,
        cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
    };
    let m = MachineSettings::default();
    let t0 = std::time::Instant::now();
    let r = bench::run(&env, &m, &Mutex::new(Default::default()), &CancelToken::new()).expect("benchmark");
    eprintln!("benchmark took {:.1} s\n{r:#?}", t0.elapsed().as_secs_f64());
    let mut sys = resources::gather(&env, &m);
    if sys.vram_free.is_none() {
        sys.vram_free = sys.vram_total.map(|t| t * 85 / 100);
    }
    eprintln!("{sys:#?}");
    let typical = EstimateInput {
        video: true,
        src_w: 1920,
        src_h: 1080,
        frames: 1800,
        k: 2,
        ai: true,
        heavy: false,
        model_w: 7680,
        model_h: 4320,
        out_w: 3840,
        out_h: 2160,
        out_fps: 60.0,
    };
    let (tuned, changes) = resources::auto_tune(&sys, Some(&r), &m, &typical);
    eprintln!("auto-tune: {tuned:#?}");
    for c in &changes {
        eprintln!("  {}: {} -> {}", c.what, c.from, c.to);
    }
    assert!(r.tiles.iter().any(|t| t.ok), "no tile size worked");
    assert!(r.png_write_mb_s > 0.0);
    assert!(resources::machine_issues(&sys, &tuned).iter().all(|i| i.level != resources::Level::Block));
}
