"""PyTorch super-resolution / restoration weights (any architecture spandrel can load) -> ONNX, fp32.

    python export_onnx.py WEIGHTS OUT.onnx [--tile N]

--tile N   static N x N input (needed by transformer models such as DAT / HAT / SCUNet, and
           faster on DirectML); without it the height and width are dynamic.

Input `input` [1, 3, H, W] RGB in 0..1, output `output` [1, 3, H*s, W*s] in 0..1 (the app's
"01" range). Opset 17, TorchScript exporter.
"""
import argparse
import time

import spandrel_extra_arches
import torch
from spandrel import ModelLoader

spandrel_extra_arches.install()

ap = argparse.ArgumentParser()
ap.add_argument("weights")
ap.add_argument("out")
ap.add_argument("--tile", type=int, default=0)
a = ap.parse_args()

m = ModelLoader().load_from_file(a.weights)
print("arch", m.architecture.name, "scale", m.scale, "size requirements", m.size_requirements)
net = m.model.eval().float()
s = a.tile or 64
dyn = None if a.tile else {"input": {2: "h", 3: "w"}, "output": {2: "H", 3: "W"}}
t = time.time()
with torch.no_grad():
    torch.onnx.export(net, torch.rand(1, 3, s, s), a.out, input_names=["input"], output_names=["output"],
                      opset_version=17, dynamic_axes=dyn, dynamo=False)
print(f"exported {a.out} in {time.time() - t:.1f}s")
