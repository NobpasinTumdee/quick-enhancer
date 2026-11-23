"""fp32 ONNX -> fp16 weights and math with fp32 I/O (onnxconverter-common).

    python to_fp16.py IN.onnx OUT.onnx

Stale value_info is cleared first: otherwise the converter can leave Cast nodes whose types don't
match. For transformer graphs (DAT, SCUNet) simplify the graph with onnxslim first:

    onnxslim IN_fp32.onnx slim.onnx --size-threshold 1000000
    python to_fp16.py slim.onnx OUT_fp16.onnx
"""
import sys

import onnx
from onnxconverter_common import float16

if len(sys.argv) != 3:
    sys.exit(__doc__)
m = onnx.load(sys.argv[1])
del m.graph.value_info[:]
h = float16.convert_float_to_float16(m, keep_io_types=True, disable_shape_infer=True)
del h.graph.value_info[:]
onnx.save(h, sys.argv[2])
print("ok", sys.argv[2])
