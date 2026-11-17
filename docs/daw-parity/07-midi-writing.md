# Four-DAW parity, section 7: writing MIDI

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-17 · **Change:** minor (bug fixes and tier-2 items over three rounds, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
3. **Much of what the four offer around notes was absent.** The second round added note mute,
   grooves and playback quantize, a drum view, chord stamping and scale folding, several clips
   in the editor, controller ramps and a full event list; the third round editing across
   clips, ramps that play smoothly, shared clip copies, stretching a clip and per-note
   probability (the rows below say how far each goes); the follow-up round notes edited on the
   track, 14-bit and RPN lanes, tempo from a performance, chord pads and the MIDI writers.
4. **Instruments are few and thin.** Four built in; the synth has two oscillators with a sub and
   noise, five filter types, an LFO, mono and legato play and factory presets; the sampler holds
   one sample (a multisample instrument is being built on another branch). Instrument parameters now take
   automation lanes, instrument latency is compensated and notes from clips keep their channel.

## A. The note editor

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Piano roll opened from a clip, docked or in its own window | ● | ● | ● | ● | **Works** |
| Notes edited directly on the track | ● | ◐ | ○ | ● | **Works**: a MIDI or instrument track showing Notes (their default view) is a small piano roll: its clip's lowest to highest note fill the clip's height. With the Smart, Grabber or Pencil tool a drag on a note moves it in time and pitch (the editor's selected notes with it), a drag on its right end resizes it, a click selects it in the MIDI Editor (Shift adds), Alt-click deletes it, and the Pencil clicked on empty clip space adds a note there; moves snap to the editor's grid from the bar (Alt: free). Each is the editor's own command, one undo step. A double-click still opens the editor. Clip moves are made by the clip's empty space or name bar, or with the track showing Clips (`ui-egui/src/track_notes.rs`) |
| Draw, move, resize, copy by dragging, delete | ● | ● | ● | ● | **Works**; drawing needs the main window's Pencil tool |
| Marquee, shift-click, select all | ● | ● | ● | ● | **Works** |
| Select by clicking a key; invert; select by rule (pitch, velocity, position) | ● | ● | ● | ● | **Works**: a click on a piano key selects that pitch's notes (Shift adds, a drag along the keys adds each key's); Invert Selection, Select Same Pitch and Select by Rule… (pitch, velocity, length and beat-of-the-bar ranges, in the MIDI Operations window) in the editor's More menu |
| Split a note, join notes | ● | ● | ● | ● | **Works** |
| Mute notes | ● | ● | ● | ● | **Works**: Mute Notes / Unmute Notes in the editor's More menu (`midi.notes_set` with `muted`); a muted note is drawn faint, does not play and is left out of MIDI and DAWproject export; sessions saved before load unmuted, and unmuted notes save as before |
| Hear a note on click and while dragging it | ● | ● | ● | ● | **Works**: on click, draw and drop, and while a note is dragged to another pitch (each new pitch sounds as a held piano key does, stopping when it is let go); with Audition on |
| Type exact start, length, pitch, velocity | ● | ● | ● | ● | **Works** (note bar) |
| Ghost notes from other tracks | ● | ● | ● | ● | **Works**, per track, read-only |
| Edit several clips or tracks at once | ● | ● | ● | ● | **Works**: with several MIDI clips selected, the roll spans them all, the others' notes in their track's colour. A marquee, Shift-click, Select All and Ctrl+A select notes in any of them; moving, copying, resizing, nudging, deleting and Quantize (and the MIDI Operations on the selection) act on the selected notes of every clip as one undo step; the Pencil draws into the selected clip under it. A plain click on another clip's note makes it the one the velocity and controller lanes and the note bar show (the Clip menu too); those lanes edit that clip only. Scripts run the per-clip commands |
| Notes can run past the clip's edge and still play, or the clip grows | ● | ● | ● | ● | **Works**: an edit that puts notes past the end grows the clip to the next bar line, up to the next clip (bug 4, fixed); notes a trim left past the end stay hidden |
| Every edit undoable, one step per drag | ● | ● | ● | ● | **Works**, the event list too (bug 21, fixed) |
| Every edit scriptable | ○ | ◐ | ◐ | ◐ | **Works** |

## B. Grid, snap and keys

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Grid from whole notes to 64ths, triplets | ● | ● | ● | ● | **Works** |
| Dotted values, other tuplets | ● | ● | ● | ● | **Works**: the editor's Grid menu takes each value straight, triplet, dotted, quintuplet (5:4) or septuplet (7:4) (`ui.midi_editor` grid ids `1/8d`, `1/16q`, `1/8s`); the lines are counted from each bar |
| Grid that adapts to zoom | ○ | ● | ● | ● | **Works**: Grid › Adapt to Zoom snaps to the finest straight value (1/64 to 1/1) whose lines stay 14 points apart, following each zoom (`auto`) |
| Snap to the grid, or keep the offset from it | ● | ● | ● | ● | **Works**: edges snap to lines; a moved note keeps its offset from the grid, or with More › Snap Moves to the Grid its start lands on a line (Alt: free either way) |
| New note takes the last note's length and velocity | ● | ● | ● | ● | **Works** |
| Zoom, scroll, zoom to the notes | ● | ● | ● | ● | **Works**: Fit shows the clip; Notes zooms to the selected notes (or every note), in time and in pitch (time only on a folded roll) |
| Keys: nudge, transpose by semitone and octave | ● | ● | ● | ● | **Works** |
| Keys: lengthen and shorten | ● | ● | ● | ● | **Works**: Alt+Right and Alt+Left, a grid step |

## C. Velocity and controllers

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Velocity lane; drag one or many; draw a ramp | ● | ● | ● | ● | **Works**; stacked notes are spread so each can be grabbed |
| Velocity shown as note colour | ● | ● | ● | ● | **Works** |
| Scale, compress, limit, randomise velocity | ● | ● | ● | ● | **Works**: Event › More MIDI › Change Velocity… sets, adds, scales, ramps, limits to a range, randomises (± an amount, a new seed each time) and compresses or expands (a percent of each velocity's distance from the notes' average), on the editor's selected notes or the selected clips (`event.change_velocity` and `midi.notes_velocity` `min`, `max`, `random`, `compress`, `center`) |
| Release velocity | ● | ◐ | ● | ● | **Works**: the note bar's Release field shows and sets the selected notes' note-off velocity (`midi.notes_set` `release_velocity`); recorded notes keep theirs, drawn notes take 64, and it goes out with the note-off |
| Per-note probability | ○ | ● | ◐ | ◐ | **Works**: MIDI Editor › More › Probability (0-100 %) on the selected notes (`midi.notes_set` `probability`; the hover shows it). Each pass the clip plays from somewhere new (a start, a locate, a loop turn) rolls every such note again with a generator seeded from the note, its clip and the pass, so a bounce or freeze plays the same notes each time. Sessions saved before load at 100 %, and notes at 100 % save as before. MIDI export writes every note. Pattern steps keep their own |
| Lanes for any controller, pitch bend, channel pressure; several at once | ● | ● | ● | ● | **Works**, per MIDI channel or every channel with a line each (bug 20, fixed) |
| Lanes for poly aftertouch and program change | ● | ● | ● | ● | **Works**: a Program Change lane and a Poly Pressure lane per key (Lanes › Poly Pressure lists the keys with data and the selected notes' keys first); a clip with such data opens with them shown |
| 14-bit controllers and RPN/NRPN shown and edited as one value | ● | ◐ | ● | ● | **Works**: Lanes › 14-bit and RPN shows a 14-bit pair (CC n with CC n+32, for n 0–31) or a registered or non-registered parameter (Pitch Bend Range, Fine and Coarse Tuning, Modulation Depth offered, any other found in the clip) as one lane of 0–16383; a click sets a value at the grid line, a drag draws freehand, Alt-click removes one (`midi.ctrl14s`, `midi.ctrl14_draw`, `midi.ctrl14_clear`). A pair writes both controllers; a parameter its select, Data Entry and then the null RPN, so a later CC 6 is not taken for it. A clip with such data opens with the lanes shown. Playback is unchanged (ordinary controllers; the chase already sends a parameter's select before its value) |
| Draw freehand | ● | ● | ● | ● | **Works** |
| Line, curve and shape tools; ramps between points | ● | ● | ● | ● | **Works**: Lanes › a drag on empty space Draws a Line or Draws a Curve, written as two points with a straight or curved segment between them; right-click › Ramp Between Points replaces the points between the selected ones with such a segment, Step Between Points makes it a held value again. A ramp plays as a value every 32 samples where it changes (and is chased mid-ramp on a locate), is drawn as a line, and goes to MIDI files as steps at most every 10 ticks; DAWproject keeps straight ones as linear points. Sessions saved before load as steps (the field is new and absent from steps). No sine, square or other periodic shapes |
| Scale or thin a range of controller data | ● | ● | ● | ● | **Works**: right-click on selected points › Scale Points (percent) and Thin Points (drops points a straight line passes within 2 of, and repeats); `midi.ctrl_scale` also adds an offset |
| Controllers move and copy with their notes | ● | ● | ● | ● | **Works** as an option (More › Controllers Follow Notes): moving or copying notes takes the controllers from the first note's start to the last one's end along, replacing what was on those lanes and channels where they land |
| Per-note expression (MPE) editing | ◐ | ● | ● | ● | **Partial**: an MPE clip keeps each note on its own channel (section 6), and Lanes › Channel shows and edits one channel's pitch bend, pressure and CC74, which in an MPE clip is that note's expression. Left: lanes tied to the selected note and a per-note expression model (`Note::expr`), as designed below (est. 14–20 h with the plug-in side; needs an MPE controller or a scripted input to test end to end) |

## D. Operations

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Quantize the selected notes: grid, strength, swing | ● | ● | ● | ● | **Works**: the selected notes (every note when none is), on the grid counted from the bar (bugs 1 and 2, fixed); triplet, dotted and quintuplet grids |
| Quantize window, note ends, keep length, random offset | ● | ● | ● | ● | **Works**: window, random offset, note ends and keep lengths in the MIDI Operations window (`event.quantize` `window`, `random`, `ends`, `keep_length`) |
| Groove templates; take a groove from audio or MIDI | ● | ● | ● | ● | **Works**: built-in swung 8ths and 16ths (54 to 66 %), pushed and laid-back 16ths, made in code; Groove from Selected Clip takes a MIDI clip's timing (each grid step where its notes fall, over one bar), kept in the session; Quantize and playback quantize follow either. A groove taken from a clip keeps its accents too (each step's median velocity against the steps' average), which Quantize, playback quantize, Apply and Write to Notes and MIDI export add to the velocity, by the strength; the built-in grooves and grooves saved before are timing only. When a clip plays with a swing or a groove, the editor's drawing grid swings with it (its lines and the snap). From audio too: Groove from Selected Clip on an audio clip takes the timing of its hits (the transients Beat Detective finds, in its first two minutes; `event.groove_from_clip` `sensitivity`), timing only (no accents) (ddff138) |
| Quantize applied at playback, original kept | ● | ● | ● | ● | **Works**: per clip (MIDI Editor › More › Playback Quantize: a grid or a groove) or per track (Real-Time Properties › Quantize, with strength); the notes stay where they are and play, freeze, bounce and export quantized; Apply to Notes and Write to Notes write it and keep the performance for Restore Performance. The editor draws the notes where they are written, and an outline where each one playback quantize moves plays |
| Restore the original performance | ● | ● | ● | ● | **Works** with playback quantize (turn it off), after applying it (Apply to Notes, Write to Notes), for takes recorded with input quantize and after an explicit flatten; kept in place through trims and splits (bug 12, fixed). A destructive Quantize is undone with Undo |
| Humanise timing | ● | ● | ● | ● | **Works**: timing and velocity, seeded, in the MIDI Operations window |
| Transpose | ● | ● | ● | ● | **Works** by semitones |
| Transpose within the key | ● | ● | ● | ● | **Works**: MIDI Operations › Transpose › In the Key moves the notes by steps of the key on the Key ruler at each clip's start (else the band's key); a note out of the key keeps its distance from the key's tone below it, and a chord stops as a whole at the keyboard's ends (`event.transpose` `steps`) |
| Snap notes to the scale or the chord | ◐ | ● | ● | ● | **Works**, following the key ruler |
| Legato, set or scale length, fix overlaps | ● | ● | ● | ● | **Works**: legato, set, add and scale length (bug 7, fixed) and Fix Overlaps in the MIDI Operations window, on the selected notes |
| Remove duplicate notes | ● | ● | ● | ● | **Works** |
| Strum, flam, reverse, invert, half and double time | ◐ | ● | ● | ● | **Works**: strum (up or down), flam (a grace note of the same pitch a set number of ticks before each note, at a share of its velocity; `event.flam`), reverse, invert pitches (mirrored around the middle of their range, or a given pitch; `event.invert_pitch`), half and double time in the MIDI Operations window |
| Split notes out by pitch or channel | ● | ● | ● | ● | **Works**: MIDI Operations › Select Notes picks notes by pitch, velocity, length, place in the bar and channel, and Move Them to a New Track splits them out (`event.select_split_notes` `action: split`, `channel_from`, `channel_to`); the first version of this page said Flag only, but the pitch split was already built |
| Rule-based transformer | ◐ | ◐ | ● | ● | **Partial**: for notes. MIDI Operations › Select Notes is a transformer: the notes matching every condition (pitch, velocity, length, start, beats of the bar, channel ranges) are selected, moved to a new track, deleted, muted or unmuted, or changed (velocity scaled and offset, transposed, length scaled or set, channel set): `event.select_split_notes` `action: select|split|delete|mute|unmute|change`, one undo step, on the editor's clip or the selected clips. Not for controller events, no conditions other than ranges (no "not", no either/or), and rules are not saved by name (a9dec8e) |

## E. Scales and chords

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Key signature track | ● | ● | ● | ● | **Works** |
| Scale shown in the editor; rows folded to the scale | ◐ | ● | ● | ● | **Works**: with a key on the Key ruler, rows out of the key are shaded, and More › Fold to Scale keeps only the scale's rows (and those of pitches with notes) |
| Notes drawn or dragged stay in the scale | ○ | ● | ● | ● | **Works** as an option (More › Keep to Scale): the Pencil and a vertical drag land on the nearest pitch of the key at the clip's start |
| Chord track | ◐ | ● | ● | ● | **Partial**: a chord lane feeds the Band, Cleanup, Write Progression and MIDI clips that follow it; shown above the editor. Audio following the chord track is **not planned here**: it needs polyphonic note editing of audio (Studio Pro's Harmonic Editing, Melodyne-style detection), which FreeDAWm does not have; monophonic audio can be moved by hand in the Vocal Editor |
| Chords detected from MIDI or audio | ◐ | ● | ● | ● | **Works** (Detect Chords) |
| Clips follow the chord track when it changes | ○ | ● | ● | ● | **Works** per clip, opt in (More › Follow Chord Lane, `clip.follow_chords`): the clip keeps the chords it was written against; when the chord lane or the band key changes, its chord tones go to the nearest tone of the new chord and other notes move as the root does, in the same undo step. Destructive (the notes are rewritten); the Band is unchanged |
| Chord stamp tool, chord pads | ○ | ◐ | ◐ | ● | **Works**: More › Chord Stamp makes the Pencil draw a chord on the note clicked (major, minor, 7, maj7, m7, sus4, dim, aug, or the triad in key). Event › More MIDI › Chord Pads… opens eight pads from the key on the Key ruler at the playhead (its seven triads, I ii iii IV V vi vii° in major, and V7), an octave setting; holding a pad plays its chord on the Musical Typing input, so armed or monitored instrument tracks hear it and a take records it. Pads are not yet played from a MIDI keyboard's keys |
| Wrong notes corrected against key and chords, switchable | ○ | ○ | ○ | ○ | **Works** (MIDI Cleanup, non-destructive) |

## F. Drums and patterns

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Drum editor: one named row per drum, hits without length | ○ | ● | ◐ | ● | **Works** in the note editor for Juke, the drum synth and any track with a drum map (More › Drum View, on by default there): a named row per drum, hits drawn as diamonds on their start and moved, not resized |
| Drum maps and note names from the instrument | ◐ | ● | ● | ● | **Works**: Juke's pad names on their trigger notes, General MIDI names for the drum synth, and a drum map per track (MIDI Editor › More › Drum Map: name the selected notes' pitch, fill in General MIDI names, clear; `track.drum_map`, saved with the track) whose names override the instrument's and give any instrument, hosted ones too, the drum view. Names only: a map does not send a drum to another note (no input/output note pairs) and does not reorder the rows (5817ba3) |
| Show only the rows in use | ◐ | ● | ● | ● | **Works**: the drum view shows the rows with hits (every named drum while the clip is empty, or with More › Every Drum Row) |
| Step sequencer with pattern clips | ○ | ● | ● | ● | **Works** (Juke Steps) |
| Per-step velocity, probability, repeats, nudge | ○ | ● | ● | ● | **Works**, plus flam, accent and conditions (every Nth, fills) |
| Rows of different lengths, rates and directions | ○ | ● | ● | ◐ | **Works**, up to 64 steps |
| Pattern variations | ○ | ● | ● | ● | **Partial**: fills and conditions only. Left: up to eight variations (A–H) per pattern clip and a per-bar switch lane (design below, est. 4–6 h); not reached |
| Turn a pattern into ordinary notes | ○ | ● | ● | ● | **Works** |
| Step patterns for melodic instruments | ○ | ● | ● | ● | **Partial**: the drum machine's step sequencer; for a melodic part the MIDI Editor's grid and Write Arpeggio. Left: a step-pattern clip whose steps hold a pitch, on any instrument (design below, est. 4–6 h); not reached |

## G. Event list and score

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Event list of every kind of event, typed values, insert, filter | ● | ○ | ● | ● | **Works**: notes, CC, pitch bend, aftertouch, poly pressure and program changes in time order; position (written when the drag or typing ends), channel, note or number, velocity or value and length typed; an Insert row for each kind at the insertion point; filters by kind and channel. No SysEx (the model keeps none) |
| Score view | ● | ● | ● | ● | **Works**: a grand staff with heads by length, stems and flags, ledger lines, bar lines from the meter map, flats in a flat key (sharps otherwise); notes entered and edited there (next row). No key signature, beams, ties or rests drawn |
| Enter and edit notes in the score | ● | ● | ● | ● | **Works** as designed below: a value row (whole to 32nd, dotted, triplet, ♯ ♭ ♮ for the next note); a click on empty staff adds a note of that value snapped to the value's bar-counted grid, its pitch the line's or space's note in the key on the Key ruler (an F in G major is F♯); a click on a head selects it (Shift adds; the piano roll's selection); over the staff, Delete removes, Up and Down move a step of the key (Alt: a semitone), 1–6 choose the value and set the selected notes' length, A–G enter the nearest such note at the cursor and move it on, R rests. Each is one undo step through the piano roll's commands. No vertical drag of a head, and step entry from a MIDI keyboard is not wired |
| Print the score | ● | ● | ● | ● | **Works** |
| Lyrics, layout, parts | ◐ | ◐ | ● | ● | **Partial**: lyrics work. A MIDI clip's lyrics (typed in the Lyrics panel, imported with a MIDI or MusicXML file) are sung under the notes they fall on in the printed score and the MusicXML export (each word on the note starting nearest it, within an eighth; `lyric` elements) (2ac9936). Parts: with File › Score Setup… › Print each track as a part on its own page, Print Score writes a page per track, titled with its name; Staff size (70–150 %) scales the page. Left: page size and breaks at sections, lyrics in the score view, and beams, ties and rests in the score view (est. 6–8 h) |
| MusicXML export and import | ● | ● | ● | ● | **Works**: export to 16ths. The key at the start (the Key ruler's, else the band's) is written as the key signature, and each later key change on the Key ruler as a new signature in the measure it falls in (a change up to a 16th after the bar line counts as on it); the notes are spelled for the key in force (flats in a flat key) with accidentals only where they leave it. Import (File › Import › MusicXML…, `file.import_musicxml`): a partwise `.musicxml`/`.xml` file at the insertion point, a track per part with one clip (chords, ties, voices through `backup` and `forward`, any `divisions`), its meters and `sound tempo` marks on the tempo map, its key signatures on the Key ruler and its first verse's words as the clip's lyrics; an unpitched part plays its drums on the drum synth through its `midi-unpitched` numbers. Not read: compressed `.mxl`, timewise scores, dynamics, articulations, repeats and grace notes (8135515, 0d5f801) |

## H. MIDI clips on the timeline

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Create an empty clip; draw one | ● | ● | ● | ● | **Works** |
| Notes shown in the clip | ● | ● | ● | ● | **Works** |
| Clips stay on their bars when the tempo changes | ● | ● | ● | ● | **Works** for MIDI clips on tick-timebase tracks, the default for MIDI and instrument tracks (bug 3, fixed) |
| Track set to follow bars or absolute time | ● | ● | ● | ● | **Works** for MIDI clips: a tick track's clips follow tempo edits, a sample track's stay; audio clips stay in samples on either |
| Split | ● | ● | ● | ● | **Works**: by default a note across the cut stays in the left clip and stops at the cut, and the right clip starts in the controller state from before the cut (bug 11, fixed). With MIDI Editor › More › Split Notes at the Cut (`midi.split_notes`, a session setting, off by default) the note is cut in two: the left part ends at the cut and the right clip starts with the rest (same velocities, channel and release velocity), so both halves sound it; this holds for every edit that splits (Separate, a range cut or clear). Heal Separation joins the clips but leaves the two parts as two notes (1d9783f) |
| Trim the end; drag it back out | ● | ● | ● | ● | **Works** |
| Trim the start; drag it back out | ● | ● | ● | ● | **Works**: what falls before the new start is hidden, its controllers set the starting state, and dragging back returns it (bug 5, fixed) |
| Join clips | ● | ● | ● | ● | **Works**: Consolidate merges any MIDI clips; Edit › Heal Separation joins neighbouring MIDI clips whose settings match (transpose, velocity, mute, playback quantize and the rest; not shared copies, patterns, clips under Cleanup or with a stored original performance), so a split heals back with the note across the cut whole; what either hid past the join is dropped |
| Move, copy, duplicate, repeat | ● | ● | ● | ● | **Works**; Loop and Repeat space MIDI copies on tick tracks in bars (bug 15, fixed) |
| Loop a clip; copies that change together | ◐ | ● | ● | ● | **Works**: Clip › Duplicate as Shared Copy (`clip.share`) puts a copy after each selected MIDI clip that shares its notes and controllers (marked `=` on the timeline); a note edit in any (the editor, the event list, the MIDI Operations, Reverse, Apply to Notes) is made in all, in the same undo step, and Duplicate or paste of a shared clip shares too. Each copy keeps its own place, length, mute, transpose and velocity. Clip › Make Independent ends it; an edit that changes one copy's notes some other way (a start trim, a split, Consolidate) leaves that copy on its own. Loop still makes independent copies |
| Mute, lock, colour, name | ● | ● | ● | ● | **Works** |
| Transpose and velocity offset per clip | ● | ● | ● | ● | **Works**: MIDI Editor › More › Clip transpose and Clip velocity (`clip.midi_offsets`), applied as the clip plays and in MIDI export, with the track's Real-Time Properties; the notes stay as written |
| Stretch a clip so the notes scale | ● | ● | ● | ● | **Works**: trimming a MIDI clip's edge with Alt held (as for audio; `clip.stretch_to`) scales its notes' starts and lengths and its controllers in ticks from its start, the other edge staying; one undo step. A Juke pattern clip is refused |
| Reverse a clip | ◐ | ● | ● | ● | **Works**: More › Reverse Clip (`clip.midi_reverse`): notes and controllers play backwards in the clip's window, each controller value holding over the mirror of its span; a ramp runs the other way with its curve bent the other way, so the reversed controller curve is the mirror image (it kept each point's ramp on the same point before, so a reversed ramp did not mirror) |
| Bounce a clip or selection to audio in place | ● | ● | ● | ● | **Works**: Track › Commit… › Commit Selection (`track.commit` with `start` and `end`) renders only the edit selection's range to audio, the source track muted there; freeze and commit also take the whole track (the first version of this page said Partial: Commit Selection was already built) |

