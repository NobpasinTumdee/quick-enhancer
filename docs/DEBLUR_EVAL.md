# "Make sharp (AI)" — model evaluation (v2.3)

How the models behind **Make sharp (AI)** were chosen. All tests ran on one PC: RTX 5060 Ti
16 GB, 16 GB RAM with a small page file (commit limit about 24 GB), Windows 11. The scripts,
images and outputs live in `.feasibility/deblur/` (not in git). The sample crops are listed at
the end.

## Test set

These are public-domain photos from Wikimedia Commons (US federal government works, listed in
`.feasibility/deblur/src/SOURCES.txt`), plus one synthetic text page. Each was downscaled to 1–2
MP; the result is the sharp ground truth (GT). Every case has one synthetic degradation:

| case | kind | degradation |
|---|---|---|
| landscape_motion15 | motion | linear motion blur, 15 px, 30° |
| sign_motion31 | motion | linear motion blur, 31 px, 120° (sign text) |
| bird_shake | motion | random camera-shake path, 25 px |
| bird_gauss_light | defocus | Gaussian σ 1.2 (light) |
| arches_defocus_med | defocus | disk r 4 (medium) |
| landscape_defocus_heavy | defocus | disk r 8 (heavy) |
| portrait_defocus | defocus | disk r 5 (blurry face) |
| sign_defocus | defocus | disk r 3 |
| text_gauss | defocus | Gaussian σ 2.2 on a text page (Latin, monospace, Thai) |
| landscape_lowres480 | lowres | 480 px tall, bicubic back to 1080p |
| portrait_lowres | lowres | 320 px wide, bicubic back up |
| arches_blur_jpeg_noise | mixed | σ 2 + noise 6 + JPEG q40 |

Metrics:

- **PSNR** and **LPIPS-alex** compare against GT. LPIPS is the main score, because it agrees
  best with how sharp and natural a result looks.
- **MUSIQ** has no reference. Higher looks sharper; a score above GT's means the result looks
  *more* detailed than the real photo, i.e. invented detail.
- Hallucination was judged by eye on the crops.

## Results (averages per blur kind)

LPIPS, lower is better:

| candidate | defocus | lowres | mixed | motion | all | MUSIQ all |
|---|---|---|---|---|---|---|
| input (blurry) | 0.409 | 0.329 | 0.468 | 0.455 | 0.412 | 34.7 |
| NAFNet-GoPro (deblur) | 0.569 | 0.484 | 0.470 | 0.385 | 0.501 | 43.8 |
| NAFNet-REDS (deblur) | 0.407 | 0.332 | 0.494 | 0.410 | 0.402 | 48.8 |
| Restormer motion | 0.367 | 0.298 | 0.452 | **0.293** | 0.344 | 48.2 |
| Restormer defocus | **0.294** | 0.287 | 0.450 | 0.454 | 0.346 | 50.8 |
| Restormer routed by blur type | 0.294 | 0.287 | 0.450 | 0.293 | 0.306 | 56.4 |
| smart downscale + RealPLKSR 4x | 0.305 | 0.215 | 0.236 | 0.430 | 0.316 | 54.5 |
| smart downscale + DAT2 4x | 0.334 | 0.239 | 0.407 | 0.438 | 0.350 | 45.8 |
| downscale 2x + RealPLKSR | 0.365 | **0.186** | 0.281 | 0.448 | 0.349 | 43.0 |
| **Faithful**: routed Restormer → smart downscale → RealPLKSR | 0.249 | 0.260 | **0.230** | 0.269 | 0.254 | 64.6 |
| routed Restormer → downscale 2x → RealPLKSR | 0.248 | 0.237 | 0.246 | 0.263 | 0.250 | 63.9 |
| SeedVR2-3B (int8) on the blurry input | 0.233 | 0.246 | 0.363 | 0.450 | 0.300 | 60.2 |
| routed Restormer → SeedVR2 | 0.269 | 0.321 | 0.390 | 0.314 | 0.299 | 66.0 |
| OSEDiff on the blurry input | 0.242 | 0.230 | 0.258 | 0.397 | 0.280 | 67.5 |
| routed Restormer → OSEDiff | 0.256 | 0.256 | 0.266 | 0.280 | 0.263 | **70.6** |
| **Creative 50 %**: Faithful ½ + SeedVR2 ½ | **0.225** | 0.234 | 0.280 | 0.284 | **0.246** | 65.1 |
| Creative 100 %: SeedVR2 (from the deblurred image for motion) | 0.233 | 0.246 | 0.363 | 0.314 | 0.266 | 66.2 |
| sharp original (GT) | 0 | 0 | 0 | 0 | 0 | 64.7 |

