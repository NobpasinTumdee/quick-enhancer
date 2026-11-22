# Quick Enhancer

A lightweight native Windows app (Rust + egui/wgpu) that makes **images and videos** look better,
entirely on your PC:

* AI **upscaling** with Real-ESRGAN (Photo / Anime / Pixel art / General / No AI profiles)
* AI **frame generation** with RIFE (2x/3x/4x or a target FPS) with scene-cut protection
* **Sharpen / denoise** (CAS, unsharp mask, edge-preserving bilateral denoise)
* Before / After **preview** with a wipe slider, side-by-side and a **sample clip player with
  sound** (per-side FPS and resolution)
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
3. At the end the installer opens the **tool downloader**: pick FFmpeg (required), Real-ESRGAN
   (upscaling) and RIFE (frame generation), tick *I have read the licenses* and press **Install
   selected**. Each archive is downloaded from its official release page and checked against a
   pinned SHA-256 before anything is extracted. You can skip this and do it later from
   Settings → Application → **Manage tools**, or install from archives you downloaded yourself
   (**Install from files… / From folder…**).

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
| `bin/rife-ncnn-vulkan.exe` | `rife-ncnn-vulkan-20221029-windows.zip` from <https://github.com/nihui/rife-ncnn-vulkan/releases> |
| `models/realesrgan/*.param` + `*.bin` | the `models/` folder of the Real-ESRGAN zip |
| `models/rife/<model>/` | the model folders of the RIFE zip (`rife-v4.6/` recommended) |

The app starts without tools and the **Setup check** window lists exactly what is missing. A
profile whose model is missing is greyed out with a tooltip naming the file.

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

### Profiles

| Profile | Engine / model |
|---|---|
| Realistic / Photo | Real-ESRGAN `realesrgan-x4plus` (native 4x) |
| Anime / 2D | video: `realesr-animevideov3` x2/x3/x4 · images: `realesrgan-x4plus-anime` |
| Pixel Art / Retro | no AI: integer nearest-neighbour (AI models smear pixel art) |
| General | `realesr-general-x4v3` if present, else `realesr-animevideov3`, else Lanczos3 |
| No AI | Lanczos3 resize + filters only |

Scale: 1x (enhance only), 2x, 3x, 4x or a target resolution (720p/1080p/1440p/4K, short side, aspect kept, even dimensions for video). If the model's native scale doesn't match, the nearest native scale ≥ requested is run and the result is resized with Lanczos3 — the UI shows this (e.g. "4x model → downscaled to 3x"). At 1x an AI profile runs its smallest native scale and scales back down (AI restoration without enlargement).

### Frame generation

Off / 2x / 3x / 4x / target FPS. Output FPS = source FPS × integer multiplier, capped by *Max FPS* (default 120); a target FPS picks the largest multiplier that fits (e.g. 24 → "60" gives 2x = 48 fps; shown before starting). 3x/4x need a RIFE v4.x model.

* **Order:** interpolation runs at source resolution *before* upscaling — RIFE's cost scales with pixel count, so interpolating small frames and upscaling all of them is much cheaper than interpolating 4x-sized frames.
* **Scene cuts:** each frame gets a 64×36 luma signature; mean absolute difference + histogram distance above the threshold marks a cut. Generated frames that would blend across a cut are replaced by copies of the frame before the cut (and are deleted before upscaling so no GPU time is wasted).
* **VFR** sources are converted to CFR at their average frame rate (warning shown).

### Filters

Order: **denoise → (interpolate) → upscale → resize → sharpen**. CAS (strength 0–1) or unsharp mask (radius / amount / threshold). Denoise is a 5×5 bilateral filter with a luma range kernel — edge preserving, single pass and fast enough for video (NL-means would be 10–50× slower). The preview runs exactly the same functions as the export.

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
* **Encoders:** detected at startup with `ffmpeg -encoders` **and** a 1-frame test encode each (an encoder can be listed but unusable, e.g. NVENC without an NVIDIA GPU). Unusable ones are greyed out with the reason. *Auto* prefers HEVC NVENC → AMF → QSV, then H.264 hardware, then libx265/libx264/SVT-AV1. Quality is CQ (NVENC), QP (AMF), global_quality (QSV) or CRF (software) with per-encoder defaults.

