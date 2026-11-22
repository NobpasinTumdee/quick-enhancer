# Quick Enhancer — Plan & Design Decisions

Native Rust desktop app (eframe/egui + wgpu) for local image & video enhancement:
AI upscaling (Real-ESRGAN ncnn), frame interpolation (RIFE ncnn), CAS / unsharp / denoise
filters, explicit compute device + encoder selection, live resource monitor, bounded
chunked video pipeline and an interactive before/after preview.

## Architecture

```
          ┌──────────── UI thread (eframe) ────────────┐
          │ app.rs: queue, settings, preview canvas,    │
          │ monitor panel, log, setup panel             │
          └──┬─────────┬──────────┬──────────┬──────────┘
   probe req │ preview │ job cmds │ snapshot │ (Arc<Mutex>)
             ▼  req    ▼          ▼          │
      probe worker  preview worker  job runner thread ── monitor thread (500 ms)
      (ffprobe +    (same stage     (sequential queue;    sysinfo + NVML/PDH
       thumbnail)    code as export) one job at a time)
                                     │
                     video job = 3 threads + child processes:
     ffmpeg decode ─rgb24 pipe─▶ decoder thread ──chunk dirs (≤2 on disk)──▶ processor
     (CFR at avg fps)            denoise + scene-cut     RIFE ▶ Real-ESRGAN (ncnn CLIs)
                                 signature + PNG write   load ▶ resize ▶ sharpen (rayon)
                                                              │ bounded channel (RAM budget)
                                                              ▼
                                       writer thread ─rgb24 pipe─▶ ffmpeg encode + audio/subs mux
```

All communication uses `crossbeam-channel` (bounded where data volume matters) and
`std::thread`. No async runtime.

## File tree

```
src/
  main.rs            entry: data paths, migration, logging, language, CLI flags, eframe launch
  lib.rs             module tree (lets integration tests drive the pipeline)
  app.rs             eframe::App, top-level state, event pumping
  config.rs          Config / JobSettings (serde + toml, in the data folder)
  paths.rs           installed / portable data + tools folders, 1.x migration, tool + model
                     discovery, setup check
  i18n.rs            t! macro, live language switch, locales/en.toml + th.toml (embedded)
  history.rs         history.json + output-folder scan, merge / de-dup / missing
  outputs.rs         Outputs tab state, background thumbnail worker + cache
  net/               ALL network code: http.rs (ureq + SHA-256 streaming), tools.rs
                     (tools.json, verified install, runner), update.rs (latest.json, versions)
  logging.rs         tracing setup (rolling file in logs/) + in-memory UI log layer
  util.rs            child-process helpers (no console window, priority, stderr tail, kill)
  ui/                queue.rs, settings.rs, monitor.rs, progress.rs, log.rs, setup.rs, widgets.rs,
                     outputs.rs, tools.rs (downloader), update.rs, fonttest.rs
  preview/           canvas.rs (wipe/side-by-side/toggle, zoom/pan), worker.rs (bg render),
                     player.rs (sample clip playback, audio master clock), audio.rs (rodio)
  pipeline/          plan.rs (scale/fps/chunk math), job.rs (job types, progress),
                     runner.rs (queue thread), image_job.rs, video_job.rs, stages.rs
                     (shared denoise→upscale→resize→sharpen code used by preview AND export),
                     scene.rs (cut detector + slot filling), temp.rs (temp dirs, disk checks),
                     cancel.rs
  engines/           mod.rs (Upscaler / Interpolator traits), realesrgan_ncnn.rs, rife_ncnn.rs,
                     ort_stub.rs, filters.rs (CAS, unsharp, bilateral denoise), resize.rs
  media/             probe.rs (ffprobe JSON), ffmpeg.rs (decode/encode pipes, mux args),
                     encoders.rs (detection + test encode + per-encoder quality args)
  monitor/           mod.rs (unified provider + history), nvml.rs, pdh.rs (cfg(windows))
  devices/           mod.rs (Vulkan enumeration via ash, Auto selection)
tests/pipeline_filters.rs   integration test (skips if bin/ffmpeg missing)
tests/tools_archives.rs     #[ignore] real-archive / HTTPS install tests
installer/QuickEnhancer.iss Inno Setup 6 script;  scripts/release.ps1 release builder
tools.json                  pinned tool downloads (embedded);  locales/*.toml UI strings
bin/      ffmpeg, ffprobe, realesrgan-ncnn-vulkan, rife-ncnn-vulkan (+ their DLLs) — not committed
models/   realesrgan/*.param|bin, rife/<model-dir>/            — not committed
```

## Crates (and why)