PSNR (all cases): input 25.5 dB, Faithful 24.0 dB, Creative 50 % 24.7 dB, SeedVR2 22.4–22.6 dB,
OSEDiff 22.0–22.3 dB. As usual, PSNR rewards blur, so it ranks the sharp outputs below the
input.

### What the crops show

- **Motion blur**: Restormer-motion recovers the sign ("MOUNT MORAN TURNOUT") almost exactly.
  - Every other model needs the deblurred image first. On raw motion blur, SeedVR2 and OSEDiff
    turn the streaks into invented letters; OSEDiff gave "UNFMONAN TURNOUP".
  - Smart downscale + SR alone cannot fix motion either.
- **Defocus, Gaussian blur**: Restormer-defocus helps with disk blur but barely touches
  Gaussian blur. It leaves a slight orange overshoot on eyelids.
  - The faithful SR step then makes text strokes thick and outlined (`text_gauss`).
  - SeedVR2 restores the text page almost perfectly, Thai included (LPIPS 0.124 vs 0.227), but
    it **invents the smallest line**: "1/8 in = 3.175 mm; tolerance" became "18 in = 3.179 mm,
    srarsece".
- **Faces**:
  - The faithful path looks waxy with dark eye outlines when the downscale is large. Capping
    the factor at 2.5x for portraits, plus GFPGAN, fixes most of it.
  - SeedVR2 gives a realistic face and mostly keeps the identity, but adds skin texture and
    looks a little older.
  - OSEDiff gives a sharper but *different-looking* face.
- **Small / low-res photos**: no deblur model; a plain 2x round trip + RealPLKSR is best
  (0.186). The app's Old-photo preset does exactly this.
- **Blending**: a 50/50 blend of Faithful and SeedVR2 scored best overall. Blending averages two
  versions of the same structure: it keeps SeedVR2's realism with fewer invented details.

### Speed and memory (RTX 5060 Ti 16 GB)

| stage | runtime | 1 MP | 2 MP | peak VRAM | peak commit |
|---|---|---|---|---|---|
| Restormer fp16 ONNX, 512 px tiles | DirectML | 3.5 s | 7 s | < 1 GB | small |
| RealPLKSR 4x on the downscaled image | DirectML | ~1 s | ~2 s | < 1 GB | small |
| SeedVR2-3B int8, 3 worker processes, 256 px VAE tiles | DirectML | **91 s** | **114 s** | +5.9 GB | +7.2 GB |
| SeedVR2-3B int8, 512 px VAE tiles (Python eval) | CUDA | 40 s (1.3 MP) | 50–75 s | +14 GB (decoder) | +5.7 GB |
| OSEDiff, PyTorch fp16 (eval only) | CUDA | 2–23 s (+154 s load) | – | 5.4–6.3 GB | – |

SeedVR2's peak is in the **VAE decoder**, not the DiT (DiT about 5.6 GB):

- At 512 px tiles the decoder took 14 GB on CUDA.
- At 384 px tiles it ran out of memory on DirectML at 2 MP.
- The app therefore decodes with 256 px tiles (192 below 7 GB VRAM) and shrinks them
  automatically after an out-of-memory error.
- The generative model works at up to 2.1 MP (1 MP below 7 GB VRAM). Larger outputs are
  enlarged after it.
- On CUDA the app uses heuristic cuDNN algorithm search and an exact-size memory arena, to keep
  the peak close to what the graph needs.

## Decision

**Faithful path** (default, fast, nothing invented). Restormer deblur, with the model chosen by
the blur analysis:

- one-directional streaks → *motion*;
- otherwise → *defocus*;
- small photos → none.

Then:

1. Smart downscale to the measured sharp resolution (capped at 4x; 2.5x for portraits and light
   blur; 2x for old photos).
2. RealPLKSR Nomos2 4x.
3. Back to the target size.
4. GFPGAN on detected faces (CodeFormer when chosen and installed).
5. Light CAS.

About 5 s per MP on DirectML.

**Generative path** (the Creative slider): SeedVR2-3B one-step restoration (int8 ONNX), run in
worker processes:

- Input: the motion-deblurred image after motion deblur, the original otherwise.
- Colour fix: the input's low frequencies are put back.
- The result is blended with the Faithful result by the slider amount.

