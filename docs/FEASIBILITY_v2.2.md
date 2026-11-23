# Quick Enhancer v2.2 — Feasibility report (more AI models, ONNX backend, Model Manager)

Measured on 2026-11-23 on the target PC:
- **GPU:** RTX 5060 Ti 16 GB, driver 621.24
- **RAM:** 15.8 GB
- **OS:** Windows 11 Pro

All experiments live in `./.feasibility/` (git-ignored, about 16 GB) and no app source code was changed.

- **Test inputs:** synthetic, plus public-domain / permissive scikit-image samples (the NASA "astronaut" photo, "coffee", "chelsea"). They are 1080p, 540p and 270p mosaics, a JPEG-degraded face, a flat-colour "anime" drawing, and a 3 s 720p pan clip (72 frames).
- **Tools:** ONNX Runtime 1.24.4 (DirectML package) and 1.32.0 (CUDA 13 package). Both ran inside a local venv (Python 3.12, uv), with nothing installed system-wide.

How to read the numbers:
- **Speed:** "s/MP" = seconds per megapixel of **input**, with 256 px tiles and 16 px overlap, including tiling and blending in numpy (`conv/bench_onnx.py`).
- **ncnn timings:** wall time of the CLI, including its ~0.5 s start-up.
- **VRAM:** the increase over idle (NVML, sampled every 20 ms).

---

## 1. Summary table

