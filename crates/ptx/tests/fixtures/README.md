# Test sessions

- `CR_min.ptx` (4 tracks: mono audio, stereo instrument, MIDI, master) and `CR_max3.ptx`
  (30 tracks: folders, auxes, sends, groups, loops, crossfades, a clip group, alternate playlists,
  MIDI with controllers, program changes and SysEx, 17 stock plug-ins) are copied unchanged from
  ptx-session-kit, <https://github.com/knobmob/ptx-session-kit>, commit `294c59a`, `test/CR_min/`
  and `test/CR_max3/`. Copyright (c) 2026 John Forward (jbl>>FF) / knobmob prod., MIT licence
  (`../../LICENSE-MIT-ptx-session-kit`).
- They are clean-room files made by the kit's own writer, not sessions saved by Pro Tools.
- Their audio is not copied. The engine tests that need audio write a short stand-in of their
  own into a temporary folder.