## I. How written MIDI plays

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Notes start on their exact sample | ● | ● | ● | ● | **Works**, never before their clip (bug 8, fixed) |
| Controllers, program and bend chased on a locate | ● | ● | ● | ● | **Works**; poly aftertouch is not |
| A long note already sounding at the play position is started | ● | ● | ● | ● | **Works** as an option (More › Note Chase, `midi.note_chase`, off by default): when playback starts or locates inside a note, it starts there |
| No hanging notes on stop, locate, loop or clip end | ● | ● | ● | ● | **Works** |
| Two overlapping notes of one pitch each get their own end | ● | ● | ● | ● | **Works**: the later note retriggers the voice and the sound lasts to the later end (bug 19, fixed) |
| Each note plays on its own channel | ● | ● | ● | ● | **Works**: any clip with an event off channel 1 plays every event on its channel (bug 9, fixed) |
| Track delay, transpose, velocity and length changed at playback | ● | ● | ● | ● | **Works** (Real-Time Properties), undoable; a negative delay plays early notes before the clip (bug 10, fixed) |
| MIDI effects on a track or clip, rendered to notes on request | ● | ● | ● | ● | **Partial**: on a track (arpeggiator, chord, scale, note repeat; section 6, H), played live, and rendered to notes on request: Event › More MIDI › Write Arpeggio prints the track's MIDI effects over a clip into its notes, the clip then playing past them. Not on a clip. Left to [section 6](06-midi-recording.md), which owns MIDI effects |
| Dense clips play without cost growing | ● | ● | ● | ● | **Works**: beside each MIDI clip's notes the session keeps a derived index (`Clip::midi_index`, never saved: whether the notes are in start order, the longest note), made by the engine after every command and undo step and when a session is opened, from the notes as they then are (`engine/src/note_index.rs`). The scheduler binary-searches the notes that can sound in the block (start at least the block's first tick less the longest length, the delay and the largest playback-quantize move, a grid step or a groove's period) and stops past its end; the retrigger check looks only at the notes that can sound across that point. The index is used only for the very notes it was made from (their address and count; a hash when it is refreshed), so a copy, a cleanup's output or notes edited outside a command are searched whole, as before. The index covers the controllers too (`NoteIndex::of_sequence`): whether they are in tick order and the longest ramp (from a ramp point to the next point of its lane), so each block looks only at the controllers in it and the ramp points whose ramp can reach into it (`NoteIndex::ctrl_window`). Checked by playing random dense clips (overlaps, mutes, channels, probabilities, delays and lengths, controllers on several lanes and channels with ramps and hidden ones, playback quantize by grid, swing and groove, start trims, locates with Note Chase) block by block with and without their indexes and comparing every message and offset (`indexed_clips_play_event_for_event_as_before`). A controller chase on a locate still reads the clip's controllers once (7ee22e2) |

