# FreeDAWm roadmap: milestones and what's next

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-11-06 · **Change:** trivial (G-21 done: compressed export dropped from the after-beta list; previously trivial: renamed to FreeDAWm, sessions saved as .fdawm; previously minor: Tune Vocal complete: track pitch view and mixer PITCH section, toward M16; previously minor: the Vocal Editor, toward M16; previously minor: Tune Vocal on clips, toward M16; previously minor: the Band built, round 2 is G-23; previously minor: audio I/O built, G-01 closed pending the owner's hardware pass; previously minor: VST-faithful FireStation editors; previously trivial: CleanQ's dynamic ceiling deferred; previously trivial: the Vint strip graph is done; previously minor: mixing-core FireStation ports milestone) · **Compared with:** Avid Pro Tools Ultimate 2026.4.1

> Pro Tools, AudioSuite and Avid are trademarks of Avid Technology, Inc. They are named here only to
> compare features (a workflow many users already know); FreeDAWm is not affiliated with or endorsed
> by Avid. See [NOTICE](../NOTICE).

Forward-looking plan. Status and numbers: [`../ROADMAP.md`](../ROADMAP.md) and
[`target-app-parity.md`](target-app-parity.md); the ranked work list: [`gaps.md`](gaps.md).
Hours are Opus 5.5 agent-hours.

## Current focus

