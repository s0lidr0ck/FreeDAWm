# Four-DAW parity, section 7: writing MIDI

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-16 · **Change:** major (first version) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
| Select by clicking a key; invert; select by rule (pitch, velocity, position) | ● | ● | ● | ● | **Missing**; the rule command only counts |
| Split a note, join notes | ● | ● | ● | ● | **Works** |
| Mute notes | ● | ● | ● | ● | **Missing** |
| Hear a note on click and while dragging it | ● | ● | ● | ● | **Partial**: on click, draw and drop, not during the drag |
| Type exact start, length, pitch, velocity | ● | ● | ● | ● | **Works** (note bar) |
| Ghost notes from other tracks | ● | ● | ● | ● | **Works**, per track, read-only |
| Edit several clips or tracks at once | ● | ● | ● | ● | **Missing**: one clip at a time |
| Notes can run past the clip's edge and still play, or the clip grows | ● | ● | ● | ● | **Missing**: they are kept, unseen and silent (bug 4) |
| Every edit undoable, one step per drag | ● | ● | ● | ● | **Works**, except the event list (bug 21) |
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
| Keys: lengthen and shorten | ● | ● | ● | ● | **Missing** |

## C. Velocity and controllers

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Velocity lane; drag one or many; draw a ramp | ● | ● | ● | ● | **Works**; stacked notes are spread so each can be grabbed |
| Velocity shown as note colour | ● | ● | ● | ● | **Works** |
| Scale, compress, limit, randomise velocity | ● | ● | ● | ● | **Partial**: scale has a dialog; limit and randomise are **Flag only** |
| Release velocity | ● | ◐ | ● | ● | **Flag only**: stored; every edit writes the middle value |
| Per-note probability | ○ | ● | ◐ | ◐ | **Missing** in notes; present on pattern steps |
| Lanes for any controller, pitch bend, channel pressure; several at once | ● | ● | ● | ● | **Works** |
| Lanes for poly aftertouch and program change | ● | ● | ● | ● | **Missing**: recorded data of those kinds is not shown |
| Draw freehand | ● | ● | ● | ● | **Works** |
| Line, curve and shape tools; ramps between points | ● | ● | ● | ● | **Missing**: values are stepped |
| Scale or thin a range of controller data | ● | ● | ● | ● | **Missing** |
| Controllers move and copy with their notes | ● | ● | ● | ● | **Missing** |
| Per-note expression (MPE) editing | ◐ | ● | ● | ● | **Missing** |

## D. Operations

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Quantize the selected notes: grid, strength, swing | ● | ● | ● | ● | **Partial**: acts on the whole clip (bug 1) on a clip-relative grid (bug 2) |
| Quantize window, note ends, keep length, random offset | ● | ● | ● | ● | **Flag only** |
| Groove templates; take a groove from audio or MIDI | ● | ● | ● | ● | **Missing** |
| Quantize applied at playback, original kept | ● | ● | ● | ● | **Missing**: quantize rewrites the notes |
| Restore the original performance | ● | ● | ● | ● | **Partial**: only for takes recorded with input quantize, or after an explicit flatten; misaligned after a trim or split (bug 12) |
| Humanise timing | ● | ● | ● | ● | **Flag only** |
| Transpose | ● | ● | ● | ● | **Works** by semitones |
| Transpose within the key | ● | ● | ● | ● | **Missing** |
| Snap notes to the scale or the chord | ◐ | ● | ● | ● | **Works**, following the key ruler |
| Legato, set or scale length, fix overlaps | ● | ● | ● | ● | **Partial**: legato has a button (whole clip); the rest are **Flag only** |
| Remove duplicate notes | ● | ● | ● | ● | **Works** |
| Strum, flam, reverse, invert, half and double time | ◐ | ● | ● | ● | **Missing** |
| Split notes out by pitch or channel | ● | ● | ● | ● | **Flag only** |
| Rule-based transformer | ◐ | ◐ | ● | ● | **Missing** |

## E. Scales and chords

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Key signature track | ● | ● | ● | ● | **Works** |
| Scale shown in the editor; rows folded to the scale | ◐ | ● | ● | ● | **Missing** |
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
| Clips stay on their bars when the tempo changes | ● | ● | ● | ● | **Missing**: the notes follow the tempo, the clip's start and length do not (bug 3) |
| Track set to follow bars or absolute time | ● | ● | ● | ● | **Flag only** |
| Split | ● | ● | ● | ● | **Partial**: a note across the cut stays in the left clip and stops at the cut; the right clip gets none of it and loses the controller state before the cut |
| Trim the end; drag it back out | ● | ● | ● | ● | **Works** |
| Trim the start; drag it back out | ● | ● | ● | ● | **Partial**: notes and controllers before the new start are deleted (bug 5) |
| Join clips | ● | ● | ● | ● | **Partial**: Consolidate works; Heal is for audio only |
| Move, copy, duplicate, repeat | ● | ● | ● | ● | **Works**; copies are spaced in samples, not bars |
| Loop a clip; copies that change together | ◐ | ● | ● | ● | **Partial**: independent copies only |
| Mute, lock, colour, name | ● | ● | ● | ● | **Works** |
| Transpose and velocity offset per clip | ● | ● | ● | ● | **Missing**: per track only |
| Stretch a clip so the notes scale | ● | ● | ● | ● | **Missing** |
| Reverse a clip | ◐ | ● | ● | ● | **Missing** |
| Bounce a clip or selection to audio in place | ● | ● | ● | ● | **Partial**: freeze and commit work on the whole track |

