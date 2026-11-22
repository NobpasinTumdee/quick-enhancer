# Task: Build "Enhancer" — a native Rust desktop app for local image & video enhancement

Build a complete, working, lightweight native desktop application in **Rust** that enhances local images and videos (AI upscaling, frame generation/interpolation, sharpen/denoise) with explicit **GPU / CPU selection**, **real-time resource monitoring**, **strictly bounded memory/disk usage**, and an interactive **Before / After preview**.

---

## 0. How to work (read first)

- **Build the whole app in one continuous run.** Do not stop after the plan or after each module to ask for approval. Write a short plan to `PLAN.md` (architecture, file tree, crates, external binaries), then immediately implement everything through to a finished, compiling, documented app.
- Run `cargo check` after each major module and `cargo build --release`, `cargo clippy -- -D warnings`, `cargo test` at the end. Fix all errors and warnings before reporting done.
- Communicate progress, questions and the final summary to me **in Thai**. Code, comments, UI text and README are in **English**.
- Use the latest stable versions of crates that compile together; don't guess APIs — check docs/source when unsure.

## 1. Target platform & hardware

- **Primary target: Windows 10/11 x64.** Code should stay portable where cheap (Linux build should compile, with Windows-only features behind `cfg(windows)`), but Windows is what must fully work.
- Dev/test machine: **NVIDIA RTX 5060 Ti 16 GB** (Blackwell). Must also work cleanly on AMD/Intel GPUs and on machines without a usable GPU.
- Release build should be a single `.exe` plus `bin/` and `models/` folders next to it (portable, no installer).

## 2. Fixed technology choices

- **UI:** `eframe`/`egui` with the `wgpu` renderer. (Not Tauri.) Custom canvas for the preview (zoom/pan/wipe) done with egui textures/painter.
- **Concurrency:** `std::thread` worker threads + `crossbeam-channel` bounded channels. **Do not add `tokio`** unless a concrete need appears.
- **Errors/logging:** `thiserror` in library modules, `anyhow` at app level, `tracing` + `tracing-subscriber` with a rolling log file in `logs/`.
- **Images:** `image` for decode/encode, `fast_image_resize` for resizing, `rayon` for CPU-parallel filters.
- **Video:** FFmpeg/FFprobe as **subprocesses** (no FFmpeg Rust bindings): ffprobe JSON for metadata, ffmpeg raw-video pipes (`-f rawvideo -pix_fmt rgb24`) for decode/encode where streaming is possible.
- **AI engines (default backend):** `realesrgan-ncnn-vulkan` and `rife-ncnn-vulkan` executables run as subprocesses. Put engines behind traits so an `ort` (ONNX Runtime + DirectML/CUDA) in-process backend can be added later — implement the trait and stub, not the full ort backend.
- **Telemetry:** `sysinfo` (CPU/RAM), `nvml-wrapper` (NVIDIA, loaded dynamically; app must start fine if NVML is absent), Windows **PDH** counters via the `windows` crate (`\GPU Engine(*)\Utilization Percentage`, `\GPU Adapter Memory(*)\Dedicated Usage`) as the vendor-neutral fallback. GPU enumeration via Vulkan (`ash`) so device indices match the `-g` index used by ncnn tools.
- **Settings:** persisted to `config.toml` next to the exe (`serde` + `toml`).
- Keep dependencies lean; justify any heavy crate in `PLAN.md`.

## 3. Folder structure (adjust if needed, document in PLAN.md)

```
src/
  main.rs
  app.rs                 # eframe App, top-level state
  ui/                    # panels: input/queue, settings, monitor, output, log
  preview/               # before/after canvas: wipe, side-by-side, toggle, zoom/pan, video scrubber
  pipeline/              # job queue, job runner, chunk planner, cancel, progress/ETA, temp management
  engines/               # traits + realesrgan_ncnn, rife_ncnn, ort_stub, filters (CAS, unsharp, denoise), resize
  media/                 # ffprobe metadata, ffmpeg decode/encode pipes, encoder detection, audio mux
  monitor/               # sysinfo, nvml, pdh, unified GpuStats provider
  devices/               # Vulkan GPU enumeration, "Auto" selection logic
  config.rs
bin/       # ffmpeg.exe, ffprobe.exe, realesrgan-ncnn-vulkan.exe, rife-ncnn-vulkan.exe (not committed)
models/    # realesrgan/*.param|bin, rife/<model-dir>/ (not committed)
```

## 4. Functional requirements

### 4.1 Input
- Drag-and-drop files and folders onto the window, plus file picker and folder picker (`rfd`).
- Images: PNG, JPG/JPEG, WebP, BMP, TIFF. Videos: MP4, MKV, MOV, WebM, AVI. Detect type by content/extension; skip unsupported files with a visible message.
- Folders are added recursively (option) as **a job queue**. Each queue item shows thumbnail and metadata: resolution, FPS (and whether VFR), duration, video/audio codec, bit depth / HDR flag, file size.
- Queue: reorder, remove, per-item status (pending / running / done / failed / cancelled), "apply current settings to all / selected".

