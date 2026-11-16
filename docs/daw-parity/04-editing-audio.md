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
| E. Clip properties | Clip gain, mute, locks, rating, sync point, per-clip EQ and compressor | **Large**: clip groups are not honoured; no clip transpose |
| F. Fades | Five shapes with adjustable curve, real-time, auto edge fades, drag handles | Small to medium: thin fade dialog, no batch fades on separate clips |
| G. Consolidate and offline processing | Consolidate, ten offline processes, strip silence | Medium: processing discards handles, no process history, bounce in place is whole-track only |
| H. Takes and comping | Take layers, swipe comping, automatic crossfades | Small |
| I. Timing | Tempo detection and phase-coherent multitrack straightening work | **Large**: no warp markers to drag, the stretch-algorithm menu is a label, audio never follows tempo edits |
| J. Pitch and spectral | A real note-level vocal editor with formants; stem separation and de-noise | Medium: no ARA 2, no spectral view, mono sources only |
| K. Arrangement and tracks | Insert and cut time, duplicate, freeze, tempo curves | **Large**: no arranger track, tempo edits do not move audio, no organising folders |
| L. Clip list and sample editing | Clip list with a filter, peak files, sample-level zoom | Medium: no relink of missing audio, no audition, no sample editor |
| M. Undo and keys | 200 levels, history window, single-key editing, a key editor | Small: undo also rewinds zoom, scroll and tool |

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
| Scrub tool with audio | ● | ◐ | ● | ● | **Partial**: moves the playhead silently (section 1) |
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
| Grid in time, frames and samples | ● | ● | ● | ● | **Partial**: steps round to whole samples, so a frame grid at 29.97 fps drifts |
| Grid that adapts to zoom | ◐ | ● | ● | ● | **Missing** |
| Snap to clip edges, markers and the cursor | ◐ | ● | ● | ● | **Missing**: only the grid is ever snapped to |
| Snap to zero crossings | ○ | ◐ | ● | ● | **Missing** |
| Modifier to suspend snap while dragging | ● | ● | ● | ● | **Partial**: works for moves and trims, not for selecting |
| Nudge by a chosen value | ● | ● | ● | ● | **Partial**: moves whole clips even when only part is selected |
| Nudge the audio inside a clip (slip contents) | ● | ● | ● | ● | **Missing** |
| Nudge only the selection's start or end | ● | ◐ | ◐ | ◐ | **Missing** |

## C. Selection and navigation

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Range across several tracks | ● | ● | ● | ● | **Works** |
| Select several clips as objects and act on only those | ● | ● | ● | ● | **Works**: Delete, Cut and Copy act on the selected clips only (bug 1, fixed) |
| Rubber-band selection of clips | ◐ | ● | ● | ● | **Missing**: the rubber band picks automation points only |
| Select all on a track; select all following; select by colour, name or mute | ◐ | ● | ● | ● | **Missing** except Select All |
| Extend selection to session start or end | ● | ● | ● | ● | **Missing** |
| Type the selection's start, end and length | ● | ● | ● | ● | **Missing**: the numbers are display only |
| Tab to clip boundaries, Tab to Transient | ● | ● | ● | ● | **Works** |
| Link timeline and edit selection | ● | — | — | — | **Works** |
| Link track and edit selection | ● | ◐ | ● | ● | **Flag only** |
| Edit groups: an edit on one grouped track applies to all | ● | ● | ● | ● | **Partial**: selecting a track selects its group; clip grabs, moves and trims do not follow |
| Zoom: horizontal, vertical, waveform height, presets, toggle, to selection | ● | ● | ● | ● | **Works**; the keys for zoom-to-selection and fit assume a 1200-pixel window |
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
| Insert silence; insert and cut time on all tracks | ● | ● | ● | ● | **Partial**: the tempo and meter map, key signatures, take layers and other playlists do not move (bug 7) |
| Separate at the cursor or selection, on the grid, at transients | ● | ● | ● | ● | **Works**; transient cuts are found per track, so multi-mic cuts do not line up |
| Heal a separation | ● | ◐ | ● | ● | **Partial**: loses the second piece's settings (bug 6) |
| Trim to selection, start or end to the cursor | ● | ● | ● | ● | **Works** |
| Trim clips to close gaps | ◐ | ● | ● | ● | **Missing** |
| Overlap modes: overwrite, keep both, crossfade on overlap | ◐ | ● | ● | ● | **Partial**: always overwrite |
| Clip order (front/back) where clips overlap | ● | ● | ● | ● | **Flag only**, and it leaves the playlist out of time order (bug 14) |
| Drag a file from the clip list or the desktop to a track | ● | ● | ● | ● | **Works**; whole files only, not a clip from the list |