## I. How written MIDI plays

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Notes start on their exact sample | ● | ● | ● | ● | **Works** |
| Controllers, program and bend chased on a locate | ● | ● | ● | ● | **Works**; poly aftertouch is not |
| A long note already sounding at the play position is started | ● | ● | ● | ● | **Missing** |
| No hanging notes on stop, locate, loop or clip end | ● | ● | ● | ● | **Works** |
| Two overlapping notes of one pitch each get their own end | ● | ● | ● | ● | **Partial**: tracked by pitch, so the first note-off ends both |
| Each note plays on its own channel | ● | ● | ● | ● | **Partial**: only in recorded and Band-printed clips; in other clips the channel field changes nothing |
| Track delay, transpose, velocity and length changed at playback | ● | ● | ● | ● | **Works** (Real-Time Properties); not undoable, and a negative delay piles early notes on the clip start |
| MIDI effects on a track or clip, rendered to notes on request | ● | ● | ● | ● | **Missing** |
| Dense clips play without cost growing | ● | ● | ● | ● | **Partial**: every block scans every note of each clip under the playhead and walks the tempo map for each |

## J. Tempo and arrangement

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Tempo track: points, ramps, curves; meter changes | ● | ● | ● | ● | **Works** |
| Tempo taken from a selection | ● | ● | ● | ● | **Works** (Identify Beat) |
| Tempo taken from a MIDI performance | ◐ | ◐ | ● | ● | **Missing** |
| Insert and cut time across the song | ● | ● | ● | ● | **Partial**: clips, automation and markers move; tempo, meter, key and chord lanes stay behind |
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
| Instrument latency compensated | ● | ● | ● | ● | **Missing** (it is for the same plug-in used as an insert) |
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
| Drag a .mid file in; drag a clip out as .mid | ● | ● | ● | ● | **Missing**; the built-in browser imports on double-click |
| DAWproject | ○ | ● | ○ | ● | **Works**, notes and controllers both ways |
| Copy and paste between sessions | ● | ● | ● | ● | **Missing**: the clipboard lives in one session |

## Bugs found on the way

Found while reading; none was reproduced by running the program. Paths are under `crates/`.

