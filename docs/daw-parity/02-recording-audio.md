# Four-DAW parity, section 2: recording audio

> **Last reviewed:** 2026-11-15 · **Last updated:** 2026-11-17 · **Change:** minor (round three) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
| A. Arming and inputs | Arming, record safe, arm follows selection; inputs of up to 16 channels, or a bus | Small: tracks cannot join or leave a take under way |
| B. Starting and stopping | Record, count-off, pre-roll, punch in on the fly, discard and restart take, record lock | Small: no recording at another speed |
| C. Punching | Automatic and on-the-fly punches with handles and crossfades | Small: three Pro Tools modes are hidden labels |
| D. Loop recording and takes | Takes to layers, swipe comping, three record modes, a take soloed in place of the comp | Small: no stacked "layer" record mode |
| E. Monitoring | Auto / On / Off through the strip, input-only mode, low-latency mode, cue mixes, click on its own output, a tuner | Medium: no dual buffer; no hardware direct monitoring (not planned) |
| F. Latency and placement | Reported, measured and manual offsets, stored per interface | **Level or ahead** |
| G. Capture and files | Lock-free capture to disk, crash recovery, RF64; BWF or AIFF at 16, 24 or 32-bit float; per-track record folders | **Level** |
| H. Safety and feedback | Device and disk trouble keep the take, remaining record time, record lock, feedback protection | Small: no audio pre-record |
| I. Remote recording | LAN and internet recorders with clock alignment | **Ahead**: none of the four has it built in |

Three things every one of the four does and FreeDAWm does not: **record from a bus or another
track** (internal recording, and with it printing effects), **choose the recorded file's bit
depth and write a timestamp into it**, and **smooth the punch points**.

## A. Arming and inputs

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Arm one, several or all tracks | ● | ● | ● | ● | **Works**; arming is an undo step, which the four do not do |
| Arming follows the selected track | ○ | ● | ● | ● | **Works**: Options › Recording › Arm Follows Selection: selecting tracks arms them (not a record-safe one) and disarms the others; not while a take records, and clicking off every track leaves the arms alone |
| Record-safe (a track that cannot be armed) | ● | ○ | ○ | ◐ | **Works**: Record Safe on the R key's right-click menu, or Cmd-click (macOS) / Ctrl+Shift-click: a padlock on the key, the track is disarmed and cannot be armed until it is turned off (`mix.record_safe`); saved with the session |
| Arm or disarm during a take | ● | ◐ | ◐ | ● | **Partial**: allowed, but the set of recorded tracks is fixed when Record is pressed. Left: the recorder opens one file per armed track and sizes its lock-free ring for their inputs at Record; a track armed mid-take needs a file opened then and a ring with room for it, and a disarmed track's take should end at the disarm point |
| Named hardware inputs, mono and stereo | ● | ● | ● | ● | **Works** (Setup › I/O) |
| Multichannel (surround) inputs | ● | ● | ● | ● | **Works**: an input in Setup › I/O can be any format up to 16 channels (LCR, quad, 5.1, 7.1, Ambisonics…); a track of that width hears and records every channel. A mono input feeds every channel of a wider track; a wider input feeds the track's channels in order and leaves those past it silent (a stereo input on a quad track: the rears), heard and recorded alike (bug 10, fixed) |
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
| Discard the take in progress | ● | ◐ | ● | ● | **Works**: `transport.discard_take` (Cmd+Alt+.) stops and deletes the take's files; nothing is placed |
| Undo a take | ● | ● | ● | ● | **Works**: the file goes when no undo step, session file or autosave names it any more (on save or close) |
| Stop and start the take again in one step | ○ | ○ | ◐ | ● | **Works**: Restart Take (Cmd+Alt+Space, `transport.restart_take`) throws the take and its files away and records again as Record would, count-off and pre-roll included |
| Recording at another speed | ● | ○ | ● | ○ | **Missing**: refused (half-speed record is a Pro Tools and Logic feature). Left: Varispeed (section 1) plays at any speed, but the input is then read elastically and a take would lose input; recording needs the input converted into the take at the session rate and the take placed at the session's speed |

