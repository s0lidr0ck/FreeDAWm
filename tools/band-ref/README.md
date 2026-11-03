# band-ref

MIDI Studio's band, played by MIDI Studio's own C++ through a JSON scenario, written as event lines.
The Rust port (`soundcraft-band`) replays the same scenarios through `BandRuntime` and must produce
identical lines (`crates/band/tests/fidelity.rs`).

```
cargo xtask band-refs [--library <dir>] [name…]
```

- Builds `BandRef` (Release) with the Visual Studio 18 CMake into `target/band-ref`. MIDI Studio
  (`-DMIDISTUDIO_DIR`, required; the xtask passes `$MIDISTUDIO_DIR`, else `<FireStation>/MIDI Studio`
  next to this checkout) is added with `add_subdirectory(... EXCLUDE_FROM_ALL)`: its sources are compiled into our
  build tree, with its `ms_precise` flags (`/fp:precise /bigobj /Zc:__cplusplus /utf-8`). Nothing is
  written into the MS tree.
- Refuses a build older than any file in `MS/Source` (relinks once first).
- Copies the Library (`crates/band/library`, or `--library <dir>`) to `%TEMP%\band-ref-lib` and runs
  every `crates/band/tests/scenarios/*.json` (or the named ones), writing
  `crates/band/tests/refs/<name>.events` (git-ignored).

The tool on its own:

```
BandRef --scenario <file.json> --out <file.events> [--library <dir>]
BandRef --dump-timeline <factory song name> [--out <file.json>] [--library <dir>]
```

`--library` defaults to a temporary copy of `<repo>/crates/band/library`. The tool points
`MIDISTUDIO_LIBRARY` at it (and clears `MIDISTUDIO_SONG`) before the processor exists.

## Scenario file

One JSON object; every key is optional except `name`, which must equal the file name. Unknown keys,
feel flags, parameters, players and targets are errors, never silent defaults.

```json
{ "name": "smoke-factory-44", "pending": false,
  "sample_rate": 48000, "block": 512, "bpm": 120, "num": 4, "den": 4, "bars": 16,
  "song": "Pop Verse Chorus Bridge",
  "key": 0, "scale": 0, "take": 1,
  "feel": {"natural": true, "tension": true, "bass_lock": false, "fills_yield": false, "leave_space": false, "dynamics": false, "mod_wheel": false},
  "players": [null, null, null, null, null, null], "targets": [null, null, null, null, null, null],
  "params": {"m1_energy": 3},
  "events": [ {"at_ppq": 16.0, "param": "m1_energy", "value": 4},
              {"at_ppq": 30.0, "seek_to": 8.0}, {"at_ppq": 40.0, "bpm": 96},
              {"at_ppq": 44.0, "meter": [3, 4]}, {"at_ppq": 52.0, "stop": true},
              {"stopped_blocks": 4, "play": true} ] }
```

| key | default | meaning |
|---|---|---|
| `pending` | `false` | Read by the Rust runner only (`PENDING`, skipped). The tool renders pending scenarios too. |
| `description` | | Free text, ignored. |
| `sample_rate`, `block` | 48000, 512 | `prepareToPlay (sample_rate, block)`. |
| `bpm`, `num`, `den` | 120, 4, 4 | The play head's starting tempo and meter (`num`, `den`: whole numbers 1..32; `block` is a whole number too). |
| `bars` | 8 | Length: the run ends at `bars * (num * 4 / den)` ppq of the **starting** meter. |
| `song` | `defaultSong()` | A factory song name (`<library>/Songs/Factory/<name>.song.json`), or an inline `Song` object in the `.song.json` `song` shape (`songFromVar`, strict). |
| `key`, `scale`, `take` | the factory song's, else the processor's | Host parameter values. |
| `feel` | the factory song's `natural` / `tension`, else the processor's (Natural and Tension on) | `setFeel` per flag: `natural`, `tension`, `bass_lock`, `fills_yield`, `leave_space`, `dynamics`, `mod_wheel`. |
| `players`, `targets` | `null` = the processor default | Six names. |
| `params` | | `Parameters.h` host ids with real values (choice index, integer, 0/1), in file order. |
| `events` | | See below. |
| `library_overlay` | | A folder name (letters, digits, `-`, `_`) under `crates/band/tests/fixtures/libraries/`. Its `Players/*.json` and `Targets/*.json` replace the Library's files of the same name, or add to them, for this scenario only (test targets, a changed target, players moved off an instrument). The tool copies the Library to a temporary folder, copies the overlay's files over it and points `MIDISTUDIO_LIBRARY` there; the Rust runner loads the same files through `LibrarySource::with_user_folder`. The overlay's files are part of the `# case` hash (see Output), so editing one makes its scenarios' refs stale. |

