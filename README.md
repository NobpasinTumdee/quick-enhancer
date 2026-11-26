# Quick Enhancer

A lightweight native Windows app (Rust + egui/wgpu) that makes **images and videos** look better,
entirely on your PC:

* AI **upscaling** with Real-ESRGAN, Real-CUGAN and modern community models (RealPLKSR, SPAN,
  DAT2, HAT-L), plus your own ncnn / ONNX models
* AI **cleanup** at the original size (JPEG artefacts, noise) and **face restoration** (GFPGAN,
  CodeFormer)
* **Make sharp (AI)** for blurry photos: camera shake and out-of-focus blur removed (Restormer),
  detail rebuilt, and an optional **Creative** generative pass (SeedVR2) for very blurry pictures
* AI **frame generation** with RIFE 4.26 (2x/3x/4x or a target FPS) with scene-cut protection
* An **AI models** page: install only the models you need (sizes, speed / quality, whether they
  fit your GPU, licence of each model), plus a first-run setup wizard
* **Sharpen / denoise** (CAS, unsharp mask, edge-preserving bilateral denoise)
* Before / After **preview** with a wipe slider, side-by-side and a **sample clip player with
  sound** (per-side FPS and resolution)
* Separate **Video** and **Image** pages (plus the classic All-in-one page), named **profiles**
  per page, and a simple **Editor** (trim, crop, join, rotate)
* **Resource-aware** settings: live temp-disk / RAM / VRAM / time estimates with warnings and
  "Fix automatically", a 20–60 s **benchmark** and **auto-tune** for your PC
* **Outputs** tab: browse, compare, open or delete everything you made
* English and **Thai (ไทย)** interface, switchable live
* Explicit GPU / CPU and video-encoder choice (NVENC / AMF / QSV / x264 / x265 / SVT-AV1), live
  CPU / RAM / GPU / VRAM monitor, bounded RAM and temp-disk use for any video length

Windows 10/11 x64. No account, no telemetry; nothing leaves your PC.

---

## Install

1. Download `QuickEnhancer-Setup-<version>.exe` from the project's **GitHub Releases** page.
2. Run it. It installs for your user only (no admin prompt; "all users" is offered in the first
   dialog). Choose English or Thai, read the third-party notices, optionally tick *desktop
   shortcut*.