## C. Punching

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Automatic punch at a range, with pre- and post-roll | ● | ● | ● | ● | **Works**: plays in, records the range, plays out and stops |
| Audio before and after the punch kept, so the edit can be trimmed back | ● | ● | ● | ● | **Works**: the whole pass is in the file |
| Punch in on the fly | ● | ● | ● | ● | **Works** |
| Punch out on the fly and keep playing; several punches in one pass | ● | ● | ● | ● | **Works** with Options › QuickPunch: Record while recording punches out (playback goes on, the clips are heard again) and in again, any number of times; the take records throughout, so each punch has handles. Without QuickPunch Record stops, as before. MIDI takes ignore the punches |
| Crossfades at the punch points | ● | ● | ● | ● | **Works**: 10 ms, centred on each punch point, out of the take's and the old audio's handles (at most half a short punch) |
| Punch modes for many tracks, destructive recording | ● | ○ | ○ | ○ | **Partial**: QuickPunch works (above, back on the Options menu); TrackPunch, DestructivePunch and Destructive Record stay off the menus and store a value nothing reads. Left, and of little use: TrackPunch (QuickPunch armed per track) and recording over the file in place; the other three DAWs record non-destructively with handles, as FreeDAWm does |

The four Pro Tools modes exist there for historical reasons; the others record non-destructively
with handles all the time, as FreeDAWm already does. They are worth hiding or wiring, not a gap in
what a user can do, apart from punch out on the fly.

## D. Loop recording and takes

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Loop record, one take per pass | ● | ● | ● | ● | **Works**: one continuous file, split into passes; up to 1,000 |
| Takes kept as lanes under the track | ● | ● | ● | ● | **Works** on a comping track |
| Swipe comping | ● | ● | ● | ● | **Works** |
| Choice of what a new take does to old audio (replace, keep as takes, layer) | ● | ● | ● | ● | **Works**: Options › Record Mode: Replace, Keep Takes as Lanes (the track becomes a comping one) or New Playlist per Take; a comping track always keeps takes. No overlapping "layer" mode as in Cubase's stacked mix |
| Loop record on a track without lanes | ● | ● | ● | ● | **Works**: each pass goes to a new playlist that copies the track's playlist, so only the loop differs |
| Loop record without a selection (use the loop range) | ● | ● | ● | ● | **Works** |
| Take names and numbers per track | ● | ● | ● | ● | **Works**: `Vox_01`, `Vox_02`… per track, skipping files already there |
| Audition a take without making it active | ● | ● | ● | ● | **Works**: the S in a take layer's header (or Solo Take in its menu, `comp.audition`) plays that take in place of the comp, its clip effects included, until it is clicked again or the transport stops; the comp is untouched and nothing is saved or undone |

## E. Monitoring

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Automatic input monitoring (input when stopped or recording, track on playback) | ● | ● | ● | ● | **Works**, punch-aware |
| Per-track input monitor switch | ● | ● | ● | ● | **Works**: Auto / On / Off |
| Global input-only mode | ● | ◐ | ◐ | ● | **Works**: Options › Recording › Input Only Monitoring: every armed track on Auto hears its input in playback too, as in Pro Tools' Input Only mode; a track set to On or Off keeps its setting. Track › Set Record Tracks to Input Only still sets each armed track to On |
| Monitor through the track's inserts | ● | ● | ● | ● | **Works** |
| Low-latency mode that bypasses slow plug-ins | ● | ● | ● | ● | **Works**: only inserts with latency, only on live paths, crossfaded |
| Small buffer for live tracks, large for playback | ● | ● | ● | ● | **Missing**: one buffer for everything; low-latency monitoring (above) is the nearest. Left (large, with section 1's engine): render playback ahead in a large buffer and run the strips hearing live input at the device's small callback, summed at the output |
| Control the interface's own direct monitoring | ◐ | ● | ○ | ● | **Not planned here**: the only general route is ASIO's input-monitor call, which needs Steinberg's licensed SDK (a personal build only) and an interface that supports it to develop and test at all; Core Audio and ALSA have no common call for it |
| Existing audio muted under a take when monitoring is off | ● | ● | ● | ● | **Works**: silent within the take, heard in the pre- and post-roll |
| Cue mixes to hardware outputs | ● | ● | ◐ | ● | **Works**: Window › Control Room's cue mixes, fed by pre-fader sends to a hardware output, with a level and mute each (section 3) |
| Click on its own output | ● | ● | ● | ● | **Works**: Setup › Click/Countoff › Output plays the click (and the count-off) on one hardware output alone, a performer's phones, kept out of the main mix; Main mix is the default |
| Talkback | ◐ | ◐ | ○ | ● | **Works**: a talkback input into the cues, held (Talk button or F11) or latched, dimming the monitor (Control Room) |
| Input meters before the fader | ● | ● | ● | ● | **Works**: Options › More › Pre-Fader Metering makes every strip's meter, and so its clip light, read the level after the inserts and before the fader and mute (0.10.0; `pre_fader_metering_reads_the_level_before_the_fader`) |
| Tuner | ● | ● | ● | ● | **Works**: the Chromatic Tuner plug-in (Insert › Utilities), which reads the note and cents and can mute the output while tuning (0.10.0) |

## F. Latency and placement

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Takes placed by the device's reported latency | ● | ● | ● | ● | **Works** (the driver's own figures with ASIO) |
| Manual record offset | ◐ | ● | ● | ● | **Works**, ±96,000 samples |
| Measure the round trip with a loopback | ○ | ○ | ◐ | ○ | **Works**, stored per interface, buffer and rate |
| Placement independent of plug-in delay and of low-latency mode | ● | ● | ● | ● | **Works**, tested |
| Input and output on different devices | ● | ● | ● | ◐ | **Works**: drift-corrected; at one nominal rate (clock drift alone) by linear interpolation, at different rates through the band-limited resampler (next row) |
| Device rate differs from the session rate | n/a | ● | ● | n/a | **Works**: playback goes through section 1's band-limited resampler, and an input device at another rate is read into the session rate through the same 32-tap windowed sinc (cut-off below the lower Nyquist), so what is monitored and recorded does not alias; the latency placement is unchanged (`a_faster_input_is_read_band_limited`) |

