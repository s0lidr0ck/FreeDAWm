# Four-DAW parity, section 6: recording MIDI

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-16 · **Change:** minor (fixes and features, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). FreeDAWm is not affiliated with or endorsed by any of them.
> See [NOTICE](../../NOTICE).

The sixth section ([1: playing audio](01-playback-audio.md),
[2: recording audio](02-recording-audio.md), [3: mixing](03-mixing.md),
[4: editing audio](04-editing-audio.md), [5: built-in effects](05-built-in-effects.md)).

**Scope of this section:** getting a performance from a keyboard, pads or the computer keys onto
a track: devices, what a track listens to, hearing yourself, timing, what is kept, record modes,
loops and takes, punch and capture, processing on the way in, step entry, the instruments there
are to play, and controllers and sync. Editing the notes afterwards (piano roll, quantize as an
edit, drum and score editors) is section 7.

## How this was measured

- **FreeDAWm:** read from the code on the working branch (0.9.11 plus work in progress on
  sections 1–5), not from the docs. The headline claims (recording always replaces, the MIDI
  Merge switch is not read, loop passes land on empty playlists, notes played before the transport
  rolls are dropped, All MIDI Notes Off does nothing, no MIDI output) were re-read by hand.
  Nothing was played on a keyboard, so "works" means the path is there and wired, and the timing
  figures are what the code computes, not what was measured.
- **The four DAWs:** from their published manuals and release notes, as known up to the versions
  above. Nothing was run side by side.

**Legend.** Other DAWs: ● has it · ◐ partly or by another route · ○ does not.
FreeDAWm: **Works** · **Partial** · **Flag only** (a switch or setting exists and is stored, but
nothing acts on it) · **Missing**.

## Summary

The middle of the path is sound. A hardware port or the computer keys feed lock-free queues; the
audio thread never allocates or locks on the way; an armed instrument track plays what it hears
and, on Stop, gets a clip with the notes placed by when they were played, corrected for output
latency, cut to the punch range and aware of loop passes. Stuck notes are guarded against in most
cases. Several tracks can record different ports and channels in one pass, with audio alongside.

Around that middle there are three kinds of gap:

1. **One record mode.** A take always erases what is under it. There is no merge or overdub, no
   drum-machine style loop where passes add up, and no MIDI takes to comp. The MIDI Merge switch
   in the transport is drawn and stored but not read.
2. **Switches that do nothing.** All MIDI Notes Off, MIDI Merge, Input Quantize (the engine can
   do it; no control turns it on), MIDI Track Offsets, the input filter, MIDI Thru in the Options
   menu, and capture of what was just played ("retrospective record") all exist as commands or
   settings with nothing behind them.
3. **Nothing goes out, and nothing is mapped.** There is no MIDI output at all: no hardware
   synths, no clock or timecode, no MIDI track playing an instrument on another track. There is no
   MIDI learn, no control surface, and no transport control from a controller.

The built-in instruments are four (a subtractive synth, a sampler, a drum machine and a drum
synth). There is no piano, and only the synth answers the sustain pedal and pitch bend.

## A. Devices

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| USB and class-compliant MIDI inputs (macOS, Windows, Linux as applicable) | ● | ● | ● | ● | **Works** (CoreMIDI, WinMM, ALSA) |
| Device list with enable and disable, activity lights | ● | ● | ● | ● | **Works** (Setup › MIDI Input Devices) |
| Devices found when plugged in while running | ● | ● | ● | ● | **Works**, within 2 s |
| Two identical devices used at once | ● | ● | ● | ● | **Missing**: ports are told apart by name only |
| More than 15 input ports | ● | ● | ● | ● | **Missing**: later ports are skipped |
| Device names and aliases, a studio setup of what is connected | ● | ● | ● | ● | **Flag only** |
| Per-device timing offset | ● | ◐ | ● | ● | **Missing** |
| Virtual ports from other apps; network and Bluetooth MIDI | ● | ● | ● | ● | **Partial**: whatever the system shows as an ordinary port; FreeDAWm opens no virtual port of its own |
| MIDI 2.0 devices | ○ | ○ | ● | ◐ | **Missing** |
| MIDI in the browser | ○ | ○ | ○ | ○ | **Missing**: the web build has no live MIDI, though its menus still show the windows |
| A take recorded on another computer arrives on the track | ○ | ○ | ○ | ○ | **Works** (remote recorder; the take arrives after the pass, not as a live stream) |

