# ARA plug-ins (Melodyne, VocAlign, RePitch, VoiceAssist and others)

ARA ("Audio Random Access", by Celemony) lets a plug-in read a clip's whole audio straight from
FreeDAWm instead of hearing it once as it plays, and play the clip back with its edits: pitch and
timing editors, vocal aligners, clean-up tools. FreeDAWm hosts **ARA 2** plug-ins in **VST3** form
on Windows, macOS and Linux (64-bit).

## Finding your ARA plug-ins

FreeDAWm finds ARA plug-ins when it scans for VST3 plug-ins (the plug-in browser's ⏷ menu ›
Rescan plugins). **Clip › ARA Plug-ins...** lists every ARA 2 plug-in found, one per line (name,
maker, version); hover one to see what it can do (its roles, what it analyses, the ARA API
generations it supports) and where it is installed.

**Not listed?** Open the "Not listed?" section of the window:

- Check the plug-in is installed **as a VST3**. Many ARA plug-ins install their VST3 only when
  you tick it in their installer (some install only AAX or AU by default).
- The window lists the folders FreeDAWm scans for VST3 plug-ins; the plug-in's `.vst3` must be in
  one of them (on Windows usually `C:\Program Files\Common Files\VST3`).
- Press **Rescan**, then look again.
- The ARA log (its path is at the bottom of the window; on Windows
  `%APPDATA%\FreeDAWm\logs\ara.log`) lists every VST3 file the scan examined and whether it offers
  ARA 2, and if not, why: "no ARA main factory class in its binary", "its main factory gives no
  ARA factory", "ARA 1 only", or that the scanner could not load the file at all.

## Giving clips to a plug-in

Right-click an audio clip in the Edit window and open its **ARA** submenu:

- **Edit with *plug-in*** gives the clip (or every selected clip, when the clicked clip is one of
  them) to that plug-in and opens its editor.
- On a clip already given to one: **Open *plug-in* Editor**, **Bypass *plug-in*** (the clip plays
  its own audio; the plug-in keeps its edits) and **Remove *plug-in*** (the clip is taken back).
- **ARA Plug-ins...** opens the window, which does the same for the selected clips and shows, per
  clip, what the plug-in does with it (analysing, analysed, plays through the plug-in, or bypassed
  and why).

Double-clicking a clip given to a plug-in opens the plug-in's editor too. Each of these is one undo
step.

Clips on several tracks given to the same plug-in are in one ARA document, each track its own
region sequence, so plug-ins that work on pairs of clips (a guide and a dub, as VocAlign,
Revoice Pro and RePitch do) see all of them. Select both clips before opening the editor.

What can be given to a plug-in: any audio clip, also one that is time-stretched when the plug-in
supports time-stretched regions. Warped (straightened), reversed, looped or transposed clips are
refused with the reason: ARA has no way to describe them.

## Hearing it

A clip given to a plug-in plays through it: the plug-in renders the clip (with its edits) at the
transport position, and FreeDAWm applies the clip's gain, fades and crossfades to what it renders,
exactly as it does to a file. The plug-in's latency is made up for. While its editor is open the
plug-in is also heard when the transport is stopped, so it can preview what you edit.

Bounces, freezes, commits and Consolidate render each such clip through the plug-in
first (in its offline mode), so what they make is what you heard. Process and Render Clip Gain
work on the clip's file, under the plug-in: the plug-in then plays the processed file.

If the plug-in cannot be loaded, refuses to be bound to its document, gives no renderer, or renders
something that is not a number, the clip plays its own audio and the window says why; the ARA log
says what was asked and what failed.

## Saving

The plug-in's work is saved with the session (ARA's own archive of its document, plus the window
state of its editor) and autosaves include it. When you open the session again the plug-in
restores its edits. On a computer without the plug-in, or with a version that cannot read the
archive, the clips play their own audio, the window says so, and the saved archive is kept as it
is, also through later saves, so nothing is lost when you go back to a computer that has it.

## Undo

The plug-in keeps its own undo history for edits you make in its window: use its own undo there.
Those edits are not on FreeDAWm's undo list. FreeDAWm's undo and redo (moving, trimming, splitting,
duplicating, deleting clips) reach the plug-in as one change each, so it always follows the
session. Duplicating or splitting a clip gives the copy the plug-in's edits at that moment (ARA's
clone of the audio modification); from then on the two are edited apart.

## The plug-in's transport buttons

A plug-in may ask FreeDAWm to play, stop, locate or loop (its transport bar). FreeDAWm does it: a
loop range becomes the edit selection, which Loop Playback plays.

## When something goes wrong

Please send the ARA log (`%APPDATA%\FreeDAWm\logs\ara.log` on Windows; the window shows its path).
It names each plug-in, the ARA API generation used, each document made, each clip that plays
through the plug-in or not, every call the plug-in refused, and every programming error the
plug-in itself reported through ARA.

## For developers

FreeDAWm's ARA host is its own Rust bindings to Celemony's ARA 2.3 C header (vendored unmodified,
Apache-2.0) in `crates/ara-host`; no C++ library. The pieces:

- `crates/ara-host`: the bindings, one ARA document per plug-in (`Document`: the session as region
  sequences, audio sources, audio modifications and playback regions, kept in step in editing
  cycles; audio access; archiving; content reading; model updates; playback requests), the binding
  of plug-in instances (`BoundInstance`, which keeps the ARA extension with the instance it came
  from), and a test ARA plug-in (`test_plugin`, feature `test-plugin`) that renders each region as
  its source times a gain it archives.
- `crates/vst3-host`: discovery in the scan, `ara_factory`, and `instantiate_ara` (an instance
  bound with `IPlugInEntryPoint2::bindToDocumentControllerWithRoles` right after it is made, before
  it is activated). `tests/ara_fixture` is the test plug-in as a real `.vst3`.
- `crates/mix/src/ara.rs`: the realtime engine's board of voices (one playback renderer instance
  per clip, read without waiting by the audio thread) and offline renders on the session.
- `crates/engine/src/ara.rs`: `AraHost`, on the UI thread (ARA's model thread): documents, voices,
  offline renders, archives, the editor view and the selection it is told.