### 4.2 Upscaling (content-type profiles, like Lossless Scaling)
Map each profile to a concrete engine and model:

| Profile | Engine / model | Notes |
|---|---|---|
| Realistic / Photo | Real-ESRGAN `realesrgan-x4plus` | native 4x only |
| Anime / 2D | `realesr-animevideov3` (x2/x3/x4) for video, `realesrgan-x4plus-anime` for images | |
| Pixel Art / Retro | **no AI**: integer nearest-neighbor scaling (optionally xBRZ-style) | AI models smear pixel art |
| General | `realesr-animevideov3` or a general model if present; fall back to Lanczos | |

- Scale options: `1x` (enhance only, no upscale), `2x`, `3x`, `4x`, or **target resolution** (720p / 1080p / 1440p / 4K, keep aspect ratio, even dimensions for video).
- When the model's native scale doesn't match the requested scale (e.g. 3x with a 4x-only model, or a target resolution), run the nearest native scale ≥ requested, then downscale with `fast_image_resize` (Lanczos3). Show this in the UI ("4x model → downscaled to 3x").
- If a model file is missing, disable that option in the UI with a tooltip saying which file to place where.

### 4.3 Frame generation (video only)
- Options: `Off`, `2x`, `3x`, `4x`, or **target FPS** (e.g. 60 / 120). Output FPS = source FPS × multiplier, with a configurable max (default 120). Show the resulting FPS before starting.
- Engine: `rife-ncnn-vulkan` (model dir selectable if multiple RIFE versions are present).
- VFR sources: convert to CFR at the source's average frame rate before processing; warn in the UI.
- **Scene-cut handling:** detect cuts yourself (e.g. downscaled-luma histogram/SAD difference between consecutive frames with a configurable threshold, or ffmpeg `scdet`). At a detected cut, **do not interpolate across it** — fill the generated slots by duplicating the frame before the cut. Unit-test the detector and the slot-filling logic.
- Order when combined with upscaling: interpolate at source resolution first, then upscale (cheaper). Document this.

### 4.4 Sharpen / clarity / denoise
- Implement in Rust (rayon), applied to RGB frames, usable alone or combined with upscale/frame-gen:
  - **CAS** (Contrast Adaptive Sharpening) with strength 0–1.
  - Unsharp mask (radius, amount, threshold) as an alternative mode.
  - Denoise with strength slider (e.g. fast bilateral or simplified NL-means; pick one that is fast enough for video, document the choice).
- Filter order: denoise → upscale → sharpen.
- **The preview must use exactly the same code path as the final export** so what the user sees is what they get.

### 4.5 Device selection & resource budget
Keep **compute device** and **video encoder** as two separate settings:
- **Compute device:** `Auto`, each Vulkan GPU listed by name + VRAM, or `CPU` (with thread count). `Auto` = discrete GPU with most VRAM, else integrated, else CPU. Passed to ncnn tools via `-g <index>` (`-g -1` for CPU — warn that CPU is very slow for AI models; filters still run on CPU threads).
- **Video encoder:** `Auto`, `NVENC (H.264/HEVC/AV1)`, `AMF`, `QSV`, `libx264`, `libx265`, `libsvtav1`. Detect availability at startup with `ffmpeg -encoders` **and** a 1-frame test encode (an encoder can be listed but unusable). Grey out unavailable ones.
- Performance preset sets defaults that the user can still override individually:

| Preset | ncnn tile size | ncnn `-j` load:proc:save | Filter threads | Process priority |
|---|---|---|---|---|
| Eco | small (e.g. 128) | 1:1:1 | 25% of cores | Below normal |
| Balanced | auto (e.g. 256) | 1:2:2 | 50% | Normal |
| Max | 0 (auto/largest) | 2:4:4 | all − 1 | Above normal |

  Apply priority to child processes on Windows (`SetPriorityClass`).
- Tile size: `Auto` derives from free VRAM; on ncnn out-of-memory / failure, automatically retry the chunk with half the tile size (down to a minimum) and log it.
- FP16: ncnn tools choose FP16 automatically on supported GPUs — show it as informational only. (FP16 toggle is only real for the future ort backend.)

