# Four-DAW parity, section 4: editing audio on the timeline

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-16 · **Change:** minor (bug fixes, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). They are named here only to compare features; FreeDAWm is not
> affiliated with or endorsed by any of them. See [NOTICE](../../NOTICE).

The fourth section ([1: playing audio](01-playback-audio.md),
[2: recording audio](02-recording-audio.md), [3: mixing](03-mixing.md)). It was added to the
original plan of seven, so built-in effects is now section 5, MIDI recording 6, MIDI writing 7 and
the features unique to each DAW 8.

**Scope of this section:** what you do to audio clips and tracks in the Edit window: tools, edit
modes, grid and nudge, selection and navigation, cutting, trimming and overlaps, clip properties,
fades, consolidating and offline processing, takes and comping, timing and pitch editing,
arrangement-level edits, the clip list, and undo. Scrub audio and live tempo-following stretch are
playback features and are in [section 1](01-playback-audio.md); loop recording into takes is in
[section 2](02-recording-audio.md); automation editing is in [section 3](03-mixing.md).

## How this was measured

- **FreeDAWm:** read from the code in the main working folder on the working branch (0.9.11 plus
  work in progress on sections 1–3), not from the docs. Options that store a value nothing reads
  are marked **flag only**. The headline bugs (1, 2, 3, 5, 6 and 8 below) were re-read by hand.
  Nothing was run, clicked or listened to.
- **The four DAWs:** from their published manuals and release notes, as known up to the versions
  above, for each product's top edition. Nothing was run side by side.

**Legend.** Other DAWs: ● has it · ◐ partly or by another route · ○ does not.
FreeDAWm: **Works** · **Partial** · **Flag only** · **Missing**.

## Summary

| Area | FreeDAWm today | Distance from the four |
|---|---|---|
| A. Tools | Selector, Grabber, Trimmer, Smart Tool, Zoomer; fade handles on clips | Medium: no stretch-by-dragging, no waveform pencil, scrub tool is silent |
| B. Edit modes, grid, nudge | Shuffle, Slip, Spot and Grid all enforced; relative grid | Medium: snaps to the grid only, never to clips, markers or zero crossings |
| C. Selection and navigation | Tab to Transient, linked selections, zoom presets, markers | Medium: edit groups do not edit together; no rubber-band clip selection |
| D. Cut, trim, overlaps | Full cut/copy/paste/repeat set, separate on grid and at transients | Medium: overwrite is the only overlap mode |
| E. Clip properties | Clip gain, mute, locks, rating, sync point, per-clip EQ and compressor | **Large**: clip groups only select together; no clip transpose |
| F. Fades | Five shapes with adjustable curve, real-time, auto edge fades, drag handles | Small to medium: thin fade dialog, no batch fades on separate clips |
| G. Consolidate and offline processing | Consolidate, ten offline processes, strip silence | Medium: processing discards handles, no process history, bounce in place is whole-track only |
| H. Takes and comping | Take layers, swipe comping, automatic crossfades | Small |
| I. Timing | Tempo detection and phase-coherent multitrack straightening work | **Large**: no warp markers to drag, the stretch-algorithm menu is a label, audio never follows tempo edits |
| J. Pitch and spectral | A real note-level vocal editor with formants; stem separation and de-noise | Medium: no ARA 2, no spectral view, mono sources only |
| K. Arrangement and tracks | Insert and cut time, duplicate, freeze, tempo curves | **Large**: no arranger track, tempo edits do not move audio, no organising folders |
| L. Clip list and sample editing | Clip list with a filter, peak files, sample-level zoom | Medium: no relink of missing audio, no audition, no sample editor |
| M. Undo and keys | 200 levels, history window, single-key editing, a key editor | Small: redo by clicking in the history |

Three things every one of the four does and FreeDAWm does not: **stretch audio by dragging**
(a clip edge or a warp marker), with a real choice of algorithm; **edits that respect groups and
locks** (grouped clips and grouped tracks edit together, locked clips cannot be changed); and
**audio that follows the tempo map** when a track is set to musical time.

## A. Tools

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Range selector | ● | ● | ● | ● | **Works** |
| Object grabber, drag between tracks, modifier to copy | ● | ● | ● | ● | **Works** |
| Grabber variants (separation grab: drag a selection out of a clip) | ● | ◐ | ◐ | ◐ | **Missing** |
| Trim tool | ● | ● | ● | ● | **Works**; stops at file ends and neighbours |
| Stretch by dragging a clip edge | ● | ● | ● | ● | **Missing** as a tool; a command stretches a clip to the selection |
| Loop by dragging a clip edge | ● | ◐ | ● | ◐ | **Missing** |
| Smart / combined tool | ● | ● | ● | ● | **Works**: trim, select, grab, fades, fade curve, clip gain by zone |
| Crossfade from the Smart Tool at a clip boundary | ● | ● | ● | ● | **Missing** |
| Scissors / split tool | ◐ | ● | ● | ● | **Missing** as a tool; Separate is a key (B, Cmd+E) |
| Mute tool | ◐ | ● | ● | ● | **Missing** as a tool; Mute Clips is a key |
| Pencil redraws the waveform at sample level | ● | ○ | ◐ | ● | **Missing**: on audio the pencil draws nothing |
| Zoom tool (click, drag a box) | ● | ● | ● | ● | **Works** |
| Scrub tool with audio | ● | ◐ | ● | ● | **Works** on clips: plays what it passes, both ways, at the drag's speed (not the track's inserts; section 1) |
| Tool chord shown in the tooltip (F6+F7) | ● | — | — | — | **Missing**: the tooltip promises it |

