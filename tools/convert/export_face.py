"""GFPGAN / CodeFormer weights -> ONNX, static 512 x 512.

    python export_face.py WEIGHTS OUT.onnx

The graph takes the aligned face as `input` [1, 3, 512, 512] RGB in 0..1 and returns `output` in
0..1 (the models themselves work in -1..1). CodeFormer also takes `weight`, a scalar fidelity
(0 = strongest restoration, 1 = closest to the input); for GFPGAN the exporter drops the unused
input.

GFPGAN: run fix_double.py on the result, or DirectML refuses the graph (float64 casts).
"""
import sys

import spandrel_extra_arches
import torch
from spandrel import ModelLoader

spandrel_extra_arches.install()

if len(sys.argv) != 3:
    sys.exit(__doc__)
m = ModelLoader().load_from_file(sys.argv[1])
print("arch", m.architecture.name, type(m.model).__name__)
net = m.model.eval().float()
codeformer = "CodeFormer" in type(net).__name__


class Wrap(torch.nn.Module):
    def __init__(self, n):
        super().__init__()
        self.n = n

    def forward(self, x, w):
        y = self.n(x * 2 - 1, weight=w) if codeformer else self.n(x * 2 - 1)
        if isinstance(y, (tuple, list)):
            y = y[0]
        return (y.clamp(-1, 1) + 1) / 2


with torch.no_grad():
    torch.onnx.export(Wrap(net), (torch.rand(1, 3, 512, 512), torch.tensor(0.5)), sys.argv[2],
                      input_names=["input", "weight"], output_names=["output"], opset_version=17, dynamo=False)
print("ok", sys.argv[2])