| # | Model | Works? | License (commercial OK?) | Backend | Speed / VRAM on this PC | Effort | Recommendation |
|---|---|---|---|---|---|---|---|
| 1 | **Real-CUGAN** (se/pro, 2x/3x/4x, denoise levels) | ✅ | MIT (both the model and the ncnn port) ✅ | ncnn CLI | 1080p frame: 2x 1.45 s / 1.1 GB, 3x 2.4 s / 2.1 GB, 4x 2.9 s / 1.3 GB | S | **Include** |
| 2a | **ESRGAN community, permissive** (4xNomos2_otf_esrgan, Phhofm, CC-BY-4.0) | ✅ | CC-BY-4.0 ✅ (attribution) | ncnn (converted, runs in the existing realesrgan-ncnn-vulkan) **and** ONNX | ncnn: 540p→4x 6.6 s / 0.6 GB. ONNX DML fp16: 4.0 s/MP / 1.3 GB | S | **Include** |
| 2b | **4x-UltraSharp, 4x-Remacri** | ✅ (Remacri converted and run) | **CC-BY-NC-SA-4.0** ❌ non-commercial. UltraSharp v1 is only on Mega, so its URL can't be pinned | ncnn / ONNX | same as 2a (same ESRGAN arch) | S | **Not in the catalog**. Users can drop them into the custom-models folder (documented) |
| 3 | **Custom models folder** (ncnn `.param/.bin`, `.onnx`) | ✅ | user's responsibility | ncnn + ONNX | — | M | **Include**. Scale is read from the model where possible (ONNX: run a 64 px probe; ncnn: from the file name `4x…`, else ask once) |
| 4a | **RealPLKSR** (4xNomos2_realplksr_dysample, CC-BY-4.0) | ✅ (**static** 256 export only, because dynamic shapes give garbage with DySample) | CC-BY-4.0 ✅ | ONNX | DML fp32: **4.0 s/MP**, 0.64 GB | S | **Include**: best quality/speed for photos |
| 4b | **SPAN** (2xNomosUni_span_multijpg_ldl, CC-BY-4.0) | ✅ | CC-BY-4.0 ✅ | ONNX | DML: **0.35 s/MP**, 0.24 GB (1080p frame ≈ 0.7 s) | S | **Include**: fast video 2x |
| 4c | **DAT2** (4xNomos2_hq_dat2, CC-BY-4.0) | ✅ static 256 (dynamic export fails on DML) | CC-BY-4.0 ✅ | ONNX | DML fp16: **19.9 s/MP**, 2.8 GB. fp32: 30.8 s/MP, 5.4 GB | M | **Include** as "slow, highest quality (images)" |
| 4d | **HAT-L** (4xBHI_small_hat-l, CC-BY-4.0) | ✅ fp32 static 128. fp16 gives NaN (black output) | CC-BY-4.0 ✅ | ONNX | DML fp32: **61.8 s/MP**, 2.1 GB (1080p photo ≈ 2 min) | M | **Include as experimental / very slow**, or drop (DAT2 is similar quality and 2–3× faster) |
| 4e | **ATD** (4xNomos2_hq_atd) | ❌ export fails (`aten.sort` in the token grouping; both exporters) | CC-BY-4.0 ✅ | ONNX | — | L (patch sort→TopK in the arch) | **Postpone** (DAT2 covers the use case) |
| 5a | **1x DeJPG / DeNoise** (RealPLKSR, Phhofm, CC-BY-4.0) | ✅ (dynamic shapes OK: no DySample) | CC-BY-4.0 ✅ | ONNX | DML fp16: **1.4 s/MP**, 0.2 GB | S | **Include** ("Restore at original size") |
| 5b | **SCUNet** (real PSNR, cszn) | ✅ static 256 (dynamic export **crashes DML**) | Apache-2.0 ✅ | ONNX | DML fp16: 2.7 s/MP, 1.2 GB. CUDA fp16: 0.9 s/MP | S | **Include** (strong denoiser) |
| 5c | **NAFNet** | not tested: weights only on Google Drive, not pinnable | MIT ✅ | ONNX | — | M | **Postpone** (5a + 5b cover denoise / JPEG) |
| 6a | **GFPGAN 1.4** + **YuNet** face detector | ✅ with a DML-compatible export (see §3.4). Our first own export runs on CUDA/CPU only | Apache-2.0 ✅ (YuNet MIT ✅) | ONNX | DML: **32 ms/face** (512²), 1.5 GB. Detector: < 5 ms | M | **Include** (default face model) |
| 6b | **CodeFormer** (+ fidelity weight slider) | ✅ own export (needs a 2-line arch patch at export time) | **S-Lab License 1.0, non-commercial** ❌ | ONNX | DML: **73 ms/face**, 1.75 GB | M | **Include as optional, clearly labelled "non-commercial use only"**, with an accept step |
| 7 | **GIMM-VFI** | ❌ not feasible: CuPy CUDA kernels (softsplat) + a custom CUDA correlation op | **S-Lab, non-commercial** ❌ | — | — | XL | **Drop**. Use **RIFE 4.25/4.26 (MIT)** instead (below) |
| 7' | **RIFE v4.26 / v4.26-large / v4.25-lite** (replacement for 7) | ✅ with the TNTwise rife-ncnn-vulkan fork (same CLI; nihui's 2022 exe crashes on these models) | MIT ✅ | ncnn CLI | 72 frames 720p → 2x: 6.0 s / 0.63 GB (v4.6: 5.0 s / 0.37 GB) | S | **Include**. Also switch the RIFE exe to this fork |
| 8 | **SeedVR2-3B** | ⚠️ graphs **load** (DML 40 s / 3.4 GB; CUDA 1.9 s / 4.3 GB). **Full run not done** (see §3.6) | Apache-2.0 ✅ | ONNX (5 graphs + our own Rust port of the numpy driver) | upstream numbers: ~24 s per 17-frame 384×384 clip on CUDA; decoder peaks near 15 GB commit | XL | **Experimental, gated by a first real run in Phase 2.** Drop it if it doesn't fit 16 GB RAM / VRAM |

Also kept: Real-ESRGAN (ncnn) and RIFE (ncnn).

Side-by-side samples are in `.feasibility/samples/` (all crops are taken from the same inputs):

