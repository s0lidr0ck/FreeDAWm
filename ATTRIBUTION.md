# Attribution

Every asset bundled with or committed to FreeDAWm is listed here with its author, source and
license. `cargo xtask assets` fails CI when a tracked asset file has no row.

**Rule:** no Avid, Pro Tools, Adobe or other proprietary icons, images, sounds or presets — ever.
Assets must be original, public domain (CC0) or permissively licensed. See `AGENTS.md`.

## Icons

All UI icons (tools, transport, track buttons) are drawn procedurally in
`crates/ui-egui/src/icons.rs` by the SoundCraft contributors (MIT OR Apache-2.0). No UI icon image
files are used. The app icon and wordmark (`assets/app-icon/`, the owner's FreeDAWm bird logo and
wordmark) are listed under Files below.

## Audio

The demo session's audio (drums, bass, pad) and its MIDI parts are synthesised in code in
`crates/engine/src/demo.rs` by the SoundCraft contributors (MIT OR Apache-2.0). No sample files
are bundled.

Juke's synth voices (`crates/dsp/src/plugins/juke.rs`) and its starter kits
(`crates/engine/src/juke_kits.rs`) are original work by the SoundCraft contributors (MIT OR
Apache-2.0). The rendered kits' samples are made from those voices when a kit loads; no sample
files are bundled, and nothing is recorded from or modelled on another product's sounds.

## Fonts

FreeDAWm bundles one font file: Inter Regular (`assets/fonts/Inter-Regular.ttf`, listed under
Files below), used only for the launch splash's statement. The interface uses the default fonts
shipped inside the `egui` crate (Ubuntu-Light: Ubuntu Font Licence 1.0; Hack: MIT; Noto Emoji:
OFL-1.1; emoji-icon-font: OFL-1.1/MIT) and, at runtime, the operating system's own UI fonts when
available (never redistributed).

## Files