## E. Clip properties

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip gain as one value, with bypass and render | ● | ● | ● | ● | **Works** |
| Clip gain as a drawn line | ● | ● | ◐ | ● | **Partial**: played and shown, but points cannot be added or dragged; it only arrives by converting volume automation |
| Clip mute | ● | ● | ● | ● | **Works** |
| Clip lock (position, or all edits) | ● | ● | ● | ● | **Works**: a command that would change an edit-locked clip is refused and undone (bug 4, fixed) |
| Clip name, colour, rating | ● | ● | ● | ● | **Works** |
| Clip groups that select and move as one | ● | ● | ● | ● | **Flag only**: the group is stored and exported, never used in editing |
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
| Fade dialog with preview, length and separate in and out shapes | ● | ◐ | ◐ | ● | **Partial**: a shape picker |
| Batch fades over many clips | ● | ● | ● | ● | **Partial**: only at boundaries that touch; clips standing apart inside the range get none |
| Crossfade placement (before, across, after the cut) | ● | ◐ | ● | ● | **Partial**: follows where the selection sits |
| Warning when the audio beyond the cut is too short | ● | ◐ | ◐ | ● | **Missing**: the fade lengths silently differ |
| Automatic short fades at clip edges | ● | ● | ◐ | ● | **Works** in playback (1 ms default); not applied when consolidating or exporting |
| Delete fades in a selection | ● | ● | ● | ● | **Partial**: also damages crossfades outside the selection (bug 9) |

## G. Consolidate and offline processing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Consolidate a selection to one new file | ● | ● | ● | ● | **Works** |
| Bounce a clip or range in place, with or without effects | ● | ● | ● | ● | **Partial**: whole track only, always through its inserts; pan and automation are not carried to the new track (bug 10) |
| Export clips as files | ● | ● | ● | ● | **Partial**: a neighbour's crossfade audio is included; a muted clip exports as silence |
| Normalize, gain, reverse, invert, DC removal, pitch, stretch | ● | ● | ● | ● | **Works**; each writes a new file |
| Run a plug-in on a clip offline | ● | ● | ● | ● | **Works** |
| Processed clip keeps audio beyond its edges (handles) | ● | ● | ● | ● | **Missing**: only the visible part is rendered |
| Process history that can be changed or removed later | ○ | ◐ | ○ | ● | **Missing** |
| Strip silence with threshold, minimum length and padding | ● | ● | ● | ● | **Works**; it also deletes muted clips (bug 11) |
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
| Flatten a comp | ● | ● | ● | ● | **Partial**: by consolidating |
| Show only takes that match the selection | ● | ○ | ○ | ○ | **Missing** |
| Comp grouped tracks together (drums) | ● | ● | ● | ● | **Missing** |

## I. Timing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Warp markers you drag to move a hit | ● | ● | ● | ● | **Flag only**: markers are stored by Straighten, never drawn or editable |
| Choice of stretch algorithm (polyphonic, rhythmic, monophonic, tape) | ● | ● | ● | ● | **Partial**: every choice but varispeed runs the same offline stretch; varispeed plays with a simple resampler that aliases |
| Transient detection with a sensitivity control | ● | ● | ● | ● | **Works**; sensitivity is reachable by command only |
| Quantize audio to the grid with strength and swing | ● | ● | ● | ● | **Partial**: moves whole clips by their sync point; no strength or swing |
| Quantize multitrack drums keeping phase | ● | ● | ● | ● | **Works** as Straighten, to a steady tempo only |
| Slice, conform and smooth (Beat Detective style) | ● | ◐ | ◐ | ● | **Partial**: detect and separate; no conform, no gap filling |
| Extract a groove and apply it | ● | ● | ● | ● | **Missing** |
| Detect tempo from audio and build a tempo map | ◐ | ● | ● | ● | **Works**: constant, follow and smooth modes |
| Audio on a musical-time track follows tempo edits | ● | ● | ● | ● | **Flag only**: the track setting exists; nothing moves or stretches |
| Conform a clip to the session tempo | ● | ● | ● | ● | **Partial**: changes pitch even on a pitch-preserving track (bug 12) |
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
| Make inactive, hide, colour, reorder, height presets | ● | ● | ● | ● | **Works**; dragging a folder leaves its members behind (bug 15) |
| Folders that only organise, and edit a closed folder as one clip | ● | ● | ● | ● | **Missing**: every folder is a routing folder; no folder editing |
| Lock a whole track | ◐ | ● | ● | ● | **Missing** |
| Arranger track: named sections you move, copy and repeat | ○ | ● | ● | ● | **Missing** |
| Scratch pads or alternative arrangements | ○ | ● | ● | ◐ | **Missing** |
| Tempo changes, ramps and curves; meter changes | ● | ● | ● | ● | **Works** |
| Tap tempo | ● | ● | ● | ● | **Missing** |
| Identify a beat to set the tempo from a selection | ● | ◐ | ● | ● | **Partial**: everything after the selection shifts (bug 8) |
| Video stays in sync through edits; snap to frames | ● | ● | ● | ● | **Works** |

