# Four-DAW parity, section 2: recording audio

> **Last reviewed:** 2026-11-15 · **Last updated:** 2026-11-16 · **Change:** minor (take bit depth, file type and BWF timestamp) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). They are named here only to compare features; FreeDAWm is not
> affiliated with or endorsed by any of them. See [NOTICE](../../NOTICE).

The second of seven sections ([section 1: playing audio](01-playback-audio.md)).

**Scope of this section:** everything between arming a track and having a take on the timeline:
arming and inputs, starting and stopping, punching, loop recording and takes, monitoring, latency,
the capture path and its files, and safety. MIDI recording is section 5; cue mixes and the monitor
section are counted here only as far as a performer needs them.

## How this was measured

- **FreeDAWm:** read from the code at d9ba45b (0.9.11 plus section 1), not from the docs. Options
  that store a value nothing reads are marked **flag only**. The headline claims were re-read by
  hand. Nothing here was exercised on an audio interface: that is the owner's hardware pass
  ([`hardware-checklist.md`](../hardware-checklist.md)).
- **The four DAWs:** from their published manuals and release notes, as known up to the versions
  above, for each product's top edition. Nothing was run side by side.

**Legend.** Other DAWs: ● has it · ◐ partly or by another route · ○ does not.
FreeDAWm: **Works** · **Partial** · **Flag only** · **Missing**.

## Summary

| Area | FreeDAWm today | Distance from the four |
|---|---|---|
| A. Arming and inputs | Arming works; inputs are mono or stereo hardware, or a bus | Medium: wrong channels with more than two (bug 10) |
| B. Starting and stopping | Record, count-off, pre-roll, punch in on the fly | Small: no discard-take |
| C. Punching | Automatic punch with handles works | Medium: no punch out on the fly, no punch crossfades, four modes are labels |
| D. Loop recording and takes | Takes to layers, swipe comping | Small to medium: plain tracks lose the rest of the song; no record modes |
| E. Monitoring | Auto / On / Off through the strip, low-latency mode | Medium: no hardware direct monitoring, no dual buffer, thin cue mixes |
| F. Latency and placement | Reported, measured and manual offsets, stored per interface | **Level or ahead** |
| G. Capture and files | Lock-free capture to disk, crash recovery, RF64; BWF or AIFF at 16, 24 or 32-bit float | Small: no per-track record folders |
| H. Safety and feedback | Device and disk trouble keep the take | Small: no pre-record, no remaining-time display |
| I. Remote recording | LAN and internet recorders with clock alignment | **Ahead**: none of the four has it built in |

Three things every one of the four does and FreeDAWm does not: **record from a bus or another
track** (internal recording, and with it printing effects), **choose the recorded file's bit
depth and write a timestamp into it**, and **smooth the punch points**.

## A. Arming and inputs

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Arm one, several or all tracks | ● | ● | ● | ● | **Works**; arming is an undo step, which the four do not do |
| Arming follows the selected track | ○ | ● | ● | ● | **Missing** |
| Record-safe (a track that cannot be armed) | ● | ○ | ○ | ◐ | **Missing** |
| Arm or disarm during a take | ● | ◐ | ◐ | ● | **Partial**: allowed, but the set of recorded tracks is fixed when Record is pressed |
| Named hardware inputs, mono and stereo | ● | ● | ● | ● | **Works** (Setup › I/O) |
| Multichannel (surround) inputs | ● | ● | ● | ● | **Missing**: inputs are one or two channels wide |
| One input on several tracks | ● | ● | ● | ● | **Works** |
| A bus or another track's output as the input | ● | ● | ● | ● | **Works** for a bus: the track hears and records it as the mix renders it, with no input device needed, placed by the bus's delay compensation; a track's output is recorded by routing it (or a send) to a bus, as in Pro Tools |
| Record with effects (print) | ◐ | ● | ◐ | ● | **Works**: a per-track Record with inserts switch (input menu, `mix.record_inserts`) takes the strip after trim, polarity and inserts, before the fader, heard or not; placed by the round trip plus the inserts' latency. A bus can be printed the same way |
| Record many inputs at once | ● | ● | ● | ● | **Works** in code (up to 64 named inputs, one writer thread); never measured on a large interface |

