"""ESRGAN-family PyTorch weights -> ncnn .param / .bin for realesrgan-ncnn-vulkan (fp16).

    python to_ncnn.py WEIGHTS OUT_DIR NAME

Writes OUT_DIR/NAME.param and NAME.bin with the blob names realesrgan-ncnn-vulkan expects
(`data` in, `output` out). Zip the two files for the models release; the catalog installs them
to models/realesrgan/. Users can also drop such files into models/custom/ (scale from a "4x"
prefix in the name).
"""
import os
import re
import sys

import pnnx
import torch
from spandrel import ModelLoader

if len(sys.argv) != 4:
    sys.exit(__doc__)
src, out_dir, name = sys.argv[1:4]
m = ModelLoader().load_from_file(src)
print("arch", m.architecture.name, "scale", m.scale)
net = m.model.eval()
os.makedirs(out_dir, exist_ok=True)
pt = os.path.join(out_dir, name + ".pt")
pnnx.export(net, pt, (torch.rand(1, 3, 64, 64),), (torch.rand(1, 3, 96, 80),), fp16=True)
p = open(os.path.join(out_dir, name + ".ncnn.param"), encoding="utf-8").read()
p = re.sub(r"\bin0\b", "data", p)
p = re.sub(r"\bout0\b", "output", p)
open(os.path.join(out_dir, name + ".param"), "w", encoding="utf-8").write(p)
os.replace(os.path.join(out_dir, name + ".ncnn.bin"), os.path.join(out_dir, name + ".bin"))
for f in os.listdir(out_dir):
    if f.startswith(name) and not f.endswith((".param", ".bin")):
        os.remove(os.path.join(out_dir, f))
print("ok")