## B. What a track listens to, and hearing yourself

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Input: one port or all, one channel or all | ● | ● | ● | ● | **Works** |
| Several tracks record different ports or channels in one pass | ● | ● | ● | ● | **Works** |
| Layer several instruments from one keyboard | ● | ● | ● | ● | **Works** (arm them all) |
| Channel changed on the way in | ● | ● | ● | ● | **Missing**; the played channel is kept in the clip |
| Record arm and input monitor (auto, on, off) on the track | ● | ● | ● | ● | **Works** |
| The selected track is armed and heard automatically | ○ | ● | ● | ● | **Missing** |
| Global MIDI thru switch | ● | ● | ● | ● | **Works** in the devices window; the Options menu item is **Flag only** |
| A played note is heard through the track's instrument with the transport stopped | ● | ● | ● | ● | **Works** |
| Stuck-note protection on stop, disarm, input change, device unplugged | ● | ● | ● | ● | **Works**, except a quick unplug and replug |
| Panic (all notes off) | ● | ● | ● | ● | **Works** (Event › All MIDI Notes Off, Cmd+Shift+.) |
| Low-latency mode for the instrument being played | ◐ | ● | ● | ● | **Missing**: Low Latency Monitoring covers audio tracks only |
| MIDI activity or velocity meter on the track | ● | ● | ● | ● | **Partial**: only on the take being recorded |
| MIDI track that plays an instrument on another track; several MIDI tracks into one multi-timbral instrument | ● | ● | ● | ● | **Missing**: a MIDI track records but is silent |
| Instrument with several audio outputs to separate mixer channels | ● | ● | ● | ● | **Missing** |

## C. Timing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Notes placed where they were played, corrected for output latency | ● | ● | ● | ● | **Works** (output plus delay-compensation latency) |
| Timing kept inside an audio buffer, not snapped to its start | ● | ● | ● | ● | **Works** for recording; for what you hear, **Partial**: one buffer late by design, and timing flattens when a buffer is split |
| The driver's own timestamp used | ● | ● | ● | ● | **Missing**: events are stamped when FreeDAWm's callback runs |
| Follows the tempo map, including ramps | ● | ● | ● | ● | **Works** |
| Resolution | 960 | 960+ | 960 | 480+ | 960 ticks per quarter note |
| Per-track playback offset also applied to what is recorded | ● | ● | ● | ● | **Missing**: a track with an offset plays its new take shifted |
| Play freely with no click, then fit the tempo to the performance | ◐ | ◐ | ● | ● | **Missing** for MIDI |

## D. What is kept

| Data | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Notes with velocity and channel | ● | ● | ● | ● | **Works** |
| Release velocity | ● | ● | ● | ● | **Partial**: recorded; when played live it reaches CLAP and AU instruments, not VST3 or the built-in ones |
| Sustain and other controllers, pitch bend, channel and poly aftertouch, program change | ● | ● | ● | ● | **Works**, stored in the clip unthinned |
| Bank select, 14-bit controllers, RPN and NRPN understood as such | ● | ◐ | ● | ● | **Partial**: kept as separate plain controllers |
| SysEx | ● | ○ | ● | ● | **Missing** |
| MPE (per-note bend, pressure, timbre) | ● | ● | ● | ● | **Partial**: the channels pass through and are kept; nothing sets up or understands a zone |
| MIDI 2.0 resolution | ○ | ○ | ● | ◐ | **Missing** |
| Controllers recorded onto notes already there | ● | ● | ● | ● | **Missing**: the take replaces the notes |
| Controller moves recorded as plug-in or mixer automation | ● | ● | ● | ● | **Missing** |
| Thin or reduce controller data on the way in | ● | ◐ | ● | ● | **Missing** |
| A note held when recording starts | ◐ | ◐ | ◐ | ◐ | Dropped |
| A note still held at Stop or at the loop end | ● | ● | ● | ● | **Works**: cut at the end; a pedal still down comes up there |

