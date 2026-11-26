# Quick Enhancer — Plan & Design Decisions

Native Rust desktop app (eframe/egui + wgpu) for local image & video enhancement:
AI upscaling (Real-ESRGAN / Real-CUGAN ncnn, ONNX models in-process), AI cleanup and face
restoration (ONNX), frame interpolation (RIFE ncnn), CAS / unsharp / denoise
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
  engines/           mod.rs (Upscaler / Interpolator traits, backend choice), realesrgan_ncnn.rs,
                     realcugan_ncnn.rs, rife_ncnn.rs, filters.rs (CAS, unsharp, bilateral
                     denoise), resize.rs
    onnx/            mod.rs (sessions, OnnxUpscaler), runtime.rs (DLL loading, EP chain),
                     tiler.rs (tiles, reflect padding, feather blend), face.rs (YuNet,
                     alignment, GFPGAN / CodeFormer, paste-back)
  models/            catalog.rs (models.json), install.rs (state, plans, uninstall),
                     custom.rs (models/custom scan), fit.rs (GPU fit, time estimate)
  ui/models.rs, ui/wizard.rs   AI models page + model pickers, first-run wizard
  media/             probe.rs (ffprobe JSON), ffmpeg.rs (decode/encode pipes, mux args),
                     encoders.rs (detection + test encode + per-encoder quality args)
  monitor/           mod.rs (unified provider + history), nvml.rs, pdh.rs (cfg(windows))
  devices/           mod.rs (Vulkan enumeration via ash, Auto selection)
