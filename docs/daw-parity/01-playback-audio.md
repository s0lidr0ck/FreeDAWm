# Four-DAW parity, section 1: playing audio

> **Last reviewed:** 2026-11-15 · **Last updated:** 2026-11-17 · **Change:** minor (round three, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
| Play start separate from the selection | ● | ● | ● | ● | **Works**: Options › More › Dynamic Transport: Play starts at the play start marker (a green triangle on the main timebase ruler, moved by a click there or Edit › Selection › Set Play Start), whatever the edit selection; off, Play starts from the selection as before |
| Return to start on stop, or stay | ● | ● | ● | ● | **Works** (Insertion Follows Playback) |
| Audition around an edit (play up to / from the start or end) | ● | ◐ | ◐ | ● | **Works**: Edit › Selection › Audition › Play to Start, Play from Start, Play to End, Play from End: the pre-roll up to the edge or the post-roll after it (2 s where none is set), once, whatever Loop Playback says (`transport.play_to_start` …) |
| Follow modes: none, page, continuous, centred | ● | ● | ● | ● | **Works**: five modes, pauses when you scroll by hand |
| Keep playing while you edit | ● | ● | ● | ● | **Works**: a new session snapshot is swapped in without a gap |
| Click during playback | ● | ● | ● | ● | **Works**: synthesized, accent on beat 1; no sample sounds, no separate click output |
| Chase external timecode or clock | ● | ● | ● | ● | **Not planned here**: following another machine's MTC, LTC or MIDI clock needs a second machine or hardware sending it, to develop and tune the lock against; MIDI clock and MTC go out (section 6) |

## B. Looping

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Gapless loop | ● | ● | ● | ● | **Works**: wraps inside the audio callback on the exact sample, disk cache pre-reads the loop start |
| Loop range separate from the selection | ● | ● | ● | ● | **Works**: with Dynamic Transport a range selection no longer sets the loop; a drag along the main timebase ruler does, as do Edit › Selection › Set Loop to Selection and the loop strip's end drags, and Loop Playback loops it (from the play start marker when it is inside). Without it the sticky loop follows the selection as before |
| Move or resize the loop while playing | ● | ● | ● | ● | **Works**: a new range selection or loop-end drag moves the loop under way, without a jump |
| Turn looping on or off while playing | ● | ● | ● | ● | **Works**: off plays on; on loops the loop range (a loop behind the playhead waits for the next Play or locate); not while recording |
| Loop follows the selection | ● | ● | ● | ● | **Works** (it is the default behaviour) |
| Skip a range during playback | ○ | ○ | ● | ● | **Works**: Edit › Selection › Skip Range marks the selection (drawn red on the main timebase ruler; touching skips join) and playback jumps over it as a declicked locate; Clear Skip Ranges takes them off. A play started inside one plays it; recording plays through (`transport.skip_add`, `transport.skip_clear`) |
| Smooth loop join (no click at the wrap) | ● | ● | ● | ● | **Works**: clips running across the loop's edges ramp out and back in over 1 ms (a short dip, no crossfade); plug-in tails run on |

## C. Speed, scrub and shuttle

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Scrub audio by dragging | ● | ◐ | ● | ● | **Works**: the Scrubber plays the track it drags over (or the selected tracks), and a Scrubber drag along a timebase ruler every audible track, forwards and backwards at the drag's speed, through the track's strip: clips, the inserts when all are built in (started afresh each frame, so no tails carry), the fader and mute. Not while playing; a strip with a third-party insert scrubs its clips and fader only |
| Shuttle / jog at variable speed | ● | ○ | ● | ● | **Works**: Options › More › Shuttle (Back, Stop, Forward; bind them to J, K and L in the key editor): forward plays at 1×, each press doubling to 4× (tape style, or with the pitch kept); back plays backwards from the playhead at 1× to 8×, heard frame by frame through the preview voice (the clips, as the Scrubber hears them); `transport.shuttle {speed}` sets a speed |
| Half-speed playback | ● | ○ | ● | ○ | **Works** (tape style, pitch drops) |
| Varispeed: any speed, with or without pitch change | ○ | ○ | ● | ○ | **Works**: Options › More › Varispeed… plays at 0.1–4× until set back to 1 (not during a take); Varispeed Keeps Pitch shifts the output back by 1 ÷ the speed (a two-tap delay-line shifter on the device output, about 21 ms later), otherwise the pitch moves as tape does |
| Reverse playback of the timeline | ◐ | ○ | ◐ | ◐ | **Partial**: Shuttle Back plays backwards through each track's strip as the Scrubber hears it (clips, built-in inserts, fader, mute). Left: sends, busses, the master and third-party inserts; designed below (Reverse playback through the mixer: the player renders reversed blocks), not built |

Studio Pro's Listen tool auditions a track from the pointer; it is not a true scrub. Logic's
Varispeed is the only whole-project speed control among the four.

## D. Audio engine

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Choose device, sample rate and buffer | ● | ● | ● | ● | **Works** (Playback Engine window) |
| ASIO on Windows | ● | ● | n/a | ● | **Not planned here**: Steinberg's licensed SDK cannot be shipped; a personal build has it (`cargo xtask build-asio`) |
| Aggregate or separate input and output devices | ● | ● | ● | ◐ | **Partial**: one output device; a different input device is drift-corrected. Left: several output devices at once, not planned here: it needs two output devices to develop and to tune the drift correction between them |
| Recover when the device disappears | ● | ● | ● | ● | **Works**: when the output device goes away it is looked for every 2 s (on a worker thread) and the app reconnects by itself when it is back (`ui-egui/src/device_return.rs`); ASIO is never probed while a driver may be loaded, so it keeps the manual Reconnect button. Not tried with a real unplug |
| Session rate differs from the device rate | n/a | ● | ● | n/a | **Works**: a 32-tap windowed-sinc resampler whose cut-off follows the speed (no files are converted) |
| Multi-core rendering | ● | ● | ● | ● | **Partial**: tracks render in parallel by level on the worker pool; aux, folder and master strips are serial; worker threads have no realtime priority. Left: busses in parallel by level, designed below (Busses on the worker pool), not built; realtime priority for the workers needs a new dependency (`audio_thread_priority` or `thread-priority`) or `unsafe` outside the FFI crates: not planned here until the owner approves one |
| Delay compensation | ● | ● | ● | ● | **Works**: inserts, pre-fader sends, aux returns, masters, sidechain keys, hardware outputs; up to about 22 s |
| Large-buffer playback path with a low-latency live path | ● | ● | ● | ● | **Partial**: Low Latency Monitoring bypasses latent inserts on live paths; no separate large playback buffer. Left: rendering the playback-only strips ahead at a larger buffer and adding them to the live strips each callback; designed below (Large playback buffer with a small live path; the same item as section 2's small-buffer row), not built: the largest engine item, 15–22 h |
| Stream audio from disk with a cache | ● | ● | ● | ● | **Works** for WAV at the session rate (cache of a quarter of RAM, 4 s read-ahead) |
| Skip plug-ins on silent tracks | ● | ● | ● | ● | **Works**: Options › More › Suspend Plug-ins on Silence (off by default, so sessions sound as they did): an insert whose input and output have both been below −100 dBFS for a second is not run while its input stays silent, and the first sound runs it again in the same block; keyed modules and modules that send MIDI always run, and the strip's delay compensation is unchanged. A suspended module's state (a compressor's release) holds where it was |
| CPU meter and dropout counter | ● | ● | ● | ● | **Works**; no per-plug-in or per-core breakdown |
| No click on stop, locate and start | ● | ● | ● | ● | **Works**: 3 ms output ramps on start, stop, locate, a Play while playing and a known end of play (not while recording, where the take's edges stay exact); plug-ins are still reset after the ramp, so reverb tails stop |
| Realtime-safe audio thread | ● | ● | ● | ● | **Works**: plug-ins (built-in and hosted), Clip Effects modules and a new track's strip (its audio buffers, delay-compensation and key lines, MIDI rings) and a new bus's buffers are made on the UI thread by `BuiltinPrep` and handed over with the session; when the engine's tables of strips, meters and busses would grow, larger empty ones come along; a removed track's strip and outgrown tables go back to be freed off the audio thread (`RetiredStrips`). Fixed on the way: a strip's pre-fader and output buffers were made with only the last channel reserved, so the others grew while rendering. Lines are reserved for a mix delay up to a quarter second; a larger one still grows them in the render's sync, as before. The small lists `sync` rebuilds each time the session's structure changes are still made there |
| Change the session sample rate later | ◐ | ● | ● | ● | **Missing**: set at New Session only; designed below (Changing the session rate), not built |
| Monitor section (dim, mono, cue mixes) | ◐ | ◐ | ◐ | ● | **Works**: Window › Control Room (section 3, E) |
| Runs in a browser | ○ | ○ | ○ | ○ | **Works**, single-threaded |

## E. Media and formats

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| WAV, BWF, RF64, AIFF | ● | ● | ● | ● | **Works** |
| FLAC, MP3, AAC/M4A, Ogg, Opus, CAF | ◐ | ● | ● | ● | **Works** (more than Pro Tools reads) |
| Files at another sample rate | ● | ● | ● | ● | **Works**: converted with a high-quality sinc at import or open |
| Mixed rates without converting files | ○ | ● | ○ | ○ | **Missing** (converted copies only; the converted copy sounds the same). Designed below (Mixed rates without converting): files kept at their own rate and read through the varispeed reader, positions staying in session samples; 8–12 h, not built: it saves disk space and import time only, so it waits for the owner's call |
| Compressed and long files without filling memory | ● | ● | ● | ● | **Works** from the second open: a file that is not WAV at the session rate is decoded into memory once and kept as a float WAV in the project's `Decoded` folder, which later opens stream; a session never saved still decodes every time |
| Waveform cache | ● | ● | ● | ● | **Works** |

## F. How a clip is rendered

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip gain | ● | ● | ● | ● | **Works**, −144 to +36 dB |
| Gain envelope inside the clip | ● | ● | ○ | ● | **Works**, with render and conversion to and from volume automation |
| Fades drawn in real time, several shapes | ● | ● | ● | ● | **Works**: five shapes plus a continuous curve |
| Crossfades | ● | ● | ● | ● | **Works**: Edit › Fades › Create… gives each side its own shape and bend (`b6812b6`, section 4, F) |
| Automatic crossfade when clips overlap | ○ | ● | ◐ | ● | **Works**: Options › Overlap › Crossfade (section 4, D) |
| Automatic short fades at clip edges | ● | ◐ | ◐ | ● | **Works**: 1 ms on edges without a fade that face silence (Options › Automatic Edge Fades, on by default, length by command); butted clips play straight through; edits take the material unfaded |
| Per-clip effects | ● | ● | ○ | ● | **Partial**: a fixed EQ → compressor → gain chain; the others take any plug-in. Left: a chain of any module per clip (instances per clip made in `BuiltinPrep` as the EQ and compressor are now, latency per clip, a drawer per clip); not reached |
| Non-destructive offline processing stack | ○ | ◐ | ○ | ● | **Missing** here: Process writes a new file. Not built in this section: it is section 4's process history (G), designed there, and is tracked on that row |
| Mute a clip | ● | ● | ● | ● | **Works** |
| Reverse a clip without rewriting the file | ○ | ◐ | ● | ◐ | **Works**: Clip › Real-Time › Reverse in Place (section 4, E) |
| Per-clip transpose and fine tune | ◐ | ● | ● | ● | **Works**: Clip › Real-Time › Transpose…, heard at once in real time, then (in the app) by a background Pitch Shift render (section 4, E) |
| Loop a clip by dragging its edge | ● | ○ | ● | ○ | **Works**: the loop trim (Shift at the end trim) repeats the material in real time |
| Overlapping clips: top one plays, the lower one returns when uncovered | ● | ● | ● | ● | **Works** with Options › Overlap › Layer: the clip read gives way under a higher layer (the top clip's fades cross over to it); Overwrite, the default, still cuts the overlap away |
| Takes and swipe comping | ● | ● | ● | ● | **Works**; a take cannot be auditioned without making it active |
| Stereo clip on a mono track | ● | ● | ● | ● | **Works**: folded to mono at equal level |
| Split a stereo track into mono | ● | ● | ● | ● | **Works**: each new track plays its own channel |
| Freeze, commit, render in place | ● | ● | ● | ● | **Works**: freeze up to a chosen module; third-party plug-ins stay loaded |
| Built-in pitch correction heard live | ◐ | ● | ● | ● | **Works** (Tune Vocal), except on stretched clips |
| ARA editors | ● | ● | ● | ● | **Not planned here**: needs Celemony's ARA SDK, which is not in the tree (section 4, J) |

## G. Tempo and time-stretch

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Real-time, pitch-preserving stretch | ● | ● | ● | ● | **Works** in the app: a pitch-preserving stretch (Elastic Properties, Conform, TCE, a tempo edit on an Elastic tick track, a warp-marker move) is heard at once through a granular reader in the clip read (no latency) while the offline WSOLA render is made in the background, which then replaces it; grainier than the render on sustained notes. The CLI and scripts still render in place |
| Audio follows tempo changes | ● | ● | ● | ● | **Works**: on a tick-timebase track a tempo edit moves clips to their bars and, on an Elastic track, gives them the new length, heard at once through the real-time stretcher until the render is in |
| Choice of stretch algorithm | ● | ● | ● | ● | **Works**: polyphonic, rhythmic, monophonic and X-Form each splice differently in the background render, which replaces the one real-time grain stretcher heard while it is made |
| Warp markers you can drag | ● | ● | ● | ● | **Works** (offline render on release): the Warp track view; section 4, I |
| Quantize audio to the grid | ● | ● | ● | ● | **Works**: Clip › Quantize Audio… moves clips by their sync points with a strength and swing; Straighten conforms whole tracks (section 4, I) |
| Tempo detection | ● | ● | ● | ● | **Works**: at import a loop's own tempo is read (its `acid` chunk or name), and a file of 30 s or less that says none has its tempo detected (70–180 bpm) and recorded on the clip for Conform to Tempo, never stretched on a guess (with Imported Loops Follow Tempo and Key on); longer audio is detected by command (Detect Tempo: constant, follow and smooth) |
| Conform a clip to the session tempo | ● | ● | ● | ● | **Works**: varispeed on a Varispeed or plain track, a pitch-preserving stretch on an Elastic one (section 4, bug 12) |
| Loops that match tempo when dropped in | ● | ● | ● | ● | **Works**: a file that says its tempo (a WAV `acid` chunk, or "120bpm" in its name; `audio-io/src/loop_info.rs`) is stretched to the session tempo with its pitch kept as it is imported (Options › More › Imported Loops Follow Tempo and Key, on in new sessions; sessions saved before keep importing as they did). Its tempo is recorded on the clip either way, so Conform to Tempo uses it later. Nothing is guessed from audio that says no tempo |
| Loops that match key | ○ | ◐ | ● | ◐ | **Works**: with the same option, a loop that says its key (the `acid` chunk's root note, or a key word such as "Am" beside a tempo in its name) is transposed in real time (Clip › Real-Time › Transpose) to the session's key at that point, by at most a tritone; a minor loop goes to the relative minor of a major key and the other way. Only when the session sets a key |

## H. Auditioning

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Preview a file before importing | ● | ● | ● | ● | **Works** in the Workspace window: a click plays the file (read on a worker thread), again stops it; not in the import dialog |
| Preview in time with the session tempo | ● | ● | ● | ● | **Works** for files that say their tempo: the Workspace's In tempo box (on by default) stretches the file, pitch kept, to the tempo at the playhead on the audition worker |
| Preview while the session plays, in sync | ● | ● | ● | ● | **Works**: the Workspace's In sync box (on by default): while the session plays, a file repeats in time with it from the start of the bar the playhead was in, each output frame read from the playhead, so locates, loop wraps and speed changes keep it in step (`PreviewSync` in `playback`); stopped, it plays once from its start |
| Audition a clip from the clip list or pool | ● | ● | ● | ● | **Works**: Alt-click or Audition in the Clip List, with its clip gain |
| Preview volume and output | ● | ● | ● | ● | **Works**: a preview level and an output pair (Workspace window), used by file and Clip List auditions |

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
| Scrub audio (Scrubber tool, ruler drag) and held rewind / fast forward | 10–16 | **Done** (2026-11-16): held rewind / fast forward; the Scrubber and a Scrubber drag on the ruler play what they pass through the track's built-in inserts and fader (`ui-egui/src/scrub.rs`). Left: scrubbing while playing, third-party inserts (they would need the audio thread's instances), and insert state carried from one frame to the next (the strip render is built afresh per call; keeping it needs a strip kept across frames, not a small change) |
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
stateless, as the search only reads the file). Built so far: the stretcher itself
(`dsp::live_stretch`, fourth round) and the clip read: a command that would render a
pitch-preserving stretch can instead reserve the render's source and record how to hear it
meanwhile (`Session::live_stretch`), the mixer reading the original through the stretcher until
the render's audio is in (`engine::stretch_jobs`: the job, `install`, renders kept for undo and
redo, a save renders what is still live). The app's worker (`ui-egui/src/stretch_worker.rs`) makes the render on a thread and swaps it in; the
app turns deferral on (not the web build, which has no threads, nor the CLI). Left: grain
alignment (point 4); hearing a warp marker while it is dragged (it is heard from the release).

**Changing the session rate: the design (round three, not built).** A command
(`session.set_sample_rate {rate}`, Setup › Session) converts the open session in one undo step.
(1) *Sources*: every pool source is resampled with the import's high-quality sinc
(`audio_io::Resampler`) on a worker with the open-progress window, written to the project's
`Audio Files` as new files (the old ones stay for undo; Compact removes them later). (2)
*Positions*: everything stored in samples is scaled by new ÷ old and rounded: clip start,
length, source offset and fades, warp markers, gain envelopes, automation points, markers and
memory locations, the selection, loop, skip ranges, pre/post-roll, the tempo map's origin and
the stretch renders' keys (those renders are made again). Tick-timebase material keeps its ticks.
(3) *Plug-ins* are prepared again at the new rate (the sync after the swap). (4) Tests: a session
at 44.1 kHz converted to 48 kHz renders the same 10 s within −60 dB of the original resampled,
and every stored position lands within one sample of its time. The risk is completeness: a
position field missed plays in the wrong place, so the step that scales them must be one
function over `Session` with a test that fails when a new `Samples` field is added without it.

**Mixed rates without converting: the design (round three, not built).** Studio Pro alone
plays files at their own rate in a session at another. (1) *Kept as is.* With "Keep files at
their own rate" (Setup › Session, off by default) an import at another rate is stored as it is,
with no converted copy; `SourceAudio::sample_rate` already records the file's rate. (2) *Positions
stay in session samples.* A clip's start, length and source offset stay in samples at the session
rate, so every edit, the waveform's horizontal scale and the Clip List are unchanged; only the
reads convert. (3) *The read.* The clip reader turns a timeline sample into the file's frame by
the ratio file ÷ session and reads through the band-limited varispeed reader the clips already
use for real-time transpose and varispeed (fractional positions, a kernel whose cut-off follows
the ratio), so a 44.1 kHz file in a 48 kHz session plays as varispeed at 0.91875 with no pitch
change; the disk streamer reads ahead in file frames. (4) *What converts.* Everything that writes
audio (Process, Consolidate, bounce, freeze) writes at the session rate, as now; the waveform
overview is read at the file's rate and scaled. (5) *Tests.* A 44.1 kHz file played in a
48 kHz session renders within −60 dB of the same file converted at import, and a clip edited
(trimmed, split, moved) plays the same audio at the same times either way. (6) *Size and worth.*
8–12 h; the converted copy sounds the same, so this saves disk space and import time only. Not
built: the owner's call whether it is worth it.

**Busses on the worker pool: the design (round three, not built).** Tracks already render in
parallel by the planner's levels (`sidechain::resolve` gives every strip a level: a strip waits
only for strips of lower levels: its key sources and the busses it reads). Aux, folder and master
strips are run afterwards one by one because each reads a bus that the earlier strips summed into
(`MixEngine::busses`, written by every source). (1) *Order.* The planner already knows which
strips feed which bus; an aux's level becomes one more than the highest level of the strips that
feed its input bus, so all strips of one level read only busses that are complete. (2) *Summing.*
The worker pool (the rayon pool the tracks already use: no new dependency)
renders a level's strips into their own buffers; the summing into busses and the main stays on
the audio thread between levels, in strip order, so the sum (and every bounce) is bit-identical
to the serial render. (3) *Tests.* A session of three nested aux levels renders sample-identical
with one worker and with four; a counter shows aux strips of one level ran on more than one
worker. (4) *Not in it.* Realtime priority for the workers: it needs the `audio_thread_priority`
or `thread-priority` crate (or `unsafe` outside the FFI crates), which the owner has not approved.

**Reverse playback through the mixer: the design (round three, not built).** Shuttle Back plays
each track's strip backwards on the UI side (the Scrubber's voice). To take sends, busses, the
master and hosted inserts along, the audio thread's player renders the block backwards instead:
(1) the clip read already takes a negative speed for varispeed; a block at speed −1 reads clips
from `pos` down, reversed per block; (2) automation is read at the reversed positions (the
lanes are read per position already); (3) the strips, busses and master process the reversed
audio forwards in time (effects ring "into" the past, as tape reversed through a desk does,
which is what Pro Tools and Logic do); (4) the transport moves backwards, and Stop, loop and
locate treat the block's end as its start. The Scrubber's own reverse voice is removed then.
Tests: a reversed render of a dry impulse track is the forward render reversed sample for
sample; a reverb send's tail comes before the impulse. About 8–12 h.

**Large playback buffer with a small live path: the design (round three, not built; shared with
section 2's "small buffer for live tracks, large for playback").** (1) *Split.* Each block, the
planner marks the strips that hear live input (record-armed, input monitoring, live MIDI, a
track whose key comes from one of those) and everything they feed, down to the master: the
*live* set; the rest is *playback*. (2) *Ahead.* A worker renders the playback set ahead of the
transport, `N` device blocks at a time (a "playback buffer" setting, 1024–8192 frames), into a
ring per bus input and per main contribution, timestamped; it takes session updates as the audio
thread does now and drops what it rendered ahead when a locate, an edit to a playback strip or a
loop wrap invalidates it (the ring is refilled from the new position: an edit is heard after one
large buffer, as in Pro Tools' and Cubase's large-buffer modes). (3) *Each callback.* The audio
thread renders only the live set at the device's small block and adds the playback set's samples
for the same timeline position from the rings, at the busses and the main. Delay compensation is
unchanged: both sets use the same per-strip latencies. (4) *Plug-ins.* A strip moving between
sets (armed, disarmed) is rendered by one side only from a block boundary, with its instances
handed over (they are never run by two threads). (5) *Tests.* Playback with the split on is
sample-identical to the serial render for a session without live input; with one armed track,
its input reaches the output within the small block while the rest of the session is rendered
ahead. (6) *Risk.* It is the largest engine item (15–22 h): instance hand-over between threads
and invalidation on edits are where it can go wrong, so it should be built behind an option,
off by default, and compared sample for sample with the present render in the tests.

**3. What most have (≈ 45–75 h).** Varispeed in the interface with a pitch-preserving option
(Logic) and shuttle (Pro Tools, Logic, Cubase), 6–10 (**done** 2026-11-16: varispeed, tape style or
keeping the pitch; shuttle forwards through the mixer and backwards through the preview voice); per-clip transpose and non-destructive
reverse, 8–12; clip effects that take any module, 8–12; automatic crossfades on overlap and layered
editing, 8–12; skipping plug-ins on silent tracks, 5–8; realtime safety (plug-ins, Clip Effects and new strips are made off the audio thread now) and worker thread priority, 8–14; automatic device recovery, 3–6.

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
| 2026-11-17 | minor | Round three: a design for playing files at their own rate (mixed rates) |
| 2026-11-17 | minor | Round three: designs for busses on the worker pool, reverse playback through the mixer and the large playback buffer; the processing stack row points at section 4 |
| 2026-11-17 | minor | Round three: a new track's strip and a new bus's buffers are made off the audio thread |
| 2026-11-17 | minor | Round three: a short import's tempo is detected and recorded |
| 2026-11-17 | minor | Round three: skip ranges; the open rows say what is left and why; a design for changing the session rate |
| 2026-11-17 | minor | Round three: audition around an edit, Dynamic Transport's play start and separate loop, automatic device return, suspend plug-ins on silence, loops that follow tempo and key at import, preview in tempo, in sync and on a chosen output |
| 2026-11-17 | minor | Round three reconciliation: crossfade shapes, audio quantize, conform and the stretch algorithms were already built; ASIO and ARA not planned here |
| 2026-11-16 | minor | clip transpose: a background high-quality render replaces the live shifter |
| 2026-11-16 | minor | the real-time stretch is heard at once in the app; a background worker swaps in the render |
| 2026-11-16 | minor | the real-time stretcher in the clip read, behind the Elastic and warp paths |
| 2026-11-16 | minor | the real-time stretcher (not wired to clips yet) |
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
