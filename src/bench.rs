//! "Benchmark this PC": a short (~20–60 s), cancellable test on built-in synthetic frames —
//! Real-ESRGAN at a few tile sizes, one RIFE step, the CPU filters, PNG write speed to the
//! temp folder and the encoder. Results are stored per device in `<data>/benchmark.json`
//! and feed the time estimate and auto-tune (`resources::auto_tune`).

use crossbeam_channel::{Receiver, unbounded};
use image::RgbImage;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::{ComputeDevice, MachineSettings, Priority, Profile};
use crate::engines::{EngineCtx, Interpolator, UpscaleBackend, choose_backend};
use crate::media::MediaKind;
use crate::pipeline::cancel::CancelToken;
use crate::pipeline::job::Env;
use crate::pipeline::temp::{TempDir, temp_root};
use crate::resources::{BenchResult, TileBench};
use crate::t;

/// Benchmark results of every device that was tested.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BenchStore {
    pub results: Vec<BenchResult>,
}

impl BenchStore {
    pub fn load(path: &Path) -> BenchStore {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
    pub fn get(&self, key: &str) -> Option<&BenchResult> {
        self.results.iter().find(|r| r.device_key == key)
    }
    pub fn put(&mut self, r: BenchResult) {
        self.results.retain(|x| x.device_key != r.device_key);
        self.results.push(r);
    }
}

/// Key of the compute device the machine settings select.
pub fn device_key(env: &Env, compute: ComputeDevice) -> (String, String) {
    match crate::devices::resolve(compute, &env.gpus) {
        Some(g) => (format!("gpu:{}:{}", g.index, g.name), g.name.clone()),
        None => ("cpu".into(), "CPU".into()),
    }
}

#[derive(Clone, Debug, Default)]
pub struct BenchState {
    pub step: String,
    pub fraction: f32,
}

pub struct BenchRun {
    pub state: Arc<Mutex<BenchState>>,
    pub cancel: CancelToken,
    pub done: Receiver<Result<BenchResult, String>>,
}

impl BenchRun {
    pub fn start(env: Arc<Env>, m: MachineSettings, ctx: egui::Context) -> BenchRun {
        let state = Arc::new(Mutex::new(BenchState::default()));
        let cancel = CancelToken::new();
        let (tx, rx) = unbounded();
        let (st, c) = (state.clone(), cancel.clone());
        let _ = std::thread::Builder::new().name("benchmark".into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&env, &m, &st, &c)))
                .unwrap_or_else(|_| Err(t!("err.panic")));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        BenchRun { state, cancel, done: rx }
    }
    pub fn state(&self) -> BenchState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Deterministic, natural-looking test frame: smooth gradients, shapes, fine texture.
pub fn synthetic(w: u32, h: u32, seed: u32) -> RgbImage {
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    RgbImage::from_fn(w, h, |x, y| {
        s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let n = ((s >> 16) % 13) as f32 - 6.0;
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let cx = fx - 0.5 - 0.05 * seed as f32 % 0.3;
        let cy = fy - 0.5;
        let ring = ((cx * cx + cy * cy).sqrt() * 40.0).sin() * 30.0;
        let edge = if (x / 37 + y / 23) % 5 == 0 { 50.0 } else { 0.0 };
        let r = 60.0 + 120.0 * fx + ring + edge + n;
        let g = 80.0 + 100.0 * fy - ring * 0.5 + n;
        let b = 140.0 - 60.0 * fx + edge * 0.5 + n;
        image::Rgb([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8])
    })
}

fn set(st: &Mutex<BenchState>, step: String, fraction: f32) {
    if let Ok(mut g) = st.lock() {
        g.step = step;
        g.fraction = fraction;
    }
}

fn write_frames(dir: &Path, n: u32, w: u32, h: u32) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for i in 0..n {
        crate::pipeline::stages::write_png_uncalibrated(&synthetic(w, h, i), &dir.join(format!("{:08}.png", i + 1)))
            .map_err(|e| e.user_message())?;
    }
    Ok(())
}

