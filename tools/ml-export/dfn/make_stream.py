"""Makes DeepFilterNet 3's three ONNX graphs streamable, for Mend's Live mode and Clean Vocal.

The graphs of the DeepFilterNet repository (models/DeepFilterNet3_onnx.tar.gz: enc.onnx,
erb_dec.onnx, df_dec.onnx) take a whole sequence: their GRUs start from zero and their
convolutions over time pad the start with zeros. This rewrites them, weights untouched, so a host
can run them a few frames at a time:

  - every GRU's initial state becomes an input (`h_<n>`, [1, 1, 256]) and its final state an
    output (`h_<n>_out`); feed zeros first, then each run's outputs to the next;
  - the zero padding in time is removed (enc: 2 frames before feat_erb and feat_spec; df_dec: 4
    frames before c0), so the host gives those frames itself: zeros first, then the last frames
    of the run before. enc then takes S + 2 frames and gives S; df_dec takes S frames of emb and
    S + 4 of c0.

Fed zeros for the states and the leading frames, the new graphs compute exactly what the old ones
did over a whole sequence. `--check` proves it on random input, whole and in pieces.

    python make_stream.py <in.tar.gz> <out.tar.gz> [--check]
"""

import io
import sys
import tarfile

import numpy as np
import onnx
from onnx import TensorProto, helper

NAMES = ("enc.onnx", "erb_dec.onnx", "df_dec.onnx")
# Frames of zero padding in time each graph's Pad nodes add before its inputs.
LEAD = {"enc.onnx": 2, "erb_dec.onnx": 0, "df_dec.onnx": 4}


def rewrite(model: onnx.ModelProto, lead: int) -> int:
    g = model.graph
    consts = {n.output[0]: n for n in g.node if n.op_type == "Constant"}
    inits = {i.name: i for i in g.initializer}
    states = 0
    for node in list(g.node):
        if node.op_type == "GRU":
            hidden = next(a.i for a in node.attribute if a.name == "hidden_size")
            name = f"h_{states}"
            while len(node.input) < 6:
                node.input.append("")
            node.input[5] = name
            g.input.append(helper.make_tensor_value_info(name, TensorProto.FLOAT, [1, 1, hidden]))
            while len(node.output) < 2:
                node.output.append("")
            if not node.output[1]:
                node.output[1] = f"{node.name}_h"
            g.node.append(helper.make_node("Identity", [node.output[1]], [f"{name}_out"], name=f"{name}_identity"))
            g.output.append(helper.make_tensor_value_info(f"{name}_out", TensorProto.FLOAT, [1, 1, hidden]))
            states += 1
        if node.op_type == "Pad":
            # Only the padding in time ([N, C, T, F]: T is axis 2), checked against LEAD.
            src = node.input[1]
            pads = None
            if src in inits:
                pads = onnx.numpy_helper.to_array(inits[src])
            else:
                # The exporter computes the pads from constants: follow Cast back to its source.
                cur = src
                for _ in range(32):
                    prod = next((n for n in g.node if cur in n.output), None)
                    if prod is None:
                        break
                    if prod.op_type == "Constant":
                        pads = onnx.numpy_helper.to_array(prod.attribute[0].t)
                        break
                    cur = prod.input[0]
            out = node.output[0]
            for user in g.node:
                for i, name in enumerate(user.input):
                    if name == out and user is not node:
                        user.input[i] = node.input[0]
            g.node.remove(node)
            print(f"  removed {node.name} (pads source {None if pads is None else pads.tolist()})")
    # Shapes change with the padding gone: let the runtime infer them.
    del g.value_info[:]
    _ = consts
    return states


def run_whole(sessions, erb, spec):
    enc, erbd, dfd = sessions
    names = [o.name for o in enc.get_outputs()]
    o = dict(zip(names, enc.run(None, {"feat_erb": erb, "feat_spec": spec})))
    m = erbd.run(None, {k: o[k] for k in ("emb", "e3", "e2", "e1", "e0")})[0]
    c = dfd.run(None, {"emb": o["emb"], "c0": o["c0"]})[0]
    return o["lsnr"], m, c


