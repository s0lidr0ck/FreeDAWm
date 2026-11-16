# Four-DAW parity, section 7: writing MIDI

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-16 · **Change:** minor (bug fixes and first tier-2 items, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). Third-party instruments named below belong to their makers.
> FreeDAWm is not affiliated with or endorsed by any of them. See [NOTICE](../../NOTICE).

The seventh section ([1: playing audio](01-playback-audio.md),
[2: recording audio](02-recording-audio.md), [3: mixing](03-mixing.md),
[4: editing audio](04-editing-audio.md), [5: built-in effects](05-built-in-effects.md),
[6: recording MIDI](06-midi-recording.md)).

**Scope of this section:** writing and editing notes after or instead of playing them: the note
editor, velocity and controllers, quantize and the other operations, help with scales and chords,
drums and patterns, the event list and the score, MIDI clips on the timeline, how written MIDI is
played back, tempo and arrangement, tools that write MIDI for you, the instruments that sound it,
and getting MIDI in and out. Getting a performance in from a keyboard is
[section 6](06-midi-recording.md).

## How this was measured

- **FreeDAWm:** read from the code on the working branch (0.9.11 plus work in progress on
  sections 1–6), not from the docs. The headline claims (Quantize and Legato from the editor act
  on the whole clip, trimming a clip's start deletes notes, notes past a clip's end are kept but
  silent, the track's tick timebase is not read by tempo edits, instrument parameters take no
  automation, clip notes go out without their channel) were re-read by hand. Nothing was listened
  to, so "works" means the code is there and wired.
- **The four DAWs:** from their published manuals and release notes, as known up to the versions
  above. Nothing was run side by side.

**Legend.** Other DAWs: ● has it · ◐ partly or by another route · ○ does not.
FreeDAWm: **Works** · **Partial** · **Flag only** (a command, field or control exists, but no
control reaches it or nothing acts on it) · **Missing**.

## Summary

The note editor does the core jobs well: draw, move, resize, copy, split and join notes; a
velocity lane; controller lanes for any CC, bend and pressure; ghost notes from other tracks; a
bar for typing values. Nearly every edit is one undoable command that scripts can run. Playback
is sample-accurate and chases controllers on a locate. Three things go beyond the four: a drum
step sequencer with per-step probability, ratchets and conditions; non-destructive MIDI Cleanup
against the chord track; and tools that write MIDI (a six-player Band, audio-to-MIDI models and
text-to-MIDI).

The gaps are of four kinds:

1. **Operations hit the wrong notes.** Quantize and Legato from the editor ignore the selection
   and change the whole clip, on a grid counted from the clip's start, not from the bar lines.
   Several menu items run with no values and do something unexpected (Change Duration applies
   legato) or nothing.
2. **MIDI clips are placed in samples.** A clip's start and length do not follow the tempo, so a
   tempo change leaves clips off their bars and can silence their last notes. Trimming a clip's
   start deletes the notes and controllers before it for good. Notes moved past a clip's end stay
   in the clip, unseen and unheard.
3. **Much of what the four offer around notes is absent:** note mute, groove templates, playback
   quantize, a drum-map view, chord and strum tools, scale highlight, editing several clips at
   once, shared clip copies, stretching a clip, a full event list, note entry in the score.
4. **Instruments are few and thin.** Four built in; the synth has two oscillators, one low-pass
   filter, no LFO and no presets; the sampler holds one sample. No instrument parameter can be
   automated, instrument latency is not compensated, and notes from clips reach the instrument
   without their channel.

## A. The note editor

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Piano roll opened from a clip, docked or in its own window | ● | ● | ● | ● | **Works** |
| Notes edited directly on the track | ● | ◐ | ○ | ● | **Flag only**: the view exists in the menu; nothing handles it |
| Draw, move, resize, copy by dragging, delete | ● | ● | ● | ● | **Works**; drawing needs the main window's Pencil tool |
| Marquee, shift-click, select all | ● | ● | ● | ● | **Works** |
| Select by clicking a key; invert; select by rule (pitch, velocity, position) | ● | ● | ● | ● | **Works**: a click on a piano key selects that pitch's notes (Shift adds, a drag along the keys adds each key's); Invert Selection, Select Same Pitch and Select by Rule… (pitch, velocity, length and beat-of-the-bar ranges, in the MIDI Operations window) in the editor's More menu |
| Split a note, join notes | ● | ● | ● | ● | **Works** |
| Mute notes | ● | ● | ● | ● | **Works**: Mute Notes / Unmute Notes in the editor's More menu (`midi.notes_set` with `muted`); a muted note is drawn faint, does not play and is left out of MIDI export; sessions saved before load unmuted, and unmuted notes save as before |
| Hear a note on click and while dragging it | ● | ● | ● | ● | **Partial**: on click, draw and drop, not during the drag |
| Type exact start, length, pitch, velocity | ● | ● | ● | ● | **Works** (note bar) |
| Ghost notes from other tracks | ● | ● | ● | ● | **Works**, per track, read-only |
| Edit several clips or tracks at once | ● | ● | ● | ● | **Missing**: one clip at a time |
| Notes can run past the clip's edge and still play, or the clip grows | ● | ● | ● | ● | **Works**: an edit that puts notes past the end grows the clip to the next bar line, up to the next clip (bug 4, fixed); notes a trim left past the end stay hidden |
| Every edit undoable, one step per drag | ● | ● | ● | ● | **Works**, the event list too (bug 21, fixed) |
| Every edit scriptable | ○ | ◐ | ◐ | ◐ | **Works** |