### 4.6 Bounded video pipeline (memory and disk)
The ncnn CLIs work on image files, so true zero-temp-file streaming is not possible with this backend. Instead:
- Process video in **chunks** (default 120 source frames, configurable): ffmpeg extracts one chunk to a temp folder → filters/RIFE/Real-ESRGAN process it → results are streamed into a **single long-running ffmpeg encoder** via raw-video pipe → chunk temp files are deleted immediately.
- At most 2 chunks exist on disk at once (one being processed, one being prepared). Show the temp-space cap in settings and check free disk space before starting.
- In-memory frame queues use bounded channels sized from the RAM budget; RAM usage must stay roughly flat regardless of video length.
- Temp folder configurable (default system temp); fully cleaned on finish, cancel, error, and on next startup (stale folders).
- Preserve audio (copy stream if container allows, otherwise re-encode AAC), subtitles where the container supports them, and basic metadata. Audio stays in sync when FPS changes.
- 10-bit/HDR sources: process as 8-bit SDR in this version and **warn clearly in the UI** before starting.

### 4.7 Output settings
- Output folder (default: next to source, `<name>_enhanced.<ext>`), overwrite policy (ask / skip / auto-rename).
- Images: format (PNG / JPG / WebP) + quality.
- Video: container (MP4 / MKV), encoder (see 4.5), quality mode (CQ/CRF value) with sensible per-encoder defaults.

### 4.8 Real-time resource monitor
- Panel updated ~2×/second from a dedicated thread (never blocks UI or workers): CPU %, RAM used/total, GPU % and VRAM used/total for the active GPU, plus a small 60-second sparkline for each.
- Provider order: NVML (NVIDIA) → PDH (any vendor on Windows) → "not available" shown gracefully, never a crash.
- Job panel: overall %, current file, frames done/total, current stage (extract / denoise / interpolate / upscale / sharpen / encode), speed (FPS or items/s), ETA, active device + encoder, temp disk used.

### 4.9 Before / After preview (key feature)
- Modes: **wipe slider** (drag divider), **side-by-side** (synced zoom/pan), **toggle** (button + hotkey, e.g. Space).
- Zoom & pan: mouse-wheel zoom around cursor, drag to pan, `Fit` and `100%` buttons, zoom level indicator. Use nearest-neighbor sampling at high zoom so pixels are inspectable.
- Because the enhanced image is larger, display the original **scaled to the output resolution** so both sides align pixel-for-pixel.
- Preview generation runs in the background with a spinner and is cancellable; changing settings re-renders the preview (debounced).
- **Video:**
  - Frame scrubber (timeline slider + frame number) → process just that frame (upscale + filters) for the wipe/side-by-side compare.
  - "Preview sample" button: process a short clip (default 3 s from the scrubber position, all settings incl. frame generation) and play Original vs Enhanced side-by-side in a loop inside the app (decode via ffmpeg pipe to textures; playback can be at reduced display size). This is how frame generation is judged, since a single still can't show it.
  - After export, the preview can load the final output file for comparison.

### 4.10 Cancel, errors, robustness
- Cancel current job and cancel whole queue: kill child processes, flush/abort the encoder, delete partial output (option to keep), clean temp.
- Pause/resume between chunks (nice to have).
- Every external-process failure shows a readable error in the UI with the stderr tail and is written to the log. One failed file must not stop the queue.
- First-run check: verify `bin/` tools and `models/` files, show a setup panel listing exactly what's missing and where to put it, instead of failing.

## 5. Tests (minimum)
- Unit tests: ffprobe JSON parsing, scale/target-resolution math, chunk planner, scene-cut detector + slot filling, CAS/unsharp on small synthetic images, preset → settings mapping.
- Integration test (skipped if `bin/ffmpeg` is missing): generate a 3-second test video with `ffmpeg -f lavfi -i testsrc`, run the pipeline with filters only (no AI), check output duration/FPS/resolution.

## 6. README.md
Concise, covering:
- Build & run (`cargo run --release`), requirements (Rust stable, Vulkan-capable GPU driver).
- Exactly which files go in `bin/` and `models/` (expected file names and folder layout) with links to the official release pages (FFmpeg Windows builds, `xinntao/Real-ESRGAN-ncnn-vulkan` or `Real-ESRGAN` releases, `nihui/rife-ncnn-vulkan` releases).
- How GPU/CPU selection, presets, tile size and encoders work.
- How the chunked pipeline bounds RAM/disk use.
- Known limitations (8-bit SDR only, CPU AI is slow, pixel-art profile is non-AI, no ort backend yet, license note: FFmpeg builds with x264/x265 are GPL).

## 7. Definition of done
- `cargo build --release`, `cargo clippy -- -D warnings` and `cargo test` all pass.
- App launches without any `bin/` or `models/` present and shows the setup panel (no panic).
- With tools present: an image and a short video can be enhanced end-to-end from the UI, with live monitor, working before/after preview, and working cancel.
- `PLAN.md` (plan + decisions) and `README.md` are complete.
- Final message to me (in Thai): what was built, how to run it, what I must download and where to put it, decisions you made, and any limitations or unfinished parts — honestly.