It is optional and installed separately (3.8 GB).

**Defaults per preset** (Creative amount while the slider is on *Auto*):

| preset | deblur | max downscale | Creative |
|---|---|---|---|
| Auto | from the analysis | per resolved preset | per resolved preset |
| Light blur | defocus (motion if streaks) | 2.5x | 0 % |
| Strong blur | defocus (motion if streaks) | 4x | 50 % |
| Camera shake | motion | 4x | 0 % |
| Out of focus | defocus (motion if streaks) | 4x | 50 % |
| Old / low-res photo | none | 2x (at least 2x) | 50 % |
| Portrait | defocus (motion if streaks) | 2.5x | 50 % + faces |

Without the generative model installed, every preset runs Faithful only.

**Rejected:**

- **NAFNet**: rings badly on anything but motion blur (GoPro), and is weak (REDS).
- **MPRNet**: non-commercial licence.
- **OSEDiff**:
  - Apache-2.0, but it needs the 5.2 GB Stable Diffusion 2.1 base (OpenRAIL++, official repo
    withdrawn) and a 5.6 GB tagger for prompts.
  - It garbles text, changes faces the most, and scores *above* the real photo on MUSIQ, which
    signals invented detail.
- **DAT2 4x**: slower and worse than RealPLKSR here.
- **SUPIR**: skipped as instructed.

## Models, licences, sizes

| model | role | licence | files | size | hosted at |
|---|---|---|---|---|---|
| Restormer — motion deblurring (Zamir et al., CVPR 2022) | deblur | MIT | `restormer-motion-fp16.onnx` | 54.9 MB | `models-2026.12` release (our fp16 export of the official `motion_deblurring.pth`) |
| Restormer — single-image defocus deblurring | deblur | MIT | `restormer-defocus-fp16.onnx` | 54.9 MB | `models-2026.12` release (export of `single_image_defocus_deblurring.pth`) |
| RealPLKSR Nomos2 4x | faithful detail | CC-BY-4.0 (already in 2.2) | existing | – | `models-2026.11` |
| SeedVR2-3B (ByteDance Seed), ONNX int8 by Moonbox | generative | Apache-2.0 | `dit.onnx` + `dit.onnx.data`, `vae_enc_first.onnx`, `vae_dec_first.onnx` | 4.06 GB | pinned Hugging Face revision `moonbox-dev/SeedVR2-3B-ONNX@e2a111d5` (`dit.onnx.data` is 3.56 GB, over GitHub's 2 GB asset limit) |
| GFPGAN 1.4 / CodeFormer | faces | already in 2.2 | – | – | – |

The ONNX files were made with the existing scripts in `tools/convert/` (`export_onnx.py`, then
`to_fp16.py`; see its README). `compare.py` against PyTorch gives 71–72 dB PSNR for both fp16
files.

## Samples

All in `.feasibility/deblur/samples/` (each shows the sharp original, the blurry input and the
results):

- `portrait_defocus.jpg`: input, Faithful, Creative 50 %, SeedVR2, OSEDiff, GT
- `portrait_lowres.jpg`: Faithful (routed), SeedVR2, OSEDiff
- `sign_motion31.jpg`: Faithful, deblurred + SeedVR2, deblurred + OSEDiff
- `text_gauss.jpg`: deblur only, Faithful, SR only, SeedVR2
- `landscape_defocus_heavy.jpg`: input, Faithful, Creative 50 %, SeedVR2, OSEDiff, GT
- `bird_gauss_light.jpg`: Faithful, SeedVR2, OSEDiff
- `arches_blur_jpeg_noise.jpg`: input, Faithful, Creative 50 %, SeedVR2, OSEDiff, GT

## Limitations

- Generative output redraws detail. Small text, licence plates and faces can look right but be
  wrong. The app says so next to the slider, and marks such outputs in the file metadata and in
  History / Outputs.
- JPEG-damaged blur ("mixed") is best faithful (0.230 vs 0.280 at 50 %). Auto cannot tell JPEG
  damage apart from strong blur yet, so it may pick 50 % there.
- The blur analysis:
  - detects one-directional motion reliably (scores 0.39–0.66 vs ≤ 0.2 for defocus);
  - is borderline on random camera shake (0.26);
  - underestimates upscaled low-res photos (they read as light blur).

  The presets let the user override it.
- SeedVR2 takes 1.5–2 minutes per image on DirectML; a 1 MP preview region takes about as long
  as the whole image would at 1 MP.
