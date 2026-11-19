# Four-DAW parity, section 8: what only one of them has

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-18 · **Change:** minor (tier 4) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). Other product names below (Dolby Atmos, Melodyne, Splice,
> Moises and the instruments the Band can drive) belong to their owners. FreeDAWm is not affiliated
> with or endorsed by any of them. See [NOTICE](../../NOTICE).

The last section ([1: playing audio](01-playback-audio.md),
[2: recording audio](02-recording-audio.md), [3: mixing](03-mixing.md),
[4: editing audio](04-editing-audio.md), [5: built-in effects](05-built-in-effects.md),
[6: recording MIDI](06-midi-recording.md), [7: writing MIDI](07-midi-writing.md)).

**Scope of this section:** the features each of the four is known for, that the others lack or
do differently, and whether FreeDAWm has an answer; then what FreeDAWm has that none of the four
does. Sections 1–7 compared what the four share. This one compares what sets them apart, so there
is no "all four have it" list: the question for each row is whether it is worth having.

## How this was measured

- **FreeDAWm:** read from the code in the main working folder (0.9.11 plus the part of the work
  on sections 1–7 that had landed there), not from the docs. A feature that another agent has
  since finished on the branch may be marked lower here than it now is; the earlier sections' own
  rows are the record for those. Re-read by hand: higher-order Ambisonics encodes first order
  only, the object/bed switch is read only by drawing code, timing alignment in Fit to Lead was
  refused (since built), nothing outside the video crate calls its encoder.
- **The four DAWs:** from their manuals and from release coverage of the versions above (listed
  under Sources). "Signature" is a judgement: a feature is listed under the product most
  identified with it, even where another has since added something similar.

**Legend.** FreeDAWm: **Works** · **Partial** · **Flag only** (a switch, command or menu item
exists and is stored, but nothing acts on it) · **Missing**.

## Summary

Of 67 signature features checked, FreeDAWm has a working answer to 28 and a partial one to 26; the
other 13 are not planned here, each with its reason (re-counted 2026-11-18, after rounds three and
four; the first count, of 60, found about 12, 28 and 20).

- **Where it answers well:** virtual players (the Band), stem separation, tempo detection and
  conforming, vocal pitch editing with harmony, clip effects, a sampler track, amp models,
  summing folders, and remote recording over the internet.
- **Where it has nothing:** a clip launcher (Pro Tools Sketch, Studio Pro Launcher, Logic Live
  Loops: three of the four now have one), a mastering or release page, Dolby Atmos objects and
  renderer, AAF/OMF exchange, a loop library, ARA. (A set list for playing live, within one
  session, has since been built.) (Modulators and
  articulation maps have since been built.) (A control room with cue mixes, mixer snapshots and Mackie/HUI surfaces have
  since been built in sections 3 and 6.)
- **What it has that none of the four does:** an interface for scripts and AI agents that reaches
  every command; a mix analyser that asks an AI model for advice and draws it on the EQ; local
  models for karaoke, drum splitting, text-to-MIDI and whole-song generation; a build-your-own
  room reverb and impulse capture of gear; a resonance suppressor and a masking EQ in the box;
  lyric-writing tools with a teleprompter.

Four controls once promised a signature feature and did nothing: the Object/Bed switch and the
Renderer window (Atmos; now labelled as not rendering), QuickPunch and TrackPunch (both now
work), and Ambisonics above first order (2nd and 3rd order now encoded). See "Things that
promise more than they do" below.