| File | What it compares |
|---|---|
| `cmp_sr_face.png`, `cmp_sr_face3.png` | Real-ESRGAN x4plus vs Nomos2-ESRGAN / RealPLKSR / DAT2 / HAT-L. RealPLKSR and DAT2 keep natural skin texture; x4plus looks waxy |
| `cmp_sr_face2.png` | correct vs broken exports (dynamic RealPLKSR = garbage) and the 1x models |
| `cmp_esrgan_coffee.png` | ncnn conversions of Nomos2-ESRGAN and Remacri vs Real-ESRGAN x4plus |
| `cmp_face.png` | GFPGAN 1.4 vs CodeFormer (w = 0.7) on a JPEG-degraded face |
| `cmp_video2x.png` | animevideov3-x2 vs Real-CUGAN se 2x vs SPAN 2x on a 540p frame |
| `cmp_anime.png` | animevideov3 vs Real-CUGAN 4x on flat-colour line art |

Real-CUGAN keeps line art crisper and less "painted" than animevideov3.

---

## 2. Execution providers and the `ort` crate

### 2.1 DirectML vs Windows ML vs CUDA / TensorRT

| | Status (Nov 2026) | Delivery | Size | Speed here |
|---|---|---|---|---|
| **DirectML** | Supported, but in **sustained engineering**: Microsoft moved new feature work to Windows ML. The pip/NuGet DML package lags (1.24.4, while ORT core is at 1.32) | `onnxruntime.dll` + `DirectML.dll`, loaded dynamically | 40 MB unpacked, ≈ 17 MB zipped | Baseline. Conv nets on par with ncnn, e.g. ESRGAN 540p→4x: DML fp16 2.1 s vs ncnn 3.5 s |
| **Windows ML** | GA and Microsoft's recommended path. Vendor EPs are downloaded by Windows. Needs the Windows App SDK runtime / `Microsoft.Windows.AI.MachineLearning`; best on Win 11 24H2+ | NuGet / WinAppSDK, WinRT-flavoured bootstrap | small app-side; EPs fetched by the OS | not measured (no Rust binding; can't use it without system components) |
| **CUDA EP** (ORT 1.32, CUDA 13 + cuDNN 9) | Fully maintained. Works on Blackwell | ORT CUDA provider 187 MB + CUDA/cuDNN DLLs ≈ 1.7 GB | **≈ 1.9 GB** | Mixed. Faster for SCUNet (3×) and transformers in fp32, similar or slower for conv SR. Much slower than DML for GFPGAN (900 ms vs 32 ms) |
| **TensorRT EP** | Maintained | needs TensorRT DLLs (+1–2 GB) and per-shape engine builds | > 3 GB total | **not tested** (size; engine builds per tile shape) |
| **CPU** | always | inside `onnxruntime.dll` | 0 | 20–50× slower. Fine for the face detector or tiny inputs only |

**Recommendation:**
- **Default to DirectML, downloaded on demand** ("ONNX runtime (GPU, all vendors)", ≈ 17 MB). It runs on NVIDIA, AMD and Intel under Windows 10/11 and is still supported. Keep the EP layer pluggable so **Windows ML** can be added later, once there is a usable Rust path and the user base is on 24H2+.
- **Fallback order:** CUDA (only if installed) → DirectML → CPU, with a notice.
- **CUDA add-on:** offer it only as an optional, clearly-labelled 1.9 GB download, mainly for SeedVR2, whose reference driver targets CUDA. Don't build TensorRT in v2.2.

### 2.2 `ort` crate and the toolchain (important)

- **Version:** `ort 2.0.0-rc.15` (wraps ORT 1.31 API). The DirectML runtime is 1.24, so use `api-24` with `load-dynamic`. ORT's C API is backward compatible.
- **Loading:** `load-dynamic` + `ort::init_from(path)` loads the DLL we downloaded; nothing is bundled.
- **Binary size:** the exe grows by about 1–2 MB.
- **Blocker on this PC's toolchain (`stable-x86_64-pc-windows-gnu`):** `ort-sys` and `libloading 0.9` use `raw-dylib`. On the GNU target, rustc then needs `dlltool.exe` **and** `as.exe` (binutils).
  - Rust's self-contained mingw ships `dlltool` but not `as`, so the build fails with "dlltool could not create import library".
  - Older ort (rc.10) fails the same way.
  - Fetching a portable binutils (WinLibs) into `.feasibility/` was blocked by this session's permission policy, so the GNU workaround is **unverified**.