| Path | Author | Source | License |
|---|---|---|---|
| `docs/superpowers/specs/mockups/remote_mockup.png` | FreeDAWm contributors (Claude) | Rendered from `docs/superpowers/specs/mockups/remote_mockup.html` | MIT OR Apache-2.0 |
| `docs/superpowers/specs/mockups/mix_tweaks_mock.png` | FreeDAWm contributors (Claude) | Mockup for the Mix view tweaks spec (`docs/superpowers/specs/2026-11-02-mix-view-tweaks.md`), drawn for this project | MIT OR Apache-2.0 |
| `docs/superpowers/specs/mockups/pitch_compact_mock.png` | FreeDAWm contributors (Claude) | Mockup of the compact pitch box for the same spec, drawn for this project | MIT OR Apache-2.0 |
| `docs/superpowers/specs/mockups/vocal_studio_mock.png` | FreeDAWm contributors (Claude) | Mockup for the Vocal Studio spec (`docs/superpowers/specs/2026-11-03-vocal-studio-design.md`), drawn for this project | MIT OR Apache-2.0 |
| `docs/superpowers/specs/mockups/vocal_studio_edit_all_mock.png` | FreeDAWm contributors (Claude) | Mockup for the Vocal Studio Edit All spec (`docs/superpowers/specs/2026-11-04-vocal-studio-edit-all-design.md`) and the ML model downloads spec, drawn for this project | MIT OR Apache-2.0 |
| `docs/superpowers/specs/mockups/editors_link_midi_mock.png` | FreeDAWm contributors (Claude) | Mockup for the editor timeline link spec (`docs/superpowers/specs/2026-11-04-editor-timeline-link-design.md`) and the MIDI Editor editing spec (`docs/superpowers/specs/2026-11-04-midi-editor-editing-design.md`), drawn for this project | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-vocal-bars-dark.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-vocal-bars-light.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-vocal-minsecs-dark.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-vocal-minsecs-light.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-midi-bars-dark.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-midi-bars-light.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-midi-minsecs-dark.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/editor-link-midi-minsecs-light.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/editor_link.jsonl` (synthetic audio and the demo's MIDI) for the editor timeline link spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/midi-editor-editing-dark.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/midi_editor_editing.jsonl` (the demo's synthesised session) for the MIDI Editor editing spec | MIT OR Apache-2.0 |
| `docs/superpowers/specs/renders/midi-editor-editing-light.png` | FreeDAWm contributors (Claude) | Rendered from the app by `crates/ui-egui/examples/ui_shot.rs` with `examples/scripts/midi_editor_editing.jsonl` (the demo's synthesised session) for the MIDI Editor editing spec | MIT OR Apache-2.0 |
| `assets/splash/freedawm-splash-source.mp4` | the FreeDAWm owner | supplied by the owner (the launch animation, 1280×720, 24 fps, 6 s; its audio track is unused) | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `assets/fonts/Inter-Regular.ttf` | The Inter Project Authors (Rasmus Andersson) | Inter 4.1 release, `Inter-4.1.zip` › `extras/ttf/Inter-Regular.ttf` (https://github.com/rsms/inter/releases/tag/v4.1); embedded in the app for the splash statement | OFL-1.1 (`assets/fonts/Inter-LICENSE.txt`, the release's `LICENSE.txt`) |
| `assets/splash/frames/` | the FreeDAWm owner | `packaging/splash.py` from `freedawm-splash-source.mp4` (144 JPEG frames, 960×540, 24 fps, embedded in `freedawm`) | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `assets/app-icon/freedawm-logo-source.png` | the FreeDAWm owner | supplied by the owner (the FreeDAWm bird logo, 1536 px RGBA) | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `assets/app-icon/freedawm-wordmark-source.png` | the FreeDAWm owner | supplied by the owner (the FreeDAWm wordmark, 1536 × 300 RGBA) | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `docs/guide/images/` | FreeDAWm contributors (Claude) | screenshots of FreeDAWm rendered by `crates/ui-egui/examples/ui_shot.rs` with `docs/guide/shots.jsonl` from the synthesised demo session, for the user guide | MIT OR Apache-2.0 |
| `docs/guide/fonts/Fraunces-latin.woff2` | The Fraunces Project Authors (Undercase Type) | Fraunces variable font, Latin subset as served by Google Fonts (https://fonts.google.com/specimen/Fraunces; upstream https://github.com/undercasetype/Fraunces); used only by the user guide's pages | OFL-1.1 (`docs/guide/fonts/Fraunces-LICENSE.txt`) |
| `docs/guide/fonts/Inter-latin.woff2` | The Inter Project Authors (Rasmus Andersson) | Inter variable font, Latin subset as served by Google Fonts (https://fonts.google.com/specimen/Inter; upstream https://github.com/rsms/inter); used only by the user guide's pages | OFL-1.1 (`assets/fonts/Inter-LICENSE.txt`) |
| `packaging/macos/dmg/` (all files) | @XusBadia | original (macOS DMG window background and layout, from the app icon; see `packaging/macos/dmg/README.md`) | MIT OR Apache-2.0 |
| `assets/app-icon/freedawm-1024.png` | the FreeDAWm owner | rendered from `freedawm-logo-source.png` by `packaging/icons.py` | as the source |
| `assets/app-icon/freedawm-macos-512.png` | the FreeDAWm owner | rendered from `freedawm-logo-source.png` by `packaging/icons.py` | as the source |
| `assets/app-icon/freedawm.icns` | the FreeDAWm owner | rendered from `freedawm-logo-source.png` by `packaging/icons.py` | as the source |
| `assets/app-icon/freedawm.ico` | the FreeDAWm owner | rendered from `freedawm-logo-source.png` by `packaging/icons.py` | as the source |
| `crates/ui-egui/assets/brand/` | the FreeDAWm owner | the bird and the wordmark for the About box and menu bar, rendered from `assets/app-icon/freedawm-*-source.png` by `packaging/icons.py` | as the sources |
| `assets/app-icon/hicolor/` | the FreeDAWm owner | rendered from `freedawm-logo-source.png` by `packaging/icons.py` (16–512 px PNGs) | as the source |
| `docs/images/` | SoundCraft and FreeDAWm contributors | screenshots of FreeDAWm rendered by `ui_shot` from the synthesised demo session (the Vocal Editor shot uses its synthetic sung line) | MIT OR Apache-2.0 |
| `crates/ui-egui/assets/browser-thumbs/` | SoundCraft contributors | original: the plugin browser's card pictures, rendered by the app from its own built-in and FireStation editors (`cargo xtask browser-thumbs`) | MIT OR Apache-2.0 |
| `crates/dsp/src/plugins/fs/sat/presets/` | the FireStation owner | FireStation `Engine 8 - Sat/Presets/Factory/` (written by its `Tools/export-sat-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/tape/presets/` | the FireStation owner | FireStation `Engine 9 - Tape/Presets/Factory/` (written by its `Tools/export-tape-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/cleanq/presets/` | the FireStation owner | FireStation `CleanQ/Presets/Factory/` (written by its `Tools/export-cleanq-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/vint/presets/` | the FireStation owner | FireStation `Engine 11 - Vint/Presets/Factory/` (written by its `Tools/export-vint-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/mod_engine/presets/` | the FireStation owner | FireStation `Engine 4 - Mod/Presets/Factory/` (written by its `Tools/export-mod-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/transient/presets/` | the FireStation owner | FireStation `Engine 10 - Transient/Presets/Factory/` (written by its `Tools/export-transient-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/delay/presets/` | the FireStation owner | FireStation `Engine 6 - Delay/Presets/Factory/` (written by its `Tools/export-delay-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/reverb/presets/` | the FireStation owner | FireStation `Engine 7 - Reverb/Presets/Factory/` (written by its `Tools/export-reverb-presets.py`), copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/dsp/src/plugins/fs/amp/presets/` | the FireStation owner | FireStation `Engine 12 - Amp/Presets/Factory/`, copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `assets/amp-irs/` | the FireStation owner (FireStation) | FireStation `Engine 12 - Amp/IRs/{Fender,Marshall}/` (51 cabinet impulse responses of a 1971 Twin Reverb 2x12 and a Marshall-style cabinet; Fender, Twin Reverb and Marshall are trademarks of their owners, named only to identify the cabinets measured; no affiliation, 48 kHz 24-bit mono WAV), copied verbatim; the owner holds the rights (owner's decision, 2026-11-02) | MIT (`LICENSE-MIT`; copyright the FireStation owner) |
| `crates/band/library/` | the FireStation owner (A1:8) | FireStation `MIDI Studio/Library/{Players,Targets,Presets/Bands,Songs/Factory}` and `factory.json`, copied verbatim | MIT (`LICENSE-MIT`; copyright the FireStation owner; owner's decision, 2026-11-02) |
| `crates/ml/assets/DeepFilterNet3_onnx.tar.gz` | Hendrik Schröter (DeepFilterNet) | github.com/Rikorose/DeepFilterNet `models/DeepFilterNet3_onnx.tar.gz` at d375b2d, copied verbatim (the DeepFilterNet 3 encoder and decoders as ONNX, built into the app for Clean Vocal's de-noise) | MIT OR Apache-2.0 |
| `assets/lexicon/pron.lex` | Carnegie Mellon University and the cmudict contributors | The CMU Pronouncing Dictionary, `cmudict.dict` from https://github.com/cmusphinx/cmudict (master, fetched 2026-10-31), repacked by `tools/lexicon/build_lexicon.py` (zlib text; one line per word; a commonness score from WordNet sense counts added) for the Lyrics tab's syllables, stress and rhymes | BSD-style (2-clause) licence, `assets/lexicon/cmudict-LICENSE.txt` |
| `assets/lexicon/wordnet.lex` | Princeton University | WordNet 3.1 (https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz), pruned by `tools/lexicon/build_lexicon.py` (synsets with lemmas, glosses, examples and the hypernym/hyponym/similar/also-see/antonym pointers; the index; the exception lists) for the Lyrics tab's word finder and dictionary | WordNet 3.1 licence (permissive, keep the notice), `assets/lexicon/wordnet-LICENSE.txt` |
| `crates/engine/tests/fixtures/dawproject/bitwig-*.xml` | Bitwig GmbH (the DAWproject authors) | the projects `DawProjectTest.java` in https://github.com/bitwig/dawproject (master, fetched 2026-11-01) writes (`createAudioExample`, `createMIDIAutomationExample`, `createDummyProject`), transcribed by hand as XML; the test sample `white-glasses.wav` is not copied (the tests synthesise one of its shape) | MIT (the dawproject repository's `LICENSE`) |
| `crates/engine/tests/fixtures/dawproject/studio-one-style.xml` | FreeDAWm contributors (Claude) | original: a song laid out as Studio One's DAWproject export, written for the tests | MIT OR Apache-2.0 |
| Steinberg ASIO SDK 2.3.4 (`ASIO-SDK_2.3.4_2025-10-15.zip`, not in the repo; downloaded by `release.yml` and compiled into the Windows release builds) | Steinberg Media Technologies GmbH | ASIO audio device support on Windows | GPL-3.0 (dual-licensed by Steinberg: GPLv3 or the Steinberg ASIO License; used under GPLv3) |
| `packaging/windows/GPL-3.0.txt` | Free Software Foundation | Licence text shipped with the Windows ASIO builds | verbatim copying permitted |

## Recorded drum kits (not bundled)

Downloaded by the app from the `kits-v1` release into the kits folder, never committed; each kit's
licence and credit travel in its `kit.json` and the release's `kits.json`, and the Kits menu shows
them. Made from their sources by `tools/kits/`.

- `sm_drums`: SM Drums by Scott McLean, Tod Stillwell and Suleiman Ali, SFZ programming by
  DSmolken (smmdrums.wordpress.com, `SMDrums_Sforzando_1.2.zip`), trimmed to Juke's 4 velocity
  layers × 4 takes: public domain as listed by SFZ Instruments (sfzinstruments.github.io/drums/sm_drums);
  the authors offer it free of charge for any use. The source zip carries no licence file.

## Third-party ML models (not bundled)

Installed separately into the model folder by `cargo xtask ml-models` (`tools/ml-export`); never
committed or re-hosted: the app downloads each from its authors' own page. Listed with their
licences because FreeDAWm runs them; the app shows each model's licence status wherever models
are listed, downloaded or chosen (from the model card's `license` field in
`crates/ml/src/models.rs`). A model whose author states no licence is shown as "Licence not
stated by the author — personal use".

- `vocals_melroformer`: Mel-Band RoFormer vocals, Kimberley Jensen, huggingface.co/KimberleyJSN/melbandroformer: MIT.
- Song engines (Generate Song), downloaded on request from the `models-v1` release, not bundled with the app: `yue2` is yue2.cpp's `yue-server` (github.com/ServeurpersoCom/yue2.cpp, MIT, Copyright the yue2.cpp authors; built by `.github/workflows/engines.yml`, its licence shipped as `LICENSE-server` in the engine zip) with YuE2's weights (m-a-p, code Apache-2.0, weights CC BY-NC 4.0 with an additional creator permission; GGUF conversion huggingface.co/Serveurperso/YuE2-GGUF, fetched from there); `acestep` is acestep.cpp's `ace-server` (github.com/ServeurpersoCom/acestep.cpp, MIT) with ACE-Step 1.5's weights (MIT; huggingface.co/Serveurperso/ACE-Step-1.5-GGUF). Both servers include ggml (MIT). The app asks before downloading YuE2 (non-commercial for companies).
- `midi_llm`: MIDI-LLM (Shih-Lun Wu, Dave Carlton, Ryan Mikayawa, Yoon Kim, Chris Donahue, Cheng-Zhi Anna Huang; ISMIR 2026), huggingface.co/slseanwu/MIDI-LLM_Llama-3.2-1B, built on Meta's Llama 3.2 1B: Llama 3.2 Community License Agreement ("Built with Llama"). Its MIDI tokens are the Anticipatory Music Transformer's arrival-time encoding (github.com/jthickstun/anticipation, Apache-2.0), which `crates/ml/src/textmidi.rs` decodes.
- `bs_roformer_sw`: BS-RoFormer SW 6-stem (`BS-Rofo-SW-Fixed.ckpt`), huggingface.co/enerjazzer/BS-ROFO-SW-Fixed: licence unknown, personal use.
- `karaoke_melroformer`: Mel-Band RoFormer karaoke, aufr33 + viperx (UVR model repo): no licence stated, private use.
- Model code they run on (lucidrains BS-/Mel-RoFormer, ZFTurbo Music-Source-Separation-Training): MIT.