## B. Grid, snap and keys

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Grid from whole notes to 64ths, triplets | ● | ● | ● | ● | **Works** |
| Dotted values, other tuplets | ● | ● | ● | ● | **Partial**: dotted only by following the main window's grid; no quintuplets and the like |
| Grid that adapts to zoom | ○ | ● | ● | ● | **Missing** |
| Snap to the grid, or keep the offset from it | ● | ● | ● | ● | **Partial**: edges snap to lines, bodies keep their offset; no switch |
| New note takes the last note's length and velocity | ● | ● | ● | ● | **Works** |
| Zoom, scroll, zoom to the notes | ● | ● | ● | ● | **Partial**: fit shows the clip, not the notes |
| Keys: nudge, transpose by semitone and octave | ● | ● | ● | ● | **Works** |
| Keys: lengthen and shorten | ● | ● | ● | ● | **Works**: Alt+Right and Alt+Left, a grid step |

## C. Velocity and controllers

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Velocity lane; drag one or many; draw a ramp | ● | ● | ● | ● | **Works**; stacked notes are spread so each can be grabbed |
| Velocity shown as note colour | ● | ● | ● | ● | **Works** |
| Scale, compress, limit, randomise velocity | ● | ● | ● | ● | **Partial**: scale has a dialog; limit and randomise are **Flag only** |
| Release velocity | ● | ◐ | ● | ● | **Flag only**: stored; every edit writes the middle value |
| Per-note probability | ○ | ● | ◐ | ◐ | **Missing** in notes; present on pattern steps |
| Lanes for any controller, pitch bend, channel pressure; several at once | ● | ● | ● | ● | **Works**, per MIDI channel or every channel with a line each (bug 20, fixed) |
| Lanes for poly aftertouch and program change | ● | ● | ● | ● | **Missing**: recorded data of those kinds is not shown |
| Draw freehand | ● | ● | ● | ● | **Works** |
| Line, curve and shape tools; ramps between points | ● | ● | ● | ● | **Missing**: values are stepped |
| Scale or thin a range of controller data | ● | ● | ● | ● | **Missing** |
| Controllers move and copy with their notes | ● | ● | ● | ● | **Missing** |
| Per-note expression (MPE) editing | ◐ | ● | ● | ● | **Missing** |

## D. Operations

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Quantize the selected notes: grid, strength, swing | ● | ● | ● | ● | **Works**: the selected notes (every note when none is), on the grid counted from the bar (bugs 1 and 2, fixed); triplet, dotted and quintuplet grids |
| Quantize window, note ends, keep length, random offset | ● | ● | ● | ● | **Flag only** |
| Groove templates; take a groove from audio or MIDI | ● | ● | ● | ● | **Missing** |
| Quantize applied at playback, original kept | ● | ● | ● | ● | **Missing**: quantize rewrites the notes |
| Restore the original performance | ● | ● | ● | ● | **Partial**: only for takes recorded with input quantize, or after an explicit flatten; kept in place through trims and splits (bug 12, fixed) |
| Humanise timing | ● | ● | ● | ● | **Works**: timing and velocity, seeded, in the MIDI Operations window |
| Transpose | ● | ● | ● | ● | **Works** by semitones |
| Transpose within the key | ● | ● | ● | ● | **Missing** |
| Snap notes to the scale or the chord | ◐ | ● | ● | ● | **Works**, following the key ruler |
| Legato, set or scale length, fix overlaps | ● | ● | ● | ● | **Works**: legato, set, add and scale length (bug 7, fixed) and Fix Overlaps in the MIDI Operations window, on the selected notes |
| Remove duplicate notes | ● | ● | ● | ● | **Works** |
| Strum, flam, reverse, invert, half and double time | ◐ | ● | ● | ● | **Partial**: strum (up or down), reverse, half and double time in the MIDI Operations window; no flam or pitch inversion |
| Split notes out by pitch or channel | ● | ● | ● | ● | **Flag only** |
| Rule-based transformer | ◐ | ◐ | ● | ● | **Missing** |

## E. Scales and chords

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Key signature track | ● | ● | ● | ● | **Works** |
| Scale shown in the editor; rows folded to the scale | ◐ | ● | ● | ● | **Partial**: with a key on the Key ruler, rows out of the key are shaded; no folding |
| Notes drawn or dragged stay in the scale | ○ | ● | ● | ● | **Missing** |
| Chord track | ◐ | ● | ● | ● | **Partial**: a chord lane feeds the Band and Cleanup; shown above the editor |
| Chords detected from MIDI or audio | ◐ | ● | ● | ● | **Works** (Detect Chords) |
| Clips follow the chord track when it changes | ○ | ● | ● | ● | **Missing** |
| Chord stamp tool, chord pads | ○ | ◐ | ◐ | ● | **Missing** |
| Wrong notes corrected against key and chords, switchable | ○ | ○ | ○ | ○ | **Works** (MIDI Cleanup, non-destructive) |

