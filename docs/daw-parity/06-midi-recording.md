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

Around that middle there were three kinds of gap; where they stand after the work listed in the
revision history:

1. **Record modes.** Settled: Merge (the MIDI Merge switch: overdub, loop passes adding up in
   one clip; the default for new sessions), Replace and New Playlist. Loop passes after the first keep the rest of the
   song. Still missing: MIDI takes to comp, and hearing earlier passes while the loop runs.
2. **Switches that did nothing.** All MIDI Notes Off, MIDI Merge, Input Quantize, MIDI Track
   Offsets, count-off set-up and capture of what was just played now work. Still stored and
   unread: the input filter, MIDI Thru in the Options menu and the other settings bug 22 lists.
3. **Going out and mapping.** MIDI output to hardware ports works (External MIDI), and Mackie
   Control and HUI surfaces drive the mixer and transport. Still missing: clock and timecode out,
   a MIDI track playing an instrument on another track, and MIDI learn.

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
| MIDI in the browser | ○ | ○ | ○ | ○ | **Missing**: the web build has no live MIDI (its menus leave the MIDI windows out) |
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
| Per-track playback offset also applied to what is recorded | ● | ● | ● | ● | **Works**: a take on a track with a MIDI Track Offset is placed that much earlier, so it plays back where it was played |
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
| Replace what is under the take | ● | ● | ● | ● | **Works** (Replace in the MIDI Merge switch's menu; the default before 2026-11-16, and sessions saved with it keep it) |
| Merge into the existing clip (overdub) | ● | ● | ● | ● | **Works**: MIDI Merge joins the take and the MIDI clips it overlaps into one clip. The default for new sessions (owner's decision); a saved session keeps the switch it stored |
| New clip on top, old one kept | ◐ | ● | ● | ● | **Partial**: New Playlist puts the take on a copy of the track's playlist and keeps the old one; clips do not overlap on one playlist |
| Choose the mode | ● | ● | ● | ● | **Works**: the MIDI Merge switch, its right-click menu (Replace, Merge, New Playlist), `options.midi_record_mode` |
| The existing part is heard while you record over it | ● | ● | ● | ● | **Works**, which makes replace surprising: you hear the old part, then lose it under the whole take |
| MIDI and audio recorded in one pass | ● | ● | ● | ● | **Works**, as separate undo steps |
| Record the output of another track or a MIDI plug-in | ● | ● | ● | ● | **Partial**: only a chain module's MIDI output (the drum gate's triggers) |
| One undo for the pass | ● | ● | ● | ● | **Partial**: two or three steps when audio and MIDI are both armed |
| The take survives a crash | ◐ | ◐ | ◐ | ◐ | **Missing**: a MIDI take is held in memory until Stop |

## F. Loops and takes

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Each loop pass kept as a take | ● | ● | ● | ● | **Works**: each pass after the first on a new playlist that starts as a copy of the track's |
| Passes add up into one clip (drum-machine style) | ● | ● | ● | ● | **Works** with MIDI Merge on; the passes appear at Stop |
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
| Punch out and keep playing | ● | ● | ● | ● | **Works** (QuickPunch: Record while recording punches out, again punches in); a MIDI take is kept to each punch |
| Pre-roll and post-roll | ● | ● | ● | ● | **Works** |
| Count-in | ● | ● | ● | ● | **Works**: Setup › Click/Countoff… sets the bars and the click |
| Notes played during the count-in are kept as a pickup | ● | ● | ● | ● | **Missing**: deliberately not taken |
| Metronome, optionally only while recording | ● | ● | ● | ● | **Works** |
| Capture what was just played without having pressed Record | ● | ● | ● | ● | **Works** while the transport plays: Event › More MIDI › Capture MIDI (desktop) makes a take of the playback under way or the last one, on the armed (or selected) tracks; not with the transport stopped |
| Recording starts at the first note played | ● | ○ | ○ | ○ | **Missing** |
| The take is drawn as it is played | ● | ● | ● | ● | **Works** |
| Footswitch punch; record from a controller's transport keys | ● | ● | ● | ● | **Missing** |

## H. Processing on the way in

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Input quantize with strength and swing, the played timing kept | ● | ● | ● | ● | **Works** (Event › More MIDI › Input Quantize). Notes only |
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
| Hardware synth as an instrument, with its latency measured | ◐ | ● | ● | ● | **Partial**: External MIDI plays it (MIDI output); its audio comes back on an ordinary audio track, and its own latency is not measured or compensated |

## K. Controllers and sync

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| MIDI output to hardware | ● | ● | ● | ● | **Works**: an instrument or MIDI track's output menu › MIDI Output sends what it plays (clips, live input, chased controllers) to a hardware port, on its own channels or one; ports are enabled in Setup › MIDI Input Devices › Outputs; each message leaves when the audio beside it is heard (output latency and delay compensation included); a stop, a locate, All MIDI Notes Off and closing the port let go of every note left on. The audio thread only pushes into a lock-free ring; a sender thread per port talks to the driver |
| MIDI clock out and in | ● | ● | ● | ● | **Flag only** (out), **Missing** (in) |
| MIDI timecode and machine control | ● | ◐ | ● | ● | **Missing** |
| MIDI learn: move a knob, it is mapped to a parameter | ◐ | ● | ● | ● | **Missing** |
| Mapped controller moves kept out of the recording | ● | ● | ● | ● | **Missing** |
| Control surfaces (Mackie Control, HUI and maker scripts) | ● | ● | ● | ● | **Partial**: Setup › Control Surfaces adds Mackie Control (with extenders) and HUI on chosen input and output ports. Faders with touch (touch is the on-screen fader held, for Touch and Latch), V-Pots as pan, mute, solo, rec and select, transport, bank and channel shift, automation-mode keys, scribble strips, meters, motor faders and LEDs following the mix, the jog wheel, the MCU handshake and HUI's ping. Tested against simulated devices (documented bytes in, bytes out), not yet against hardware. No maker scripts, no plug-in or send pages on the V-Pots, no flip; Avid EUCON is not possible (proprietary, licensed SDK) |
| Transport from a controller | ● | ● | ● | ● | **Partial**: from a Mackie Control or HUI surface's transport keys |

## L. Files

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Import a standard MIDI file with its tempo map | ● | ● | ● | ● | **Works** (formats 0, 1 and 2; tempo optional) |
| Format 0 split by channel; markers imported | ● | ● | ● | ● | **Missing** (markers are read, not placed) |
| Imported tracks ready to record on | ● | ● | ● | ● | **Works**: they listen to every MIDI input |
| Export a MIDI file | ● | ● | ● | ● | **Works**: what plays (trims kept, muted clips left out) |

## Bugs found on the way

Found while reading; none was reproduced by playing. Paths are under `crates/`.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed.** Notes played between pressing Record and the transport rolling, or a first downbeat played a little early, moved before the take's start by the output latency and were dropped. An event heard from the take's start on that the latency would move before it now begins the take (`midi_record::played_at`). Test: `midi_record::tests::notes_played_before_the_transport_rolled_begin_the_take_despite_latency` (512 samples of latency). Commit `f8a6d2d`. | `engine/src/midi_record.rs` (`played_at`) |
| 2 | **Fixed.** Each loop pass after the first went on a new, empty playlist, so the rest of the song vanished from playback. Each now goes on a copy of the track's playlist as it was before the take, as audio passes do. Test: `midi_record::tests::loop_recording_makes_a_clip_per_pass_on_new_playlists`. Commit `79fca68`. | `engine/src/midi_record.rs` (`record_midi_on`) |
| 3 | **Settled** by the record modes (tier 2): Replace still clears the take's whole range, as Replace does in the four; Merge (the MIDI Merge switch) keeps what was there and adds the take; New Playlist keeps the old part on its own playlist. Commit `68d0dc0`. | `engine/src/midi_record.rs` (`RecordMode`) |
| 4 | **Fixed.** Record pressed while a loop plays and the playhead is before it: the take now knows the loop (`midi_devices::finish_take` takes the transport's), its first pass runs from the record point to the loop's end and each later pass is the loop. Test: `midi_record::tests::record_pressed_before_a_playing_loop_keeps_every_pass`. Commit `120c0de`. | `engine/src/midi_record.rs` (`pass_range`); `ui-egui/src/midi_devices.rs` |
| 5 | **Fixed.** A note begun in the last 32nd note of a pass (or half the input-quantize grid, if larger) and still held at its end, or released but about to be quantized onto it, was played early for the next downbeat: in a loop it now starts the next pass, its note-off with it. At the end of a take it stays as played, a sliver if Stop cut it (left out, the last note of a take that ends on its last event, as a remote recorder's does, would be lost). Tests: `midi_record::tests::a_downbeat_played_early_at_a_loop_end_starts_the_next_pass`, `with_input_quantize_an_early_downbeat_does_not_snap_outside_the_clip`, `a_note_begun_just_before_the_last_pass_ends_stays_as_played`. Commits `78904fa`, `985028c`. | `engine/src/midi_record.rs` (`settle_early_notes`) |
| 6 | **Fixed.** All MIDI Notes Off now reaches the audio thread (`Player::all_notes_off`): every instrument gets the note-offs of the keys live input holds, sustain and sostenuto up, All Notes Off and All Sound Off on all 16 channels and its own voice reset; clips playing chase their controllers again. Test: `mix` `tests::all_notes_off_lets_go_of_every_note_and_pedal_now`. Commit `602cd08`. | `mix/src/lib.rs` (`MixEngine::all_notes_off`); `playback/src/lib.rs`; `ui-egui/src/lib.rs` |
| 7 | **Claim wrong.** Checked in midir 0.11.1's source: its CoreMIDI backend walks each packet and calls back once per message (`backend/coremidi/mod.rs`, `handle_input`); ALSA decodes one sequencer event per call and WinMM delivers one short message per call. So `parse` never sees two messages in one buffer and nothing is lost there; no code was changed. (midir itself stops reading a CoreMIDI packet at a data byte with no status byte, i.e. running status inside a packet; that is the library's, not FreeDAWm's.). Commit `47d0223`. | `playback/src/midi_in.rs:110-122` |
| 8 | **Fixed.** A sustain or sostenuto pedal still down when a clip ends comes up on its last tick; one pressed before the take, before the punch point or held over a loop wrap is down from the clip's start (the hub keeps each port's pedals and puts them into the take's first block, `MidiHub::record_pedals`). Tests: `midi_record::tests::a_pedal_down_at_stop_comes_up_at_the_clip_end_and_one_pressed_before_is_down_from_the_start`, `a_pedal_held_over_a_loop_wrap_is_down_in_the_next_pass`; `midi_in::tests::a_pedal_down_before_the_take_is_down_in_it`. Commit `1372c52`. | `engine/src/midi_record.rs` (`pedals_down_at`, `lift_pedals`); `playback/src/midi_in.rs` |
| 9 | **Fixed.** Recorded events the window did not collect in time are now reported at Stop, in the status line and the log, with what a lost note-off does (`midi_devices::lost_in_take`). The loss itself is not prevented: the record ring holds 16,384 events between frames. Commit `9046321`. | `ui-egui/src/midi_devices.rs`; `ui-egui/src/lib.rs` |
| 10 | **Not started** (part of the tier-2 devices item; midir's port ids are stable on macOS and Windows, so a quick replug cannot be told from a port that stayed). Unplug and replug within two seconds: the port keeps its slot, so notes held at the unplug are never released | `playback/src/midi_in.rs:437-470` |
| 11 | **Not started** (part of the tier-2 devices item (ports would be told apart by midir's port id, not their name)). Two devices with the same name collapse into one; the second cannot be used | `playback/src/midi_in.rs:526-535` |
| 12 | **Not started** (part of the tier-2 low-latency item: the player would keep the callback's later events for its later parts; needs a loopback measurement to confirm). What you hear loses its timing when an audio buffer is rendered in several parts (large device buffers, a loop end inside the buffer, sample-rate conversion): all the notes land at the start of the first part | `playback/src/midi_in.rs:136-145`; `playback/src/lib.rs:1197-1203` |
| 13 | **Fixed.** Module MIDI recorded in a loop now prints over the loop (the print is the same each pass), from the punch point when Record was pressed inside it (`module_record::print_range`). Test: `engine/tests/midi_recording.rs`. Commit `ac4252a`. | `engine/src/module_record.rs`; `ui-egui/src/lib.rs` |
| 14 | **Fixed** with bug 4: the first pass of a take punched in partway through a playing loop runs from the punch point to the loop's end, so what lies before it stays. Test: `midi_record::tests::a_punch_in_inside_a_playing_loop_leaves_what_came_before_it`. Commit `120c0de`. | `engine/src/midi_record.rs` (`pass_range`) |
| 15 | **Fixed.** Choosing Instrument in the New Tracks dialog turns Mono into Stereo (other formats are kept). Not covered by a test (dialog state inside an egui closure); checked by reading. Commit `b1f5bbc`. | `ui-egui/src/dialogs.rs` |
| 16 | **Partly fixed.** Event › More MIDI › Input Quantize opens a window (on/off, grid, strength, swing) and MIDI Track Offsets one (milliseconds per MIDI track); Setup › Click/Countoff… opens one too (level, sounds, only while recording, count-off bars) instead of being greyed out. Step Input still runs its command with defaults (step entry is its own tier-2 item). Commit `6d66dce`. | `ui-egui/src/ops_windows.rs`; `ui-egui/src/menus.rs`; `ui-egui/src/menubar.txt` |
| 17 | **Fixed.** The live drawing keeps only the track's channel and draws each event where it will be placed (the take's latency earlier). Test: `live_midi::tests::a_track_on_one_channel_shows_only_that_channels_notes`. Commit `83ce5f3`. | `ui-egui/src/live_midi.rs` |
| 18 | **Fixed.** Musical Typing's step mode enters through `event.step_input` on the selected clip's track: at the keys' velocity, the clip lengthened to hold the note, a new clip where there is none. Test: `musical_typing::tests::step_mode_uses_the_velocity_and_lengthens_the_clip`. Commits `f6a245f`, `0f4e38a`. | `ui-egui/src/musical_typing.rs` |
| 19 | **Fixed.** The MIDI Real-Time Properties window reads the first selected MIDI track's values when the selection changes, writes only the property moved, and its changes can be undone (`event.midi_rtp` is an undo step; a drag is one). Test: `engine/tests/midi_recording.rs` (undo); the window by reading. Commits `149f728`, `f2ecfd9`. | `ui-egui/src/ops_windows.rs`; `engine/src/cmd/event_more.rs` |
| 20 | **Fixed.** New MIDI tracks and the tracks a MIDI file import makes listen to every MIDI input, as new instrument tracks do, and a MIDI track's strip shows its real input. Test: `engine/tests/midi_recording.rs`. Commit `91059d3`. | `engine/src/cmd/track.rs`; `engine/src/io.rs`; `ui-egui/src/mix_window.rs` |
| 21 | **Fixed.** MIDI export writes what plays: muted clips are left out, notes outside a clip's trimmed window too, and a note running past its clip's end ends there. Test: `engine/tests/midi_recording.rs`. Commit `b34b914`. | `engine/src/io.rs` (`export_midi`) |
| 22 | **Partly fixed.** MIDI Merge is read now (record modes); QuickPunch by the recording-audio work. Still stored and unread: TrackPunch, Destructive Record, MIDI Thru (Options), MIDI live mode, input filter, beat clock, MIDI studio, the second set of device flags. Commit `68d0dc0`. | `engine/src/cmd/options.rs:22-59`; `engine/src/cmd/setup_more.rs:30-107` |
| 23 | **Fixed.** Musical Typing and MIDI Input Devices are left out of the web build's menus. Test: `ui-egui/tests/menu_bar.rs` (`live_midi_windows_are_desktop_only`). Commit `352090a`. | `ui-egui/src/menubar.txt` |

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
| Record modes: merge, replace, new clip on top; MIDI Merge switch wired | 6–10 | Done (`68d0dc0`): Replace, Merge, New Playlist; settles bug 3 |
| Loop recording that adds up, earlier passes heard, undo last pass | 8–14 | Partly: passes add up with MIDI Merge (`68d0dc0`). Earlier passes heard and undo of the last pass are not done: they need clips to exist while the loop runs |
| MIDI takes: lanes, pick a take, comp between takes | 12–18 | Not started (large: the audio comping model would have to take MIDI clips) |
| Capture what was just played (a listening buffer behind the existing command) | 5–8 | Done (`3675b3d`) while the transport plays; not with it stopped |
| Input quantize control; count-in and click setup window; pickup notes from the count-in | 5–8 | Partly: the Input Quantize and Click/Countoff windows (`6d66dce`); pickup notes from the count-in not done |
| Punch out and keep playing | 2–4 | Done: QuickPunch (recording-audio work); MIDI takes follow its punches (`1b92b9f`) |
| Input filter and input transforms (transpose, velocity, channel) applied to thru and take | 6–10 | Not started |
| MIDI effects before the instrument: arpeggiator, chord, scale; record raw or processed | 14–22 | Not started (large; needs a design: a MIDI stage ahead of the instrument on the audio thread, its output recordable) |
| MIDI routing: a MIDI track plays an instrument elsewhere; multi-timbral; multi-output instruments | 14–22 | Not started (large; touches the mixer, section 3) |
| MIDI output: hardware ports, external instrument with measured latency, clock and timecode out | 14–22 | Partly: hardware ports, enable/disable, notes off on stop and close (External MIDI). The external instrument's latency, clock and timecode out remain |
| MIDI learn, a mapping list, transport from a controller, mapped moves kept out of takes | 10–16 | Not started |
| Mackie Control and HUI surfaces | 12–20 | Done for the roles above (`soundcraft_engine::surface`): a protocol trait with Mackie Control (and XT) and HUI behind it; tested against simulated devices. Hardware checks, V-Pot pages (sends, plug-ins) and flip remain |
| Step entry from a MIDI keyboard and in the editor, with length, rests, ties, chords | 6–10 | Not started (Musical Typing's step mode fixed: bug 18) |
| Low-latency path for the instrument being played; driver timestamps; per-device offset | 8–12 | Not started; bug 12 open |
| SysEx kept; bank, RPN and NRPN understood; controllers as automation | 8–12 | Not started |
| Devices: identical names, more than 15 ports, aliases | 4–6 | Not started; bugs 10 and 11 open |
| Built-in instruments answer sustain, bend and mod wheel; a piano | 8–14 | Not started: the instruments live in the built-in effects work's crate (`dsp`); the piano needs a sample set whose licence allows shipping it (an owner question) |
| MIDI take survives a crash | 3–5 | Not started |

**3. What most have (≈ 40–65 h).** Not started yet. Auto-arm of the selected track, 2–3; note repeat, 4–6;
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
| 2026-11-16 | minor | Bug 20 fixed: new MIDI tracks and imported tracks listen to every MIDI input |
| 2026-11-16 | minor | Bug 9 fixed: MIDI events lost during a take are reported at Stop |
| 2026-11-16 | minor | Bug 13 fixed: module MIDI recorded in a loop prints over the loop |
| 2026-11-16 | minor | Bug 18 fixed: Musical Typing's step mode uses the velocity and grows the clip |
| 2026-11-16 | minor | Bug 19 fixed: MIDI Real-Time Properties shows the track's values, writes what moved and undoes |
| 2026-11-16 | minor | Bug 21 fixed: MIDI export keeps clip trims and leaves muted clips out |
| 2026-11-16 | minor | Bug 23 fixed: the web build's menus leave out the live MIDI windows |
| 2026-11-16 | minor | Bug 16 partly fixed: Input Quantize, MIDI Track Offsets and Click/Countoff have windows |
| 2026-11-16 | minor | MIDI takes follow QuickPunch's punches: punch out and keep playing works for MIDI |
| 2026-11-16 | minor | Record modes: Replace, Merge (the MIDI Merge switch, read at last) and New Playlist; merged loop passes add up; settles bug 3 |
| 2026-11-16 | minor | MIDI Track Offsets apply to what is recorded |
| 2026-11-16 | minor | Capture MIDI: what was just played without Record becomes a take |
| 2026-11-16 | minor | Status of the work list, commits on the fixed bugs, summary brought up to date |
| 2026-11-16 | minor | Merge is the default MIDI record mode for new sessions; saved sessions keep theirs |
| 2026-11-16 | minor | MIDI output to hardware ports (External MIDI), enabled in Setup › MIDI Input Devices |
| 2026-11-16 | minor | Control surfaces: Mackie Control (with extenders) and HUI, Setup › Control Surfaces |
