# The Band: owner's checklist

> **Last reviewed:** 2026-10-21 · **Last updated:** 2026-10-21 · **Change:** major (created) · **Target:** MIDI Studio (the owner's C++/JUCE plugin) inside FreeDAWm

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

- **A track plays a musician:** on an Instrument or MIDI track, open the track's **input** menu
  (the input selector on the strip in the Mix window, or the I/O column in the Edit window). Under
  the heading **Musicians** are the six, each as `N · player (instrument)`. Pick one. A greyed
  musician is switched off: switch them on in their drawer.
- **The band drawer:** a strip whose input is a musician has a **Band** key (**B** on slim strips)
  in the Mix window. It opens that musician's drawer beside the strip: Enabled, Mute, Instrument,
  Player, Energy, the feel dials, Octave, Length, Velocity, Speed, Voice, the band's Take, and the
  **Target** (Auto and what it picked, or a name) with the rows that target needs (keyswitch
  octave, articulations, …).
- **The song:** View › Rulers › **Sections** and View › Rulers › **Chords** show the two
  lanes in the Edit window. Drag on a lane to lay a block or a chord; right-click for its menu;
  double-click on Chords to type a chord.
- **Window › Studio:** the song-wide controls. Key and Scale at the top, then four tabs:
  **Song** (the Band view, the factory songs, Import MIDI chords…, **Print…**), **Sections** (the
  definitions A–H), **Feel** (Natural / Classic, Tension, the song feel and its presets) and
  **Band** (band presets: Load, Save as…, Reload Library).
- **Print:** Window › Studio › Song › **Print…**: the whole song, one section block, or the
  Edit selection, and which musicians.

## 1. Setup

- [ ] After everything has merged: `cargo build --release -p soundcraft` from the repo root, then
  start `target\release\freedawm.exe`. Expected: the app opens; Window › Studio is in the menu.
- [ ] macOS only: `cargo clippy -p soundcraft-au-host --all-targets -- -D warnings`. Expected: no
  warnings or errors (the Audio Units host's band changes have never been compiled on a Mac).
- [ ] Window › Studio › Song: lay the factory song **Pop Verse Chorus Bridge** at bar 1. Expected:
  the Sections and Chords rulers appear in the Edit window with the song's blocks and
  chords, and the Band view in the Studio fills with bars.

## 2. MODO DRUM and MODO BASS 2

- [ ] New Instrument track "Drums" with **MODO DRUM** on it; input → Musicians → the drummer
  (load the band preset **Factory Band** first if no slot plays Drums). New Instrument track
  "Bass" with **MODO BASS 2**; input → the bassist. Expected: each strip shows the Band key.
- [ ] Open each strip's band drawer. Expected: the **Target** row reads **Auto** with
  "MODO DRUM" (and "MODO BASS 2") as what it picked, from the plugin on the track.
- [ ] Play from bar 1. Expected: both musicians play along with the song, following its sections
  (the Band view's playhead moves; Energy changes are heard at section changes). No other channel
  setup is needed.
- [ ] Window › Studio › Feel: **Natural** (the default). Listen to a chorus and the ending.
  Expected: Natural's extras are audible: on drums, cymbal chokes at breaks and the ending,
  rimshots on hard snare hits, open hats and the ride bell at Energy 4, flams; on bass, slides,
  hammer-ons, ghost notes in busy grooves and the occasional fall.
- [ ] Switch to **Classic** and play the same bars. Expected: the extras stop (plain MODO maps,
  as MIDI Studio's Classic plays them); back to Natural, they return.
- [ ] In the bass drawer, change Energy 2 → 4 while playing. Expected: the bass gets busier from
  the next bar; Edit › Undo puts it back to 2.

## 3. Kontakt keyswitches and the Hammond

- [ ] Instrument track with **Kontakt** loading your Evolution or Indiginus instrument (set up as
  its target's help text says: for Evolution, every articulation set to Keyswitch, Chords mode
  off). Input → a guitar (or the matching instrument's) musician; in the drawer pick the Target
  by name if Auto didn't. Expected: the drawer shows the target's rows (keyswitch octave,
  articulations), and the guitar plays with its articulations changing (the Kontakt keyboard shows
  the keyswitch keys being pressed). No stuck articulation after a stop.
- [ ] Instrument track with **Hammond B-3X**; input → a musician playing Electric Keys (organ);
  Target **Hammond B-3X**. For the lower manual, copy `crates\band\library\Targets\Hammond B-3X.json`
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

- [ ] Window › Studio › Song › Print…: Song, all musicians, **Print**. Expected: the status reads
  "Printed N clip(s)"; each musician track gets one MIDI clip over the song and its input becomes
  **None**; playback sounds the same as before the print.
- [ ] Open a printed clip in the MIDI editor and move or delete a few notes. Expected: the edit
  is heard; the clip's controllers (pedals, keyswitches, the Hammond's lower manual) still play.
- [ ] Edit › Undo (Ctrl+Z) until the print is undone. Expected: first your note edit goes, then
  the print: the clips are gone (or the clips they replaced are back) and every track's input is
  its musician again, in one undo step.

## 7. Band presets

- [ ] Window › Studio › Band: Load **Funk Machine**. Expected: the six slots change (instruments,
  players, names in the input menus); the song is unchanged; one Undo step brings the old band
  back.
- [ ] Change a player or two, then **Save as…** "My Band". Expected: `%APPDATA%\FreeDAWm\Band\Bands\My Band.json`
  exists and "My Band" is listed (not as factory). Load another preset, then "My Band". Expected:
  your changes come back. Saving as a factory name (e.g. "Bluegrass") is refused.

## 8. Your own players (the user Library)

- [ ] Copy a player JSON from MIDI Studio (or a copy of one from `crates\band\library\Players`
  with its `name` changed) into `%APPDATA%\FreeDAWm\Band\Players`. Window › Studio › Band ›
  **Reload Library** (or `band.library.reload` from the CLI/MCP). Expected: the new player is in
  the drawer's Player browser under its instrument, and plays. A broken JSON file is named in the
  status (skipped, with the reason) and nothing else changes.

## 9. Project round trip

- [ ] With a song, a custom band, a section edited in the Studio's Sections tab and a feel preset
  set: File › Save, close the project, open it again. Expected: the same six musicians, drawer
  values, targets, key, scale, Take, feel, sections and chords; it plays the same.

## 10. CPU

- [ ] Six musicians on six instrument tracks (MODO DRUM, MODO BASS 2, the Kontakt guitar, B-3X,
  Pianoteq, one more), buffer **64** samples (Setup › Playback Engine), Natural feel, Energy 4 on
  all, play the whole song. Expected: no dropouts; the status bar's underrun count stays 0.

## 11. The web build

- [ ] `trunk serve` in `apps\soundcraft-web`, open the page. Window › Studio › Song: lay the
  factory song **12-bar Blues**; give a track the bassist; play. Expected: the song lays (the
  embedded Library: no user folder on the web, so Save as… is refused and Import MIDI chords is
  unsupported) and the built-in instrument plays the bass part.
- [ ] Open a band project saved by the desktop app (File › Open Session…, or drop the file on the
  page). Expected: it opens with its band and lanes.

## Not in this round

Live mode (playing chords on a MIDI keyboard; needs MIDI input, gap G-02), kept takes, audition
while stopped, a per-song meter override and MIDI Studio's MIDI export tabs come in round 2:
[gaps.md](gaps.md) G-23. The Mod wheel feel switch stays hidden until MIDI input exists.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-21 | major | Created: the owner's pass over the Band with real plugins |