- **Options:**
  1. **Switch to the MSVC toolchain** (`stable-x86_64-pc-windows-msvc`). This is ort's officially supported Windows target and also gives a real `rc.exe` path. It **needs Visual Studio 2022 Build Tools (C++ workload, about 2–4 GB) installed system-wide, by you**. The `build.rs` resource writer from v2.1 already works with link.exe. **Recommended.**
  2. **Stay on GNU** and add mingw-w64 binutils (`as`, `dlltool`) to PATH for builds, e.g. a WinLibs zip, documented in BUILDING.md. It's lighter, but unverified and less common for ort users.

---

## 3. Notes per model (sources, pins, format path, problems)

All URLs are immutable release assets or HF revisions. SHA-256 values will go into `models.json` in Phase 2.

### 3.1 Real-CUGAN
- **Source:** `https://github.com/nihui/realcugan-ncnn-vulkan/releases/download/20220728/realcugan-ncnn-vulkan-20220728-windows.zip`
  - 45.98 MB, sha256 `c6e08d46…1e400e01`
- **Contents:** models-se (2/3/4x; no-denoise, conservative, denoise1x/2x/3x where available), models-pro (2/3x), models-nose.
- **CLI:** like realesrgan (`-n` noise -1..3, `-s` scale, `-t` tile, `-g` gpu). Note that 4x supports noise −1/0/3 only.
- **Risks:** none significant. The 3x model uses more VRAM (2.1 GB at 1080p).

### 3.2 ESRGAN-family community models
- **Conversion:** spandrel → pnnx (fp16) → ncnn. Rename the blobs `in0`/`out0` → `data`/`output`. The result runs unchanged in `realesrgan-ncnn-vulkan -n <name> -s 4` (`conv/to_ncnn.py`).
- **ONNX:** the same models export to ONNX with dynamic H/W.
- **Phhofm models** (CC-BY-4.0) are on `github.com/Phhofm/models/releases/download/<tag>/<file>`, which is pinnable. Several already ship ONNX files.

### 3.3 Transformer SR and 1x restoration: export rules learned
- **DySample (RealPLKSR "dysample"):** must be exported **static** (fixed tile). Dynamic exports produce garbage on every EP.
- **DAT / HAT / SCUNet:** export **static** tiles.
  - DAT dynamic: "parameter is incorrect" on DML.
  - SCUNet dynamic: hard crash in DML.
  - The app then always feeds fixed-size tiles (pad the edges by reflection) — fine with our tiler.
- **fp16:**
  - Conv nets: trace `model.half()` (fast export).
  - Transformers: `onnxslim` then `float16.convert_float_to_float16(keep_io_types=True)` after clearing `value_info`. This works for DAT and SCUNet.
  - HAT-L overflows in fp16 (black output), so keep it fp32.
  - The slim + fp16 DAT file is 641 MB because constant folding expands the weights; the fp32 file is 51 MB. Phase 2 will try a non-folding fp16 pass, otherwise ship fp32 for DAT.
- **ATD:** blocked by `aten.sort` (stable) in both exporters. It would need a model-code patch (sort → TopK).

### 3.4 Faces
- **Pipeline (verified in Python, `conv/face_test.py`):**
  1. YuNet detection with 5 landmarks
  2. Similarity transform to the FFHQ 512 template
  3. Restore
  4. Inverse warp with a feathered mask and paste back