## E. Record modes

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Replace what is under the take | ● | ● | ● | ● | **Works**, and it is the only mode |
| Merge into the existing clip (overdub) | ● | ● | ● | ● | **Flag only**: MIDI Merge is shown and stored, never read |
| New clip on top, old one kept | ◐ | ● | ● | ● | **Missing** |
| Choose the mode | ● | ● | ● | ● | **Flag only** |
| The existing part is heard while you record over it | ● | ● | ● | ● | **Works**, which makes replace surprising: you hear the old part, then lose it under the whole take |
| MIDI and audio recorded in one pass | ● | ● | ● | ● | **Works**, as separate undo steps |
| Record the output of another track or a MIDI plug-in | ● | ● | ● | ● | **Partial**: only a chain module's MIDI output (the drum gate's triggers) |
| One undo for the pass | ● | ● | ● | ● | **Partial**: two or three steps when audio and MIDI are both armed |
| The take survives a crash | ◐ | ◐ | ◐ | ◐ | **Missing**: a MIDI take is held in memory until Stop |

## F. Loops and takes

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Each loop pass kept as a take | ● | ● | ● | ● | **Works**: each pass after the first on a new playlist that starts as a copy of the track's |
| Passes add up into one clip (drum-machine style) | ● | ● | ● | ● | **Missing** |
| Earlier passes heard while later ones are recorded | ● | ● | ● | ● | **Missing**: clips appear only after Stop |
| Undo the last pass, or erase held notes, while the loop runs | ○ | ● | ◐ | ◐ | **Missing** |
| Take lanes for MIDI, pick or comp between takes | ◐ | ● | ● | ● | **Missing**: comping is for audio tracks only |
| Note repeat | ○ | ● | ● | ◐ | **Missing** |
| Record straight into a step pattern or drum grid | ○ | ● | ● | ◐ | **Missing**: recording over a pattern clip erases it; the drum machine's pads are not recorded |

## G. Punch, count-in and capture

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Punch over a selected range | ● | ● | ● | ● | **Works** |
| Punch in by pressing Record while playing | ● | ● | ● | ● | **Works** |
| Punch out and keep playing | ● | ● | ● | ● | **Missing**: Record again stops the transport |
| Pre-roll and post-roll | ● | ● | ● | ● | **Works** |
| Count-in | ● | ● | ● | ● | **Partial**: works, but the bar count and click sound can only be set by a command; the setup window is greyed |
| Notes played during the count-in are kept as a pickup | ● | ● | ● | ● | **Missing**: deliberately not taken |
| Metronome, optionally only while recording | ● | ● | ● | ● | **Works** |
| Capture what was just played without having pressed Record | ● | ● | ● | ● | **Flag only**: the command exists; no buffer listens |
| Recording starts at the first note played | ● | ○ | ○ | ○ | **Missing** |
| The take is drawn as it is played | ● | ● | ● | ● | **Works** |
| Footswitch punch; record from a controller's transport keys | ● | ● | ● | ● | **Missing** |

