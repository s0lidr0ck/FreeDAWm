# The Band: owner's checklist

> **Last reviewed:** 2026-10-21 · **Last updated:** 2026-11-19 · **Change:** minor (the Band ruler replaces the Studio window; previously: New Band, the musicians list and As a band) · **Target:** MIDI Studio (the owner's C++/JUCE plugin) inside FreeDAWm

The owner's hands-on pass over the Band, MIDI Studio's six musicians inside the DAW
([design](superpowers/specs/2026-10-21-band-design.md), [plan](superpowers/plans/2026-10-21-band.md)).
The automated side is done: every MIDI Studio scenario in the fidelity suite plays event for event
as MIDI Studio plays it, and the Classic feel matches MIDI Studio's own goldens. What tests can't
hear is the real plugins, so the Band counts as done when this list passes.

You need, installed as VST3 (or CLAP) instruments:
- **MODO DRUM** and **MODO BASS 2** (first: most of the list uses them);
- one Kontakt library the Library has a target for: an **Orange Tree Evolution** guitar
  (Songwriter, Hollowbody Blues, Stratosphere, Mandolin, Bluegrass Banjo) or an **Indiginus**
  instrument (Torch, The Mandolin, The Banjo, The Ukulele, The Fiddle, The Resonator, Blue Street
  Brass);
- **Hammond B-3X**;
- **Pianoteq** or **Miroslav Philharmonik 2**;
- your audio interface at a 64-sample buffer (for the CPU line).

Each line has a tick box, what to do, and the expected result. Tick a line when the result
matches. When it doesn't, write what you saw under the line: the status bar text, and which bar.

## Where things are

- **The quick start:** **Track › New Band** (or **+ Band** in the Band ruler's name cell) makes a track for each musician who is on, playing them, on an installed
  instrument their target names (MODO DRUM for the drummer…) when the plug-in scan found it, else a
  built-in one (Juke for drums; piano, bass, organ, pad, brass and pluck presets on the built-in
  synths). With empty lanes it lays Pop Verse Chorus Bridge. One undo step.
- **The Band ruler** (View › Rulers › **Band**; New Band shows it): one thin row per musician under
  the Sections and Chords rulers, bar by bar, brighter for more Energy, hatched where they sit out,
  dimmed when nobody hears them; hover a bar for who plays it. Its name cell, top to bottom: the
  **band preset** (♪ Factory Band ▾: load one, Save as…, Reload Library); **Natural / Classic** and
  **Feel** ▾ (Tension, As a band, Bass lock, Fills yield, Leave space, Dynamics together, the song
  feel with its presets and dials, who ignores the feel); **Take** ▾; **Musicians** ▾ (reads heard /
  on, e.g. 5/5; a yellow dot when someone who is on is not heard); **+ Band** (New Band).
- **One key:** the song's key is the **Key Signature** ruler's key at bar 1. The band plays in it
  and the Chords ruler names its chords in it; a factory song writes its key there. Key changes
  later in the song are for the Vocal Editor and MIDI Cleanup; the band plays the whole song in the
  bar-1 key.
- **Who plays what:** the Band ruler's **Musicians** ▾: each musician's on/off, track,
  instrument and preset, target, and whether they are heard. **Settings** opens their drawer at
  a small panel. In the Edit window, a track that plays a musician shows a badge at the end of its
  name (**♪2**; hover names the player, ♪ 2 · Benny): click it for the same panel (or right-click the name › **Musician
  Settings...**).
- **A track plays a musician:** on an Instrument or MIDI track, open the track's **input** menu
  (the input selector on the strip in the Mix window, or the I/O column in the Edit window). Under
  the heading **Musicians** are the six, each as `N · player (instrument)`. Pick one. A greyed
  musician is switched off: switch them on in their drawer.
- **The band drawer:** a strip whose input is a musician has a **Band** key (**B** on slim strips)
  in the Mix window. It opens that musician's drawer beside the strip: Enabled, Mute, Instrument,
  Player, Energy, the feel dials, Octave, Length, Velocity, Speed, Voice, the band's Take, and the
  **Instrument plug-in** (the target: Auto and what it picked, or a name) with the rows it needs (keyswitch
  octave, articulations, …).
- **The song:** View › Rulers › **Sections** and View › Rulers › **Chords** show the two
  lanes in the Edit window. Drag on a lane to lay a block or a chord; right-click for its menu;
  double-click on Chords to type a chord. Chords copy as clips do: click one (Shift-click another
  for the run between), then Ctrl/Cmd+C, X, V (at the insertion point), D, or Ctrl-drag a chord
  to drop a copy; Left/Right move the pick.
- **A section's settings:** right-click a block on the Sections ruler. For the block: its letter,
  **Energy (+ bigger, − smaller)** and Take. For every block of its letter: Energy, **Shape**
  (None / Build / Drop), **Role** (Normal / Intro / Ending), Take, **Who Plays** (each musician in
  or out, and at what Energy), **Feel Here** (the section's own Density, Pocket, Groove, Touch) and
  **Rename...**. **Lay Factory Song** lays a factory song from the bar you right-clicked (over what
  is there, or replacing it). On empty ruler space: the letter new blocks take, and **Delete Unused
  Section**.
- **Import chords:** **File › Import › MIDI Chords...** reads a MIDI file's chords onto the Chords
  ruler from the insertion point.
- **Print:** **Track › Print Band to MIDI...** (or the Band ruler's right-click): the whole song, one section block, or the
  Edit selection, and which musicians.

## 1. Setup

- [ ] After everything has merged: `cargo build --release -p soundcraft` from the repo root, then
  start `target\release\freedawm.exe`. Expected: the app opens; View › Rulers › Band and Track ›
  New Band are in the menus (there is no Window › Studio any more).
- [ ] macOS only: `cargo clippy -p soundcraft-au-host --all-targets -- -D warnings`. Expected: no
  warnings or errors (the Audio Units host's band changes have never been compiled on a Mac).
- [ ] View › Rulers › Sections, then right-click the Sections ruler at bar 1 › **Lay Factory Song**
  › **Pop Verse Chorus Bridge**. Expected: the Sections and Chords rulers fill with the song's
  blocks and chords, the Key Signature ruler reads the song's key at bar 1, and (View › Rulers ›
  Band) the Band ruler fills with bars.
- [ ] Key Signature ruler: double-click the song's key at bar 1 and pick **D major**. Expected: the
  chord names on the Chords ruler move to D and the band plays in D; one Undo puts both back.

## 2. MODO DRUM and MODO BASS 2

- [ ] With MODO DRUM and MODO BASS 2 scanned (the plug-in menus list them), in a new session:
  **Track › New Band**. Expected: five tracks (Piano, Drums, Bass, Strings, Brass); Drums plays
  MODO DRUM and Bass MODO BASS 2, the others built-in instruments; the Band ruler's Musicians list
  says so and the status line counts the built-in ones; each track header shows its musician's
  badge (♪1, ♪2…); the Band, Sections and Chords rulers show; Play is heard at
  once; one Undo removes it all.
- [ ] New Instrument track "Drums" with **MODO DRUM** on it; input → Musicians → the drummer
  (load the band preset **Factory Band** first if no slot plays Drums). New Instrument track
  "Bass" with **MODO BASS 2**; input → the bassist. Expected: each strip shows the Band key.
- [ ] Open each strip's band drawer. Expected: the **Instrument plug-in** row reads **Auto** with
  "MODO DRUM" (and "MODO BASS 2") as what it picked, from the plugin on the track.
- [ ] Play from bar 1. Expected: both musicians play along with the song, following its sections
  (Energy changes on the Band ruler are heard at section changes). No other channel
  setup is needed.
- [ ] Band ruler: **Natural** (the default). Listen to a chorus and the ending.
  Expected: Natural's extras are audible: on drums, cymbal chokes at breaks and the ending,
  rimshots on hard snare hits, open hats and the ride bell at Energy 4, flams; on bass, slides,
  hammer-ons, ghost notes in busy grooves and the occasional fall.
- [ ] Still on Natural, with **As a band** on (the default in a new session): play the pop song
  from bar 1 to the bridge. Expected: the band sounds like one band, not six sequencers: the time
  moves a little and everyone moves with it; the bass sits with the kick but not glued to it; the
  bar before each chorus swells and the drummer fills into it while the others keep to the beats;
  piano chords are struck, not rolled up every time. Switch **As a band** off and play the same
  bars: Natural as before (tighter, every bar's timing alike, every chord rolled from the bottom).
  The same Take plays the same every time.
- [ ] Switch to **Classic** and play the same bars. Expected: the extras stop (plain MODO maps,
  as MIDI Studio's Classic plays them); back to Natural, they return.
- [ ] In the bass drawer, change Energy 2 → 4 while playing. Expected: the bass gets busier from
  the next bar; Edit › Undo puts it back to 2.

## 3. Kontakt keyswitches and the Hammond

- [ ] Instrument track with **Kontakt** loading your Evolution or Indiginus instrument (set up as
  its target's help text says: for Evolution, every articulation set to Keyswitch, Chords mode
  off). Input → a guitar (or the matching instrument's) musician; in the drawer pick the Instrument plug-in
  by name if Auto didn't. Expected: the drawer shows the target's rows (keyswitch octave,
  articulations), and the guitar plays with its articulations changing (the Kontakt keyboard shows
  the keyswitch keys being pressed). No stuck articulation after a stop.
- [ ] Instrument track with **Hammond B-3X**; input → a musician playing Electric Keys (organ);
  Instrument plug-in **Hammond B-3X**. For the lower manual, copy `crates\band\library\Targets\Hammond B-3X.json`
  to `%APPDATA%\FreeDAWm\Band\Targets`, set its `lowerManualChannel` to 2, and Reload Library
  (section 8). In B-3X (Controls › MIDI), set the Upper Manual to channel 1 and the Lower Manual to 2.
  Expected: the upper manual plays; in Natural feel the lower manual plays on some bars on its
  own channel, from C3 up, with its own registration; the drawbar keys change the registration at
  section starts.

## 4. A VST3 program change

- [ ] Instrument track with **Pianoteq** (or **Miroslav Philharmonik 2**). Import (File › Import
  › MIDI) a short `.mid` with a program change at bar 1 and bar 5, onto that track. Play from
  bar 1. Expected: the instrument's preset changes at bar 5 (and is back to the bar 1 preset when
  you locate before bar 5 and play again). If nothing changes, the plugin maps no program
  parameter: note which plugin.

## 5. Stop, locate, loop

- [ ] With the band playing (drums, bass, keys): Stop in the middle of a long chord. Expected:
  silence at once; no note hangs on any instrument.
- [ ] While playing, click the ruler to locate elsewhere (several times, fast). Expected: no
  hanging note; the band picks up at the new place within a bar.
- [ ] Set a loop over 4 bars across a section change and play for a minute. Expected: every pass
  sounds the same in shape; no hanging note at the loop's end; no keyswitch left held.

## 6. Print, edit, undo

- [ ] Track › **Print Band to MIDI...**: Song, all musicians, **Print**. Expected: the status reads
  "Printed N clip(s)"; each musician track gets one MIDI clip over the song and its input becomes
  **None**; playback sounds the same as before the print.
- [ ] Open a printed clip in the MIDI editor and move or delete a few notes. Expected: the edit
  is heard; the clip's controllers (pedals, keyswitches, the Hammond's lower manual) still play.
- [ ] Edit › Undo (Ctrl+Z) until the print is undone. Expected: first your note edit goes, then
  the print: the clips are gone (or the clips they replaced are back) and every track's input is
  its musician again, in one undo step.

- [ ] On the Chords ruler: click a chord, Shift-click the chord three bars on, Ctrl+C (Cmd+C);
  click in a track at a later bar, Ctrl+V. Expected: the run lands there on the half bar with its
  chords, lengths and gaps, outlined as the new pick; Ctrl+D then puts another copy straight
  after; Ctrl+Z takes back one paste or duplicate at a time. Ctrl-drag a chord: a ghost with a "+"
  follows, the copy drops on release and the original stays; Alt-drag still moves off the grid.

## 7. Band presets

- [ ] Band ruler › ♪ Factory Band ▾: click **Funk Machine**. Expected: the six slots change (instruments,
  players, names in the input menus); the song is unchanged; one Undo step brings the old band
  back.
- [ ] Change a player or two, then **Save as…** "My Band". Expected: `%APPDATA%\FreeDAWm\Band\Bands\My Band.json`
  exists and "My Band" is listed (not as factory). Load another preset, then "My Band". Expected:
  your changes come back. Saving as a factory name (e.g. "Bluegrass") is refused.

## 8. Your own players (the user Library)

- [ ] Copy a player JSON from MIDI Studio (or a copy of one from `crates\band\library\Players`
  with its `name` changed) into `%APPDATA%\FreeDAWm\Band\Players`. Band ruler › ♪ band preset ▾ ›
  **Reload Library** (or `band.library.reload` from the CLI/MCP). Expected: the new player is in
  the drawer's Player browser under its instrument, and plays. A broken JSON file is named in the
  status (skipped, with the reason) and nothing else changes.

## 9. Project round trip

- [ ] With a song, a custom band, a section edited from the Sections ruler's right-click (Shape
  Build, a musician out in Who Plays) and a feel preset
  set: File › Save, close the project, open it again. Expected: the same six musicians, drawer
  values, targets, key, scale, Take, feel, sections and chords; it plays the same.

## 10. CPU

- [ ] Six musicians on six instrument tracks (MODO DRUM, MODO BASS 2, the Kontakt guitar, B-3X,
  Pianoteq, one more), buffer **64** samples (Setup › Playback Engine), Natural feel, Energy 4 on
  all, play the whole song. Expected: no dropouts; the status bar's underrun count stays 0.

## Not in this round

Live mode (playing chords on a MIDI keyboard; needs MIDI input, gap G-02), kept takes, audition
while stopped, a per-song meter override and MIDI Studio's MIDI export tabs come in round 2:
[gaps.md](gaps.md) G-23. The Mod wheel feel switch stays hidden until MIDI input exists.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-19 | minor | The Band ruler replaces Window › Studio: band controls in its name cell, section settings on the Sections ruler's right-click, one key (the Key ruler at bar 1), track header badges, Print Band to MIDI and Import MIDI Chords as menu items, Instrument plug-in, strings at Octave 0 |
| 2026-11-19 | minor | Track › New Band; the Studio's Musicians list and a musician's Settings in the Studio; Musician Settings... in the Edit window |
| 2026-11-19 | minor | As a band: the band-feel layer on top of Natural (on in new sessions, off in older ones) |
| 2026-11-18 | minor | Chords copy: Copy, Cut, Paste, Duplicate, Shift-click runs, arrows, Ctrl-drag copy (sections too) |
| 2026-10-21 | major | Created: the owner's pass over the Band with real plugins |