## L. Clip list and sample-level editing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| List of clips and files with a search | ● | ● | ● | ● | **Works**; shows the active playlists only |
| Audition, rename and locate from the list | ● | ● | ● | ● | **Partial**: selects the clip but does not scroll to it; no audition or rename |
| Select and remove unused clips | ● | ● | ● | ● | **Partial**: remove works, nothing finds the unused ones |
| Relink missing audio files | ● | ● | ● | ● | **Missing** (video can be relinked) |
| Compact files to what is used | ● | ● | ● | ● | **Missing** |
| "Capture" a selection as a new clip in the list | ● | ◐ | ◐ | ● | **Missing**: the command only renames the clip "Captured" |
| Audio file / sample editor window | ○ | ● | ● | ● | **Missing** |
| Waveform from peak files, down to single samples, rectified and outline views | ● | ● | ● | ● | **Works**; a stretched clip's waveform is drawn unstretched |

## M. Undo and keyboard

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Multi-level undo and redo, every edit covered | ● | ● | ● | ● | **Works** (200 levels) |
| Undo leaves zoom, scroll and the current tool alone | ● | ● | ● | ● | **Missing**: they jump back with the edit |
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
| 6 | Heal joins two pieces when only their clip gain matches; the second piece's gain line, mute, stretch and channel are dropped | `engine/src/edit.rs:432-455` |
| 7 | Insert Time and Cut Time move the active playlist, automation and markers but not take layers, other playlists, the tempo and meter map or key signatures | `engine/src/cmd/event.rs:244-293` |
| 8 | Identify Beat and Beat Detective's tempo option set a tempo at the selection start without restoring the old one at its end | `engine/src/cmd/event_more.rs:702, 723` |
| 9 | Deleting fades in a selection trims every crossfade on the track back to its midpoint, including those outside the selection | `engine/src/edit.rs:866-877` |
| 10 | Commit / Bounce copies volume, output and sends to the new track but not pan or automation, then hides the original | `engine/src/io.rs:1191-1202` |
| 11 | Strip Silence treats muted clips as silence and deletes them | `engine/src/io.rs:979-992` |
| 12 | Conform to Tempo sets a varispeed ratio, so pitch changes even on a pitch-preserving (Elastic) track | `engine/src/cmd/clip_more.rs:255-260` |
| 13 | **Partly fixed.** The first time a clip with effects played, its buffer and processors were created inside `render`. They are now made by `MixEngine::sync` when the session changes (`sync_clip_fx`; the structure fingerprint includes which clips have effects and which modules they need), modules no longer needed are retired rather than dropped, and `render` only looks them up and sets parameters. Left: `sync` itself runs on the audio thread when the player receives a new session, as it does for built-in insert modules; making them on the control thread needs the third-party adopt path (`SlotKey`) extended to clip effects. Test: `mix` `clip_effects_are_prepared_before_the_clip_plays` | `mix/src/lib.rs` (`sync_clip_fx`, `ClipFx::prepare_modules`, `process_strip`) |
| 14 | Send to Back / Bring to Front reorder the playlist without re-sorting; code that assumes time order can misbehave until the next edit | `engine/src/cmd/clip.rs:305-315` |
| 15 | Dragging a folder track moves the folder without its members | `ui-egui/src/edit_window.rs:1301` |
| 16 | Nudge with a partial selection moves whole clips; clips nudged past zero are each clamped, closing the gaps between them | `engine/src/cmd/edit.rs:907-921`; `engine/src/edit.rs:481` |
| 17 | Shuffle trim ripples clips from one point and automation from another | `engine/src/edit.rs:783-784` |
| 18 | Time and frame grids round the step to whole samples: 29.97 fps at 48 kHz drifts about 0.9 s an hour, and both ignore the timecode start | `time/src/grid.rs:127-128, 159-160` |
| 19 | "Delete Unused Playlists" removes take layers but leaves the comp's references to them; the stored target playlist is an index that goes stale | `engine/src/cmd/track.rs:159-173`; `engine/src/cmd/comp.rs:325` |
| 20 | Processing a clip does not shorten fades to the new length, clear warp markers or swap the fades on Reverse; and it drops the handles | `engine/src/cmd/audiosuite.rs:69-121` |
| 21 | Coalescing volume automation to clip gain changes only the active playlist, then zeroes the fader, so other playlists change level | `engine/src/cmd/edit_more.rs:812-826` |
| 22 | Export Clips renders the clip's time range, not the clip, so overlapping neighbours are in the file | `engine/src/io.rs:1212` |
| 23 | Zoom to Selection and Fit by key assume a 1200-pixel timeline | `engine/src/cmd/view.rs:32, 42` |
| 24 | Smaller: "+" on a US keyboard changes the grid, not nudge; default marker names repeat after a delete; Enter while playing can make a marker with a stray length; a view-only location still moves the playhead; a dragged fade defaults to S-curve and a commanded one to equal power | `ui-egui/src/shortcuts.rs:478, 517`; `engine/src/cmd/event.rs:404-424, 518-521`; `ui-egui/src/edit_window.rs:3301` |

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