## H. Processing on the way in

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Input quantize with strength and swing, the played timing kept | ● | ● | ● | ● | **Partial**: the engine does it and keeps the performance; no control turns it on. Notes only |
| Input filter (leave out aftertouch, SysEx and so on) | ● | ◐ | ● | ● | **Flag only** |
| Transpose, velocity scale or curve, fixed velocity, key range on input | ● | ● | ● | ● | **Missing**: Real-Time Properties change clip playback only |
| Live MIDI effects before the instrument (arpeggiator, chord, scale) | ● | ● | ● | ● | **Missing** |
| Record the effect's output, not the raw keys | ● | ● | ● | ● | **Missing** |
| Scale or key lock on what you play | ○ | ◐ | ◐ | ● | **Missing** |
| The chord track follows or shapes live input | ○ | ● | ◐ | ● | **Partial**: the chord picker names the chord you hold |
| Programmable input transformer | ○ | ○ | ● | ● | **Missing** |

## I. Step entry and the computer keyboard

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Play and record from the computer keys, with octave and velocity | ● | ● | ● | ● | **Works** (Musical Typing; also a sustain key) |
| On-screen piano you can click | ● | ● | ● | ● | **Works** |
| Step entry from a MIDI keyboard, transport stopped | ● | ● | ● | ● | **Missing** |
| Step entry with chosen length, rests, ties and chords | ● | ● | ● | ● | **Partial**: the command supports length, chords and rests; Musical Typing's step mode enters one eighth note per key, and the menu item always enters the same note |
| Step entry inside the note editor | ● | ● | ● | ● | **Missing** |
| The editor shows the take while it is recorded | ● | ● | ● | ● | **Partial**: follows the playhead; the take is in the editor only after Stop |

## J. Instruments to play

Compared in full in section 7; listed here for what matters while recording.

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| New instrument track ready to play | ● | ● | ● | ● | **Works** (subtractive synth, input set to all, stereo from the New Tracks dialog too) |
| Drag an instrument from the browser to make a track | ● | ● | ● | ● | **Works** |
| Third-party instruments | ● AAX | ● | ● AU | ● VST | **Works**: CLAP, VST3 and AU, with channels kept |
| Piano and other sampled instruments in the box | ● | ● | ● | ● | **Missing**: a sampler with no library |
| Synth | ● | ● | ● | ● | **Works**: 16 voices; velocity, sustain, pitch bend fixed at ±2 semitones; no mod wheel or aftertouch |
| Sampler | ● | ● | ● | ● | **Partial**: velocity only; no sustain pedal, bend or mod wheel |
| Drum machine and drum synth | ● | ● | ● | ● | **Works**: velocity layers; notes only |
| Hardware synth as an instrument, with its latency measured | ◐ | ● | ● | ● | **Missing**: no MIDI output |

## K. Controllers and sync

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| MIDI output to hardware | ● | ● | ● | ● | **Missing** |
| MIDI clock out and in | ● | ● | ● | ● | **Flag only** (out), **Missing** (in) |
| MIDI timecode and machine control | ● | ◐ | ● | ● | **Missing** |
| MIDI learn: move a knob, it is mapped to a parameter | ◐ | ● | ● | ● | **Missing** |
| Mapped controller moves kept out of the recording | ● | ● | ● | ● | **Missing** |
| Control surfaces (Mackie Control, HUI and maker scripts) | ● | ● | ● | ● | **Missing** |
| Transport from a controller | ● | ● | ● | ● | **Missing** |

## L. Files

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Import a standard MIDI file with its tempo map | ● | ● | ● | ● | **Works** (formats 0, 1 and 2; tempo optional) |
| Format 0 split by channel; markers imported | ● | ● | ● | ● | **Missing** (markers are read, not placed) |
| Imported tracks ready to record on | ● | ● | ● | ● | **Partial**: they arrive with no MIDI input set |
| Export a MIDI file | ● | ● | ● | ● | **Works**; ignores clip trims and exports muted clips (bug 21) |

## Bugs found on the way