## B. Edit modes, grid and nudge

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Slip (free) editing | ● | ● | ● | ● | **Works** |
| Shuffle / ripple editing | ● | ● | ● | ● | **Partial**: drag, trim, cut, paste, duplicate honour it; nudge, the A/S trims and Trim to Selection do not |
| Shuffle lock | ● | — | — | — | **Works** |
| Spot: place by typed time, with the original timestamp | ● | ◐ | ◐ | ◐ | **Works** |
| Grid, absolute and relative | ● | ● | ● | ● | **Partial**: trims snap absolute even in relative grid |
| Grid in bars and beats with triplets and dotted | ● | ● | ● | ● | **Works**; "1/4 dotted" typed as a command fails to parse |
| Grid in time, frames and samples | ● | ● | ● | ● | **Partial**: lines no longer drift (bug 18, partly fixed); time and frame lines still count from zero, not the timecode start |
| Grid that adapts to zoom | ◐ | ● | ● | ● | **Missing** |
| Snap to clip edges, markers and the cursor | ◐ | ● | ● | ● | **Works** for clip edges and markers (Options › Snap, within 8 pixels; a nearer grid line wins; the suspend modifier lets go); not the cursor |
| Snap to zero crossings | ○ | ◐ | ● | ● | **Works**: Options › Snap › Zero Crossings moves the insertion and selection edges to the nearest crossing within 5 ms of the audio on the track |
| Modifier to suspend snap while dragging | ● | ● | ● | ● | **Partial**: works for moves and trims, not for selecting |
| Nudge by a chosen value | ● | ● | ● | ● | **Works**: a range nudges the clips wholly inside it; a clip it only touches stays (bug 16, fixed) |
| Nudge the audio inside a clip (slip contents) | ● | ● | ● | ● | **Missing** |
| Nudge only the selection's start or end | ● | ◐ | ◐ | ◐ | **Missing** |

## C. Selection and navigation

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Range across several tracks | ● | ● | ● | ● | **Works** |
| Select several clips as objects and act on only those | ● | ● | ● | ● | **Works**: Delete, Cut and Copy act on the selected clips only (bug 1, fixed) |
| Rubber-band selection of clips | ◐ | ● | ● | ● | **Works**: a Grabber drag from empty space selects the clips the box touches across tracks (Shift adds) |
| Select all on a track; select all following; select by colour, name or mute | ◐ | ● | ● | ● | **Missing** except Select All |
| Extend selection to session start or end | ● | ● | ● | ● | **Missing** |
| Type the selection's start, end and length | ● | ● | ● | ● | **Missing**: the numbers are display only |
| Tab to clip boundaries, Tab to Transient | ● | ● | ● | ● | **Works** |
| Link timeline and edit selection | ● | — | — | — | **Works** |
| Link track and edit selection | ● | ◐ | ● | ● | **Flag only** |
| Edit groups: an edit on one grouped track applies to all | ● | ● | ● | ● | **Partial**: selecting a track selects its group; clip grabs, moves and trims do not follow |
| Zoom: horizontal, vertical, waveform height, presets, toggle, to selection | ● | ● | ● | ● | **Works**; zoom-to-selection and fit size to the timeline on screen (bug 23, fixed) |
| Scroll modes (page, continuous, none) | ● | ● | ● | ● | **Works** |
| Markers on several rulers, a marker list | ● | ● | ● | ● | **Works** |
| A location that recalls zoom, track heights and shown tracks | ● | ○ | ◐ | ◐ | **Missing** |
| Markers attached to a track | ● | ○ | ○ | ◐ | **Flag only**: the field exists, nothing sets it |
| Session overview strip | ● | ● | ◐ | ● | **Works** |