Studio Pro and Cubase have input channels in the mixer whose inserts are recorded. Pro Tools and
Logic print effects by recording an aux's bus, which needs the row above it.

## B. Starting and stopping

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Record from the transport or one key | ● | ● | ● | ● | **Works** |
| Count-off in bars | ● | ● | ● | ● | **Works**: 1–16 bars at the tempo and meter of the start point |
| Pre-roll | ● | ● | ● | ● | **Works**: heard, not recorded, with or without a selection |
| Start recording while playing | ● | ● | ● | ● | **Works**, gapless; inside a playing loop the take splits into its passes |
| Discard the take in progress | ● | ◐ | ● | ● | **Missing** |
| Undo a take | ● | ● | ● | ● | **Works**: the file goes when no undo step, session file or autosave names it any more (on save or close) |
| Stop and start the take again in one step | ○ | ○ | ◐ | ● | **Missing** |
| Recording at another speed | ● | ○ | ● | ○ | **Missing**: refused (half-speed record is a Pro Tools and Logic feature) |

## C. Punching

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Automatic punch at a range, with pre- and post-roll | ● | ● | ● | ● | **Works**: plays in, records the range, plays out and stops |
| Audio before and after the punch kept, so the edit can be trimmed back | ● | ● | ● | ● | **Works**: the whole pass is in the file |
| Punch in on the fly | ● | ● | ● | ● | **Works** |
| Punch out on the fly and keep playing; several punches in one pass | ● | ● | ● | ● | **Missing**: the second Record press stops the transport |
| Crossfades at the punch points | ● | ● | ● | ● | **Missing**: a hard cut at both ends |
| Punch modes for many tracks, destructive recording | ● | ○ | ○ | ○ | **Flag only**: QuickPunch, TrackPunch, DestructivePunch and Destructive Record store a value nothing reads |

The four Pro Tools modes exist there for historical reasons; the others record non-destructively
with handles all the time, as FreeDAWm already does. They are worth hiding or wiring, not a gap in
what a user can do, apart from punch out on the fly.

## D. Loop recording and takes

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Loop record, one take per pass | ● | ● | ● | ● | **Works**: one continuous file, split into passes; up to 1,000 |
| Takes kept as lanes under the track | ● | ● | ● | ● | **Works** on a comping track |
| Swipe comping | ● | ● | ● | ● | **Works** |
| Choice of what a new take does to old audio (replace, keep as takes, layer) | ● | ● | ● | ● | **Partial**: a comping track keeps takes, a plain track replaces; there is no setting, and the record-mode value is unused |
| Loop record on a track without lanes | ● | ● | ● | ● | **Works**: each pass goes to a new playlist that copies the track's playlist, so only the loop differs |
| Loop record without a selection (use the loop range) | ● | ● | ● | ● | **Missing**: it falls back to a normal take |
| Take names and numbers per track | ● | ● | ● | ● | **Works**: `Vox_01`, `Vox_02`… per track, skipping files already there |
| Audition a take without making it active | ● | ● | ● | ● | **Missing** (also in section 1) |

## E. Monitoring

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Automatic input monitoring (input when stopped or recording, track on playback) | ● | ● | ● | ● | **Works**, punch-aware |
| Per-track input monitor switch | ● | ● | ● | ● | **Works**: Auto / On / Off |
| Global input-only mode | ● | ◐ | ◐ | ● | **Partial**: a command sets every armed track to On |
| Monitor through the track's inserts | ● | ● | ● | ● | **Works** |
| Low-latency mode that bypasses slow plug-ins | ● | ● | ● | ● | **Works**: only inserts with latency, only on live paths, crossfaded |
| Small buffer for live tracks, large for playback | ● | ● | ● | ● | **Missing**: one buffer for everything |
| Control the interface's own direct monitoring | ◐ | ● | ○ | ● | **Missing** |
| Existing audio muted under a take when monitoring is off | ● | ● | ● | ● | **Works**: silent within the take, heard in the pre- and post-roll |
| Cue mixes to hardware outputs | ● | ● | ◐ | ● | **Partial**: sends reach busses only; a cue mix needs a send, a bus, an aux and an output |
| Click on its own output | ● | ● | ● | ● | **Missing**: the click is summed into the main mix |
| Talkback | ◐ | ◐ | ○ | ● | **Missing** |
| Input meters before the fader | ● | ● | ● | ● | **Flag only**: meters are always post-fader, so the clip light does not show a clipped input |
| Tuner | ● | ● | ● | ● | **Missing** |

