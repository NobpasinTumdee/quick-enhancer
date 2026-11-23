# Model conversion scripts (developer-only)

These scripts turn the original PyTorch weights into the ONNX / ncnn files that the Model
Manager downloads from the `models-YYYY.MM` release. **End users never run Python:** the app
only downloads finished files, pinned by SHA-256 in `models.json`.

## Setup

Python 3.12, in a virtual environment outside the repository (or in the git-ignored
`.feasibility/`):

```powershell
py -3.12 -m venv .venv-convert
.venv-convert\Scripts\pip install -r tools\convert\requirements.txt
```

`torch` from PyPI is the CPU build, which is enough for exporting. `onnxruntime-directml` is
only used by `compare.py`.

## Scripts

| Script | What it does |
|---|---|
| `export_onnx.py WEIGHTS OUT [--tile N]` | spandrel-loaded model → ONNX fp32; static `N×N` input with `--tile`, else dynamic H / W |
| `export_half.py WEIGHTS OUT [--tile N]` | the same, traced in half precision (fp16 weights, math and I/O) |
| `to_fp16.py IN OUT` | fp32 ONNX → fp16 weights / math with fp32 I/O |
| `export_face.py WEIGHTS OUT` | GFPGAN / CodeFormer → ONNX 512×512, input / output in 0..1, CodeFormer `weight` input |
| `fix_double.py IN OUT` | removes float64 casts / constants (DirectML has no float64 kernels) |
| `to_ncnn.py WEIGHTS DIR NAME` | ESRGAN-family model → ncnn `.param` / `.bin` (fp16) for realesrgan-ncnn-vulkan |
| `compare.py WEIGHTS MODEL.onnx [--tile N]` | PSNR of the ONNX output against PyTorch on random tiles |

Every ONNX file the app uses has one image input `input` `[1, 3, H, W]` (RGB, 0..1) and one
output `output` (0..1), as `models.json` declares (`"range": "01"`). Precision, static tile size
and scale also go into the model's `onnx` entry in `tools/catalog/gen_models_json.py`.

## How each published file was made (`models-2026.11`)

Weights are the original releases listed in `licenses/THIRD_PARTY.md`.

| Published file | Commands |
|---|---|
| `realplksr-nomos2-4x-s256.onnx` | `export_onnx.py 4xNomos2_realplksr_dysample.pth OUT --tile 256` |
| `span-nomosuni-2x.onnx` | `export_onnx.py 2xNomosUni_span_multijpg_ldl.pth OUT` (dynamic) |
| `dat2-nomos2-4x-s256-fp16.onnx` | `export_onnx.py 4xNomos2_hq_dat2.pth fp32.onnx --tile 256`, then `onnxslim fp32.onnx slim.onnx --size-threshold 1000000`, then `to_fp16.py slim.onnx OUT` |
| `hatl-bhi-small-4x-s128.onnx` | `export_onnx.py 4xBHI_small_hat-l.safetensors OUT --tile 128` |
| `dejpg-realplksr-1x-fp16.onnx` | `export_half.py 1xDeJPG_realplksr_otf.safetensors OUT` (dynamic) |
| `denoise-realplksr-1x-fp16.onnx` | `export_half.py 1xDeNoise_realplksr_otf.safetensors OUT` (dynamic) |
| `scunet-real-psnr-s256-fp16.onnx` | `export_onnx.py scunet_color_real_psnr.pth fp32.onnx --tile 256`, then `to_fp16.py fp32.onnx OUT` |
| `gfpgan-1.4-512.onnx` | `export_face.py GFPGANv1.4.pth raw.onnx`, then `fix_double.py raw.onnx OUT` |
| `codeformer-0.1.0-512.onnx` | `export_face.py codeformer.pth OUT` |
| `esrgan-nomos2-4x-ncnn.zip` | `to_ncnn.py 4xNomos2_otf_esrgan.pth DIR 4xNomos2_otf_esrgan`, zip the `.param` and `.bin` |
| `yunet-2023mar.onnx` | unchanged copy of OpenCV Zoo's `face_detection_yunet_2023mar.onnx` |

With the pinned versions above, the DAT2 and SCUNet recipes reproduce the published files byte
for byte (checked 2026-11-23); the others depend on the exporter's node naming and may differ in
bytes but not in output.

## Checks before publishing

1. `compare.py` on the exported file: every tile ≥ ~45 dB for fp32, ≥ ~35 dB for fp16
   (measured: SPAN identical, DeJPG fp16 ≈ 60 dB, DAT2 fp16 59.7 dB).
2. Run it in the app on DirectML and on the CPU: the ignored tests in `tests/models_real.rs`
   (`cargo test --test models_real -- --ignored --nocapture`) compare GPU and CPU output and
   check the cleanup / face models.
3. Then follow `docs/RELEASING.md` → *Models release* (licence check, upload, `models.json`).