## D. Cut, copy, trim and overlaps

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Cut, copy, paste, clear, duplicate, repeat N times | ● | ● | ● | ● | **Works**, crossfaded pairs included (bug 2, fixed); Repeat leaves automation behind, Duplicate takes it |
| Paste repeated to fill the selection | ● | ◐ | ◐ | ● | **Works**; ignores Shuffle |
| Insert silence; insert and cut time on all tracks | ● | ● | ● | ● | **Works**: every playlist and take layer, automation, markers, key changes and the tempo and meter map move (bug 7, fixed); refused inside a tempo ramp |
| Separate at the cursor or selection, on the grid, at transients | ● | ● | ● | ● | **Works**; transient cuts are found per track, so multi-mic cuts do not line up |
| Heal a separation | ● | ◐ | ● | ● | **Works**: pieces that sound alike are joined, their clip-gain lines too; pieces that differ stay apart (bug 6, fixed) |
| Trim to selection, start or end to the cursor | ● | ● | ● | ● | **Works** |
| Trim clips to close gaps | ◐ | ● | ● | ● | **Missing** |
| Overlap modes: overwrite, keep both, crossfade on overlap | ◐ | ● | ● | ● | **Partial**: always overwrite |
| Clip order (front/back) where clips overlap | ● | ● | ● | ● | **Missing**: overlapping clips are mixed together; Send to Back and Bring to Front say so and change nothing (bug 14, fixed) |
| Drag a file from the clip list or the desktop to a track | ● | ● | ● | ● | **Works**; whole files only, not a clip from the list |

## E. Clip properties

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip gain as one value, with bypass and render | ● | ● | ● | ● | **Works** |
| Clip gain as a drawn line | ● | ● | ◐ | ● | **Works**: on a track showing Clip Gain a drag on the line adds a breakpoint and moves it, a breakpoint drags between its neighbours, Alt-click deletes it (`clip.gain_line`) |
| Clip mute | ● | ● | ● | ● | **Works** |
| Clip lock (position, or all edits) | ● | ● | ● | ● | **Works**: a command that would change an edit-locked clip is refused and undone (bug 4, fixed) |
| Clip name, colour, rating | ● | ● | ● | ● | **Works** |
| Clip groups that select and move as one | ● | ● | ● | ● | **Works**: selecting a grouped clip selects its group; a drag from an unselected member takes the group; a trim moves the same edge of every member as far, and a fade drawn on one goes on all (`alone` for one) |
| Loop a clip without making copies | ● | ◐ | ● | ◐ | **Partial**: makes real copies, N times or to a length |
| Clip transpose and fine tune, non-destructive | ◐ | ● | ● | ● | **Missing**: offline pitch shift only |
| Clip stretch ratio | ● | ● | ● | ● | **Works**: trims, splits, Trim to File and fade handles count file samples at the clip's ratio (bug 3, fixed) |
| Reverse a clip without a new file | ○ | ● | ● | ◐ | **Missing**: offline only |
| Per-clip effects in real time | ● | ● | ◐ | ◐ | **Works** (EQ, compressor, gain), kept by a split's pieces and every copy (bug 5, fixed); made when the session changes, not when a clip first plays (bug 13, partly) |
| Sync point | ● | ● | ● | ● | **Works** |
| Clip notes | ○ | ● | ● | ● | **Missing** |

