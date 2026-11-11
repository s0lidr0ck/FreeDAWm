# ML model export (PyTorch → ONNX)

Reproducible exports of the local-ML models used by `crates/ml` (see
`docs/superpowers/specs/2026-10-21-ml-audio-design.md`). Nothing here is built into the app;
the scripts produce `<models>/<id>/{<id>.onnx, card.json, golden/}`.

| id | model | task | graph I/O | STFT |
|---|---|---|---|---|
| `htdemucs` | HTDemucs v4 (Meta) | 4 stems | wave `[1,2,343980]` → `[1,4,2,343980]` | in the graph |
| `drumsep_mdx23c` | MDX23C DrumSep 6-stem (aufr33 + jarredou) | drum split | wave `[1,2,130560]` → `[1,6,2,130560]` | in the graph |
| `srs_small` | Smule Renaissance Small | vocal de-reverb / de-noise, 48 kHz mono | spec `[1,2049,T,2]` → same (T dynamic) | host |
| `karaoke_melroformer` | Mel-Band RoFormer karaoke (aufr33 + viperx) | lead vs backing | spec `[2,1025,801,2]` → same | host |
| `vocals_melroformer` | Mel-Band RoFormer vocals (Kimberley Jensen, MIT) | vocals vs instrumental | spec `[2,1025,801,2]` → same | host |
| `bs_roformer_sw` | BS-RoFormer SW (licence unknown, personal use) | 6 stems | spec `[2,1025,801,2]` → `[12,1025,801,2]` | host |
| `basicpitch` | Basic Pitch (Spotify) | audio → MIDI, general | wave `[N,43844,1]` → note/onset/contour | in the graph |
| `piano_kong` | ByteDance high-res piano (Kong) | audio → MIDI, piano + pedal | wave `[1,160000]` → 7 roll outputs | in the graph |
| `adtof_drums` | ADTOF Frame_RNN | audio → MIDI, drums (GM) | log-filterbank `[1,T,84,1]` → `[1,T,5]` | host |
| `game_vocal` | GAME small (openvpi) | audio → MIDI, vocal | 5 graphs (encoder, segmenter loop, estimator, converters) | in the graph |
| `btc_chords` | BTC large vocabulary (Park et al.) | audio → chords (170 classes) | 10 s audio `[1,220500]` + n_frames → logits `[1,108,170]` | CQT in the graph |
| `whisper_lyrics` | Whisper large-v3 (ggml, **not ONNX**) | sung vocal → words + times | whisper.cpp / whisper-rs | in whisper.cpp |
| `midi_llm` | MIDI-LLM (Llama 3.2 1B + AMT MIDI tokens) | text → multitrack MIDI | `input_ids`, `attention_mask`, 16 × key/value cache → MIDI logits `[1,n,55030]` + cache | no audio |

Audio-to-MIDI `reference_infer.py` files include the full note decoding and return note lists
(`start`, `end` in seconds, `pitch`, `velocity`, plus pitch bends / pedals / fractional pitch where
the model has them); goldens are an input excerpt, the raw network outputs and `notes_5s.json`.

Each model folder has:
- `export.py`: downloads the original weights and exports the network (fp32, opset 17);
- `reference_infer.py`: **the complete inference pipeline in numpy + onnxruntime**
  (resampling assumptions, chunking, windows, STFT/iSTFT, stem assembly). This is the spec the
  Rust port follows; its docstring lists every step and constant;
- `run.py`: export → parity against the PyTorch original → golden files → quality numbers →
  24-bit renders → install + `READY.txt`.

