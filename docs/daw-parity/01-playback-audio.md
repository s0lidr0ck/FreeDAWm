# Four-DAW parity, section 1: playing audio

> **Last reviewed:** 2026-11-15 · **Last updated:** 2026-11-16 · **Change:** minor (tier 2 work, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). They are named here only to compare features; FreeDAWm is not
> affiliated with or endorsed by any of them. See [NOTICE](../../NOTICE).

The first of seven sections comparing FreeDAWm with four reference DAWs: **playing audio**,
recording audio, mixing, built-in effects, MIDI recording, MIDI writing, and each DAW's unique
features. The existing parity docs ([`target-app-parity.md`](../target-app-parity.md),
[`gaps.md`](../gaps.md)) measure against Pro Tools alone; this series widens the reference.

**Scope of this section:** everything between pressing Play and hearing audio clips: transport and
locating, looping, speed and scrub, the audio engine, media and formats, how a clip is rendered,
tempo-following and time-stretch, and auditioning. Recording, the mixer, plug-ins and MIDI are
later sections.

## How this was measured

- **FreeDAWm:** read from the code on `main` at 8649ca7 (0.9.11), not from the docs. Each feature is
  marked by what the audio thread or the playhead really does. Several menu options store a value
  that nothing reads; those are marked **flag only**. The headline claims were re-read by hand.
- **The four DAWs:** from their published manuals and release notes, as known up to the versions
  above. Nothing was run side by side, so behaviour details (fade curves, stretch quality) are not
  compared. Entries are for each product's top edition.

**Legend.** Other DAWs: ● has it · ◐ partly or by another route · ○ does not.
FreeDAWm: **Works** · **Partial** · **Flag only** · **Missing**.

## Summary

| Area | FreeDAWm today | Distance from the four |
|---|---|---|
| A. Transport and locating | Solid core; several play commands are wrong | Small: bugs and small features |
| B. Looping | Gapless, sample-accurate, declicked, changes while playing | Small: no skip range |
| C. Speed, scrub, shuttle | Half speed only; no scrub audio at all | **Large**: all four scrub |
| D. Audio engine | Device choice, wide delay compensation, disk cache, declicked transport | Medium: sound quality at mismatched rates, realtime safety |
| E. Media and formats | Reads everything; streams only WAV | Small to medium |
| F. Clip rendering | Gain, envelopes, fades, automatic edge fades, clip EQ/comp, comping, real-time transpose, reverse and loop | Small to medium |
| G. Tempo and time-stretch | Offline WSOLA only; "Elastic" is a label | **Large**: all four stretch in real time and follow tempo |
| H. Auditioning | None for files or clips | **Large**: all four preview before placing |

Three things every one of the four does and FreeDAWm does not: **scrub audio**, **audio that
follows the tempo in real time**, and **previewing a file or clip before using it**.

## A. Transport and locating

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Play, stop, toggle on Space | ● | ● | ● | ● | **Works** |
| Play the selection once | ● | ● | ● | ● | **Works**; with only an insertion point it plays on from there |
| Pause (resume from where it stopped) | ◐ | ● | ● | ● | **Works**: Edit › Selection › Pause or Resume holds the playhead and the selection, and plays on from there with the same end and loop; while recording it is Stop |
| Return to zero, go to end | ● | ● | ● | ● | **Works**, seamless while playing |
| Rewind / fast forward while held | ● | ● | ● | ● | **Works**: a click jumps 1 s; held, the keys wind from 2× doubling each second up to 32× (no audio while winding) |
| Locate by clicking the ruler | ● | ● | ● | ● | **Works** |
| Locate by typing in the counter | ● | ● | ● | ● | **Works**: click the main counter's number, type a position in its format, Enter (the triangle beside it picks the format) |
| Counter formats (bars, time, timecode, samples) | ● | ● | ● | ● | **Works**: five formats, main and sub counter |
| Markers, recall while playing | ● | ● | ● | ● | **Works**, five marker rulers |
| Pre-roll and post-roll on playback | ● | ● | ◐ | ● | **Works** (not when looping) |
| Play start separate from the selection | ● | ● | ● | ● | **Flag only**: Dynamic Transport stores a flag; the timeline selection is never drawn or played |
| Return to start on stop, or stay | ● | ● | ● | ● | **Works** (Insertion Follows Playback) |
| Audition around an edit (play up to / from the start or end) | ● | ◐ | ◐ | ● | **Missing** |
| Follow modes: none, page, continuous, centred | ● | ● | ● | ● | **Works**: five modes, pauses when you scroll by hand |
| Keep playing while you edit | ● | ● | ● | ● | **Works**: a new session snapshot is swapped in without a gap |
| Click during playback | ● | ● | ● | ● | **Works**: synthesized, accent on beat 1; no sample sounds, no separate click output |
| Chase external timecode or clock | ● | ● | ● | ● | **Flag only** (belongs to the hardware sections) |