| Crate | Purpose |
|---|---|
| eframe/egui (wgpu) | UI, required. Default features trimmed (no glow, no persistence). |
| crossbeam-channel | bounded channels between threads |
| thiserror / anyhow | library errors / app-level errors |
| tracing, tracing-subscriber, tracing-appender | logging + daily rolling file in `logs/` |
| image | decode/encode PNG/JPEG/WebP/BMP/TIFF (no AVIF/EXR features → leaner) |
| fast_image_resize | SIMD Lanczos3 / nearest resizing |
| rayon | parallel filters; per-job thread pool sized by preset |
| serde, serde_json, toml | config + ffprobe JSON |
| rfd | native file/folder pickers |
| sysinfo (system, disk features only) | CPU / RAM / free disk / live-PID check |
| nvml-wrapper | NVIDIA GPU stats; loads `nvml.dll` dynamically, absent ⇒ graceful fallback |
| windows (cfg(windows)) | PDH GPU counters, `SetPriorityClass` |
| ash (`loaded`) | Vulkan GPU enumeration; loads `vulkan-1.dll` at runtime so the app starts without Vulkan |
| rodio | preview audio output (cpal/WASAPI); audio decoded by ffmpeg to f32 PCM |
| ureq (rustls) + sha2 | blocking HTTPS for tool downloads / update check, streaming SHA-256 |
| zip | extracting the tool archives |
| trash | "Delete" in the Outputs tab goes to the Recycle Bin |
| winresource (build) | exe icon + version resource from Cargo.toml |

Heaviest pieces are eframe/wgpu (required) and `windows` (only 3 feature modules enabled).
No tokio, no FFmpeg bindings, no ONNX runtime.

## Key decisions

