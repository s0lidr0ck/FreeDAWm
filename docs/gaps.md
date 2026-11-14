# Where FreeDAWm falls short of a complete studio workflow

> **Last reviewed:** 2026-10-18 · **Last updated:** 2026-11-06 · **Change:** trivial (G-20: the Juke drum machine design; previously minor: G-21 closed: MP3 and Opus export; AAC/M4A export not planned; previously minor: G-02 shrunk: hardware MIDI input built, output and clock left; previously trivial: G-24: stem separation quality; previously trivial: G-19: Shale's MIDI Out feeds instrument tracks, module MIDI inputs; previously trivial: renamed to FreeDAWm, sessions saved as .fdawm; previously minor: G-13: Tune Vocal complete, follow-ups listed; previously minor: G-13: the Vocal Editor; previously minor: G-13: built-in vocal tuning on clips; previously minor: G-19: slim editors for all 18 FireStation plugins; previously minor: G-19: slim Comp editor; previously minor: G-23 added: the Band's round 2; previously minor: G-01 closed pending the owner's hardware pass; G-06 shrunk to streaming sources; previously minor: G-19 VST-faithful FireStation editors) · **Compared with:** Avid Pro Tools Ultimate 2026.4.1

> Pro Tools, AudioSuite and Avid are trademarks of Avid Technology, Inc. They are named here only to
> compare features (a workflow many users already know); FreeDAWm is not affiliated with or endorsed
> by Avid. See [NOTICE](../NOTICE).

Every known shortfall, one entry each, ranked by how much it stops someone used to a professional DAW (Pro Tools is the reference) from doing
real work in FreeDAWm. This is the work list: pick from the top. Rows marked **alpha blocker**
fail the core-workflow gate in [`roadmap.md`](roadmap.md#alpha-gate) and keep FreeDAWm pre-alpha. Numbers and weights live in
[`target-app-parity.md`](target-app-parity.md); the plan that orders these into milestones is
[`roadmap.md`](roadmap.md). Hours are Opus 5.5 agent-hours (calibration in target-app-parity.md).
Estimates marked "≈" are judgements; issue numbers are `storytold/soundcraft` GitHub issues.

When you close a gap, delete its entry (or shrink it) in the same PR, and bump the timestamp.

## Ranked gaps

| # | Gap | Dimension | Doc | Est. (h) |
|---|---|---|---|---:|
| G-01 | **Closed pending the owner's hardware pass** ([hardware-checklist.md](hardware-checklist.md)). Left: ASIO in official builds (owner decision), macOS/Linux verification (JACK included) | Hardware | [hardware-parity.md](hardware-parity.md) | 5–15 |
| G-02 | **Alpha blocker.** No MIDI device input or output | Hardware / MIDI | [hardware-parity.md](hardware-parity.md) | 20–35 |
| G-03 | **Alpha blocker.** Cannot exchange sessions with Pro Tools (no AAF/OMF, no `.ptx`) | File formats | [file-format-parity.md](file-format-parity.md) | 50–90 |
| G-04 | Open correctness bugs in features counted as done (~25 issues) | Stability | this file | 25–40 |
| G-05 | (Entitlement part: alpha blocker for the mix row) Third-party VST3s blocked in the signed macOS build; AU editors missing | Ecosystem | [plugin-parity.md](plugin-parity.md) | 15–25 |
| G-06 | Sources decoded whole into RAM (takes already record to disk) | Performance | [hardware-parity.md](hardware-parity.md) | 10–20 |
| G-07 | Nothing is compared against Pro Tools' output (no fidelity harness) | Features | this file | 20–30 |
| G-08 | Built-in plugin catalogue thin and unverified | Features | [plugin-parity.md](plugin-parity.md) | 80–120 |
| G-09 | English only, no string catalog, no CJK/RTL rendering | Localization | [localization-parity.md](localization-parity.md) | 70–110 |
| G-10 | No control surfaces (HUI/MCU, EUCON) | Hardware | [hardware-parity.md](hardware-parity.md) | 40–70 |
| G-11 | No external sync (MTC, LTC, MIDI clock, video reference) | Hardware | [hardware-parity.md](hardware-parity.md) | 15–25 |
| G-12 | Immersive delivery: no Dolby Atmos/ADM BWF, MPEG-H, binaural | Features | [file-format-parity.md](file-format-parity.md) | 50–80 |
| G-13 | Sidechain inputs, ARA, out-of-process plugin sandboxing | Ecosystem | [plugin-parity.md](plugin-parity.md) | 30–50 |
| G-14 | Windows/Linux/Web exercised only lightly at runtime; no PR CI running the tests | Platforms / Stability | this file | 20–40 |
| G-15 | Elastic Audio depth: one algorithm, no Elastic Pitch | Features | [target-app-parity.md](target-app-parity.md) | 30–45 |
| G-16 | Session templates, track presets, workspace browser, batch rename | Features | [target-app-parity.md](target-app-parity.md) | 25–40 |
| G-17 | Video: HEVC/DNx decode, video export, external timecode | Features / Formats | [file-format-parity.md](file-format-parity.md) | 30–50 |
| G-18 | Speech-to-text transcription | AI | this file | 30–50 |
| G-19 | UI feel: generic plugin windows, shortcut collisions, pro-grade meters and scaling | UI/UX | [ui-parity.md](ui-parity.md) | 60–100 |
| G-20 | MIDI/instrument depth: MIDI learn, sampler, notation editing | Features | [target-app-parity.md](target-app-parity.md) | 40–60 |
| G-22 | Avid cloud services (collaboration, Sketch, Splice, sign-in) | Ecosystem | — | not estimated (owner decision) |
| G-23 | The Band (MIDI Studio) round 2: Live mode (needs G-02), kept takes, audition while stopped, per-song meter override, MIDI export tabs | Features / MIDI | [band design](superpowers/specs/2026-10-21-band-design.md) | ≈ 30–50 |

## Details

### G-01. Audio interface support

- **Closed (2026-10-20), pending the owner's hardware pass.** [`hardware-checklist.md`](hardware-checklist.md)
  (X32 and Scarlett 2i2 at 96 and 128 samples, Scarlett Solo at 128) decides it. Built:
  - `cargo xtask build-asio`: a personal release build with ASIO. The driver's own latency is read
    through `ASIOGetLatencies` (a small C++ shim, `soundcraft-asio-latency`). `CPAL_ASIO_DIR` is required.
  - The mix runs at the device buffer (no 512-sample block) when the rates match at speed 1.
  - Input reaches the mix in the output callback through a lock-free feed. It is fixed for one
    device (ASIO, duplex) and drift-corrected for two WASAPI devices.
  - Named inputs and outputs in Setup › I/O, mapped to device channels; the route menus list them.
    Tracks and busses routed to hardware outputs play there. A channel the device lacks shows
    "(not available)".
  - The three-state I button (Auto / On / Off for direct monitoring), and Low Latency Monitoring
    (transport key, Options, "LLM" badge on bypassed inserts).
  - A record offset kept per interface, buffer and rate: the reported or loopback-measured round
    trip, plus a manual offset, plus the mix's live delay. Takes land at their measured position.
  - The Playback Engine window shows the mix block, the reported latency, the offset, Measure and
    Cancel, the underruns with Reset, and the DSP load. Recording at half speed is refused.
- **Left:** ASIO in official builds (owner decision: Steinberg's SDK is proprietary or GPLv3),
  verification on macOS and Linux (JACK and PipeWire through it included), and aggregate devices. Known limits, in the checklist: the input
  count is unknown until the input opens (menus offer In 1–8 until then); a measured split WASAPI
  setup reads "not measured" until its input opens.
- **Estimate:** ≈ 5–15 h (macOS/Linux verification and fixes).

### G-02. MIDI devices

- **Built (2026-10-30, spec `2026-10-30-midi-input`):** MIDI input through `midir` (USB and 5-pin
  ports; Setup › MIDI Input Devices; MIDI: All Inputs / port / channel track inputs; live monitoring
  through instruments, stopped too; recording with punch, loop passes and Input Quantize; Musical
  Typing; chords from held keys in the Chords picker). Awaiting the owner's pass with real hardware.
- **Missing:** MIDI output to external gear (MIDI output tracks), MIDI Beat Clock / MTC out and
  chase, the MIDI input filter, MIDI merge recording (a pass replaces what it covers), MIDI learn,
  and a crash-safe autosave of MIDI heard during a take (audio takes have one).
- **Impact:** you can play and record from a keyboard; external synths and sync are out of reach.
- **Estimate:** ≈ 10–20 h (output tracks, clock out, merge, input filter).

### G-03. Session interchange with Pro Tools

- **Missing:** opening or saving `.ptx`/`.ptf`, AAF and OMF import and export (#40), Media
  Composer–compatible sessions. Our session format is `.fdawm` JSON.
- **Impact:** the standard's main-format bar for beta. A Pro Tools user cannot bring a project in
  or hand one back; post-production (picture editors deliver AAF) is blocked.
- **Estimate:** AAF/OMF import + export ≈ 50–90 h (FilmCraft already writes AAF/OMF; we can learn
  from it but not share code). `.ptx` is proprietary and undocumented: owner decision on whether
  black-box format study is in the clean-room rules; not included in the estimate.

### G-04. Correctness bugs in "done" features

**Fixed (2026-10-18):** Mono device playback averages the stereo channels before output clipping,
**Fixed (2026-10-16):** Master fader volume automation uses the first and last samples of each
**Fixed (2026-10-16):** Aux, folder and master latency calculations now skip inserts already
render block, matching track faders. A breakpoint at the next block no longer ramps the preceding
block, and ramps reach their last-sample value. Regression tests cover nonzero render starts,
one-sample callbacks and several block sizes. Interior automation breakpoints remain block-dependent.

disabled for producing NaN or infinity. Subsequent blocks compensate only for healthy inserts;
regression tests verify rendered impulse timing, retained healthy latency and reset/retry at
64- and 512-sample blocks, with audio tracks as the control.

instead of discarding the right side. Surround mixes average both sides of the ITU stereo
fold-down. Regression tests cover right-only and left-only audio, equal and opposite-polarity
channels, clipping order, centered mono tracks, each 5.1 channel and 1-, 37- and 512-frame callbacks.

Open on 2026-10-10 (several have PRs open):
editing #3, #5, #140, #144; clip/session #86 (PR #123), #88 (PR #130), #91 (PR #124), #92 (PR #132);
automation #104 (PR #134); mix/bounce #59 (PR #131), #103 (PR #133), #152; playback #101 (PR #129);
MIDI #87, #98 (PR #125); import #89 (PR #128); CLI #95, #96 (PR #126, #127); MCP #145 (PR #146);
web #63, #64, #136; Clip List #4 (PR #75); plugin view #6; shortcuts #7, #41;
launch #142; Linux #45, #107.
- **Impact:** each one is small, but together they say "presence is not parity": an outside
  contributor found seven in one sitting (#116–#122).
- **Estimate:** ≈ 25–40 h at 0.5–1.5 h each, plus review of the open PRs.

### G-05. Plugin hosting on macOS

- **Missing:** the `com.apple.security.cs.disable-library-validation` entitlement in the signed
  build, so hardened runtime refuses third-party VST3 bundles (#153, #149). AU editor views
  (`crates/au-host/src/lib.rs` header: "Not hosted yet: Audio Unit editor views"). Embedded CLAP
  GUIs; VST3 editors on Windows/Linux.
- **Estimate:** ≈ 15–25 h (entitlement ≈ 1 h; AU Cocoa views and VST3 HWND/X11 views most of it).

### G-06. Memory-bound media

- **Done:** recording streams each take to its own WAV file, rewriting the header every second, so a
  crash keeps what was written.
- **Missing:** streaming sources from disk. Every source is decoded whole into the `SourcePool`.
- **Impact:** hour-long multitrack sessions exhaust memory.
- **Known cost:** FireStation LevelUp keeps its reference audio in RAM as `f32` (about twice the C++ plugin's memory, at most 8 min per slot); kept as is.
- **Estimate:** ≈ 10–20 h.

### G-07. Fidelity harness against Pro Tools

- **Missing:** any measured comparison: null tests of bounces of the same material, fade curve
  and pan-law checks, plugin frequency/dynamics responses, elastic-audio quality, automation
  timing. Pro Tools is installed; it can be driven black-box with synthetic sessions (outputs kept
  under `plan/`, never committed).
- **Estimate:** ≈ 20–30 h for the harness and the first 20 scored cases; turns many "≈" in
  target-app-parity.md into measurements.

### G-08. Built-in plugins

See [`plugin-parity.md`](plugin-parity.md): 28 processors + 2 instruments against Pro Tools'
roughly 80 bundled processors and instruments; no fidelity checks; generic parameter UIs; EQ
parametric request #139. ≈ 80–120 h.

### G-09. Localization

See [`localization-parity.md`](localization-parity.md): no catalog, every string literal in code;
egui has no complex-script shaping or RTL; no CJK font fallback (PR #141 open). ≈ 70–110 h plus
native-speaker review.

### G-10. Control surfaces

None: no HUI or Mackie Control, no EUCON (Avid-proprietary, needs Avid's SDK: owner decision),
no generic MIDI controller mapping. Pro Tools users on S1/S3/S4/S6/Dock or HUI devices can't
switch. ≈ 40–70 h for HUI/MCU plus a mapping layer; hardware needed to verify.

### G-11. Sync

No MTC/LTC chase or generation, no MIDI Beat Clock (`setup.midi_beat_clock` stores a flag only),
no video reference or word clock awareness, no Satellite-style linking. ≈ 15–25 h after G-01/G-02.

### G-12. Immersive delivery

No Dolby Atmos renderer integration, ADM BWF import/export, re-renders, binaural monitoring,
MPEG-H (new in Pro Tools 2026.4), surround/object automation. ≈ 50–80 h; Dolby's renderer and
SDKs are licensed, so we'd write our own ADM and object renderer.

### G-13. Plugin ecosystem depth

No sidechain (key input) buses for any host; no ARA (Melodyne-style editing is a menu in Pro
Tools; built-in vocal tuning, AI › Tune Vocal, now covers its main use: the Vocal Editor, the
track's "waveform + pitch" view and the mixer's PITCH section. Its follow-ups: bringing the
resynthesis wobble on held notes (harmonic flux; v1 gates at 0.88 dB) down to Melodyne's 0.76 dB,
the channel-drawer fallback for the PITCH section if a
strip proves too tall (the row is a Mix Window View that can be switched off today), and
recalibrating the step and macro values against more of the owner's own edits); plugins run in-process, so a crashing plugin can take the session down. ≈ 30–50 h. AAX
cannot be hosted (#109): Avid's AAX SDK licence and PACE signing; owner decision, likely never.

### G-14. Platforms and CI

Only `freebsd.yml` and `windows-arm64.yml` run on pull requests, and only for manifest paths;
nothing runs `cargo xtask ci` on PRs. Windows and Linux have startup and rendering reports
(#41, #45, #107, #142). ≈ 20–40 h (a PR CI matrix plus runtime smoke tests on three OSes).

### G-15. Elastic Audio depth

One warp algorithm; Pro Tools offers several per track (polyphonic, rhythmic, monophonic,
varispeed, high-quality offline) plus Elastic Pitch and warp-marker editing depth. ≈ 30–45 h.

### G-16. Session management

No session templates, track presets, workspace/sound browser with previews, batch rename (new in
2026.4), disk-usage and relink depth. ≈ 25–40 h.

### G-17. Video and post

No HEVC, DNxHD/HR or AV1 decode; no bounce with video; no external timecode/9-pin; no iXML
field-recorder conform. ≈ 30–50 h.

### G-18. Speech-to-text

Pro Tools transcribes dialogue to a searchable transcript (upgraded in 2026.4). We have a
settings command only. Needs an openly licensed local speech model. ≈ 30–50 h.

### G-19. UI feel

See [`ui-parity.md`](ui-parity.md). ≈ 60–100 h.

- **Partly addressed (2026-10-19):** the Mix window's chain view shows every built-in module as a
  tile with live graphs and knobs (wide or slim), and an in-place drawer holds all of its controls,
  so built-in modules no longer need a separate generic parameter window. Third-party plug-ins still
  use their own editor windows and generic parameter lists; shortcut collisions, pro-grade meters and
  scaling remain. The estimate is unchanged.
- **Partly addressed (2026-10-20):** every FireStation module (18) opens a wide drawer that is a
  translation of its JUCE VST editor: its layout at the VST's `setSize`, its palette in both themes,
  every gesture, its presets browser, Undo / A-B, dirty star and tooltips (the narrow drawer stays
  behind View › Mix Window Views › Plugin Editors as in the VST, `ui.mix.wide_editors`; every module also has a slim 280 px layout, View › Mix Window Views › Slim Plugin Editors, `ui.mix.slim_editors`). What the
  editors still lack, from their inventories (`docs/superpowers/editors/*.md`), all small:
  - Shale's SC Listen pill sets its parameter but has no effect: the port does not read it
    (engine work). MIDI Out works since 2026-10-28: it feeds any track whose input is MIDI from
    the module (`track.input module:…`, the input menu's MIDI from).
  - Amp's blackface / silverface script is bold upright: the UI font has no italic (also in its slim layout).
  - Slim layouts taller than a strip with More open (Shale about 1100 px, ChamberMade about 1440 px) and VintEngine's turned faceplates scroll the drawer column.
  - Editors wider than the Mix window's room beside their strip (Vint 1000 px, ChamberMade 1190 px,
    and any editor on a strip far to the right) scroll the mixer sideways; they are never squashed.
  - The preset dirty star is kept by the UI, not the session (the VST saved it in its state): after
    a session opens, a module starts dirty only when its parameters differ from its preset's.
  - ChamberMade's readouts use the system's bold monospace (Consolas Bold on Windows; JUCE's
    default fixed face there, Lucida Console, has no bold).

### G-20. MIDI and instrument depth

MIDI learn, multi-sample instruments and a drum machine with a step sequencer (designed: **Juke**,
[juke.md](juke.md); the single-sample Sampler is built), notation editing (tuplets, lyrics,
articulations, parts). ≈ 40–60 h.

### G-22. Avid cloud services

Collaboration, cloud projects, Sketch, Splice, Learn, sign-in: 11 of the 30 menu leaves still
missing. Out of scope unless the owner decides otherwise.

### G-23. The Band, round 2

Round 1 (2026-10-21, plan `2026-10-21-band`) brought MIDI Studio's six musicians into the session:
musicians as track inputs, the band drawer, the Sections and Band Chords lanes, Window › Studio,
Print, band and feel presets, the user Library, and event-for-event fidelity with MIDI Studio
(owner's pass: [band-checklist.md](band-checklist.md)). Left for round 2, from the design's
non-goals and the round's reviews. ≈ 30–50 h, judgement.

- **Live mode:** playing chords on a MIDI keyboard, performance keys, recording live chords, and
  the Mod wheel feel switch (hidden until then). The Performer (`Live.h`) is ported and runs with
  Live off; MIDI device input exists now (G-02), the Performer is not fed from it yet.
- **Kept takes:** MIDI Studio's frozen / kept takes (`play_frozen`) are not reachable from the DAW
  (the Studio's Band view only marks them).
- **Audition while stopped:** instruments sound only while the transport plays; MIDI Studio's
  groove audition while stopped is missing.
- **Per-song meter override:** the session's meter rules (4/4, 3/4, 6/8; others play as 4/4);
  MIDI Studio's song meter cannot be set apart from it.
- **MIDI export tabs:** MIDI Studio's MIDI export tabs are not ported; Print (to clips) and the
  session's MIDI export stand in.
- **Imported songs:** an imported MIDI Studio song with unused (or post-Ending) sections builds a
  different Ample slot table, since a section's chord list comes only from the chords laid on the
  lane.

### G-24. Stem separation quality

Separate Stems has three models (HTDemucs, BS-RoFormer SW, Best of each: Mel-Band vocals + SW for
the rest, spec `2026-10-29-roformer-separation`). Left, from the 2026-10-30 measurement. ≈ 6–12 h.

- **Measured on one song:** best mode's routing comes from proxies (bleed correlation, mix minus
  sum) on two excerpts of one band mix, not SDR against references; the drums call is a tie.
  A small multitrack set with references would settle drums and bass.
- **Best's stems don't add back up exactly:** about -19 dB of the mix is missing or doubled
  (SW's vocals differ from Mel-Band's, which replace them); SW alone is -30 dB, HTDemucs -31 to
  -35 dB. Putting the residual into Other would close it.
- **Speed and memory:** best runs both RoFormers (~1 s of GPU time per second of audio,
  ~6.4 GB of an 8 GB GPU each, one after the other); no fp16 graphs yet.
- **Solo-piano isolation:** Convert to MIDI's piano isolation from SW's piano stem keeps 46 of a
  solo-piano golden excerpt's 62 notes (HTDemucs' other keeps 50–85); on a band mix SW's piano is
  the cleaner source.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-30 | minor | G-02 shrunk: hardware MIDI input, monitoring, recording and Musical Typing built; MIDI output, clock, input filter, merge recording and MIDI learn left (≈ 10–20 h) |
| 2026-10-30 | trivial | G-24 (new): stem separation quality, what the RoFormer measurement left: one-song proxies, best mode's -19 dB residual, two models' GPU time and memory, solo-piano isolation |
| 2026-10-28 | trivial | G-19: Shale's MIDI Out pill works: a module's MIDI output is a track input (`Route::Modules`, one or several modules merged), rendered in order, recorded as MIDI clips; SC Listen stays inert |
| 2026-10-26 | trivial | The product is renamed FreeDAWm (was SoundCraft); sessions are saved as `.fdawm` and `.scraft` still opens. Estimates unchanged |
| 2026-10-25 | minor | G-13: Tune Vocal complete (track pitch view, mixer PITCH section); shrunk to its follow-ups (wobble 0.76 dB, drawer fallback, recalibration); estimate unchanged |
| 2026-10-25 | minor | G-13: the Vocal Editor edits the built-in vocal tuning; estimate unchanged |
| 2026-10-25 | minor | G-13: built-in vocal tuning on clips (Tune Vocal) covers Melodyne's main use; ARA itself still missing; estimate unchanged |
| 2026-10-21 | minor | G-19: VintEngine's slim layout; all 18 FireStation editors have one; estimate unchanged |
| 2026-10-21 | minor | G-19: slim layouts for all FireStation editors but VintEngine (pending design); slim More overflow noted; estimate unchanged |
| 2026-10-21 | minor | G-19: Comp's editor also has a slim layout in the 280 px drawer width (`ui.mix.slim_editors`); estimate unchanged |
| 2026-10-21 | minor | G-23 added: the Band's round 2 (Live mode after G-02, kept takes, audition while stopped, per-song meter override, MIDI export tabs, imported songs' unused sections), ≈ 30–50 h |
| 2026-10-20 | minor | G-01 closed pending the owner's hardware pass (`hardware-checklist.md`); what's left re-estimated at 5–15 h. G-06 shrunk to streaming sources (takes record to disk), 10–20 h |
| 2026-10-20 | minor | G-19 partly addressed: VST-faithful editors for the 18 FireStation modules (wide drawer default, narrow behind a Mix view toggle); their open items listed; estimate unchanged |
| 2026-10-20 | trivial | G-06: noted LevelUp's in-RAM `f32` reference storage (~2x the C++ memory, ≤ 8 min per slot), kept as is; estimates unchanged |
| 2026-10-19 | minor | G-19 partly addressed: module chain view and in-place drawer for built-in modules; estimate unchanged |
| 2026-10-18 | trivial | G-04: mono device stereo/surround fold-down fixed and regression-tested; estimates unchanged |
| 2026-10-16 | trivial | G-04: master volume automation block boundaries fixed and regression-tested; estimates unchanged |
| 2026-10-16 | trivial | G-04: failed aux/folder/master inserts excluded from latency compensation after containment; estimates unchanged |
| 2026-10-12 | minor | G-01 shrunk: audio device, rate and buffer choice; opt-in ASIO |
| 2026-10-10 | minor | Marked the alpha blockers (G-01, G-02, G-03; parts of G-05 and G-06) from the core-workflow gate |
| 2026-10-10 | major | Created from the full re-measure against Pro Tools Ultimate 2026.4.1: 22 ranked gaps with evidence and estimates |