### Setup order

1. `MidiStudioProcessor (true)` (the offline clone: no `migrateSetups`, no `applyDefaultBand`).
2. `setSong` (only when `song` is given).
3. `setParamValue` for `key`, `scale`, `take` (each only when given, or when a factory song supplies it).
4. `setFeel`: a factory song's `natural` and `tension` first, then each flag of `feel` in file order.
5. The `mK_instrument` entries of `params` (file order), so a player of a non-default instrument can be named
   (each instrument change re-picks players as the processor does), then `setPlayer (k, name)` for k = 0..5, then
   `setTargetByName (k, name)` for k = 0..5 (non-null only).
6. The rest of `params`, in file order.
7. `setPlayHead (&head)`, `prepareToPlay (sample_rate, block)`.

### Playing (`Tests/Harness.h renderBand`, extended)

The play head is Harness.h's `Head`: `isPlaying`, `bpm`, `ppqPosition` and the time signature, nothing
else. `ppq` starts at 0 and the loop is:

```
loop:
    apply due events (below)
    if playing and ppq >= endPpq: stop looping
    if stopped and the next unfired event is not a stopped_blocks one: stop looping
    processBlock; record its events under the current block index; block += 1
    if playing: ppq += (block as f64 / sample_rate) * bpm / 60.0
                (C++: `s.block / s.sr * h.bpm / 60.0` with block an int converted to double;
                 left to right in f64: divide, multiply, divide)
    else:       stoppedFor += 1
then: playing = false; one more processBlock (recorded too)
```

Ending: the loop ends either when playing reaches `endPpq`, or when the transport is stopped (a
`stop` event) and the next unfired event is not a `stopped_blocks` one (there is none left, or it is
an `at_ppq` event, which a stopped transport can never reach). In **both** cases the one extra
stopped block is still played and recorded after the loop. So a `stop` with no `stopped_blocks`
event after it ends the run: the stop's own blocks are not played; only the final stopped block is.

**Events** fire once each, **in file order**: event *i* is only considered after event *i-1* has fired.
An event has exactly one trigger:

- `at_ppq` (a number >= 0): due when the block about to play starts at `ppq >= at_ppq`;
- `stopped_blocks` (a whole number >= 0): due when the transport is stopped and `stoppedFor >= stopped_blocks`
  (`stoppedFor` counts the blocks played since the last `stop`).

Its actions apply in this order: `param` + `value` (`setParamValue`), `bpm`, `meter: [num, den]`,
`seek_to` (`ppq = seek_to`; `meter` takes whole numbers 1..32), `stop` (only if playing: `playing = false`, `stoppedFor = 0`), `play`
(`playing = true`). All due events apply before the block plays. An event that never fires is
reported on stderr.

## Output (`.events`)

```
# case <fnv1a64 of the case bytes, 16 lowercase hex digits>
<block> <offset> <status> <d1> <d2>
...
```

One line per event, decimal, `status` the full byte with its channel (internal musician channels
`k+1`). The events are the ones `BufferSink` receives (`setRenderTap`): non-`rendered` ones, offsets
clamped to `0..block-1`, in the order the band emits them (the host's `MidiBuffer` holds the same,
stably sorted by offset). The hash is `xtask::band_refs::scenario_hash` of the case bytes
(`band_refs::case_bytes`): the scenario file's bytes as they are on disk, then, for a `library_overlay`,
each of its `Players/*.json` and `Targets/*.json` (sorted by `<Players|Targets>/<file name>`, byte
order) as a newline, that path, a newline and the file's bytes. Scenario and overlay files are
`-text` in `.gitattributes`, so every checkout hashes the same bytes.

## `--dump-timeline`

For one factory song: `buildTimeline` (every `TimelineEntry` field, plus `total_half`,
`arrangement_length`, `uniform_len_half`, `intro_half`, `has_ending`, `meter_override`) and 200
`songLocate` probes, each with the uniform (`forceGeneral = false`) and general (`true`) result.
Probe *i*: `ppq = -1 + 0.75 i` for i < 100, else `(i - 100) * 1.37 - 0.00005`; `bar_len` 4 on even
*i*, 3 on odd. Doubles are printed with `%.17g` (round-trip exact).
