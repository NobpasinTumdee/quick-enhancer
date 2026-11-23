"""Make an ONNX graph float32-only (for DirectML, which has no float64 kernels).

    python fix_double.py IN.onnx OUT.onnx

torch's TorchScript exporter inserts Cast(to=DOUBLE) for GFPGAN's modulated-convolution weights.
Casts to DOUBLE become casts to FLOAT; DOUBLE constants, initializers and graph inputs / outputs
are converted to float32.
"""
import sys

import numpy as np
import onnx
from onnx import TensorProto, numpy_helper

if len(sys.argv) != 3:
    sys.exit(__doc__)
m = onnx.load(sys.argv[1])
fixed = 0
for n in m.graph.node:
    for a in n.attribute:
        if n.op_type == "Cast" and a.name == "to" and a.i == TensorProto.DOUBLE:
            a.i = TensorProto.FLOAT
            fixed += 1
        if a.type == onnx.AttributeProto.TENSOR and a.t.data_type == TensorProto.DOUBLE:
            a.t.CopyFrom(numpy_helper.from_array(numpy_helper.to_array(a.t).astype(np.float32), a.t.name))
            fixed += 1
for i, t in enumerate(m.graph.initializer):
    if t.data_type == TensorProto.DOUBLE:
        m.graph.initializer[i].CopyFrom(numpy_helper.from_array(numpy_helper.to_array(t).astype(np.float32), t.name))
        fixed += 1
del m.graph.value_info[:]
for v in list(m.graph.output) + list(m.graph.input):
    if v.type.tensor_type.elem_type == TensorProto.DOUBLE:
        v.type.tensor_type.elem_type = TensorProto.FLOAT
        fixed += 1
onnx.checker.check_model(m)
onnx.save(m, sys.argv[2])
print("fixed", fixed, "->", sys.argv[2])