### How the video pipeline bounds RAM and disk

The ncnn tools work on image files, so true streaming isn't possible with this backend. Instead:

1. One ffmpeg process decodes the source to raw `rgb24` frames over a pipe (CFR at the average fps).
2. A decoder thread denoises, computes the scene-cut signature and writes PNGs into a **chunk** folder (default 120 frames; each chunk also contains the first frame of the next chunk so interpolation works across chunk boundaries).
3. The processor runs RIFE and/or Real-ESRGAN on the chunk, then loads, resizes and sharpens the frames and pushes them into **one long-running ffmpeg encoder** through a raw-video pipe. The chunk's files are deleted immediately.
4. A 2-token semaphore guarantees **at most 2 chunks on disk** (one being processed, one being prepared). The temp-space cap for the selected file is shown in the settings and free space is checked before starting.
5. The only in-memory frame queues are bounded channels sized from the **RAM budget** setting, so memory stays flat whatever the video length.
6. Without AI or frame generation, frames never touch the disk at all.

Audio is copied when the container allows it (otherwise AAC), text subtitles are kept (MKV: all subtitles), and metadata/chapters are copied. Audio stays in sync because the duration is unchanged when the FPS is multiplied. Temp folders (`<temp>/quick-enhancer-tmp/<pid>-…`) are removed on finish, cancel and error, and stale folders from crashed runs are removed on the next start. Partial outputs are deleted on cancel/error unless *Keep partial video* is enabled.

## Output

Default: next to the source as `<name>_enhanced.<ext>`; or a chosen folder. If the file exists: ask / skip / auto-rename / overwrite. Images: PNG, JPG (quality), WebP (lossless) or same as source; alpha channels are preserved (resized with Lanczos, AI runs on RGB). Videos: MP4 or MKV.

## Tests

```sh
cargo test                      # unit + integration tests
cargo test -- --ignored         # also the network / real-archive tests (see docs/BUILDING.md)
```

Unit tests cover ffprobe JSON parsing, scale / target-resolution math, FPS planning, the chunk planner, scene-cut detection and slot filling, CAS / unsharp / denoise on synthetic images, resizing, preset → settings mapping, tile retry, encoder detection parsing and mux arguments. Integration tests (`tests/pipeline_filters.rs`) generate test videos with `ffmpeg -f lavfi -i testsrc` and run the real pipeline (filters only; cancel + cleanup; image with alpha; and — if the ncnn tools, models and a Vulkan GPU are present — Real-ESRGAN + RIFE with a scene cut). They are skipped when `bin/ffmpeg` is missing. v2 adds tests for i18n key parity (every key in both languages, every `t!` key used in the code exists), history merge/de-dup, version comparison, `latest.json` / `tools.json` parsing, SHA-256 verification and safe extraction, data-folder resolution (installed / portable / override / migration) and the audio-clock ↔ video-frame mapping of the sample player.

## Known limitations

* Video is processed as **8-bit SDR**; 10-bit/HDR sources are converted (a warning is shown before starting).
* **CPU AI is very slow** (RIFE) or unavailable (Real-ESRGAN ncnn → Lanczos fallback).
* The **Pixel Art** profile is non-AI (integer nearest-neighbour; no xBRZ yet).
* The **ONNX Runtime (`ort`) backend** is only a stub behind the `Upscaler` trait.
* Frame generation uses integer multipliers only (target FPS rounds down to one).
* WebP output is lossless only.
* GPU telemetry: NVML for NVIDIA, PDH counters for other vendors on Windows; on Linux non-NVIDIA GPUs show "not available".
* The interface is English and Thai only; other languages fall back to English.
* The installer and exe are **not code-signed** until you add a certificate, so Windows SmartScreen shows a warning on first run (see docs/RELEASING.md).
* Preview audio needs an output audio device; without one the sample plays silently.
* The Outputs tab scans the output folder only when one is set; with "next to source" it shows the history only.
* **Licensing:** FFmpeg "gpl" builds include x264/x265 and are GPL-licensed; Real-ESRGAN and RIFE ncnn binaries/models carry their own licenses (BSD-3 / MIT). This app runs them as separate programs and does not bundle them.