- **Ports:** YuNet runs on ORT (CPU EP is enough). The alignment and paste are about 150 lines of Rust; no OpenCV is needed (Rust: `imageproc` affine warp or our own bilinear).
- **GFPGAN:**
  - Our first export (spandrel `GFPGANv1Clean`, opset 17) uses modulated convs with runtime weights, which DML refuses ("Provider type for Conv … is not set"). It runs on CUDA (900 ms).
  - The FaceFusion-assets conversion of the same Apache-2.0 weights (`facefusion-assets/releases/download/models-3.0.0/gfpgan_1.4.onnx`, sha256 `accc4757…385`) runs on **DML at 32 ms/face**. So a DML-friendly export is possible; it is a matter of how the modulated conv is expressed (batch-folded grouped conv).
  - Phase 2: reproduce that export ourselves (M) and host our own file. Fallback: download the FaceFusion file directly (pinned URL), with attribution.
- **CodeFormer:** exported ourselves after making the `str(x.shape[-1])` dict keys static (`str(int(...))`, 2 lines in spandrel-extra-arches; dev-time only). Inputs: `input` (0..1) and `weight` (fidelity 0..1).

### 3.5 Frame interpolation
- **GIMM-VFI** (`GSeanCDAT/GIMM-VFI`, weights `GSean/GIMM-VFI` on HF, 65 MB `.pt`):
  - Uses CuPy runtime-compiled CUDA kernels (softmax splatting) and a CUDA correlation extension, so there is no ONNX path without rewriting those ops.
  - The license is **S-Lab non-commercial**.
  - Dropped.
- **RIFE 4.26** (Practical-RIFE, MIT) as ncnn:
  - Source: `https://github.com/TNTwise/rife-ncnn-vulkan/releases/download/20250112/windows.zip` (827 MB because it bundles ~50 model folders; sha256 `42ed35e1…aba3`). Each model folder is about 11 MB.
  - The CLI is identical to nihui's, so the existing `rife_ncnn` engine works with only a path change.
  - Proposal: **repack** exe + v4.6 + v4.26 + v4.25-lite (≈ 40 MB) as our own release asset (MIT allows it). This shrinks the starter-pack RIFE download from **412 MB to ≈ 40 MB**.

### 3.6 SeedVR2-3B
- **Repository:** the repo named in the task (`Merserk/SeedVR2-3B-ONNX`) now returns **401 / not found**. A current equivalent is `moonbox-dev/SeedVR2-3B-ONNX` @ `e2a111d55922e9059a0d663bc189da766fc6b775`, Apache-2.0.
- **Graphs:** 5 graphs (VAE encoder first/next, DiT, VAE decoder first/next):
  - fp16 folder 7.7 GB; DiT external data 6.69 GB.
  - int8 folder 4.56 GB; DiT 3.56 GB.
  - Frames must be multiples of 16; clips are padded to 4n+1 frames.
- **Tested:** all graphs load. DiT int8 on DML: 40 s load, +3.4 GB VRAM; on CUDA: 1.9 s, +4.3 GB.
- **Not tested — the full restore.** The only ready-made driver is the repo's `pipeline.py` (a numpy port; I read it, it only uses numpy + onnxruntime). Running a downloaded third-party script was **blocked by this session's permission policy**, and writing my own replacement driver just to get around that block would not be appropriate.
- **Upstream notes (same author, RTX 3080 Laptop 16 GB):**
  - ~24 s per 17-frame clip at 384×384 / 512×288 on CUDA.
  - The decoder peaks near 15 GB commit, and runs sometimes had to be repeated to fit.
  - With **15.8 GB RAM** here, 720p+ output is unrealistic; small clips (≤ 512 px side, 5–17 frames) are the realistic envelope.
- **Phase 2 plan:**
  1. First task: port the driver to Rust (windows, RoPE tables, causal VAE slices; ≈ 250 lines) and run 5 frames at 384² on DML and CUDA.
  2. If peak memory or time is unacceptable, drop it and tell you.
- **Effort:** XL. **Risk:** high.

---

## 4. Proposed architecture (short)