## J. Tempo and arrangement

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Tempo track: points, ramps, curves; meter changes | ● | ● | ● | ● | **Works** |
| Tempo taken from a selection | ● | ● | ● | ● | **Works** (Identify Beat) |
| Tempo taken from a MIDI performance | ◐ | ◐ | ● | ● | **Works**: Event › More MIDI › Tempo from MIDI Performance (`event.tempo_from_midi`, the selected MIDI clip): the clip's note onsets are tracked as beats (an onset curve, a period from its autocorrelation between 60 and 200 bpm, dynamic programming for steady beats near onsets, each beat moved onto its strongest onset) and a tempo point written at each so they land on the quarter-note grid; the beat or two before the first is stretched to meet it. The clip keeps its samples (its notes are given new ticks), so the performance sounds as played; other clips on tick tracks follow the new tempo. One undo step. Tested on a performance drifting from 100 to 110 bpm (each beat's tempo within 1 bpm). Audio performances: Detect Tempo |
| Insert and cut time across the song | ● | ● | ● | ● | **Works**: clips, automation, markers, tempo, meter, key and the Band's chord and section lanes move (bug 14, fixed) |
| Arranger sections to reorder the song | ○ | ● | ● | ● | **Works**: a block on the Sections ruler (the Band's song lane) is a handle on the song: its right-click menu Copies with the Song (right after it), Moves with the Song (before another section or to the end) and Removes with the Song (`arrange.copy`, `arrange.move`, `arrange.remove`). Every track's clips (the playlist shown) and automation, the Band's chords and the block go with it; later material makes room or closes up as with Insert and Cut Time; one undo step. A section that changes tempo or meter inside is refused, and a copy goes only where the same tempo and meter are in force (the material is copied in samples). Markers, key changes and take layers other than the one shown are not copied (c8cfbb1) |
| Track alternatives for MIDI | ● | ● | ● | ● | **Works** (playlists) |
| Global transpose track | ○ | ● | ● | ● | **Works** for MIDI: the transpose track (`Session::transpose_track`), shown on the Key ruler as "T+2" tags and set from its right-click › Global Transpose from Here (−12 to +12; `song.transpose` takes −24 to 24, `song.transpose_remove`). Every MIDI and instrument track plays its notes transposed by the change in force where each note is written, with the clip's and the track's own transpose; drum instruments (Juke, the drum synth) and tracks set to ignore it (`track.global_transpose`, from the same menu) play as written. Insert and Cut Time move the changes; MIDI export writes the notes transposed, as they play. Audio clips do not follow; poly pressure keys are not moved with it (c8e1963) |

## K. Tools that write MIDI

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Virtual players that follow chords and sections | ○ | ○ | ● | ◐ | **Works**: the Band, six players (drums, bass, keys, organ, guitar, bowed) with feels; printed to editable clips |
| Audio to MIDI | ● | ● | ● | ● | **Works**: general, piano, drum and vocal models, and a single-line pitch tracker |
| Drum hits in audio to notes | ◐ | ● | ● | ● | **Works**: through the drum model (kick, snare, hats apart), and Beat Detective › Write MIDI (`event.hits_to_midi`): a note for each hit found in the selection on the selected audio tracks (one pitch, the kick's unless given, and one velocity; hits within 30 ms are one), in a new MIDI clip on a new drum-synth track. Hits are not sorted into drums by sound, and their loudness is not read (58c4add) |
| Arpeggio, strum and chord-progression writers | ○ | ● | ● | ● | **Works**: Event › More MIDI › Write Arpeggio (`event.write_arpeggio`) turns the selected clip's held notes into the notes an arpeggiator plays (rate, pattern, octaves, gate; the track's own MIDI effects when it has some, the clip then playing past them as Record Effects' Output does), by the same effect code run offline on the song's grid. Write Progression (`event.write_progression`) writes a clip of voiced chords on the selected MIDI track over the edit selection (else four bars): the chord lane's chords, or a progression in the key (I–V–vi–IV and six others, or degrees typed so), whole, half, quarter or strummed, each chord moving to the inversion nearest the one before with its root below. Strum: the MIDI Operations' Strum. One undo step each. The menu items run with the defaults; scripts set the rest |
| MIDI from a text description; a song from a prompt | ○ | ○ | ○ | ○ | **Works** (downloaded models) |
| Loop library of MIDI phrases, previewed at the song tempo | ◐ | ● | ● | ● | **Not planned here**: needs a library of phrases (content the owner would source) |

## L. Instruments in the box

What ships with the four, in broad terms: Pro Tools has a workstation (Xpand!2), piano, organ,
synths, a sampler and a drum machine; Studio Pro has a sample player with a library, two synths,
a sampler and a drum sampler; Logic has the largest set (sampler with a large library, several
synths, drum kits and drum machine designer, modelled pianos, organs and strings); Cubase has a
workstation (HALion Sonic), a drum instrument, synths and a sampler track. FreeDAWm has four.

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Piano, electric piano, organ, strings and other sampled instruments | ● | ● | ● | ● | **Not planned here**: needs recorded samples the owner is sourcing; every imported MIDI part plays the synth |
| Analogue-style synth | ● | ● | ● | ● | **Partial**: two oscillators (four waves) with a sub oscillator and noise, a 12 or 24 dB low-pass, high-pass, band-pass or notch filter with key tracking, two envelopes, an LFO (sine, triangle, square, saw) to pitch, cutoff and level, poly, mono or legato play, glide, 16 voices, a Bend Range setting and mod-wheel vibrato. The new parameters default to the sound it had, so sessions saved before sound the same. Left: unison with detune and stereo spread (the output is mono), hard sync, effects, and factory presets using the new parameters (est. 8–12 h); written without listening |
| Wavetable, FM, granular synths | ◐ | ● | ● | ● | **Partial** (design only): none built; the Subtractive Synth's upgrade is what this round did. Wavetable (12–20 h) and FM (10–16 h) as designed below, granular not designed; each needs listening |
| Sampler: drop a sound, play it across the keys, loop, slice | ● | ● | ● | ● | **Works**: loop with crossfade, slicing to keys, filter, LFO, tempo-following mode, root detection |
| Sampler: many samples mapped by key and velocity; SFZ or other formats | ● | ● | ● | ● | **Not planned here**: built on its own branch (`claude/festive-lamport-gt6s3l`) by the session making Pantry, a multisample instrument with zones by key and velocity, layers, release samples and SFZ and Decent Sampler import; this row follows it when that branch lands |
| Drum machine with pads, layers, chokes, per-pad shaping | ● | ● | ● | ● | **Works** (Juke): 16 pads, four velocity layers and round robins, pad synth, sends and bus effects |
| Drum kits supplied | ● | ● | ● | ● | **Works**: 28 kits made by the pad synth; recorded kits are a separate download |
| Separate mixer channels per pad | ● | ● | ● | ● | **Missing**; left to [section 6](06-midi-recording.md)'s instruments with several outputs, which is the same feature |
| Factory presets for instruments | ● | ● | ● | ● | **Partial**: the Subtractive Synth has 12 factory presets, FreeDAWm's own parameter settings (basses, leads, pads, keys, plucks, a riser), from the Presets menu of its window (`mix.instrument` with their parameters); written without listening, so they are starting points. Juke has its own kits. The sampler has none: its sound is the sample it is given (048efd1) |
| Custom editors | ● | ● | ● | ● | **Partial**: Juke and the sampler have editors; the synth is a list of sliders (its new parameters among them). Left: a synth panel with its filter and envelopes drawn (design below, est. 6–8 h) |
| Layer and split several instruments on one track | ◐ | ● | ● | ● | **Partial**: live, several instrument tracks listening to one input each take a key range (the input transform's Keys from/to), which layers and splits a keyboard; a MIDI track's clips can play another track's instrument (MIDI to Track). Left: one track feeding several instruments by key and velocity range (design below, est. 6–8 h) |

## M. Instrument tracks and hosted instruments

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Swap the instrument, keep the notes | ● | ● | ● | ● | **Works** |
| Track presets: instrument plus effects | ● | ● | ● | ● | **Works** |
| Automate instrument parameters | ● | ● | ● | ● | **Works** for built-in and hosted instruments: the lane picker lists the instrument's parameters under its name, a lane plays into the instrument as an insert's does (sample-block rate), and scripts name it `plugin:instrument:<param>` (stored with the instrument's own module id, `INSTRUMENT_MODULE`). Sessions without such lanes sound as before. Not carried to DAWproject |
| Instrument latency compensated | ● | ● | ● | ● | **Works** (bug 23, fixed) |
| Freeze with the release tail | ● | ● | ● | ● | **Works** |
| Mix bounce keeps the last note's tail | ● | ● | ● | ● | **Works**: the Bounce dialog's tail (`file.bounce_mix` `tail`, up to 30 s past the range; see [section 3](03-mixing.md)) lets the last notes ring out; it is 0 unless set |
| Third-party instruments with state saved in the session | ● | ● | ● | ● | **Works** (CLAP, VST3, AU) |
| Their editor windows | ● | ● | ● | ● | **Partial**: not for AU. **Not planned here**: an AU editor needs a Mac to build and test |
| Per-note expression sent to plug-ins | ◐ | ● | ● | ● | **Partial**: MPE-style data (a channel per note) reaches CLAP note-port plug-ins as per-note expressions on the sounding notes (bug 24: bend as tuning, pressure, CC74 as brightness, volume, pan, expression), and MIDI plug-ins get the channels as played. Left: note ids and note expressions from the clip itself, rotating notes onto channels for MIDI-only plug-ins and VST3 note expressions (design below) |
| Parameter moves in the plug-in window recorded as automation | ● | ● | ● | ● | **Works**: a parameter changed in a plug-in's own window (or its drawer) while playing in Write, Touch or Latch is written to its lane, an instrument's too. For inserts this was already built (every parameter command reports the move to the automation pass); the instrument's now reports it as well |
| A crashing plug-in does not take the session down | ○ | ○ | ◐ | ○ | **Partial**: scanning is isolated; a CLAP plug-in can run in a helper process (opt-in, a crash silences that plug-in; [section 8](08-unique-features.md) owns the rest: VST3, AU, editors, a setting) |
| Preview an instrument preset or a MIDI file from the browser | ◐ | ● | ● | ● | **Partial**: a click on a `.mid` in the Workspace browser plays it on the Subtractive Synth at the session tempo where the playhead is (every track and channel but the drum channel, rendered on a worker, up to two minutes), through the preview voice at the preview level; a click again stops it, a double-click still imports it. Left: playing it on the selected track's own instrument, and previewing an instrument preset (a hidden copy of the instrument playing a phrase; est. 3–4 h) |

## N. Getting MIDI in and out

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Import and export standard MIDI files | ● | ● | ● | ● | **Works**; see [section 6](06-midi-recording.md#l-files) and bugs 16 and 17 |
| Drag a .mid file in; drag a clip out as .mid | ● | ● | ● | ● | **Partial**: a `.mid` dropped on the desktop window imports (the first version of this page said Missing: it did, at the session start; now at the insertion point); the browser imports on double-click. Dragging a clip out to the desktop is **not planned here** (the window toolkit receives drops but cannot start a drag to other programs); Clip › Export Clip as MIDI… (`file.export_clip_midi`) writes the selected MIDI clips as a .mid file starting at the clip's start (with several, at the earliest one's, the others keeping their spacing; owner's decision), opened by the tempo and meter in force there |
| DAWproject | ○ | ● | ○ | ● | **Works**, notes and controllers both ways; muted notes are left out of an export (the format cannot mark a note muted, so they would come back sounding; owner's decision), and the export report says so |
| Copy and paste between sessions | ● | ● | ● | ● | **Works** for MIDI: the clipboard belongs to the program, not the session, so clips copied in one session (MIDI clips with their notes and controllers) paste into the next one opened (the first version of this page said Missing). Not between two copies of the program running at once, nor through the system clipboard |

### Design: entering notes in the score (built)

The score (`ui-egui/src/score_editor.rs`, about 170 lines) only draws: heads placed at
`start × px_per_tick` on a grand staff, pitch from `diatonic`, sharps only. Entry needs no new
model or command: every edit goes through the commands the piano roll uses, so it is undoable
and scriptable as they are.

- **Values.** A row above the staff: whole to 32nd, dotted, triplet, and a rest; ♯ ♭ ♮ for the
  next note; the current value is the length of new notes and the step the cursor moves.
- **Where.** The inverse of the drawing: x to a tick snapped to the bar-counted grid of the
  chosen value (`midi_edit_tools::snap_abs`), y to a diatonic step (`c4_y`, `STEP`) and to its
  natural pitch, raised or lowered by the chosen accidental or by the key on the Key ruler
  (so F in G major is F♯). A ghost head follows the pointer before the click.
- **Click on empty staff** adds a note (`midi.note_add`, the last velocity); a click with
  Shift on a beat that has notes adds to the chord. **Click on a head** selects it (shared
  with the piano roll's selection, `app.midi.selected`); Delete removes it (`midi.note_delete`);
  up and down arrows move it a diatonic step (`midi.note_edit` pitch), Alt+arrows a semitone;
  a vertical drag does the same; a value key with a head selected sets its length.
- **Step entry.** With the score focused, the computer keyboard's A–G (later a MIDI keyboard's
  notes) enter the pitch nearest the last one at the cursor, then the cursor moves on by the
  value; a rest key moves it without a note.
- **Drawing to match.** Flats when the key has flats (today only sharps are drawn), and the
  heads placed by tick so an entered note lands where it was clicked.
- **Tests.** A UI test per gesture: a click on the treble G line at beat 2 with a quarter chosen
  adds G4, 960 ticks long, at tick 960; Delete and the arrows edit it; each is one undo step;
  the key of G sharpens an F. Est. 6–10 h with the tests.

## Designs (round three, follow-up)

Designs for the rows still open, in the order they would be built. None of these is built; each
row above says what is. Estimates are agent-hours with tests, as above.

### Notes edited on the track (A)

Today the "notes" view of a MIDI track draws the clip's notes and edits nothing. Make the lane a
small piano roll that reuses the note editor's model: `midi_editor::RollGeometry` laid over the
clip's rectangle (pitch range: the clip's lowest to highest note, at least an octave), and the
editor's own hit-testing and commands (`midi.notes_move`, `midi.notes_resize`, `midi.note_add`
with the Pencil, `midi.note_delete`), so every edit stays one undo step and scriptable. The
selection is the editor's (`app.midi.selected`, its clip the one under the pointer). No lanes,
no keyboard: Smart Tool behaviour only (move, trim edges, Pencil draws, Alt-click deletes). Test:
a UI test drags a note on the track one row up and one beat right, as in `midi_quantize.rs`.
Est. 6–10 h.

### 14-bit controllers and RPN/NRPN as one value (C)

The model keeps 7-bit CCs; keep it. Add derived lanes in the editor only:
- **14-bit CC n (0–31).** Shown as one line `msb × 128 + lsb`, the LSB being CC n+32 at the same
  tick (or the last one before). Drawing writes both events at each point (one command,
  `midi.ctrl14_set {clip, cc, points: [[tick, value 0..16383]]}`, which replaces the pair in the
  range). Picked under Lanes › 14-bit.
- **RPN/NRPN.** A parameter (CC 101/100 or 99/98 then data entry 6/38) shown as one lane per
  parameter number found in the clip; drawing writes the select, the data entry and, after it,
  the null RPN (127/127), so a later CC 6 is not taken for it.
- Playback is unchanged (the events are ordinary CCs); the chase on a locate already sends the
  last value of each CC, so the pair arrives; a chased RPN needs its select sent before the data:
  `chase_channel` would send each parameter's select + data entry in that order.
Test: a clip with CC1/CC33 pairs shows one line whose values round-trip through the command;
an NRPN lane reads parameter 0x0102 from four CCs. Est. 6–8 h.

### Per-note expression: editing and plug-ins (C, M)

Model: `Note` gains an optional, skip-if-empty `expr: Vec<NoteExpr>` (tick from the note's start,
kind: tuning in semitones, pressure, brightness (timbre), volume, pan; value), so old sessions
load unchanged. Recording: an MPE input (a channel per note, section 6) keeps each note's
channel's bend, pressure and CC74 as its expressions instead of channel controllers. Editing: with
one note selected, the controller lanes show its expressions (the same drawing code, the lane's
events read from and written to `expr` through `midi.note_expr_set`). Playback: CLAP plug-ins get
`CLAP_EVENT_NOTE_EXPRESSION` with the note id the host already gives each note
(`clap-host/src/plugin.rs` sends channel messages as expressions today, bug 24); MIDI-only
plug-ins get MPE (each sounding note rotated onto its own channel 2–16, its expressions as that
channel's bend, pressure and CC74; an MPE configuration message first). VST3 note expressions
follow the same path later. Est. 14–20 h; needs an MPE controller or a scripted input to test end
to end.

### Rule-based transformer: what is left (D; doc 08's logical editor)

The note part is built (Select Notes' actions). Left: conditions on controller events (CC
number and value, bend, pressure, program) with the same actions; "not" and "either/or" between
conditions; actions from a table (a velocity curve) and random ranges; rules saved by name
(`Session::transformers`, and presets in the settings folder) and run from a menu; a project-wide
editor that acts on tracks and plug-ins (Cubase's Project Logical Editor). Est. 6–8 h.
### Chord pads (E)

A pad window (Window › Chord Pads): 8 pads, each a chord (root, quality, voicing, inversion)
filled from the key on the Key ruler (I ii iii IV V vi vii° and one free pad) or set by hand.
Clicking a pad (or its key: the eight keys of a MIDI keyboard's lowest octave when Pads Take
Input is on) plays the chord on the selected instrument track through the same audition path
the editor's piano keys use, and while recording records it. Built on the chord stamp's chord
spelling (`midi_edit_tools::Chord`) and the Band's chord names. Est. 6–8 h.

### Pattern variations and melodic step patterns (F)

Juke Steps keeps one pattern per clip with fills and conditions. Variations: a pattern clip holds
up to 8 variations (A–H), each a full step grid; the clip plays the one chosen per clip (and a
lane on the clip may switch it per bar). Melodic steps: a pattern row for a pitched instrument
whose steps hold a pitch (and gate, velocity, probability as today), in a "Step Sequencer" clip
type that plays on any instrument; Turn into Notes works as it does for drums. Est. 6–10 h.

### Score layout and parts (G)

Layout: the printed score (`score::svg`) gains page size, staff size, systems per page and
breaks at sections; the score view shows beams, ties and rests (it draws heads only). Parts:
File › Print Score… picks the tracks (it takes `tracks` already) and "One page per part" prints
each track's staff alone with its own title. Est. 8–12 h. Not started: notation is a small part of
what FreeDAWm users are expected to need.

### Tempo from a MIDI performance (J)

`event.tempo_from_midi {clip, beats?: bars to fit}`: the note onsets of a clip played without a
click are tracked as beats (onset strength over time; a period from autocorrelation between 40
and 240 bpm; dynamic programming picks beats near onsets with a steady period), and a tempo point
is written at each beat so the beats land on the grid. The clip's notes keep their sample times:
they are given new ticks in the new map, and the clip is excluded from the tick-track move of
bug 3 (it moves with its notes, not with its ticks). Test: a clip played at a drifting 100–110
bpm comes back with tempo points within 1 bpm of the drift and its notes at the same samples.
Est. 8–12 h.

### Arpeggio and chord-progression writers (K)

The arpeggiator exists as a live MIDI effect (section 6). Writers: "Write Arpeggio" renders the
track's arpeggiator over the selected clip's held notes to a new clip (the same effect code run
offline); "Write Progression" turns the chord lane (or a chosen progression in the key: I–V–vi–IV
and others) into a clip of voiced chords with a rhythm (whole, half, quarter or a strum pattern).
Est. 6–10 h.

### Previewing presets and MIDI files from the browser (M)

A selected `.mid` in the browser plays on the selected instrument track (or the synth) at the
song tempo through the audition path, from the browser's Play button, stopping on another
selection. An instrument preset previews by loading it into a hidden copy of the track's
instrument (made off the audio thread, as a built-in added while playing is) and playing a short
phrase. Est. 4–6 h.

### Instruments (L)

New instruments are the largest items here and need listening; none is started.
- **Synth upgrade (14–22 h):** LFOs (2, to pitch, cutoff, amp, pan), filter types (12/24 dB LP,
  HP, BP, notch), unison with detune and spread (stereo output), sub and noise oscillators, hard
  sync, mono/legato mode and key tracking, the factory presets extended to the new parameters
  (parameter settings, no samples).
- **Synth editor (6–8 h):** a panel like the sampler's: oscillators, filter with its envelope
  drawn, amp envelope, LFOs, presets menu.
- **Wavetable synth (12–20 h):** tables made in code (basic shapes, additive spectra, morphs
  between them), position modulated by LFO and envelope; same voice and filter code as the synth.
- **FM synth (10–16 h):** four operators, eight algorithms, per-operator envelope and ratio;
  presets authored as parameter settings.
- **Sampler zones and SFZ (12–18 h):** the sampler gains a zone list (sample, key range, root,
  velocity range, round-robin group, loop); the SFZ reader parses `<region>`, `<group>`,
  `<control>` headers and the opcodes sample, lokey/hikey/key, pitch_keycenter, lovel/hivel,
  loop_mode/loop_start/loop_end, tune, volume, pan, ampeg_*, seq_length/seq_position, into zones;
  no content is bundled.
- **Layer and split (6–8 h):** an instrument track hosts up to four instruments, each with a key
  range and velocity range (layer = overlapping ranges), summed into the track; built on the
  existing instrument slot as a small rack.

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
| 27 | **Fixed.** The triangle (synth and signal generator) has PolyBLAMP corners: the folded harmonics in the lower half of the band are 20 dB or more quieter. The drum synth's six metal squares (hats, cymbals) are PolyBLEP squares. A sample played faster than recorded (pitched up, or at a higher rate than the session's) in the sampler or Juke is read through a Kaiser-windowed sinc stretched by the step (`dsp/src/resample.rs`), so what lies above the output's Nyquist is filtered out instead of folding back; at or below its own speed the 4-point cubic reads it as before. Audible change: pitched-up samples lose their aliasing (and a little of their very top). Tests: `the_triangle_is_band_limited`, `the_metal_squares_are_band_limited`, `pitching_up_does_not_fold_the_top_back`, `a_pitched_up_sample_does_not_alias`, `a_pitched_up_pad_does_not_alias` | `dsp/src/osc.rs`; `plugins/drums.rs`; `dsp/src/resample.rs`; `juke.rs`; `sampler.rs` |
| 28 | **Fixed.** The synth's oscillator mix, resonance, detune and filter-envelope amount are smoothed (20 ms) and settle exactly on the value, so a steady sound is unchanged. The sampler's filter glides to each 16-frame control value instead of stepping to it. Tests: `mix_resonance_detune_and_envelope_amount_are_smoothed`, `the_cutoff_glides_between_control_updates` (commit 451e7b4) | `dsp/src/plugins/synth.rs`; `sampler.rs:908-914` |
| 29 | **Fixed.** A built-in instrument (or insert) added while playing is made off the audio thread (commit 856ebba, `MixEngine::set_prepared_builtins`). A sampler given a new sound hands the old one back (`take_released_asset`) and the mixer drops it with its other retired assets, so the last reference to a large sample is never freed on the audio thread. Tests: `sync_on_the_render_path_makes_no_builtin_module`, `a_replaced_sample_goes_back_to_the_host` (sampler) | `mix/src/lib.rs` (`sync_sample`); `dsp/src/plugins/sampler.rs` (`set_sample`) |
| 30 | **Fixed.** The synth and the drum synth report tails that reach silence; the drum synth counts every drum's decay. Juke's tail now runs to the -80 dB where its voices stop (its decay times are to -60 dB), stretched by the longest decay a note variant gives a pad and lengthened by a pad's timing humanise and stack delays; its stereo pads keep their two sides (they were summed to mono; dual-mono samples sound as before) with Pan as a balance. Audible change: stereo pad samples play in stereo. Tests: `reported_tails_cover_the_release`, `the_tail_covers_a_variants_decay`, `a_stereo_pad_stays_stereo` | `dsp/src/plugins/synth.rs`; `drums.rs`; `juke.rs` (`tail_samples`, `render`) |
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
| Sampler with many zones, SFZ import, sustain and bend; presets | 12–18 | Sustain pedal, pitch bend with a Bend Range setting (0–24 semitones, 2 by default) and the mod wheel (vibrato at the LFO's rate up to Mod Wheel Vibrato, 0 by default by the owner's decision so a session made before plays its mod-wheel data as it did; turn it up for vibrato; `2a60fad`) done, in the synth too; zones, SFZ and presets remain |
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
| 2026-11-16 | minor | Quantize done more fully: window, random offset, note ends and lengths; built-in grooves and a groove from a clip; playback quantize per clip and per track with the notes left in place |
| 2026-11-16 | minor | Controller lanes: Line and Curve tools, ramps between points, scale and thin, program change and poly pressure lanes, controllers that follow their notes |
| 2026-11-16 | minor | Several selected MIDI clips in the editor: the others show in their colour, a click switches to one |
| 2026-11-16 | minor | Scales and chords: fold to scale, keep to scale, chord stamp, clips that follow the chord lane (opt in) |
| 2026-11-16 | minor | Drum view in the note editor (named rows, diamonds, rows in use); the event list shows and edits every event kind, inserts and filters |
| 2026-11-16 | minor | Bugs 27, 29 and 30 fixed: band-limited reading of pitched-up samples, a replaced sampler sound freed off the audio thread, Juke's stereo pads and full tails; Bend Range and mod-wheel vibrato on the synth and sampler |
| 2026-11-16 | minor | MIDI clips: per-clip transpose and velocity offset, Reverse Clip, Note Chase |
| 2026-11-16 | minor | Mod Wheel Vibrato on the synth and sampler defaults to 0 (owner's decision): older sessions sound as before |
| 2026-11-16 | minor | Commits noted on the third round's MIDI rows |
| 2026-11-16 | minor | Several MIDI clips edited at once: the roll spans every selected clip; select, move, copy, resize, delete, draw and quantize act on notes in any of them, one undo step |
| 2026-11-16 | minor | Controller ramps in the model: Line, Curve and Ramp Between Points write a point with a ramp to the next, played as a value every 32 samples, drawn as a line, written to MIDI files as steps |
| 2026-11-16 | minor | Shared MIDI clip copies that change together (Make Independent ends it); an Alt-trim stretches a MIDI clip, its notes scaling; a design for indexing dense clips |
| 2026-11-16 | minor | Per-note probability, rolled each pass with a seeded generator; Export Clip as MIDI (a clip cannot be dragged out to the desktop) |
| 2026-11-16 | minor | MIDI effects on a track (section 6): the row on MIDI effects is Partial |
| 2026-11-16 | minor | Reverse Clip mirrors controller ramps (a ramp runs the other way, its curve bent the other way) |
| 2026-11-16 | minor | Owner's decision: DAWproject export leaves muted notes out (the format cannot mark them) |
| 2026-11-16 | minor | Owner's decision: Export Clip as MIDI starts the file at the clip's start (several clips: the earliest's) |
| 2026-11-16 | minor | Grooves from a clip keep velocity accents; the drawing grid swings with a clip's playback quantize; notes playback quantize moves are outlined where they play |
| 2026-11-16 | minor | Dense clips: an index of each clip's notes lets the scheduler look only at the notes near the playhead; played event for event as before |
| 2026-11-16 | minor | A design for entering notes in the score (not built) |
| 2026-11-17 | minor | Round three reconciled: bouncing a selection in place and a bounce's tail already worked; sampled instruments, a MIDI phrase library, AU editors and dragging a clip out are not planned here; pad outputs and MIDI effects on clips left to section 6 |
| 2026-11-17 | minor | Change Velocity… limits, randomises and compresses or expands |
| 2026-11-17 | minor | Notes entered and edited in the score (click, keys, step entry), spelled in the key |
| 2026-11-17 | minor | Transpose within the key, by steps of the key on the Key ruler |
| 2026-11-17 | minor | Flam and pitch inversion in the MIDI Operations window |
| 2026-11-17 | minor | The release velocity shown and set in the note bar |
| 2026-11-17 | minor | Notes split out by channel too (the pitch split was already built) |
| 2026-11-17 | minor | Instrument parameters take automation lanes, and their moves are written in Touch, Latch and Write |
| 2026-11-17 | minor | The editor's grid takes dotted, quintuplet and septuplet values, and one that adapts to the zoom |
| 2026-11-17 | minor | Snap Moves to the Grid: a moved note's start lands on a line instead of keeping its offset |
| 2026-11-17 | minor | A note dragged to another pitch sounds there during the drag |
| 2026-11-17 | minor | Zoom to the notes: the selected notes (or every note) fill the roll |
| 2026-11-17 | minor | Copying MIDI between sessions already worked (the clipboard outlives the session); a test pins it |
| 2026-11-17 | minor | MusicXML export writes the key signature and spells the notes for it |
| 2026-11-17 | minor | Heal Separation joins neighbouring MIDI clips with matching settings |
| 2026-11-17 | minor | MusicXML export writes later key changes as new key signatures and spells the notes after them for the new key |
| 2026-11-17 | minor | Split Notes at the Cut: a note across a split is kept in both halves |
| 2026-11-17 | minor | Dense clips: the clip index covers the controllers, so each block looks only at the controllers near it |
| 2026-11-17 | minor | Drum maps: name a track's drums (or fill in General MIDI names) for any instrument |
| 2026-11-17 | minor | MusicXML import: a track per part, with meters, tempo, keys and lyrics |
| 2026-11-17 | minor | Arranger: copy, move and remove a section with the song's clips and automation |
| 2026-11-17 | minor | A rule-based transformer for notes: the matching notes deleted, muted or changed |
| 2026-11-17 | minor | Lyrics under their notes in the printed score and the MusicXML export |
| 2026-11-17 | minor | A global transpose track for MIDI and instrument tracks, on the Key ruler |
| 2026-11-17 | minor | Groove from an audio clip's hits |
| 2026-11-17 | minor | Beat Detective writes the hits it finds as MIDI notes |
| 2026-11-17 | minor | Factory presets for the Subtractive Synth |
| 2026-11-17 | minor | Designs for the rows still open: notes on the track, 14-bit and RPN lanes, per-note expression, the rest of the transformer, chord pads, pattern variations and melodic steps, score layout, tempo from a performance, arpeggio and progression writers, previews, and the instruments |
| 2026-11-17 | minor | Commits noted on the follow-up's MIDI rows |
| 2026-11-17 | minor | Subtractive Synth: filter types, LFO, sub and noise, key tracking, mono and legato |
| 2026-11-17 | minor | 14-bit controllers and RPN/NRPN parameters as one lane value |
| 2026-11-17 | minor | Notes edited directly on a MIDI track showing Notes |
| 2026-11-17 | minor | Tempo from a MIDI performance |
| 2026-11-17 | minor | MIDI files previewed from the browser |
| 2026-11-17 | minor | Chord Pads |
| 2026-11-17 | minor | Write Arpeggio and Write Progression |
| 2026-11-17 | minor | Score parts on their own pages and a staff size |
| 2026-11-17 | minor | End states for the last open rows: MPE, pattern variations and melodic steps, chord track and audio, wavetable and FM, sampler zones (Pantry), synth editor, layer and split |
