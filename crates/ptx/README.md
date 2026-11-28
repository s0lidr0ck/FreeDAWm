# soundcraft-ptx

The Pro Tools session (`.ptx`, Pro Tools 10 and later) reader behind File › Import › Pro Tools
Session... (`file.import_ptx` in `crates/engine/src/ptx_import.rs`, which maps what it reads onto a
FreeDAWm session and writes the import report).

## Where it comes from

The decoder is vendored from **ptx-session-kit** by John Forward (jbl>>FF) / knobmob prod.,
<https://github.com/knobmob/ptx-session-kit>, commit `294c59a` (2026-11-27, version 0.7.0 beta),
under the MIT licence, kept verbatim in `LICENSE-MIT-ptx-session-kit`. The owner approved its use
on 2026-11-28. Every vendored file starts with a comment naming the source file, the commit and
the licence, and listing what FreeDAWm changed in it.

### Taken

From `scraft-bridge/crates/ptx-format/src/`: `scramble.rs` (the descrambler), `tree.rs` (the
`0x5A` block tree), `primitives.rs` (bounds-checked byte readers), `error.rs` and `decode/`
(`mod.rs`, `clips.rs`, `midi.rs`, `mixer.rs`, `plugins.rs`, `timeline.rs`, `tracks.rs`).
`lib.rs` is FreeDAWm's own.

### Left out

- `encode/` (the writer and its stock plug-in data), and every write-side function of the files
  taken (`scramble`, `DEFAULT_HEADER`, `encode_block`, `build_directory_0002`, `lp_bytes`,
  `unix_ns_to_filetime`). FreeDAWm only imports `.ptx`.
- `decode_file` (it read with `std::fs`; the engine reads through its checked file access).
- The `ptx-scraft` mapper (it targets upstream SoundCraft's `craft-*` libraries); FreeDAWm's
  mapping is written against its own model. The mapper was read to learn what decoded fields
  mean, nothing of it was copied.
- The kit's Python tools, command-line app, examples, tests, reports and audio.
- The unused timecode-string helpers and `seconds_to_ticks` of `primitives.rs`.

### Changed (hardening: the input is a file from anywhere)

- **No indexing or slicing** that can panic: the crate denies `clippy::indexing_slicing`; every
  `d[i]` became `byte(d, i)` / `get`, every `&d[a..b]` `span(d, a, b)` / `get`.
- **A panic fixed:** a preset name was cut with `file[..len - 4]`, which panics when the name ends
  in a multi-byte character; it is now cut with `get`.
- **Overflow fixed:** plug-in automation positions (`p0 + offset`) and clip starts
  (`samples - sync_offset`) could overflow on crafted values (a panic in debug builds); they
  saturate now. `find_bytes` refused nothing for an empty needle (`windows(0)` panics).
- **Bounded size:** files over `MAX_PTX_BYTES` (128 MiB) are refused before they are read;
  at most 2 000 000 blocks (was 4 000 000) and 64 levels of nesting.
- **Bounded output and work** for a crafted file: at most 8 192 tracks, 4 096 groups, 256 alternate
  playlists per track, 64 inserts and 64 sends per channel, 100 000 markers and tempo or meter
  events, 2 000 000 MIDI notes, 4 000 000 controller points, 1 MiB of SysEx, 8 000 000 mixer
  automation points, 1 000 000 playlist entries looked at per list read, 65 536 strings per
  string scan, 64 bytes of a loop record kept as hex.
- **Quadratic scans removed:** the MIDI reader searched the rest of a chunk again for every
  controller record (a few megabytes of overlapping records took hours); it now finds them from
  lists made in one pass and counts the records it looks at. Nested playlists made the playlist
  readers read the same entries again at every level; the reading is now counted. Playlist,
  group and alternate-playlist names were de-duplicated by scanning a list (quadratic); they go
  through sets and maps.
- The MPE sub-record loop always moves forward.
- Tests rewritten without the writer: `tests/fixtures.rs` (the kit's two clean-room sessions)
  and `tests/hostile.rs` (a file cut at every 97th byte, random bit flips, empty and foreign
  files, huge declared counts, deep nesting, too many blocks, overlapping MIDI records, nested
  playlists, each within a time bound).

## Not tried

Neither fixture was saved by Pro Tools itself on a paid tier: both were built by the kit's
clean-room writer. Nothing here has been tried on a session from a real Pro Tools user yet.