```
src/models/                 NEW — catalog + install state, no UI
  catalog.rs                models.json schema, embedded default + optional remote update (release asset)
  install.rs                install/uninstall/verify via the existing net::http downloader (pinned URL, SHA-256,
                            atomic move, local-file install, cancel); per-model folders under models/<id>/
  fit.rs                    "fits your PC" from detected VRAM/RAM (+ v2.1 resources estimator costs)
src/engines/
  realcugan_ncnn.rs         NEW — subprocess engine like realesrgan_ncnn
  realesrgan_ncnn.rs        + arbitrary ncnn model dirs (custom / community models)
  rife_ncnn.rs              exe path → TNTwise build; model list from catalog
  onnx/                     NEW — replaces ort_stub.rs
    runtime.rs              locate + load onnxruntime.dll (load-dynamic), EP chain CUDA→DML→CPU, session cache
    tiler.rs                static/dynamic tiles, reflect padding, overlap + feather blend, VRAM-aware tile size,
                            automatic retry with a smaller tile on OOM
    sr.rs                   OnnxUpscaler (Upscaler trait) — in-memory frames
    restore.rs              1x models (same tiler, scale 1)
    face.rs                 YuNet → align → GFPGAN/CodeFormer → paste back (strength / fidelity)
    seedvr2.rs              experimental driver (only if the Phase-2 gate passes)
src/ui/models.rs            NEW — Models page; src/ui/setup_wizard.rs — first-run wizard
```

- **Trait change:** `Upscaler` gets an in-memory path `fn upscale_frame(&self, img: &RgbImage, ctx) -> RgbImage`, used by ONNX engines (no PNG round-trip). ncnn engines keep `upscale_dir`. `pipeline::stages` picks the path by engine capability, so preview and export stay one code path.
- **New stage:** `FaceRestore` runs after SR, optional, with strength.
- **Profiles:** reference models by catalog id. A missing model gives an "Install missing model" action.
- **Manifest (`models.json`, schema 1):**

```json
{
  "schema": 1, "catalog_version": "2026.11.1",
  "runtimes": [{ "id": "ort-dml", "version": "1.24.4", "files": [/* url, sha256, size, to */] },
               { "id": "ort-cuda", "version": "1.32.0", "size_hint": 1900000000, "optional": true }],
  "models": [{
    "id": "realplksr-nomos2-4x", "group": "upscale_image",
    "name": { "en": "RealPLKSR Photo 4x", "th": "RealPLKSR ภาพถ่าย 4x" },
    "desc": { "en": "Best for: photos, natural skin. Not for: anime.", "th": "…" },
    "backend": "onnx", "task": "sr", "scale": [4], "tile": { "static": 256 }, "precision": "fp32",
    "speed": "normal", "quality": 4, "vram_min_mb": 1024, "vram_rec_mb": 2048,
    "cost": { "s_per_mp_ref": 4.0, "vram_mb_per_tile": 640 },
    "license": { "name": "CC-BY-4.0", "url": "…", "commercial": true, "redistribute": true, "attribution": "Philip Hofmann" },
    "source": "https://github.com/Phhofm/models/releases/tag/4xNomos2_realplksr_dysample",
    "files": [{ "url": "…", "sha256": "…", "size": 29914682, "to": "models/onnx/realplksr-nomos2-4x.onnx" }],
    "samples": { "before": "…png", "after": "…png" }, "version": "1", "experimental": false,
    "requires": ["ort-dml"]
  }]
}
```

---

## 5. UI mockups

**Navigation:** add **"Models"** ("โมเดล AI") as a top-level page between Editor and Outputs. "Manage tools" stays for FFmpeg / runtimes, and the setup wizard links both.

