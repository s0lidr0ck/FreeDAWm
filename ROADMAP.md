# FreeDAWm roadmap

**Stage: pre-alpha** · next: alpha, ~5% and ~110–180 h away (the alpha gate's three failing workflows)

> **Last reviewed:** 2026-10-18 · **Last updated:** 2026-11-02 · **Change:** minor (remote audio recording, the remote mix stream and connecting over the internet; previously minor: LAN remote recording: service remote page, remote recorders; previously minor: Lyrics writing tools; previously minor: Notes and Lyrics tabs, Lyrics ruler, Teleprompter; previously minor: hardware MIDI input, Musical Typing, MIDI Editor window; previously minor: RoFormer separation: Separate Stems › Best of each, Mel-Band vocals, SW for the rest; previously minor: Edit and Mix windows in step: Window › Sync; previously minor: Tune Vocal: track pitch view and mixer PITCH section; previously minor: Tune Vocal: the Vocal Editor; previously minor: Tune Vocal: tuning on clips; previously minor: MIDI Cleanup; previously minor: Menus clean-up and the AI menu; previously minor: Plugin browser; previously minor: Detect Chords and Transcribe Lyrics; previously minor: audio to MIDI: Convert to MIDI with four local models; previously minor: local ML audio: stems, karaoke, drum split, vocal cleanup; previously minor: slim editors for all 18 FireStation plugins; previously minor: slim Comp editor; previously minor: the Band: MIDI Studio's musicians, pending the owner's checklist; previously minor: audio I/O, pending the owner's hardware pass; previously minor: VST-faithful FireStation editors; previously trivial: the Vint strip graph is done; CleanQ's dynamic ceiling deferred; previously minor: mixing-core FireStation ports) · **Compared with:** Avid Pro Tools Ultimate 2026.4.1

> Pro Tools, AudioSuite and Avid are trademarks of Avid Technology, Inc. They are named here only to
> compare features (a workflow many users already know); FreeDAWm is not affiliated with or endorsed
> by Avid. See [NOTICE](NOTICE).

FreeDAWm is a pure-Rust digital audio workstation with a familiar Edit/Mix workflow, aiming to
cover everything a professional studio DAW does, and then go beyond it: faster, open, scriptable
and agent-controllable. Coverage is measured against Pro Tools, the most widely used reference. This page is
the one-page summary; the detail is in [`docs/target-app-parity.md`](docs/target-app-parity.md)
(assessment), [`docs/gaps.md`](docs/gaps.md) (ranked work list), [`docs/roadmap.md`](docs/roadmap.md)
(milestones and beta plan) and [`docs/parity-checklist.md`](docs/parity-checklist.md) (generated
menu checklist).

## Headline numbers

| Number | Value | Kind |
|---|---|---|
| Menu breadth (against the reference DAW, Pro Tools) | **482 / 512 menu leaves (94.1 %)**; engine alone 397 / 512 | measured (`cargo xtask parity` + UI-layer aliases) |
| Feature breadth | **~70 %** | estimated (menus plus plugins, hardware, formats, immersive, cloud) |
| **Ready for real work** | **~40 %** | estimated, weighted by dimension |
| Mainstream practitioner (tracking/mixing engineer) | **~36 %** | estimated: 50.6 % weekly-area depth × 0.90 interaction × 0.92 stability × 0.85 file exchange ([how](docs/target-app-parity.md#mainstream-practitioner-and-essentials-user)) |
| Essentials user (song or podcast, defaults) | **~57 %** | estimated: 72.4 % core-feature depth × 0.90 launch/stability × 0.90 discoverability × 0.97 files ([how](docs/target-app-parity.md#mainstream-practitioner-and-essentials-user)) |
| Remaining to alpha | **~110–180 Opus 5.5 agent-hours** (the failing gate rows) | estimated, calibrated on PR history |
| Remaining to beta | **~350–570 Opus 5.5 agent-hours** (includes the alpha work; ~60–70 % parallelizable) | estimated |
| Remaining to full coverage | **~950–1,450 Opus 5.5 agent-hours** | estimated |

| Audience | Ready % | Agent-hours to ~95 % | Work that dominates |
|---|---:|---:|---|
| Full target (ready for real work) | ~40 % | ~950–1,450 h | Hardware, the stock plugin catalogue, AAF/OMF and immersive, localization, fidelity |
| Mainstream practitioner | ~36 % | ~380–600 h | Interface and MIDI recording, AAF/OMF, stock-plugin depth, core-path bugs |
| Essentials user | ~57 % | ~110–200 h | Launch and platform fixes, onboarding, effect panels and presets, MP3/AAC export |

Hours are Opus 5.5 agent wall-clock hours, calibrated on this repo's PRs; 60–70 % parallelizes.
Detail: [target-app-parity.md](docs/target-app-parity.md#readiness-by-audience).

**Why pre-alpha:** the core-workflow gate in [docs/roadmap.md](docs/roadmap.md#alpha-gate) fails.
Editing, mixing, bouncing, and saving and reopening work end to end. But recording from a chosen
audio interface at a chosen buffer size is built and still waits for the owner's hardware pass
(and macOS verification). A Pro Tools user can't record MIDI from a keyboard (there is
no MIDI device layer). And they can't bring a session in or hand one back (no AAF/OMF/`.ptx`).
Readiness (~40 %) sits right at the alpha bar, but a gate failure keeps the app pre-alpha. The
2026-10-07 estimate ("~65 % overall, 110–150 h to 100 %") counted feature presence. This one
measures readiness and adds hardware, localization and field bug reports.

## By dimension

| Dimension | Ready | Remaining (h) | Doc |
|---|---:|---:|---|
| Features | ~45 % | 520–810 | [target-app-parity.md](docs/target-app-parity.md#feature-areas) |
| UI/UX fidelity | ~55 % | 60–100 | [ui-parity.md](docs/ui-parity.md) |
| File formats | ~40 % | 70–120 | [file-format-parity.md](docs/file-format-parity.md) |
| Hardware (interfaces, MIDI, surfaces, sync) | ~12 % | 110–180 | [hardware-parity.md](docs/hardware-parity.md) |
| Localization | ~12 % (1 of Pro Tools' 8 languages) | 70–110 | [localization-parity.md](docs/localization-parity.md) |
| Performance | ~35 % (unmeasured) | 40–70 | [hardware-parity.md](docs/hardware-parity.md#performance) |
| Stability | ~45 % | 50–80 | [gaps.md](docs/gaps.md) |
| Platforms | ~65 % (5 platforms built, macOS exercised) | 20–40 | [gaps.md](docs/gaps.md) |
| Ecosystem / plugins | ~30 % | in Features + Hardware | [plugin-parity.md](docs/plugin-parity.md) |
| AI features | ~5 % | 30–50 | [gaps.md](docs/gaps.md) |
| Agent control | ahead of Pro Tools | 3 | [control-protocol.md](docs/control-protocol.md), [mcp.md](docs/mcp.md) |

## Features

| Area | Breadth | Ready | Remaining (h) |
|---|---:|---:|---:|
| Recording and monitoring | 75 % | 40 % | 40–60 |
| Editing | 92 % | 65 % | 40–60 |
| Mixing and routing | 85 % | 60 % | 35–50 |
| Automation | 80 % | 55 % | 20–30 |
| Built-in plugins and Process | 40 % | 35 % | 80–120 |
| Third-party plugin hosting | 70 % | 45 % | 40–60 |
| MIDI and instruments | 70 % | 30 % | 50–80 |
| Score and notation | 60 % | 35 % | 20–30 |
| Elastic Audio, TCE, Beat Detective | 70 % | 35 % | 40–60 |
| Surround and immersive | 55 % | 35 % | 50–80 |
| Video and post | 60 % | 40 % | 40–60 |
| Session, file management, interchange | 55 % | 35 % | 60–100 |
| Collaboration and cloud (Avid services) | 0 % | 0 % | owner decision |
| Agent control | 100 % | 100 % | 3 |

Weights and evidence per row: [target-app-parity.md](docs/target-app-parity.md#feature-areas).

## Languages

| Language | Code | Status | Translated |
|---|---|---|---:|
| English | en | full | 100 % |
| Simplified Chinese | zh-Hans | none | 0 % |
| Spanish | es | none (PR #143 open) | 0 % |
| Hindi | hi | none | 0 % |
| Arabic | ar | none | 0 % |
| French | fr | none (PR #143 open) | 0 % |
| Portuguese | pt | none | 0 % |
| Indonesian | id | none | 0 % |
| Japanese | ja | none | 0 % |
| German | de | none | 0 % |
| Korean | ko | none | 0 % |
| Vietnamese | vi | none | 0 % |

Other languages shipped: 0. Detail: [localization-parity.md](docs/localization-parity.md).

## Upcoming

Ranked; the alpha gate, the alpha checklist and the beta plan are in [docs/roadmap.md](docs/roadmap.md).

1. **Alpha blocker:** audio interfaces: the owner's hardware pass ([checklist](docs/hardware-checklist.md)), then macOS/Linux verification (G-01). 5–15 h.
2. **Alpha blocker:** MIDI devices: record from keyboards, play instruments live, output (G-02). 20–35 h.
3. **Alpha blocker:** AAF/OMF import and export (G-03). 50–90 h.
4. macOS plugin entitlement (#153), then AU editors (G-05). 1 h for the entitlement (needed for the mix row), then 15–25 h.
5. Land the open bug-fix PRs and clear the correctness backlog (G-04). 15–25 h.
6. PR CI running `cargo xtask ci` on three OSes (G-14). 5–10 h.
7. Fidelity harness against Pro Tools (G-07). 20–30 h.

## Robustness

Property tests drive random edit sequences (with full undo), mutated session files and hostile
command parameters through the engine and mixer; they have caught fade, overflow and
unbounded-allocation bugs. Every command also runs with empty parameters on empty and demo
sessions. Hosted plugins live in isolated unsafe crates (`clap-host`, `vst3-host`, `au-host`) and
are created, loaded and destroyed off the audio thread. The video decoders survive hundreds of
mutated and truncated movies. Field reports from the first public week are tracked in
[gaps.md](docs/gaps.md) (G-04).

## Progress log

- **2026-11-02:** Edit › Split into Files, for a whole service recorded in one session. At each ruler marker inside the selection (or the whole session) and at the selection's edges, every clip of the selected tracks (else every audio, MIDI and instrument track) is cut, and each audio piece gets its own file, `<track> - <song marker>` (or `Part N`), copied sample for sample into `Audio Files` on save; the clips keep their places and play the same, MIDI clips are cut too, one undo step, and the long files leave the session (not the disk). The copies run behind a progress window. Vocal tuning analyses a clip's whole source, so on a piece it hears that song only (an hour-long file is over its 30-minute limit, and the Vocal Editor now says to split it); Clean Vocal, stems, Detect Tempo and Scout already worked on the clip's range. No readiness percentage change.
- **2026-11-02:** Remote audio recording and the remote mix stream (spec `2026-11-02-remote-audio-recording-design.md`). A follower PC records its armed audio tracks at full quality; at Stop each take travels as FLAC at the booth project's bit depth in resumable chunks and lands on the booth track on `Remote: <PC> › <input>` as a normal take in the project's format, drift-corrected through the sync points (rig test: 0-3 samples off at 2000 ppm). The booth sees the remote input's live level, a growing level sketch and clip markers; the performer gets a "Your level" window (big meter, held clip lamp, plain-language advice, 10-second level check). The booth can stream its live mix (Opus, pure Rust) to a remote performer, who records against it: his take lands where he played relative to what he heard (0 samples off end to end in tests, ≤1.4 samples across rates with drift, loss and jitter). Over the internet: iroh (direct encrypted QUIC, relay fallback), one-time 30-minute invites, configurable relay, link quality. Awaiting the owner's two checklists (guitar PC in the building; a friend at home). No readiness percentage change.
- **2026-10-31:** LAN remote recording (for the owner's three-PC church rig). Setup › Network › Allow control from this network (off by default) serves a service remote page on port 48100 (phone or stage PC: Record, Play, Stop with press-and-hold while recording, Arm All, Add Marker, track meters, the rig's PCs and disk space) and opens the control protocol to paired PCs; devices pair with a 4-digit code shown on the booth's screen and PCs find each other over mDNS. Another FreeDAWm PC can **follow the booth** as a remote recorder: it records, plays and stops with it at the booth's timeline position, streams a live sketch to the booth track on `Remote: <PC> › <port>`, and at Stop sends its exact MIDI take, aligned through NTP-style sync pings (piecewise-linear clock mapping; in tests, notes land within 0.15 ms with 2000 ppm of drift), queued on disk and resent after a dropped link. New crate `soundcraft-net`. Awaiting the owner's pass on the real rig. No readiness percentage change.
- **2026-10-31:** Lyrics writing tools (offline). The CMU Pronouncing Dictionary and WordNet 3.1 ship compressed (1 MB and 5.8 MB, built by `tools/lexicon/build_lexicon.py`) and load in the background on first use. The Lyrics tab counts syllables and stress from the dictionary, colours rhymes across all words by type (perfect, slant, multi-syllable, internal), flags lines that do not fit the same line of the section's first take (syllables, stresses), and gets a Word Finder (rhymes by syllable count and stress, synonyms and related words, definitions; a click puts the word in the line), alternate lines with a switcher, Drafts (save, restore, compare), a Hook counter with the most repeated words, and Copy for AI (a prompt with the lyrics, structure, counts, stresses and rhyme scheme).
- **2026-10-30:** Notes and Lyrics. Window › Notes: a session to-do list with done boxes, #tags and notes pinned to a time (flags on the Markers ruler). Window › Lyrics: write the song with line numbers, syllable counts and rhyme letters. View › Rulers › Lyrics: lines placed as range markers whose titles follow the text (drag, retime, drop from the tab, L, spread over a selection or from Transcribe Lyrics' words); clicking one selects its range and shows the comp's takes there. Window › Teleprompter: big lyrics following the playhead, docked or in its own window.
- **2026-10-30:** Hardware MIDI input. USB MIDI keyboards and an interface's 5-pin MIDI In now play and record: Setup › MIDI Input Devices lists the ports (Enable, activity light, MIDI Thru, rescans every 2 s); instrument and MIDI tracks take MIDI: All Inputs (new instrument tracks' default), a port, Omni or a channel; armed or monitored tracks play it through their instrument, stopped too, and a take records it into MIDI clips with punch, a playlist per loop pass and Input Quantize. Messages travel from the driver to the audio thread through lock-free per-port rings with sample-accurate offsets. The MIDI Keyboard window and the computer keys are a live port, Musical Typing; the Chords ruler's picker names chords as they are played; the MIDI Editor can live in its own window. Gap G-02 shrinks to MIDI output and clock. Awaiting the owner's pass with real hardware. No readiness percentage change.
- **2026-10-30:** RoFormer separation, each model for its strengths. Separate Stems gets a Model choice: HTDemucs (4 stems), BS-RoFormer SW (6 stems: adds guitar and piano) and **Best of each** (the default when both RoFormer models are installed): vocals from Mel-Band RoFormer, drums, bass, guitar, piano and other from SW, routed by a table measured on the owner's song (Mel-Band vocals carry the least instrument; SW's bass has half HTDemucs' bleed; drums a tie). Make Karaoke takes its vocals from Mel-Band RoFormer; Convert to MIDI isolates piano, drums, bass and vocals from the best installed separator (`ml.separate model: demucs|sw6|best`, `ml.midi isolate: piano`). No readiness percentage change.
- **2026-10-27:** The Edit and Mix windows stay in step with Mix in its own window. Both Tracks lists show the Edit window's folder tree (nesting, arrows, folder colours; closing a folder in either closes it in both), a closed folder hides its members' strips in the Mix window, a track selected in one window scrolls into view in the other, dialogs, the Send window and plugin windows opened from the Mix window open over it, and every global shortcut works from whichever window has the keyboard (Edit-only keys stay in Edit). Window › Sync switches Sync Selection, Scroll to Selected Track, Follow Folder Collapse and Sync Track Visibility (all on, kept in ui.json). No readiness percentage change.
- **2026-10-26:** Renamed to **FreeDAWm** (was SoundCraft): window titles, menus, About box (the owner's bird and wordmark), the programs `freedawm`, `freedawm-cli` and `freedawm-scan`, packaging, icons, the MCP server name and the settings folders (the old SoundCraft folder is copied on first start). Sessions are saved as `.fdawm` (bundles `.fdawmz`); `.scraft` files still open and save back to themselves. `FREEDAWM_*` environment variables, with the old `SOUNDCRAFT_*` names as fallback. The crates keep their `soundcraft-*` names. No readiness percentage change.
- **2026-10-25:** Tune Vocal, the track and the mixer (plan 4 of 4; Tune Vocal is complete). The track view "waveform + pitch" (`track.view {view: "pitch"}`, offered after waveform on audio tracks) draws a thick pitch line per note over the dimmed waveform, coloured and sized as in the Vocal Editor; hovering shows the note and its offset, a double-click opens the editor on that note, and tuned clips carry a TUNED / ORIGINAL badge (a click flips it). The Mix window gains a PITCH section on vocal strips, under the inserts and above Sends (View › Mix Window Views › Pitch, on by default; other strips leave the row blank so Sends, I/O and faders line up): the last ~3 s around the playhead, the note under it with its offset, and Centre note (one 40 % step) / Flatten drift (one 30 % step) through `vocal.step`, undoable and shown in the editor. Follow-ups in gaps G-13. No readiness percentage change.
- **2026-10-25:** Tune Vocal, the Vocal Editor (plan 3 of 4; the track lane and mixer panels are next). A "Vocal Editor" tab beside the MIDI Editor (Window › Vocal Editor, AI › Tune Vocal…, the clip menu): a piano roll of a vocal track's notes coloured by how far off they are and thick by loudness, the original and tuned pitch curves, breaths, scoops with Keep / Smooth / Shorten, read-only chord and key strips, a Level lane, selection across clips with Split and Join (S / J), and the Tuning panel: Tuned / Original, the five sliders (live, one undo step per drag), Snap to key, macro presets (and the user's own), Reset, Remove and Commit. No readiness percentage change.
- **2026-10-25:** Tune Vocal, clips and playback (plan 2 of 4; the Vocal Editor is next). Melodyne-style vocal tuning on audio clips, local and model-free: a per-track Vocal flag (`track.vocal`, following the track name until set), background analysis of what each clip sounds like (Clean Vocal blended in), note edits (`vocal.edit`, `vocal.macro` with three presets, `vocal.step`, `vocal.reset`, `vocal.split`, `vocal.join`, the `vocal.notes` query), an off-thread render cache that re-renders only the changed notes (a slider step is heard within a quarter second) and plays in playback and bounce, AI › Tune Vocal › Analyse Now, Tuned / Original, Commit and Remove Tuning. Edits save with the session; analyses and renders are derived. No readiness percentage change.
- **2026-10-24:** MIDI Cleanup. Per-clip, non-destructive tidy-up of transcribed piano MIDI (`midi.cleanup`, `midi.cleanup_commit`, `midi.cleanup_remove`): four stages in order, Wrong notes, Held over (the sustain pedal is changed just after chord changes), Legato and Extend, following the chord track, with Protect melody. The settings live on the clip; the cleaned notes are a derived preview that plays and renders instead of the take, while the piano roll, MIDI export and score keep the original until Commit. The Cleanup panel docks right of the piano roll with a chord strip and overlays; defaults Wrong 40 / Held over 70 / Legato 30 / Extend 40. Caveat: a loud, short (about 125 ms or less) chromatic top note over a chord that lacks it can be removed by Wrong notes; passing-note protection only covers steps between chord tones. The AI > MIDI Cleanup menu entry comes with the menus work. No readiness percentage change. AI › MIDI Cleanup… opens the MIDI Editor with the panel showing (`ui.midi_cleanup`).
- **2026-10-24:** Menus clean-up. The menu bar (FreeDAWm · File · Edit · View · Track · Clip · Event · AudioSuite (now Process) · AI · Options · Setup · Window) is now laid out by `crates/ui-egui/src/menubar.txt`: items sit under small headings, rarely used ones in More submenus, about 116 items that did nothing (Pro Tools leftovers, toggles nothing reads) and 24 duplicates are gone, and the ML tools have their own AI menu. Working toggles show a check mark, Freeze reads Unfreeze for frozen tracks, "…" items open a dialog (new: Pre/Post-Roll…, Track › Rename…, AI › Clean Vocal › Amount…, Session Info as Text… asks for a file), File › Export gains Stems…, Clips… and Clip Groups…, Track gains Rename…, Colour…, Hide and New Playlist. Six features with no window yet (Change Meter, Add Key Change, Change Velocity, Click/Countoff, MIDI Input Devices, Preferences) and MIDI Cleanup stay greyed out as "coming soon". The incumbent catalog (`menus.txt`) and the parity numbers are unchanged. No readiness percentage change.
- **2026-10-23:** Plugin browser. Window › Plugin Browser (`window.plugin_browser`, Alt+B) docks a panel with Instruments, Effects and Presets tabs, search, Favourites and category chips, and a grid of plugin cards showing the plugin's own window (bundled pictures for the built-in and FireStation plugins, regenerated with `cargo xtask browser-thumbs`; captured from the plugin's window for third-party ones). Drag a card onto a track or insert slot, or double-click, to place it. The Presets tab lists the selected plug-in's built-in and user presets, VST3 `.vstpreset` files and programs, and CLAP discovered presets; a hosted preset loads into the running plugin as one undo step (`mix.module_preset`). VST3 plug-ins are scanned out of process (`freedawm-scan`, cache `<config>/PluginScan/vst3.json`), so a crashing plugin no longer takes down the app. No readiness percentage change.
- **2026-10-22:** Detect Chords and Transcribe Lyrics (desktop only, private build). Track › Detect Chords… (`ml.chords`, `freedawm-cli ml chords`) runs BTC on a track or clip and writes the chords onto the Band's chord lane at the source's position on the tempo map, snapped to half bars (or bars) with repeats merged, replacing the lane over that range or only filling its gaps, in an estimated or chosen key; the bass/inversion guess is for full mixes. Track › Transcribe Lyrics… (`ml.lyrics`, `freedawm-cli ml lyrics`) runs Whisper large-v3 in whisper.cpp (CPU) on a vocal and puts timed lines and words on the track (a new per-track lyric lane, drawn along the bottom of the lane), exported from File › Export › Lyrics (LRC)… / Lyrics as Text… and as MIDI lyric events. One undo step each; both save and reload. BTC's decoder and Whisper's host steps are ports of the export lane's reference pipelines and give the golden segments and the golden transcript (the same whisper.cpp tokens). No readiness percentage change.
- **2026-10-22:** Audio to MIDI (desktop only, private build). Track › Convert to MIDI… (also on the track and clip context menus and the palette; `ml.midi`, `freedawm-cli ml midi`) transcribes each audio clip with a local model: General (Spotify Basic Pitch, with optional pitch bends), Piano (Kong et al., notes and sustain pedal), Drums (ADTOF, GM drum notes) or Vocal (openvpi GAME). The result is a new instrument track `<track> MIDI` under the source with a MIDI clip per source clip, notes placed on the session tempo map, one undo step; it saves, reloads and exports as .mid. The Rust decoders are ports of the export lane's reference pipelines and give the same notes as the golden files (times within 2e-6 s, equal velocities). No readiness percentage change.
- **2026-10-22:** Local ML audio (desktop only, private build). Track › Separate Stems… (HTDemucs v4), Make Karaoke… (HTDemucs + Mel-Band RoFormer karaoke), Split Drums… (MDX23C DrumSep, 6 stems) and Clean Vocal… (DeepFilterNet 3 de-noise, built in; Smule Renaissance de-reverb) run offline on a worker thread with progress and Cancel; separation results are new tracks under the source at its position, Clean Vocal replaces the clip like Process (then AudioSuite), one undo step each (`ml.separate/karaoke/drums/clean/models`, `freedawm-cli ml …`). Models run on ONNX Runtime with DirectML (no CUDA install) or the CPU, live outside the app in `%LOCALAPPDATA%\FreeDAWm\models` (Setup › ML Models…), are installed by `cargo xtask ml-models`, and match the export lane's reference pipelines (golden SNR 78–135 dB). No readiness percentage change.
- **2026-10-21:** Slim plugin editors. View › Mix Window Views › Slim Plugin Editors (`ui.mix.slim_editors`) now draws Comp, Transient, PitchEngine, DistEngine, Shale, Sat, Tape, Reverb, CleanQ, FilterEngine, ModEngine, Amp, ChamberMade, Delay, Swale, Twine, LevelUp and VintEngine (every FireStation plugin) at the narrow drawer's 280 px width: the same plugin, narrower (the full editor's sections, names and order; many-way pickers as one dropdown whose entries carry their descriptions; one display at a time; secondary controls in More), with the same commands, param ids and undo steps as the full editor, one kittest file per plugin and a shared slim kit (`editor_kit::slim`). VintEngine's eight faceplates are turned on their side (knobs, switches, VU and lamp moved with the panel, text upright); its flavour circuits stack their columns as rows. No readiness percentage change.
- **2026-10-21:** The Band: MIDI Studio's six session musicians inside the session. An Instrument or MIDI track takes a musician as its input (the input menu's Musicians); the strip's Band key opens the musician's drawer (instrument, player, Energy, feel dials, target and its keyswitch rows), and the band picks its target from the plugin on the track. The song is two Edit-window lanes, Sections and Band Chords (View › Rulers); Window › Studio holds the key, the section definitions, the feel and song feel, the factory songs, MIDI chord import, band presets and Print, which renders the band into MIDI clips. MIDI Studio's engine is translated line by line (`soundcraft-band`) and checked event for event against MIDI Studio's own processor over 153 scenarios, with the Classic feel byte-identical to its goldens; the Library (players, targets, presets, songs) is embedded, with a user overlay. Every action is a `band.*` command. Round 2 (Live mode after MIDI input, kept takes, audition while stopped) is gap G-23. The owner's checklist is [docs/band-checklist.md](docs/band-checklist.md). No readiness percentage change.
- **2026-10-20:** Audio I/O for tracking (G-01, pending the owner's hardware pass). The mix runs at the device buffer (96 or 128 samples, no 512-sample block). Input reaches the mix through a lock-free feed: one clock for ASIO, drift-corrected for two WASAPI devices. Setup › I/O maps named inputs and outputs to device channels; tracks and busses play on hardware outputs, and missing channels read "(not available)". The I button has three states (Auto, On, Off for direct monitoring). Low Latency Monitoring (transport key and Options) bypasses latent inserts on live audio tracks, with an "LLM" badge. Takes are placed by a record offset kept per interface, buffer and rate: the ASIO driver's figures (through a small C++ shim) or a loopback Measure, plus a manual offset and the mix's own delay. The Playback Engine window shows the mix block, latency, offset, Measure and underruns. `cargo xtask build-asio` makes the personal ASIO build. The owner's checklist is [docs/hardware-checklist.md](docs/hardware-checklist.md). No readiness percentage change until it passes.
- **2026-10-20:** VST-faithful plugin editors. Each of the 18 FireStation modules opens, in the Mix window, a wide drawer translated from its JUCE editor: its size and palette (in both themes), every drag, wheel, double-click, key and menu, the preset browser, Copy / Paste of FireStation preset text, the editor's own Undo and A/B, the VST's dirty star, its tooltips and live meters; panel state the VST saved (More, display spans) is saved with the session. It is the default; View › Mix Window Views › Plugin Editors as in the VST (`ui.mix.wide_editors`) brings back the narrow drawer, and View › Mix Window Views › Slim Plugin Editors (`ui.mix.slim_editors`, added 2026-10-21) draws Comp's editor at the narrow drawer's 280 px width. No readiness percentage change.
- **2026-10-19:** Mixing-core FireStation ports (channel-strip design phase 2). Six engines translated line by line from the owner's C++ and verified against C++ renders at RMS -60 dB or better: Sat (45 cases, worst about -117.5 dB), Tape (50, about -119.7 dB), Comp (55; `preset_blue_stripe_trash` at -50.6 dB under a written exception), CleanQ (55, about -107 dB, 254 host parameters), Twine (26, about -118 dB) and Vint (62, about -106.7 dB). Added the fidelity harness (`cargo xtask fidelity-inputs`/`fidelity-refs`/`fs-tables`; audio is local-only), analysis rings (`Plugin::analysis`), latency-change re-alignment with a 10 ms crossfade, `ParamInfo.display`/`group`, `Category::FireStation`, and the strip UI (per-engine graphs, Station accents and key knobs, drawer tabs, FireStation-first picker, factory presets, `presets.import_firestation`). Vint's strip graph is its measured frequency response, computed off the UI thread. Deferred: CleanQ's dynamic-ceiling feature (drawing or learning the ceiling shape; inert until then) and the Plan 3/4 engines. No readiness percentage change.
- **2026-10-19:** Channel module chain (channel-strip design phases 0-1). Each channel has a dense, reorderable chain of up to 32 modules with stable ids; automation, plug-in state, editors and meters follow the id. Session format v2 migrates v1 inserts, automation lanes and the lane view. New commands `mix.module_add/remove/move/bypass/param/params/state/copy/knobs` and `mix.strip_size` (the `mix.insert*` commands stay as position aliases), per-module meters, a Mix chain view of tiles with live graphs and knobs (wide 150 px or slim 80 px) with add, drag, copy and menu, an in-place 280 px drawer with every control, and the UI commands `ui.mix.drawer` and `view.mix_slim`. The chain fuzz and soak stress tests are deferred by the owner. No readiness percentage change.
- **2026-10-18:** Mono audio devices receive the average of the stereo channels before output clipping, retaining right-only audio. Surround mixes use the same average after their ITU stereo fold-down. Regression tests cover channel balance, polarity cancellation, clipping, centered mono tracks, each 5.1 channel and several callback sizes. No readiness percentage change.
- **2026-10-16:** Master fader volume automation uses each render block's actual last sample, matching track faders. Breakpoints in the next block no longer pull the preceding block's level toward them, and ramps reach their last-sample value. Regression tests cover nonzero starts, one-sample callbacks and several block sizes. No readiness percentage change.
- **2026-10-16:** Aux, folder and master latency calculations exclude inserts already taken out for producing NaN or infinity, matching audio tracks. Subsequent blocks no longer compensate for an insert that is bypassed after failure. Regression tests check impulse timing, healthy insert latency and reset/retry at 64- and 512-sample blocks. No readiness percentage change.
- **2026-10-14:** MCP `batch` rejects more than 1000 calls before executing any command, instead of silently running only the first 1000 (#460). No readiness percentage change.
- **2026-10-13:** WAV: an empty data chunk followed by metadata (LIST, bext) reads as no audio; a recording that was never finalised (sizes left at 0) still gives what was written (#466)
- **2026-10-13:** Sampler: a ping-pong loop keeps its phase and direction when a step is wider than the loop (a narrow loop with high transposition), instead of reflecting once and dropping the rest of the travel (#463)
- **2026-10-13:** `mix.instrument` takes the built-in instrument's parameters in `params` (validated, clamped) and refuses any other field, so a synth setting written beside `plugin` is an error instead of being accepted and ignored (#265)
- **2026-10-13:** A bounce of a bus that an aux returns onto lines up with the timeline: the aux insert's lookahead is compensated like the main mix's (it left the bus late and cut the end off), and tracks routed to that bus wait for it too (#246)
- **2026-10-13:** Delay compensation covers the inserts of the master faders: a bounce of the main mix with a Maximizer on the master lines up with the same mix without it (it came out 71 samples late) (#351)
- **2026-10-12 (audio device settings):** Setup › Playback Engine chooses the audio system, output and input device, sample rate and buffer size, kept per machine with the preferences; 16/32-bit integer devices are converted; ASIO is available behind the opt-in `asio` build feature (verified with an SSL 2+ at 128 samples). Channel routing, one duplex clock and latency compensation remain (G-01). No readiness percentage change.
- **2026-10-12 (Mix in a separate window):** Window › Mix in Separate Window shows the Mix window in its own native window, so Edit and Mix can be on screen at once (e.g. one per monitor), as in Pro Tools. No readiness percentage change.
- **2026-10-12 (plugin editors on Windows):** VST3 editors and embedded CLAP editors open in a FreeDAWm host window on Windows (owned by the app window, resized on request, closed from its title bar); verified with a JUCE VST3 compressor and a CLAP effect. No readiness percentage change.
- **2026-10-12:** `freedawm-cli convert` rejects an unparsable or zero `--rate` and an unknown `--bit-depth` (exit 1, nothing written) instead of converting with a default (#256)
- **2026-10-12:** Timecode labels with huge fields no longer overflow: frame counts are computed in 128 bits and clamp, so `setup.current_timecode` and `session.format_time` saturate instead of failing (#248)
- **2026-10-12:** `session.format_time` with the smallest bar number no longer fails with an internal error: the bar offset saturates (#249)
- **2026-10-12 (plugin editors on Windows):** VST3 editors and embedded CLAP editors open in a FreeDAWm host window on Windows (owned by the app window, resized on request, closed from its title bar); verified with a JUCE VST3 compressor and a CLAP effect. No readiness percentage change.
- **2026-10-12 (Mix in a separate window):** Window › Mix in Separate Window shows the Mix window in its own native window, so Edit and Mix can be on screen at once (e.g. one per monitor), as in Pro Tools. No readiness percentage change.
- **2026-10-12 (audio device settings):** Setup › Playback Engine chooses the audio system, output and input device, sample rate and buffer size, kept per machine with the preferences; 16/32-bit integer devices are converted; ASIO is available behind the opt-in `asio` build feature (verified with an SSL 2+ at 128 samples). Channel routing, one duplex clock and latency compensation remain (G-01). No readiness percentage change.
- **2026-10-10 (UI docking):** Tracks and Groups, Clips, MIDI Editor, Memory Locations, and Undo History use movable panels. Edit and Mix retain separate arrangements; Clips and MIDI Editor belong to Edit. Transport, plug-in, and specialist utility windows retain their existing hosts. Panel moves and reset are available as UI commands, and saved UI preferences retain arrangements. Common toolbar and track record-enable buttons share interaction, accessible labels and keyboard focus through `craft-ui`, retaining blue tool selection and red record-arm states. No readiness percentage change.
- **2026-10-10** — Stage re-normalized to **pre-alpha** by the core-workflow gate (interface recording, MIDI recording and session interchange fail). Full re-measure against Pro Tools Ultimate 2026.4.1 and the standard
  progress-docs set (this page, target-app parity, gaps, roadmap, UI/format/hardware/plugin/
  localization parity). v0.4.0 released. Community fixes: Cut All Automation, snap to neighbours,
  tempo stretch, I/O Setup, clip export names, menu-only dialogs, playback stop on new session.
- **2026-10-09** — Light theme and live system appearance, clip-gain fader and nudge, missing-media
  warnings, session audio health, malformed-JSON and encoder-extension checks in the CLI, macOS
  system menu bar, Windows DX12 default, Linux fontconfig, metronome click in playback.
- **2026-10-08** — v0.2.x–v0.3.0 released (Flatpak, AppImage zsync); VST3 quit crash fixed;
  Ctrl-as-Cmd shortcuts on Windows/Linux; plugin menus by vendor, AU inserts, instrument picker;
  rotating log file; branded DMG.
- **2026-10-07** — Repository published; About window credits; release CI on macOS runners.
- **2026-10-06** — Video track, Audio Unit hosting, plugin state and editors, Renderer; Mix
  window views; score export and print; surround mixing; VST3 hosting; property tests.
- **2026-10-05** — Foundation crates, Edit and Mix windows, playback, CLI/MCP, parity report,
  recording and punch, CLAP hosting, Elastic Audio, Beat Detective, release pipeline.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-02 | minor | Progress log: Edit › Split into Files (one file per song per track, in the same session); readiness estimates unchanged |
| 2026-10-31 | minor | Progress log: Lyrics writing tools (cmudict and WordNet offline, rhyme types, line fit, Word Finder, alternates and drafts, repetition check, Copy for AI); readiness estimates unchanged |
| 2026-10-27 | minor | Progress log: the Edit and Mix windows in step (Window › Sync); readiness estimates unchanged |
| 2026-10-25 | minor | Progress log: the track pitch view and the mixer PITCH section (Tune Vocal plan 4, complete); readiness estimates unchanged |
| 2026-10-25 | minor | Progress log: the Vocal Editor (Tune Vocal plan 3); readiness estimates unchanged |
| 2026-10-25 | minor | Progress log: Tune Vocal on clips (analysis, note edits, render cache, playback, commit); readiness estimates unchanged |
| 2026-10-24 | minor | Progress log: menus clean-up and the AI menu; readiness estimates unchanged |
| 2026-10-23 | minor | Progress log: plugin browser, out-of-process VST3 scanning, hosted presets; readiness estimates unchanged |
| 2026-10-22 | minor | Progress log: Detect Chords (BTC to the Band chord lane) and Transcribe Lyrics (Whisper large-v3, per-track lyric lane); readiness estimates unchanged |
| 2026-10-22 | minor | Progress log: audio to MIDI (Convert to MIDI, four local models); readiness estimates unchanged |
| 2026-10-22 | minor | Progress log: local ML audio (stem separation, karaoke, drum split, vocal de-noise/de-reverb); readiness estimates unchanged |
| 2026-10-21 | minor | Progress log: slim VintEngine editor; all 18 FireStation plugins have a slim layout; readiness estimates unchanged |
| 2026-10-21 | minor | Progress log: slim editors for 17 FireStation plugins (all but VintEngine); readiness estimates unchanged |
| 2026-10-21 | minor | Progress log: Slim Plugin Editors (Comp) noted under the VST-faithful editors entry; readiness estimates unchanged |
| 2026-10-21 | minor | Progress log: the Band (MIDI Studio's musicians), pending the owner's checklist; readiness estimates unchanged |
| 2026-10-20 | minor | Progress log: audio I/O for tracking, pending the owner's hardware checklist; Why pre-alpha and Upcoming item 1 updated; readiness estimates unchanged |
| 2026-10-20 | minor | Progress log: VST-faithful FireStation editors, the Mix drawer's default; readiness estimates unchanged |
| 2026-10-19 | trivial | Progress log: the Vint strip graph is done (no longer deferred); CleanQ's dynamic ceiling deferred |
| 2026-10-19 | minor | Progress log: mixing-core FireStation ports; readiness estimates unchanged |
| 2026-10-19 | minor | Progress log: module chain, session v2, strip and drawer; readiness estimates unchanged |
| 2026-10-18 | trivial | Mono device playback retains both stereo sides and surround fold-down channels; readiness estimates unchanged |
| 2026-10-16 | trivial | Master volume automation block boundaries corrected; readiness estimates unchanged |
| 2026-10-16 | trivial | Failed inserts no longer contribute aux, folder or master latency after containment; readiness estimates unchanged |
| 2026-10-14 | trivial | Progress log: oversized MCP batches rejected before execution (#460) |
| 2026-10-13 | trivial | Progress log: empty WAV data chunk followed by metadata (#466) |
| 2026-10-13 | trivial | Progress log: sampler ping-pong phase for wide steps (#463) |
| 2026-10-13 | trivial | Progress log: mix.instrument params, unknown fields refused (#265) |
| 2026-10-13 | trivial | Progress log: bus bounce compensates aux returns (#246) |
| 2026-10-13 | trivial | Progress log: master insert latency compensated (#351) |
| 2026-10-12 | trivial | Progress log: convert option validation (#256) |
| 2026-10-12 | trivial | Progress log: timecode overflow (#248) |
| 2026-10-12 | trivial | Progress log: bar offset saturates (#249) |
| 2026-10-10 | minor | Readiness-by-audience table with hours: full 950–1,450 h, mainstream 380–600 h, essentials 110–200 h |
| 2026-10-10 | minor | Added mainstream practitioner ~36 % and essentials user ~57 % to the headline numbers |
| 2026-10-10 | minor | Stage alpha → pre-alpha: the core-workflow gate fails on interface recording, MIDI recording and session interchange; alpha distance added |
| 2026-10-10 | major | Rewritten to the progress-docs standard: stage alpha (re-normalized the same day), two numbers (breadth ~70 %, ready ~40 %), dimension/feature/language tables, hours re-calibrated; area table moved to docs/target-app-parity.md, milestones to docs/roadmap.md |
| 2026-10-07 | major | Menu parity 94 %, ~65 % estimated feature parity, alpha list |
| 2026-10-06 | minor | Alpha and parity estimates |