3. At the end the installer opens the **setup wizard**: FFmpeg (required) plus what you want to
   do — upscale photos, upscale anime, clean up old photos, restore faces, smooth motion — and,
   on NVIDIA cards, the optional CUDA runtime. The wizard shows the total download size. Each
   file is downloaded from a pinned URL and checked against its SHA-256 before anything is
   extracted. You can skip it and install models later on the **AI models** page (or from a
   model's ⬇ entry in any model list), or install tools from archives you downloaded yourself
   (Settings → Application → **Manage tools** → *Install from files… / From folder…*).

> Windows SmartScreen may say "Windows protected your PC" because the installer is not
> code-signed: choose **More info → Run anyway**.

**Uninstall** from Windows Settings → Apps. The uninstaller asks whether to delete your settings,
history and downloaded tools as well; the default is to keep them.

### Portable use

Put `QuickEnhancer.exe` in a folder together with an (empty) `portable.txt` file. Everything —
settings, history, thumbnails, logs, `bin/` and `models/` — then lives in that folder, and the tool
downloader installs there too. `cargo run` is always portable (it uses the project folder).

### Where things are stored

| | Installed | Portable |
|---|---|---|
| Program | `%LOCALAPPDATA%\Programs\QuickEnhancer` (per user) | the exe's folder |
| Settings, history, thumbnails, logs | `%APPDATA%\QuickEnhancer` | the exe's folder |
| Tools and models (`bin\`, `models\`, `licenses\`) | `%LOCALAPPDATA%\QuickEnhancer` | the exe's folder |

`QUICK_ENHANCER_HOME=<folder>` (or the old name `ENHANCER_HOME`) forces everything into one
folder. Settings → **About / Paths** shows every folder with an *Open folder* button. Settings from
Enhancer 1.x are imported once on first start.

### Language

Settings → Application → **Language** (English / ไทย) switches immediately and is remembered. On
first start Thai is chosen when Windows' display language is Thai. Thai uses the embedded Noto Sans
Thai font; *Thai font test* (or `QuickEnhancer.exe --font-test`) shows tone-mark combinations.

### Updates

Settings → Application → **Check for updates** asks GitHub Releases whether a newer version
exists, shows what's new, downloads the installer, verifies its SHA-256, installs silently and
restarts the app. Settings, history and tools are kept; a tool is re-downloaded only if its pinned
version changed. Updating is refused while a job is running. *Check for updates when the app
starts* is **off** by default (you are asked once). If anything fails the app keeps working and
links to the Releases page for a manual download. The only network traffic is these update checks
and the tool downloads; no data about you or your files is sent.

## Command line

```
QuickEnhancer.exe [files or folders…]   add them to the queue
QuickEnhancer.exe --install-tools       only the tool downloader (used by the installer)
QuickEnhancer.exe --font-test           open the Thai font test window
```

## Building from source

See [docs/BUILDING.md](docs/BUILDING.md) (Rust toolchain, MSVC or GNU, installer). Publishing a
release: [docs/RELEASING.md](docs/RELEASING.md). Changes: [CHANGELOG.md](CHANGELOG.md). Third-party
licenses: [licenses/THIRD_PARTY.md](licenses/THIRD_PARTY.md).

Tools can also be placed by hand (e.g. for development): put them in `bin/` and `models/` of the
data folder for tools (see the table above):

| Path | Where to get it |
|---|---|
| `bin/ffmpeg.exe`, `bin/ffprobe.exe` | <https://www.gyan.dev/ffmpeg/builds/> (essentials) or <https://github.com/BtbN/FFmpeg-Builds/releases>. If missing, ffmpeg/ffprobe on `PATH` are used. |
| `bin/realesrgan-ncnn-vulkan.exe` + `bin/vcomp140.dll` | `realesrgan-ncnn-vulkan-20220424-windows.zip` from <https://github.com/xinntao/Real-ESRGAN/releases> (v0.2.5.0) |
| `bin/rife-ncnn-vulkan.exe` | the TNTwise build `20250112` from <https://github.com/TNTwise/rife-ncnn-vulkan/releases> (needed for RIFE v4.26; nihui's `20221029` build still runs v4.6 and older) |
| `models/realesrgan/*.param` + `*.bin` | the `models/` folder of the Real-ESRGAN zip |
| `models/rife/<model>/` | the model folders of the RIFE zip (`rife-v4.26/` recommended) |
| everything else (runtimes, ONNX models, Real-CUGAN) | install from the **AI models** page; see `models.json` for the files and URLs |

The app starts without tools and the **Setup check** window lists exactly what is missing. A
profile whose model is missing is greyed out with a tooltip naming the file.

## AI models

The **AI models** page lists every model by task with its download size, speed and quality
(1–5), whether it fits your GPU (*Good / Tight / Too big for this GPU*), its licence and its
state (*Installed / Not installed / Update available / Incomplete*). **Install**, **Uninstall**
and **Repair** (re-download and verify) work per model; files shared with other models are kept.
Model lists in the settings show installed models first and the others greyed out with ⬇ and
their size; picking one asks to install it. When the current settings need a model that isn't
installed, the settings show **Install missing parts** (all of them in one go, with the total
size).

| Task | Models | Backend |
|---|---|---|
| Upscale (photo / video) | Real-ESRGAN x4plus, RealPLKSR Nomos2 4x, SPAN NomosUni 2x (fast), DAT2 Nomos2 4x (best detail), HAT-L BHI 4x (experimental, slow), ESRGAN Nomos2 4x | ncnn / ONNX |
| Upscale (anime) | Real-ESRGAN x4plus-anime, animevideov3, Real-CUGAN SE / Pro (2x/3x/4x, denoise levels) | ncnn |
| AI cleanup (1x) | DeJPG, DeNoise (RealPLKSR), SCUNet | ONNX |
| Faces | GFPGAN 1.4, CodeFormer (**non-commercial only**, accepted once before install); YuNet detector | ONNX |
| Make blurry photos sharp | Restormer motion / defocus deblurring; SeedVR2 3B generative (Creative, optional, 3.8 GB) | ONNX |
| Frame generation | RIFE v4.26 (default), v4.25-lite, v4.6, v4 | ncnn |

* **ONNX runtime:** ONNX models run inside the app with ONNX Runtime — **DirectML** (any DirectX
  12 GPU, ~25 MB download, the default) or **CUDA** (NVIDIA only, ~1 GB). One runtime is loaded
  per run of the app; switching takes effect after a restart. If the GPU can't run a model the
  app falls back to the CPU and says so; if video memory runs out, the tile size is halved
  (down to 64) and, for fixed-size models, the job stops with an explanation.
* **Custom models:** put `*.param` + `*.bin` (ESRGAN-family ncnn) or `*.onnx` (image-to-image,
  RGB 0..1) into `models/custom/` in the tools folder; **Rescan** on the AI models page. The scale
  comes from the file name (`4x_Name`, `Name_x2`) or, for ONNX, from a test run. 1x ONNX models
  can also be used for AI cleanup.
* **Licences:** every model shows its licence and credits; non-commercial models carry a badge.
  The full list is in [licenses/THIRD_PARTY.md](licenses/THIRD_PARTY.md). The converted model
  files are hosted on this repository's `models-YYYY.MM` release (scripts in
  [tools/convert/](tools/convert/README.md)); the catalog (`models.json`) can be updated without
  updating the app.

## Using it

1. Drag files/folders onto the window (or **Add files… / Add folder…**; folders are scanned recursively if *recursive* is on). Unsupported files are skipped with a message. Images: PNG, JPG, WebP, BMP, TIFF. Videos: MP4, MKV, MOV, WebM, AVI.
2. Pick settings on the right. Queue items follow the current settings unless you pin a snapshot to them with **Apply settings → all / → checked** (e.g. different profiles per file); ↺ un-pins an item. Each step — **Upscaling**, **Frame generation**, **Sharpen / denoise** — has a checkbox in its header; unticked steps are skipped (e.g. frame generation only: untick Upscaling). Video items show their length and frame count.
3. Click a queue item to preview it. Changing settings re-renders the preview automatically (debounced, cancellable). For videos, drag the scrubber to pick a frame, or press **Preview sample** to process a short clip (default 3 s, all settings including frame generation) and play Original vs Enhanced side by side in a loop.
4. **Start queue**. Cancel current / cancel all / pause (between chunks) at any time. After an export, **Compare with exported file** loads the real output into the preview.
5. The sample player plays the clip's **sound** (it is the master clock, so both sides stay in
   sync): play/pause, restart, loop, ¼ speed, mute and volume. Each side shows its FPS and
   resolution and the frame rate actually displayed; if the enhanced FPS is above the monitor's
   refresh rate a notice explains that not every frame can be shown (the file still has them).
6. The **Outputs** tab lists everything you made (history plus the files in the output folder):
   grid or list, sort, filter images/videos, search, details (resolution, length, FPS, settings,
   processing time), **Open**, **Show in Explorer**, **Compare with source** and **Delete** (to the
   Recycle Bin, after confirmation). Missing files are marked and can be removed from the list.

### Pages

* **All-in-one** — every option on one page (the 2.0 layout).
* **Video** / **Image** — only the options for that kind of file, in processing order, each
  section with an on/off switch; *Simple* hides the expert knobs, *Advanced* shows them. The
  summary line under the title shows what the selected file will become
  (`1920×1080 · 30 fps → 3840×2160 · 60 fps · NVENC HEVC · est. temp · est. time`). The Image page
  has **Restore at original size** (AI clean-up without enlarging) and a **local contrast**
  slider. Each page keeps its own files and settings; all jobs share one queue, which shows the
  page each job came from.
* **Editor** — trim (Fast: keyframe cut, no re-encode; Accurate: frame-exact re-encode), crop
  (drag the handles, aspect presets, numeric x/y/w/h), join clips (drag to reorder, trim each
  clip; lossless when all clips match, otherwise letterboxed to the first clip), crop / rotate /
  flip images. Ctrl+Z / Ctrl+Y undo and redo; I / O set In / Out; ← → step one frame. Results are
  saved next to the source as `<name>_edited.<ext>`, appear in Outputs as "✂ Edited", and
  **Send to Video/Image page** queues them for enhancement.
* **Settings** — Performance (Simple / Advanced), language, updates, tools, paths.

### Make sharp (AI)

A mode on the **Image** page for blurry photos (camera shake, out of focus, small or old
pictures). Select an image and the app says in plain words what kind of blur it sees, suggests a
preset and shows how much real detail is left.

* **Presets:** Auto, Light blur, Strong blur, Camera shake, Out of focus, Old / low-res photo,
  Portrait (faces).
* **Faithful ↔ Creative:** *Faithful* (left) only sharpens what is in the photo: it deblurs with
  Restormer, shrinks the image to the detail it really has and rebuilds it with RealPLKSR, so
  it's quick (10–20 s per photo on a mid-range GPU). *Creative* blends in **SeedVR2**, a generative
  model that redraws lost detail. It needs a GPU with 6 GB or more and takes about 1.5–2 minutes
  per photo. Auto picks 50 % for strong / focus blur, old photos and portraits, and 0 % for
  light blur and camera shake.
* **Fix faces** (GFPGAN, or CodeFormer), output size (original / 2x / 4x), estimated time and
  whether it fits your PC; **Advanced** has the deblur and detail models, downscale, tile, seed,
  face strength and final sharpen.
* **Preview region:** the preview processes only a box (drag it on the picture; by default it is
  on the largest face), so trying settings is quick. **Faithful vs Creative** shows both side by
  side; **Process full image** previews everything.
* **Install missing parts** downloads the models the mode needs; the Creative model is offered
  separately.

> **About Creative results.** AI redraws details. Results look sharp but may not match reality:
> text, faces and licence plates can look right and be wrong. Don't use them as evidence or for
> documents. Files made with Creative are marked "✦ AI-generated details" in History and Outputs,
> and carry the note "Processed with generative AI by Quick Enhancer" in their metadata (PNG
> text chunk / JPEG comment). Everything still runs on your PC.

### Saved settings profiles

The profile bar on each page loads a built-in or your own profile. **Save** overwrites your
loaded profile, **Save as… / Duplicate** create a new one, **☆ Default** loads it on that page
every time the app starts, **Revert** undoes changes (a `*` shows the profile was modified).
Profiles are stored one file per profile in `<data>\profiles\` and can be **exported /
imported** as `.toml`. Performance values (tile, chunk, RAM, threads, temp folder) belong to the
PC and are saved in a profile only when *Include performance settings* is ticked; an imported
profile with values too high for this PC offers **Adjust to this PC**.

### Resources, benchmark and auto-tune

The *Resources* section of each page (and Settings → Performance) estimates for the selected
file: temp disk per chunk and at peak (2 chunks; calibrated from the real PNG sizes written so
far), RAM, VRAM, CPU threads and processing time (after a benchmark), next to what this PC has.
Problems are shown inline as warnings (yellow: e.g. RAM budget over 60 % of RAM, temp over 80 %
of free space, tile above the VRAM-safe size, all cores used, temp folder on an HDD, output FPS
above the monitor's refresh rate) or blocking errors (red: not enough temp space or RAM, invalid
folder, encoder unavailable). **Fix automatically** changes only performance settings. The same
check runs when you press *Start queue*. While a video runs, it pauses if the temp drive gets
too full and continues by itself when space is freed.

**Benchmark this PC** (Settings, ~20–60 s, cancellable) measures Real-ESRGAN at tiles
128/192/256, RIFE, the CPU filters, PNG writing and the encoder on built-in test frames and
stores the result per GPU. **Auto-tune for this PC** then picks the tile, ncnn `-j`, threads,
RAM budget and a chunk size with enough GPU work per chunk that fits the temp drive, and lists
what it changed.

### Upscale profiles

Each AI profile has a **Model** list: *Automatic* uses the profile's default below; any
installed upscale model (including custom ones) can be chosen instead.

| Profile | Engine / model (Automatic) |
|---|---|
| Realistic / Photo | Real-ESRGAN `realesrgan-x4plus` (native 4x) |
| Anime / 2D | video: `realesr-animevideov3` x2/x3/x4 · images: `realesrgan-x4plus-anime` |
| Pixel Art / Retro | no AI: integer nearest-neighbour (AI models smear pixel art) |
| General | `realesr-general-x4v3` if present, else `realesr-animevideov3`, else Lanczos3 |
| No AI | Lanczos3 resize + filters only |

Scale: 1x (enhance only), 2x, 3x, 4x or a target resolution (720p/1080p/1440p/4K, short side, aspect kept, even dimensions for video). If the model's native scale doesn't match, the nearest native scale ≥ requested is run and the result is resized with Lanczos3 — the UI shows this (e.g. "4x model → downscaled to 3x"). At 1x an AI profile runs its smallest native scale and scales back down (AI restoration without enlargement).

### Frame generation

Off / 2x / 3x / 4x / target FPS, with RIFE v4.26 by default (v4.25-lite for slower GPUs, v4.6 and
v4 also included). Output FPS = source FPS × integer multiplier, capped by *Max FPS* (default 120); a target FPS picks the largest multiplier that fits (e.g. 24 → "60" gives 2x = 48 fps; shown before starting). 3x/4x need a RIFE v4.x model.

* **Order:** interpolation runs at source resolution *before* upscaling — RIFE's cost scales with pixel count, so interpolating small frames and upscaling all of them is much cheaper than interpolating 4x-sized frames.
* **Scene cuts:** each frame gets a 64×36 luma signature; mean absolute difference + histogram distance above the threshold marks a cut. Generated frames that would blend across a cut are replaced by copies of the frame before the cut (and are deleted before upscaling so no GPU time is wasted).
* **VFR** sources are converted to CFR at their average frame rate (warning shown).

### Filters

Order: **AI cleanup → denoise → (interpolate) → upscale → face restoration → resize → sharpen**.
Face restoration finds faces with YuNet, aligns each to a 512×512 crop, restores it and blends it
back with a soft mask; *Strength* blends GFPGAN or sets CodeFormer's fidelity. CAS (strength 0–1) or unsharp mask (radius / amount / threshold). Denoise is a 5×5 bilateral filter with a luma range kernel — edge preserving, single pass and fast enough for video (NL-means would be 10–50× slower). The preview runs exactly the same functions as the export.

### GPU / CPU selection, presets, tile size

* **Compute device:** *Auto* (discrete GPU with most VRAM → integrated → CPU), any Vulkan GPU by name + VRAM, or *CPU*. Device indices come from Vulkan enumeration, which is the same order as ncnn's `-g <index>`. FP16 support is shown for information (ncnn uses it automatically).
* **CPU mode:** RIFE runs on the CPU (`-g -1`, very slow). The official `realesrgan-ncnn-vulkan` build has no CPU mode, so AI upscaling falls back to Lanczos3 (a warning is shown). Filters always run on CPU threads.
* **Presets** (each value can be overridden):

  | Preset | Tile | ncnn `-j` | Filter threads | Priority |
  |---|---|---|---|---|
  | Eco | 128 | 1:1:1 | 25 % of cores | Below normal |
  | Balanced | auto, ≤ 256 | 1:2:2 | 50 % | Normal |
  | Max | auto (from VRAM) | 2:4:4 | all − 1 | Above normal |

  *Auto* tile derives from free VRAM (≥ 3 GB → 256, ≥ 1.5 GB → 200, ≥ 0.7 GB → 128, else 64). It is capped at 256 because bigger tiles of the heavy x4plus model can keep the GPU busy long enough for Windows to reset the driver (TDR, "device lost"). If ncnn fails or reports a Vulkan error, the chunk is retried with half the tile size (down to 32) and the smaller size is kept for the rest of the job; a lost device stops the tool immediately and waits for the driver to recover before retrying. Priorities are applied to the child processes with `SetPriorityClass`.
* **Encoder preset / rate control:** Fast / Balanced / Quality maps to each encoder's preset
  (NVENC p3/p5/p7, x264/x265 fast/medium/slow, …); *Bitrate* switches from constant quality to
  average bitrate with a peak cap.
* **Encoders:** detected at startup with `ffmpeg -encoders` **and** a 1-frame test encode each (an encoder can be listed but unusable, e.g. NVENC without an NVIDIA GPU). Unusable ones are greyed out with the reason. *Auto* prefers HEVC NVENC → AMF → QSV, then H.264 hardware, then libx265/libx264/SVT-AV1. Quality is CQ (NVENC), QP (AMF), global_quality (QSV) or CRF (software) with per-encoder defaults.

### How the video pipeline bounds RAM and disk

The ncnn tools work on image files, so true streaming isn't possible with this backend. Instead:

1. One ffmpeg process decodes the source to raw `rgb24` frames over a pipe (CFR at the average fps).
2. A decoder thread denoises, computes the scene-cut signature and writes PNGs into a **chunk** folder (default 120 frames; each chunk also contains the first frame of the next chunk so interpolation works across chunk boundaries).
3. The processor runs RIFE and/or Real-ESRGAN on the chunk, then loads, resizes and sharpens the frames and pushes them into **one long-running ffmpeg encoder** through a raw-video pipe. The chunk's files are deleted immediately.
4. A 2-token semaphore guarantees **at most 2 chunks on disk** (one being processed, one being prepared). The temp-space cap for the selected file is shown in the settings and free space is checked before starting.
5. The only in-memory frame queues are bounded channels sized from the **RAM budget** setting, so memory stays flat whatever the video length.
6. Without ncnn AI or frame generation, frames never touch the disk at all: ONNX models
   (upscale, cleanup, faces) run on the in-memory frames.

Audio is copied when the container allows it (otherwise AAC), text subtitles are kept (MKV: all subtitles), and metadata/chapters are copied. Audio stays in sync because the duration is unchanged when the FPS is multiplied. Temp folders (`<temp>/quick-enhancer-tmp/<pid>-…`) are removed on finish, cancel and error, and stale folders from crashed runs are removed on the next start. Partial outputs are deleted on cancel/error unless *Keep partial video* is enabled.

## Output

Default: next to the source as `<name><suffix>.<ext>` (suffix `_enhanced`, changeable per page); or a chosen folder. If the file exists: ask / skip / auto-rename / overwrite. Images: PNG, JPG (quality), WebP (lossless) or same as source; alpha channels are preserved (resized with Lanczos, AI runs on RGB). Videos: MP4 or MKV.

## Tests

```sh
cargo test                      # unit + integration tests
cargo test -- --ignored         # also the network / real-archive tests (see docs/BUILDING.md)
```

Unit tests cover ffprobe JSON parsing, scale / target-resolution math, FPS planning, the chunk planner, scene-cut detection and slot filling, CAS / unsharp / denoise on synthetic images, resizing, preset → settings mapping, tile retry, encoder detection parsing and mux arguments. Integration tests (`tests/pipeline_filters.rs`) generate test videos with `ffmpeg -f lavfi -i testsrc` and run the real pipeline (filters only; cancel + cleanup; image with alpha; and — if the ncnn tools, models and a Vulkan GPU are present — Real-ESRGAN + RIFE with a scene cut). They are skipped when `bin/ffmpeg` is missing. v2.1 adds tests for the resource estimator and validation rules (fixed numbers), auto-tune,
profile storage (create / rename / duplicate / delete / reload, unknown and missing fields),
encoder preset/bitrate arguments, the local-contrast filter and the Editor's ffmpeg arguments
(fast/accurate trim, even crop, concat compatibility and filter graph, time parse/format);
`tests/editor_real.rs` runs trims, crops and both kinds of joins with the real ffmpeg and
`tests/bench_real.rs` (`--ignored`) benchmarks the real GPU. v2 adds tests for i18n key parity (every key in both languages, every `t!` key used in the code exists), history merge/de-dup, version comparison, `latest.json` / `tools.json` parsing, SHA-256 verification and safe extraction, data-folder resolution (installed / portable / override / migration) and the audio-clock ↔ video-frame mapping of the sample player. v2.2 adds tests for the model
catalog (parsing and validation), install state and plans, single-file packages, custom model
scanning, tiling / reflection padding / feather blending, the ONNX execution-provider fallback
order, face alignment / detection decoding / paste-back, the catalog-based estimate and the model
choice for profiles with missing models. `tests/models_real.rs` (`--ignored`) downloads real
models and runs them: Real-CUGAN and ncnn custom models, ONNX models on DirectML vs the CPU,
cleanup and transformer models, face restoration against reference images and all RIFE models.
v2.3 adds tests for the blur analysis (sharp factor, motion direction, kinds), presets and the
stage plan, downscale / target-size math, the SeedVR2 window / tile / blend math (checked
against the reference pipeline), colour fix, the worker protocol (progress, errors, out of
memory, crashes, cancel; `tests/sharp_worker.rs` runs the real exe), preview regions, the
generative metadata note and the `sharp` catalog group. `tests/sharp_real.rs` (`--ignored`) runs
Make sharp end to end with the real models, Faithful and Creative.

## Known limitations

* Video is processed as **8-bit SDR**; 10-bit/HDR sources are converted (a warning is shown before starting).
* **CPU AI is very slow** (RIFE) or unavailable (Real-ESRGAN ncnn → Lanczos fallback).
* The **Pixel Art** profile is non-AI (integer nearest-neighbour; no xBRZ yet).
* **ONNX models on the CPU** work but are slow (a 4x RealPLKSR frame is ~10× slower than on a
  GPU); HAT-L is slow even on a GPU and marked experimental.
* **Face restoration** works frame by frame, so faces can flicker slightly in videos; very small
  faces (under ~24 px) are left alone.
* The ONNX runtime comes with its own copy of the Microsoft Visual C++ runtime (installed next to
  `onnxruntime.dll`), so the VC++ Redistributable is not needed. If an older system copy is
  already loaded by something else, Windows uses that one; if loading fails, the message suggests
  **Repair** or the official VC++ Redistributable.
* Switching between the DirectML and CUDA runtimes needs an app restart.
* **SeedVR2** is used for single images only (Make sharp → Creative). For video it is too heavy:
  17 frames at 512×288 needed over 10 GB of VRAM and about 3.4 s per frame.
* **Make sharp:** Creative works on at most ~2 MP at a time (1 MP on GPUs with up to 7 GB) and is
  scaled back up, so very large photos gain less. Auto cannot tell JPEG damage from strong blur,
  so it may choose Creative 50 % where Faithful would be better. On the CPU only Faithful is
  practical.
* Frame generation uses integer multipliers only (target FPS rounds down to one).
* WebP output is lossless only.
* GPU telemetry: NVML for NVIDIA, PDH counters for other vendors on Windows; on Linux non-NVIDIA GPUs show "not available".
* The interface is English and Thai only; other languages fall back to English.
* The installer and exe are **not code-signed** until you add a certificate, so Windows SmartScreen shows a warning on first run (see docs/RELEASING.md).
* Preview audio needs an output audio device; without one the sample plays silently.
* The Outputs tab scans the output folder only when one is set; with "next to source" it shows the history only.
* The Editor's **Fast** trim cuts at the keyframe before the In point, so it may start a little
  early (use *Accurate* for frame-exact cuts). Selection playback plays at most the first 20 s.
* Editor exports are written next to the source (not to the page's output folder).
* Removing a clip from the join list can't be undone.
* Time estimates need a benchmark (scaled per model from the catalog's speed figures); the VRAM
  estimate is a heuristic per model and tile size.
* **Licensing:** FFmpeg "gpl" builds include x264/x265 and are GPL-licensed; Real-ESRGAN and RIFE ncnn binaries/models carry their own licenses (BSD-3 / MIT). This app runs them as separate programs and does not bundle them.
