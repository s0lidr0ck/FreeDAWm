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
   song. Loop passes are heard while the loop runs, and the last can be undone while it does. Still missing: MIDI takes to comp.
2. **Switches that did nothing.** All MIDI Notes Off, MIDI Merge, Input Quantize, MIDI Track
   Offsets, count-off set-up, capture of what was just played and the input filter now work. Still stored and
   unread: MIDI Thru in the Options menu and the other settings bug 22 lists.
3. **Going out and mapping.** MIDI output to hardware ports works (External MIDI), and Mackie
   Control and HUI surfaces drive the mixer and transport, MIDI learn maps controllers, and a MIDI
   track can play an instrument on another track, and MIDI clock and timecode go out. Still
   missing: clock and timecode in.

The built-in instruments are four (a subtractive synth, a sampler, a drum machine and a drum
synth). There is no piano, and only the synth answers the sustain pedal and pitch bend.

## A. Devices

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| USB and class-compliant MIDI inputs (macOS, Windows, Linux as applicable) | ● | ● | ● | ● | **Works** (CoreMIDI, WinMM, ALSA) |
| Device list with enable and disable, activity lights | ● | ● | ● | ● | **Works** (Setup › MIDI Input Devices) |
| Devices found when plugged in while running | ● | ● | ● | ● | **Works**, within 2 s |
| Two identical devices used at once | ● | ● | ● | ● | **Works**: the second is `<name> #2` (in the system's order, so which is which can swap after a restart) |
| More than 15 input ports | ● | ● | ● | ● | **Works**: 63 hardware ports at once (64 slots, one for Musical Typing) |
| Device names and aliases, a studio setup of what is connected | ● | ● | ● | ● | **Partial**: an alias per port (Setup › MIDI Input Devices, per computer) names it in the track input menus; no studio setup |
| Per-device timing offset | ● | ◐ | ● | ● | **Works**: 0–100 ms per input port in Setup › MIDI Input Devices (per computer): its messages are placed that much earlier, heard and recorded |
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
| Channel changed on the way in | ● | ● | ● | ● | **Works**: the track input menu › Input Transform › Channel (otherwise the played channel is kept in the clip) |
| Record arm and input monitor (auto, on, off) on the track | ● | ● | ● | ● | **Works** |
| The selected track is armed and heard automatically | ○ | ● | ● | ● | **Missing** |
| Global MIDI thru switch | ● | ● | ● | ● | **Works** in the devices window; the Options menu item is **Flag only** |
| A played note is heard through the track's instrument with the transport stopped | ● | ● | ● | ● | **Works** |
| Stuck-note protection on stop, disarm, input change, device unplugged | ● | ● | ● | ● | **Works**; a quick unplug and replug (within a rescan) is caught where the system gives the port a new identity (Linux/ALSA), not on macOS or Windows (bug 10) |
| Panic (all notes off) | ● | ● | ● | ● | **Works** (Event › All MIDI Notes Off, Cmd+Shift+.) |
| Low-latency mode for the instrument being played | ◐ | ● | ● | ● | **Missing**: Low Latency Monitoring covers audio tracks only |
| MIDI activity or velocity meter on the track | ● | ● | ● | ● | **Partial**: only on the take being recorded |
| MIDI track that plays an instrument on another track; several MIDI tracks into one multi-timbral instrument | ● | ● | ● | ● | **Works** (`f7169c2`): the track's output menu › Play Instrument On picks an instrument or MIDI track and a channel (`mix.midi_to_track`); the track's instrument becomes MIDI to Track, which passes what it plays (clips, live input, chase, the panic) to the target's instrument in the same block, each event on its channel, merged with the target's own notes. The source renders first, so its notes land at the same sample offsets as the target's own and share its delay compensation; a route that would feed a track into itself is refused. Mute or solo of the source lets go of its notes; a route added, changed or taken off lets go only of the notes of that track (another source's held notes sound on). Multi-output instruments (one instrument's outputs on several mixer channels) remain |
| Instrument with several audio outputs to separate mixer channels | ● | ● | ● | ● | **Missing** |

## C. Timing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Notes placed where they were played, corrected for output latency | ● | ● | ● | ● | **Works** (output plus delay-compensation latency) |
| Timing kept inside an audio buffer, not snapped to its start | ● | ● | ● | ● | **Works** for recording; for what you hear, **Partial**: one buffer late by design, and timing flattens when a buffer is split |
| The driver's own timestamp used | ● | ● | ● | ● | **Works** where midir passes one: put on FreeDAWm's clock by the smallest delivery delay seen (drift allowed), so messages delivered together keep their spacing; not measured on hardware |
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
| New clip on top, old one kept | ◐ | ● | ● | ● | **Works** as take lanes: Keep Takes as Lanes puts each take on a lane of its own under the track, the comp playing the newest; New Playlist still keeps the old part on its own playlist. Clips do not overlap on one playlist |
| Choose the mode | ● | ● | ● | ● | **Works**: the MIDI Merge switch, its right-click menu (Replace, Merge, New Playlist, Keep Takes as Lanes), `options.midi_record_mode` |
| The existing part is heard while you record over it | ● | ● | ● | ● | **Works**, which makes replace surprising: you hear the old part, then lose it under the whole take |
| MIDI and audio recorded in one pass | ● | ● | ● | ● | **Works**, as separate undo steps |
| Record the output of another track or a MIDI plug-in | ● | ● | ● | ● | **Partial**: only a chain module's MIDI output (the drum gate's triggers) |
| One undo for the pass | ● | ● | ● | ● | **Partial**: two or three steps when audio and MIDI are both armed |
| The take survives a crash | ◐ | ◐ | ◐ | ◐ | **Works**: what a take hears is spilled to the autosave folder as it comes (`recording-midi.jsonl`), and recovery places it as Stop would have, with the audio takes |