tests/pipeline_filters.rs   integration test (skips if bin/ffmpeg missing)
tests/tools_archives.rs     #[ignore] real-archive / HTTPS install tests
tests/models_real.rs        #[ignore] real model downloads + runs (ncnn, ONNX, faces, RIFE)
installer/QuickEnhancer.iss Inno Setup 6 script;  scripts/release.ps1 release builder
tools.json                  pinned tool downloads (embedded);  locales/*.toml UI strings
models.json                 model catalog (embedded; refreshed from the models release)
tools/catalog/              gen_models_json.py (writes models.json);  tools/convert/ conversion
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
| windows (cfg(windows)) | PDH GPU counters, `SetPriorityClass`, DXGI adapter LUIDs, DLL search path |
| ort (`load-dynamic`, `directml`, `cuda`) | ONNX Runtime API; `onnxruntime.dll` is loaded at run time from the Model Manager's runtime folder, so the exe has no ORT dependency |
| half | fp16 tensors for fp16 ONNX models |
| ash (`loaded`) | Vulkan GPU enumeration; loads `vulkan-1.dll` at runtime so the app starts without Vulkan |
| rodio | preview audio output (cpal/WASAPI); audio decoded by ffmpeg to f32 PCM |
| ureq (rustls) + sha2 | blocking HTTPS for tool downloads / update check, streaming SHA-256 |
| zip | extracting the tool archives |
| trash | "Delete" in the Outputs tab goes to the Recycle Bin |
| winresource (build) | exe icon + version resource from Cargo.toml |

Heaviest pieces are eframe/wgpu (required) and `windows` (only 3 feature modules enabled).
No tokio, no FFmpeg bindings; ONNX Runtime only as a downloaded DLL.

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
* **ONNX backend** (v2.2): `OnnxUpscaler` implements `Upscaler` with `in_memory() == true`, so
  the pipeline skips the PNG round trip for it. See *v2.2 decisions*.
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

## v2.1 plan (Quick Enhancer 2.1)

Order of work: (1) Outputs/list overflow fix → (2) Video / Image pages → (3) named profiles →
(4) resource-aware settings (estimator, validation, benchmark, auto-tune) → (5) Editor page.
One commit per feature on the `v2.1` branch.

### How it fits the existing architecture

* **Pages are views, not pipelines.** `Page` grows to All-in-one / Video / Image / Editor /
  Outputs / Settings. The three *studio* pages (All-in-one, Video, Image) each own a
  `Workspace` (input list, selection, preview canvas, sample player, scrub position) and a
  `JobSettings`. The active workspace lives in `App::ws`; the other two are parked and swapped in
  on page switch (`App::switch_studio`), so all existing panel code keeps borrowing `app.ws.*`
  disjointly from `app.config` / `app.env`. Item ids stay globally unique, so runner and preview
  events are routed to whichever workspace holds the item. All jobs still go to the single
  `Runner`; `QueueItem::origin` records the page and the progress strip / queue cards show it.
* **Same engine.** Video and Image pages build ordinary `JobSettings` and use `StagePlan`,
  `run_video`, `run_image` and the preview worker unchanged. New per-stage switches
  (`denoise_on`, `sharpen_on`) and the new `clarity` filter are fields of `JobSettings`
  neutralised in `effective()`, defaulting to values that keep the All-in-one page's behaviour
  identical. "Restore at original size" is `ScaleMode::X1` with an AI profile (already
  implemented as AI at native scale → Lanczos back down).
* **Machine vs. recipe.** Hardware-dependent values move to `Config::machine`
  (`MachineSettings`: device, CPU threads, preset, tile, `-j`, filter threads, priority, chunk
  size, RAM budget, temp folder). `JobSettings` keeps the same fields because the pipeline reads
  them, but they are overwritten from `machine` right before a job / preview starts
  (`JobSettings::with_machine`). Old configs are migrated once: if `[machine]` is missing it is
  taken from the old `[settings]` values.
* **Profiles** (`src/profiles.rs`): one TOML file per user profile in `<data>/profiles/`
  (`schema_version`, `app_version`, `kind`, `name`, `settings`, optional `performance`).
  Built-ins are defined in code with translated names. Identified as `builtin:<id>` /
  `user:<name>`. The comparison for the "modified" marker uses `recipe()` (settings with
  machine fields, output folder and overwrite policy normalised).
* **Resources** (`src/resources.rs`, pure + unit-tested): `estimate()` (temp per chunk / total,
  RAM, VRAM, threads, time) and `validate()` → `Issue { level: Warn | Block, … }` with safe
  automatic fixes. `src/bench.rs`: cancellable benchmark (Real-ESRGAN at several tile sizes,
  RIFE, filters, PNG write, encoder) stored per GPU in `<data>/benchmark.json`; `auto_tune()`
  derives tile / chunk / RAM / threads / `-j` from it. Pre-start check runs before the overwrite
  prompt; the video decoder re-checks free temp space before each new chunk and pauses the job
  (instead of failing) when it is too low.
* **Editor** (`src/editor/`): pure ffmpeg argument builders (trim fast/accurate, crop, concat
  demuxer list, re-encode filter graph) + an `EditOp` carried by `JobSpec::edit`, executed by the
  same `Runner` (progress via `-progress pipe:1`). Image edits (crop / rotate / flip) use the
  `image` crate and keep EXIF/ICC where the encoder supports it. Results go to history with
  `edited = true`.

### Files
* changed: `app.rs`, `config.rs`, `history.rs`, `ui/mod.rs`, `ui/queue.rs`, `ui/settings.rs`,
  `ui/outputs.rs`, `pipeline/{job,runner,stages,video_job}.rs`, `engines/filters.rs`,
  `locales/*.toml`, `main.rs`
* new: `profiles.rs`, `resources.rs`, `bench.rs`, `editor.rs`,
  `ui/{studio,profiles,perf,editor,settings_page}.rs` (the Video and Image pages share
  `ui/studio.rs`; the editor's ffmpeg builders live in `editor.rs`)

### v2.1 Decisions
* **No questions were needed**; everything below was decided from the prompt + code.
* Work happens on branch `v2.1` (one commit per feature), not directly on `main`.
* Outputs grid: fixed-size cards (thumbnail 176 px) laid out in `floor((w+gap)/(card+gap))`
  columns inside `ScrollArea::show_rows`, so only visible rows are laid out and only their
  thumbnails are requested. List view: fixed type/size/date columns, name + folder columns share
  the rest and are elided in the middle (full text on hover).
* Application settings (language, updates, tools, font test) and About/Paths moved from the
  All-in-one side panel to the new **Settings** page, together with the performance
  (Simple/Advanced) controls. The All-in-one panel keeps its processing sections; its Device and
  Video-pipeline sections now edit the global machine settings.
* Output folder and overwrite policy are per page but **not** part of a profile (they are
  machine/user specific); file-name suffix, formats, encoder and quality are.
* Hardware values are global machine settings; a profile stores them only when "include
  performance settings" is ticked on save. Loading such a profile applies them to the machine
  settings (toast shown); importing one runs the resource checks and offers "Adjust to this PC".
* Max chunks on disk stays fixed at 2 (the 2-token semaphore); shown read-only.
* Local contrast ("clarity"): large-radius (σ≈ 1.2 % of the short side, min 6 px) unsharp on
  luma only, applied after sharpening; cheap (separable blur) and halo-limited by a soft clamp.
* VRAM estimate: Real-ESRGAN heavy models ≈ 1.9 GB at tile 200 scaling with tile², compact
  (animevideo) ≈ 35 % of that, × ncnn proc threads; RIFE ≈ 600 B per source pixel (½ in UHD
  mode). Auto tile stays capped at 256 (TDR safety, see v1 decision); auto-tune never exceeds it.
* Fast trim uses stream copy with `-ss` before `-i` (keyframe cut); accurate trim re-encodes
  with the app's encoder. Join without crop and with matching codec/size/fps/pixel format/audio
  layout uses the concat demuxer with `inpoint`/`outpoint` (stream copy); otherwise a
  `filter_complex` concat with scale+pad (letterbox), `fps`, `aresample`, and `anullsrc` for
  clips without audio.
* Profiles are stored one TOML per profile as `<data>/profiles/<page>-<safe name>.toml`
  (`schema_version`, `app_version`, `name`, `kind`, `settings`, optional `performance`). Loading
  applies only the recipe (`JobSettings::recipe()`): output folder, overwrite policy and machine
  fields are never taken from a profile file. "Modified" (`*`) compares the recipe with the
  loaded profile. Built-ins are code, read-only, translated, and can be duplicated. Default and
  last-used profile and the recent list (5) are stored per page in `config.toml`; the default
  is applied at startup.
* Resource checks are pure functions over a `SystemInfo` snapshot (refreshed every 3 s, not
  per frame) so they can be unit-tested with fixed numbers. Levels: *Warn* allows starting
  (after a confirmation in the pre-start dialog), *Block* doesn't. "Fix automatically" only
  changes machine settings (+ encoder → Auto when the chosen one is unusable); it never changes
  the recipe. The "FPS above monitor refresh" warning is shown inline only, not in the pre-start
  dialog (it is informational: the file still has every frame).
* Temp estimate: PNG ratio measured on real frames (EMA over `write_png_fast`), else the
  benchmark's ratio, else 0.6. A running video pauses before creating a chunk while the temp
  drive has less free space than one expanded chunk + 256 MB (status shows why) and resumes on
  its own; cancel still works.
* Benchmark: ~20 s on this RTX 5060 Ti (PNG write 8 frames, CPU filters, Real-ESRGAN x4plus on
  640x360 at tiles 128/192/256 with a 1- vs 4-frame run to separate start-up from per-frame
  cost, RIFE 2x on 4 frames, encoder 120 frames at 720p). Results per device in
  `<data>/benchmark.json`; offered once after the first tools install. Auto-tune runs after a
  benchmark and on request: fastest working tile (≤ 256 and VRAM-safe), `-j` 2:2:4 with ≥ 8
  cores and spare VRAM, filter threads ≈ 75 % of cores, RAM budget = min(½ available, ¼ total),
  chunk ≥ 25 s of GPU work (min 32) limited to 40 % of free temp space. Measured here: tile 128
  was the fastest (2.2 s/MP for x4plus), so auto-tune picks 128, chunk 32, RAM 3.3 GB.
* Encoder preset (Fast/Balanced/Quality → NVENC p3/p5/p7, x264/x265 fast/medium/slow, QSV, AMF,
  SVT-AV1 presets) and bitrate mode (VBR with maxrate 1.5× / bufsize 2×) are per page output
  settings; *Balanced + constant quality* produces exactly the 2.0 arguments.
* Editor jobs are `JobSpec`s with `edit: Some(EditJob)` on the same runner thread (queued,
  cancellable, progress via ffmpeg `-progress <file>`). Outputs go next to the source as
  `<name>_edited.<ext>` (auto-numbered, never overwritten) and are recorded in the history with
  `edited = true` ("✂ Edited" on the Outputs page). Crop rectangles are always even and inside
  the frame; image crops are defined on the rotated/flipped view (ops order: rotate → flip →
  crop). Images keep their ICC profile and EXIF when the format supports it; EXIF is dropped
  when the source had its own orientation tag (it is baked into the pixels instead). Undo/redo
  coalesces drags and typing into one step; a clip removed from the join list can't be undone.
* Selection playback in the editor decodes at most 20 s at ≤ 960x540 into memory and reuses
  `SamplePlayer` in a new single-view mode (audio is the clock, as in the sample preview).

## v2.2 decisions (Quick Enhancer 2.2: more models, ONNX backend, Model Manager)

Feasibility study and approved choices: `docs/FEASIBILITY_v2.2.md`. One commit per feature on
`feature/v2.2-models`.

* **Catalog:** `models.json` (schema 1) lists *packages* (downloads: url, sha256, size, archive
  `zip` or `file`, files → destinations, licence) and *models* (group, backend, scales, speed /
  quality 1–5, VRAM min / recommended, cost s/MP on the reference PC, licence with commercial /
  redistribute flags, `onnx` spec: file, tile, static, precision, range). It is embedded and
  refreshed from the `models-YYYY.MM` release; a cached copy is used only when its
  `catalog_version` is newer. Generated by `tools/catalog/gen_models_json.py`; packages share the
  `tools.json` installer (verify SHA-256, safe extraction, atomic rename) and the tools.json
  components (FFmpeg, Real-ESRGAN, RIFE) appear as packages too.
* **Install state** is derived from the files on disk (`InstallSnapshot`), not from a database:
  Installed / NotInstalled / Partial / Outdated. An install plan is runtime → helper models →
  the model's own packages, skipping what is present (Repair re-fetches the model's own
  packages). Uninstall keeps packages and files other usable models still need, and never
  removes tools.json components or runtimes (those have their own buttons).
* **Runtimes:** DirectML (`onnxruntime-directml` wheel, DirectML.dll redistributable only inside
  an app, so we download it from PyPI instead of hosting it) or CUDA (onnxruntime-gpu + NVIDIA
  CUDA / cuBLAS / NVRTC / cuDNN wheels, NVIDIA only). Only one `onnxruntime.dll` per process:
  the preferred runtime if installed, else the other; a switch takes effect after a restart.
  `SetDllDirectoryW` + preloading make the runtime's own DLLs win over System32 copies. The
  Visual C++ runtime is deployed app-local (`vcrt-dml` / `vcrt-cuda`: the three DLLs from the
  official vc_redist, hosted on the models release) and preloaded first, so a PC without the
  VC++ Redistributable works; the log records which `msvcp140.dll` is in use. Load errors
  suggest Repair or the official redistributable.
* **Execution providers:** DirectML on the DXGI adapter whose LUID matches the chosen Vulkan GPU
  (or CUDA on that NVIDIA card's CUDA index) → CPU, with a toast when falling back. DirectML
  sessions use no memory pattern and sequential execution. Up to 5 sessions stay cached
  (cleanup + upscaler + detector + face model of one job, plus a preview).
* **Tiling:** static exports run exactly `tile × tile` (reflect-padded inside the tile region),
  dynamic ones a multiple of 8; overlap tile/16 (8–32 px), linear feather across overlaps;
  output rows are blended into a band buffer and flushed once no later tile touches them.
  Out-of-memory (DirectML `887A0005/6`, `E_OUTOFMEMORY`, CUDA OOM) halves the tile of dynamic
  models down to 64 with a notice; static models stop with an explanation.
* **Pipeline order:** AI cleanup (1x ONNX) → denoise → (RIFE) → upscale → faces → resize →
  sharpen. ONNX stages run on in-memory frames; ncnn stages keep the chunked PNG pipeline. With
  frame generation, ONNX upscaling runs after the frames are read back from the RIFE folder.
* **Faces:** YuNet 2023mar (static 640, letterboxed, BGR 0..255), score ≥ 0.6, NMS 0.3, at most 32
  faces, faces < 24 px skipped; least-squares similarity to the FFHQ 512 five-point template;
  bilinear warp; restore; inverse warp with a 24 px border mask blurred with σ 12. GFPGAN is
  blended by *strength*; CodeFormer gets `weight = 1 − strength`. Faces run after the upscale,
  on the upscaled frame. Our own GFPGAN export works on DirectML after `fix_double.py`, so
  FaceFusion's files were not needed.
* **Frame generation:** the TNTwise rife-ncnn-vulkan build (RIFE v4.26 default, v4.25-lite, v4.6,
  v4), repacked slim on our models release. A configured model that is missing falls back to
  v4.26 → v4.6 → v4.25-lite → v4. GIMM-VFI was dropped in the feasibility study.
* **SeedVR2 dropped:** the one allowed reference run of the upstream `pipeline.py` (int8, CUDA,
  RTX 5060 Ti 16 GB, 16 GB RAM) took 58 s for 17 frames at 512×288 with +10.5 GB VRAM and
  +6.3 GB RAM. Memory grows with the frame area (windowed DiT attention, VAE decoder), so useful
  sizes can't fit 16 GB, and a Rust port would run the same graphs.
* **Licences:** each model's licence, credits and commercial flag come from the catalog; models
  with `commercial: false` (CodeFormer) need a one-time "I will not use it commercially"
  confirmation (stored in `config.accepted_licenses`). Only files whose licence allows
  redistribution are hosted on our release; the rest download from their original source.
* **Estimator:** time = this PC's x4plus benchmark × the model's catalog cost relative to x4plus
  (6 s/MP), plus cleanup and ~0.05 s per face; VRAM from the catalog for ONNX sessions;
  no upscaled PNG temp for in-memory ONNX stages.
* **Custom models:** `models/custom/*.param+bin` (ESRGAN-family, run by realesrgan-ncnn-vulkan)
  and `*.onnx` (RGB 0..1 in / out; scale from the name or a 32×32 probe run).
* **Build:** GNU toolchain (WinLibs) kept; `ort` is `load-dynamic`, so no import library is
  needed; libgcc / winpthread are linked statically and `scripts/release.ps1` fails if the exe
  imports a MinGW DLL.

## v2.3 decisions (Quick Enhancer 2.3: "Make sharp (AI)")

The evaluation behind these choices (candidates, LPIPS / PSNR numbers, timings, licences) is in
[docs/DEBLUR_EVAL.md](docs/DEBLUR_EVAL.md).

* **Where:** a third mode on the Image page, next to Upscale and Restore at original size. Images
  only; video is unchanged. One code path: `StagePlan::process_still` runs `SharpPlan::process`
  for the preview, the region preview and the export.
* **Pipeline:** blur analysis → deblur (Restormer motion or defocus, by blur type) → smart
  downscale to the effective sharp resolution (capped per preset: Portrait / Light 2.5, Old photo
  2.0, else 4.0) → RealPLKSR Nomos2 4x back up → target size (original / 2x / 4x) → optional
  generative pass blended in by the Faithful ↔ Creative amount → faces (GFPGAN, or CodeFormer
  when chosen) → light CAS.
* **Blur analysis** is pure Rust: the sharp factor is the smallest downscale that loses < 10 %
  of the gradient energy after a round trip; the motion score compares directional gradient
  energy. Kinds: sharp / light / strong / motion / low-res (under 0.6 MP).
* **Faithful by default, Creative on request:** the faithful chain cut LPIPS from 0.412 to 0.254
  across the test set; 50 % SeedVR2 gave 0.246 overall and was best on defocus and low-res, but
  worse on motion and JPEG-damaged blur. So Auto uses 50 % only for Strong blur / Out of focus /
  Old photo / Portrait, and 0 % for Light blur and Camera shake. The generative model gets the
  original image, except after motion deblur (the defocus model's overshoot hurt it).
* **Generative model: SeedVR2-3B** (Moonbox ONNX int8 export, Apache-2.0), one image = one frame.
  Ported from Moonbox's `pipeline.py` (windows, RoPE inputs, tiled VAE with linear-ramp blending)
  and checked against its numbers. A colour fix (`out − blur(out) + blur(ref)`, σ 8) removes its
  warm shift. On an RTX 5060 Ti (DirectML): about 90 s at 1 MP, 115 s at 2 MP with 256 px VAE
  tiles, peak +5.9 GB VRAM. The working size is capped at 2.1 MP (1 MP on GPUs with ≤ 7 GB).
  The 2.2 "SeedVR2 dropped" decision was about video; a single still at ≤ 2 MP fits.
* **Worker process:** the generative phases (encode, restore, decode) each run in
  `QuickEnhancer.exe --worker <task.json>` so a driver-level out-of-memory or crash can't take
  the app down, and memory is returned to the OS between phases. The protocol is one JSON task
  file in, lines on stdout out (`progress`, `error`, `oom`, `done`). Out-of-memory in the restore
  phase halves the working size; elsewhere the tile shrinks (256 → 192 → 128), then a clear
  message suggests Faithful.
* **Honesty about generative output:** the notice next to the slider, a "✦ AI-generated details"
  mark in History and Outputs, and a note in the file (PNG `tEXt` Comment / JPEG `COM`:
  "Processed with generative AI by Quick Enhancer").
* **Preview region:** the preview processes a box (default 512 px around the largest face, else
  the centre), using the whole image's analysis so the box gets the same preset and models as
  the export. Faithful vs Creative side by side and Process full image are buttons.
* **Hosting:** Restormer ONNX fp16 files (MIT) and the schema 2 catalog are on the
  `models-2026.12` release (`--latest=false`); SeedVR2 (3.56 GB weights, over GitHub's 2 GB
  asset limit) downloads from Hugging Face at a pinned revision with SHA-256 checks. The catalog
  schema went to 2 (new group `sharp`, backend `seedvr2`, `sample` links); 2.2 keeps reading the
  unchanged schema 1 catalog on `models-2026.11`.
* **Not used:** NAFNet (rings on anything but motion blur), MPRNet (non-commercial), OSEDiff
  (needs the withdrawn Stable Diffusion 2.1 base and a tagger, garbles text, changes faces),
  DAT2 4x as the detail model (slower and worse than RealPLKSR here), SUPIR (not evaluated).
