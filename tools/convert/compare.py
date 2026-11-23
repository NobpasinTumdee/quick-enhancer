"""Check a converted ONNX model against the original PyTorch weights on random tiles.

    python compare.py WEIGHTS MODEL.onnx [--tile N] [--provider dml|cpu]

Prints the PSNR of the ONNX output against PyTorch (fp32, CPU) on three smooth random tiles.
Release a file only when every tile is above about 45 dB (fp32) or 35 dB (fp16). --tile must
match a static export. Not for the face models (they need an aligned face and a weight input).
"""
import argparse

import numpy as np
import onnxruntime as ort
import spandrel_extra_arches
import torch
from spandrel import ModelLoader

spandrel_extra_arches.install()

ap = argparse.ArgumentParser()
ap.add_argument("weights")
ap.add_argument("onnx")
ap.add_argument("--tile", type=int, default=128)
ap.add_argument("--provider", default="dml", choices=["dml", "cpu"])
a = ap.parse_args()

net = ModelLoader().load_from_file(a.weights).model.eval().float()
prov = ["DmlExecutionProvider", "CPUExecutionProvider"] if a.provider == "dml" else ["CPUExecutionProvider"]
s = ort.InferenceSession(a.onnx, providers=prov)
inp = s.get_inputs()[0]
dtype = np.float16 if inp.type == "tensor(float16)" else np.float32
rng = np.random.default_rng(0)
for k in range(3):
    # Smooth random content: pure noise is not representative for SR models.
    x = torch.from_numpy(rng.random((1, 3, a.tile // 8, a.tile // 8), dtype=np.float32))
    x = torch.nn.functional.interpolate(x, size=(a.tile, a.tile), mode="bicubic").clamp(0, 1)
    with torch.no_grad():
        ref = net(x).clamp(0, 1).numpy()
    out = s.run(None, {inp.name: x.numpy().astype(dtype)})[0].astype(np.float32).clip(0, 1)
    mse = float(((out - ref) ** 2).mean())
    print(f"tile {k}: PSNR {10 * np.log10(1 / max(mse, 1e-12)):.1f} dB")