1. **Merge or close the open bug-fix PRs** (#123–#134, #146, #148, #75) and fix the rest of the
   correctness backlog (G-04). ≈ 15–25 h.
2. **Audio interface pass** (G-01): built (mix at the device buffer, duplex feed, I/O Setup, LLM, record offset, ASIO build); the owner runs [`hardware-checklist.md`](hardware-checklist.md). macOS/Linux verification after it. ≈ 5–15 h.
3. **MIDI devices** (G-02): input to armed tracks and instruments, output, thru, clock. ≈ 20–35 h.
4. **macOS plugin hosting** (G-05): library-validation entitlement now (#153), then AU editors. ≈ 15–25 h.
5. **A PR CI that runs `cargo xtask ci`** on Linux, macOS and Windows (G-14). ≈ 5–10 h.

## Alpha gate

The core workflows a Pro Tools professional does every day, checked end to end on macOS (the
main platform). Any **no** keeps FreeDAWm **pre-alpha**, and those rows are the alpha checklist
(craftrules `standards/progress-docs.md`, Stages).

| Workflow | Works end to end? | Evidence | Hours to pass |
|---|---|---|---:|
| Record audio from a chosen interface at a chosen buffer size, monitor, punch, keep the takes | **no** | Built on Windows: device, rate and buffer choice; the mix at the device buffer; a one-clock input feed; named I/O on device channels; DAW, direct and low-latency monitoring; takes recorded to disk and placed by the measured round trip. Waiting for the owner's [hardware checklist](hardware-checklist.md); not verified on macOS. | 5–15 |
| Record MIDI from a controller and play it back through an instrument | **no** | Playback of drawn and imported MIDI works. No MIDI device layer at all, so nothing can be recorded and instruments can't be played live (G-02). | 20–35 |
| Edit: select, trim, fade, comp playlists, Slip/Shuffle/Grid | yes (rough) | Every Edit menu command; property-tested with undo; open bugs #3, #5, #140 don't block the workflow | — |
| Mix with built-in and third-party plugins, sends, busses, automation | partial (passes on CLAP/AU) | 26 built-in processors; CLAP and AU host on the signed macOS build, but its missing entitlement blocks third-party VST3 (#153). No AU plugin windows yet; VST3 windows on macOS only | 1–2 (entitlement) |
| Bounce mix and stems; save and reopen the session | yes (rough) | WAV/BWF/AIFF/FLAC/MP3/Opus bounce, stems, `.fdawm` save/reopen with plugin state; bugs #59, #103 in bus bounce and aux latency (PRs open) | — |
| Exchange a session with Pro Tools or a picture editor | **no** | No `.ptx`, AAF or OMF import or export (#40, G-03) | 50–90 |

**Result: pre-alpha.** Three workflows fail. Passing them is ~110–180 h, about 70 % of it
parallelizable (interfaces, MIDI and AAF are independent crates).

## Milestones

Status: ✅ done · 🟡 in progress · ⬜ not started.

| # | Milestone | Status | Remaining (h) |
|---|---|---|---:|
| M0–M4 | Workspace, gates, model, I/O, DSP, mix engine, command engine, Edit/Mix UI, playback, metering, plugins, bounce | ✅ | — |
| M5 | Editing depth (tools, modes, playlists, fades, comping, clip groups) | ✅ (bugs open, G-04) | in G-04 |
| M6 | Recording, punch, loop record, monitoring | 🟡 first cut; device choice and record-to-disk missing | 40–60 |
| M7 | MIDI and notation editing | 🟡 | 50–80 |
| M8 | CLI, control channel, MCP, agent acceptance test | ✅ | 3 |
| M9 | Process (offline processing) breadth ✅, Elastic Audio 🟡, Beat Detective 🟡 | 🟡 | 40–60 |
| M10 | Public releases on all platforms | ✅ v0.3.0 (2026-10-08) and v0.4.0 (2026-10-10) released for every platform | — |
| M11 | Third-party plugin hosting (CLAP, VST3, AU), video, surround | 🟡 mostly done | 40–60 |
| M12 | **Hardware**: audio interfaces, MIDI devices, sync, control surfaces | ⬜ | 110–180 |
| M13 | **Interchange**: AAF/OMF import/export, disk streaming, record to disk | ⬜ | 65–115 |
| M14 | **Fidelity harness** against Pro Tools (null tests, plugin responses) | ⬜ | 20–30 |
| M15 | **Localization**: catalog, fonts/RTL, the 8 Pro Tools languages first | ⬜ (PRs #141, #143 open) | 70–110 |
| M16 | Built-in plugin depth, sidechain, ARA | ⬜ | 110–170 |
| M17 | Immersive delivery (ADM BWF, object renderer, binaural) and video depth | ⬜ | 80–130 |

Channel strip, phases 0-1 (2026-10-19, branch `feat/chain-foundation`): every channel has a dense,
reorderable module chain (at most 32) with stable module ids; automation, plug-in state, editors and
meters are keyed by id. Session format v2 migrates v1 inserts, automation lanes and the lane view.
`mix.module_*` and `mix.strip_size` commands (the `mix.insert*` commands stay as position aliases),
per-module meters, a Mix chain view of tiles with live graphs and knobs (wide or slim), and an
in-place module drawer. Design: [`superpowers/specs/2026-10-19-firestation-channel-strip-design.md`](superpowers/specs/2026-10-19-firestation-channel-strip-design.md).
The chain fuzz and soak stress tests (`chain_fuzz.rs`, `chain_soak.rs`) are deferred by the owner;
they are not part of this milestone. Stage and readiness estimates are unchanged.

Channel strip, phase 2 (2026-10-19, branch `feat/mixing-core`): six FireStation engines ported
line by line from the owner's C++ (`fs_sat`, `fs_tape`, `fs_comp`, `fs_cleanq`, `fs_twine`, `fs_vint`;
`Category::FireStation`) and verified against C++ renders at RMS -60 dB or better (one written Comp
exception). Also: the fidelity harness (`crates/dsp/tests/fidelity.rs`, local-only audio), analysis rings
(`Plugin::analysis`), latency-change re-alignment with a 10 ms crossfade, `ParamInfo.display`/`group`,
and the strip UI (per-engine graphs, including Vint's measured frequency response computed off the UI
thread, Station accents and key knobs, drawer tabs, FireStation-first picker, factory presets,
`presets.import_firestation`). Deferred: CleanQ's dynamic-ceiling feature (drawing or learning the
ceiling shape; the ceiling is inert until then, its drawer tab greyed) and the Plan 3/4 engines. Details: [`plugin-parity.md`](plugin-parity.md#firestation-engines). Stage and readiness
estimates are unchanged.

The Band (2026-10-21, branch `feat/band`): MIDI Studio's six musicians inside the session, as track
inputs with a drawer per musician, the Sections and Band Chords lanes, Window › Studio and Print to
MIDI clips; the engine is a line-by-line translation checked event for event against MIDI Studio.
Design: [`superpowers/specs/2026-10-21-band-design.md`](superpowers/specs/2026-10-21-band-design.md);
the owner's pass: [`band-checklist.md`](band-checklist.md). Round 2 (Live mode, which needs MIDI
input, G-02; kept takes; audition while stopped; a per-song meter; MIDI export tabs) is G-23 in
[`gaps.md`](gaps.md). Stage and readiness estimates are unchanged.

Tune Vocal, plan 2 of 4 (2026-10-25, branch `feat/vocal-tuning`): Melodyne-style vocal tuning on audio
clips without ARA: the Vocal flag, background analysis, note edit commands, an off-thread render
cache read by playback and bounce, Tuned / Original, Commit and Remove (AI › Tune Vocal). Plan 3
(2026-10-25, branch `feat/vocal-editor`) adds the Vocal Editor: the note roll, curves, Level lane and
Tuning panel in the lower dock. Plan 4 (2026-10-25, branch `feat/vocal-mixer`) completes it: the
"waveform + pitch" track view with its TUNED badge, and the mixer's PITCH section with Centre note
and Flatten drift. Follow-ups are in gaps G-13. Part of M16's built-in depth; stage and readiness
estimates are unchanged.

M8 follow-up (2026-10-14): MCP batches above 1000 calls are rejected before execution (#460).
The limit is advertised in the tool schema and description; stage and readiness estimates are unchanged.

## To beta (~350–570 h, about 60–70 % parallelizable)

Beta needs ~75 % ready for real work and no blocking gap in session interchange.

| Step | Gaps | Est. (h) |
|---|---|---:|
| Correctness backlog and a PR CI | G-04, G-14 | 40–65 |
| Audio interfaces, MIDI devices, sync | G-01, G-02, G-11 | 60–100 |
| AAF/OMF interchange, disk streaming, record to disk | G-03, G-06 | 65–115 |
| Plugin hosting completeness on every OS, sidechain | G-05, G-13 | 35–60 |
| Fidelity harness and the fixes it finds | G-07 | 40–60 |
| Built-in plugin essentials (EQ/dynamics/reverb/delay depth, sampler) | G-08 (half) | 40–60 |
| UI feel pass: shortcuts, plugin windows, meters | G-19 (half) | 30–50 |
| Localization of the 8 Pro Tools languages (catalog, CJK fonts) | G-09 (part) | 40–60 |

Needs a human: hardware for G-01/G-02/G-10/G-11, native-speaker review, and owner decisions on
AAX, `.ptx` and Avid cloud features.

## After beta (to full parity, a further ~600–900 h)

Control surfaces (G-10), immersive delivery (G-12), the rest of the built-in plugin catalogue
(G-08), Elastic Audio depth (G-15), session templates and browser (G-16), video depth (G-17),
speech-to-text (G-18), notation and instrument depth (G-20), and the
remaining languages.

There is no `cargo xtask scorecard` in this repo; the measured number is `cargo xtask parity`
([`parity-checklist.md`](parity-checklist.md)).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-06 | trivial | Compressed export (G-21) done: dropped from the after-beta list |
| 2026-10-26 | trivial | The product is renamed FreeDAWm (was SoundCraft); sessions are saved as `.fdawm` and `.scraft` still opens. Estimates unchanged |
| 2026-10-25 | minor | Tune Vocal complete (plan 4 of 4: track pitch view, mixer PITCH section) noted under the milestones |
| 2026-10-25 | minor | The Vocal Editor (Tune Vocal plan 3 of 4) noted under the milestones |
| 2026-10-25 | minor | Tune Vocal on clips (plan 2 of 4) noted under the milestones |
| 2026-10-21 | minor | The Band: MIDI Studio's musicians built (owner's checklist pending); round 2 listed as G-23 |
| 2026-10-20 | minor | Audio I/O built (G-01 closed pending the owner's hardware pass): focus item and the recording alpha-gate row updated; that row stays **no** until the checklist passes and macOS is verified |
| 2026-10-20 | minor | Plugin editors: the 18 FireStation modules open VST-faithful editors in the Mix drawer by default (`ui.mix.wide_editors` toggles the narrow drawer) |
| 2026-10-19 | trivial | Channel strip phase 2: CleanQ's dynamic ceiling (draw/learn shape) deferred, inert until then |
| 2026-10-19 | trivial | Channel strip phase 2: the Vint strip graph is done, no longer deferred |
| 2026-10-19 | minor | Channel strip phase 2: six FireStation mixing engines ported and verified against the C++; fidelity harness, analysis rings, strip UI |
| 2026-10-19 | minor | Channel strip phases 0-1: module chain, session v2, strip and drawer; chain fuzz/soak tests deferred by the owner |
| 2026-10-14 | trivial | M8: reject oversized MCP batches before execution (#460) |
| 2026-10-10 | minor | Added the alpha gate (3 of 6 core workflows fail → pre-alpha) |
| 2026-10-10 | major | Created from ROADMAP.md's milestones and alpha list; added M12–M17 and the beta plan with estimates |
