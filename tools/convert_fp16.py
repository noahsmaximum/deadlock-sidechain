"""Make the half-precision copy of the neural stems model that Sidechain prefers.

    python -m pip install onnx onnxconverter-common
    python tools/convert_fp16.py                     # models/htdemucs_fwd.onnx -> models/htdemucs_fwd_fp16.onnx

Inputs and outputs stay float32 (keep_io_types), so the Rust side doesn't change. On an RTX 4070
the network runs 4x faster through DirectML (~85 ms vs ~365 ms per 7.8 s window) and matches the
fp32 result at 45-52 dB SNR per stem (`sidechain neural-bench`). Sidechain picks `<name>_fp16.onnx`
automatically when it sits next to the configured model.
"""
import sys

import onnx
from onnxconverter_common import float16

src = sys.argv[1] if len(sys.argv) > 1 else "models/htdemucs_fwd.onnx"
dst = sys.argv[2] if len(sys.argv) > 2 else src.replace(".onnx", "_fp16.onnx")
model = onnx.load(src)  # pulls in the external .data weights
onnx.save(float16.convert_float_to_float16(model, keep_io_types=True), dst)
print(f"wrote {dst}")