## F. Drums and patterns

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Drum editor: one named row per drum, hits without length | ○ | ● | ◐ | ● | **Missing** in the note editor |
| Drum maps and note names from the instrument | ◐ | ● | ● | ● | **Missing**: only C rows are labelled |
| Show only the rows in use | ◐ | ● | ● | ● | **Missing** |
| Step sequencer with pattern clips | ○ | ● | ● | ● | **Works** (Juke Steps) |
| Per-step velocity, probability, repeats, nudge | ○ | ● | ● | ● | **Works**, plus flam, accent and conditions (every Nth, fills) |
| Rows of different lengths, rates and directions | ○ | ● | ● | ◐ | **Works**, up to 64 steps |
| Pattern variations | ○ | ● | ● | ● | **Partial**: fills and conditions only |
| Turn a pattern into ordinary notes | ○ | ● | ● | ● | **Works** |
| Step patterns for melodic instruments | ○ | ● | ● | ● | **Missing**: drum machine only |

## G. Event list and score

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Event list of every kind of event, typed values, insert, filter | ● | ○ | ● | ● | **Partial**: notes only; only velocity can be changed |
| Score view | ● | ● | ● | ● | **Partial**: a read-only grand staff |
| Enter and edit notes in the score | ● | ● | ● | ● | **Missing** |
| Print the score | ● | ● | ● | ● | **Works** |
| Lyrics, layout, parts | ◐ | ◐ | ● | ● | **Missing** |
| MusicXML export and import | ● | ● | ● | ● | **Partial**: export only, to 16ths, with no key signature |

## H. MIDI clips on the timeline

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Create an empty clip; draw one | ● | ● | ● | ● | **Works** |
| Notes shown in the clip | ● | ● | ● | ● | **Works** |
| Clips stay on their bars when the tempo changes | ● | ● | ● | ● | **Works** for MIDI clips on tick-timebase tracks, the default for MIDI and instrument tracks (bug 3, fixed) |
| Track set to follow bars or absolute time | ● | ● | ● | ● | **Works** for MIDI clips: a tick track's clips follow tempo edits, a sample track's stay; audio clips stay in samples on either |
| Split | ● | ● | ● | ● | **Partial**: a note across the cut stays in the left clip and stops at the cut; the right clip gets none of it but starts in the controller state from before the cut (bug 11, fixed) |
| Trim the end; drag it back out | ● | ● | ● | ● | **Works** |
| Trim the start; drag it back out | ● | ● | ● | ● | **Works**: what falls before the new start is hidden, its controllers set the starting state, and dragging back returns it (bug 5, fixed) |
| Join clips | ● | ● | ● | ● | **Partial**: Consolidate works; Heal is for audio only |
| Move, copy, duplicate, repeat | ● | ● | ● | ● | **Works**; Loop and Repeat space MIDI copies on tick tracks in bars (bug 15, fixed) |
| Loop a clip; copies that change together | ◐ | ● | ● | ● | **Partial**: independent copies only |
| Mute, lock, colour, name | ● | ● | ● | ● | **Works** |
| Transpose and velocity offset per clip | ● | ● | ● | ● | **Missing**: per track only |
| Stretch a clip so the notes scale | ● | ● | ● | ● | **Missing** |
| Reverse a clip | ◐ | ● | ● | ● | **Missing** |
| Bounce a clip or selection to audio in place | ● | ● | ● | ● | **Partial**: freeze and commit work on the whole track |

## I. How written MIDI plays

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Notes start on their exact sample | ● | ● | ● | ● | **Works**, never before their clip (bug 8, fixed) |
| Controllers, program and bend chased on a locate | ● | ● | ● | ● | **Works**; poly aftertouch is not |
| A long note already sounding at the play position is started | ● | ● | ● | ● | **Missing** |
| No hanging notes on stop, locate, loop or clip end | ● | ● | ● | ● | **Works** |
| Two overlapping notes of one pitch each get their own end | ● | ● | ● | ● | **Works**: the later note retriggers the voice and the sound lasts to the later end (bug 19, fixed) |
| Each note plays on its own channel | ● | ● | ● | ● | **Works**: any clip with an event off channel 1 plays every event on its channel (bug 9, fixed) |
| Track delay, transpose, velocity and length changed at playback | ● | ● | ● | ● | **Works** (Real-Time Properties), undoable; a negative delay plays early notes before the clip (bug 10, fixed) |
| MIDI effects on a track or clip, rendered to notes on request | ● | ● | ● | ● | **Missing** |
| Dense clips play without cost growing | ● | ● | ● | ● | **Partial**: every block scans every note of each clip under the playhead and walks the tempo map for each |