## B. Looping

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Gapless loop | ● | ● | ● | ● | **Works**: wraps inside the audio callback on the exact sample, disk cache pre-reads the loop start |
| Loop range separate from the selection | ● | ● | ● | ● | **Partial**: a sticky loop range exists, used when the insertion point is inside it |
| Move or resize the loop while playing | ● | ● | ● | ● | **Works**: a new range selection or loop-end drag moves the loop under way, without a jump |
| Turn looping on or off while playing | ● | ● | ● | ● | **Works**: off plays on; on loops the loop range (a loop behind the playhead waits for the next Play or locate); not while recording |
| Loop follows the selection | ● | ● | ● | ● | **Works** (it is the default behaviour) |
| Skip a range during playback | ○ | ○ | ● | ● | **Missing** |
| Smooth loop join (no click at the wrap) | ● | ● | ● | ● | **Works**: clips running across the loop's edges ramp out and back in over 1 ms (a short dip, no crossfade); plug-in tails run on |

## C. Speed, scrub and shuttle

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Scrub audio by dragging | ● | ◐ | ● | ● | **Works**: the Scrubber plays the track it drags over (or the selected tracks), and a Scrubber drag along a timebase ruler every audible track, forwards and backwards at the drag's speed, through the track's strip: clips, the inserts when all are built in (started afresh each frame, so no tails carry), the fader and mute. Not while playing; a strip with a third-party insert scrubs its clips and fader only |
| Shuttle / jog at variable speed | ● | ○ | ● | ● | **Works**: Options › More › Shuttle (Back, Stop, Forward; bind them to J, K and L in the key editor): forward plays at 1×, each press doubling to 4× (tape style, or with the pitch kept); back plays backwards from the playhead at 1× to 8×, heard frame by frame through the preview voice (the clips, as the Scrubber hears them); `transport.shuttle {speed}` sets a speed |
| Half-speed playback | ● | ○ | ● | ○ | **Works** (tape style, pitch drops) |
| Varispeed: any speed, with or without pitch change | ○ | ○ | ● | ○ | **Works**: Options › More › Varispeed… plays at 0.1–4× until set back to 1 (not during a take); Varispeed Keeps Pitch shifts the output back by 1 ÷ the speed (a two-tap delay-line shifter on the device output, about 21 ms later), otherwise the pitch moves as tape does |
| Reverse playback of the timeline | ◐ | ○ | ◐ | ◐ | **Partial**: Shuttle Back plays the clips backwards (not through the mixer) |

Studio Pro's Listen tool auditions a track from the pointer; it is not a true scrub. Logic's
Varispeed is the only whole-project speed control among the four.