Found while reading; none was reproduced by playing. Paths are under `crates/`.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed.** Notes played between pressing Record and the transport rolling, or a first downbeat played a little early, moved before the take's start by the output latency and were dropped. An event heard from the take's start on that the latency would move before it now begins the take (`midi_record::played_at`). Test: `midi_record::tests::notes_played_before_the_transport_rolled_begin_the_take_despite_latency` (512 samples of latency) | `engine/src/midi_record.rs` (`played_at`) |
| 2 | **Fixed.** Each loop pass after the first went on a new, empty playlist, so the rest of the song vanished from playback. Each now goes on a copy of the track's playlist as it was before the take, as audio passes do. Test: `midi_record::tests::loop_recording_makes_a_clip_per_pass_on_new_playlists` | `engine/src/midi_record.rs` (`record_midi_on`) |
| 3 | A take erases everything under its whole range, even if one note was played, although the old part was audible during the take | `engine/src/edit.rs:254-255` |
| 4 | **Fixed.** Record pressed while a loop plays and the playhead is before it: the take now knows the loop (`midi_devices::finish_take` takes the transport's), its first pass runs from the record point to the loop's end and each later pass is the loop. Test: `midi_record::tests::record_pressed_before_a_playing_loop_keeps_every_pass` | `engine/src/midi_record.rs` (`pass_range`); `ui-egui/src/midi_devices.rs` |
| 5 | **Fixed.** A note begun in the last 32nd note of a pass (or half the input-quantize grid, if larger) and still held at its end, or released but about to be quantized onto it, was played early for the next downbeat: in a loop it now starts the next pass, its note-off with it; at the end of the take it is left out rather than kept as a sliver. Tests: `midi_record::tests::a_downbeat_played_early_at_a_loop_end_starts_the_next_pass`, `with_input_quantize_an_early_downbeat_does_not_snap_outside_the_clip`, `a_note_begun_just_before_stop_is_left_out` | `engine/src/midi_record.rs` (`settle_early_notes`) |
| 6 | **Fixed.** All MIDI Notes Off now reaches the audio thread (`Player::all_notes_off`): every instrument gets the note-offs of the keys live input holds, sustain and sostenuto up, All Notes Off and All Sound Off on all 16 channels and its own voice reset; clips playing chase their controllers again. Test: `mix` `tests::all_notes_off_lets_go_of_every_note_and_pedal_now` | `mix/src/lib.rs` (`MixEngine::all_notes_off`); `playback/src/lib.rs`; `ui-egui/src/lib.rs` |
| 7 | **Claim wrong.** Checked in midir 0.11.1's source: its CoreMIDI backend walks each packet and calls back once per message (`backend/coremidi/mod.rs`, `handle_input`); ALSA decodes one sequencer event per call and WinMM delivers one short message per call. So `parse` never sees two messages in one buffer and nothing is lost there; no code was changed. (midir itself stops reading a CoreMIDI packet at a data byte with no status byte, i.e. running status inside a packet; that is the library's, not FreeDAWm's.) | `playback/src/midi_in.rs:110-122` |
| 8 | **Fixed.** A sustain or sostenuto pedal still down when a clip ends comes up on its last tick; one pressed before the take, before the punch point or held over a loop wrap is down from the clip's start (the hub keeps each port's pedals and puts them into the take's first block, `MidiHub::record_pedals`). Tests: `midi_record::tests::a_pedal_down_at_stop_comes_up_at_the_clip_end_and_one_pressed_before_is_down_from_the_start`, `a_pedal_held_over_a_loop_wrap_is_down_in_the_next_pass`; `midi_in::tests::a_pedal_down_before_the_take_is_down_in_it` | `engine/src/midi_record.rs` (`pedals_down_at`, `lift_pedals`); `playback/src/midi_in.rs` |
| 9 | Events lost because the window stopped redrawing during a take (more than 16,384 waiting) are counted but never reported. Lost note-offs show as notes running to the end of the clip | `playback/src/midi_in.rs:28, 318`; `ui-egui/src/lib.rs:2456-2458` |
| 10 | Unplug and replug within two seconds: the port keeps its slot, so notes held at the unplug are never released | `playback/src/midi_in.rs:437-470` |
| 11 | Two devices with the same name collapse into one; the second cannot be used | `playback/src/midi_in.rs:526-535` |
| 12 | What you hear loses its timing when an audio buffer is rendered in several parts (large device buffers, a loop end inside the buffer, sample-rate conversion): all the notes land at the start of the first part | `playback/src/midi_in.rs:136-145`; `playback/src/lib.rs:1197-1203` |
| 13 | Module MIDI input recorded in a loop keeps only the last partial pass, or an empty range after a punch partway through | `ui-egui/src/lib.rs:2162-2166` |
| 14 | **Fixed** with bug 4: the first pass of a take punched in partway through a playing loop runs from the punch point to the loop's end, so what lies before it stays. Test: `midi_record::tests::a_punch_in_inside_a_playing_loop_leaves_what_came_before_it` | `engine/src/midi_record.rs` (`pass_range`) |
| 15 | **Fixed.** Choosing Instrument in the New Tracks dialog turns Mono into Stereo (other formats are kept). Not covered by a test (dialog state inside an egui closure); checked by reading | `ui-egui/src/dialogs.rs` |
| 16 | Menu items that run their command with no values and so change nothing: Input Quantize, MIDI Track Offsets. Step Input always enters the same eighth note | `ui-egui/src/menus.rs:949-965, 1075-1076`; `ui-egui/src/menubar.txt:435-440` |
| 17 | **Fixed.** The live drawing keeps only the track's channel and draws each event where it will be placed (the take's latency earlier). Test: `live_midi::tests::a_track_on_one_channel_shows_only_that_channels_notes` | `ui-egui/src/live_midi.rs` |
| 18 | Musical Typing's step mode ignores the velocity setting, does not lengthen the clip (later notes do not play), and a position before the clip collapses to its start | `ui-egui/src/musical_typing.rs:138-151` |
| 19 | The MIDI Real-Time Properties window never loads the selected track's values, writes all four on any change, and its changes cannot be undone | `ui-egui/src/ops_windows.rs:29-57, 283`; `engine/src/cmd/event_more.rs:116` |
| 20 | A MIDI track's mixer strip always reads "MIDI in: all" though its input defaults to none; tracks made by MIDI import also have none and record nothing | `ui-egui/src/mix_window.rs:401-402`; `model/src/mixer.rs:1029`; `engine/src/io.rs:429` |
| 21 | MIDI export writes notes outside a clip's trimmed window and includes muted clips | `engine/src/io.rs:492-585` |
| 22 | Settings stored that nothing reads: MIDI Merge, QuickPunch, TrackPunch, Destructive Record, MIDI Thru (Options), MIDI live mode, input filter, beat clock, MIDI studio, and a second set of device flags | `engine/src/cmd/options.rs:22-59`; `engine/src/cmd/setup_more.rs:30-107` |
| 23 | The web build shows Musical Typing and MIDI Input Devices though it has no MIDI | `ui-egui/src/lib.rs:996-999` |