## J. Tempo and arrangement

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Tempo track: points, ramps, curves; meter changes | ● | ● | ● | ● | **Works** |
| Tempo taken from a selection | ● | ● | ● | ● | **Works** (Identify Beat) |
| Tempo taken from a MIDI performance | ◐ | ◐ | ● | ● | **Missing** |
| Insert and cut time across the song | ● | ● | ● | ● | **Works**: clips, automation, markers, tempo, meter, key and the Band's chord and section lanes move (bug 14, fixed) |
| Arranger sections to reorder the song | ○ | ● | ● | ● | **Missing** for clips; sections drive the Band only |
| Track alternatives for MIDI | ● | ● | ● | ● | **Works** (playlists) |
| Global transpose track | ○ | ● | ● | ● | **Missing** |

## K. Tools that write MIDI

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Virtual players that follow chords and sections | ○ | ○ | ● | ◐ | **Works**: the Band, six players (drums, bass, keys, organ, guitar, bowed) with feels; printed to editable clips |
| Audio to MIDI | ● | ● | ● | ● | **Works**: general, piano, drum and vocal models, and a single-line pitch tracker |
| Drum hits in audio to notes | ◐ | ● | ● | ● | **Partial**: through the drum model; Beat Detective writes no notes |
| Arpeggio, strum and chord-progression writers | ○ | ● | ● | ● | **Missing** |
| MIDI from a text description; a song from a prompt | ○ | ○ | ○ | ○ | **Works** (downloaded models) |
| Loop library of MIDI phrases, previewed at the song tempo | ◐ | ● | ● | ● | **Missing** |

## L. Instruments in the box

What ships with the four, in broad terms: Pro Tools has a workstation (Xpand!2), piano, organ,
synths, a sampler and a drum machine; Studio Pro has a sample player with a library, two synths,
a sampler and a drum sampler; Logic has the largest set (sampler with a large library, several
synths, drum kits and drum machine designer, modelled pianos, organs and strings); Cubase has a
workstation (HALion Sonic), a drum instrument, synths and a sampler track. FreeDAWm has four.

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Piano, electric piano, organ, strings and other sampled instruments | ● | ● | ● | ● | **Missing**; every imported MIDI part plays the synth |
| Analogue-style synth | ● | ● | ● | ● | **Partial**: two oscillators (four waves), one 12 dB low-pass, two envelopes, glide, 16 voices. No LFO, unison, sub, noise, sync, mono mode, key tracking or effects. Mono output |
| Wavetable, FM, granular synths | ◐ | ● | ● | ● | **Missing** |
| Sampler: drop a sound, play it across the keys, loop, slice | ● | ● | ● | ● | **Works**: loop with crossfade, slicing to keys, filter, LFO, tempo-following mode, root detection |
| Sampler: many samples mapped by key and velocity; SFZ or other formats | ● | ● | ● | ● | **Missing**: one sample |
| Drum machine with pads, layers, chokes, per-pad shaping | ● | ● | ● | ● | **Works** (Juke): 16 pads, four velocity layers and round robins, pad synth, sends and bus effects |
| Drum kits supplied | ● | ● | ● | ● | **Works**: 28 kits made by the pad synth; recorded kits are a separate download |
| Separate mixer channels per pad | ● | ● | ● | ● | **Missing** |
| Factory presets for instruments | ● | ● | ● | ● | **Missing** for the synth and sampler; Juke has its own |
| Custom editors | ● | ● | ● | ● | **Partial**: Juke and the sampler; the synth is a list of sliders |
| Layer and split several instruments on one track | ◐ | ● | ● | ● | **Missing** |

## M. Instrument tracks and hosted instruments

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Swap the instrument, keep the notes | ● | ● | ● | ● | **Works** |
| Track presets: instrument plus effects | ● | ● | ● | ● | **Works** |
| Automate instrument parameters | ● | ● | ● | ● | **Missing**, for built-in and hosted instruments alike |
| Instrument latency compensated | ● | ● | ● | ● | **Works** (bug 23, fixed) |
| Freeze with the release tail | ● | ● | ● | ● | **Works** |
| Mix bounce keeps the last note's tail | ● | ● | ● | ● | **Missing**: the bounce ends at the last clip |
| Third-party instruments with state saved in the session | ● | ● | ● | ● | **Works** (CLAP, VST3, AU) |
| Their editor windows | ● | ● | ● | ● | **Partial**: not for AU |
| Per-note expression sent to plug-ins | ◐ | ● | ● | ● | **Missing**: no note ids or note expressions |
| Parameter moves in the plug-in window recorded as automation | ● | ● | ● | ● | **Missing** |
| A crashing plug-in does not take the session down | ○ | ○ | ◐ | ○ | **Partial**: scanning is isolated; running is not |
| Preview an instrument preset or a MIDI file from the browser | ◐ | ● | ● | ● | **Missing** |