## G. Capture and files

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Record straight to disk without blocking the audio thread | ● | ● | ● | ● | **Works**: lock-free ring, one writer thread, lost samples counted and reported |
| No length limit, files over 4 GB | ● | ● | ● | ● | **Works**: switches to RF64 in place |
| Choose bit depth (16, 24, 32 float) | ● | ● | ● | ● | **Works**: the session's bit depth (`setup.record_format`), for local and remote takes |
| Choose file type (WAV/BWF, AIFF, others) | ● | ● | ● | ● | **Works** for WAV (BWF) and AIFF; an AIFF take loads into memory instead of streaming |
| Timestamp in the file (BWF time reference) | ● | ● | ● | ● | **Works**: a `bext` chunk with FreeDAWm as originator, the date and time, and the time reference of the take's first frame from the timecode start |
| Stereo as one interleaved file | ● | ● | ● | ● | **Works** |
| Choose where each track records | ● | ◐ | ◐ | ◐ | **Works**: Setup › Disk Allocation… sets the folder takes record into, for the session and for any track (a relative folder is inside the session's); a take in a folder outside the session stays there when the session is saved. Undone takes there are not cleaned up as they are in `Audio Files` |
| Recover a take after a crash | ● | ◐ | ◐ | ◐ | **Works**: header rewritten every second, the take placed on recovery; no fsync, so a power loss can lose the last second |

## H. Safety and feedback

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Waveform drawn while recording | ● | ● | ● | ● | **Works**, at the place the take will land |
| Record time counter | ● | ● | ● | ● | **Works** |
| Remaining record time or disk space | ● | ◐ | ◐ | ● | **Works**: the counter reads `● REC 1:23 · 2 h 05 min left` while recording, and the Record button's tooltip gives the room for the armed tracks: the free space on the takes' disk over their channels at the session's bit depth and rate, read every 5 seconds |
| Audio captured before Record was pressed | ◐ | ● | ○ | ● | **Missing** (MIDI has Retrospective Record). Left: while tracks are armed, the output callback keeps the last few seconds of their inputs in a second lock-free ring; at Record the writer drains it into the take first and the clip starts at the Record point with those seconds as a handle before it |
| A lost device or a full disk keeps the take | ● | ● | ● | ● | **Works** |
| Lock against stopping a take by accident | ○ | ○ | ○ | ● | **Works**: Options › Recording › Lock Record: while recording, the first Stop (Space, the button, a controller) is refused with a message and a second within 2 s stops; Discard Take and Restart Take are not locked |
| Feedback protection when monitoring through speakers | ○ | ○ | ● | ○ | **Works** (by level, not Logic's analysis): Options › Recording › Feedback Protection, on by default: a monitored hardware input that clips (every block peaking at -0.1 dBFS or above) for a second is silenced on its track and the status line says so; turning its monitoring off and on brings it back, and the take records on. A loud sustained source just under clipping is never silenced. Not yet tried against real feedback through speakers |

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
| 10 | **Fixed:** inputs can now be as wide as a surround track, so a 5.1 track on a 5.1 input hears and records all six; a track wider than a stereo or wider input hears and records silence past the input's channels instead of repeating its last one (a mono input still feeds every side). Tests: `a_mono_input_feeds_both_sides`, `a_track_wider_than_its_input_records_silence_past_it` | `track_input_channel` in `crates/model/src/mixer.rs`; `plan_recording` in `crates/engine/src/io.rs` |

## Suggested order of work

Hours are agent-hours on this repo's calibration ([`target-app-parity.md`](../target-app-parity.md)); all are judgements.

**1. Fix what is wrong (≈ 10–16 h).** Bugs 1 and 3–9. Bug 2 and bug 10 are closed by tier 2.

**2. What all four have (≈ 60–95 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Record from a bus or a track's output (internal recording) | 10–16 | **Done** (bus input; a track's output via a bus); closed bug 2 |
| Record through input effects (an input strip, or a "record with inserts" switch) | 8–12 | **Done** (the switch) |
| Bit depth and file type for takes; BWF timestamp and originator | 5–8 | **Done** |
| Punch out on the fly, several punches per pass, crossfades at punch points | 8–12 | **Done** (QuickPunch) |
| Record mode setting: replace, keep takes as lanes, layer; loop record from the loop range | 6–10 | **Done** (replace, lanes, playlists) |
| Discard take; delete files of undone takes when the session closes | 3–5 | **Done** (undone takes: bug 6) |
| A small buffer for live tracks and a large one for playback | 15–22 | The largest item; shared with section 1's engine work |
| Cue mixes: sends to hardware outputs, click to its own output | 6–10 | **Done** (Control Room cues; click output) |
| Input meters before the fader, multichannel inputs, a tuner | 6–10 | **Done**; bug 10 partly (a track wider than its input) |

**3. What most have (≈ 25–40 h).** Arming follows selection, 2–3 (**done**); audio pre-record buffer
(Studio Pro, Cubase, and Pro Tools in QuickPunch), 5–8; remaining record time, 2–3 (**done**); band-limited
conversion on the input feed (shared with section 1's resampler), 4–6 (**done**); control of the interface's
direct monitoring with ASIO, 6–10, hardware needed (not planned here); record-safe and record lock, 2–4
(**done**); talkback, 4–6 (**done**).

**Where FreeDAWm is already level or ahead:** take placement (measured round trip, stored per
interface, independent of plug-in delay), the lock-free capture path with crash recovery, the
live waveform, device and disk failure handling, swipe comping, and remote recording.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Feature entries for the four come from
their manuals as known, not from a side-by-side run; treat a ◐ as "check before relying on it".

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-17 | minor | round three follow-up: feedback protection trips only at clipping; a track wider than its input is silent past it (bug 10 fixed); an input at another rate is read band-limited; Solo Take auditions a take in place of the comp |
| 2026-11-17 | minor | round three: built record safe; arm follows selection; restart take; lock record; input only monitoring; feedback protection; click on its own output; multichannel inputs; remaining record time; disk allocation; what is left written into the open rows; summary and suggested order brought up to date |
| 2026-11-17 | minor | round three reconciled: pre-fader metering and the tuner were already built; direct monitoring not planned here (ASIO SDK, hardware); playback's side of a rate difference is band-limited |
| 2026-11-16 | minor | cue mixes and talkback in the Control Room |
| 2026-11-16 | minor | bug 1 fixed; bugs 3 and 5 fixed; bug 4 fixed; bug 6 fixed; bug 7 fixed; bug 8 fixed; bug 9 was not a bug; internal recording from a bus; bug 2 fixed; record with inserts; take bit depth, file type and BWF timestamp; QuickPunch punches out and in on the fly; crossfades at punch points; record modes; loop record from the loop range; discard take |
| 2026-11-15 | major | First version: recording audio against Pro Tools, Studio Pro, Logic Pro and Cubase |