## F. Latency and placement

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Takes placed by the device's reported latency | ● | ● | ● | ● | **Works** (the driver's own figures with ASIO) |
| Manual record offset | ◐ | ● | ● | ● | **Works**, ±96,000 samples |
| Measure the round trip with a loopback | ○ | ○ | ◐ | ○ | **Works**, stored per interface, buffer and rate |
| Placement independent of plug-in delay and of low-latency mode | ● | ● | ● | ● | **Works**, tested |
| Input and output on different devices | ● | ● | ● | ◐ | **Works**: drift-corrected, by linear interpolation without a filter |
| Device rate differs from the session rate | n/a | ● | ● | n/a | **Partial**: allowed with a warning; the same unfiltered conversion, which aliases |

## G. Capture and files

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Record straight to disk without blocking the audio thread | ● | ● | ● | ● | **Works**: lock-free ring, one writer thread, lost samples counted and reported |
| No length limit, files over 4 GB | ● | ● | ● | ● | **Works**: switches to RF64 in place |
| Choose bit depth (16, 24, 32 float) | ● | ● | ● | ● | **Works**: the session's bit depth (`setup.record_format`), for local and remote takes |
| Choose file type (WAV/BWF, AIFF, others) | ● | ● | ● | ● | **Works** for WAV (BWF) and AIFF; an AIFF take loads into memory instead of streaming |
| Timestamp in the file (BWF time reference) | ● | ● | ● | ● | **Works**: a `bext` chunk with FreeDAWm as originator, the date and time, and the time reference of the take's first frame from the timecode start |
| Stereo as one interleaved file | ● | ● | ● | ● | **Works** |
| Choose where each track records | ● | ◐ | ◐ | ◐ | **Flag only**: Disk Allocation stores paths nothing reads |
| Recover a take after a crash | ● | ◐ | ◐ | ◐ | **Works**: header rewritten every second, the take placed on recovery; no fsync, so a power loss can lose the last second |

