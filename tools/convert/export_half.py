"""PyTorch weights -> true fp16 ONNX (fp16 weights, math and I/O), traced from the half() model.

    python export_half.py WEIGHTS OUT.onnx [--tile N]

Used for the 1x RealPLKSR cleanup models (DeJPG / DeNoise): half the size and faster than a
fp32 graph converted afterwards. The app feeds fp16 input when the graph asks for it.
"""
import argparse

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
print("arch", m.architecture.name, "scale", m.scale)
net = m.model.eval().half()
s = a.tile or 64
dyn = None if a.tile else {"input": {2: "h", 3: "w"}, "output": {2: "H", 3: "W"}}
with torch.no_grad():
    torch.onnx.export(net, torch.rand(1, 3, s, s).half(), a.out, input_names=["input"], output_names=["output"],
                      opset_version=17, dynamic_axes=dyn, dynamo=False)
print("ok", a.out)