class Streamer:
    def __init__(self, sessions):
        self.enc, self.erbd, self.dfd = sessions
        self.h = {}
        for s in sessions:
            for i in s.get_inputs():
                if i.name.startswith("h_"):
                    self.h[(id(s), i.name)] = np.zeros(i.shape, np.float32)
        self.erb = np.zeros((1, 1, LEAD["enc.onnx"], 32), np.float32)
        self.spec = np.zeros((1, 2, LEAD["enc.onnx"], 96), np.float32)
        self.c0 = np.zeros((1, 64, LEAD["df_dec.onnx"], 96), np.float32)

    def _run(self, s, feeds):
        for (sid, name), v in self.h.items():
            if sid == id(s):
                feeds[name] = v
        names = [o.name for o in s.get_outputs()]
        out = dict(zip(names, s.run(None, feeds)))
        for k, v in out.items():
            if k.endswith("_out") and k.startswith("h_"):
                self.h[(id(s), k[:-4])] = v
        return out

    def step(self, erb, spec):
        e = np.concatenate([self.erb, erb], 2)
        sp = np.concatenate([self.spec, spec], 2)
        self.erb, self.spec = e[:, :, -LEAD["enc.onnx"]:], sp[:, :, -LEAD["enc.onnx"]:]
        o = self._run(self.enc, {"feat_erb": e, "feat_spec": sp})
        m = self._run(self.erbd, {k: o[k] for k in ("emb", "e3", "e2", "e1", "e0")})["m"]
        c0 = np.concatenate([self.c0, o["c0"]], 2)
        self.c0 = c0[:, :, -LEAD["df_dec.onnx"]:]
        c = self._run(self.dfd, {"emb": o["emb"], "c0": c0})["coefs"]
        return o["lsnr"], m, c


def check(old: dict, new: dict) -> None:
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = 1
    before = [ort.InferenceSession(old[n], so) for n in NAMES]
    after = [ort.InferenceSession(new[n], so) for n in NAMES]
    rng = np.random.default_rng(3)
    frames = 300
    erb = (rng.standard_normal((1, 1, frames, 32)) * 0.5).astype(np.float32)
    spec = (rng.standard_normal((1, 2, frames, 96)) * 0.5).astype(np.float32)
    want = run_whole(before, erb, spec)
    for sizes in ([frames], [1] * 40 + [4] * 40 + [100], [7, 293]):
        st = Streamer(after)
        parts, at = [], 0
        for n in sizes:
            parts.append(st.step(erb[:, :, at:at + n], spec[:, :, at:at + n]))
            at += n
        assert at == frames
        worst = 0.0
        for k, w in enumerate(want):
            axis = 1 if k != 1 else 2  # lsnr and coefs run [1, S, ...]; the gains [1, 1, S, 32]
            got = np.concatenate([p[k] for p in parts], axis)
            assert got.shape == w.shape, (k, got.shape, w.shape)
            worst = max(worst, float(np.abs(got - w).max()))
        print(f"  pieces {sizes[:3]}…: worst difference {worst:.2e}")
        assert worst < 1e-3, worst


def main() -> None:
    src, dst = sys.argv[1], sys.argv[2]
    old, new, others = {}, {}, []
    with tarfile.open(src) as tar:
        for member in tar.getmembers():
            if not member.isfile():
                continue
            data = tar.extractfile(member).read()
            base = member.name.rsplit("/", 1)[-1]
            if base in NAMES:
                old[base] = data
                model = onnx.load_from_string(data)
                print(base)
                states = rewrite(model, LEAD[base])
                onnx.checker.check_model(model)
                new[base] = model.SerializeToString()
                print(f"  {states} GRU state(s)")
            others.append((member, base, data))
    if "--check" in sys.argv:
        check(old, new)
    with tarfile.open(dst, "w:gz") as tar:
        for member, base, data in others:
            data = new.get(base, data)
            info = tarfile.TarInfo(member.name)
            info.size, info.mtime, info.mode = len(data), 0, 0o644
            tar.addfile(info, io.BytesIO(data))
    print(f"wrote {dst}")


if __name__ == "__main__":
    main()