## H. Safety and feedback

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Waveform drawn while recording | ● | ● | ● | ● | **Works**, at the place the take will land |
| Record time counter | ● | ● | ● | ● | **Works** |
| Remaining record time or disk space | ● | ◐ | ◐ | ● | **Missing** |
| Audio captured before Record was pressed | ◐ | ● | ○ | ● | **Missing** |
| A lost device or a full disk keeps the take | ● | ● | ● | ● | **Works** |
| Lock against stopping a take by accident | ○ | ○ | ○ | ● | **Missing** |
| Feedback protection when monitoring through speakers | ○ | ○ | ● | ○ | **Missing** (issue #42) |

## I. Remote recording

Only FreeDAWm has this built in: other computers on the network, or over the internet, record
their own inputs when the main session records, and their takes arrive aligned to the session's
clock and are placed with the same punch and loop rules. The performer hears the host's mix as a
stream. It belongs in section 7 (unique features) and is listed here so the table above is not
read as the whole picture.

## Bugs found on the way

Lines move while section 1's work is in progress, so functions are named.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed:** a stereo track with no input chosen monitored input 1 on both sides but recorded inputs 1 and 2; both now use the device's first channels (In 1-2; In 1 twice on a mono device) | `crates/mix/src/lib.rs` (input span for no input) against `plan_takes` in `crates/engine/src/io.rs` |
| 2 | **Fixed:** an audio track whose input is a bus heard and recorded hardware inputs instead; it now hears and records the bus, and the menu no longer offers busses to instrument and MIDI tracks | the same two places; `crates/ui-egui/src/mix_window.rs` input menu |
| 3 | **Fixed:** Record pressed while a loop is playing placed the take as one straight clip running past the loop end; it now splits into that loop's passes (`record_start` in `crates/engine/src/io.rs`) | `start_recording` in `crates/ui-egui/src/lib.rs` (the plan has no pass) |
| 4 | **Fixed:** loop record on a plain track left playlists that held only the loop, so the track was silent elsewhere; each pass's playlist now copies the track's playlist from before the take | `place_take` in `crates/engine/src/io.rs` |
| 5 | **Fixed:** pre-roll with no selection recorded and placed the pre-roll, overwriting what was there; the take is now kept from the insertion point on | `start_recording`, `place_take` |
| 6 | **Fixed:** undoing a take, or deleting an armed track during one, left the file in Audio Files; such files are deleted on save (when no undo step can bring them back) or when the session closes, never while the session, its history, a session file in the project or an autosave names them (`clean_unused_takes`) | `add_recording_in` in `crates/engine/src/io.rs` |
| 7 | **Fixed:** with monitoring Off, an armed track played its old clips under the take being recorded; inside the take (the punch range) they are silent now | clip branch of the strip render in `crates/mix/src/lib.rs` |
| 8 | **Fixed:** take numbers started at the session's source count plus one; they now count each track's takes | `plan_takes` |
| 9 | **Not a bug:** Clear Clip Indicators does nothing in the engine because the clip lights are the app's meters: the app clears them after the command runs, from the menu, Alt+C, the control channel and MCP alike (tested); a headless engine has none | `track.clear_clip_indicators` in `crates/engine/src/cmd/track.rs` |
| 10 | A track with more than two channels repeats the last input channel | `track_input_channel` in `crates/model/src/mixer.rs` |

## Suggested order of work

Hours are agent-hours on this repo's calibration ([`target-app-parity.md`](../target-app-parity.md)); all are judgements.

**1. Fix what is wrong (≈ 10–16 h).** Bugs 1 and 3–9. Bug 2 and bug 10 are closed by tier 2.

**2. What all four have (≈ 60–95 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Record from a bus or a track's output (internal recording) | 10–16 | **Done** (bus input; a track's output via a bus); closed bug 2 |
| Record through input effects (an input strip, or a "record with inserts" switch) | 8–12 | **Done** (the switch) |
| Bit depth and file type for takes; BWF timestamp and originator | 5–8 | **Done** |
| Punch out on the fly, several punches per pass, crossfades at punch points | 8–12 | QuickPunch can then mean something or go |
| Record mode setting: replace, keep takes as lanes, layer; loop record from the loop range | 6–10 | Replaces the unused record-mode value; closes bug 4 by design |
| Discard take; delete files of undone takes when the session closes | 3–5 | |
| A small buffer for live tracks and a large one for playback | 15–22 | The largest item; shared with section 1's engine work |
| Cue mixes: sends to hardware outputs, click to its own output | 6–10 | Shared with the mixing section |
| Input meters before the fader, multichannel inputs, a tuner | 6–10 | Closes bug 10 |

**3. What most have (≈ 25–40 h).** Arming follows selection, 2–3; audio pre-record buffer
(Studio Pro, Cubase, and Pro Tools in QuickPunch), 5–8; remaining record time, 2–3; band-limited
conversion on the input feed (shared with section 1's resampler), 4–6; control of the interface's
direct monitoring with ASIO, 6–10, hardware needed; record-safe and record lock, 2–4; talkback,
4–6.

**Where FreeDAWm is already level or ahead:** take placement (measured round trip, stored per
interface, independent of plug-in delay), the lock-free capture path with crash recovery, the
live waveform, device and disk failure handling, swipe comping, and remote recording.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Feature entries for the four come from
their manuals as known, not from a side-by-side run; treat a ◐ as "check before relying on it".

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | minor | bug 1 fixed; bugs 3 and 5 fixed; bug 4 fixed; bug 6 fixed; bug 7 fixed; bug 8 fixed; bug 9 was not a bug; internal recording from a bus; bug 2 fixed; record with inserts; take bit depth, file type and BWF timestamp |
| 2026-11-15 | major | First version: recording audio against Pro Tools, Studio Pro, Logic Pro and Cubase |