```
┌ Models ───────────────────────────────────────────────────────────────────────────┐
│ [★ Recommended for your PC]  Show: (•) All ( ) Installed ( ) Fits my PC   Disk used: 1.2 GB  [Open folder] │
│ ── Upscale video ───────────────────────────────────────────────────────────────── │
│ ┌──────────────────────────┐ ┌──────────────────────────┐ ┌──────────────────────────┐ │
│ │[before|after thumb]      │ │[before|after]            │ │[before|after]            │ │
│ │Real-CUGAN (anime) 2-4x   │ │SPAN 2x (fast)            │ │Real-ESRGAN animevideo v3 │ │
│ │Crisp lines, clean colours│ │Light, good for 1080p→4K  │ │Fast anime video          │ │
│ │⚡fast ★★★★  🟢 fits  46 MB│ │⚡fast ★★★  🟢 fits  2 MB  │ │⚡fast ★★★ 🟢 fits ✔ inst. │ │
│ │MIT          [Install]    │ │CC-BY-4.0     [Install]   │ │BSD-3   [Uninstall]       │ │
│ └──────────────────────────┘ └──────────────────────────┘ └──────────────────────────┘ │
│ ── Upscale image ── RealPLKSR · DAT2 (slow) · HAT-L (very slow, experimental) · ESRGAN Nomos2 │
│ ── Restore at same size ── JPEG cleanup · Denoise · SCUNet                         │
│ ── Faces ── GFPGAN 1.4 · CodeFormer [NON-COMMERCIAL]                               │
│ ── Smoother motion ── RIFE 4.6 (installed) · RIFE 4.26 · RIFE 4.25 lite            │
│ ── Ultra quality (experimental) ── SeedVR2-3B (4.6 GB, 🔴 needs 16 GB VRAM+RAM)     │
│ ── My custom models ── 2 found in models/custom/  [Add…] [Rescan]                  │
└───────────────────────────────────────────────────────────────────────────────────┘
  Downloading DAT2 …  ▓▓▓▓▓▓░░░░ 61 %  12.4 MB/s   [Cancel]
```

**First-run wizard** (extends `--install-tools`; Back/Next):

```
1 Language      (•) English  ( ) ไทย
2 Required      [x] FFmpeg + FFprobe ............ 114 MB   (locked)
3 Starter pack  [x] Real-ESRGAN (photo + anime) .. 43 MB
                [x] RIFE (frame generation) ...... 40 MB
4 Optional      [ ] GPU AI runtime (ONNX, DirectML) – needed for the models below .. 17 MB
                [ ] Better photo upscaling (RealPLKSR, SPAN) .............. 32 MB
                [ ] Anime upscaling (Real-CUGAN) .......................... 46 MB
                [ ] Restore at same size (JPEG cleanup, denoise, SCUNet) .. 125 MB
                [ ] Faces (GFPGAN) ....................................... 340 MB
                [ ] NVIDIA CUDA add-on (only for experimental models) .... 1.9 GB
5 Summary       Download 197 MB · Free on C: 412 GB · "You can add more later on the Models page."
                [Install]
```

**On-demand install:**
- Model dropdowns list installed models first. Models that aren't installed are greyed out, with ⬇ and their size.
- Picking one shows "Install RealPLKSR? 30 MB · normal speed · CC-BY-4.0 [Install] [Cancel]".
- The last entry is "Get more models…", which opens the Models page.

---

## 6. Hosting plan and sizes

- **Limit:** GitHub release assets must be under **2 GiB each**; there is no total cap.
- **Proposal:**
  - **Our converted files** (ncnn/ONNX conversions of MIT / Apache / CC-BY models; the RIFE repack; CodeFormer under its NC licence terms, with the notice) go to a dedicated GitHub release tag `models-2026.11` in `NobpasinTumdee/quick-enhancer`. Its `models.json` can be refreshed by the app without an app update.
  - **Large or unmodified files** are downloaded from the **original source**, pinned:
    - Real-CUGAN zip (nihui release)
    - SeedVR2 (moonbox-dev HF revision)
    - FFmpeg / Real-ESRGAN, as today
  - **Nothing NC-only from Mega**, and nothing whose licence forbids redistribution.
- **Sizes:**