## F. Fades

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Fade in, fade out, crossfade, calculated in real time | ● | ● | ● | ● | **Works** |
| Shapes: linear, equal power, S-curve, adjustable slope | ● | ● | ● | ● | **Works**, plus exponential and logarithmic |
| Drag fade handles on the clip | ● | ● | ● | ● | **Works** for one clip; no crossfade by dragging |
| Fade dialog with preview, length and separate in and out shapes | ● | ◐ | ◐ | ● | **Works**: a shape and bend for each side (a crossfade's two sides too), a drawn preview, presets (built-in and saved); the length is the selection's |
| Batch fades over many clips | ● | ● | ● | ● | **Works**: the fade dialog's batch option fades both ends of every clip inside the selection, at a length in ms |
| Crossfade placement (before, across, after the cut) | ● | ◐ | ● | ● | **Partial**: follows where the selection sits |
| Warning when the audio beyond the cut is too short | ● | ◐ | ◐ | ● | **Missing**: the fade lengths silently differ |
| Automatic short fades at clip edges | ● | ● | ◐ | ● | **Works** in playback (1 ms default); not applied when consolidating or exporting |
| Delete fades in a selection | ● | ● | ● | ● | **Works**: only the fades and crossfades in the selection go (bug 9, fixed) |

## G. Consolidate and offline processing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Consolidate a selection to one new file | ● | ● | ● | ● | **Works** |
| Bounce a clip or range in place, with or without effects | ● | ● | ● | ● | **Works**: with effects, the track menu's Commit Selection renders the selection through the inserts to a new track at its place and mutes it on the source, which stays active (whole-track Commit as before); without effects, Consolidate. The new track keeps the pan, fader, pan and send automation (bug 10, fixed) |
| Export clips as files | ● | ● | ● | ● | **Works**: each clip alone, a muted one as it sounds unmuted (bug 22, fixed) |
| Normalize, gain, reverse, invert, DC removal, pitch, stretch | ● | ● | ● | ● | **Works**; each writes a new file; fades, sync point and warp markers follow (bug 20, partly fixed) |
| Run a plug-in on a clip offline | ● | ● | ● | ● | **Works** |
| Processed clip keeps audio beyond its edges (handles) | ● | ● | ● | ● | **Works** for processes that keep the length: up to 1 s (`handle_ms`) of the file each side is processed too; stretch, pitch and varispeed render the clip only |
| Process history that can be changed or removed later | ○ | ◐ | ○ | ● | **Missing** |
| Strip silence with threshold, minimum length and padding | ● | ● | ● | ● | **Works**; muted clips are left whole (bug 11, fixed) |
| Strip silence preview before committing | ● | ● | ● | ● | **Missing** |

## H. Takes and comping

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Alternate playlists or versions per track | ● | ● | ● | ● | **Works**: new, duplicate, switch |
| Delete one playlist | ● | ● | ● | ● | **Partial**: only "delete all unused" |
| Take lanes under the track | ● | ● | ● | ● | **Works** |
| Swipe comping | ◐ | ● | ● | ● | **Works** |
| Copy a selection up to the main playlist | ● | ● | ● | ● | **Works**; this route adds no crossfades |
| Automatic crossfades at comp joins | ◐ | ● | ● | ● | **Works** (10 ms equal power) |
| Name and rate takes | ● | ● | ● | ● | **Works** |
| Flatten a comp | ● | ● | ● | ● | **Partial**: by consolidating; Delete Unused Playlists keeps the layers the comp came from (bug 19, fixed) |
| Show only takes that match the selection | ● | ○ | ○ | ○ | **Missing** |
| Comp grouped tracks together (drums) | ● | ● | ● | ● | **Missing** |

## I. Timing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Warp markers you drag to move a hit | ● | ● | ● | ● | **Flag only**: markers are stored by Straighten, never drawn or editable |
| Choice of stretch algorithm (polyphonic, rhythmic, monophonic, tape) | ● | ● | ● | ● | **Partial**: every choice but varispeed runs the same offline stretch; varispeed plays with a simple resampler that aliases |
| Transient detection with a sensitivity control | ● | ● | ● | ● | **Works**; sensitivity is reachable by command only |
| Quantize audio to the grid with strength and swing | ● | ● | ● | ● | **Works** for whole clips: Clip › Quantize Audio… moves each by its sync point with a strength and a swing; no slicing at hits (Beat Detective) |
| Quantize multitrack drums keeping phase | ● | ● | ● | ● | **Works** as Straighten, to a steady tempo only |
| Slice, conform and smooth (Beat Detective style) | ● | ◐ | ◐ | ● | **Partial**: detect and separate; no conform, no gap filling |
| Extract a groove and apply it | ● | ● | ● | ● | **Missing** |
| Detect tempo from audio and build a tempo map | ◐ | ● | ● | ● | **Works**: constant, follow and smooth modes |
| Audio on a musical-time track follows tempo edits | ● | ● | ● | ● | **Flag only**: the track setting exists; nothing moves or stretches |
| Conform a clip to the session tempo | ● | ● | ● | ● | **Works**: varispeed on a Varispeed or plain track, a pitch-preserving render on an Elastic one (bug 12, fixed) |
| Re-stretch after trimming | ● | ● | ● | ● | **Partial**: a second stretch rebuilds from the first-stretch length and discards later trims |

## J. Pitch and spectral editing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Built-in note-level pitch editor | ◐ | ◐ | ● | ● | **Works**: pitch, drift, vibrato, formant, per-note gain; monophonic sources |
| Pitch editor on a stretched clip | ● | ● | ● | ● | **Missing**: a stretched clip plays untuned |
| Automatic pitch correction in real time | ○ | ○ | ● | ● | **Partial**: offline per clip (centre and snap to key) |
| ARA 2 for third-party editors | ● | ● | ● | ● | **Missing** |
| Audio to MIDI | ● | ● | ● | ● | **Works**, including polyphonic and drum models |
| Spectral view or spectral editing on the timeline | ○ | ◐ | ○ | ● | **Missing** |
| Stem separation | ◐ | ● | ● | ● | **Works** (desktop, downloaded model) |
| De-noise as clip processing | ○ | ○ | ○ | ◐ | **Works** (desktop, downloaded model) |

Pro Tools and Studio Pro reach note-level pitch editing through bundled Melodyne over ARA; that is
why they score ◐ for "built-in".

## K. Arrangement and track-level edits

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Duplicate a track with a choice of what comes along | ● | ● | ● | ● | **Works**; the copy's clips keep the original's group and loop ids |
| Freeze, commit | ● | ● | ● | ● | **Works** (see G for the commit bug) |
| Make inactive, hide, colour, reorder, height presets | ● | ● | ● | ● | **Works**; a dragged folder takes its members (bug 15, fixed) |
| Folders that only organise, and edit a closed folder as one clip | ● | ● | ● | ● | **Missing**: every folder is a routing folder; no folder editing |
| Lock a whole track | ◐ | ● | ● | ● | **Missing** |
| Arranger track: named sections you move, copy and repeat | ○ | ● | ● | ● | **Missing** |
| Scratch pads or alternative arrangements | ○ | ● | ● | ◐ | **Missing** |
| Tempo changes, ramps and curves; meter changes | ● | ● | ● | ● | **Works** |
| Tap tempo | ● | ● | ● | ● | **Missing** |
| Identify a beat to set the tempo from a selection | ● | ◐ | ● | ● | **Works**: the tempo after the selection resumes where it ends (bug 8, fixed) |
| Video stays in sync through edits; snap to frames | ● | ● | ● | ● | **Works** |

## L. Clip list and sample-level editing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| List of clips and files with a search | ● | ● | ● | ● | **Works**; shows the active playlists only |
| Audition, rename and locate from the list | ● | ● | ● | ● | **Works**: a click selects the clip and scrolls to it when off screen; Alt-click or Audition plays it; Rename… from its menu |
| Select and remove unused clips | ● | ● | ● | ● | **Works**: Select Unused Files picks the files no clip plays; Clear removes them |
| Relink missing audio files | ● | ● | ● | ● | **Missing** (video can be relinked) |
| Compact files to what is used | ● | ● | ● | ● | **Missing** |
| "Capture" a selection as a new clip in the list | ● | ◐ | ◐ | ● | **Missing**: the command only renames the clip "Captured" |
| Audio file / sample editor window | ○ | ● | ● | ● | **Missing** |
| Waveform from peak files, down to single samples, rectified and outline views | ● | ● | ● | ● | **Works**; a stretched clip's waveform is drawn unstretched |

## M. Undo and keyboard

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Multi-level undo and redo, every edit covered | ● | ● | ● | ● | **Works** (200 levels) |
| Undo leaves zoom, scroll and the current tool alone | ● | ● | ● | ● | **Works**: undo and redo keep the zoom, scroll, waveform height and tool, as they keep the Edit mode |
| Undo history list | ● | ● | ● | ● | **Works**; redo by clicking is not offered |
| Single-key editing commands | ● | ● | ● | ● | **Works** |
| Editable key bindings | ● | ● | ● | ● | **Works**; a few fixed keys cannot be changed |

## Bugs found on the way

Found while reading; none was reproduced by running the program. Paths are under `crates/`.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed.** Selecting clips A and C and pressing Delete also removed clip B between them, on every track involved. Delete, Cut and Copy now act on the selected clips alone while the selection is still the span they set (`object_clips`, `edit::remove_clips`); a range passed or chosen still clears the range. Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/edit.rs` (`object_clips`, `cut_copy_objects`, `clear`) |
| 2 | **Fixed.** Paste, Duplicate, Repeat, clip loop and Ctrl-drag copy placed clips one by one, each clearing what is under it, so the second clip of a crossfaded pair cut the first off. They now place their clips together (`edit::put_clips`). Test: `engine/tests/editing_parity.rs` | `engine/src/edit.rs` (`paste`, `put_clips`); `engine/src/cmd/clip.rs` (`loop_clips`); `engine/src/cmd/clip_more.rs` (`copy_to`) |
| 3 | **Fixed.** Trimming or splitting a stretched clip advanced the file position by timeline samples, not source samples, so the remaining piece played from the wrong place. `Clip::trim_start_to` now moves the offset by `source_span` (timeline ÷ ratio, as playback reads it), and the Trim tool, Trim to File, Trim to Fill and crossfade handles measure the file at the clip's ratio. Test: `engine/tests/editing_parity.rs` | `model/src/clip.rs` (`trim_start_to`, `source_span`, `timeline_span`); `engine/src/edit.rs` (`trim_clip`, `create_fades`) |
| 4 | **Fixed.** An edit-locked clip could still be cut, cleared, separated, trimmed by menu, faded, healed, gain-changed, processed and stripped. One check after every command (`edit::locked_edit_refused`, next to the frozen-track check in `Engine::execute`) refuses and undoes any command that changed or removed a clip it left edit-locked; renaming, colouring, unlocking, deleting the track or playlist, and Insert/Cut Time moving it are allowed. Recording does not run through commands and still overwrites. Test: `engine/tests/editing_parity.rs` | `engine/src/edit.rs` (`locked_edit_refused`); `engine/src/lib.rs` (`execute`) |
| 5 | **Fixed.** Per-clip effects were stored under the clip's id (`clip_fx.<clip>.*` edit values), so a split's right half and every copy played without them. They are now carried on the clip (`Clip::effects`, `Clip::effects_bypass`), which the mixer reads; older sessions are moved over on load (`Session::sanitize`). Edit Lock protects them too. Test: `engine/tests/editing_parity.rs` | `model/src/clip.rs`; `model/src/session.rs` (`migrate_clip_effects`); `mix/src/lib.rs` (`clip_fx_params`) |
| 6 | **Fixed.** Heal joined two pieces when only their clip gain matched, dropping the second piece's gain line, mute, stretch and channel. It now joins only pieces of the same file continuing at the same stretch whose gain, bypass, mute, channel, clip effects, Clean Vocal and tuning match and with no fade at the join, and joins their clip-gain lines (`Clip::joined_env`). Test: `engine/tests/editing_parity.rs` | `engine/src/edit.rs` (`heal`, `heals`); `model/src/clip.rs` (`joined_env`) |
| 7 | **Fixed.** Insert Time and Cut Time moved the active playlist, automation and markers but not take layers, other playlists, the tempo and meter map or key signatures. They now work on every playlist, move key changes, and insert or remove ticks in the tempo and meter map (`TempoMap::insert_ticks`, `remove_ticks`; inserted time plays at the tempo where it goes in, a moved meter change goes to the next bar line). A point inside a tempo ramp is refused. Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/event.rs` (`insert_time`, `cut_time`); `time/src/tempo.rs` |
| 8 | **Fixed.** Identify Beat and Beat Detective's tempo option set a tempo at the selection start without restoring the old one at its end. Both now make the selection that many beats at the new tempo and resume the tempo in effect at its end there, moving later changes with it (`TempoMap::set_span`), so what follows stays where it was in time. Ends inside a ramp are refused. Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/event_more.rs`; `time/src/tempo.rs` (`set_span`) |
| 9 | **Fixed.** Deleting fades in a selection trimmed every crossfade on the track back to its midpoint, including those outside the selection. Only fades and crossfades that lie in the selection go now (with a point, those of the clip under it). Test: `engine/tests/editing_parity.rs` | `engine/src/edit.rs` (`delete_fades`) |
| 10 | **Fixed.** Commit / Bounce copied volume, output and sends to the new track but not pan or automation, then hid the original. It now also carries the pan, trim, surround panner, automation mode and every automation lane but the inserts' own (which is in the render). Test: `io::strip_tests::commit_carries_pan_trim_and_automation` | `engine/src/io.rs` (`commit_track`) |
| 11 | **Fixed.** Strip Silence treated muted clips as silence and deleted them. Muted clips are now set aside before stripping and put back whole. Test: `io::strip_tests::muted_clips_are_left_whole` | `engine/src/io.rs` (`strip_silence`) |
| 12 | **Fixed.** Conform to Tempo set a varispeed ratio, so pitch changed even on a pitch-preserving (Elastic) track. On an Elastic track (any algorithm but Varispeed) it now renders the clip with the pitch-preserving stretch from its original, as Elastic Properties does. Test: `cmd::clip::tests::conform_on_an_elastic_track_keeps_the_pitch` | `engine/src/cmd/clip_more.rs` (`conform`) |
| 13 | **Partly fixed.** The first time a clip with effects played, its buffer and processors were created inside `render`. They are now made by `MixEngine::sync` when the session changes (`sync_clip_fx`; the structure fingerprint includes which clips have effects and which modules they need), modules no longer needed are retired rather than dropped, and `render` only looks them up and sets parameters. Left: `sync` itself runs on the audio thread when the player receives a new session, as it does for built-in insert modules; making them on the control thread needs the third-party adopt path (`SlotKey`) extended to clip effects. Test: `mix` `clip_effects_are_prepared_before_the_clip_plays` | `mix/src/lib.rs` (`sync_clip_fx`, `ClipFx::prepare_modules`, `process_strip`) |
| 14 | **Fixed.** Send to Back / Bring to Front reordered the playlist without re-sorting, and the next edit's sort undid the order anyway. Overlapping clips are mixed together, so there is no order to keep: the commands now leave the playlist alone and say so. A real front/back order belongs with the overlap modes (tier 2). Test: `cmd::clip::tests::layer_commands_keep_the_playlist_in_time_order` | `engine/src/cmd/clip.rs` (`layer`) |
| 15 | **Fixed.** Dragging a folder track moved the folder without its members. `track.move` now moves a folder with the tracks inside it at any level unless a count is given (`edit::folder_block`), and the drag passes that count and refuses a drop inside the folder itself. Test: `engine/tests/editing_parity.rs` (the drag itself not checked in a GUI) | `engine/src/cmd/track.rs` (`move_track`); `ui-egui/src/edit_window.rs` |
| 16 | **Fixed.** Nudge with a partial selection moved whole clips; clips nudged past zero were each clamped, closing the gaps between them. A range selection now nudges only the clips wholly inside it, and a move left stops at the session start for all moving clips together. Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/edit.rs` (`nudge_clips`); `engine/src/edit.rs` (`move_clips`) |
| 17 | **Fixed.** A Shuffle trim rippled clips from just after the clip's start (its crossfade partner included) but automation from its end, so points under the crossfade were deleted. Automation now moves from the same place as the clips. Test: `engine/tests/editing_parity.rs` | `engine/src/edit.rs` (`trim_clip`) |
| 18 | **Partly fixed.** Time and frame grids rounded the step to whole samples, so 29.97 fps at 48 kHz drifted about 0.9 s an hour. Line *k* is now at *k* × the exact step, rounded (`GridValue::exact_step`). Left: the lines still count from sample 0, not the timecode start; that needs the start passed to every grid call. Test: `time` `frame_lines_do_not_drift_over_an_hour` | `time/src/grid.rs` (`line_at_or_before`, `next_line`) |
| 19 | **Fixed.** "Delete Unused Playlists" removed take layers but left the comp's references to them, and the record target (an index) went stale. It now keeps the layers the comp was swiped from, re-points the comp's references and the record target at the layers that stay, and clears a target that went. (`comp.layer_delete` already re-indexed them.) Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/track.rs` (`delete_unused_playlists`) |
| 20 | **Fixed.** Processing a clip did not shorten fades to the new length, clear warp markers or swap the fades on Reverse, and rendered a muted clip to silence. Fades are now kept inside the new length, warp markers cleared, Reverse swaps the fades and mirrors the sync point, and a muted clip is processed as it sounds unmuted. The handles are kept now too (tier 2). Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/audiosuite.rs` |
| 21 | **Fixed.** Coalescing volume automation to clip gain changed only the active playlist, then zeroed the fader, so other playlists changed level. Every playlist and take layer's clips now take the level. Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/edit_more.rs` (`coalesce_volume_to_clip_gain`) |
| 22 | **Fixed.** Export Clips rendered the clip's time range, not the clip, so overlapping neighbours were in the file (and a muted clip was silence). Each clip now renders alone and unmuted. Test: `io::strip_tests::exported_clips_hold_only_their_own_audio` | `engine/src/io.rs` (`export_clips`) |
| 23 | **Fixed.** Zoom to Selection and Fit by key or menu assumed a 1200-pixel timeline. The app now passes the timeline's width on screen when the call names none (`SoundApp::run_with`); headless callers still default to 1200. Not checked in a running GUI | `ui-egui/src/lib.rs` |
| 24 | **Fixed.** Default marker names no longer repeat after a delete; a marker added while playing is a point (no stray length to the selection start); a view-only location no longer moves the playhead. "+" on a US keyboard is Shift+=, the Grid value key: the unshifted "=" beside "−" now nudges later (the keypad "+" still does) and Shift+= stays the Grid key. Dragged fades are equal power by default, as commanded ones are (a fade already there keeps its shape; sessions keep their stored fades). Tests: `engine/tests/editing_parity.rs`, `ui-egui/tests/pro_tools_keys.rs` (`equals_nudges_later_while_shift_equals_steps_the_grid`), `edit_window` `fade_drag_previews_and_commits_stay_inside_the_clip` | `engine/src/cmd/event.rs` (`add_marker`, `recall_marker`); `ui-egui/src/shortcuts.rs`; `ui-egui/src/edit_window.rs` (`fade_drag_preview`) |

## Suggested order of work

Estimates are agent-hours for one implementer with tests, as in the earlier sections.

**1. Fix what is wrong (≈ 16–26 h).** Bugs 1–24. Bugs 1 and 2 first: both silently remove audio
the user did not touch. Bug 3 next, then 4 and 5, which share one fix each (one lock check in the
edit primitives; clip effects carried on the clip rather than looked up by id).

**2. What all four have (≈ 95–150 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Object editing: clip-only delete, cut and copy; rubber-band clips; clip groups that select and move as one; edit groups that move, trim and separate together | 10–16 | Fixes bug 1 properly |
| Stretch by dragging: a stretch trim tool, and warp markers drawn and draggable on the clip | 14–22 | Needs the stretched-clip offset fix (bug 3) first |
| Real stretch algorithms: a transient-preserving mode for drums and a monophonic mode, with the menu choosing between them | 12–20 | Shared with section 1's live stretch |
| Audio on musical-time tracks follows tempo edits (move, and stretch where the track is elastic) | 10–16 | |
| Audio quantize with strength and swing; Beat Detective conform and smoothing; groove extract and apply | 10–16 | Straighten already has the phase-coherent core |
| Snap to clip edges, markers and the cursor; adaptive grid; slip the audio inside a clip; typed selection start, end and length | 8–12 | |
| Clip gain line editable on the clip; clip transpose; real-time clip loop and reverse | 8–14 | |
| Fades: dialog with preview and lengths, batch fades on separate clips, crossfade by dragging, short-handle warning | 6–10 | |
| Bounce in place for a clip or range, with or without effects; processing that keeps handles; strip-silence preview | 8–12 | |
| Overlap modes (keep both, crossfade on overlap) with a working front/back order | 5–8 | |
| Clip list: relink missing audio, audition, rename, find unused, scroll to the clip, capture | 6–10 | Audition shares section 1's preview work |
| ARA 2 hosting | 20–30 | Largest single item; VST3 first |
| Undo that leaves the view alone; tap tempo; scissors and mute tools | 4–6 | |

**3. What most have (≈ 45–70 h).** An arranger track (all but Pro Tools), 12–18; folders that only
organise, with folder editing, 8–12; a sample editor with a waveform pencil, 8–12; process history
(Cubase, partly Studio Pro), 8–12; comping grouped tracks together, 4–6; a location that recalls
zoom and track layout, and track markers (Pro Tools), 3–5; snap to zero crossings and real-time
pitch correction, 4–8.

**Status of tiers 2 and 3 (2026-11-16).** Done: undo and redo leave the zoom, scroll and tool
alone. First slice: clip groups select as one (see E). Not started: every other tier-2 and tier-3
item, ARA 2 hosting included. Real stretch algorithms were not started either, so as not to
duplicate section 1's live time-stretch work.

**Where FreeDAWm is already level or ahead:** Spot mode with the original timestamp (only Pro
Tools matches it), fade shapes with a continuous curve, swipe comping with automatic crossfades,
phase-coherent multitrack straightening in one command, a built-in vocal editor with formant
control (Pro Tools and Studio Pro need Melodyne), stem separation and de-noise as clip processes,
and audio to MIDI for polyphonic and drum material.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Feature entries for the four come from
their manuals as known, not from a side-by-side run; treat a ◐ as "check before relying on it".

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | major | First version: editing audio against Pro Tools, Studio Pro, Logic Pro and Cubase |
| 2026-11-16 | minor | Bug 1 fixed: Delete, Cut and Copy of selected clips leave the clips between them alone |
| 2026-11-16 | minor | Bug 2 fixed: pasted, duplicated, repeated, looped and copied crossfaded pairs keep their crossfade |
| 2026-11-16 | minor | Bug 3 fixed: trimming or splitting a stretched clip keeps each piece on its audio |
| 2026-11-16 | minor | Bug 4 fixed: edits that would change an edit-locked clip are refused |
| 2026-11-16 | minor | Bug 5 fixed: clip effects live on the clip, so splits and copies keep them. Bug 13 partly: clip effects are made at sync, not in render |
| 2026-11-16 | minor | Bug 6 fixed: Heal joins only pieces that sound alike and keeps both clip-gain lines |
| 2026-11-16 | minor | Bug 9 fixed: deleting fades leaves crossfades outside the selection alone |
| 2026-11-16 | minor | Bugs 7 and 8 fixed: Insert/Cut Time move every playlist, the tempo and meter map and keys; Identify Beat restores the tempo after the selection |
| 2026-11-16 | minor | Bugs 10, 11 and 22 fixed: Commit keeps pan and automation, Strip Silence keeps muted clips, Export Clips writes each clip alone |
| 2026-11-16 | minor | Bugs 12 and 14 fixed: Conform keeps the pitch on Elastic tracks; front/back no longer unsorts the playlist |
| 2026-11-16 | minor | Bugs 16 and 17 fixed: partial-selection nudge, spacing at the session start, Shuffle trim automation |
| 2026-11-16 | minor | Bug 18 partly fixed: time and frame grid lines no longer drift |
| 2026-11-16 | minor | Bugs 19 and 21 fixed, 20 partly: unused playlists, processed clips' fades and markers, coalescing on every playlist |
| 2026-11-16 | minor | Bugs 15 and 23 fixed, 24 partly; tier 2: undo leaves the view and tool alone |
| 2026-11-16 | minor | Tier 2, first slice: clip groups select as one; status of tiers 2 and 3 |
| 2026-11-16 | minor | Tier 2: processing keeps handles |
| 2026-11-16 | minor | Tier 2: Commit Selection (bounce a range in place with effects) |
| 2026-11-16 | minor | Tier 2: fade dialog with a shape for each side, presets and batch fades |
| 2026-11-16 | minor | Tier 2: audio quantize with strength and swing |
| 2026-11-16 | minor | Tier 2: the clip gain line is edited on the clip |
| 2026-11-16 | minor | Tier 2: snap to clip edges, markers and zero crossings; rubber-band clip selection; clip groups trim, fade and drag as one |
| 2026-11-16 | minor | Tier 2: the Clip List auditions, renames, scrolls to clips and finds unused files |
| 2026-11-16 | minor | Bug 24 fixed: "=" nudges later beside "−"; dragged fades default to equal power |
