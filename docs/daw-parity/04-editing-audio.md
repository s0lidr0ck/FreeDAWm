# Four-DAW parity, section 4: editing audio on the timeline

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-17 · **Change:** minor (bug fixes and tier-2 work, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
| A. Tools | Selector, Grabber (with separation grab), Trimmer, Smart Tool (with crossfades at joins), Zoomer, Scrubber, Scissors, Mute; fade handles on clips | Small: no waveform pencil |
| B. Edit modes, grid, nudge | Shuffle, Slip, Spot and Grid all enforced; relative grid for moves and trims; an adaptive grid; snap to clip edges, markers, the playhead and zero crossings | Small |
| C. Selection and navigation | Tab to Transient, linked selections, zoom presets, markers, rubber-band clip selection, edit groups, typed selection numbers | Small |
| D. Cut, trim, overlaps | Full cut/copy/paste/repeat set, separate on grid and at transients, trim to close gaps; overwrite, layer or crossfade on overlap | Small |
| E. Clip properties | Clip gain, mute, locks, rating, sync point, notes, per-clip EQ and compressor, real-time transpose, reverse and loop | Small |
| F. Fades | Five shapes with adjustable curve, real-time, auto edge fades, drag handles | Small to medium: thin fade dialog, no batch fades on separate clips |
| G. Consolidate and offline processing | Consolidate, ten offline processes, strip silence | Medium: processing discards handles, no process history, bounce in place is whole-track only |
| H. Takes and comping | Take layers, swipe comping, automatic crossfades | Small |
| I. Timing | Tempo detection, phase-coherent straightening, warp markers you drag and hear while dragging, stretches heard at once | Small |
| J. Pitch and spectral | A real note-level vocal editor with formants; stem separation and de-noise | Medium: no ARA 2, no spectral view, mono sources only |
| K. Arrangement and tracks | Insert and cut time, duplicate, freeze, tempo curves | **Large**: no arranger track, tempo edits do not move audio, no organising folders |
| L. Clip list and sample editing | Clip list with a filter, audition and relink, peak files, sample-level zoom | Medium: no sample editor, no compacting |
| M. Undo and keys | 200 levels, history window with undo and redo by clicking, single-key editing, a key editor | Small |

Three things every one of the four does and FreeDAWm does not: **stretch audio by dragging**
(a clip edge or a warp marker), with a real choice of algorithm; **edits that respect groups and
locks** (grouped clips and grouped tracks edit together, locked clips cannot be changed); and
**audio that follows the tempo map** when a track is set to musical time.

## A. Tools

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Range selector | ● | ● | ● | ● | **Works** |
| Object grabber, drag between tracks, modifier to copy | ● | ● | ● | ● | **Works** |
| Grabber variants (separation grab: drag a selection out of a clip) | ● | ◐ | ◐ | ◐ | **Works**: a Grabber drag that starts inside a range selection over a clip the range cuts into separates the clips at the range's edges on every selected track and drags the pieces inside it (Ctrl copies, Alt keeps the time); the separate and the move are two undo steps |
| Trim tool | ● | ● | ● | ● | **Works**; stops at file ends and neighbours |
| Stretch by dragging a clip edge | ● | ● | ● | ● | **Works**: Alt held as a trim starts (Trim tool or the Smart Tool's edge) stretches the material to the new edge, by the track's Elastic algorithm (pitch-preserving on a plain track, speed on Varispeed) |
| Loop by dragging a clip edge | ● | ◐ | ● | ◐ | **Works**: Shift held as an end trim starts makes it a loop trim, the clip's material repeating to the new end in real time (no copies) |
| Smart / combined tool | ● | ● | ● | ● | **Works**: trim, select, grab, fades, fade curve, clip gain by zone |
| Crossfade from the Smart Tool at a clip boundary | ● | ● | ● | ● | **Works**: the Smart Tool's lower corners where two clips meet (within 8 points of the join) draw a crossfade centred on the join, as wide as the drag either way, through `edit.fades_create` (its short-audio warning included) |
| Scissors / split tool | ◐ | ● | ● | ● | **Works**: the Scissors tool (toolbar) separates the clip under a click there (snapped as edit points are), on the tracks an active edit group spreads the point to; Separate is still a key (B, Cmd+E) |
| Mute tool | ◐ | ● | ● | ● | **Works**: the Mute tool (toolbar) mutes or unmutes the clip under a click, with its clip group and the clips an edit group brings along; Mute Clips is still a key |
| Pencil redraws the waveform at sample level | ● | ○ | ◐ | ● | **Missing**: on audio the pencil draws nothing |
| Zoom tool (click, drag a box) | ● | ● | ● | ● | **Works** |
| Scrub tool with audio | ● | ◐ | ● | ● | **Works**: plays what it passes, both ways, at the drag's speed, through the track's built-in inserts and fader; on a ruler it scrubs every audible track (section 1) |
| Tool chord shown in the tooltip (F6+F7) | ● | — | — | — | **Works**: F6+F7 or F7+F8 held together, in either order, choose the Smart Tool |

## B. Edit modes, grid and nudge

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Slip (free) editing | ● | ● | ● | ● | **Works** |
| Shuffle / ripple editing | ● | ● | ● | ● | **Works**: drag, trim, cut, paste, duplicate, Trim to Selection and the A/S trims (Start / End to Insertion) honour it, the clip keeping its start and what follows closing up. A nudge moves the clips alone, without rippling: as designed (the owner's decision) |
| Shuffle lock | ● | — | — | — | **Works** |
| Spot: place by typed time, with the original timestamp | ● | ◐ | ◐ | ◐ | **Works** |
| Grid, absolute and relative | ● | ● | ● | ● | **Works**: in relative Grid a trim moves the clip's edge by whole grid steps from where it was, keeping its offset from the grid, as a move does; absolute Grid puts it on a line |
| Grid in bars and beats with triplets and dotted | ● | ● | ● | ● | **Works**; typed values read "1/4.", "1/4 dotted", "1/8t" and "1/8 triplet" alike |
| Grid in time, frames and samples | ● | ● | ● | ● | **Works**: lines do not drift and count from the session's timecode start (bug 18, fixed) |
| Grid that adapts to zoom | ◐ | ● | ● | ● | **Works**: Options › Snap › Adaptive Grid: a musical grid's note value follows the zoom, the one whose lines are nearest 16 points apart (bar down to 1/64; no dotted or triplet); a time, frame or sample grid stays as chosen |
| Snap to clip edges, markers and the cursor | ◐ | ● | ● | ● | **Works**: Options › Snap › Clip Edges, Markers and Playhead, within 8 pixels; a nearer grid line wins; the suspend modifier lets go |
| Snap to zero crossings | ○ | ◐ | ● | ● | **Works**: Options › Snap › Zero Crossings moves the insertion and selection edges to the nearest crossing within 5 ms of the audio on the track |
| Modifier to suspend snap while dragging | ● | ● | ● | ● | **Works** for moves, trims and selecting: held through a Selector drag, the selection's end goes free of the grid, clip edges and markers |
| Nudge by a chosen value | ● | ● | ● | ● | **Works**: a range nudges the clips wholly inside it; a clip it only touches stays (bug 16, fixed) |
| Nudge the audio inside a clip (slip contents) | ● | ● | ● | ● | **Works**: Clip › Real-Time › Nudge Contents Earlier / Later moves the audio inside the selected clips by the nudge value while they stay put (a loop turns, a reversed clip moves the way it sounds), stopping at the file's ends; a warped clip is left alone |
| Nudge only the selection's start or end | ● | ◐ | ◐ | ◐ | **Works**: Edit › Selection › Nudge Edit Start / End Earlier / Later, by the nudge value; the edges never cross |

## C. Selection and navigation

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Range across several tracks | ● | ● | ● | ● | **Works** |
| Select several clips as objects and act on only those | ● | ● | ● | ● | **Works**: Delete, Cut and Copy act on the selected clips only (bug 1, fixed) |
| Rubber-band selection of clips | ◐ | ● | ● | ● | **Works**: a Grabber drag from empty space selects the clips the box touches across tracks (Shift adds) |
| Select all on a track; select all following; select by colour, name or mute | ◐ | ● | ● | ● | **Works**: Edit › Selection › Select All Clips on Track, Select All Following (from the insertion, the selected tracks or every track), Select Clips with Same Name, with Same Color, Select Muted Clips |
| Extend selection to session start or end | ● | ● | ● | ● | **Works**: Edit › Selection › Extend Edit to Session Start / End (the end of the last clip) |
| Type the selection's start, end and length | ● | ● | ● | ● | **Works**: a click on Start, End or Length in the counters opens a field that takes the value in the main counter's format (a length counts from the start: bars, beats and ticks in its meter) |
| Tab to clip boundaries, Tab to Transient | ● | ● | ● | ● | **Works** |
| Link timeline and edit selection | ● | — | — | — | **Works** |
| Link track and edit selection | ● | ◐ | ● | ● | **Works**, as designed: FreeDAWm keeps one selection list, so the selected tracks and the edit selection are always linked (the toolbar button stores a value nothing needs) |
| Edit groups: an edit on one grouped track applies to all | ● | ● | ● | ● | **Works**: in an active group with Edit on (Track › Group…; not while groups are suspended) a clip grabbed, moved, nudged, trimmed or faded on one track brings the clips the other members hold at that time, as a clip group does (`alone` for one); a range selected on one track spreads to the group, so separate, cut, copy, paste and fades over it apply to all. One undo step |
| Zoom: horizontal, vertical, waveform height, presets, toggle, to selection | ● | ● | ● | ● | **Works**; zoom-to-selection and fit size to the timeline on screen (bug 23, fixed) |
| Scroll modes (page, continuous, none) | ● | ● | ● | ● | **Works** |
| Markers on several rulers, a marker list | ● | ● | ● | ● | **Works** |
| A location that recalls zoom, track heights and shown tracks | ● | ○ | ◐ | ◐ | **Works**: Memory Locations' + View keeps the zoom, scroll, track heights and which tracks are shown, and recalling it puts them back without moving the playhead; any marker made with `view` recalls them too |
| Markers attached to a track | ● | ○ | ○ | ◐ | **Works**: Add Track Marker Here in a track lane's menu puts a marker on that track, drawn down its lane with its name; `markers.add`/`markers.edit` take `track` (null moves it back to the rulers) |
| Session overview strip | ● | ● | ◐ | ● | **Works** |

## D. Cut, copy, trim and overlaps

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Cut, copy, paste, clear, duplicate, repeat N times | ● | ● | ● | ● | **Works**, crossfaded pairs included (bug 2, fixed); Repeat leaves automation behind, Duplicate takes it. Audio clips pasted into another session bring their audio along as new sources of that session (written to its Audio Files on save; reused by later pastes); refused, naming the file, when that audio was never loaded or is at another sample rate |
| Paste repeated to fill the selection | ● | ◐ | ◐ | ● | **Works**; the fill is exactly the selection's length, so in Shuffle too nothing after it moves |
| Insert silence; insert and cut time on all tracks | ● | ● | ● | ● | **Works**: every playlist and take layer, automation, markers, key changes and the tempo and meter map move (bug 7, fixed); refused inside a tempo ramp |
| Separate at the cursor or selection, on the grid, at transients | ● | ● | ● | ● | **Works**; transient cuts are found per track, so multi-mic cuts do not line up |
| Heal a separation | ● | ◐ | ● | ● | **Works**: pieces that sound alike are joined, their clip-gain lines too; pieces that differ stay apart (bug 6, fixed) |
| Trim to selection, start or end to the cursor | ● | ● | ● | ● | **Works** |
| Trim clips to close gaps | ◐ | ● | ● | ● | **Works**: Edit › Trim › To Close Gaps runs each selected clip's end on to the next clip's start, as far as its file (or loop) has audio |
| Overlap modes: overwrite, keep both, crossfade on overlap | ◐ | ● | ● | ● | **Works**: Options › Overlap › Overwrite (the default), Layer (audio clips stay whole underneath; the newest is on top and heard, the lower one plays again where uncovered, crossing over through the top clip's fades or a 1 ms ramp) or Crossfade (overwrite with a 10 ms crossfade at each join, out of the handles). Moves, pastes, duplicates and recorded takes follow it; MIDI and video clips always overwrite; kept by undo like the Edit mode (`edit.overlap_mode`) |
| Clip order (front/back) where clips overlap | ● | ● | ● | ● | **Works**: each audio clip has a layer; Send to Back and Bring to Front put it below or above the clips it overlaps, which changes what is heard there; the Edit window draws the top one last and a click picks it |
| Drag a file from the clip list or the desktop to a track | ● | ● | ● | ● | **Works**: whole files, and clips from the Clip List (a copy lands where dropped, snapped, by `clip.copy_to`, onto a track of its kind) |

## E. Clip properties

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip gain as one value, with bypass and render | ● | ● | ● | ● | **Works** |
| Clip gain as a drawn line | ● | ● | ◐ | ● | **Works**: on a track showing Clip Gain a drag on the line adds a breakpoint and moves it, a breakpoint drags between its neighbours, Alt-click deletes it (`clip.gain_line`) |
| Clip mute | ● | ● | ● | ● | **Works** |
| Clip lock (position, or all edits) | ● | ● | ● | ● | **Works**: a command that would change an edit-locked clip is refused and undone (bug 4, fixed) |
| Clip name, colour, rating | ● | ● | ● | ● | **Works** |
| Clip groups that select and move as one | ● | ● | ● | ● | **Works**: selecting a grouped clip selects its group; a drag from an unselected member takes the group; a trim moves the same edge of every member as far, and a fade drawn on one goes on all (`alone` for one) |
| Loop a clip without making copies | ● | ◐ | ● | ◐ | **Works**: Clip › Real-Time › Loop in Place… (N times, or by the loop trim) repeats the material as the clip plays, and its waveform is drawn repeat by repeat; a start trim keeps each repeat where it was; `off` ends it. Clip › Loop… still makes copies |
| Clip transpose and fine tune, non-destructive | ◐ | ● | ● | ● | **Works**: Clip › Real-Time › Transpose… (semitones, fractions as cents, ±24) plays the clip transposed in real time at its length, by a two-grain shifter reading the file (no state, so locates and loops are exact), heard at once; in the app a background render by Pitch Shift's algorithm then replaces it at the same length (derived, not saved, kept across undo; not an undo step), a clip playing a Clean Vocal mix included (the render is of the mix, made again when the mix changes) |
| Clip stretch ratio | ● | ● | ● | ● | **Works**: trims, splits, Trim to File and fade handles count file samples at the clip's ratio (bug 3, fixed) |
| Reverse a clip without a new file | ○ | ● | ● | ◐ | **Works**: Clip › Real-Time › Reverse in Place plays the clip's window backwards in real time; trims take audio off the matching end; the waveform is drawn reversed too, looped or not |
| Per-clip effects in real time | ● | ● | ◐ | ◐ | **Works** (EQ, compressor, gain), kept by a split's pieces and every copy (bug 5, fixed); made off the audio thread when the session changes, not when a clip first plays (bug 13, fixed) |
| Sync point | ● | ● | ● | ● | **Works** |
| Clip notes | ○ | ● | ● | ● | **Works**: Notes… in the clip's menu; a clip with notes shows a mark in its name bar, and the notes beside the pointer on it; kept by splits and copies |

## F. Fades

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Fade in, fade out, crossfade, calculated in real time | ● | ● | ● | ● | **Works** |
| Shapes: linear, equal power, S-curve, adjustable slope | ● | ● | ● | ● | **Works**, plus exponential and logarithmic |
| Drag fade handles on the clip | ● | ● | ● | ● | **Works** for one clip, and a crossfade by dragging from where two clips meet (Smart Tool, row A) |
| Fade dialog with preview, length and separate in and out shapes | ● | ◐ | ◐ | ● | **Works**: a shape and bend for each side (a crossfade's two sides too), a drawn preview, presets (built-in and saved); the length is the selection's |
| Batch fades over many clips | ● | ● | ● | ● | **Works**: the fade dialog's batch option fades both ends of every clip inside the selection, at a length in ms |
| Crossfade placement (before, across, after the cut) | ● | ◐ | ● | ● | **Partial**: follows where the selection sits |
| Warning when the audio beyond the cut is too short | ● | ◐ | ◐ | ● | **Works**: a crossfade made shorter on one side than the selection, for want of audio beyond the cut, says so in the status line (`edit.fades_create` returns `short` and a `warning`) |
| Automatic short fades at clip edges | ● | ● | ◐ | ● | **Works** in playback, bounces and exports (1 ms default), and Consolidate renders them too, as heard |
| Delete fades in a selection | ● | ● | ● | ● | **Works**: only the fades and crossfades in the selection go (bug 9, fixed) |

## G. Consolidate and offline processing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Consolidate a selection to one new file | ● | ● | ● | ● | **Works** |
| Bounce a clip or range in place, with or without effects | ● | ● | ● | ● | **Works**: with effects, the track menu's Commit Selection renders the selection through the inserts to a new track at its place and mutes it on the source, which stays active (whole-track Commit as before); without effects, Consolidate. The new track keeps the pan, fader, pan and send automation (bug 10, fixed) |
| Export clips as files | ● | ● | ● | ● | **Works**: each clip alone, a muted one as it sounds unmuted (bug 22, fixed) |
| Normalize, gain, reverse, invert, DC removal, pitch, stretch | ● | ● | ● | ● | **Works**; each writes a new file; fades, sync point and warp markers follow (bug 20, partly fixed) |
| Run a plug-in on a clip offline | ● | ● | ● | ● | **Works** |
| Processed clip keeps audio beyond its edges (handles) | ● | ● | ● | ● | **Works** for processes that keep the length: up to 1 s (`handle_ms`) of the file each side is processed too, the clip's own window rendered as it played (a Clean alternate's edges are not faded by the longer render); stretch, pitch and varispeed render the clip only |
| Process history that can be changed or removed later | ○ | ◐ | ○ | ● | **Missing** |
| Strip silence with threshold, minimum length and padding | ● | ● | ● | ● | **Works**; muted clips are left whole (bug 11, fixed) |
| Strip silence preview before committing | ● | ● | ● | ● | **Works**: while the Strip Silence dialog is open, the Edit window shades what it would remove from the selection, worked out again as the settings change (`edit.strip_silence_preview`) |

## H. Takes and comping

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Alternate playlists or versions per track | ● | ● | ● | ● | **Works**: new, duplicate, switch |
| Delete one playlist | ● | ● | ● | ● | **Works**: any playlist but the one playing is a take layer, and Delete Take in its menu deletes it (one the comp uses: Remove and Delete); Delete Unused Playlists deletes them all |
| Take lanes under the track | ● | ● | ● | ● | **Works** |
| Swipe comping | ◐ | ● | ● | ● | **Works** |
| Copy a selection up to the main playlist | ● | ● | ● | ● | **Works**; this route adds no crossfades |
| Automatic crossfades at comp joins | ◐ | ● | ● | ● | **Works** (10 ms equal power) |
| Name and rate takes | ● | ● | ● | ● | **Works** |
| Flatten a comp | ● | ● | ● | ● | **Works**: Flatten Comp in a take layer's menu keeps the comp as plain clips, crossfades and all, and deletes the take layers (one undo step); Delete Unused Playlists keeps the layers the comp came from (bug 19, fixed) |
| Show only takes that match the selection | ● | ○ | ○ | ○ | **Partial**: cycling takes over the selection (Up/Down, `comp.cycle`) already offers only the layers with audio there. Left: hiding the other layer lanes while a selection is made, which needs the lane layout (`layer_rows`, `layers_height`) to see the selection |
| Comp grouped tracks together (drums) | ● | ● | ● | ● | **Works**: a swipe (or click) on one track of an active edit group comps the same take layer over the same range on the group's other tracks where that layer has audio, in one undo step (`comp.swipe` `alone` for one track; not while groups are suspended). Takes are matched by layer number, as tracks recorded together number them |

## I. Timing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Warp markers you drag to move a hit | ● | ● | ● | ● | **Works**: on a track showing Warp, a click on a clip adds a marker, a drag moves it and the material on each side stretches to fit (by the track's Elastic algorithm, one pass with no joins), Alt-click deletes it; Straighten's markers show and edit the same way (`clip.warp_marker`). The marker moves as it is dragged (several times a second), so the new warp is heard through the real-time stretcher while dragging, its render made in the background; the whole drag is one undo step |
| Choice of stretch algorithm (polyphonic, rhythmic, monophonic, tape) | ● | ● | ● | ● | **Works** offline: the track's algorithm sets how the stretch splices (Rhythmic short and tight, Monophonic searching wide, Polyphonic general, X-Form long and smooth); varispeed plays in real time through the band-limited resampler (section 1). Not real-time |
| Transient detection with a sensitivity control | ● | ● | ● | ● | **Works**: the Beat Detective window has a Sensitivity slider (its Separate uses it too); `edit.separate_at_transients` takes `sensitivity` |
| Quantize audio to the grid with strength and swing | ● | ● | ● | ● | **Works** for whole clips: Clip › Quantize Audio… moves each by its sync point with a strength and a swing; no slicing at hits (Beat Detective) |
| Quantize multitrack drums keeping phase | ● | ● | ● | ● | **Works** as Straighten, to a steady tempo only |
| Slice, conform and smooth (Beat Detective style) | ● | ◐ | ◐ | ● | **Partial**: detect and separate; no conform, no gap filling |
| Extract a groove and apply it | ● | ● | ● | ● | **Missing** |
| Detect tempo from audio and build a tempo map | ◐ | ● | ● | ● | **Works**: constant, follow and smooth modes |
| Audio on a musical-time track follows tempo edits | ● | ● | ● | ● | **Works**: on a tick track a clip moves to its bar; on an Elastic one it also takes the new length (Varispeed by its stretch, the others by a pitch-preserving render from the original) |
| Conform a clip to the session tempo | ● | ● | ● | ● | **Works**: varispeed on a Varispeed or plain track, a pitch-preserving render on an Elastic one (bug 12, fixed) |
| Re-stretch after trimming | ● | ● | ● | ● | **Partial**: a second stretch rebuilds from the first-stretch length and discards later trims |

## J. Pitch and spectral editing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Built-in note-level pitch editor | ◐ | ◐ | ● | ● | **Works**: pitch, drift, vibrato, formant, per-note gain; monophonic sources |
| Pitch editor on a stretched clip | ● | ● | ● | ● | **Partial**: the note commands refuse a stretched clip with "tuning can't follow a stretched clip — commit the stretch first" rather than play it untuned (the design's first step). Left: analyse the stretched render (a source of its own) and map note times through the ratio |
| Automatic pitch correction in real time | ○ | ○ | ● | ● | **Not planned here**: real-time pitch correction was dropped by the owner; offline per clip (centre and snap to key) works |
| ARA 2 for third-party editors | ● | ● | ● | ● | **Not planned here**: needs Celemony's ARA SDK, which is not in the tree |
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
| Folders that only organise, and edit a closed folder as one clip | ● | ● | ● | ● | **Missing**: every folder is a routing folder; no folder editing. Left to the mixing section's follow-up, which has the same row (doc 03); design below |
| Lock a whole track | ◐ | ● | ● | ● | **Works**: Lock Track in the track's menu; a command that would add, change or remove a clip on any of its playlists, or record-arm it, is refused and undone (Insert and Cut Time still move it) |
| Arranger track: named sections you move, copy and repeat | ○ | ● | ● | ● | **Missing** |
| Scratch pads or alternative arrangements | ○ | ● | ● | ◐ | **Missing** |
| Tempo changes, ramps and curves; meter changes | ● | ● | ● | ● | **Works** |
| Tap tempo | ● | ● | ● | ● | **Works**: the Tempo Operations window's Tap button; from the second tap the tempo in effect at the insertion point takes the average of the last eight taps (a pause of 2 s starts over), one undo step per run of taps |
| Identify a beat to set the tempo from a selection | ● | ◐ | ● | ● | **Works**: the tempo after the selection resumes where it ends (bug 8, fixed) |
| Video stays in sync through edits; snap to frames | ● | ● | ● | ● | **Works** |

## L. Clip list and sample-level editing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| List of clips and files with a search | ● | ● | ● | ● | **Works**; shows the active playlists only |
| Audition, rename and locate from the list | ● | ● | ● | ● | **Works**: a click selects the clip and scrolls to it when off screen; Alt-click or Audition plays it; Rename… from its menu |
| Select and remove unused clips | ● | ● | ● | ● | **Works**: Select Unused Files picks the files no clip plays; Clear removes them |
| Relink missing audio files | ● | ● | ● | ● | **Works**: the Clip List marks a file the session could not find as missing, and Relink… in a file's menu reads it from another file (`file.relink_audio`), its clips playing again from where they were; a file of another length or width is taken with a warning. One file at a time; no search of a folder |
| Compact files to what is used | ● | ● | ● | ● | **Missing** |
| "Capture" a selection as a new clip in the list | ● | ◐ | ◐ | ● | **Missing**: the command only renames the clip "Captured" |
| Audio file / sample editor window | ○ | ● | ● | ● | **Missing** |
| Waveform from peak files, down to single samples, rectified and outline views | ● | ● | ● | ● | **Works**; a stretched clip's waveform is drawn unstretched |

## M. Undo and keyboard

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Multi-level undo and redo, every edit covered | ● | ● | ● | ● | **Works** (200 levels) |
| Undo leaves zoom, scroll and the current tool alone | ● | ● | ● | ● | **Works**: undo and redo keep the zoom, scroll, waveform height and tool, as they keep the Edit mode |
| Undo history list | ● | ● | ● | ● | **Works**: the undone steps are listed after the current one, dimmed; a click on any step undoes or redoes to it |
| Single-key editing commands | ● | ● | ● | ● | **Works** |
| Editable key bindings | ● | ● | ● | ● | **Works**. Fixed, by design: the context keys that act by where the pointer or focus is or name a family of keys the binding strings cannot hold (Space, Shift/Cmd+Space, F12, Enter, F1–F10, Alt+1–4, `, Tab, = / keypad + −, Shift/Alt+Shift with = −, Cmd+=, Delete, R / T, and single-key editing), and five commands those keys run (track height up/down, clip gain nudge up/down, search). The key editor lists them as "(fixed)" and refuses to give them away |

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
| 13 | **Fixed.** The first time a clip with effects played, its buffer and processors were created inside `render`. They are made when the session changes (`sync_clip_fx`), and for playback on the UI thread: the player's `BuiltinPrep` makes the modules a session newly needs and sends them with it (`PreparedClipFx`), so `sync` on the audio thread only seats them; modules no longer needed are retired rather than dropped, and `render` only looks them up and sets parameters. The clip buffer of a track that gets its first clip effect is still sized in `sync`. Tests: `mix` `clip_effects_are_prepared_before_the_clip_plays`, `sync_on_the_render_path_makes_no_builtin_module` | `mix/src/lib.rs` (`sync_clip_fx`, `ClipFx::prepare_modules`, `process_strip`) |
| 14 | **Fixed.** Send to Back / Bring to Front reordered the playlist without re-sorting, and the next edit's sort undid the order anyway. Overlapping clips were mixed together, so there was no order to keep: the commands left the playlist alone. Since the overlap modes (tier 2) they change the clip's layer, never the playlist's order. Test: `cmd::clip::tests::layer_commands_keep_the_playlist_in_time_order` | `engine/src/cmd/clip.rs` (`layer`) |
| 15 | **Fixed.** Dragging a folder track moved the folder without its members. `track.move` now moves a folder with the tracks inside it at any level unless a count is given (`edit::folder_block`), and the drag passes that count and refuses a drop inside the folder itself. Test: `engine/tests/editing_parity.rs` (the drag itself not checked in a GUI) | `engine/src/cmd/track.rs` (`move_track`); `ui-egui/src/edit_window.rs` |
| 16 | **Fixed.** Nudge with a partial selection moved whole clips; clips nudged past zero were each clamped, closing the gaps between them. A range selection now nudges only the clips wholly inside it, and a move left stops at the session start for all moving clips together. Test: `engine/tests/editing_parity.rs` | `engine/src/cmd/edit.rs` (`nudge_clips`); `engine/src/edit.rs` (`move_clips`) |
| 17 | **Fixed.** A Shuffle trim rippled clips from just after the clip's start (its crossfade partner included) but automation from its end, so points under the crossfade were deleted. Automation now moves from the same place as the clips. Test: `engine/tests/editing_parity.rs` | `engine/src/edit.rs` (`trim_clip`) |
| 18 | **Fixed.** Time and frame grids rounded the step to whole samples, so 29.97 fps at 48 kHz drifted about 0.9 s an hour. Line *k* is now at *k* × the exact step, rounded (`GridValue::exact_step`). Time, frame and sample lines now count from the session's timecode start: every grid call takes one `GridCtx` (rate, tempo map, timecode rate, timecode start; `Session::grid_ctx`) instead of three loose values. Tests: `time` `frame_lines_do_not_drift_over_an_hour`, `absolute_lines_count_from_the_timecode_start` | `time/src/grid.rs` (`GridCtx`, `line_at_or_before`, `next_line`) |
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

**Status of tiers 2 and 3 (2026-11-16, second round).** Done: undo and redo leave the zoom,
scroll and tool alone; clip groups select, drag, trim and fade as one; rubber-band clip selection;
snap to clip edges, markers and zero crossings; stretch by Alt-dragging a clip edge; the Elastic
algorithm menu chooses a different (offline) stretch; audio on tick tracks follows tempo-map
edits; quantize audio with strength and swing; an editable clip gain line; the fade dialog with a
shape and bend per side, presets and batch fades; Commit Selection (bounce a range in place
through the inserts); processing keeps handles; the Clip List auditions, renames, scrolls to
clips and finds unused files; the Scrubber plays audio; warp markers drawn and editable (third
round); clip transpose, real-time clip loop and reverse (third round). Overlap modes with a front/back order and edit groups editing together are done (fourth round).
Not done: slip contents, typed selection numbers, adaptive
grid; relink, compact, capture; strip-silence preview; a short-handle warning and crossfade by
dragging; every tier-3 item; ARA 2 hosting.

**Status after round three (2026-11-17).** Built: typed selection numbers, selection commands
(extend to the session ends, select by track, following, name, colour or mute, nudge one edge),
slip contents, trim to close gaps, Shuffle for the menu trims, view-recalling locations, track
markers, clip notes, track lock, tap tempo, Flatten Comp, Strip Silence preview, relink, a
short-crossfade warning, the Smart Tool chord, redo from the history, the looped clip's waveform,
warp markers heard while dragged, and Clean Vocal clips' transpose render. Not done: every item
in the designs below.

**Status after the round-three follow-up (2026-11-17).** Built: Scissors and Mute tools, the
Smart Tool's crossfade at a join (by dragging), separation grab, snap to the playhead, an adaptive
grid, relative-grid trims, dragging a clip from the Clip List, comping grouped tracks together,
soloing a take in place of the comp, and audio clips pasted into another session bringing their
audio along. Decided: nudge in Shuffle does not ripple, and the track and edit selections are
always linked (both as designed). Not done: the designs below that are not marked built.

**Where FreeDAWm is already level or ahead:** Spot mode with the original timestamp (only Pro
Tools matches it), fade shapes with a continuous curve, swipe comping with automatic crossfades,
phase-coherent multitrack straightening in one command, a built-in vocal editor with formant
control (Pro Tools and Studio Pro need Melodyne), stem separation and de-noise as clip processes,
and audio to MIDI for polyphonic and drum material.

## Designs (round three)

Short designs for the open rows too large to build in one step. Each names the first tested step.
None is built yet.

- **Arranger track** (K). A session-level `arranger: Vec<Section { id, name, start, end, color }>`
  (non-overlapping, sorted), drawn as a ruler. Commands `arranger.add/rename/delete` and
  `arranger.move {section, to}` / `arranger.copy` / `arranger.repeat {count}`, each built on the
  Insert/Cut Time machinery of bug 7 (every playlist, take layers, automation, markers, key changes
  and the tempo and meter map move together): a move is "copy the section's time to the target,
  then cut the source". First step: the model, the ruler and add/rename/delete; then copy (insert
  time + paste of everything in the span); then move and repeat.
- **Folders that only organise; edit a closed folder as one clip** (K). `Track::folder_kind:
  Routing | Organise`; an organise folder has no bus, its members route as before, and Convert
  toggles it. Editing a closed folder: its lane draws the members' clips summed as blocks; a
  selection on it spreads to the members, as an edit group's does (the edit-group spreading of
  round two is the mechanism). First step: the kind, routing that ignores an organise folder, and the
  convert command.
- **Scratch pads / alternative arrangements** (K). A whole-session alternative: `Session::scratch:
  Vec<Arrangement { name, playlists: per-track active-playlist index, markers }>` switched by a
  command, built on playlists (each track gets a playlist per arrangement). First step: save and
  switch, one undo step each.
- **Audio file / sample editor with a waveform pencil** (L, A). A window on one source: the waveform
  at sample level (the waveform module already draws sample lines), a selection, and destructive
  edits that write a new source and point the clips at it (as Process does, handles kept): gain,
  silence, reverse, cut/copy/paste inside the file, and the pencil redrawing samples between two
  points (linear in the drawn samples, a short crossfade at both ends). First step: the window,
  selection and the pencil on a copy of the file, one undo step.
- **Spectral view** (J). A per-track view `spectral`: an STFT image cached per source in the peak
  cache's folder (log frequency, colour by dB), drawn in place of the waveform. Spectral editing
  (Cubase) is a later step: a rectangle selection that attenuates bins and writes a new source.
- **Process history** (G). `Clip::processes: Vec<ProcessStep { id, params, from_source }>` kept by
  Process; "Remove" re-renders from `from_source` of the step removed with the later steps
  re-applied in order (or simply plays the earlier source when it was the last). First step: record
  the chain and offer removing the last step.
- **Groove from audio and apply** (I). Transients (the detector Tab to Transient uses) quantized to
  the grid make a `Groove` like `event.groove_from_clip`'s; applying moves audio by the warp-marker
  path (markers at hits, moved to the groove). Needs the Beat Detective conform below.
- **Beat Detective conform and smoothing** (I). Detect hits (exists), add warp markers at them and
  move each to the grid by a strength (the warp-marker render already stretches between markers):
  conform without cutting. "Separate and smooth" (cut at hits, move pieces, fill gaps by trimming
  the previous piece and crossfading) is the classic alternative, built on separate, nudge and
  trim to close gaps.
- **Pitch editor on a stretched clip** (J). The vocal editor analyses `clip_heard_window`; for a
  stretched clip it should analyse the stretched render (the elastic render is a source of its
  own) and map note times through the stretch ratio. First step: refuse with a clear message rather
  than play untuned; then analyse the render.
- **Re-stretch after trimming** (I). Keep the clip's original source and its window in the
  original's samples (`edit.values` already remembers the original for Remove Warp); a second
  stretch renders from that window rather than from the first-stretch length.
- **Real-time stretch algorithms** (I). The live stretcher is one algorithm; the track's Elastic
  choice picks only the offline render. Make the live grain size and search width follow the
  track's algorithm (short and tight for rhythmic, wide search for monophonic). Shared with
  section 1.
- **Capture into the Clip List and dragging a clip from it** (L, D). `Session::captured: Vec<Clip>`
  (clips kept in the list, not on a track); `clip.capture` stores the selection's clips there;
  the Clip List lists them and a drag places a copy, as whole files are placed today.
- **Compact** (L). For each source, the union of the windows its clips (all playlists and undo
  history) use plus handles; write a new file of those spans and point the clips at it with new
  offsets. Needs a confirmation (it is the one destructive file operation) and runs off the UI
  thread like Commit.
- **Smaller UI items** (A, B; built in the follow-up except the last: a stretched clip's
  waveform is still drawn unstretched): a Scissors and a Mute tool (two `Tool` variants: click separates or
  mutes the clip under the pointer, edit-group aware); separation grab (a Grabber drag inside a range
  selection separates at its edges, then drags the middle); a crossfade made by the Smart Tool at a
  clip boundary (bottom corner zone over two touching clips → `edit.fades_create` over the drag);
  an adaptive grid (a grid value `adaptive` choosing the note value whose line spacing is nearest
  16 px at the zoom); relative-grid trims (the trim tool moving the edge by whole grid steps from
  where it was); the snap-suspend modifier applied to selecting; nudge in Shuffle rippling what
  follows; a stretched clip's waveform mapped through its ratio.

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
| 2026-11-16 | minor | Tier 2: stretch by dragging a clip edge (Alt-trim) |
| 2026-11-16 | minor | Status of tiers 2 and 3 after the second round |
| 2026-11-16 | minor | Tier 2: the Elastic algorithm menu chooses a different stretch |
| 2026-11-16 | minor | Tier 2: audio on tick tracks follows tempo-map edits |
| 2026-11-16 | minor | Tier 2: processing keeps handles |
| 2026-11-16 | minor | Tier 2: Commit Selection (bounce a range in place with effects) |
| 2026-11-16 | minor | Tier 2: fade dialog with a shape for each side, presets and batch fades |
| 2026-11-16 | minor | Tier 2: audio quantize with strength and swing |
| 2026-11-16 | minor | Tier 2: the clip gain line is edited on the clip |
| 2026-11-16 | minor | Tier 2: snap to clip edges, markers and zero crossings; rubber-band clip selection; clip groups trim, fade and drag as one |
| 2026-11-16 | minor | Tier 2: the Clip List auditions, renames, scrolls to clips and finds unused files |
| 2026-11-16 | minor | Bug 24 fixed: "=" nudges later beside "−"; dragged fades default to equal power |
| 2026-11-16 | minor | Bug 13 fixed: Clip Effects modules are made off the audio thread |
| 2026-11-16 | minor | Tier 2: warp markers drawn on the clip, added, dragged and deleted |
| 2026-11-16 | minor | Tier 2: clip transpose, reverse and loop in place, in real time; the loop trim |
| 2026-11-16 | minor | Bug 18 fixed: grid lines count from the timecode start |
| 2026-11-16 | minor | The Scrubber is heard through the strip, and scrubs on the ruler |
| 2026-11-16 | minor | Processing with handles keeps the clip's sound inside its edges (a Clean Vocal mix is no longer faded back to the original at the old edges) |
| 2026-11-16 | minor | Tier 2: overlap modes (overwrite, layer, crossfade) with a front/back order |
| 2026-11-16 | minor | Tier 2: edit groups move, trim, fade, separate and cut together |
| 2026-11-16 | minor | Stretches and warp-marker moves are heard at once through the real-time stretcher while the render is made |
| 2026-11-16 | minor | A reversed clip's waveform is drawn reversed |
| 2026-11-16 | minor | Clip transpose gets a high-quality background render |
| 2026-11-17 | minor | Round three reconciliation: transient sensitivity and edge fades in exports were already built; ARA 2 and real-time pitch correction not planned here |
| 2026-11-17 | minor | Typed grid values such as "1/4 dotted" and "1/8 triplet" parse |
| 2026-11-17 | minor | F6+F7 and F7+F8 choose the Smart Tool |
| 2026-11-17 | minor | The Undo History lists undone steps and redoes to one by clicking |
| 2026-11-17 | minor | Selection commands: extend to the session start or end, select clips on a track, following, by name, colour or mute; nudge one edge of the selection |
| 2026-11-17 | minor | Nudge the audio inside a clip |
| 2026-11-17 | minor | Tap tempo |
| 2026-11-17 | minor | Lock a whole track |
| 2026-11-17 | minor | Clip notes |
| 2026-11-17 | minor | Trim clips to close gaps |
| 2026-11-17 | minor | Consolidate keeps the automatic edge fades, as playback does |
| 2026-11-17 | minor | Memory locations that recall the view |
| 2026-11-17 | minor | The selection's start, end and length can be typed |
| 2026-11-17 | minor | Markers on a track |
| 2026-11-17 | minor | Trim to Selection and the A/S trims follow Shuffle |
| 2026-11-17 | minor | A looped clip's waveform is drawn repeat by repeat, reversed when the clip is |
| 2026-11-17 | minor | A crossfade short of audio beyond the cut warns |
| 2026-11-17 | minor | Flatten Comp; deleting one playlist and Repeat to Fill in Shuffle were already covered |
| 2026-11-17 | minor | Transposed clips with Clean Vocal get the background render too |
| 2026-11-17 | minor | A warp marker is heard while it is dragged |
| 2026-11-17 | minor | Strip Silence previews what it would remove |
| 2026-11-17 | minor | Relink a missing audio file from the Clip List |
| 2026-11-17 | minor | Round three: status, and designs for the rows too large for one step |
| 2026-11-17 | minor | The snap-suspend modifier works while selecting |
| 2026-11-17 | minor | Follow-up: Scissors and Mute tools; Smart Tool crossfades at joins; separation grab; snap to the playhead; adaptive grid; relative-grid trims; clips dragged from the Clip List; grouped tracks comp together; Solo Take; pasting audio into another session brings it along; Shuffle nudge and linked selections as designed; fixed keys listed |