| | Download |
|---|---|
| Installer | 8.9 MB today → ≈ 9 MB (ORT runtime is downloaded, not bundled) |
| Required (FFmpeg) | 114 MB |
| Starter (Real-ESRGAN 43 MB + slim RIFE 40 MB) | **≈ 83 MB** (today 455 MB with the 412 MB RIFE zip) |
| Required + starter | **≈ 200 MB** |
| Everything except SeedVR2/CUDA | ≈ 1.4 GB (ORT-DML 17, CUGAN 46, ESRGAN-community 33, RealPLKSR 30, SPAN 2, DAT2 51–640, HAT-L 319, 1x ×2 30, SCUNet 60, GFPGAN 340, CodeFormer 377, YuNet 0.2, RIFE extra 22) |
| + SeedVR2 int8 | + 4.6 GB |
| + CUDA add-on | + 1.9 GB |
| **Everything** | **≈ 8 GB** |

---

## 7. Risks

1. **Toolchain:** ort can't build on the current GNU setup without extra binutils. The MSVC switch requires your install (§2.2).
2. **DirectML is in sustained engineering:** it is stable and still supported, but frozen at ORT 1.24. New ops or quantization formats may not run; mitigation is CUDA, and later Windows ML.
3. **Exports are model-specific:** static shapes for DySample, DAT, HAT and SCUNet; fp16 NaNs for HAT. Each catalog model needs a verified export and a sample image. That is captured in `tools/convert/` plus a parity test (ONNX vs PyTorch PSNR).
4. **Transformer models are slow on this GPU** (DAT2 ≈ 20 s/MP, HAT-L ≈ 60 s/MP). They are only practical for images or a few frames; the UI must say so and the estimator must know.
5. **GFPGAN on DML depends on reproducing a DML-friendly export.** The fallback is a third-party file (FaceFusion assets).
6. **SeedVR2** may not fit 15.8 GB RAM at useful sizes, and its full run is untested (§3.6).
7. **Windows commit limit:** big ONNX sessions count GPU memory toward the commit limit. Sessions must be released between phases, and possibly run in a child process.
8. **No GitHub CLI** (`gh`) is installed here, so opening the PR in Phase 2 needs `gh` (and `gh auth login`) or a manual click.

---

## 8. Decisions needed (with my recommended default)

1. **Push target:** new branch `feature/v2.2-models` + Pull Request, not direct to `main`. *Default: yes.* `gh` is not installed; either install it and run `gh auth login`, or I push the branch and you open the PR from the link.
2. **Toolchain:** switch to **MSVC** (you install VS 2022 Build Tools with the C++ workload). *Default: MSVC.* Alternative: GNU + portable binutils in PATH (unverified).
3. **Default EP:** DirectML downloaded on demand. **CUDA add-on** optional, 1.9 GB, labelled "for experimental models". *Default: yes.* TensorRT and Windows ML are postponed.
4. **Non-commercial models:**
   - CodeFormer: *Default: include as optional with a red "Non-commercial use only" badge and an accept step.*
   - UltraSharp / Remacri: *Default: not in the catalog; usable via the custom folder.*
   - GIMM-VFI: *drop.*
5. **Model set:**
   - *Include:* Real-CUGAN, ESRGAN-Nomos2 (ncnn + ONNX), RealPLKSR, SPAN, DAT2, 1x DeJPG / DeNoise, SCUNet, GFPGAN, CodeFormer (NC), RIFE 4.26 / 4.25-lite (TNTwise build), custom folder.
   - *Experimental:* HAT-L, SeedVR2 (gated by the first real run).
   - *Postpone:* ATD, NAFNet.
   - *Drop:* GIMM-VFI.
6. **Hosting:** may I create the GitHub release `models-2026.11` with our converted model files in Phase 2? *Default: no.* I will build everything with `models.json` pointing at that tag, verify it with local-file installs, and leave the actual upload to you (steps in `docs/RELEASING.md`).
7. **RIFE exe:** switch to the TNTwise fork and repack a slim RIFE zip. *Default: yes.*
8. **SeedVR2 test:** allow running the upstream `pipeline.py` from the pinned HF revision once, as the reference check before porting? *Default: no.* I port it to Rust and test that port directly.