pub fn run(env: &Env, m: &MachineSettings, st: &Mutex<BenchState>, cancel: &CancelToken) -> Result<BenchResult, String> {
    let (device_key, device_name) = device_key(env, m.compute);
    let mut r = BenchResult {
        device_key,
        device_name,
        created: crate::history::now_unix(),
        ..Default::default()
    };
    let root = temp_root(m.temp_dir.as_deref());
    let work = TempDir::new(&root, "bench").map_err(|e| e.to_string())?;
    let check = || if cancel.is_cancelled() { Err(t!("err.cancelled")) } else { Ok(()) };

    // 1. PNG write speed + compression ratio (temp folder).
    set(st, t!("bench.step_png"), 0.02);
    let png_dir = work.path().join("png");
    std::fs::create_dir_all(&png_dir).map_err(|e| e.to_string())?;
    let frames: Vec<RgbImage> = (0..4).map(|i| synthetic(1920, 1080, i)).collect();
    let t0 = Instant::now();
    let mut bytes = 0u64;
    for (i, f) in frames.iter().cycle().take(8).enumerate() {
        let p = png_dir.join(format!("{i}.png"));
        crate::pipeline::stages::write_png_uncalibrated(f, &p).map_err(|e| e.user_message())?;
        bytes += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    }
    let secs = t0.elapsed().as_secs_f64().max(1e-3);
    r.png_write_mb_s = bytes as f64 / (1 << 20) as f64 / secs;
    r.png_ratio = (bytes as f64 / (8.0 * 1920.0 * 1080.0 * 3.0)).clamp(0.2, 1.0);
    let _ = std::fs::remove_dir_all(&png_dir);
    check()?;

    // 2. CPU filters (denoise + CAS + Lanczos to 1440p), all cores.
    set(st, t!("bench.step_filters"), 0.08);
    let t0 = Instant::now();
    let img = crate::engines::filters::denoise(&frames[0], 0.3);
    let img = crate::engines::filters::cas(&img, 0.4);
    let _ = crate::engines::resize::resize_rgb(&img, 2560, 1440, crate::engines::resize::ResizeKind::Lanczos3);
    r.filter_ms_per_mp = t0.elapsed().as_secs_f64() * 1000.0 / (2560.0 * 1440.0 / 1e6);
    drop(frames);
    check()?;

    // 3. Real-ESRGAN (the photo model) at a few tile sizes.
    let gpu = crate::devices::resolve(m.compute, &env.gpus).map(|g| g.index);
    let choice = choose_backend(Profile::Realistic, None, MediaKind::Video, &env.tools, &env.models, gpu.is_none());
    if let Ok(c) = choice
        && let UpscaleBackend::Ai(engine) = &c.backend
    {
        let scale = engine.native_scales().into_iter().max().unwrap_or(4);
        let (w, h) = (640u32, 360u32);
        let mp = w as f64 * h as f64 / 1e6;
        let one = work.path().join("in1");
        let four = work.path().join("in4");
        write_frames(&one, 1, w, h)?;
        write_frames(&four, 4, w, h)?;
        let run_tile = |input: &Path, tile: u32| -> Result<f64, String> {
            let out = work.path().join(format!("up{tile}-{}", crate::engines::count_files(input)));
            let ctx = EngineCtx {
                cancel,
                gpu,
                jobs: "1:2:2".into(),
                tile,
                priority: Priority::Normal,
                progress: None,
            };
            let t0 = Instant::now();
            let res = engine.upscale_dir(input, &out, scale, &ctx);
            let _ = std::fs::remove_dir_all(&out);
            res.map_err(|e| e.to_string())?;
            Ok(t0.elapsed().as_secs_f64())
        };
        set(st, t!("bench.step_esrgan", tile = 192), 0.15);
        let t1 = run_tile(&one, 192);
        check()?;
        set(st, t!("bench.step_esrgan", tile = 192), 0.3);
        let t4 = run_tile(&four, 192);
        check()?;
        if let (Ok(t1), Ok(t4)) = (&t1, &t4) {
            let per = ((t4 - t1) / 3.0).max(0.01);
            r.esrgan_startup_s = (t1 - per).max(0.2);
            r.tiles.push(TileBench { tile: 192, ok: true, ms_per_mp: per * 1000.0 / mp });
        } else {
            r.tiles.push(TileBench { tile: 192, ok: false, ms_per_mp: 0.0 });
        }
        for (i, tile) in [128u32, 256].into_iter().enumerate() {
            set(st, t!("bench.step_esrgan", tile = tile), 0.45 + 0.15 * i as f32);
            let t = run_tile(&four, tile);
            check()?;
            r.tiles.push(match t {
                Ok(t) => TileBench {
                    tile,
                    ok: true,
                    ms_per_mp: ((t - r.esrgan_startup_s) / 4.0).max(0.01) * 1000.0 / mp,
                },
                Err(e) => {
                    tracing::warn!("benchmark: tile {tile} failed: {e}");
                    TileBench { tile, ok: false, ms_per_mp: 0.0 }
                }
            });
        }
        r.tiles.sort_by_key(|t| t.tile);

        // 4. RIFE 2x on the same 4 frames.
        if let (Some(exe), Some(model)) = (env.tools.rife.clone(), env.models.rife.first().cloned()) {
            set(st, t!("bench.step_rife"), 0.75);
            let rife = crate::engines::rife_ncnn::RifeNcnn::new(exe, model.path.clone(), model.name.clone(), model.arbitrary_count);
            let out = work.path().join("rife");
            let ctx = EngineCtx {
                cancel,
                gpu,
                jobs: "1:2:2".into(),
                tile: 0,
                priority: Priority::Normal,
                progress: None,
            };
            let t0 = Instant::now();
            match rife.interpolate_dir(&four, &out, 8, false, &ctx) {
                Ok(()) => {
                    let t = (t0.elapsed().as_secs_f64() - r.esrgan_startup_s).max(0.01);
                    r.rife_ms_per_mp = Some(t * 1000.0 / 8.0 / mp);
                }
                Err(e) => tracing::warn!("benchmark: RIFE failed: {e}"),
            }
            check()?;
        }
    }

    // 5. Encoder (auto choice) at 720p.
    if let Some(ffmpeg) = env.tools.ffmpeg.clone() {
        set(st, t!("bench.step_encoder"), 0.88);
        for _ in 0..100 {
            if env.encoders.read().map(|a| a.done).unwrap_or(true) || cancel.is_cancelled() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let enc = env.encoders.read().ok().and_then(|a| a.auto());
        if let Some(enc) = enc {
            let frames: Vec<RgbImage> = (0..3).map(|i| synthetic(1280, 720, i)).collect();
            let spec = crate::media::ffmpeg::EncodeSpec {
                width: 1280,
                height: 720,
                fps: crate::media::Rational::new(30, 1),
                encoder: enc,
                quality: None,
                speed: Default::default(),
                bitrate_kbps: None,
                container: crate::config::Container::Mp4,
                output: work.path().join("bench.mp4"),
                mux: None,
            };
            let t0 = Instant::now();
            let res = crate::media::ffmpeg::Encoder::spawn(&ffmpeg, &spec, Priority::Normal).map_err(|e| e.to_string()).and_then(|mut e| {
                for f in frames.iter().cycle().take(120) {
                    e.write_frame(f).map_err(|e| e.to_string())?;
                }
                e.finish().map_err(|e| e.to_string())
            });
            match res {
                Ok(()) => {
                    r.encoder = Some(enc.label().into());
                    r.encode_fps_720p = Some(120.0 / t0.elapsed().as_secs_f64().max(1e-3));
                }
                Err(e) => tracing::warn!("benchmark: encoder failed: {e}"),
            }
        }
    }
    set(st, t!("bench.step_done"), 1.0);
    tracing::info!("benchmark: {r:?}");
    Ok(r)
}

/// Where benchmark results are stored.
pub fn store_file(data: &Path) -> PathBuf {
    data.join("benchmark.json")
}