## N. Getting MIDI in and out

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Import and export standard MIDI files | ● | ● | ● | ● | **Works**; see [section 6](06-midi-recording.md#l-files) and bugs 16 and 17 |
| Drag a .mid file in; drag a clip out as .mid | ● | ● | ● | ● | **Partial**: a `.mid` dropped on the desktop window imports (the first version of this page said Missing: it did, at the session start; now at the insertion point); the browser imports on double-click; dragging a clip out is **Missing** |
| DAWproject | ○ | ● | ○ | ● | **Works**, notes and controllers both ways |
| Copy and paste between sessions | ● | ● | ● | ● | **Missing**: the clipboard lives in one session |

## Bugs found on the way

Found while reading; none was reproduced by running the program. Paths are under `crates/`.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed.** Quantize and Legato from the editor ignored the note selection and changed every note in the clip. `event.quantize`, `event.change_duration`, `event.transpose` and `event.change_velocity` take `indices` (one clip) and act on those notes only, returning where they are after the re-sort; the editor passes its selection (every note when none is selected) and keeps it on the same notes. Legato on a selection runs each picked note to the next picked one. Tests: `quantize_and_legato_touch_only_the_picked_notes` (engine), `quantize_acts_on_the_selected_notes_only` (`ui-egui/tests/midi_quantize.rs`) | `ui-egui/src/midi_editor.rs` (`run_on_selection`); `engine/src/cmd/event.rs` (`midi_op`) |
| 2 | **Fixed.** Quantize counted its grid and its swing from the clip's start while the editor draws its grid from the bar. The grid and swing now count from each bar's start (`ops::quantize_on`); a note in the clip is not moved before its start. Controller points clicked into a lane snap to the bar-counted grid too. Tests: `quantize_on_counts_the_grid_and_swing_from_each_span` (`midi/tests/ops.rs`), `quantize_counts_the_grid_from_the_bar` (engine) | `midi/src/ops.rs`; `engine/src/cmd/event.rs`; `ui-egui/src/midi_lanes.rs` |
| 3 | **Fixed.** A MIDI clip's start and length were in samples and tempo edits never moved them. When a command changes the tempo map, every MIDI clip on a tick-timebase track that the command left in place is moved from its ticks in the old map to the same ticks in the new one (start and end, in fractional ticks), so it stays on its bar and its last notes stay inside it; one undo step with the tempo edit. Clips on sample-timebase tracks, audio clips, and clips the command moved itself (Insert/Cut Time, Move Song Start) are left alone; undo, redo and loading move nothing, so an old session opens and sounds as saved; a frozen MIDI track already refuses tempo edits. Tests: `engine/src/tick_clips.rs` | `engine/src/tick_clips.rs`; `engine/src/lib.rs` (`execute`) |
| 4 | **Fixed.** Notes moved, copied, added or edited past the clip's end grow the clip to the bar line after them, up to the next clip on the playlist (and not past a day); an edit-locked clip does not grow. Notes left past a clip's end by a trim stay hidden, as before. Test: `notes_put_past_the_end_grow_the_clip_up_to_the_next_clip` | `engine/src/cmd/midi.rs` (`grow_to_fit`) |
| 5 | **Fixed.** Trimming a clip's start deleted the notes and controllers before it. They are now kept at negative ticks: hidden, not played, and back when the start is dragged out again; the controllers among them set the state the clip starts in (sent at its start, chased after a locate). The editor, score, ghosts and the timeline show only the clip's window; Select All picks only visible notes; MIDI export writes the window; a copied part keeps only what reaches into it plus the controller state. A note sounding across the new start is hidden with the rest (playing it from the middle is the tier-2 item). Sessions saved before load as they were. Tests: `trimming_the_start_hides_notes_and_dragging_back_returns_them` (engine), `notes_hidden_by_a_start_trim_are_silent_and_their_controllers_set_the_start_state` (mix) | `engine/src/edit.rs` (`rebase_midi_start`, `crop_hidden_midi`); `mix/src/lib.rs` (`schedule_notes`, `chase_channel`); `engine/src/io.rs` (`export_midi`) |
| 6 | **Fixed.** Notes dragged or copied left past the clip's start now stop as a group with the earliest at the start; a move, copy or Transpose past either end of the keyboard takes only as much of the shift as every note can, so chords keep their shape (`ops::fitting_shift`, `ops::transpose_within`). Tests: `moves_past_the_start_or_the_keyboard_keep_the_shape`, `transpose_within_keeps_the_shape` | `engine/src/cmd/midi.rs` (`fitting_move`); `midi/src/ops.rs` |
| 7 | **Fixed.** Event › Quantize… and Change Duration… (and their shortcuts) open the MIDI Operations window on that operation (grid with triplets and dotted values, strength and swing; legato, set, add or scale), acting on the editor's selected notes when it shows some. `event.quantize` reads any note value (`1/4t`, `1/8d`, `1/16.`, `1/8q`) and refuses one it cannot read; `event.change_duration` and `event.change_velocity` with no values are refused instead of applying legato or nothing. Test: `quantize_and_change_duration_menu_items_ask_before_acting` | `ui-egui/src/ops_windows.rs`; `ui-egui/src/menus.rs`; `engine/src/cmd/event.rs` (`note_value_ticks`) |
| 8 | **Fixed.** A note at the start of a clip that does not begin on a tick was placed a few samples before it and skipped when playback started there. Events in the clip are never placed before its start. Test: `a_note_at_the_start_of_a_clip_off_the_tick_grid_plays_from_the_start` | `mix/src/lib.rs` (`schedule_notes`) |
| 9 | **Fixed.** A clip with any note or controller set off channel 1 (the editor's Channel field, a multi-channel file) plays every event on its own channel, as recorded and Band-printed clips do; clips all on channel 1 play exactly as before. Built-in instruments ignore the channel, so they sound the same; a hosted multi-timbral instrument now hears the parts on their channels. Test: `a_note_set_off_channel_1_plays_on_its_channel` | `mix/src/lib.rs` (`plays_channels`) |
| 10 | **Fixed** (the parts in this section's scope). A Real-Time Properties change is an undo step (a slider drag is one), so undo no longer reverts it unannounced. A negative delay plays each early note before the clip's start, at its own time, instead of piling them on the start. MIDI Track Offsets stay a setting (section 6's bug 16 covers that menu item). Tests: `rtp_and_offsets_are_settings`, `a_negative_delay_moves_early_notes_before_the_clip_rather_than_piling_them` | `engine/src/cmd/event_more.rs`; `mix/src/lib.rs` |
| 11 | **Fixed.** The right half of a split starts with the last value of each controller from before the cut (program, pedal, bend), hidden before its start. Test: `split_halves_keep_the_controller_state_and_their_original_performance` | `engine/src/edit.rs` (`split_clip`) |
| 12 | **Fixed.** The stored original performance is shifted with every start trim, and a split gives the right half its own copy in its own time, so Restore Performance puts the notes back where they were. Same test as 11 | `engine/src/edit.rs` (`shift_originals`, `separate_at`) |
| 13 | **Fixed.** Consolidate merges a clip on Cleaned as it sounds: its cleaned notes, leaving the cleanup behind. Test: `consolidate_keeps_what_a_cleaned_clip_plays` | `engine/src/edit.rs` (`copy_range_with`) |
| 14 | **Fixed.** Tempo, meter and key changes already moved (section 4's bug 7). The Band's chord and section lanes now move too: after the insertion point by the inserted time, a block across it grows; Cut Time removes what was inside the cut and closes up. Test: `insert_and_cut_time_move_the_band_lanes` | `engine/src/cmd/event.rs` (`insert_time`, `cut_time`) |
| 15 | **Fixed.** Clip › Loop places each copy of a MIDI clip on a tick track a whole clip length in ticks after the one before, so copies stay on the bars under a changing tempo. Edit › Repeat does the same on a tick track: each copy starts a whole selection length in ticks after the one before, and the MIDI clips in it are placed and sized by their ticks (audio clips keep their offset in samples; a sample-timebase track repeats in samples as before). Tests: `midi_loops_on_a_tick_track_stay_on_the_bars`, `midi_repeats_on_a_tick_track_stay_on_the_bars` | `engine/src/cmd/clip.rs` (`loop_clips`); `engine/src/cmd/edit.rs` (`repeat_in_ticks`) |
| 16 | **Fixed** (with section 6's bug 21). MIDI export writes what plays: no muted clips, no notes hidden before a clip's start or past its end (a note is cut at the end), a Cleaned clip's cleaned notes, the Real-Time Properties and the MIDI Track Offset applied. Stored channels are written, and now also play (bug 9). Test: `smf_export_writes_what_plays` | `engine/src/io.rs` (`export_midi`) |
| 17 | **Fixed.** A MIDI file imported at a later position brings its tempo and meter changes there, after its own tick 0, instead of rewriting the song from its start; the browser imports at the insertion point and recognises `.MID`, `.midi`, `.smf` and `.kar` in any case. Tests: `a_midi_file_imported_later_brings_its_tempo_with_its_notes`, `the_browser_knows_midi_files_in_any_case` | `engine/src/io.rs` (`import_midi_bytes`); `ui-egui/src/windows.rs` (`is_midi_file`) |
| 18 | **Not changed: needs a design.** A musician input plays instead of the track's clips, so keeping it after a partial print would silence the printed clip, and switching it off silences the rest of the song; a test pins today's behaviour (nothing plays outside a printed block). The fix is a mix change: a musician that plays around printed clips | `engine/src/cmd/band.rs:1321-1371`; `mix/src/lib.rs` |
| 19 | **Fixed.** Overlapping notes of one pitch share one voice: a note starting over another retriggers it (a note-off first, so every note-on has its own note-off) and only the last to end closes it, so the first note-off no longer cuts the second note short. Test: `overlapping_notes_of_one_pitch_each_get_their_own_end` | `mix/src/lib.rs` (`schedule_notes`) |
| 20 | **Fixed.** The lanes show one channel (MIDI Editor › More › Lanes › Channel) or every channel, each channel its own stepped line (the one new points go on drawn bright, the others faint); clicks and Pencil passes write the shown channel, or with every channel shown the clip's main channel (the one most of its notes use), and a Pencil pass replaces only that channel's events. Test: `lanes_show_and_write_one_channel` | `ui-egui/src/midi_lanes.rs` (`events_of`, `main_channel`) |
| 21 | **Fixed.** Dragging a velocity in the event list is one undo step (`execute_merged`, ended when the drag stops). Not covered by a test: it needs a pointer drag in the window | `ui-egui/src/windows.rs` (`event_list`) |
| 22 | **Fixed.** The D above middle C (and the B below it) sat next to a ledger line; only middle C takes the ledger between the staves. Test: `only_middle_c_takes_the_ledger_between_the_staves` | `ui-egui/src/score_editor.rs` (`ledgers`) |
| 23 | **Fixed.** An instrument's own latency is compensated as an insert's is (the built-in instruments report none, so they are unchanged; a frozen track plays its render). Test: `an_instruments_latency_is_compensated` | `mix/src/lib.rs` (`process_strip`) |
| 24 | **Fixed.** A CLAP plug-in whose note port takes only CLAP notes hears pitch bend (as tuning, ± 2 semitones), channel and poly pressure, mod wheel (vibrato), CC7 (volume), CC10 (pan), CC11 (expression) and CC74 (brightness) as note expressions on the sounding notes; a note that starts later gets its channel's current values right after its note-on. Other controllers (sustain among them) and program changes have no CLAP note equivalent and are still not sent to such plug-ins. CLAP note-offs carry the note's release velocity. Plug-ins that take MIDI are unchanged. Tests: `channel_messages_become_note_expressions`, `note_offs_and_expressions_reach_clap_whole` (not run against a real note-only plug-in) | `clap-host/src/plugin.rs` (`expression_of`); `clap-host/src/ffi.rs` |
| 25 | **Fixed.** A one-shot that loops is released by its note-off (a loop has no end to play to); one-shots without a loop still play through. Audible change: such voices now stop at the note-off and release. Test: `a_looping_one_shot_is_released_by_its_note_off` | `dsp/src/plugins/sampler.rs` (`begin`) |
| 26 | **Fixed.** Juke keeps up to 512 delayed hits waiting (humanise, flams, ratchets) instead of 64. A stolen voice (and, in the sampler's mono mode, a voice a new note cuts off) no longer stops dead: what it was playing carries on as an offset that fades with a 1.5 ms time constant, in the sampler, Juke (its sends too) and the drum synth. Tests: `a_stolen_voice_fades_instead_of_clicking` (sampler, Juke, drums) (commit 451e7b4) | `dsp/src/plugins/sampler.rs:771-776`; `juke.rs:577-585, 707-716`; `drums.rs:172-185` |
| 27 | **Partly fixed.** The triangle (synth and signal generator) has PolyBLAMP corners: the folded harmonics in the lower half of the band are 20 dB or more quieter. The drum synth's six metal squares (hats, cymbals) are PolyBLEP squares. Still open: a sample pitched up in the sampler or Juke is read with no filtering before resampling. Tests: `the_triangle_is_band_limited`, `the_metal_squares_are_band_limited` (commit 451e7b4) | `dsp/src/osc.rs:74-79`; `plugins/drums.rs:294-301`; `juke.rs:981-1005`; `sampler.rs:446-466` |
| 28 | **Fixed.** The synth's oscillator mix, resonance, detune and filter-envelope amount are smoothed (20 ms) and settle exactly on the value, so a steady sound is unchanged. The sampler's filter glides to each 16-frame control value instead of stepping to it. Tests: `mix_resonance_detune_and_envelope_amount_are_smoothed`, `the_cutoff_glides_between_control_updates` (commit 451e7b4) | `dsp/src/plugins/synth.rs`; `sampler.rs:908-914` |
| 29 | Work on the audio thread that can stall it: a built-in instrument added while playing is created and sized there, and replacing a sampler's sound frees the old one there | `mix/src/lib.rs:1510-1517, 1637-1645, 4043-4058`; `dsp/src/plugins/sampler.rs:1201-1212` |
| 30 | **Partly fixed.** The synth and the drum synth report tails that reach silence (their times are "to -60 dB"; the voices ring on to -100 and -80 dB), and the drum synth counts every drum's decay, not only the kick's and the crash's: freezes and bounces made from now on keep the whole tail. Juke's stereo pads and replay humanise remain. Test: `reported_tails_cover_the_release` | `dsp/src/plugins/synth.rs`; `drums.rs` (`tail_samples`); `juke.rs:805-810, 1046-1058, 1187-1193` |
| 31 | **Confirmed and fixed.** A note moved to the largest start (a script can) overflowed `start + length` in the editor and on the timeline: a panic in debug builds (overflow checks), a wrapped value in release. Those sums saturate now. Test: `a_note_at_the_largest_start_does_not_overflow_the_editor` | `ui-egui/src/midi_editor.rs`; `ui-egui/src/edit_window.rs` |

## Suggested order of work

Estimates are agent-hours for one implementer with tests, as in the earlier sections.
Instruments need listening; no estimate includes it.

**1. Fix what is wrong (≈ 30–48 h).** Bugs 1 and 2 first: quantize is the most used operation
and it changes notes the user did not select. Then 5 and 4 (edits that lose or hide notes), 7
(menu items), 3 (clips that follow bars: the largest single fix, 8–12 h, and it settles 8 and
15), 9, 10 and 31. The instrument bugs (23–30) can go to whoever takes the instrument work.

**2. What all four have (≈ 150–240 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Selection tools: by key, by rule, invert; note mute; keys for length | 5–8 | |
| Notes past the clip's edge: grow the clip or show and play them | 3–5 | Proper fix for bug 4 |
| Quantize done fully: window, ends, groove templates, groove from a clip, playback quantize with the original always kept | 12–18 | |
| Operations with dialogs: length, overlaps, humanise, strum, reverse, half and double time, in-key transpose, split by pitch | 8–12 | Most exist in the library |
| Controller lanes: lines, curves, ramps between points, scale and thin, poly aftertouch and program change, per-channel, move with notes | 10–16 | |
| Edit several clips at once | 6–10 | |
| Scale shown and enforced in the editor; chord stamp; clips follow the chord track | 10–16 | |
| Drum editor view with drum maps and names from the instrument; show used rows | 8–12 | |
| Event list for every event with typed values | 5–8 | |
| Score: enter and edit notes, key signature, MusicXML import | 14–22 | |
| Clips: shared copies, stretch, reverse, per-clip transpose and velocity, bounce a selection in place, heal | 12–18 | |
| Playback: start long notes already sounding, per-pitch counting, per-note channel, an index so dense clips cost little | 6–10 | Bugs 9 and 19 |
| Automation of instrument parameters; instrument latency compensated; moves in a plug-in window recorded | 8–12 | Bug 23 |
| Synth brought up to a usable standard: LFOs, filter types and slopes, unison, sub and noise, mono mode, stereo, presets, an editor | 14–22 | |
| Sampler with many zones, SFZ import, sustain and bend; presets | 12–18 | Sustain pedal and pitch bend (± 2 semitones, as the synth) done; no bend-range setting or mod wheel yet (commit 451e7b4) |
| A sampled piano and a small general library | 8–14 | Needs samples the project may ship; owner's call |
| Arranger sections for clips; insert and cut time that move everything; global transpose | 8–14 | Bug 14 |
| Drag .mid in and out; preview of presets and MIDI files; paste between sessions | 6–10 | |

**3. What most have (≈ 45–75 h).** Per-note probability, 3–5; per-note expression editing and
note expressions to plug-ins, 12–18; MIDI effects on tracks with render to notes (counted in
[section 6](06-midi-recording.md)); arpeggio and progression writers, 6–10; step patterns for
melodic instruments and pattern variations, 6–10; separate outputs per pad, 4–6 (with section
6's multi-output work); wavetable or FM synth, 12–20; a rule-based transformer (Logic, Cubase),
6–10; a MIDI phrase library, which needs content.

**Where FreeDAWm is already level or ahead:** every note edit is a scriptable, undoable command;
the step sequencer has probability, ratchets, flams, conditions and rows of different lengths;
MIDI Cleanup corrects against the key and chords and can be switched off again; the Band writes
six parts from a chord and section chart and prints them as editable clips; audio becomes MIDI
through four models; MIDI can be written from a text description; DAWproject carries notes and
controllers both ways.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Feature entries for the four come from
their manuals as known, not from a side-by-side run; treat a ◐ as "check before relying on it".
What instruments count as "in the box" differs by edition and bundle.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | major | First version: writing MIDI against Pro Tools, Studio Pro, Logic Pro and Cubase |
| 2026-11-16 | minor | Bugs 1 and 2 fixed: Quantize and Legato act on the selected notes, on the grid counted from the bar |
| 2026-11-16 | minor | Bugs 4, 5, 6, 11, 12 and 13 fixed: a start trim hides notes instead of deleting them, edits past the end grow the clip, moves keep their shape, split halves keep the controller state and their original performance, Consolidate keeps cleaned notes |
| 2026-11-16 | minor | Bug 7 fixed: Quantize… and Change Duration… ask in the MIDI Operations window; unknown grids and empty values are refused |
| 2026-11-16 | minor | Bugs 3 and 15 (Loop) fixed: MIDI clips on tick tracks stay on their bars through tempo edits |
| 2026-11-16 | minor | Bugs 8, 9, 10, 19 and 23 fixed: notes never before their clip, channels kept, Real-Time Properties undoable with a working negative delay, overlapping notes retrigger, instrument latency compensated |
| 2026-11-16 | minor | Bugs 14, 16 and 17 fixed: the Band's lanes move with Insert/Cut Time, MIDI export writes what plays, MIDI import brings its tempo to where it lands |
| 2026-11-16 | minor | Bugs 21, 22 and 31 (confirmed) fixed; bugs 25, 28 (synth) and 30 (tails) fixed among the instruments; bug 18 needs a mix design |
| 2026-11-16 | minor | Tier 2 begun: selection tools and length keys, scale shading, humanise, strum, reverse, half and double time, fix overlaps; a dropped `.mid` imports at the insertion point (the drag-in row said Missing: corrected) |
| 2026-11-16 | minor | Bugs 26 and 28 fixed, 27 partly (triangle and cymbal oscillators): stolen voices fade, the sampler's cutoff glides; the sampler answers the sustain pedal and pitch bend |
| 2026-11-16 | minor | Bugs 20, 15 (Repeat) and 24 fixed: controller lanes per channel, Repeat in bars on tick tracks, CLAP note-only plug-ins hear bend, pressure and the expression controllers, note-offs carry release velocity |
| 2026-11-16 | minor | Selection and note mute: select by clicking a key, Select by Rule… sets the editor selection, notes can be muted |