## A. Pro Tools

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Hardware DSP mixing with near-zero monitoring latency (HDX, Carbon) | **Not planned here** | Needs one maker's DSP hardware. A software low-latency monitoring path, for audio tracks |
| AAF and OMF exchange with video editors | **Not planned here** | DAWproject both ways instead. Large formats with little public documentation, and no AAF or OMF files from Pro Tools or a video editor here to build and test against (the owner's call whether to source some) |
| Import chosen tracks and settings from another session | **Works** | Import Session Data, with audio and sampler samples |
| Field-recorder workflow: match and expand location recordings by metadata | **Works** (step 1 `b6a6714`; steps 2 and 3 `b2ce0f0`) | Broadcast WAV is read and written; a WAV's iXML (project, scene, take, tape, note, each channel's name) is read on import and kept with the session (`Session::field_info`, `audio_io::ixml`), and the Clip List shows the scene and take beside the file and its clips, so its search finds them. An audio clip from a location recording has Field Recording in its right-click menu: **Expand to Location Channels** (`clip.expand_channels`) finds the recordings of the clip's moment, in the session or in its file's folder (or a folder chosen), whose timecode overlaps the clip and whose scene and take match its file's, and puts each of their channels (and the other channels of a polyphonic clip's own file) on a new mono track below it, named by the recorder's channel names and cut to the clip, in one undo step; **Alternate Takes** lists the scene's other takes (`clip.alternate_takes`) and the clip plays the one picked from the same place in its file (`clip.use_alternate_take`; on recorders writing a file per channel, the take's file with the same channel names). Split into Mono names the tracks by the channel names. Not done: matching on timecode alone when the scene or take differ, a stretched clip (refused), and lining a take up by a sync point rather than its place in the file |
| Clip effects: EQ and dynamics per clip, live | **Works** | Per-clip EQ, compressor and gain, with copy, paste and render |
| Beat Detective across many tracks, phase kept | **Partial** | The Beat Detective window finds hits across the selected tracks and cuts all at the same points (phase kept), then Conform moves each piece to the grid or a groove with a strength, and Smooth runs each piece on to the next with a short crossfade at every join, one undo step (`d8e4286`, [section 4](04-editing-audio.md), I). Left: trigger editing (the hits are not drawn, so one cannot be added, moved or removed by hand before Separate); pieces are moved whole, not stretched |
| Sketch: a clip-launcher window, also on a tablet | **Not planned here** | The clip launcher is skipped by the owner. DAWproject import drops launcher clips and says so |
| Dolby Atmos renderer built in: beds, objects, re-renders, ADM; MPEG-H | **Partial** | Channel surround to 9.1.6 with a 3-D panner works. Object/Bed is a mark that says it is one; no renderer, no ADM. Dolby's renderer needs a licence (not planned); a renderer of FreeDAWm's own is [section 3](03-mixing.md)'s work |
| Speech-to-text on dialogue with text search | **Partial** | Lyric transcription with word times, searchable (AI › Find in Transcript…); no dialogue-specific lane |
| Video: several tracks, editing, satellite link, pull up and down | **Partial** | One video track with H.264, ProRes and Motion JPEG playback, timecode and feet+frames; export to a ProRes movie with the mix; no editing |
| Destructive record, QuickPunch, TrackPunch | **Works** | QuickPunch (punch in and out on the fly, several punches in one pass) and TrackPunch (Transport › TrackPunch: disarming a track while recording punches it out, arming it punches it back in, each track kept to its own punches with handles and crossfades): [section 2](02-recording-audio.md), C. Destructive Record and DestructivePunch are not planned here: FreeDAWm never overwrites recorded audio in place (section 2's row) |
| EUCON control surfaces | **Not planned here** | Needs Avid's licensed SDK. Mackie Control and HUI surfaces and MIDI learn work instead ([section 6](06-midi-recording.md), K) |
| Scripting SDK | **Works** | Every command, from a script, a command line or an agent (section F) |
| Cloud collaboration on one project | **Not planned here** | Needs a hosted service and a second machine to develop at all |
| AudioSuite with handles; commit, freeze, bounce | **Works** | Process keeps up to 1 s of handles each side for processes that keep the length ([section 4](04-editing-audio.md)); freeze (also up to a chosen insert), commit and bounce work |
| Track Pin, batch rename and other large-session aids | **Works** (2026-11-18, 3c53abf) | Track › Pin (`track.pin`, saved on the track) keeps a track at the top of the Edit window's track area, in session order, while the others scroll under it (a line marks the edge; pinned tracks take at most half the area, the rest of them scroll); unpinned, it goes back to its place. Track › Rename Selected Tracks… (`track.rename_selected`) names the selected tracks in track order by one pattern, `*` the track's name and `#` a running number (`##` pads to two digits; `start` sets the first), in one undo step, and renames nothing when a new name would clash with another track's. In the Mix window too (round five): pinned strips stay at the left in session order while the others scroll beside them, a line marking the edge; pinned strips take at most half the width (the rest of them scroll) (`mix_window::pinned_split`, test `pinned_strips_stay_at_the_left_while_the_others_scroll`). Batch rename's other modes (round five): the Rename Selected Tracks dialog has Pattern, Find and Replace (every occurrence in each selected name, `find`/`replace`) and Trim (characters off each name's start and end, `trim_start`/`trim_end`), the same one undo step and clash check (tests `replace_and_trim_names`, `the_selected_tracks_are_renamed_by_find_and_replace_or_trimmed`) |

## B. Studio Pro (Studio One)

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Project page: master an album in the same app, songs update from their mixes, DDP and digital release | **Not planned here** | The mastering page is tabled by the owner (design below). Bounce to a loudness target with a true-peak ceiling |
| Show page: set lists for playing live | **Partial** (step 1 built, `640fac9`) | Not among the pages the owner skipped (the mastering page is tabled, not this one). Window › Transport & Session › Set List: the songs of the show, each a memory location that keeps a selection, in order (`Session::set_list`, saved with the session; `setlist.add`, `_remove`, `_move`, `_go_on`, one undo step each). Play Set or a song's ▶ plays it from its start to its end (`setlist.play`); then the transport waits on the next song's start, or with Go On plays it at once (the app starts it on the next frame: a short gap, not a seamless join). Stage View covers the whole window with the song, the next one and the counter, big, with Play/Stop (Esc leaves). Left: step 2, songs in other sessions, switching while playing with the next session loaded in the background (two to three days); a seamless Go On (the audio thread jumping); each song's own tempo is the session's tempo map, as now |
| Scratch pads: try another arrangement beside the song | **Works** (`b3776ae`) | Alternative arrangements (Edit › Arrangements): several versions of the song, each naming the playlist every track plays and keeping its own markers, switched as one undo step ([section 4](04-editing-audio.md)'s row) |
| Arranger track: drag sections to reorder the song | **Works** (`c8cfbb1`) | A block on the Sections ruler copies, moves, repeats (`b616514`) or removes its section with every track's clips and automation and the Band's chords (`arrange.copy`, `arrange.move`, `arrange.remove`, one undo step each). Left: a section that changes tempo or meter inside is refused ([section 7](07-midi-writing.md)) |
| Chord track that re-pitches audio as well as MIDI | **Partial** | Chords drive the Band and re-pitch single-line vocals (Fit to Lead, Make Harmony); MIDI clips follow the chord lane, per clip, opt in ([section 7](07-midi-writing.md)). Chordal audio following it is not planned here: it needs polyphonic note editing of audio (section 7's chord track row) |
| Mix Engine FX: one console model across every channel of a bus | **Works** (2026-11-18) | A Summing menu on the strip of an aux or routing folder (`mix.summing`, `Mixer::summing`, saved only when on): Off (clean, the default, so no session changes), Console or Tape, and a Drive. Every source going into that bus, by its output or a send, passes through the model as it is added (`soundcraft_mix::summing`): a static waveshaper per channel (odd harmonics that grow with the level), a tolerance per channel seeded from the track id and channel so channels differ a little and a bounce repeats, and crosstalk from neighbouring channels (about -46 dB at full drive). As on a console, the stage sits after each source's send level (or fader) and pan, on the bus's channels, so a hotter send drives it harder (round five: test `a_hotter_send_drives_the_summing_model_harder`); each bus channel also gets the design's gentle filter of a summing amp (a one-pole roll-off at 16 kHz blended in with the drive: about -1 dB at 18 kHz at full drive), its memory kept per strip, per way into a bus (each send slot and the output) and per bus channel, cleared on a locate so a bounce repeats (`the_filter_rolls_off_the_top_gently_and_leaves_the_low_end`). The filter is part of the model only: with Summing off, or at drive 0, the bus adds every source exactly as before (no code of the model runs; tested). The sum into the bus still happens one source at a time in the old order after the worker pool renders the strips, so it is the same on every run; nothing allocates while rendering. Bounces and freezes hear it |
| Splitter and extended chains: parallel and band-split routing inside the inserts, macro knobs | **Not planned here** | Skipped by the owner (rack and macros). Inserts are a straight chain; linked plug-ins share values, which is not a macro |
| Sound variations (articulations for orchestral libraries) | **Works** (`e16557a`) | A track's articulations, each with its keyswitch (the input menu › Articulations, `track.articulations`), chosen in clips by a controller lane (CC 3 by default) whose value picks one: playback and chase send the keyswitch in its place. Per-note articulations, other switch kinds and map import are left (design below) |
| Launcher: clips and scenes tied to the arrangement | **Not planned here** | The clip launcher is skipped by the owner |
| Lyrics track, lyrics in the score and on a display | **Works** (`2ac9936`) | Lyrics lane and sheet, a teleprompter for the singer, rhymes and word tools; lyrics sung under their notes in the printed score and the MusicXML export |
| Drag and drop for everything | **Partial** | Plug-in to a track or to empty space, audio to the sampler, clips and sources from the Clip List onto a track (`178984e`), files from the system. Left: dragging out of the app (a clip or a bounce to the desktop or another program); the window toolkit cannot start a drag to the system |
| Global transpose; audio follows tempo by default | **Partial** | A global transpose track for MIDI and instrument tracks, set on the Key ruler (`c8e1963`). Left: audio clips following it (the owner's lean: later) and audio following tempo by default (a per-clip choice; new sessions only, if ever: the owner's lean is no for old ones) |
| Listen bus with room correction | **Partial** | An AFL/PFL listen bus in the Control Room, and the monitor stage's ear correction from the Hearing Test (on this computer, never in a bounce); no room measurement or correction |
| Amp and pedal models from a guitar-amp maker | **Works** (unbranded) | Seven amps solved from schematics, cabinets, pedal circuits |
| Tablet remote app | **Partial** | A phone web page on the local network: transport, record, arm, markers; no mixing |
| Built-in assistant, stem separation and vocal tuning (8.1), MIDI 2.0 (8.2) | **Partial** | Stem separation and vocal tuning work; assistant: see Scout (section F); no MIDI 2.0 |

## C. Logic Pro

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Session Players: drummer, bass and keyboard players that follow chords | **Works** | The Band: six players chosen from 152 styles of player, following sections and chords, printable to MIDI. Sounds come from the track's instrument, and FreeDAWm ships few |
| Live Loops grid | **Not planned here** | The clip launcher is skipped by the owner |
| Smart Tempo: record without a click, then fit project or audio | **Works** | Detect Tempo (constant, follow, smooth) writes the tempo map; Straighten conforms audio; no free-tempo record mode as such |
| Flex Time and Flex Pitch on the track | **Partial** | Pitch in a separate vocal editor; time by clip stretch and by warp markers placed by hand or by Straighten, heard at once (`warp.*`); no pitch editing on the track itself |
| Stem Splitter, now down to single drums | **Works** | Four or six stems, lead and backing vocals, and drums into kick, snare, toms, hat, ride, crash. Models are downloaded |
| Mastering Assistant | **Partial** | Scout measures the mix and returns advice, and applies it a step at a time when asked; no single-press mastering chain |
| Drum Machine Designer, Alchemy, Sculpture and the rest of the instrument set | **Partial** | A drum machine (Juke: 16 pads with layers and their own outputs) and a drum synth; the Subtractive Synth (filter types, an LFO, sub and noise, unison, hard sync, factory presets; `a168d13`, `f270dea`); the Sampler; Pantry, the multisampler, made by capturing any instrument or loaded from SFZ and Decent Sampler libraries (`771ead0`, `d2def49`); one track layers and splits up to four instruments by key and velocity (`ec688c3`); the Wavetable Synth (thirteen tables made in code, band-limited) and the FM Synth (four operators, eight algorithms). Left: other kinds of synthesis (granular, physical modelling: Alchemy's and Sculpture's ground; [section 7](07-midi-writing.md#l-instruments-in-the-box)) and sampled acoustic instruments (content the owner is sourcing) |
| Smart Controls; tablet remote with touch instruments | **Partial** | A few key knobs per insert on the mixer; transport-only phone remote |
| Track Stacks and patches | **Works** | Routing folders sum their tracks; track and chain presets carry their samples |
| Scripter and the Environment | **Works** (Scripter); the Environment **Not planned here** | A **Script** MIDI effect (`r5script`, see the design below): input menu › MIDI Effects › Add › Script, Edit Script… opens the Script Editor (the text, Compile, the error with its line, the script's printed lines, its parameters as sliders, and four examples: Transpose, Velocity Curve, Arpeggio, Note Echo). A Rhai script sees notes, controllers, pitch bend, pressure and program changes with their song position, passes, changes, leaves out or adds events, now or later on the tempo (`send`, `send_after`), reads the tempo and whether the transport plays, keeps its own state, and declares up to eight named parameters. Commands `track.midi_fx_add` (`kind: script`, `example`) and `track.midi_script` (text, example, values; one undo step, a slider drag one step); the text and values are saved with the track and compiled again on opening. **Rhai cannot evaluate without allocating**, so a script never runs on the audio thread: it is compiled and run on one worker thread, fed and read through fixed lock-free rings, and is heard **one buffer late** live (exactly the largest block, timing kept); bounces and Record Effects' Output wait for the worker and are on time. Limits: 100,000 operations a call and 400,000 or 50 ms a block, calls 32 deep, strings 4 KB, arrays 1,024, maps 256; a script that errors or passes a limit is bypassed (events pass unchanged, the notes it left on are let go) and the error stays in the editor until it is compiled again. No file, network or process access: no such function exists, `eval` is off and modules are compiled out. Left: parameters are sliders, not automation lanes; a clip's own effect list takes no script. **The Environment** (Logic's patchable MIDI objects): not planned here; a graph editor of MIDI objects is a large window of its own, and the MIDI effects list, MIDI to Track, the input transformer and now scripts cover what it is mostly used for |
| Spatial audio with binaural monitoring and head tracking | **Partial** | Surround panning; no binaural path |
| A very large sound and loop library; loops follow tempo and key; one browser for everything | **Not planned here** | Content and licences, the owner's call. The browser lists plug-ins and presets; shipped content is about 20 MB |
| Round trip with the tablet edition and with MainStage | **Not planned here** | Apple's own apps and formats |
| Chord ID from audio | **Works** | Detect Chords writes the chord lane |
| Flashback capture | **Works** | MIDI: Capture MIDI makes a take of what was just played ([section 6](06-midi-recording.md#g-punch-count-in-and-capture)). Audio: with Options › Recording › Pre-Record on, the last 10 seconds of input before Record are kept at the front of the take as a handle to trim open (`40601d7`, [section 2](02-recording-audio.md), D); there is no separate capture command for audio, so Record is pressed (and can be stopped at once) to keep them |
| Step Sequencer with note and automation rows; articulation sets | **Works** | Drum step sequencer, and step sequences with a pitch per step for any instrument ([section 7](07-midi-writing.md)). Value rows under the note rows (+ Row in the step grid; `pattern.row_add`, `pattern.row`, `pattern.row_step`, `pattern.row_remove`, one undo step each, a drag one step): each sets a value per step for a CC, pitch bend or pressure (controller events in the clip, on the pattern's channel) or for a parameter of the same track's instrument or inserts, or its volume, pan or a send (the clip's own automation lane, kept in step with the row); its own length, rate, direction and mute like a note row, an empty step holding the value before it. Playback, bounces and freezes read the same events and lanes; saved with the clip; patterns saved before have no rows and play and save as they did (tests `value_rows_write_controllers_and_the_tracks_parameters_heard_as_bounced`, `a_parameter_row_is_heard_in_the_render`, `value_rows_are_added_painted_and_muted_from_the_grid`). Values step, no glide between steps. Articulation sets are a track's articulations now (Studio Pro's row) |
| Assistant that operates the app from a prompt (13, by subscription) | **Partial** | No assistant inside the app; an outside agent can operate every command (section F) |

## D. Cubase

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Expression Maps and Note Expression | **Partial** | Expression maps as articulations switched by keyswitch, chosen by a controller lane (`e16557a`, Studio Pro's sound variations row; Cubase's direction type). Note Expression: per-note tuning, pressure and brightness on a note's own channel, edited in lanes that follow the selected note (`d0d84ca`), and sent to CLAP plug-ins, and to VST3 ones that declare them, by note id (`27b58af`; [section 7](07-midi-writing.md)). Left: the attribute type (an articulation per note rather than per stretch of time), map import |
| VariAudio: pitch and formant editing, harmonies from the chord track | **Works** | Note-based tuning with formant-preserving resynthesis; Make Harmony follows the chords; a view of every vocal track with clashes marked |
| Chord Pads and Chord Assistant | **Partial** | Window › Editors › Chord Pads… (`7e0e717`): eight pads, the seven triads of the key on the Key ruler and V7, played and recorded like Musical Typing; a chord picker and a chord stamp for the Pencil ([section 7](07-midi-writing.md)). Left: choosing each pad's chord, voicing and inversion; playing the pads from a MIDI keyboard; suggestions of the chord to play next (the Chord Assistant) |
| Control Room: cue mixes, talkback, monitor sets | **Works** | Window › Transport & Session › Control Room: monitor sets with level, dim and mono, AFL/PFL listen, cue mixes from pre-fader sends, talkback ([section 3](03-mixing.md), ec5efc3) |
| Mixer undo history and mixer snapshots | **Works** | Mixer moves are ordinary undo steps; Window › Transport & Session › Mix Snapshots stores and recalls the whole mix or parts of it ([section 3](03-mixing.md)) |
| Logical Editor, Project Logical Editor, macros | **Partial** | MIDI Operations › Select Notes picks notes by pitch, velocity, length, position, beats of the bar and channel ranges, or controller events (control changes, pitch bend, pressure, program changes) by kind, number, value, position and channel, and selects, deletes or changes them in one step (notes also moved to a new track, muted, transposed or resized; events' values scaled and offset, CC number and channel set) ([section 7](07-midi-writing.md), D). Left: "not" and either/or conditions, rules saved by name, a Project Logical Editor (tracks and plug-ins) and macros |
| MIDI Remote: scripted controller support with a mapping assistant | **Partial** | MIDI learn for faders, pan, sends and plug-in knobs, Mackie Control and HUI ([section 6](06-midi-recording.md), K); no scripts for other controllers |
| Sampler Track with slicing | **Works** | Drop audio on an instrument track or send a selection to the sampler; slices by hits, beats or equal parts |
| Audio Alignment of stacked takes | **Works** | Vocal Studio's Align timing (Fit to Lead's timing half): each note of a double moves its start and end onto the lead's, with a strength and a largest shift; pitch kept (aee379b) |
| Hitpoints, AudioWarp, Time Warp | **Works** | Detection, Straighten and Conform, and warp markers placed by hand; Time Warp: drag a bar line on the Tempo ruler to where that bar should be, and the tempo before it changes so it lands there while later bars keep their tempo (`event.time_warp`, one undo step; refused inside a tempo ramp) |
| Direct Offline Processing: an editable offline chain per clip | **Partial** | Every Process is kept in the clip's process history: Process › Process History… bypasses, reorders or removes a step and rebuilds the clip from there (`5d4ebe1`, [section 4](04-editing-audio.md), G); Clean Vocal keeps the original with an amount to change later. Left: opening a step again to change its settings, and changing a step that a later length-changing step follows (refused with a message) |
| Modulators for any parameter | **Works** | Built in [section 5](05-built-in-effects.md): LFOs and envelope followers on a track, each moving up to 8 plug-in parameters |
| Track Versions, arranger chains, Render in Place | **Partial** | Track Versions: playlists per track, and Track › Arrangements keeps whole alternative arrangements of the song, switched in one step (`b3776ae`); Render in Place: commit and bounce. Left: arranger chains |
| Remote recording over the internet (VST Connect) | **Partial** | Works, by a different design (section F), with talkback (4d3e275); no video |
| ARA 2 and spectral editing | **Partial** | ARA 2 hosting is built ([section 4](04-editing-audio.md), J; `docs/ara.md`): ARA VST3 plug-ins read clips given to them, the clips play through them, offline renders hear them, their documents are saved with the session and their editors open told the selection; untried with commercial plug-ins, no editor windows on Linux. Spectral editing is [section 4](04-editing-audio.md)'s. The ARA note overlay view flag still exists and is unread |
| Score editor with engraving; drum and pattern editors, now melodic | **Partial** | Notes entered and edited in the score (`d1610d6`); rests, ties and beams, pages, parts and lyrics (`e7768d1`, `b17e0fd`, `07a98b3`); print, MusicXML export and import (`0d5f801`); drum maps (`5817ba3`), the drum step sequencer, and step sequences with a pitch per step for any instrument (`0dda09d`). Left: engraving controls (dynamics, articulations and text placed in the score, symbols moved by hand), a key signature drawn in the score view, step entry from a MIDI keyboard ([section 7](07-midi-writing.md), G) |
| Video export; loudness track and normalising on export; metering suite | **Partial** | Bounce to a loudness target with a true-peak ceiling; export to a ProRes movie; no loudness track; meters are strong (BS.1770 with true peak, LevelUp, Scout) |
| Vocal synthesis (Omnivocal, beta) | **Not planned here** | Needs a singing-voice model, and no third-party models are added here. Song generation produces finished audio, not an editable sung part |

## E. Where three or four now agree

These started as one product's signature and have spread. They are the gaps most likely to be
noticed, and the ones sections 1–7 did not already list:

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip launcher | ● | ● | ● | ○ | **Not planned here** (skipped by the owner) |
| Stem separation | ● | ● | ● | ● | **Works** |
| Sound and loop service or library in the browser (Splice or the maker's own) | ● | ● | ● | ● | **Not planned here** (content, the owner's call) |
| Dolby Atmos mixing with a renderer | ● | ● | ● | ● | **Partial** (see A) |
| Arranger sections that move the song | ○ | ● | ● | ● | **Works** (`c8cfbb1`) |
| Chord track that existing clips follow | ◐ | ● | ● | ● | **Partial** (MIDI clips, opt in; not audio) |
| ARA 2 | ● | ● | ● | ● | **Partial** ([section 4](04-editing-audio.md), J): discovery, the session as an ARA document, playback through the plug-in, offline renders, saving its document, its editor view; left: trying it with commercial plug-ins, Linux editor windows, CLAP and AU |
| An assistant inside the app | ○ | ● | ● | ○ | **Partial** (Scout advises on the mix only) |
| Tablet or phone remote | ● | ● | ● | ● | **Partial** (a phone page on the local network: transport, record, arm, markers; left: mixing from it, see B) |

## F. What FreeDAWm has that none of the four does

Each was checked in the code. "Works" means wired end to end; none was listened to or timed.

| Feature | Status | What it is, and its limits |
|---|---|---|
| Every command open to scripts and AI agents | **Works** | About 330 commands with documented parameters. A command-line tool runs them on a session without the window; a running app accepts them on a local port; an MCP server lets an AI agent list commands, run them, look at the session, take screenshots and click. Any menu item can be invoked by its path |
| Remote recording with the take landing in place | **Works** | A second computer follows the transport, records locally and sends the take (lossless audio, or MIDI). The performer hears the live mix; clocks are compared during the take so the audio lands where it was played. Works on a local network or across the internet (encrypted, by one-time invite, no account). Takes survive a dropped connection or a crash. On a local network PCs pair and connect over an encrypted link bound to their keys (4d3e275); the first pairing's code proves both keys and is never sent, so a computer posing as the booth cannot sit in between (SPAKE2, 30e56ba); a PC or booth with an older FreeDAWm is told to update. The phone's page stays on the plain link. Talkback: the control room's talk key mixes its input into the stream the performer hears, the music dimmed, while stopped too; never recorded (4d3e275). No video. A paired computer can run any command; the Network window says so and which link each uses |
| Scout: measured mix advice from an AI model | **Works** | Captures loudness, true peak, spectrum, stereo and transient measures of a mix or a channel, sends them with the channel's settings to Claude using the user's own key, and draws the returned EQ suggestions on the EQ as guides. It changes nothing by itself; each suggestion can be applied on request, one undo step each. Reference profiles can be built from other recordings, per stem |
| Local models beyond stems | **Works** | Lead from backing vocals; drums into six parts; audio to MIDI by four models (general, piano with pedal, drums, voice) with a live preview; MIDI from a text description; a whole song from a prompt. All run on the user's machine after a download; each model's licence is shown, and non-commercial ones ask for confirmation |
| Mend: vocal de-noise and de-reverb on a live insert | **Works** | The same model as Clean Vocal, running in the chain with about 2.9 s latency or a lighter live mode. Desktop only |
| MIDI Cleanup against key and chords | **Works** | Corrects a transcribed part without changing starts, velocities or pitches of kept notes; switchable until committed |
| Vocal Studio: every vocal on one pitch view | **Works** | Marks clashes and unisons across tracks, moves clashing notes to fit the lead, lines up a double's note starts and ends with the lead's, writes a harmony |
| ChamberMade; Imprint and Vault | **Works** | A reverb from a room you draw (size, materials, objects, speaker and microphone); capture of an impulse response from any plug-in, outboard unit or room, then convolution with it |
| Rez, Manners, Lull's guard, Shale | **Works** | A resonance suppressor; an EQ that reduces masking between bands by itself; a gate that listens to the source it should ignore; a drum gate that learns bleed, links across the kit's microphones and can send its hits as MIDI |
| LevelUp | **Works** | Loudness-matched before-and-after listening by a linked pair of inserts, with reference tracks and a preview of what streaming services do to the level |
| Lyric tools | **Works** | Rhymes, synonyms and stress checks offline; alternative lines; a teleprompter; timed lyrics from a recording |
| Browser build | **Not planned here** (removed by the owner, 2026-11-18) | There was a web-page version (playback, editing, built-in effects, save and export as downloads, the session kept between visits). The owner removed it: nobody would use it in a browser. FreeDAWm is a desktop app |
| Its own video decoders | **Works** | H.264, ProRes and Motion JPEG without system codecs; its own ProRes encoder writes the movie export |
| Open source, four platforms | **Works** | MIT or Apache-2.0 (the Windows build with ASIO is GPLv3). Linux, macOS, Windows and FreeBSD (the browser build was removed, 2026-11-18). English only; little work on screen readers |
| Take recovery and safety by tooling | **Works** | A running take is recovered after a crash; ten backups per session; the build refuses code that can panic outside tests. A hosted plug-in runs inside the app and can bring it down, unless it is a CLAP or VST3 plug-in and Preferences › Mixing › Run CLAP and VST3 plug-ins in their own process is on (off by default; see the design notes) |

## Things that promise more than they do

| # | What | Where | Status |
|---|---|---|---|
| 1 | Object/Bed switch on the mixer strip and the Renderer window: the switch is stored and drawn; no audio path reads it, and the Renderer only draws speakers and dots | `engine/src/cmd/track_more.rs:21-40`; `ui-egui/src/mix_window.rs:408-414`; `ui-egui/src/extra_windows.rs:86-145` | **Labelled** (943143a): the switch's hover, its command's description and the Renderer window say that objects are not rendered yet and play through their panners into the bed. Rendering objects is the mixing section's work ([section 3](03-mixing.md)) |
| 2 | Track widths of 2nd- to 7th-order Ambisonics can be chosen; the panner fills the first four channels and leaves the rest silent | `dsp/src/pan.rs:268-279` | **Done** (a2e9845): the panner encodes every order the track carries (ACN, SN3D, to 7th order in the DSP), so 2nd and 3rd order are filled; the mixer carries at most 16 channels, so 4th to 7th order are no longer offered and are refused by the commands that make tracks, busses and outputs. The decoder to speakers is still first order |
| 3 | QuickPunch, TrackPunch, Destructive Record and Destructive Punch have keys or commands and store a mode nothing reads; Prepare DPE Tracks consolidates for a punch that never happens | `engine/src/cmd/options.rs:22, 58-59, 125-130`; `engine/src/cmd/setup_more.rs:22, 130` | **Done for QuickPunch and TrackPunch** (4b98b7f; TrackPunch 2026-11-18, on the Transport menu, read while recording). Destructive Record, DestructivePunch and Prepare DPE Tracks stay off the menus (agents only, described as stored only): not planned here, FreeDAWm never records over a file in place |
| 4 | Stored and unread: transcription lane and settings, scene and take in clip names, ARA note overlay, peripherals, video sync offset, Ableton Link in the transport, calibration mode, auto-spot | `engine/src/cmd/view_more.rs:145, 154`; `engine/src/cmd/setup_more.rs:31-53` | **Done; claim partly wrong** (8fac425): these were already off the menu bar (the 2026-10 menus clean-up); agents and scripts still reach them, and their descriptions now end "stored only: nothing in FreeDAWm reads this setting yet". The video sync offset is not one of them: the Video track and window read it (it has no window of its own) |
| 5 | The web build has no way to record and no place that says so; MIDI windows are shown there with nothing behind them | `apps/soundcraft-web`; `ui-egui/src/lib.rs:996-999` | **Done for recording** (b5ef0ca): in the browser build Record says it cannot record there, on the button and in the status line. The MIDI windows are the MIDI recording section's ([section 6](06-midi-recording.md), row 23), which left them out of the web build's menus (352090a). The browser build itself was removed (2026-11-18) |
| 6 | On a local network, a paired computer can run any command over an unencrypted link. The pairing code is four digits with attempt limits. Fine for a studio network; worth saying in the window, and worth offering the encrypted path locally too | `net/src/hub.rs:633, 794, 1026`; `net/src/pairing.rs:1-30` | **Done** (7ed1bfd): the Network window and the pairing prompt say that a paired device can do everything the user can. The encrypted local link is **done** too (4d3e275, below), and a PC's first pairing proves both keys with the code, never sending it (30e56ba, below); the window and the prompt say so |
| 7 | Video muxer and ProRes encoder exist in the video crate and nothing calls them | `video/src` | **Done** (3b781d6): Bounce to Movie uses both |

## Suggested order of work

Estimates are agent-hours for one implementer with tests. These are larger and less certain than
in sections 1–7, because most are new features, not repairs.

**1. Make the controls honest (≈ 6–10 h).** Rows 1–5 above: remove or grey out what does nothing
(object switch, Renderer, higher Ambisonics orders, the punch modes not being built in section 2,
the web build's MIDI and record controls), or build it where a later row here does. Say what the
local link allows in the Network window (row 6).

**2. Signature features worth taking, best value first (≈ 190–300 h).**

| Work | Est. (h) | Why | Status |
|---|---:|---|---|
| Arranger sections that move everything, and clips that follow the chord track | 14–22 | Sections and chords already exist for the Band; this makes them act on the user's own tracks. Also in [section 7](07-midi-writing.md) | **Section 7's**: MIDI clips follow the chord lane there (per clip, opt in); arranger sections now copy, move, repeat and remove the song's clips and automation (`c8cfbb1`, `b616514`) |
| Bounce to a loudness target; video export | 10–16 | Both have their parts already (loudness meter; muxer and encoder) | **Loudness target done** (ca44ec5): Bounce Mix takes a target in LUFS (integrated, BS.1770) with a true-peak ceiling (default -1 dBTP), measured in a first pass; gain only, so a mix that would pass the ceiling stops short and says by how much. Video export **done** (3b781d6): File › Export › Movie (ProRes)… writes the Video tracks' picture and the main mix as a QuickTime movie, ProRes 422 (proxy, LT, standard or HQ) at the first movie's size and frame rate, with 24-bit PCM stereo |
| Scout builds a starting chain from its advice, with the user's consent per step | 8–12 | Turns advice into Logic's Mastering Assistant, on any channel | **Done** (06244b1): each suggestion of an answer has an Apply button; `scout.plan` says what each would do, `scout.apply` does one, as one undo step. An EQ guide becomes a band on the track's Clean Q (one is added if missing); a module suggestion sets its parameter to the middle of the suggested range when that range is in the parameter's own units, and is refused otherwise. Uses the answer already received: nothing more is sent |
| Timing alignment of stacked vocals | 10–16 | The pitch half is done; the command already has the option | **Done** (aee379b): `vocal.fit_to_lead {align_timing: true}` (the studio bar's Align timing, which leaves pitch alone with `fit_pitch: false`). Each backing note is paired with the lead note it overlaps most; its start and end move `strength` (default 100 %) of the way to the lead's, at most `max_shift_ms` (default 100). Where a note starts out of silence or stops into it the edge is read from the audio (within 25 ms of the tracker's), so edges land closer than the tracker's 5 ms hop. The clip is rendered again with Straighten's pitch-keeping stretch between those points (its edges stay; Remove Warp restores the original), its tuning carried through the warp and re-matched to an analysis run before the command returns: one undo step. Test: a double sung 17 to 41 ms off lands every start and end within 3 ms of the lead's, pitch within 8 cents, a fitted note keeps its fit. Limits: Remove Warp goes back to the original audio without the tuning; Clean Vocal's amount at the time is baked into the render; timed lyrics on the clip are not moved |
| Search the transcript; transcript lane | 6–10 | Whisper output is there; Pro Tools' dialogue workflow | **Search done** (ade867d): `lyrics.search` lists every place a phrase is sung or spoken across the tracks' timed lyrics (case, punctuation and line breaks ignored); AI › Find in Transcript… selects the next one and moves the playhead there. The lane is the lyric strip each track already draws; a separate taller dialogue lane is not built |
| Macro knobs and a splitter in the insert chain | 14–22 | Also in [section 5](05-built-in-effects.md)'s list; count once | **Not planned here**: rack and macros skipped by the owner |
| Modulators for any parameter | 10–16 | Also in section 5's list | **Done** in [section 5](05-built-in-effects.md) (eee0a00) |
| Control room: cue mixes from sends, talkback, monitor sets, listen bus | 14–22 | Pairs with remote recording, which has no talkback | **Done** in [section 3](03-mixing.md) (ec5efc3) |
| Mixer snapshots | 6–10 | | **Done** in [section 3](03-mixing.md) (Window › Transport & Session › Mix Snapshots) |
| Clip launcher: cells, scenes, record the performance to the timeline | 40–60 | Three of the four have one. The largest item; needs a design first | **Deferred by the owner** (2026-11-16): not to be built for now |
| A mastering page: several songs in order, loudness per song, export a release | 24–36 | Only Studio Pro has it; a strong reason people choose it | **Not planned here**: tabled by the owner (design below, kept) |
| Atmos: objects and beds routed for real, ADM export, a binaural monitor | 30–50 | A renderer of FreeDAWm's own; Dolby's needs a licence. Also in [section 3](03-mixing.md) | **Another section's** ([section 3](03-mixing.md)) |
| ARA 2 hosting | 30–45 | Also in [section 4](04-editing-audio.md) | **Another section's** ([section 4](04-editing-audio.md)) |

**3. Not worth chasing, or not possible.** Hardware DSP mixing and EUCON (tied to one maker's
hardware); cloud collaboration and tablet companions (a service and second app to run); a large
sound library (content and licences, the owner's call); AAF and OMF (large formats with little
public documentation; DAWproject covers the modern case); a live-show page and field-recorder
workflow (specialist; revisit if users ask; both since built in part: the field-recorder workflow, and a set list inside one session); vocal synthesis.

**4. Build on what only FreeDAWm has (≈ 40–65 h).** Recording in the browser build, with
sessions kept between visits (12–18): sessions kept **done** (270aee2), recording **dropped by the owner**
(the browser build will not be used; then the whole browser build was removed, 2026-11-18); talkback and encrypted local links for remote recording
(10–16): **done** (4d3e275, see the design notes below for what was built and what remains); isolate hosted plug-ins in their own process so a crash cannot take the session (16–28,
and the scan helper is a start; **first steps built** (3b77990, and round three): CLAP, a
Preferences option off by default, the helper shipped in every package; see below); a first-run tour (4–6): **done** (a689967), seven cards opened once on the first start (no settings saved yet) and again from FreeDAWm › Tour of FreeDAWm.

**Designs for the larger items** (some since built, as each says). Each was judged too big to finish well in the
same pass as the items above; these are the starting points.

- *Mastering page (24–36 h).* A release file of its own (`.fdrel`, JSON) beside sessions: an
  ordered list of songs, each a session (bounced on demand, so a song follows its mix) or an audio
  file; per song a gain or loudness target (the bounce's two-pass measurement, already there), a
  gap and fades; one shared master chain plus a chain per song, run offline through
  `RangeRender`. A `release.*` command family (add, move, remove, set, render) and a page that
  shows the songs in a row with their integrated loudness, true peak and the album's. Export:
  one file per song plus one continuous file with a cue sheet; DDP later.
- *The browser build (removed by the owner, 2026-11-18).* It kept the session between visits in
  the browser's private file system (270aee2); recording in it was designed and then dropped
  (2026-11-17). Both designs are in this file's history before the removal.
- *Encrypted local link (built, 4d3e275; owner-approved UDP port).* The internet path's QUIC
  (TLS 1.3, iroh) on the local network: a UDP socket on the network port's number beside the TCP
  listener, with no relays, no address lookup and no router port mapping, so nothing leaves the
  local network. Each PC's key is the one it already keeps for the internet. The booth adds its
  UDP port and key to the mDNS record it already sends; a remote PC dials that key (only its
  holder can answer) and pairs with the code on the booth's screen, attempt limits as before.
  The booth binds the paired PC to its key (its token is refused on the plain link and from any
  other key); the remote PC keeps the booth's key and later dials only it. Setup › Network:
  Offer the encrypted local link (on by default), Take PCs over the encrypted link only (the
  plain link then serves the phone's page alone; on by default since the owner's decision of
  2026-11-16, a value already saved is kept), and which link each PC is on. The first pairing
  is a password-authenticated exchange over the code (SPAKE2, built 30e56ba, see the section
  below): the code is never sent, and a machine that answers the very first contact with its own
  key (by forging the booth's mDNS record) fails the check on both sides unless it guessed the
  code, one guess per try; once paired the keys are pinned as before.
  PCs paired over the plain link keep working there only while Take PCs over the encrypted link
  only is off (settings saved before the option existed now have it on: such a PC pairs again,
  over the encrypted link). The phone's page stays on plain HTTP (a browser cannot speak this
  link) and is served with the option on.
- *Talkback (built, 4d3e275).* The control room's talkback (its input, level and talk key, F11 or
  the Network window's Talk to the performer, held) is mixed into the copy of the mix the stream
  sends, the music dimmed by the control room's dim (once: the monitor's own dim already counts
  when talkback dims), ramped over 10 ms. While the transport is stopped the stream carries the
  talkback alone, on its own clock, and the performer hears it stopped too; the next roll starts
  a new epoch. It is never in the booth's main mix or a bounce, and the performer's take records
  his own inputs, never the stream.
- *Plug-ins in their own process (16–28 h; first step built, 3b77990; design refined 2026-11-16).*
  *Built in the first step:* the crate `plugin-bridge` with `BridgedPlugin` behind the `Plugin`
  trait; the helper `freedawm-plugin-host` (a binary of that crate; found next to the app or by
  `FREEDAWM_PLUGIN_HOST`); one shared file per instance mapped with `memmap2` and read only as
  atomic words; a JSON-lines pipe for open, prepare, state save and load; a block not back in time
  plays its own input (an effect) or silence (an instrument) in its place, counted, so the latency
  (one block, reported) never moves; offline renders wait for each block (at most 5 s) with no
  added latency; a helper that exits, or falls 5 s behind, silences its plug-in (an effect passes
  its input through, still one block late) and is said once, by name, on the status line and in
  the log. CLAP only, opt-in by `FREEDAWM_PLUGIN_PROCESS=1` (off by default: nothing changes for
  existing sessions). Tests: the exchange with two threads standing in for the processes (in
  order, one block late; a late block passed through and its late output dropped; offline waits;
  hostile headers refused); the real helper serving a built-in plug-in and the CLAP test plug-in
  (output equal to the input times its gain, one block late; latency its own plus one block); the
  helper killed mid-render: every block still returns at once, the plug-in passes audio through,
  the crash is reported by name, later requests fail at once. *Second step (round three):*
  Setup › Preferences › Mixing › Run CLAP (and, since below, VST3) plug-ins in their own process (`ui.plugin_process`,
  this computer's, off by default; it applies to plug-ins loaded from then on and when a session
  opens; `FREEDAWM_PLUGIN_PROCESS=1` or `0` overrides it for a run, and the window says so, and
  says when the helper is missing); the helper is built with the workspace and every package
  build (Linux, Flatpak, macOS, FreeBSD, Windows MSI and zip) carries and signs it next to the
  app (a test reads the scripts). With the option on, a CLAP plug-in's description (name,
  parameters, group) is read in a helper of its own too, so one that crashes or hangs as it
  loads only costs that helper: it is not offered, the status line says so once, and it is never
  then loaded in the app's own process (nor is one the helper could not make for an insert). A crashed or hung plug-in's tile says "crashed" (red mark, as a failed module), the status line says Restart brings it back, and Restart Plug-in on its menu (in the chain, the Mix window's inserts and the instrument slot; `ui.plugin_restart`, `mix.module_restart`, not an undo step) makes it again in a new process from the state it last had (read from it whenever the app reads states, or loaded into it; a crashed one answers that) and the parameter values the session holds. A loaded state now also sets the host's copy of the parameters, and only values the host set go to a new exchange (before, a re-prepare sent every default over a loaded state). Not built: an automatic restart, and a state fetched after each edit in the plug-in's own editor (it has none through the bridge yet). The plug-in's own presets go through the bridge too: the browser lists them from a helper (the binary is not loaded in the app for it, so they no longer wait for the plug-in to be inserted once), a preset is loaded into the running instance in its process, and a module started from a preset gets its state from a helper. *Differs from the design:* the
  unsafe code is the crate's own private `shm` module (the mapping and its view as atomics); the
  helper waits by spinning, yielding, then 200 µs sleeps (no futex or named event). *Not built
  yet:* a per-insert choice,
  the editor window,
  latency, tail and parameter changes the plug-in makes itself, the transport for the
  plug-in (tempo, position), VST3 presets and programs (a .vstpreset still loads, as a state), and AU. The scan itself already ran in `freedawm-scan`. *VST3 (round three):* the helper makes `vst3:` plug-ins too (with `vst3-host`, its bundle's folder from the scan cache), so a VST3 plug-in is described, played, saved and restarted through its own process exactly as a CLAP one (the option is now "Run CLAP and VST3 plug-ins in their own process"); its own editor is not offered while it runs there (no editors through the bridge yet; turn the option off to use one).
  The design as written: Behind an
  option (Setup › Plug-ins › Run plug-ins in their own process, off by default; per insert too),
  CLAP first, VST3 and AU after on the same bridge.
  - *Where it sits.* A new crate `plugin-bridge` with `BridgedPlugin`, which implements
    `soundcraft_dsp::Plugin` (the trait `clap-host`'s `ClapPlugin` implements now), so the mixer,
    automation, presets and Freeze see no difference. The helper is a new binary,
    `freedawm-plugin-host`, next to `freedawm-scan`; it loads one plug-in with `clap-host` as
    the app does now and serves it. `plugin-scan`'s `helper_exe` finds it and its process
    handling (spawn, kill, a timeout) is reused. The unsafe code stays in the FFI crates.
  - *Audio.* One shared-memory file per instance (`memmap2`, already in the dependency tree;
    a file in the temp folder, removed when the slot goes): a header (block size, channels,
    sample rate, a sequence number each way, the overrun count) and two blocks of planar
    `f32` audio in and out, plus a fixed event area (notes, MIDI, parameter changes with their
    sample offsets, up to the existing `EVENT_CAP`). The audio thread writes block *n* and its
    events, bumps its sequence number, and reads block *n − 1*'s output, never waiting:
    `latency()` reports one block more than the plug-in's own, so delay compensation lines it up.
    The helper waits on the sequence number (a futex on Linux, a named event on Windows, a
    short spin then a sleep elsewhere), runs `process`, bumps its own. An output not back in time
    is silence, counted and shown on the insert (an overrun), never a stall. Offline renders
    (bounce, freeze) run the helper synchronously, block by block, with no added latency.
  - *Control.* A pipe (the child's stdin/stdout, JSON lines like the scan helper's) for
    everything off the audio thread: load, activate, parameter info and values, state save and
    restore, latency and tail changes, presets, and the editor window. The editor opens in the
    helper as its own top-level window (a CLAP floating window; embedding across processes is
    platform work for later), which the app raises and closes.
  - *A crash.* A helper that exits or stops answering the pipe for 5 s is dead: its slot passes
    the audio through (an effect) or is silent (an instrument), the insert says "crashed", and
    the session is untouched. Restart (on the insert, or automatically once per minute at most)
    starts a new helper from the state last fetched, which is fetched after every edit gesture
    and before every save, so at most the last gesture is lost.
  - *Tests.* A test plug-in that crashes on a parameter value and one that hangs: the session
    keeps playing, the slot reports it, Restart brings it back with its state; a bridged
    plug-in's output equals the in-process one's delayed by one block; an offline bounce through
    a bridge equals the in-process bounce sample for sample.
  - *First step when picked up.* The shared-memory block exchange and its test (two threads of
    one process standing in for the two processes), then the helper binary, then the option.
- *First pairing by a password-authenticated key exchange (built 30e56ba; the crate approved by
  the owner 2026-11-18).* The weak spot left by the encrypted local link was the very first
  contact: a machine that forges the booth's mDNS record answers with its own key and could sit
  between the two for that one pairing. Now the four-digit code proves the keys. *Crates:*
  `spake2` 0.4 (RustCrypto's PAKEs, MIT OR Apache-2.0, pure Rust, no unsafe; SPAKE2 over
  Ed25519), with `hkdf` 0.12 (and through it `hmac` 0.12, MIT OR Apache-2.0) and `sha2` 0.10 and
  `zeroize`, already in the tree; it brings `curve25519-dalek` 4.1 (BSD-3-Clause) and
  `fiat-crypto` 0.2 beside the 5.0 and 0.3 already there (through iroh), and `rand_core` 0.6. No
  C code, nothing that opens a port or sends anything new. No cryptography is hand-written: the
  code only calls the crates and orders the fields. *Exchange* (`net/src/pairing.rs`, module
  `exchange`; inside the QUIC connection the remote PC already opens to the key in the mDNS
  record, so nothing new listens): the booth is side B, the remote PC side A, identities
  `freedawm-booth` and `freedawm-pc`, password the code shown on the booth's screen. The PC
  sends its SPAKE2 message (33 bytes, `pair.exchange`); the booth answers with its own and its
  confirmation; the PC checks that before it sends its own (`pair.finish`, the next line on the
  same connection). Each confirmation is
  `HMAC-SHA256(HKDF-SHA256(k, info "freedawm pairing confirm"), role || booth key || PC key)`
  over the two QUIC public keys as each side sees them in the TLS session (the booth's bridge
  tells its hub both keys on the stream's first line); each field goes after a byte giving its
  length, so the fields cannot run into each other. Compared in constant time (`hmac`'s
  `verify_slice`). Each exchange spends one of the request's 3 tries when it starts (whoever runs
  it learns whether its code was right), a failed check answers "wrong code, or another computer
  is in between" and drops the connection, and a damaged message or any other line in place of
  the confirmation ends the exchange the same way. The code itself is never sent, logged or put
  in an error (a request printed for a log leaves it out); this side's copies of the code, `k`
  and the confirmation key are wiped when done (the crate keeps its own copy of the password
  inside its state without wiping it). A machine in the middle holds a different TLS key towards
  each side, so the confirmations cannot match unless it guessed the code, one online guess per
  try (at most 3 out of 10 000 per request, 20 requests in 10 minutes). Once confirmed, the keys
  are pinned exactly as before. *Older FreeDAWm:* `pair.request` carries a version (`pairing:
  2`) both ways. A PC that sends none is answered "update FreeDAWm on this computer…" before any
  code is shown, and the old `pair.confirm` (the code in the clear) is refused without being
  compared; a PC whose booth answers without the version says "update FreeDAWm on the booth…"
  and never asks for the code. There is no fallback to the old way in either direction. The
  same exchange runs when a PC pairs over the plain link (Take PCs over the encrypted link only
  turned off): it then proves no keys, as that link has none, but the code is still never sent.
  The phone's page keeps typing its code (a browser cannot run the exchange), and only for a
  browser's request; a PC's request cannot be confirmed there. *Kept as is:* PCs already paired
  (their tokens and pinned keys), pairing by invite over the internet. *Tests* (`net` crate:
  `pairing.rs` and `tests/secure_link.rs`, two booths' and PCs' endpoints in one process over
  loopback): the right code pairs and binds the key, and the PC reconnects with it as before
  (also after a restart with its kept key); a wrong code fails and spends a try; a relay posing
  as the booth with its own key towards each side (the forged-record case) fails the check on
  the PC even with the right code, and on the booth when the PC confirms anyway, each counting a
  try, after which the real booth still pairs; a recorded exchange replayed fails; the attempt
  limit still locks; an older PC is told to update and its code in the clear is never compared;
  a booth with an older FreeDAWm is never sent the code; damaged, oversized and out-of-order
  messages are refused, count when they name a real request, and never stop the booth. *Not
  tried:* two real computers on a real network.
- *Editor windows, VST3 and Audio Units through the bridge (design, 2026-11-17).* *Editor:* the
  helper opens the plug-in's editor as a top-level window of its own (CLAP `gui` floating, VST3
  `IPlugView` in a window the helper makes, AU's view in an `NSWindow`), on the helper's main
  thread, which already serves the pipe; the serve loop gains a platform event pump between
  blocks (Win32 `PeekMessage`, Cocoa run loop for a slice, X11/Wayland through the same helper
  the in-process editors use). The pipe carries `editor_open`, `editor_close`, `editor_raise`,
  and the helper sends unrequested lines for parameter changes the editor makes (`param`, with
  gesture begin and end), latency and tail changes, and "editor closed", read by the pipe's reader
  thread into the queue the UI's `idle` drains as it does for in-process editors (`EditorEdit`).
  Embedding the editor in FreeDAWm's own window across processes is platform work for later
  (`XEmbed`/foreign-window parenting, a child `HWND` from another process, no supported way on
  macOS). *VST3:* the helper makes the plug-in with `soundcraft-vst3-host` (its id `vst3:…`)
  exactly as it does CLAP now; the bridge, the exchange and the crash handling do not change;
  the scan's VST3 results already carry the bundle path for `dir`. *AU:* the same with
  `soundcraft-au-host` on macOS; an AU's out-of-process loading by the system (AUv3 extensions)
  is left to the system. *Order:* VST3 first (built in round three; its test plug-in is in the tree, so it was a
  tested step), then the CLAP editor (needs a desktop session to test), then AU (needs a Mac).

**Designs (round three, second pass).** Written for the rows the first round-three pass did not
reach; what was built is said in each.

- *Sound variations and expression maps (built).* One feature closes the Studio Pro and Cubase
  rows: a track's **articulations** (`Track::articulations`: a name and a keyswitch each, and the
  controller lane that chooses, CC 3 by default), set in the instrument track's input menu ›
  Articulations (`track.articulations`). A clip chooses one with a point on that lane whose value
  is the articulation's number ("Set Here" puts one at the insertion point; the MIDI editor's
  controller lane draws and moves them like any point): playback and chase send the keyswitch (a
  short note on the point's channel) instead of the controller (`mix` `send_ctrl`), so a
  locate in the middle of a passage plays it the right way, and cut, copy, split and merge keep
  the changes because they are controller points. Left: per-note articulations (Studio Pro's and
  Cubase's attribute type, which needs a field on every note), articulations switched by
  program change or controller instead of a key, the lane drawn with the articulations' names,
  and importing a library's map files (Cubase's `.expressionmap` is XML; Studio Pro's are
  `.soundvariation`): each a later step on this model.
- *Scripter (Logic). Built (`r5script`) as designed, with the worker: Rhai's evaluation allocates
  (values, maps, strings), so "no allocation after load" cannot hold on the audio thread and the
  worker thread with lock-free rings and one block of latency was chosen (`mix::midi_fx::script`);
  scripts write `handle(e)` and `process(info)`, `send(e)` and `send_after(e, beats)`; parameters
  are sliders in the editor, not yet automatable.* Logic's Scripter runs JavaScript on the MIDI going into an
  instrument. FreeDAWm's MIDI effects run on the audio thread without allocating
  (`mix::midi_fx`); a script engine cannot promise that, so a script would run as a MIDI effect
  of its own whose engine lives on a worker thread fed by lock-free rings, one block late, or,
  better, run in the block with a hard instruction budget and no allocation after load. The
  candidate is **Rhai** (`rhai`, MIT or Apache-2.0, pure Rust, no unsafe needed in our code, a
  sandbox with operation limits): small, embeddable, and its `Engine::set_max_operations`
  bounds a call. A script defines `handle(event)` and `process(block)` (as Scripter's
  `HandleMIDI` and `ProcessMIDI`), sees the song position and tempo, and sends events with
  `send(event, offset)`; parameters it declares become automatable knobs like the built-in
  effects'. It is a new dependency, so it waits for the owner's yes; until then the four
  built-in MIDI effects, Input Transform and the scripting interface from outside (every
  command, section F) are the answer. The Environment (Logic's patchable MIDI objects) is not
  planned: the MIDI effects list, MIDI to Track and the input transformer cover its common uses.
- *Mix Engine FX (Studio Pro) (built 2026-11-18; the filter and the send level driving it, round five; see the row).* A console model applied across every channel
  summed into a bus: each input of the bus gets the same non-linear stage (drive, crosstalk
  between neighbours, a little noise and tolerance per channel so they differ) before the sum.
  In FreeDAWm the sum is `mix`'s bus accumulation, so the stage would be a per-bus setting
  (`Bus::summing`: off, or a model and drive) applied to each source as it is added: the model
  is a static waveshaper plus a gentle filter per channel (the "Console Shaper" idea), with
  each channel's tolerance seeded from the track id so a bounce repeats. Crosstalk mixes a small
  part of each channel into its neighbours in track order. Cost is one shaper per source per
  bus; nothing allocates. Folders that sum (routing folders) get the same setting. End state:
  a Summing menu on the bus strip with two or three models (clean, console, tape) and a drive
  knob, off by default so no session changes sound; bounce and freeze hear it. About a day
  with tests (a sine through two tracks into a summing bus shows the model's harmonics and the
  crosstalk; off is bit-identical to today).
- *Field-recorder workflow (Pro Tools).* *Step 1 built (`b6a6714`): the iXML parser reads the chunk from the imported file's bytes without an XML library, every length bounded; kept by source id on the session, not on `Source` (no change to the saved sources). Steps 2 and 3 built (`b2ce0f0`, `engine::cmd::field`): a folder's WAV files are judged by their headers alone (`audio_io::ixml::read_field_header`: format, length, the `bext` time reference and the iXML, the audio skipped, at most 4,096 files), and only the matching ones are read and join the session as imports do; the folder is the clip's file's own unless one is chosen; timecodes are compared at the session's rate.* Location sound comes as polyphonic or
  mono WAV files with iXML (scene, take, tape, channel names, timecode) beside the Broadcast WAV
  `bext` chunk FreeDAWm already reads and writes. Step 1, the smallest useful one: read iXML on
  import (`audio-io`: the `iXML` chunk as text, parsed for SCENE, TAKE, TAPE, TRACK_LIST names
  and the BWF time reference) and keep it on the source, shown in the clip's properties and
  searchable in the clip list, with the channel names naming the tracks a polyphonic file splits
  into. Step 2: *expand* — for a clip of the edit (from a picture editor's export, mixed down),
  find the files in a chosen folder whose timecode overlaps it and whose scene and take match,
  and put each of their channels on a new track at the same place, cut to the clip's range.
  Step 3: alternate takes (the same scene, another take) offered from the clip's menu. Tests
  use generated WAV files with hand-written iXML, so nothing from outside is needed.
- *Show page (Studio Pro).* *Step 1 built (`640fac9`, `engine::cmd::set_list`, `ui_egui::set_list_ui`): the follow is in the app, after the transport is read each frame, not in the audio thread; a song deleted from the memory locations stays in the list as missing and is skipped. Stage View is drawn over the app window, not as the computer's full screen.* A set list for playing live: an ordered list of songs
  (sessions, or ranges of this session marked by memory locations), each with its own tempo,
  a pause or "go straight on", and a big display of the current and next song, the bar and the
  lyrics (the teleprompter already exists). The smallest useful step stays inside one session:
  a **set list** of memory locations (`Session::set_list`: location ids in order, each with
  "stop after" or "go on"), played in order by the transport (at the end of one range it locates
  to the next), with Window › Transport & Session › Set List to order them and a full-screen view of the song names.
  Switching between sessions while playing (preloading the next session's audio and plug-ins
  in the background) is step 2 and the larger part. Hardware (keyboards, players) is not
  needed to build or test it.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Release coverage used for this
section:

- Pro Tools 2026.4 and 2026.4.1: [Avid, What's New in Pro Tools 2026.4](https://www.avid.com/resource-center/whats-new-in-pro-tools-20264); [Mixdown, 2026.4.1](https://mixdownmag.com.au/news/pro-tools-2026-4-1-brings-m5-mac-support-to-avids-daw/)
- Fender Studio Pro 8 to 8.2: [Fender newsroom, 8.2](https://www.fender.com/newsroom/273556-fender-studio-pro-8-2-expands-mixing-and-creative-production-tools/); [MusicTech, 8.1](https://musictech.com/news/gear/fender-studio-pro-8-1/); [Synth Anatomy, 8](https://synthanatomy.com/2026/01/fender-studio-pro-the-new-studio-one-daw-with-presonus.html)
- Logic Pro 13: [Apple release notes](https://support.apple.com/109503); [Production Expert](https://www.production-expert.com/production-expert-1/logic-pro-13-splits-drum-stems-into-kit-pieces-and-rebuilds-the-browser); [MusicRadar](https://www.musicradar.com/music-tech/apple-goes-all-in-on-ai-with-logic-pro-13-introducing-an-intelligent-assistant-that-can-control-your-daw)
- Cubase 15: [Sound On Sound review](https://www.soundonsound.com/reviews/steinberg-cubase-pro-15?amp); [Synth Anatomy](https://synthanatomy.com/2025/11/steinberg-cubase-15-daw-receives-a-major-update-with-new-features.html)

Search summaries of these pages were used, not the full pages; details such as which edition
includes a feature should be checked before being quoted.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-19 | minor | ARA 2 rows: stages two and three built (section 4, J) |
| 2026-11-18 | minor | ARA 2 rows: stage one built (section 4, J) |
| 2026-11-18 | minor | Logical Editor row: controller events in Select Notes too |
| 2026-11-18 | minor | Step Sequencer row: value rows for controllers and the track's parameters (Works) |
| 2026-11-18 | minor | TrackPunch built (section 2): the Destructive record row Works, destructive modes not planned here |
| 2026-11-18 | minor | Partial rows checked against the code: Flashback capture Works (audio pre-record); Beat Detective, Chord Pads, offline processing, Track Versions, the score and the instrument set say what is left now; the summary re-counted |
| 2026-11-18 | minor | A PC's first pairing proves both PCs' keys with the code by a password-authenticated key exchange (SPAKE2); the code is never sent; older FreeDAWm on either side is told to update |
| 2026-11-18 | minor | Mix Engine FX: a Summing menu on a bus's strip (Console, Tape, drive), off by default |
| 2026-11-18 | minor | Track Pin keeps a track in view while the others scroll; Rename Selected Tracks by a pattern and a number |
| 2026-11-17 | minor | Sound variations and expression maps built as track articulations chosen by a controller lane; designs for Scripter, Mix Engine FX, the field-recorder workflow and the show page; the modulators row in the work list says done |
| 2026-11-17 | minor | Logical Editor: Partial, for notes (the rule-based transformer of section 7) |
| 2026-11-17 | minor | Plug-ins in their own process: VST3 plug-ins run there too (the option is now for CLAP and VST3) |
| 2026-11-17 | minor | Plug-ins in their own process: the plug-in's own presets listed and loaded through its process |
| 2026-11-17 | minor | Plug-ins in their own process: a crashed plug-in says so on its tile, and Restart Plug-in brings it back with its last state |
| 2026-11-17 | minor | Designs: the first pairing by a password-authenticated key exchange (the crate named, not added), and editor windows, VST3 and AU through the plug-in bridge |
| 2026-11-17 | minor | Plug-ins in their own process: a CLAP plug-in's description is read in a helper too, and one that crashed there is never loaded in the app |
| 2026-11-17 | minor | Plug-ins in their own process: a Preferences option (off by default, the variable overrides it) and the helper in every package |
| 2026-11-17 | minor | Rows reconciled with the code: control room, mixer snapshots, handles, QuickPunch, Capture MIDI, clips following chords, warp markers and Mackie/HUI built elsewhere; launcher, mastering page, rack and macros, browser build, EUCON, ARA, hardware DSP, cloud, loop library and vocal synthesis not planned here, with the reason |
| 2026-11-17 | minor | Plug-ins in their own process, first step: CLAP, opt-in, a crash silences the plug-in; browser recording dropped by the owner |
| 2026-11-16 | minor | Take PCs over the encrypted link only is on by default (owner); a saved choice is kept |
| 2026-11-16 | minor | Plug-ins in their own process: the design refined (not built) |
| 2026-11-16 | minor | Browser build: the session kept between visits, with a notice, Export and Clear; a design for recording |
| 2026-11-16 | minor | Remote recording: the encrypted local link and talkback |
| 2026-11-16 | minor | Timing alignment of stacked vocals built (Align timing in the Vocal Studio) |
| 2026-11-16 | minor | Clip launcher deferred by the owner; the Object/Bed switch and the Renderer say what they do not do; Ambisonics: 2nd and 3rd order encoded, 4th to 7th not offered; stored-only settings say so to agents; the video sync offset was read all along; the browser build says it cannot record; the Network window says what pairing allows; bounce to a loudness target; Scout applies a suggestion when asked; search the transcript; video export; a first-run tour; where the rest stands, and designs for the larger items |
| 2026-11-16 | major | First version: signature features of Pro Tools, Studio Pro, Logic Pro and Cubase, and what only FreeDAWm has |
| 2026-11-17 | minor | Field recordings: iXML read on import and kept; Mix Engine FX and the show page left as designs with estimates |
| 2026-11-17 | minor | Rows built elsewhere reconciled (arranger, lyrics in the score, scratch pads, global transpose for MIDI, score import, note rules); what is left on the others stated |
| 2026-11-17 | minor | Field recordings: a clip expands to its location channels, and plays another take of its scene from its menu |
| 2026-11-17 | minor | Show page, step 1: Window › Transport & Session › Set List plays the session's songs in order, with Go On and a Stage View |
| 2026-11-18 | minor | Step Sequencer row: melodic step sequences |
| 2026-11-18 | minor | Note Expression row: per-note expression built |
| 2026-11-18 | minor | Round four: Time Warp by dragging a bar line on the Tempo ruler |
| 2026-11-18 | minor | The browser build removed by the owner: its mentions taken out |
| 2026-11-18 | minor | Scripter: a Script MIDI effect in Rhai (worker thread, one buffer late live, on time in bounces), its editor and four examples; the Environment not planned here |