| # | Bug | Where |
|---|---|---|
| 1 | Quantize and Legato from the editor ignore the note selection and change every note in the clip | `ui-egui/src/midi_editor.rs:425-427, 476-484`; `engine/src/cmd/event.rs:295-309` |
| 2 | Quantize counts its grid and its swing from the clip's start, the editor draws its grid from the bar. A clip that starts off the grid quantizes to lines that are not the ones shown, with swing on the wrong beats. Controller lanes place points the same way | `midi/src/ops.rs:60-75`; `ui-egui/src/midi_lanes.rs:255, 279` |
| 3 | A MIDI clip's start and length are in samples and tempo edits never move them. After a tempo change the clip is off its bar, and notes that now fall past its end are silently not played | `mix/src/lib.rs:3677, 3726-3728`; `model/src/track.rs:266-268`; `engine/src/cmd/event.rs:201-242` |
| 4 | Moving, pasting or duplicating notes past the clip's end keeps them where they cannot be seen or heard. Score, ghost notes and playback all drop them; MIDI export writes them | `engine/src/cmd/midi.rs:194-208`; `ui-egui/src/midi_editor.rs:497, 605` |
| 5 | Trimming a clip's start deletes the notes and controllers before it, including a note still sounding there and the program change or pedal the rest depends on. Dragging the start back does not return them | `engine/src/edit.rs:63-79` |
| 6 | Dragging notes left past the clip's start piles them all at the start; transposing past the top or bottom piles chords onto one pitch | `engine/src/cmd/midi.rs:202-203`; `midi/src/ops.rs:113-117` |
| 7 | Event › Change Duration from the menu applies legato to the selected clips without asking; Change Velocity with no values does nothing; Quantize from the menu always uses 16ths; a grid the command does not know (triplet quarter, dotted) silently becomes 16ths | `ui-egui/src/menus.rs:949-965`; `engine/src/cmd/event.rs:311-363` |
| 8 | A note at the very start of a clip that does not begin exactly on a tick (slipped, recorded, or the right half of a split) can be placed a few samples before the clip and skipped when playback starts there | `mix/src/lib.rs:3674-3737` |
| 9 | Clip notes are sent without their channel except in recorded and Band-printed clips, so the editor's Channel field changes nothing and multi-channel instruments cannot be addressed from drawn notes | `mix/src/lib.rs:3742` |
| 10 | Real-Time Properties and MIDI Track Offsets change the session without an undo step; undoing an earlier edit silently reverts them. A negative delay plays every early note together at the clip's start | `engine/src/cmd/event_more.rs:107-116`; `mix/src/lib.rs:3724` |
| 11 | A split leaves the right-hand clip without the controller state from before the cut (program, pedal, bend) | `engine/src/edit.rs:36-61` |
| 12 | Restore Performance after a start trim or a split puts the original notes back at the wrong place, because the stored original is not trimmed or split with the clip | `engine/src/cmd/event_more.rs:464-470` |
| 13 | Consolidate takes the raw notes of a clip set to "Cleaned" and drops its cleanup settings | `engine/src/edit.rs:182-219` |
| 14 | Insert Time and Cut Time move clips but leave tempo, meter, key and the Band's chord and section lanes where they were | `engine/src/cmd/event.rs:244-293` |
| 15 | Loop and repeat space their copies in samples, so under a changing tempo they drift off the bars | `engine/src/cmd/clip.rs:340`; `engine/src/cmd/edit.rs:714` |
| 16 | MIDI export does not match what plays: muted clips, notes hidden past a clip's end and stored channels are written; Real-Time Properties, track offsets and Cleanup are not applied | `engine/src/io.rs:536-588` |
| 17 | MIDI import at a position other than the start moves the notes but leaves the file's tempo changes at their original place; the browser imports at the session start though it says "at the insertion point", and does not recognise `.MID` | `engine/src/io.rs:407-451`; `ui-egui/src/windows.rs:480` |
| 18 | Printing the Band switches the track's input to none, even when only part of the song was printed | `engine/src/cmd/band.rs:1321-1371` |
| 19 | Overlapping notes of one pitch are tracked as one, so the first note-off ends both | `mix/src/lib.rs:3755` |
| 20 | New controller points always go on channel 1 and lanes mix every channel's data | `ui-egui/src/midi_lanes.rs:140` |
| 21 | Dragging a velocity in the event list makes one undo step per screen frame | `ui-egui/src/windows.rs:255-256` |
| 22 | The score draws a ledger line through the D above middle C | `ui-egui/src/score_editor.rs:105-106` |
| 23 | Instruments report latency that nothing reads, so a hosted instrument with look-ahead plays late against the other tracks | `mix/src/lib.rs:2753, 4149-4167` |
| 24 | A plug-in that accepts only CLAP note events gets no controllers, bend or pressure; CLAP note-offs always carry release velocity 0 | `clap-host/src/plugin.rs:121-128, 304-308` |
| 25 | Sampler in one-shot mode with a loop on never stops: the voice plays until all notes off | `dsp/src/plugins/sampler.rs:772, 835-838, 1082-1086` |
| 26 | Voice stealing cuts the old voice dead (a click) in the sampler, Juke and the drum synth; Juke drops hits when more than 64 are waiting | `dsp/src/plugins/sampler.rs:771-776`; `juke.rs:577-585, 707-716`; `drums.rs:172-185` |
| 27 | Aliasing: the synth's triangle, the cymbal and cowbell square banks, and any sample pitched up in the sampler or Juke (no filtering before resampling) | `dsp/src/osc.rs:74-79`; `plugins/drums.rs:294-301`; `juke.rs:981-1005`; `sampler.rs:446-466` |
| 28 | Stepping when a synth's mix, resonance, detune or envelope amount is changed; the sampler's cutoff moves in steps of 16 samples | `dsp/src/plugins/synth.rs:423-426`; `sampler.rs:908-914` |
| 29 | Work on the audio thread that can stall it: a built-in instrument added while playing is created and sized there, and replacing a sampler's sound frees the old one there | `mix/src/lib.rs:1510-1517, 1637-1645, 4043-4058`; `dsp/src/plugins/sampler.rs:1201-1212` |
| 30 | Juke mixes a stereo pad sample to mono before panning; instruments under-report their tails; Juke's humanise differs on each live replay | `dsp/src/plugins/juke.rs:805-810, 1046-1058, 1187-1193`; `synth.rs:513-515`; `drums.rs:375-377` |
| 31 | Possible crash, not confirmed: the editor adds a note's start and length without a guard, and a move command allows a start at the largest value | `ui-egui/src/midi_editor.rs:697, 781`; `engine/src/cmd/midi.rs:586-599` |

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
| Sampler with many zones, SFZ import, sustain and bend; presets | 12–18 | |
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