* **Data folders** (v2): installed → settings/history/thumbs/logs in `%APPDATA%\QuickEnhancer`,
  tools in `%LOCALAPPDATA%\QuickEnhancer` (roaming vs. machine-local data). Portable when
  `portable.txt` is next to the exe → everything next to the exe; under `cargo run` /
  `cargo test` (`CARGO_MANIFEST_DIR` set) → everything in the project dir. `QUICK_ENHANCER_HOME` / `ENHANCER_HOME` override all.
  A 1.x `config.toml` is imported once (only if the new one doesn't exist).
* **Same code path for preview and export**: `pipeline::stages` holds `pre_filter` (denoise),
  `upscale_batch` (engine or Rust scaling), `post_process` (exact resize + sharpen).
  Image export, video export and still previews all call these. The "Preview sample" button
  runs the real video pipeline on a 3 s segment.
* **Filter order**: denoise → (interpolate) → upscale → resize-to-exact → sharpen.
  Interpolation runs at source resolution *before* upscaling because RIFE cost scales with
  pixel count and upscaling interpolated frames is equivalent but cheaper than interpolating
  4x-sized frames.
* **Scale math**: requested factor `f` (or target short side, aspect kept, even dims for video).
  Model scale = smallest native scale ≥ f (largest available if none), then Lanczos3 to the
  exact size. `1x` with an AI profile = AI restore at the smallest native scale then
  downscale back (true "enhance only"); profile *No AI* = filters/Lanczos only.
* **Frame generation**: integer multipliers only (RIFE CLI dir mode); "target FPS" picks the
  largest k with `src·k ≤ min(target, max_fps)`. Non-v4 RIFE models only support 2x → other
  options disabled for them.
* **Chunk overlap**: every chunk except the last includes the first frame of the next chunk, so
  RIFE also interpolates across chunk boundaries; outputs belonging to the overlap are dropped.
* **Scene cuts**: own detector on a 64×36 luma thumbnail (mean abs diff + 16-bin histogram
  distance, combined score vs. configurable threshold). For a cut between frame j and j+1 the
  generated slots are filled with frame j (generated files are deleted before upscaling, so no
  GPU time is wasted on them).
* **Disk bound**: a 2-token semaphore — the decoder must take a token before starting a chunk
  dir, the processor returns it after deleting the chunk's dirs ⇒ ≤ 2 chunks on disk.
  Free space checked against an estimate (2 × per-chunk size) before starting.
* **RAM bound**: the only in-memory frame queues are bounded channels whose capacity is derived
  from `ram_budget_mb` and the frame size; batch size for parallel post-processing likewise.
* **No-AI video fast path**: if neither RIFE nor an AI upscaler is needed, frames never touch the
  disk: decode → denoise → resize → sharpen → encode in memory.
* **Denoise algorithm**: 5×5 bilateral filter with range weight on luma difference (one weight per
  neighbour shared across channels, LUT-based). It's edge-preserving, single pass, O(25·N),
  parallel by rows — fast enough for 1080p video (~tens of ms/frame); NL-means is ~10–50× slower.
* **Tile size**: `Auto` derives from free VRAM (≥6 GB→512, ≥3 GB→400, ≥1.5 GB→256, ≥0.7 GB→128,
  else 64). Eco = 128, Balanced = Auto capped at 256, Max = Auto uncapped. On ncnn failure
  (non-zero exit, `vkAllocateMemory`/out-of-memory in stderr, or missing outputs) the chunk is
  retried with half the tile size, down to 32.
* **Encoders**: `ffmpeg -encoders` + a 1-frame test encode per candidate at startup (parallel,
  background). Auto order: hevc_nvenc, hevc_amf, hevc_qsv, h264_nvenc, h264_amf, h264_qsv,
  libx265, libx264, libsvtav1.
* **GPU telemetry**: Vulkan `deviceUUID` ↔ NVML UUID to bind NVML to the selected GPU;
  Vulkan `deviceLUID` ↔ PDH instance `luid_0x…_0x…` for the vendor-neutral path. GPU% = max
  over engine types of the summed per-process utilization (same as Task Manager).
* **Process priority**: applied to every child via `SetPriorityClass` right after spawn
  (Windows only); children are spawned with `CREATE_NO_WINDOW`.
* **Audio/subs**: MKV copies audio + subs. MP4 copies audio if the codec is MP4-friendly
  (aac/mp3/ac3/eac3/opus/alac/flac) else re-encodes AAC; text subs → `mov_text`, bitmap subs
  dropped (logged). Metadata + chapters mapped from the source.
* **Alpha** (images): alpha is split off, Lanczos-resized to the output size and re-attached
  (AI runs on RGB only).
* **WebP output** is lossless (the `image` crate has no lossy WebP encoder); quality slider applies
  to JPEG.
* **ort backend**: `OrtUpscaler` implements `Upscaler` and reports "not implemented" — the trait
  boundary is where a DirectML/CUDA backend plugs in.
* **Stale temp**: temp dirs are `<temp>/quick-enhancer-tmp/<pid>-<n>` (1.x: `enhancer-tmp`); at
  startup any dir whose PID is not alive is removed.

## External binaries

| File | Source |
|---|---|
| `bin/ffmpeg(.exe)`, `bin/ffprobe(.exe)` | https://github.com/BtbN/FFmpeg-Builds/releases or https://www.gyan.dev/ffmpeg/builds/ |
| `bin/realesrgan-ncnn-vulkan(.exe)` (+ `vcomp140*.dll` on Windows) | https://github.com/xinntao/Real-ESRGAN/releases (v0.2.5.0 ncnn zip) |
| `bin/rife-ncnn-vulkan(.exe)` (+ DLLs) | https://github.com/nihui/rife-ncnn-vulkan/releases |
| `models/realesrgan/*.param, *.bin` | `models/` folder of the Real-ESRGAN ncnn zip |
| `models/rife/rife-v4.6/` etc. | model folders of the RIFE ncnn zip |


## How to run
```bash
cargo run --release
```

## v2 decisions (Quick Enhancer 2.0)

* **i18n**: own tiny layer instead of fluent/rust-i18n: two TOML files embedded with
  `include_str!`, flattened to `section.key`, `{name}` placeholders, English fallback for missing
  keys, a global `RwLock<Lang>` so switching is live (the UI re-reads strings every frame).
  Tests check key parity, placeholder parity and that every `t!("…")` used in `src/` exists.
  Backend notes / errors are translated at creation time (they are shown, not parsed).
* **Thai font**: Noto Sans Thai Regular (OFL) embedded and added as a *fallback* after egui's
  default fonts, so Latin text keeps its look and only Thai glyphs come from Noto. egui has no
  OpenType shaping; Noto Sans Thai's default mark positions render ที่ / กิ่ง / น้ำ / ปู่ acceptably
  at UI sizes (checked with the font-test window). Upper vowel + tone stacks can sit slightly
  high; this is the known limit without a shaper.
* **Preview audio**: ffmpeg decodes the sample's (original) audio to stereo 48 kHz f32 PCM once;
  rodio plays it through a looping source that counts samples in an atomic. That sample position
  *is* the clock: both sides pick `frame = floor(clock × fps)` from it, so video can never drift
  from audio, and looping wraps both together. Without audio (no stream, no device, ¼ speed) a
  monotonic timer is the clock. Enhanced FPS > monitor refresh → notice (the display drops
  frames, the file doesn't).
* **History**: `history.json` in the data folder, appended when a job finishes (probe of the output
  in a background thread). The Outputs tab merges it with a scan of the configured output folder
  (only when one is set — "next to source" would mean scanning arbitrary folders), de-duplicated
  by normalized path, missing files marked. Thumbnails cached as PNG keyed by hash(path, size,
  mtime) and rendered on a worker thread.
* **Network isolation**: only `src/net/` uses ureq. Tool archives are streamed to disk while
  hashing; SHA-256 and size must match `tools.json` *before* extraction; extraction goes to a
  staging dir, every `files` rule must match, then files are moved into place (a running/locked
  exe is renamed to `.old` first). The installed versions are recorded in `tools-installed.json`;
  after an app update only components whose pinned version differs are offered again.
* **Updates**: `latest.json` attached to the newest GitHub release (`releases/latest/download/`),
  so no API token / rate limit. Check is opt-in (asked once, default off). The installer is
  downloaded to `%TEMP%\QuickEnhancer-update`, verified, launched with `/SILENT … /CLOSEAPPLICATIONS`,
  and the app closes itself; the installer's `[Run]` entry with `skipifnotsilent` relaunches it.
  Refused while a job runs. Failures only show an error + Releases link.
* **Installer**: Inno Setup 6 — small, scriptable, Thai translation built in, per-user install
  without admin (`PrivilegesRequired=lowest`). Tools are not bundled (size, separate licenses);
  the installer runs `QuickEnhancer.exe --install-tools`, i.e. the same verified downloader as
  the app. Uninstall asks before deleting user data (default No).
