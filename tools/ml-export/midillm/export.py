"""MIDI-LLM -> ONNX for crates/ml/src/textmidi.rs, in a memory-lean way (fits in 16 GB RAM).

    python export.py <hf model dir> <work dir> <out dir>

1. onnxruntime-genai's model builder makes the fp32 decoder graph (CPU, GroupQueryAttention, key/
   value cache), but its own save step loads every weight into memory at once (13.9 GB for this
   1.7B-parameter model) and is replaced here: the graph is written as is, its weights stay one
   .bin per tensor.
2. The weights are converted one tensor at a time into a single model.onnx.data:
   - every MatMul with a constant weight [K, N] becomes com.microsoft MatMulNBits (8-bit,
     symmetric, block 32, no zero points so 128 is implied: q = clip(round(w / s) + 128, 0, 255),
     s = absmax(block) / 127; see quantize_nbits);
   - the LM head keeps only the MIDI vocabulary's columns (model tokens 128256..183286): the
     logits output is [1, n, 55030], logit j is MIDI token j (card: `logits_from` 128256);
   - the token embedding is stored as float16 (Gather, then Cast to float32).
"""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto, external_data_helper, helper, numpy_helper

LLAMA_VOCAB = 128256
BLOCK = 32
# 8-bit weights: 4-bit (round to nearest) moved the logits too far from the original (top-1
# agreement 73%, logit SNR 15 dB along a 150-token piece).
BITS = 8


def builder_fp32(hf_dir: Path, work: Path) -> Path:
    """The builder's fp32 graph in work/fp32/model.onnx with its weights as per-tensor .bin files."""
    import onnxruntime_genai.models.builder as B

    # The builder reads `config.rope_theta`, which transformers 5 no longer sets (it lives in
    # `rope_parameters`): the builder then silently uses 10000 instead of Llama 3's 500000 and every
    # position after the first is wrong. Put it back on the config the builder loads.
    # (transformers 4.x has it, but its loading needs more than 16 GB of RAM here.)
    real = B.AutoConfig.from_pretrained

    def from_pretrained(*args, **kwargs):
        cfg = real(*args, **kwargs)
        params = getattr(cfg, "rope_parameters", None) or {}
        if getattr(cfg, "rope_theta", None) is None and "rope_theta" in params:
            cfg.rope_theta = params["rope_theta"]
        return cfg

    B.AutoConfig.from_pretrained = from_pretrained
    if getattr(B.AutoConfig.from_pretrained(str(hf_dir)), "rope_theta", None) != 500000.0:
        raise SystemExit("export: the config's rope_theta is not 500000")

    out = work / "fp32"
    cache = work / "cache"
    if (out / "model.onnx").exists():
        return out / "model.onnx"
    out.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)

    def save_model(self, out_dir):
        print(f"Saving the ONNX graph in {out_dir} (weights stay in per-tensor files)", flush=True)
        model = helper.make_model(
            opset_imports=[self.clear_field(helper.make_operatorsetid("", 14), "domain"), helper.make_operatorsetid("com.microsoft", 1)],
            ir_version=7,
            producer_name="onnxruntime-genai",
            producer_version="0.0.0",
            graph=self.make_graph(name="main_graph", inputs=self.inputs, outputs=self.outputs, initializer=self.initializers,
                                  value_info=self.value_infos, nodes=self.nodes),
        )
        for name in os.listdir(self.cache_dir):
            if name.endswith(".bin"):
                shutil.move(os.path.join(self.cache_dir, name), os.path.join(out_dir, name))
        onnx.save(model, os.path.join(out_dir, self.filename))

    B.Model.save_model = save_model
    B.create_model("midi_llm", str(hf_dir), str(out), "fp32", "cpu", str(cache), filename="model.onnx")
    return out / "model.onnx"


def quantize_nbits(w: np.ndarray, bits: int = BITS):
    """MatMulNBits weights for w [K, N], symmetric, no zero points (2^(bits-1) implied):
    q = clip(round(w / s) + 2^(bits-1), 0, 2^bits - 1), s = absmax(block) / (2^(bits-1) - 1).
    4 bits: two values a byte, the lower K index in the low nibble.
    Returns (packed [N, K/BLOCK, BLOCK*bits/8] uint8, scales [N * K/BLOCK] float32)."""
    k, n = w.shape
    assert k % BLOCK == 0, (k, BLOCK)
    blocks = k // BLOCK
    zp = 1 << (bits - 1)
    wt = np.ascontiguousarray(w.T, dtype=np.float32).reshape(n, blocks, BLOCK)
    scales = (np.abs(wt).max(axis=2) / (zp - 1)).astype(np.float32)
    safe = np.where(scales == 0, 1.0, scales)[:, :, None]
    q = np.clip(np.rint(wt / safe) + zp, 0, (1 << bits) - 1).astype(np.uint8)
    if bits == 4:
        q = (q[:, :, 0::2] | (q[:, :, 1::2] << 4)).astype(np.uint8)
    return q, scales.reshape(-1)