## D. Audio engine

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Choose device, sample rate and buffer | ● | ● | ● | ● | **Works** (Playback Engine window) |
| ASIO on Windows | ● | ● | n/a | ● | **Partial**: only in a personal build (`cargo xtask build-asio`) |
| Aggregate or separate input and output devices | ● | ● | ● | ◐ | **Partial**: one output device; a different input device is drift-corrected |
| Recover when the device disappears | ● | ● | ● | ● | **Partial**: detected and reported; reconnect is a manual button |
| Session rate differs from the device rate | n/a | ● | ● | n/a | **Works**: a 32-tap windowed-sinc resampler whose cut-off follows the speed (no files are converted) |
| Multi-core rendering | ● | ● | ● | ● | **Partial**: tracks render in parallel; aux, folder and master strips are serial; worker threads have no realtime priority |
| Delay compensation | ● | ● | ● | ● | **Works**: inserts, pre-fader sends, aux returns, masters, sidechain keys, hardware outputs; up to about 22 s |
| Large-buffer playback path with a low-latency live path | ● | ● | ● | ● | **Partial**: Low Latency Monitoring bypasses latent inserts on live paths; no separate large playback buffer |
| Stream audio from disk with a cache | ● | ● | ● | ● | **Works** for WAV at the session rate (cache of a quarter of RAM, 4 s read-ahead) |
| Skip plug-ins on silent tracks | ● | ● | ● | ● | **Missing**: every active strip runs every block |
| CPU meter and dropout counter | ● | ● | ● | ● | **Works**; no per-plug-in or per-core breakdown |
| No click on stop, locate and start | ● | ● | ● | ● | **Works**: 3 ms output ramps on start, stop, locate, a Play while playing and a known end of play (not while recording, where the take's edges stay exact); plug-ins are still reset after the ramp, so reverb tails stop |
| Realtime-safe audio thread | ● | ● | ● | ● | **Partial**: plug-ins (built-in and hosted) and Clip Effects modules are made on the UI thread and handed over with the session; a new track's own buffers and delay lines are still sized on the audio thread |
| Change the session sample rate later | ◐ | ● | ● | ● | **Missing**: set at New Session only |
| Monitor section (dim, mono, cue mixes) | ◐ | ◐ | ◐ | ● | **Works**: Window › Control Room (section 3, E) |
| Runs in a browser | ○ | ○ | ○ | ○ | **Works**, single-threaded |

## E. Media and formats

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| WAV, BWF, RF64, AIFF | ● | ● | ● | ● | **Works** |
| FLAC, MP3, AAC/M4A, Ogg, Opus, CAF | ◐ | ● | ● | ● | **Works** (more than Pro Tools reads) |
| Files at another sample rate | ● | ● | ● | ● | **Works**: converted with a high-quality sinc at import or open |
| Mixed rates without converting files | ○ | ● | ○ | ○ | **Missing** (converted copies only) |
| Compressed and long files without filling memory | ● | ● | ● | ● | **Works** from the second open: a file that is not WAV at the session rate is decoded into memory once and kept as a float WAV in the project's `Decoded` folder, which later opens stream; a session never saved still decodes every time |
| Waveform cache | ● | ● | ● | ● | **Works** |

## F. How a clip is rendered

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip gain | ● | ● | ● | ● | **Works**, −144 to +36 dB |
| Gain envelope inside the clip | ● | ● | ○ | ● | **Works**, with render and conversion to and from volume automation |
| Fades drawn in real time, several shapes | ● | ● | ● | ● | **Works**: five shapes plus a continuous curve |
| Crossfades | ● | ● | ● | ● | **Partial**: both sides always share one shape |
| Automatic crossfade when clips overlap | ○ | ● | ◐ | ● | **Works**: Options › Overlap › Crossfade (section 4, D) |
| Automatic short fades at clip edges | ● | ◐ | ◐ | ● | **Works**: 1 ms on edges without a fade that face silence (Options › Automatic Edge Fades, on by default, length by command); butted clips play straight through; edits take the material unfaded |
| Per-clip effects | ● | ● | ○ | ● | **Partial**: a fixed EQ → compressor → gain chain; the others take any plug-in |
| Non-destructive offline processing stack | ○ | ◐ | ○ | ● | **Missing**: Process writes a new file |
| Mute a clip | ● | ● | ● | ● | **Works** |
| Reverse a clip without rewriting the file | ○ | ◐ | ● | ◐ | **Works**: Clip › Real-Time › Reverse in Place (section 4, E) |
| Per-clip transpose and fine tune | ◐ | ● | ● | ● | **Works**: Clip › Real-Time › Transpose…, in real time (section 4, E) |
| Loop a clip by dragging its edge | ● | ○ | ● | ○ | **Works**: the loop trim (Shift at the end trim) repeats the material in real time |
| Overlapping clips: top one plays, the lower one returns when uncovered | ● | ● | ● | ● | **Works** with Options › Overlap › Layer: the clip read gives way under a higher layer (the top clip's fades cross over to it); Overwrite, the default, still cuts the overlap away |
| Takes and swipe comping | ● | ● | ● | ● | **Works**; a take cannot be auditioned without making it active |
| Stereo clip on a mono track | ● | ● | ● | ● | **Works**: folded to mono at equal level |
| Split a stereo track into mono | ● | ● | ● | ● | **Works**: each new track plays its own channel |
| Freeze, commit, render in place | ● | ● | ● | ● | **Works**: freeze up to a chosen module; third-party plug-ins stay loaded |
| Built-in pitch correction heard live | ◐ | ● | ● | ● | **Works** (Tune Vocal), except on stretched clips |
| ARA editors | ● | ● | ● | ● | **Missing** |

## G. Tempo and time-stretch

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Real-time, pitch-preserving stretch | ● | ● | ● | ● | **Missing**: stretch is rendered offline (WSOLA); the real-time path is varispeed, read through the band-limited reader (design below) |
| Audio follows tempo changes | ● | ● | ● | ● | **Partial**: on a tick-timebase track a tempo edit moves clips to their bars and, on an Elastic track, renders them to the new length (offline, not in real time) |
| Choice of stretch algorithm | ● | ● | ● | ● | **Partial**: polyphonic, rhythmic, monophonic and X-Form each splice differently, in the offline render (no real-time stretch) |
| Warp markers you can drag | ● | ● | ● | ● | **Works** (offline render on release): the Warp track view; section 4, I |
| Quantize audio to the grid | ● | ● | ● | ● | **Partial**: Straighten conforms whole tracks; clips can be moved to the grid |
| Tempo detection | ● | ● | ● | ● | **Partial**: by command, not at import |
| Conform a clip to the session tempo | ● | ● | ● | ● | **Partial**: once, as varispeed, so the pitch changes |
| Loops that match tempo when dropped in | ● | ● | ● | ● | **Missing** |
| Loops that match key | ○ | ◐ | ● | ◐ | **Missing** |

## H. Auditioning

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Preview a file before importing | ● | ● | ● | ● | **Works** in the Workspace window: a click plays the file (read on a worker thread), again stops it; not in the import dialog |
| Preview in time with the session tempo | ● | ● | ● | ● | **Missing** |
| Preview while the session plays, in sync | ● | ● | ● | ● | **Missing** |
| Audition a clip from the clip list or pool | ● | ● | ● | ● | **Works**: Alt-click or Audition in the Clip List, with its clip gain |
| Preview volume and output | ● | ● | ● | ● | **Partial**: a preview level (Workspace window); always the first two outputs |

The player already has a raw preview voice (`Player::preview`, used only by Juke's hit picker), so
the engine side of H is partly there.

## Bugs found on the way

All in features the docs count as done. Each needs a regression test with its fix.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed:** with Loop Playback on, a selection of 64 samples or less played on to the end of the session; it now plays once and stops | `crates/playback/src/lib.rs:647`, `crates/ui-egui/src/lib.rs:2166` |
| 2 | **Fixed:** `transport.play {from}` ignored `from` when stopped; Edit › Play Timeline played the edit selection. Both now play from where they say (Play Timeline loops the timeline range with Loop Playback on) | `crates/engine/src/cmd/transport.rs:10`, `cmd/edit_more.rs:128` |
| 3 | **Fixed:** Play Selection with an insertion point played one sample; it now plays on from there | `crates/ui-egui/src/lib.rs:2243` |
| 4 | **Fixed:** a stereo clip on a mono track played only the left channel; it now folds to mono at equal level (½L + ½R, as a mono device folds the main mix) | `crates/mix/src/lib.rs:3262` |
| 5 | **Fixed:** Split into Mono made two tracks that both played the left channel; each clip now carries the source channel it plays | `crates/engine/src/cmd/track.rs:463` |
| 6 | **Fixed:** after Process, the clip's effects were baked in and still active, so heard twice; Process now takes off the effects it rendered (bypassed ones, not rendered, stay) | `cmd/audiosuite.rs:118` |
| 7 | **Fixed:** Elastic Properties on a non-elastic track changed speed but not length, so the tail was cut or ran into silence; the length now follows (Remove Warp restores it) | `cmd/clip.rs:202` |
| 8 | **Fixed:** Clip Gain › Bypass erased the gain and its envelope; it is now a bypass toggle (checked in the menu) that keeps them | `cmd/clip.rs:83` |
| 9 | **Fixed:** Reconnect opened the new device before closing the old one (ASIO allows one); it now closes first, as applying new settings already did (untested on hardware) | `crates/ui-egui/src/lib.rs:2080` |
| 10 | **Partly fixed:** Solo modes AFL, PFL, Latch and Momentary did nothing. AFL and PFL now listen on the control room's listen bus (section 3, E); Latch and Momentary still behave as latch | `cmd/options.rs`, `crates/mix/src/monitor.rs` |
| 11 | **Fixed:** a missing configured device fell back to the system default with only a log line; the status line now says so once, and the Playback Engine window while it holds | `crates/playback/src/devices.rs:355` |
| 12 | **Fixed:** Clip effects allocated on the audio thread the first time a clip played. They, built-in inserts and instruments are now made and prepared on the UI thread (`BuiltinPrep`, `crates/mix/src/prepare.rs`) and handed over with the session, as hosted plug-ins were; the audio thread's `sync` only seats them and retires what it replaces. Left on the audio thread: sizing a new track's strip buffers and delay lines, and a drum kit's per-pad effects. Tests: `mix` `sync_on_the_render_path_makes_no_builtin_module` (also checks the sound is identical), `playback` `the_audio_thread_creates_no_builtin_module` | `crates/mix/src/prepare.rs`; `MixEngine::adopt_prepared`; `Side::send_session` in `crates/playback/src/lib.rs` |

## Suggested order of work

Hours are agent-hours on this repo's calibration ([`target-app-parity.md`](../target-app-parity.md)); all are judgements.

**1. Fix what is wrong (≈ 10–16 h).** Bugs 1–9 and 11. Small, and each is in a path people use daily.

**2. What all four have (≈ 75–120 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Declick: short ramps on stop, start, locate and loop wrap; automatic edge fades on clips | 6–10 | **Done** (2026-11-16) |
| Loop range and loop on/off change while playing | 3–5 | **Done** (2026-11-16) |
| Scrub audio (Scrubber tool, ruler drag) and held rewind / fast forward | 10–16 | **Done** (2026-11-16): held rewind / fast forward; the Scrubber and a Scrubber drag on the ruler play what they pass through the track's built-in inserts and fader (`ui-egui/src/scrub.rs`). Left: scrubbing while playing, and third-party inserts (they would need the audio thread's instances) |
| A band-limited resampler for device-rate mismatch, speed changes and stretched clips | 6–10 | **Done** (2026-11-16): `playback/src/resample.rs` for the device-rate and speed path (16 frames of added delay off the direct path); varispeed clips in the mixer read through the sampler's reader (`dsp::read_band_limited`: a cubic at or below the file's speed, a Kaiser-windowed sinc above it), no longer linear |
| Preview: files in the browser and import dialog, clips in the Clip List; at session tempo and in sync | 14–22 | **Partly done** (2026-11-16): Workspace files and Clip List clips at a preview level (`ui-egui/src/audition.rs`); left: the import dialog, session tempo, in sync with playback |
| Real-time stretch that follows tempo: a tick timebase that works for audio, at least two algorithms, draggable warp markers | 30–45 | **Partly done**: tick timebase, four algorithms (offline), draggable warp markers (offline render on release). Real-time stretch: designed (below), not built |
| Typing a position into the counter; pause | 2–4 | **Done** (2026-11-16) |
| Streaming for AIFF and for converted or compressed files (decode once to a cache file) | 6–10 | **Done** (2026-11-16): `engine/src/decode_cache.rs`; shrinks G-06 |

**Status of tier 2 (2026-11-16).** Done: declick; loop changes while playing; held rewind and
fast forward; a band-limited resampler for other device rates and speeds; Pause and typing a
position into the counter; decode once, then stream. Partly: scrub (the Scrubber plays the clips it
passes; not through the mixer, not on the ruler, not while playing); preview (Workspace files and
Clip List clips, at a preview level; not in the import dialog, not at session tempo or in sync);
tempo-following (audio on tick tracks moves and, on Elastic tracks, is rendered to the new length,
with the algorithm choice working; no real-time stretch, no draggable warp markers).

**Real-time stretch: the design (third round).** Not built yet; what a build should do.
(1) *Where it runs.* In the clip read (`render_clip`), never in strip processing. A stretched or
warped clip that has no current render reads its original file through a stateless granular
reader: output sample `r` of the clip is two grains half a grain apart, each centred on the
file position the warp map gives `r` and read at the file's own speed, faded sin² so they sum
to one. Every sample depends only on `r`, so a locate, a loop wrap or a new session snapshot
plays exactly what playing through would, with no state on the audio thread and nothing
allocated (the clip transpose reader, `shifted_at`, is this reader with the grains read at the
pitch ratio). The offline WSOLA render stays the high-quality result: it is made on a worker
when the edit settles (a tempo edit, a marker drop, a stretch trim) and swapped in with the
session, and the clip reads the render from then on. (2) *Latency.* None: grains read ahead in
the file, centred on the nominal position, so nothing is reported to delay compensation.
(3) *Clips with no stretch* take the run-based path they take now and are bit-identical.
(4) *Quality.* Plain grains are phasier than WSOLA on sustained material; a later step can
align each grain to the previous one by a short correlation search over the file (still
stateless, as the search only reads the file). Left: building (1) and the worker that replaces
it with the render.

**3. What most have (≈ 45–75 h).** Varispeed in the interface with a pitch-preserving option
(Logic) and shuttle (Pro Tools, Logic, Cubase), 6–10 (**done** 2026-11-16: varispeed, tape style or
keeping the pitch; shuttle forwards through the mixer and backwards through the preview voice); per-clip transpose and non-destructive
reverse, 8–12; clip effects that take any module, 8–12; automatic crossfades on overlap and layered
editing, 8–12; skipping plug-ins on silent tracks, 5–8; realtime safety (plug-ins and Clip Effects are made off the audio thread now; build strips off the audio
thread) and worker thread priority, 8–14; automatic device recovery, 3–6.

**Where FreeDAWm is already level or ahead:** gapless looping, uninterrupted playback while
editing, the breadth of delay compensation, formats read, clip gain envelopes with conversion to
automation, freeze up to a chosen module, built-in vocal tuning heard live, and the browser build.

## Sources

Versions were checked on 2026-11-15:
[Pro Tools 2026.4](https://www.avid.com/resource-center/whats-new-in-pro-tools-20264),
[Fender Studio Pro 8.2](https://www.fender.com/newsroom/273556-fender-studio-pro-8-2-expands-mixing-and-creative-production-tools/),
[Logic Pro 13.0](https://macrumors.com/2026/11/05/apples-creator-studio-updates-start-rolling-out),
[Cubase 15](https://www.musicradar.com/music-tech/steinberg-unveils-cubase-15-a-major-leap-forward-for-its-flagship-daw-with-new-instruments-effects-modulators-and-ai-powered-stem-separation).
Feature entries for the four come from their manuals as known, not from a side-by-side run; treat a
◐ as "check before relying on it".

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | minor | layered overlapping clips and crossfade on overlap (section 4, D) |
| 2026-11-16 | minor | warp markers you drag (section 4) |
| 2026-11-16 | minor | varispeed clips read through the band-limited reader; the real-time stretch design |
| 2026-11-16 | minor | tier 3: varispeed that keeps the pitch; shuttle forwards and backwards |
| 2026-11-16 | minor | tier 3: per-clip transpose and reverse, real-time clip loop |
| 2026-11-16 | minor | tier 2: scrub through the track's strip, and on the ruler |
| 2026-11-16 | minor | Bug 10 partly fixed: AFL and PFL solo listen; a monitor section |
| 2026-11-16 | minor | Bug 12 fixed: plug-ins and Clip Effects are made off the audio thread |
| 2026-11-16 | minor | status of tier 2 |
| 2026-11-16 | minor | tier 2: files that cannot stream are decoded once and then streamed |
| 2026-11-16 | minor | audio on tick tracks follows tempo edits (offline renders) |
| 2026-11-16 | minor | tier 3: varispeed |
| 2026-11-16 | minor | tier 2: scrub audio with the Scrubber |
| 2026-11-16 | minor | tier 2: auditioning files in the Workspace and clips in the Clip List |
| 2026-11-16 | minor | tier 2: Pause, typing a position into the counter, held rewind and fast forward |
| 2026-11-16 | minor | tier 2: a band-limited resampler for other device rates and speeds |
| 2026-11-16 | minor | the loop changes while playing |
| 2026-11-16 | minor | declick: transport ramps, loop-edge ramps and automatic edge fades |
| 2026-11-16 | minor | bug 11 fixed: a missing chosen device is told to the user |
| 2026-11-16 | minor | bug 9 fixed: Reconnect closes the old device first |
| 2026-11-16 | minor | bug 8 fixed: Clip Gain Bypass is a real bypass |
| 2026-11-16 | minor | bug 7 fixed: varispeed Elastic Properties changes the clip length |
| 2026-11-16 | minor | bug 6 fixed: Process takes off the clip effects it baked |
| 2026-11-16 | minor | bugs 4 and 5 fixed: stereo clips on mono tracks, Split into Mono |
| 2026-11-16 | minor | bug 3 fixed: Play Selection with an insertion point |
| 2026-11-16 | minor | bug 2 fixed: Play from a position and Play Timeline |
| 2026-11-16 | minor | bug 1 fixed: a loop of 64 samples or less plays once |
| 2026-11-15 | major | First version: playing audio against Pro Tools, Studio Pro, Logic Pro and Cubase |