Shared: `common.py` (paths, WAV I/O, metrics, card/install), `stft_onnx.py` (traceable STFT/iSTFT
without complex ops), `mss_fetch.py` (pinned ZFTurbo MSS model code, MIT), `make_testmix.py`
(synthetic 30 s mix of the owner's multitracks with the true stems), `bench.py` (CPU vs DirectML).

## Run

```sh
uv python install 3.12
uv venv --python 3.12 ../ml-venv          # outside the repo
uv pip install --python ../ml-venv/Scripts/python.exe torch==2.14.1 torchaudio==2.11.0 \
    --index-url https://download.pytorch.org/whl/cpu
uv pip install --python ../ml-venv/Scripts/python.exe -r tools/ml-export/requirements.txt
../ml-venv/Scripts/python.exe tools/ml-export/export_all.py          # idempotent
../ml-venv/Scripts/python.exe tools/ml-export/export_all.py --force htdemucs
```

`export_all.py` skips a model when its installed `card.json` sha256 matches the `.onnx`.
Defaults (override with env): `ML_DAW_ROOT` = the folder that holds this checkout, work dir
`$ML_DAW_ROOT/ml-export-work`, models `$ML_DAW_ROOT/ml-models` (the app reads
`%LOCALAPPDATA%\FreeDAWm\models` or `FREEDAWM_MODELS_DIR`), renders `$ML_DAW_ROOT/ml-renders`,
test media `ML_MEDIA_DIR` (real-song stems for the listening checks; no default, the scripts
that need it stop with a message when it is unset). The scripts name the stems by role
(`lead.wav`, `backing1.wav` .. `backing5.wav`); a `stems.json` in that folder maps each role to
the real file name. A run of all four takes about 30 min on CPU and downloads ~1.5 GB.

Basic Pitch's parity against the original TF SavedModel needs a third venv
`../ml-venv-tf` (`tensorflow-cpu`, `basic-pitch --no-deps`, `onnxruntime`, `soundfile`,
`resampy`, `librosa`, `mir_eval`, `pretty_midi`); without it that check is skipped.
The MIDI scripts also need `piano_transcription_inference`, `mido`, `pretty_midi`, `audioread`,
`torchlibrosa`, and for GAME `lightning loguru click pydantic torchmetrics colorednoise onnxslim h5py`.

MIDI-LLM (`midillm/`) needs `onnxruntime-genai==0.8.3` (its model builder makes the fp32 graph), `transformers`, `tokenizers` and `safetensors`; `export.py` then writes 8-bit `MatMulNBits` weights one tensor at a time (peak about 11 GB of RAM, 10 GB of disk). 4-bit was tried and rejected (top-1 agreement with PyTorch 73 %, 8-bit 97 %). The builder reads `config.rope_theta`, which transformers 5 no longer sets; `export.py` puts it back, or every position after the first is wrong.

Lyrics needs `openai-whisper` in ml-venv (cross-check only) and runs the whisper.cpp CLI
(downloaded by `lyrics/run.py`); its CUDA build is used when an NVIDIA GPU is present.

DirectML timing needs a second venv (the `onnxruntime` and `onnxruntime-directml` wheels clash):
`uv pip install onnxruntime-directml numpy scipy soundfile requests`, then `bench.py`.

## Golden files

`golden/README.txt` in each model folder gives shapes. All are raw little-endian float32 in C
order, **planar** (`[channel, sample]`, `[stem, channel, sample]`), at the model's sample rate.
They are outputs of `reference_infer.py` on CPU; a Rust port should reach SNR ≥ 60 dB.

## Notes for the Rust port

- `htdemucs` on DirectML needs the session config `ep.dml.disable_graph_fusion=1`
  (`card.json` → `dml_session_config`); with fusion the output is garbage.
- `karaoke_melroformer` on DirectML also needs `ep.dml.disable_graph_fusion=1` (attention is exported
  in 6 batch pieces so the 801-frame chunk fits in 8 GB unfused).
- `vocals_melroformer` is the karaoke graph with Kimberley Jensen's weights (`karaoke/export.py`'s
  `KJ_VOCALS` variant), `num_overlap` 2, fp32: an fp16 conversion held parity on CPU (62.8 dB) but
  not on DirectML (16.5 dB), see `melband_vocals/run.py`.
- `bs_roformer_sw` uses 409600-sample chunks (801 frames) instead of the config's 588800: at 1151
  frames DirectML (graph fusion off, 8 GB) hangs the GPU (887A0006). Needs graph fusion disabled too.
- `srs_small` frame-chunking is exact (context = receptive field), so any core size works.