## Suggested order of work

Estimates are agent-hours for one implementer with tests, as in the earlier sections. Timing
needs a real keyboard and a loopback measurement to confirm; no estimate includes that.

**1. Fix what is wrong (≈ 16–26 h).** Bugs 1, 2 and 4 first: each loses or hides what was played.
Then 6 (panic), 7 (check on macOS before changing), 8 and 5. Bug 3 is settled by the merge mode
in tier 2; until then, erase only from the first to the last event recorded. For bugs 16 and 22,
either wire the control or take it off the menu.

**2. What all four have (≈ 120–190 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Record modes: merge, replace, new clip on top; MIDI Merge switch wired | 6–10 | Settles bug 3 |
| Loop recording that adds up, earlier passes heard, undo last pass | 8–14 | Needs clips to exist while the loop runs |
| MIDI takes: lanes, pick a take, comp between takes | 12–18 | Extend the audio comping model to MIDI tracks |
| Capture what was just played (a listening buffer behind the existing command) | 5–8 | |
| Input quantize control; count-in and click setup window; pickup notes from the count-in | 5–8 | |
| Punch out and keep playing | 2–4 | |
| Input filter and input transforms (transpose, velocity, channel) applied to thru and take | 6–10 | |
| MIDI effects before the instrument: arpeggiator, chord, scale; record raw or processed | 14–22 | |
| MIDI routing: a MIDI track plays an instrument elsewhere; multi-timbral; multi-output instruments | 14–22 | Touches the mixer; coordinate with section 3 work |
| MIDI output: hardware ports, external instrument with measured latency, clock and timecode out | 14–22 | |
| MIDI learn, a mapping list, transport from a controller, mapped moves kept out of takes | 10–16 | |
| Mackie Control and HUI surfaces | 12–20 | Listed in section 3 as well; count once |
| Step entry from a MIDI keyboard and in the editor, with length, rests, ties, chords | 6–10 | |
| Low-latency path for the instrument being played; driver timestamps; per-device offset | 8–12 | Fixes bug 12 |
| SysEx kept; bank, RPN and NRPN understood; controllers as automation | 8–12 | |
| Devices: identical names, more than 15 ports, aliases | 4–6 | Bugs 10 and 11 |
| Built-in instruments answer sustain, bend and mod wheel; a piano | 8–14 | The piano needs a sample set with a licence that allows shipping it |
| MIDI take survives a crash | 3–5 | |

**3. What most have (≈ 40–65 h).** Auto-arm of the selected track, 2–3; note repeat, 4–6;
recording into a step pattern and from the drum machine's pads, 8–12; MPE set-up and per-note
editing data, 8–14; scale lock on input and a chord track that shapes input, 6–10; fit the tempo
to a free performance, 8–12; MIDI 2.0 (Logic, partly Cubase), 6–10. A programmable input
transformer is Logic's and Cubase's and belongs in the last section, as does Pro Tools' wait for
note.

**Outside the comparison:** Web MIDI for the browser build (6–10 h) would give that build live
input, which none of the four offers.

**Where FreeDAWm is already level or ahead:** the input path is allocation-free and lock-free
end to end; several ports and channels record to several tracks in one pass; stuck-note handling
covers disarm, input change and unplugging; a drum gate's triggers can be recorded as MIDI; and a
take played on another computer can land on a track here.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Feature entries for the four come from
their manuals as known, not from a side-by-side run; treat a ◐ as "check before relying on it".

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | major | First version: recording MIDI against Pro Tools, Studio Pro, Logic Pro and Cubase |
| 2026-11-16 | minor | Bug 1 fixed: notes played before the transport rolls begin the take instead of being dropped |
| 2026-11-16 | minor | Bug 2 fixed: loop passes after the first land on a copy of the track's playlist, not an empty one |
| 2026-11-16 | minor | Bugs 4 and 14 fixed: Record pressed before or inside a playing loop keeps every pass and leaves what came before the punch point |
| 2026-11-16 | minor | Bug 6 fixed: All MIDI Notes Off lets go of every note and pedal |
| 2026-11-16 | minor | Bug 7 corrected: the claim was wrong (midir calls back once per message on every backend FreeDAWm uses) |
| 2026-11-16 | minor | Bug 8 fixed: a pedal down at the end of a take comes up, and one pressed before it is kept |
| 2026-11-16 | minor | Bug 5 fixed: an early downbeat at a loop's end starts the next pass |
| 2026-11-16 | minor | Bug 15 fixed: the New Tracks dialog makes instrument tracks stereo |
| 2026-11-16 | minor | Bug 17 fixed: the live take shows only its channel, where its notes will be placed |