class DataFile:
    """One external data file written tensor by tensor."""

    def __init__(self, path: Path):
        self.path = path
        self.f = open(path, "wb")
        self.offset = 0

    def add(self, name: str, arr: np.ndarray) -> TensorProto:
        arr = np.ascontiguousarray(arr)
        raw = arr.tobytes()
        t = TensorProto()
        t.name = name
        t.data_type = helper.np_dtype_to_tensor_dtype(arr.dtype)
        t.dims.extend(arr.shape)
        t.raw_data = b""
        pad = (-self.offset) % 64
        self.f.write(b"\0" * pad)
        self.offset += pad
        self.f.write(raw)
        external_data_helper.set_external_data(t, location=self.path.name, offset=self.offset, length=len(raw))
        t.data_location = TensorProto.EXTERNAL
        self.offset += len(raw)
        return t

    def close(self):
        self.f.close()


def convert(fp32_onnx: Path, out_dir: Path, quantize: bool = True, keep_fp32: tuple[str, ...] = ()) -> Path:
    src_dir = fp32_onnx.parent
    model = onnx.load(str(fp32_onnx), load_external_data=False)
    g = model.graph
    inits = {t.name: t for t in g.initializer}

    def load(name: str) -> np.ndarray:
        t = inits[name]
        if t.data_location == TensorProto.EXTERNAL:
            t2 = TensorProto()
            t2.CopyFrom(t)
            external_data_helper.load_external_data_for_tensor(t2, str(src_dir))
            return numpy_helper.to_array(t2)
        return numpy_helper.to_array(t)

    out_dir.mkdir(parents=True, exist_ok=True)
    data = DataFile(out_dir / "model.onnx.data")
    new_inits: list[TensorProto] = []
    used: set[str] = set()
    new_nodes = []
    logits_out = g.output[0].name
    assert logits_out == "logits", logits_out
    for node in g.node:
        if node.op_type == "MatMul" and node.input[1] in inits and (not quantize or any(k in node.name for k in keep_fp32)):
            # Kept in float32 (only the LM head is cut to the MIDI vocabulary).
            wname = node.input[1]
            w = load(wname)
            if logits_out in node.output or "lm_head" in node.name:
                w = w[:, LLAMA_VOCAB:]
            new_inits.append(data.add(wname + ".f32", w))
            used.add(wname)
            del w
            new_nodes.append(helper.make_node("MatMul", [node.input[0], wname + ".f32"], list(node.output), name=node.name))
        elif node.op_type == "MatMul" and node.input[1] in inits:
            wname = node.input[1]
            w = load(wname)
            is_head = logits_out in node.output or "lm_head" in node.name
            if is_head:
                w = w[:, LLAMA_VOCAB:]
            k, n = w.shape
            packed, scales = quantize_nbits(w)
            del w
            new_inits.append(data.add(wname + ".q", packed))
            new_inits.append(data.add(wname + ".scales", scales))
            used.add(wname)
            new_nodes.append(helper.make_node("MatMulNBits", [node.input[0], wname + ".q", wname + ".scales"], list(node.output),
                                              name=node.name + "_Q", domain="com.microsoft", K=k, N=n, bits=BITS, block_size=BLOCK,
                                              accuracy_level=4))  # 8-bit needs the int8 compute path on the CPU
            print(f"  {node.name}: [{k}, {n}] -> {BITS}-bit", flush=True)
        elif node.op_type == "Gather" and node.input[0] in inits and inits[node.input[0]].dims and inits[node.input[0]].dims[0] > LLAMA_VOCAB:
            ename = node.input[0]
            e = load(ename).astype(np.float16)
            new_inits.append(data.add(ename + ".f16", e))
            used.add(ename)
            del e
            mid = node.output[0] + "_f16"
            new_nodes.append(helper.make_node("Gather", [ename + ".f16", node.input[1]], [mid], name=node.name, axis=0))
            new_nodes.append(helper.make_node("Cast", [mid], list(node.output), name=node.name + "_Cast", to=TensorProto.FLOAT))
            print(f"  {node.name}: embedding -> float16", flush=True)
        else:
            new_nodes.append(node)
    for name, t in inits.items():
        if name not in used:
            new_inits.append(data.add(name, load(name)))
    data.close()
    del g.node[:]
    g.node.extend(new_nodes)
    del g.initializer[:]
    g.initializer.extend(new_inits)
    # Leave the logits' vocabulary dimension symbolic (ORT infers it).
    for o in g.output:
        if o.name == "logits":
            o.type.tensor_type.shape.dim[2].ClearField("dim_value")
            o.type.tensor_type.shape.dim[2].dim_param = "midi_vocab"
    out = out_dir / "model.onnx"
    # Written as is: the external data is already in place (onnx.save would try to move it).
    out.write_bytes(model.SerializeToString())
    return out


def main(hf_dir, work, out_dir):
    fp32 = builder_fp32(Path(hf_dir), Path(work))
    return convert(fp32, Path(out_dir))


if __name__ == "__main__":
    print(main(*sys.argv[1:4]))