## F. Loops and takes

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Each loop pass kept as a take | ● | ● | ● | ● | **Works**: each pass after the first on a new playlist that starts as a copy of the track's |
| Passes add up into one clip (drum-machine style) | ● | ● | ● | ● | **Works** with MIDI Merge on; the passes appear at Stop |
| Earlier passes heard while later ones are recorded | ● | ● | ● | ● | **Works**: at each wrap the finished passes go on the armed tracks as Stop will place them (merged, they add up; otherwise the last is on top), and the next pass plays them; no undo step until Stop (`c458cdf`) |
| Undo the last pass, or erase held notes, while the loop runs | ○ | ● | ◐ | ◐ | **Partial**: Undo (Cmd+Z) or Event › More MIDI › Undo Last Pass while loop recording takes back the last finished pass, from what plays and from the take (`c458cdf`); erasing held notes is missing |
| Take lanes for MIDI, pick or comp between takes | ◐ | ● | ● | ● | **Works** (`2af9eae`): with Keep Takes as Lanes (or on a MIDI or instrument track whose lanes are open, in any mode but Merge) each take and each loop pass goes on a take lane, drawn under the track with its notes; a click on a lane picks the whole take, a drag swipes a range of it into the comp, Up/Down cycle a comp segment through the takes, as on audio tracks. MIDI joins get no crossfade: the outgoing piece's notes end at the join, a note the incoming take held over it sounds from there. Merge keeps passes in one clip as before |
| Note repeat | ○ | ● | ● | ◐ | **Works** as a MIDI effect (the track's input menu › MIDI Effects › Add › Note Repeat): each key held plays again at the rate (1/4 to 1/32T), on the song's grid while the transport plays, a step after the key when stopped; channel or poly pressure sets the repeats' velocity. What is recorded is the keys as played |
| Record straight into a step pattern or drum grid | ○ | ● | ● | ◐ | **Missing**: recording over a pattern clip erases it; the drum machine's pads are not recorded |

## G. Punch, count-in and capture

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Punch over a selected range | ● | ● | ● | ● | **Works** |
| Punch in by pressing Record while playing | ● | ● | ● | ● | **Works** |
| Punch out and keep playing | ● | ● | ● | ● | **Works** (QuickPunch: Record while recording punches out, again punches in); a MIDI take is kept to each punch |
| Pre-roll and post-roll | ● | ● | ● | ● | **Works** |
| Count-in | ● | ● | ● | ● | **Works**: Setup › Click/Countoff… sets the bars and the click |
| Notes played during the count-in are kept as a pickup | ● | ● | ● | ● | **Works** as an option (Setup › Click/Countoff… › Keep MIDI played during the count-off): the take then begins at the bar the first of them was played in; off by default, as before. Not before the session's start |
| Metronome, optionally only while recording | ● | ● | ● | ● | **Works** |
| Capture what was just played without having pressed Record | ● | ● | ● | ● | **Works** while the transport plays: Event › More MIDI › Capture MIDI (desktop) makes a take of the playback under way or the last one, on the armed (or selected) tracks; with the transport stopped, what was played since is captured at the session's tempo from the playhead's bar, its first note on the downbeat |
| Recording starts at the first note played | ● | ○ | ○ | ○ | **Missing** |
| The take is drawn as it is played | ● | ● | ● | ● | **Works** |
| Footswitch punch; record from a controller's transport keys | ● | ● | ● | ● | **Missing** |

## H. Processing on the way in

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Input quantize with strength and swing, the played timing kept | ● | ● | ● | ● | **Works** (Event › More MIDI › Input Quantize). Notes only |
| Input filter (leave out aftertouch, SysEx and so on) | ● | ◐ | ● | ● | **Works**: Setup › MIDI Input Devices › Leave out (notes, pitch bend, aftertouch, poly aftertouch, program, controllers; `setup.midi_input_filter`, the session's), for every port, from what is heard, recorded and captured; SysEx and real-time bytes are never taken |
| Transpose, velocity scale or curve, fixed velocity, key range on input | ● | ● | ● | ● | **Works**: per track, the input menu › Input Transform (`track.midi_input_transform`), applied to what the instrument hears and to the take, kept when the port or channel changes; Real-Time Properties still change clip playback only |
| Live MIDI effects before the instrument (arpeggiator, chord, scale) | ● | ● | ● | ● | **Works**: up to four per instrument or MIDI track (input menu › MIDI Effects; `track.midi_fx_add`, `_set`, `_remove`, undoable, saved with the session): Arpeggiator (rate, Up, Down, Up/Down, As Played, Random, 1-4 octaves, gate, latch; on the song's grid while playing), Chord (twelve shapes), Scale (root and twelve scales; move to the nearest note or leave out) and Note Repeat, in list order, each with a bypass. They run on the audio thread without allocating, on everything the instrument is given (clips, live input, routed MIDI tracks, the Band); a MIDI track routed to another instrument passes on what they made. A change of setting lets go of the notes they made and plays the held keys through the new list |
| Record the effect's output, not the raw keys | ● | ● | ● | ● | **Missing**: a take keeps the keys as played (the effects play them again on playback) |
| Scale or key lock on what you play | ○ | ◐ | ◐ | ● | **Works** with the Scale MIDI effect (its own root and scale; it does not follow the Key ruler) |
| The chord track follows or shapes live input | ○ | ● | ◐ | ● | **Partial**: the chord picker names the chord you hold |
| Programmable input transformer | ○ | ○ | ● | ● | **Missing** |

## I. Step entry and the computer keyboard

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Play and record from the computer keys, with octave and velocity | ● | ● | ● | ● | **Works** (Musical Typing; also a sustain key) |
| On-screen piano you can click | ● | ● | ● | ● | **Works** |
| Step entry from a MIDI keyboard, transport stopped | ● | ● | ● | ● | **Works**: Event › More MIDI › Step Input… (any input the target track listens to, its transform applied) |
| Step entry with chosen length, rests, ties and chords | ● | ● | ● | ● | **Works**: lengths 1/1 to 1/32 and triplets, velocity as played or fixed, Rest, Tie (`event.step_input` `tie`), keys held together entered as a chord when the last is let go; Musical Typing's own step mode still enters eighths |
| Step entry inside the note editor | ● | ● | ● | ● | **Works**: the MIDI editor's Step button opens Step Input on the clip it shows |
| The editor shows the take while it is recorded | ● | ● | ● | ● | **Partial**: follows the playhead; the take is in the editor only after Stop |

## J. Instruments to play

Compared in full in section 7; listed here for what matters while recording.

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| New instrument track ready to play | ● | ● | ● | ● | **Works** (subtractive synth, input set to all, stereo from the New Tracks dialog too) |
| Drag an instrument from the browser to make a track | ● | ● | ● | ● | **Works** |
| Third-party instruments | ● AAX | ● | ● AU | ● VST | **Works**: CLAP, VST3 and AU, with channels kept |
| Piano and other sampled instruments in the box | ● | ● | ● | ● | **Missing**: a sampler with no library |
| Synth | ● | ● | ● | ● | **Works**: 16 voices; velocity, sustain, pitch bend over Bend Range (0–24 semitones, 2 by default), the mod wheel as vibrato up to Mod Wheel Vibrato (0 by default: turn it up; `2a60fad`); no aftertouch |
| Sampler | ● | ● | ● | ● | **Partial**: velocity, sustain pedal, pitch bend over Bend Range, the mod wheel as vibrato up to Mod Wheel Vibrato (0 by default); one sample, no zones |
| Drum machine and drum synth | ● | ● | ● | ● | **Works**: velocity layers; notes only |
| Hardware synth as an instrument, with its latency measured | ◐ | ● | ● | ● | **Partial**: External MIDI plays it (MIDI output); its audio comes back on an ordinary audio track, and its own latency is not measured or compensated |

## K. Controllers and sync

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| MIDI output to hardware | ● | ● | ● | ● | **Works**: an instrument or MIDI track's output menu › MIDI Output sends what it plays (clips, live input, chased controllers) to a hardware port, on its own channels or one; output ports start disabled (on Windows a port is open in one program at a time) and are enabled in Setup › MIDI Input Devices › Outputs, except that a port a track sends to opens unless it was disabled there (the window then says a track wants it), and choosing one for a track enables it (`34e755e`); each message leaves when the audio beside it is heard (output latency and delay compensation included); a stop, a locate, All MIDI Notes Off and closing the port let go of every note left on. The audio thread only pushes into a lock-free ring; a sender thread per port talks to the driver |
| MIDI clock out and in | ● | ● | ● | ● | **Works** (out, `c1ae613`): Setup › MIDI Input Devices › Outputs › Sync out › MIDI clock and a port (`setup.midi_beat_clock`, the session's): 24 clocks a quarter on the tempo map's samples, Song Position Pointer then Start (Continue away from the start) on Play, Stop on Stop, Stop + position + Continue on a loop wrap or locate while playing; stamped to leave with the audio beside it, plus the setting's offset. **Missing** (in) |
| MIDI timecode and machine control | ● | ◐ | ● | ● | **Partial**: MTC out (Sync out › MIDI timecode: quarter frames at the session's frame rate from its timecode start; no full-frame message on a locate, the receiver locks within two frames); MMC in (the transport row). MTC in and MMC out missing |
| MIDI learn: move a knob, it is mapped to a parameter | ◐ | ● | ● | ● | **Works**: right-click a fader, pan knob, expanded send level or a plug-in's knob on the chain › Learn MIDI Controller, then move a CC or press a key; Setup › MIDI Mappings lists them (range, invert, delete). Parameter mappings are saved with the session (they name its tracks and plug-ins); transport keys with the app (the controller on this desk). A move is the control's own command: one undo step per gesture, written to automation as the on-screen control is |
| Mapped controller moves kept out of the recording | ● | ● | ● | ● | **Works**: the MIDI hub claims mapped CCs and notes before they reach a port's ring, so they are never heard, recorded or played through; a surface's whole ports are kept out |
| Control surfaces (Mackie Control, HUI and maker scripts) | ● | ● | ● | ● | **Partial**: Setup › Control Surfaces adds Mackie Control (with extenders) and HUI on chosen input and output ports. Faders with touch (touch is the on-screen fader held, for Touch and Latch), V-Pots as pan, mute, solo, rec and select, transport, bank and channel shift, automation-mode keys, scribble strips, meters, motor faders and LEDs following the mix, the jog wheel, the MCU handshake and HUI's ping. Tested against simulated devices (documented bytes in, bytes out), not yet against hardware. No maker scripts, no plug-in or send pages on the V-Pots, no flip; Avid EUCON is not possible (proprietary, licensed SDK) |
| Transport from a controller | ● | ● | ● | ● | **Works**: MMC from any input; learnt keys or footswitches (Play, Stop, Play/Stop, Record, Return to Zero, Loop, Punch: Record again, which punches with QuickPunch on); a surface's transport keys |

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
| 10 | **Partly fixed** (`2a34aa9`). A port whose system identity changed between two scans (ALSA gives a replugged device a new client and port) is closed and opened afresh, and the notes and pedals it held are let go on the next block (`LiveMidi::reset`). midir's port ids are stable on macOS and Windows, so there a replug within two seconds is still not seen. Test: `midi_in::tests::identical_devices_more_than_fifteen_ports_and_a_quick_replug`. | `playback/src/midi_in.rs:437-470` |
| 11 | **Fixed** (`2a34aa9`). Ports with the same name are listed as `<name>`, `<name> #2`, … (`midi_in::unique_names`) and each opens on its own. Test: as bug 10's. | `playback/src/midi_in.rs:526-535` |
| 12 | **Not started** (part of the tier-2 low-latency item: the player would keep the callback's later events for its later parts; needs a loopback measurement to confirm). What you hear loses its timing when an audio buffer is rendered in several parts (large device buffers, a loop end inside the buffer, sample-rate conversion): all the notes land at the start of the first part | `playback/src/midi_in.rs:136-145`; `playback/src/lib.rs:1197-1203` |
| 13 | **Fixed.** Module MIDI recorded in a loop now prints over the loop (the print is the same each pass), from the punch point when Record was pressed inside it (`module_record::print_range`). Test: `engine/tests/midi_recording.rs`. Commit `ac4252a`. | `engine/src/module_record.rs`; `ui-egui/src/lib.rs` |
| 14 | **Fixed** with bug 4: the first pass of a take punched in partway through a playing loop runs from the punch point to the loop's end, so what lies before it stays. Test: `midi_record::tests::a_punch_in_inside_a_playing_loop_leaves_what_came_before_it`. Commit `120c0de`. | `engine/src/midi_record.rs` (`pass_range`) |
| 15 | **Fixed.** Choosing Instrument in the New Tracks dialog turns Mono into Stereo (other formats are kept). Not covered by a test (dialog state inside an egui closure); checked by reading. Commit `b1f5bbc`. | `ui-egui/src/dialogs.rs` |
| 16 | **Partly fixed.** Event › More MIDI › Input Quantize opens a window (on/off, grid, strength, swing) and MIDI Track Offsets one (milliseconds per MIDI track); Setup › Click/Countoff… opens one too (level, sounds, only while recording, count-off bars) instead of being greyed out. Step Input opens its window now (tier 2). Commit `6d66dce`. | `ui-egui/src/ops_windows.rs`; `ui-egui/src/menus.rs`; `ui-egui/src/menubar.txt` |
| 17 | **Fixed.** The live drawing keeps only the track's channel and draws each event where it will be placed (the take's latency earlier). Test: `live_midi::tests::a_track_on_one_channel_shows_only_that_channels_notes`. Commit `83ce5f3`. | `ui-egui/src/live_midi.rs` |
| 18 | **Fixed.** Musical Typing's step mode enters through `event.step_input` on the selected clip's track: at the keys' velocity, the clip lengthened to hold the note, a new clip where there is none. Test: `musical_typing::tests::step_mode_uses_the_velocity_and_lengthens_the_clip`. Commits `f6a245f`, `0f4e38a`. | `ui-egui/src/musical_typing.rs` |
| 19 | **Fixed.** The MIDI Real-Time Properties window reads the first selected MIDI track's values when the selection changes, writes only the property moved, and its changes can be undone (`event.midi_rtp` is an undo step; a drag is one). Test: `engine/tests/midi_recording.rs` (undo); the window by reading. Commits `149f728`, `f2ecfd9`. | `ui-egui/src/ops_windows.rs`; `engine/src/cmd/event_more.rs` |
| 20 | **Fixed.** New MIDI tracks and the tracks a MIDI file import makes listen to every MIDI input, as new instrument tracks do, and a MIDI track's strip shows its real input. Test: `engine/tests/midi_recording.rs`. Commit `91059d3`. | `engine/src/cmd/track.rs`; `engine/src/io.rs`; `ui-egui/src/mix_window.rs` |
| 21 | **Fixed.** MIDI export writes what plays: muted clips are left out, notes outside a clip's trimmed window too, and a note running past its clip's end ends there. Test: `engine/tests/midi_recording.rs`. Commit `b34b914`. | `engine/src/io.rs` (`export_midi`) |
| 22 | **Partly fixed.** MIDI Merge is read now (record modes); QuickPunch by the recording-audio work. The input filter is read now (Setup › MIDI Input Devices). Still stored and unread: TrackPunch, Destructive Record, MIDI Thru (Options), MIDI live mode, MIDI studio, the second set of device flags (beat clock is read now: clock and timecode out). Commit `68d0dc0`. | `engine/src/cmd/options.rs:22-59`; `engine/src/cmd/setup_more.rs:30-107` |
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
| Loop recording that adds up, earlier passes heard, undo last pass | 8–14 | Done: passes add up with MIDI Merge (`68d0dc0`); finished passes are on the tracks while the loop runs and Undo takes the last back (`midi_record::LoopPreview`, `c458cdf`) |
| MIDI takes: lanes, pick a take, comp between takes | 12–18 | Done (`2af9eae`): Keep Takes as Lanes on the audio comp model |
| Capture what was just played (a listening buffer behind the existing command) | 5–8 | Done (`3675b3d`) while the transport plays; with it stopped too, a listening clock of its own (`e5635d1`) |
| Input quantize control; count-in and click setup window; pickup notes from the count-in | 5–8 | Partly: the Input Quantize and Click/Countoff windows (`6d66dce`); pickup notes from the count-in kept as an option (`e5635d1`) |
| Punch out and keep playing | 2–4 | Done: QuickPunch (recording-audio work); MIDI takes follow its punches (`1b92b9f`) |
| Input filter and input transforms (transpose, velocity, channel) applied to thru and take | 6–10 | Done: the filter at the hub, the transform per track in the mix and the take (`036f22e`) |
| MIDI effects before the instrument: arpeggiator, chord, scale; record raw or processed | 14–22 | Done for the effects (`mix::midi_fx`: a host plugin around the instrument, see Designs); recording the processed output remains (takes keep the keys) |
| MIDI routing: a MIDI track plays an instrument elsewhere; multi-timbral; multi-output instruments | 14–22 | Partly (`f7169c2`): a MIDI or instrument track plays another track's instrument on its channel, several into one (MIDI to Track, ordered with the module MIDI inputs in `mix::sidechain`). Multi-output instruments not started: the CLAP host already gives a plug-in a buffer per output port but plays only the main one (`clap-host` `out_main`), the instrument slot hands the mixer one buffer, and the mixer has no channel to return the other outputs on (an aux-like strip fed by an instrument output, ordered after it); that is a host, plug-in trait and mixer change, left for the mixing work |
| MIDI output: hardware ports, external instrument with measured latency, clock and timecode out | 14–22 | Partly: hardware ports, enable/disable, notes off on stop and close (External MIDI); clock and MTC out (`mix::midi_sync`, `c1ae613`). The external instrument's latency remains |
| MIDI learn, a mapping list, transport from a controller, mapped moves kept out of takes | 10–16 | Done: learn from the fader, pan, send level and chain knobs; the MIDI Mappings window; MMC and learnt transport keys; claimed messages kept out of takes and Thru. Learn from a plug-in's own window and from the drawer's knobs remain |
| Mackie Control and HUI surfaces | 12–20 | Done for the roles above (`soundcraft_engine::surface`): a protocol trait with Mackie Control (and XT) and HUI behind it; tested against simulated devices. Hardware checks, V-Pot pages (sends, plug-ins) and flip remain |
| Step entry from a MIDI keyboard and in the editor, with length, rests, ties, chords | 6–10 | Done: the Step Input window and the editor's Step button (`fa832ab`, `486cf93`; Musical Typing's step mode fixed: bug 18) |
| Low-latency path for the instrument being played; driver timestamps; per-device offset | 8–12 | Partly: driver timestamps and per-device offsets done (`2a34aa9`); bug 12 and the low-latency path open |
| SysEx kept; bank, RPN and NRPN understood; controllers as automation | 8–12 | Not started |
| Devices: identical names, more than 15 ports, aliases | 4–6 | Done: bug 11 fixed, 63 ports, aliases; bug 10 fixed where the system gives a new identity (`2a34aa9`) |
| Built-in instruments answer sustain, bend and mod wheel; a piano | 8–14 | Not started: the instruments live in the built-in effects work's crate (`dsp`); the piano needs a sample set whose licence allows shipping it (an owner question) |
| MIDI take survives a crash | 3–5 | Done: spilled to the autosave folder during the take, placed by recovery (`397e268`) |

**Designs for what remains of K (not built).**

- *MIDI clock and timecode out.* *Built with one destination port per session (the stored
  `setup.midi_beat_clock`, now read) rather than a per-port Sync column, and without MTC's
  full-frame SysEx (the output ring carries 3-byte messages).* A per-machine list of output ports that carry sync (Setup › MIDI
  Input Devices › Outputs, a Sync column: off, clock, MTC). The audio thread knows the transport
  and tempo map per block; at a block start it works out the clock ticks (24 per quarter, from the
  tempo map at the block's position) and MTC quarter frames (four per frame at the session's frame
  rate) that fall in the block, and pushes each into the port's External MIDI ring with the
  heard-time stamp the instruments use (`OutHub::begin_block` plus the offset), so the sender
  thread puts them on the wire on time. Play sends Song Position Pointer then Start (or Continue);
  Stop sends Stop; a locate while stopped sends Song Position Pointer, and for MTC a full-frame
  SysEx. Nothing new crosses to the audio thread but the port keys (atomics), so it stays
  allocation-free. Tests: render blocks at a known tempo and count the 0xF8 stamps.
- *A MIDI track playing an instrument on another track (multi-timbral).* *Built, differently from
  this sketch: no new `Route`; the source's instrument becomes MIDI to Track (`mix::track_midi`,
  the target in its state, as External MIDI keeps its port), whose events the target reads as it
  reads a module's MIDI.* A MIDI track's output
  gets a route `Route::Instrument(TrackId)` (and a channel). The mix renders MIDI sources first:
  such a track's notes (clips, live input, chase) are scheduled into a per-strip MIDI buffer
  instead of an instrument, the same fixed-capacity block buffer modules' MIDI uses
  (`Route::Modules`), and the target instrument track merges every buffer routed to it, each
  keeping its channel, before it runs; a multi-timbral instrument answers each channel. Cycles
  are refused as sends' are; a frozen or inactive target plays nothing. Recording stays on the
  MIDI track, mute and solo of the MIDI track gate its notes, and its automation lanes are not
  audio. Section 3's mixer owns the routing menus; this is about two days with tests.

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

## Designs for the open tier-2 and tier-3 work (second round)

Written before code, as the work list asks for the large items; none of these is built yet.

**MIDI take lanes and comping (tier 2).** *Built as designed, except that the comp plays the newest take (as audio comping does) rather than staying as it was.* The audio comp model (`engine/src/cmd/comp.rs`) is
playlist based: each take layer is a playlist, the comp is the active one, a swipe copies a
range of a layer into it (`edit::copy_range`, which already cuts MIDI clips) and remembers the
layer (`Clip::from_layer`). MIDI takes already land on playlists (loop passes without MIDI Merge,
New Playlist mode), so the model needs no new storage. The work: (1) a MIDI record mode *Keep
Takes as Lanes* that puts each take or pass on a new layer and leaves the comp playlist as it was
(`midi_record::place_take` choosing the layer); (2) `comp.swipe` and `comp.cycle` checks that say
"audio" read "a clip" (`layer_has_audio` is already any clip), and the join crossfades skipped
for MIDI clips (a note across a join is cut at the join and its note-off kept); (3) the take
lanes drawn under instrument and MIDI tracks as they are under audio tracks (the lane painter
reads clips only, so MIDI clips need their note thumbnails there); (4) picking a whole take
(`comp.cycle` over the take's range). Merge mode keeps passes in one clip and stays as it is.

**Low-latency monitoring of the instrument being played (tier 2).** Today a live note waits one
render block (`midi_in::block_offset`) plus the mix's delay compensation. The path: while an
armed or input-monitored instrument track hears live MIDI, its strip is rendered outside delay
compensation (as Low Latency Monitoring does for audio tracks: the latent inserts after the
instrument bypassed, the strip not delayed to line up), switched by the same Options item. Bug 12
(timing flattened when a callback is rendered in several blocks) is a separate fix in the
player: `drain_into` would keep the events whose stamp falls after the current block's share of
the callback for the next block, which needs a small fixed holding buffer in `LiveMidi`; both
need a loopback measurement on hardware to confirm and touch the transport code another agent
owns.

**MIDI effects before the instrument (tier 2/3).** *Built (arpeggiator, chord, scale, note
repeat) as a host plugin, not a strip stage: once a track has a MIDI effect, `mix::midi_fx::sync`
wraps its instrument in a `MidiFxHost` that forwards every plugin call except the notes and
controllers, which it collects through the block and runs through the effects before the
instrument processes (fixed arrays, nothing allocated after the wrap). Every route into an
instrument already arrives through those calls, so clips, live input, routed MIDI tracks, the
Band and preview notes are all processed with no change at their call sites, and a track that
never had an effect is not wrapped at all. The record switch (as played / as processed) is not
built: takes keep the keys.* A per-track MIDI effects slot list ahead of
the instrument, run on the audio thread each block on the strip's merged live and clip events
(fixed-capacity buffers, no allocation): arpeggiator (rate, pattern, octaves, gate, latch;
follows the tempo map), chord (one key plays a stored chord shape), scale (snap or drop notes
outside the session's key) and note repeat (rate from the grid, velocity from pressure). Each
keeps the notes it started so a stop, a bypass or a change of setting sends their note-offs. A
switch per track, *Record: as played / as processed*: processed, the take is printed from the
effect's output the way module MIDI is recorded today (`module_record`), raw keeps
`midi_record` as it is. Then: recording into a step pattern (a take over a pattern clip writes
its steps by grid position instead of replacing the clip) and recording the drum machine's pads
(the pads already raise note events; they would go through the hub's Musical Typing slot so a
take keeps them). SysEx would need a variable-length side ring next to the 3-byte rings, and a
`CtrlKind::SysEx` in clips; bank select, RPN and NRPN would be paired into 14-bit controller
events when a take is placed (the raw controllers kept as they are).

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
| 2026-11-16 | minor | MIDI learn, the MIDI Mappings window, MMC and learnt transport keys; mapped messages kept out of takes |
| 2026-11-16 | minor | Loop recording: earlier passes are heard while later ones record, and Undo takes back the last pass while looping |
| 2026-11-16 | minor | A MIDI take survives a crash: spilled to the autosave folder as it is played, placed by recovery |
| 2026-11-16 | minor | Capture MIDI with the transport stopped; notes played in the count-off kept as a pickup when asked |
| 2026-11-16 | minor | MIDI output ports start disabled; a port a track sends to still opens |
| 2026-11-16 | minor | MIDI input filter wired; per-track input transforms (transpose, velocity, curve, fixed velocity, key range, channel) for thru and take |
| 2026-11-16 | minor | Step input from a MIDI keyboard with lengths, rests, ties and chords; the MIDI editor's Step button |
| 2026-11-16 | minor | Identical devices, 63 input ports, aliases, per-device offsets, the driver's timestamps; bug 11 fixed, bug 10 partly |
| 2026-11-16 | minor | Commits noted on the second round's rows; designs for MIDI take lanes, low-latency monitoring and MIDI effects |
| 2026-11-16 | minor | Synth and sampler rows brought up to date (Bend Range, sustain, mod wheel; Mod Wheel Vibrato 0 by default) |
| 2026-11-16 | minor | MIDI take lanes: Keep Takes as Lanes record mode, lanes with notes under MIDI and instrument tracks, pick, swipe and cycle between MIDI takes |
| 2026-11-16 | minor | A MIDI track plays an instrument on another track (Play Instrument On, MIDI to Track), several on their channels into one multi-timbral instrument; multi-output instruments still open |
| 2026-11-16 | minor | MIDI clock and MTC out to a chosen port (Sync out in the MIDI devices window), the stored beat-clock setting wired |
| 2026-11-16 | minor | Commits noted on the third round's MIDI rows |
| 2026-11-16 | minor | MIDI effects before the instrument: Arpeggiator, Chord, Scale and Note Repeat (input menu › MIDI Effects); takes keep the keys as played |
| 2026-11-16 | minor | MIDI to Track: changing one route lets go only of that track's notes on the target, not every routed note |
