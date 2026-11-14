# Control protocol

FreeDAWm can be driven completely by other programs: scripts, test harnesses and AI agents.
Every menu item, button, mouse gesture and keystroke maps to a **command** (or to synthetic
input), and every command is reachable from:

| Surface | How |
|---|---|
| Desktop app control channel | `freedawm --control <port>` (`0` = pick a free port; printed as `FREEDAWM_CONTROL_PORT=…`, and as `SOUNDCRAFT_CONTROL_PORT=…` for older scripts) |
| The local network (paired PCs) | Setup › Network › Allow control from this network: the same methods on port 48100 after pairing and `hello` (see [LAN remote](#lan-remote-the-service-remote-page-and-remote-recorders)) |
| CLI against a running app | `freedawm-cli app --port <port> <method or command id> [JSON]` |
| Headless CLI | `freedawm-cli run --demo --cmd 'mix.volume={"track":"Kick","db":-6}' --bounce out.wav` |
| MCP (headless) | `freedawm-cli mcp [--demo]` |
| MCP (bridged to the app) | `freedawm-cli mcp --connect <port>` |

## Isolated automation profiles

Set `FREEDAWM_CONFIG_DIR` to a fresh directory to isolate UI preferences, `Autosave`, `Presets`, and `logs`. A launch with `--control` (or `FREEDAWM_CONTROL_PORT`), `--headless`, `FREEDAWM_NO_PREFS` or `CI` set never shows the launch splash, whatever the "Show splash screen at launch" preference says; `FREEDAWM_NO_SPLASH` (or `--no-splash`) turns it off for any other launch.
The nonempty value is used as the complete settings directory, even in portable mode (`portable.txt` beside the executable, which otherwise uses `FreeDAWmData` next to it). Unset retains the normal platform paths;
an explicit empty value disables the settings directory instead of falling back to a personal profile. Save synthetic sessions to explicit scratch paths. Do not use `FREEDAWM_NO_PREFS`
when testing preference persistence across restarts.

Run UI acceptance with `--no-audio`. This disables the output player; it does not disable recording
input opened by arm/monitor commands. An automated UI smoke run must never arm a track, enable input
monitoring, or start recording. Control input injects egui events; it does not establish operating-system
input delivery or screen-reader acceptance. Native screenshots use the presented viewport frame.

## Wire format

Newline-delimited JSON over TCP on `127.0.0.1`. One request per line, one reply per line:

```json
{"id": 1, "method": "engine.execute", "params": {"command": "track.new", "params": {"count": 2, "format": "Stereo"}}}
{"id": 1, "ok": true, "result": {"tracks": [12, 13]}}
```

Errors: `{"id": 1, "ok": false, "error": "bad parameters for `mix.volume`: no track named `Nope`"}`.

## Methods

| Method | Params | What it does |
|---|---|---|
| `engine.execute` (alias `command`) | `{command, params}` | Run a command programmatically. **Never opens a dialog**; empty params use defaults or the current selection. |
| `engine.commands` | `{filter?}` | Every command: id, label, menu path, shortcut (the one in effect: the user's override, else the registry's), params doc, enabled + reason. Includes UI-layer commands (`window.*`). |
| `engine.parity` | `{}` | Parity report against the incumbent's menu catalog. |
| `session.inspect` | `{detail?: "summary"|"full"}` | The document: tracks, clips (with times), inserts (+params when full), sends, routing, automation, markers, groups, busses, sources (`loaded` true when decoded audio is in the pool), selection, zoom (`samples_per_px`, `scroll`), transport, undo label. |
| `ui.inspect` | `{}` | UI state (`window`, panels, dialogs), window size, playing/position, audio device, and `edit_layout` (timeline rect and every track row's rect, for clicking). |
| `ui.set` | any `UiState` fields | e.g. `{"window": "Mix", "narrow_mix": true}`. |
| `ui.shortcuts.get` | `{}` | The user's keyboard-shortcut overrides (`ui.json` `shortcuts`): `{"shortcuts": {command id: "Cmd+Alt+K" \| ""}}`; `""` means no shortcut, a missing id keeps the registry's. |
| `ui.shortcuts.set` | `{command, shortcut: string \| null, take?: false}` | Gives a command a shortcut in the registry's form (`Cmd+Shift+K`; Cmd is Ctrl on Windows/Linux); `null` or `""` removes it. A shortcut another command has is refused with its name unless `take: true` (which removes it there); a fixed context key (Space, Enter, F1–F10, Tab…) and commands with fixed keys are always refused. Returns the override map. |
| `ui.shortcuts.reset` | `{command?}` | Back to the registry's shortcut for one command, or for all without `command`. Returns the override map. |
| `ui.menu.list` | `{}` | Every catalog menu path (the incumbent's menu leaves, the parity list) and the command that implements it (or null). The menu bar as shown is `crates/ui-egui/src/menubar.txt`; `ui.menu.list` does not list it yet (items only on the bar, such as the AI menu or File › Export › Stems…, are missing, and catalog items taken off the bar are still listed). |
| `ui.menu.invoke` | `{path}` or `{id}` | Like clicking the menu item: may open its dialog (e.g. `"Track > New..."`). `path` is a catalog path or a path as the menu bar shows it (`"Window > More > Search"`). A greyed-out "coming soon" item (e.g. `"Setup > Preferences..."`) is refused with its reason; `{id}` still runs the command. |
| `ui.click` | `{x, y, button?, count?, cmd?, shift?, alt?, ctrl?}` | Synthetic click (window points). |
| `ui.drag` | `{x, y, to_x, to_y, steps?}` | Synthetic drag with the primary button. |
| `ui.scroll` | `{x, y, dx?, dy?, unit?: "point"\|"line", steps?, cmd?, shift?, alt?, ctrl?}` | A two-finger trackpad swipe over `(x, y)`: `dx`/`dy` points spread over `steps` frames (default 8). Positive values move the content right/down (natural scrolling). `unit: "line"` sends mouse-wheel notches instead. With `cmd` it zooms; with `shift` it scrolls sideways. |
| `ui.zoom` | `{x, y, factor, steps?}` | A trackpad pinch around `(x, y)`: `factor` > 1 spreads the fingers (zooms in). |
| `ui.move` | `{x, y}` | Move the pointer. |
| `ui.key` | `{key, cmd?, shift?, alt?, ctrl?}` | Key press (`Space`, `Enter`, `A`, `F7`, `=`…). |
| `ui.text` | `{text}` | Type text into the focused field. |
| `ui.screenshot` | `{path?}` | PNG of the window (base64 in `png_base64` when no path). |
| `app.quit` | `{}` | Quit. |

### Old command ids

Renamed commands keep their old ids as aliases (`COMMAND_ALIASES` in
`crates/engine/src/cmd/mod.rs`), so existing scripts and control clients keep working. An alias
runs the current command; command lists (`engine.commands`, MCP, `freedawm-cli`) show only the
current id.

| Old id | Current id | Since |
|---|---|---|
| `audiosuite.process` | `process.run` (the Process menu: offline processing of clips) | 2026-11-02 |

## Positions and references

- Tracks: id (number) or name (`"Kick"`).
- Clips: id (from `session.inspect`).
- Positions: samples (`96000`), seconds (`{"seconds": 2.5}`), or a time string in any timebase,
  optionally prefixed: `"bars_beats:5|1|000"`, `"min_secs:0:12.500"`, `"timecode:00:00:10:00"`,
  `"samples:48000"`.
- Ranges: `start` + `end` (or `start` + `length`); omitted = the current edit selection
  (`file.bounce_mix`: the loop, else the whole session; see [Sticky loop](#sticky-loop)).

## Examples

```sh
P=$(freedawm --demo --control 0 | sed -n 's/FREEDAWM_CONTROL_PORT=//p')   # or read stderr
freedawm-cli app --port $P mix.solo '{"track":"Bass"}'
freedawm-cli app --port $P transport.play
freedawm-cli app --port $P ui.screenshot '{"path":"/tmp/shot.png"}'
freedawm-cli app --port $P ui.menu.invoke '{"path":"Track > New..."}'
```

Offscreen (no window): `cargo run -p soundcraft-ui-egui --example ui_shot -- out.png script.jsonl`,
where each script line is a request (`{"method": …, "params": …}`), `{"shot": "path.png"}` or
`{"steps": n}`. Use `--system-theme light|dark|none` to supply the initial OS appearance, and
script events such as `{"system_theme": "dark"}` to change it on subsequent frames.

Appearance defaults to Dark. `ui.theme {"mode":"system"}` follows the appearance supplied by
the native or web integration at launch and on later frames, while the saved choice stays System.
When the integration cannot report an appearance, System uses Dark. Manual Light and Dark stay
fixed when the OS appearance changes. An offscreen render without `--system-theme` supplies no
OS appearance and therefore resolves System to Dark.

## Solo

`mix.solo` (`{tracks?, value?, exclusive?}`), `mix.solo_safe` and `mix.clear_solos` set the flags;
what a solo leaves audible follows the signal path. While any active track is soloed, a track
that is neither soloed nor solo safe is muted unless it is on a soloed track's path:

- **upstream**: it feeds a soloed track through main outputs, transitively. Soloing a Kick Bus
  keeps Kick In and Kick Out (routed to the Kick Bus's input bus) audible. Sends don't count:
  soloing a reverb return does not unmute the tracks that send to it;
- **downstream**: a soloed track reaches it through outputs or sends, transitively. Soloing
  Kick In keeps the Kick Bus, the Drum Bus it plays into and any reverb return it sends to
  audible (Kick Out and the Snare stay muted).

Routing folders are auxes on their own bus, so their members follow the same rule. Inactive
tracks neither solo nor carry a path; routing cycles are handled. The S and M keys of the Mix
strips and the Edit headers show exactly what the engine plays (`soundcraft_model::SoloScope`,
worked out once per session revision and shared by every strip and header):

| Look | Meaning | Accessible name / hover |
|---|---|---|
| S lit in the solo colour | soloed | `<track> solo` (toggled) / "Solo" |
| S outlined, a low band lit in the solo colour | implied solo: plays because it is on a soloed track's path | `<track> solo, implied by <soloed track>` / "Playing because <soloed track> is soloed" |
| M in a dark shade of the mute colour | muted by another track's solo | `<track> mute, muted by solo` / "Mute (muted by another track's solo)" |

## Freeze

| Command | Params | Does |
|---|---|---|
| `track.freeze` | `{tracks?, freeze?: bool, upto?: module id}` | Freezes each audio track, and each MIDI or instrument track with an instrument: renders its clips (on a MIDI or instrument track, the instrument playing its MIDI clips) through its module chain (trim, phase, every module and its automation; pre-fader: no fader, pan, mute, sends, VCA or fader-section automation) from the session start to its last unmuted clip plus the chain's tail (audio tracks: the frozen modules' reported `tail_samples`, at least 2 s and at most 30 s; MIDI and instrument tracks: the release tail, rendered on until the output stays below −90 dBFS for 100 ms, and at most 10 s past the end of the last clip plus a positive MIDI Track Offset, so past the last note-off; a Real-Time Properties delay moves notes within their clip only, the mix ends every note at its clip's end) into a new session audio source named `<track> Freeze` (written to `Audio Files/` on save, like Commit). The track then plays that render in place of its clips, its instrument and the frozen modules; those are not processed (a thawed instrument starts clean, with no voice held over from before the freeze). With `upto` ("Freeze up to here"), the render stops after that module (it is included): the modules after it stay live, processing the render with their automation, then the fader section as usual. Without `upto`, the whole chain is frozen. Result `{frozen: [track ids], seconds, warnings}`. A frozen track asked for a different `upto` (or, with `freeze: true` and no `upto`, the whole chain) is rendered again there and the old render leaves the session ("moving the freeze point"); the same point again changes nothing. With `freeze: false`, or with no `freeze` and no `upto` when every named track is frozen already, it thaws them instead (result `{unfrozen}`). One undo step. Refused, freezing nothing: a track that is neither audio nor carries an instrument ("Freeze works on audio tracks and tracks with an instrument: `<track>` is an Aux Input with no instrument", or "… `<track>` has no instrument" on an instrument track without one, "… `<track>` is a MIDI track with no instrument (use an instrument track)" on a MIDI track), a MIDI or instrument track whose input is a band musician ("`<track>` plays a band musician live: set its input back to its clips to freeze it"), a track with no clips or only muted ones ("has nothing audible to freeze"), an `upto` that is not a module of the track ("`<track>` has no module `<id>`"). |
| `track.unfreeze` | `{tracks?}` | Thaws the tracks: clips and modules play live again, the render leaves the session (its file stays on disk; undo brings it back). Result `{unfrozen}`. One undo step. |

### Renders through the live plugins

In the app, `track.freeze`, `track.commit`, `track.bounce` and the Bounce dialog (File › Bounce
Mix…) render through the plugin instances that are playing, Pro Tools offline-bounce style,
instead of making copies of hosted plugins from their stored state (a copy restored the state and
then had the session's stale parameter values pushed over it: an instrument whose preset was
chosen in its own editor bounced quieter than it played). The transport stops; the player lends
its engine, with every hosted instance in it, to the render, which runs on the main thread a
slice per frame behind a progress window (Cancel or Esc hands everything back untouched); the
audio callback plays silence meanwhile; afterwards each instance's state is put back as it was
and the engine returns. `track.freeze`, `track.commit` and `track.bounce` therefore return
`{rendering: true}` at once from `engine.execute` and run the command itself once the render is
done (the status line then reads as usual: "Froze Keys (4.2 s rendered)", "Committed to Vox.cm");
an unfreeze, or a call with nothing to render, runs at once as before. While a render runs the
status line reads `Rendering…` and these are refused with that text: `transport.play`,
`transport.play_selection`, `transport.toggle`, `transport.record`, `transport.half_speed`,
another `track.freeze` / `track.commit` / `track.bounce` / `file.bounce_mix`, and `session.new`
/ `session.open` / `session.close` / `session.revert` (a session dropped on the window is refused
the same way). Setup › Playback Engine › Apply and Reconnect audio device are refused meanwhile too
("Finish or cancel the render first"). A programmatic `file.bounce_mix` (control
channel, CLI, MCP) still renders synchronously and returns once the file is written, through
instances of its own; headless (`freedawm-cli`, MCP) there is no player and every render does.

While a track is frozen, its fader, pan, sends, mute, solo, output and their automation stay live,
and so do the modules after the freeze point (all of them: parameters, bypass, automation, adding,
removing and reordering them among themselves, adding one right after the point). Any command that
would change how the frozen part sounds, so that the render no longer matches, is refused, changing
nothing, with `` `<command>` is not available: `<track>` is frozen. Unfreeze the track to edit
it``. That covers:

- the active playlist's clips: position, length, source and offset, gain and envelope, fades,
  mute, stretch, clip effects. Switching to a playlist that sounds different is refused too;
- the frozen modules (the whole chain, or down to and including the freeze point): plugin,
  parameters, bypass, active, state, sample, sidechain, files, link; removing one; inserting a
  module before or at the point (which would push the point down); moving a module across the
  point either way;
- the frozen modules' automation, and the automation mode;
- input, trim, phase, width and Elastic Audio;
- on a MIDI or instrument track: the MIDI clips' notes and controllers (they are read-only, as
  audio clips are), their MIDI Cleanup (the settings, the Original/Cleaned switch, Commit and
  Remove; the cleaned notes the render played are kept as they are while the track is frozen and
  derived again when it thaws; since they are not saved with the session, a chord or key edit that
  would change what a frozen clip on Cleaned derives from, a chord under the clip, the band's key,
  is refused: `` `<track>` is frozen, and its MIDI Cleanup follows the chords. Unfreeze it to
  change them ``; a chord elsewhere is allowed), the instrument (plugin, parameters, state) and
  the clips' timebase. Any
  edit of the tempo map is refused while one is frozen (its MIDI plays to the tempo), whether or
  not a note falls where it changed: `` `<track>` is frozen, and its MIDI plays to the tempo.
  Unfreeze it to change the tempo ``. So are its MIDI Real-Time Properties (`event.midi_rtp`,
  `track.write_midi_rtp`: `` `<track>` is frozen, and its MIDI Real-Time Properties shape its notes.
  Unfreeze it to change them ``) and its MIDI Track Offset (`event.midi_track_offsets`:
  `` `<track>` is frozen, and its MIDI Track Offset moves its notes. Unfreeze it to change it ``);
  the MIDI Real-Time Properties window is greyed, with that reason, while a selected MIDI or
  instrument track is frozen.

Still allowed: clip names, colours, ratings, locks, groups and sync points, edits to inactive
playlists, a duplicate playlist (it sounds the same), module knob choices and preset labels,
track name and colour. Record-enabling a frozen track is refused ("Unfreeze the track to record on
it"), and a recording does not start while a frozen track is armed. Undo, redo and
`mix.module_state` (storing a hosted plugin's own state) are not refused.

The render plays the track alone, so a sidechain key from another track is silent in it. The
freeze result's `warnings` (and the status line and messages) name each frozen module keyed that
way: `` `<track>` was rendered without the sidechain key from `<source>` ``. Live modules after
the freeze point still hear their keys, so they are not named.

Bypass Inserts (All, A-E, F-J) on a frozen track switches only its live modules after the freeze
point (none on a whole-chain freeze). A duplicate made without its playlists, automation or
modules, and a track imported from another session, play live (they don't keep the render).
Only tracks a command actually changed are checked, so a frozen track never blocks edits elsewhere.

Delay compensation: the render has the frozen modules' latency removed, so a frozen strip reports
only the latency of its live modules after the point, and lines up with the other tracks exactly
as the unfrozen track did. A track whose clips are all muted is
refused with "nothing audible to freeze". Deleting a frozen track drops its render from the session
unless something else (a clip, another frozen track, a sampler) plays it.

The session stores the render on the track (`frozen: true`, `freeze: {source, start, upto?}`;
absent on unfrozen tracks, so older sessions load unchanged; no `upto` means the whole chain, so
frozen sessions from before "Freeze up to here" load as they were). A session whose frozen render
is missing loads with the track thawed and a message (`` `<track>`: its frozen audio is missing,
so the track was unfrozen and plays live ``) besides the usual missing-media report; so does one
whose `upto` names no module of the chain (`` `<track>`: the module it was frozen up to is
missing, … ``). A `frozen` flag without a render (sessions from before Freeze rendered) loads
thawed.

In the Mix window, right-clicking a module tile (or a module drawer's header) on an audio, MIDI or
instrument track offers "Freeze up to here", "Freeze whole chain" and, on a frozen track,
"Unfreeze" (greyed, saying why, on a track that can't be frozen, such as one with no instrument).
The track menu (right-click a Mix strip's name or an Edit track's name) has a "Freeze ▸" submenu:
"Up to `<module>`" for each module in chain order (`track.freeze` with `upto` on that track), a
separator, "Whole chain" and, on a frozen track, "Unfreeze" (both for the selection the menu acts
on, track by track: a selected track that can't take the action is skipped and the status line
says so, "Froze 2 tracks, skipped `Verb`: Freeze works on audio tracks and tracks with an
instrument: …" or "Unfroze 1 track, skipped `Bass`: it is not frozen"; a selected track frozen
up to a module moves to the whole chain, one already frozen whole is left as it is); the current
freeze point is checked. The submenu is greyed, its tooltip saying why, on a track that can't be
frozen. Each audio, MIDI and instrument strip also has a snowflake key in its record row
(I, R, ❄): a click freezes the whole chain (`track.freeze` with `freeze: true`), and on a frozen
track unfreezes it. It is lit ice-blue while the whole chain is frozen and half-lit while frozen
up to a module; its tooltip names the freeze point; greyed, saying why, on a track that can't be
frozen. The strip shows a "Frozen" badge
over the frozen modules, greyed and read-only, with a line under the freeze point; the modules
below the line are live and look and work as usual (a whole-chain freeze greys the whole chain).
The trim, and the drawers of frozen modules, are read-only too, and so is the instrument's (or a
frozen module's) plugin window, greyed with "`<track>` is frozen. Unfreeze the track to edit it";
a velocity drag in the MIDI editor on a frozen track shows that refusal in the status line. The
Edit header shows a snowflake on the name plate and greys its inserts list.

Accessible names (for scripts driving the UI): the strip's snowflake key is `<track> freeze`
(toggled while frozen), the Edit track's name plate `<track> name` and its frozen snowflake
`<track> frozen`, the strip's badge `<track> frozen` and the line under the freeze point
`<track> freeze point`, the submenu `Freeze ⏵` with items `Up to <module>`, `Whole chain` and
`Unfreeze` (a checked item reads `✔ Up to <module>` or `✔ Whole chain`).

## Split into Files

Edit › Split into Files cuts a long recording (a whole service: an hour, 16+ tracks) into one
audio file per song, in the same session.

| Command | Params | Does |
|---|---|---|
| `edit.split_into_files` | `{tracks?: [track id or name], start?, end?}` | Tracks: the named ones, else the selected tracks, else every audio, MIDI and instrument track. Range: `start`/`end` (both or neither), else the edit selection, else the whole session. Cuts every clip of the tracks at each ruler marker inside the range (point markers on the ruler; track-lane markers and stored selections are not cut points) and, for a selection, at its edges, as Edit › Separate does. Each audio piece inside the range then gets **its own source**: its window of the long file, copied sample for sample, named `<track> - <marker name or Part N>` (the marker at the start of the piece's song; `Part N` for the Nth stretch with no marker at its start; characters a file name can't hold become `-`) and written to `Audio Files/` as 32-bit float WAV on the next save, like Consolidate's. The clip keeps its place, length, gain, fades and everything else, reads its new file from sample 0 and takes its name: playback is identical. MIDI clips are cut at the same points. A long file no clip uses any more leaves the session (it stays on disk; Undo brings it back). Clips cut but kept on their long file, listed in `kept` with why: stretched or warped clips, clips with Clean Vocal applied or with a vocal tuning. A clip that already plays a whole file of its own is left alone, so running it again changes no file. One undo step. Result `{files, pieces: [{clip, source, name}], kept: [{clip, why}], cuts: [positions], segments: [{start, end, name}]}`. Refused: no audio or MIDI tracks, `end` not after `start`, only one of them. |

`engine.execute` runs it at once. From the menu (or `ui.menu.invoke`) the pieces are copied a
few per frame behind a progress window ("Splitting into Files", *n of m files*, Cancel or Esc
leaves the session untouched), then the command runs on the tracks and range planned when it
started; the status line reads "Split into 6 parts: 96 new files".

The vocal tools on a split piece work on that song only: vocal tuning analyses a clip's whole
source (now the piece; a file over 30 minutes can't be analysed, and the Vocal Editor says to
split it), while Clean Vocal, Separate Stems and the other ML tools, Detect Tempo and Scout
read only the clip's range.

## Opening a project

From the UI (menus, Recent, Revert, Close, New, templates, a dropped session file, the launch
session) `session.open` / `session.new` / `session.close` / `session.revert` return
`{opening: true}` at once and load behind the progress window ("Opening <name>…": Reading
session → Loading audio → Waveforms → Plugins → Finishing); the old session keeps playing until
Finishing swaps it, and Cancel leaves it as it was. Over the control channel (and from the CLI and
MCP) the same commands stay **synchronous**: the reply comes once the session is in place, with
the usual result (`{name, tracks, missing}` for `session.open`). While a project is opening these
are refused with `Finish or cancel the open first`: `session.new` / `session.open` /
`session.close` / `session.revert`, `file.bounce_mix`, `track.freeze` / `track.commit` /
`track.bounce`, Setup › Playback Engine › Apply and Reconnect audio device; a session dropped on the
window waits for the open. Plugins that fail to load are named in the post-open messages
("plugin not loaded: …") beside the missing media.

`file.import_dawproject {path}` (File › Import › DAWproject…) is session-replacing too: from the UI
it returns `{opening: true}` and loads behind the same progress window ("Opening <name>…": Reading
session → Loading audio (the archive's audio files) → Waveforms → Plugins → Finishing), then shows
the import report window; over the control channel it is synchronous and returns
`{name, sample_rate, tracks, report: {file, application, carried: [..], plugins: [..], items:
[{fate: placeholder|changed|dropped, track, what, why}]}, report_text}`. The imported session is
new and unsaved (`path` empty, closing it asks to save); the report is also saved in the Notes tab
(tagged `#dawproject`). `file.export_dawproject {path, replace?: false}` (File › Export ›
DAWproject…) writes the session as a `.dawproject` file (`.dawproject` is added when missing) and
returns `{path, tracks, clips, audio_files, plugin_states, notes: [what other applications won't
get exactly]}`. What maps: `crates/engine/src/dawproject/mod.rs`.

## Local ML (stems, karaoke, drum split, vocal cleanup, audio to MIDI, chords, lyrics, text to MIDI, songs)

Desktop only (not in the web app). The models run on the GPU through DirectML (any DirectX 12
card, no CUDA install) or on the CPU, never on the audio thread. They are not bundled: `cargo xtask
ml-models` exports them (PyTorch → ONNX, parity-checked) into the model folder,
`%LOCALAPPDATA%\FreeDAWm\models` (or `FREEDAWM_MODELS_DIR`), one `<id>/card.json` each, whose
sha256 is checked on first load. DeepFilterNet 3 (de-noise) is built in.

Every job takes the source: `track` (id or name; its clips rendered pre-fader through its inserts,
like Commit, from its first clip to its last) or `clip` (as it plays), default the selected clip,
else the first selected track; `device`: `auto` (default: DirectML when it works, else the CPU) |
`gpu` | `cpu`; and `models_dir` to use another model folder. Audio is resampled to each model's
rate and back; results are sample-aligned with the source and exactly as long. Mono sources run as
dual mono and give mono results. **The `ml.*` commands block their caller until the job is done**
(minutes for a long song; MCP, the CLI and the control channel get the result in the reply). In the
app, choosing one from a menu or the command palette opens its dialog instead, and the job runs on
a worker thread with a progress window and Cancel; Cancel at any point applies nothing (no tracks,
no audio, no undo step). While a job runs, commands that would edit the session (anything
undoable, Undo/Redo, Record, another job) are refused with "An ML job is running: edits wait until
it finishes or is cancelled (playback still works)"; playback, views and windows keep working.
A Clean Vocal job whose clip was moved, trimmed or re-sourced meanwhile is refused at the end (Clean Vocal's `made` holds `{clip, source (the cleaned audio), original}`). Result `{command, made: [{track?, name?, clip,
source}], notes: [where it ran, fallbacks], seconds}`. One undo step.

| Command | Params | Does |
|---|---|---|
| `ml.separate` | `{track? \| clip?, model?: "demucs" \| "sw6" \| "best", stems?: ["vocals", "drums", "bass", "other"], mute_original?: false, quality?: "fast" \| "high", device?, models_dir?}` | `model`: `demucs` (the default headless; the dialog opens on `best` when both RoFormer models are installed, else `sw6` when SW is) = HTDemucs v4, four stems; `sw6` = BS-RoFormer SW (model `bs_roformer_sw`, licence unknown, personal use): six stems `bass`, `drums`, `other`, `vocals`, `guitar`, `piano` (`stems` picks among them; `quality` is HTDemucs-only); `best` = each stem from the model measured best for it (`soundcraft_ml::pipeline::BEST_ROUTING`, numbers in its comment and the spec 2026-10-29-roformer-separation): `vocals` from Mel-Band RoFormer vocals (`vocals_melroformer`), `drums`, `bass`, `guitar`, `piano`, `other` from SW, in that track order; needs both models (an error naming the missing one otherwise); about 1 s of GPU time per second of audio; its stems add back up to the mix within about -19 dB (SW's and Mel-Band's vocals differ). HTDemucs v4: a new track `<track> <Stem>` per stem (default all four) directly under the source, its clip at the source's position; audio `<source> - <Stem>` (32-bit float WAV in `Audio Files/` when saved). A name already taken gets a number (`<track> Drums 1`). The new tracks copy the source's fader level, pan, output and sends (not its automation). `quality`: `fast` (the default headless: one pass at the card's overlap 0.25) or `high` (the dialogs' default: two passes on the input shifted by a seeded random amount up to 0.5 s, shifted back and averaged, at overlap 0.5; Demucs' `shifts` trick: fewer watery transients and spectral holes, about twice the time; the same input gives the same stems every run). |
| `ml.karaoke` | `{track? \| clip?, keep_backing?: false, mute_original?: true, quality?: "fast" \| "high" (the HTDemucs step), device?, models_dir?}` | The vocals from Mel-Band RoFormer vocals (`vocals_melroformer`; the band = source − vocals), or from HTDemucs when that model is not installed (with a note), then the Mel-Band RoFormer karaoke model on the vocals: `<track> Lead Vocal`, `<track> Instrumental` (drums + bass + other, plus the backing vocals when `keep_backing`) and, without `keep_backing`, `<track> Backing Vocals`. The karaoke model runs on DirectML (about 1.3× real time) or, much slower, on the CPU (about 0.13×). Without it installed: `Vocals` + `Instrumental`, with a note. |
| `ml.drums` | `{track? \| clip?, from_mix?: false, mute_original?: false, quality?: "fast" \| "high" (the HTDemucs step, with `from_mix`), device?, models_dir?}` | MDX23C DrumSep: `Kick`, `Snare`, `Toms`, `Hi-Hat`, `Ride`, `Crash` tracks under a drum track; with `from_mix` the drums are separated out of a mix first. |
| `ml.clean` | `{clip? \| clips? \| track? (all its audio clips), mode?: "vocal" \| "speech", mix?: 0..100, amount_db?: 24, dereverb?: true, device?, models_dir?}` | Clean Vocal, **non-destructive**. **Old params (before `mode`) still work and map like this: `{denoise: true, dereverb: true}` (the old default) → `vocal`; `{dereverb: true}` with `denoise: false` → `vocal`; `{denoise: true, dereverb: false}` → `speech` without de-reverb, keeping `amount_db`; both false → refused. So an old call that asked for de-noise + de-reverb now runs Renaissance alone, without DeepFilterNet.** `mode`: `vocal` (default; singing) runs Smule Renaissance Small alone, which is trained on singing and takes noise and reverb off together; `speech` runs DeepFilterNet 3 de-noise (`amount_db`: the attenuation limit, 100 = unlimited) and then, unless `dereverb: false`, Renaissance de-reverb. (DeepFilterNet is a speech model: on sustained sung notes it pumps; measured on a sung chorus, 36–105 gain jumps over 3 dB per 20 ms window in 45 s against 2–7 for Renaissance.) Whenever Renaissance runs, one **fixed make-up gain** follows: the cleaned audio's loudness over the voiced parts matched fully to the source's (both through an 80 Hz high-pass, mean square over 400 ms blocks with a 100 ms hop, the source's blocks above −60 dBFS and within 10 dB of their mean kept, the result measured over the same blocks), clamped to ±12 dB; the only other limit is the cleaned output's sample peak, kept at or under −1 dBFS (the make-up is lowered if needed). Renaissance also de-clips, so the cleaned peaks may sit above the source's. The same gain on every channel, never time-varying. A stereo clip runs per channel. The clip keeps its original audio and gets the cleaned audio `<clip>-clean` beside it (`Clip::clean`: the cleaned source, the original it was made from, where in the original it starts, the amount and the Original/Cleaned toggle); it plays `mix % cleaned + the rest original`, sample for sample, before its gain, fades and clip effects, in playback, bounce, freeze, Consolidate, Process and every render, and its waveform shows that mix (both sources' peaks blended by the amount). `mix` sets the clip's amount (default 100; a re-run keeps the clip's amount unless given). Re-running Clean Vocal on a cleaned clip cleans the original again and replaces the cleaned audio. A stretched (vari-speed) clip's stretched sound becomes its original `<clip>-dry` (stretch 1) with the cleaned audio beside it. Moves, trims, splits, copies, undo and save/reload keep both aligned (the cleaned audio is in the original's time; a trim opened past the cleaned part plays the original there, with a 5 ms crossfade inside the cleaned part at that edge so it doesn't click); lyrics stay in the original's time. If the cleaned file is missing when the session opens, the clip plays the original, its badge reads CLEAN missing (amber), the clip menu offers Re-run Clean Vocal… and Remove Cleaned Version, and the load reports it with the missing media. |
| `clip.clean_mix` | `{clip? \| clips?, mix?: 0..100, bypass?: bool, toggle?: bool}` | A cleaned clip's Amount (% cleaned) and Original/Cleaned toggle (`bypass: true` plays the original and keeps the amount; `toggle` flips it). Undoable; the UI merges a drag into one step. Refused on clips without cleaned audio. On the clip: the CLEAN n% / ORIGINAL badge at its top right (a click flips it) and, on its context menu, Cleaned, Amount, Commit Clean Vocal and Remove Cleaned Version. |
| `clip.clean_toggle` | `{clip? \| clips?}` | Clip › Original/Cleaned: flips the toggle. |
| `clip.clean_remove` | `{clip? \| clips?}` | Clip › Remove Cleaned Version: drops the cleaned audio; the clip plays the original. Undoable. |
| `clip.clean_commit` | `{clip? \| clips?}` | Clip › Commit Clean Vocal: renders the clip's current mix over its window plus 1 s handles each side (clamped to the original) into one new source `<clip>-clean`, moves the clip's offset and lyrics with it (it sounds the same) and drops the alternate; at 0 % or bypassed it just keeps the original. |
| `midi.cleanup` | `{clip? \| clips?, wrong?, held?, legato?, extend?: 0..100, protect_melody?: bool, remove_out_of_key?: bool, original?: bool, toggle?: bool, reset?: bool}` | MIDI Cleanup, **non-destructive**, on MIDI clips (default: the selected ones; refused with "choose a MIDI clip" otherwise). The first call gives the clip settings (`wrong` 40, `held` 70, `legato` 30, `extend` 40, `protect_melody` true, `remove_out_of_key` false; a later call changes only the keys it names; `reset: true` puts the settings back to these first). The settings are saved with the session. The cleaned notes are a **derived preview** (not saved): the engine recomputes it whenever the clip's notes or controllers, the band's chord lane over the clip, the key, the tempo or the settings change, and a command that changes none of them leaves it as it was. The preview plays and shows instead of the take; `original: true` (or `toggle: true`, which flips it) plays the untouched take, keeping the settings. The clip's own notes stay the take (piano roll, export and score use it) until Commit. Undoable; the UI merges a slider drag into one step. An amount outside 0..100 is refused ("`wrong` must be a number from 0 to 100 (%)"). Result `{clips: [{clip, settings: {wrong, held, legato, extend, protect_melody, remove_out_of_key}, original, report: {total, removed, out_of_key, held_trimmed, legato_trimmed, legato_closed, extended, pedal_changes}, summary, chords}]}`: `summary` reads "Removed 6 · trimmed 14 · extended 3 of 412 notes" (plus " · pedal changes N" when the sustain pedal was changed at chord changes); `chords` is whether any chord overlaps the clip (without one the wrong-note and held-over stages have nothing to compare with). Caveat: a loud, short (about 125 ms or less) chromatic top note over a chord that lacks it can be removed by Wrong notes, even with `protect_melody`; passing-note protection only covers steps between chord tones. A note the cleanup lengthens never runs past the next note of its pitch. `remove_out_of_key: true` removes every note whose pitch class is outside the session key (the band key; a minor key counts its raised 7th), before the other stages; Protect melody does not keep them; `report.out_of_key` counts them (they are in `removed` too). Known limitation: consolidating cleaned clips merges their takes and drops the cleanup; commit first. |
| `midi.cleanup_commit` | `{clip? \| clips?}` | Commit MIDI Cleanup: replaces the clip's notes with what plays (the cleaned notes; the take itself when switched to Original) and drops the cleanup settings. Undoable (undo brings back the take and the settings). Refused on clips with no cleanup. |
| `midi.cleanup_remove` | `{clip? \| clips?}` | Remove MIDI Cleanup: drops the clip's cleanup settings; the take plays untouched. Undoable. Refused on clips with no cleanup. |
| `track.vocal` | `{tracks?, value?: bool, auto?: true}` | Track › Vocal Track: the Vocal flag that offers a track's audio clips to the vocal tuner. `value` sets it; `auto: true` makes it follow the track name again (case-insensitive whole words: vox, vocal, vocals, voc, vo, bgv, bgvs, bv, bvs, lead, harmony, harm, choir, singer, voice, with trailing digits ignored, so `Vox2` counts and `Voxengo` doesn't; `lead` alone doesn't count beside guitar/gtr/synth/keys); with neither it toggles (on when any targeted audio track is not vocal). Only audio tracks are ever vocal: the command changes only the targeted audio tracks, leaves other kinds untouched, and refuses a target with no audio track ("choose an audio track"). Saved only when set (`Track::vocal`, absent = follow the name). Undoable. Returns `{tracks, vocal: [bool…]}`: the audio tracks acted on and each one's resulting flag. |
| `vocal.analyse` | `{clip? \| clips? \| track?}` | AI › Tune Vocal › Analyse Now: analyses the targeted audio clips now, blocking (for the CLI, control channel and MCP; the app otherwise does it in the background for clips on Vocal tracks or with a tuning, the focused clip first, then nearest the playhead). It analyses what each clip sounds like (its source with Clean Vocal blended in at the current amount); a changed amount re-analyses, and note edits move to the overlapping notes. Default targets: the selected clips, else the selected tracks' audio clips. Refused on missing or non-audio clips; a source longer than an offline render holds cannot be analysed. It then renders each clip's tuning (only the notes whose pitch or gain changed are re-rendered); playback and bounce play the render once it matches the tuning, and the heard audio until then. A phrase over 60 s without a breath cannot be rendered: that clip plays untuned and its row carries a `message`. Analyses and renders are derived (never saved, kept across Undo). Not undoable. Returns `{clips: [{clip, notes, unvoiced, message?}…]}`. |
| `vocal.edit` | `{notes: [{clip, note, offset?, centre?, drift?, vibrato?, raise?, lower?, gain_db?, formant?}], centre?, drift?, raise?, lower?: 0..100 (%), vibrato?: -100..100 (%), offset?: cents (-2400..2400) \| offset_by?: cents (-4800..4800), gain_db?: dB (-12..12), formant?: semitones (-12..12), snap_to_key?: bool, transition?: keep \| smooth \| shorten, reference?: selection \| track, from?, to?}` | Tune Vocal: sets the given fields on each note's edit (the others stay as they are); a clip gets a tuning (made against its current source) when it has none. A note is `{clip, note}`, `note` being its id from `vocal.notes` (its start frame). `centre`: how far to the target pitch; `drift`: how much slow wander is removed; `vibrato`: −100 removes it, +100 doubles it; `snap_to_key`: target the session scale at the note (the latest key change at or before it, else the band key) instead of the nearest semitone; `transition`: what the note's scoop does; `offset`: the note's manual pitch move in cents on top of Centre, Drift and Vibrato (what dragging the note up or down in the Vocal Editor sets; a Keep scoop stays as sung, Smooth and Shorten aim at the moved note, unvoiced frames are never shifted), `offset_by`: added to it (the result clamped to ±2400); a note's own values in `notes` (`offset`, `centre`, `drift`, `vibrato`, `raise`, `lower`, `gain_db`, same ranges) override the shared ones for that note (a snapped drag of several notes sends one offset per note; the note bar moves several notes by one delta, one value per note, in one step). Sessions saved before the offset load with none; `vocal.reset` clears it. `gain_db`: the note's manual level change (±12 dB, the note bar's `level` field), a flat gain over the note's body with the levelling's 15 ms ramps, added to the levelling gain in dB; the Level lane shows the sum and `vocal.notes` reports it as `tuned_level_db`. Sessions saved before it load with 0; `vocal.macro` never sets it. `formant`: the note's own formant shift in semitones (±12, the note bar's FORMANT field), on top of the clip's (`vocal.formant`): its formants move, its pitch does not; saved (only when not 0) and part of the render; `vocal.macro` refuses it, `vocal.reset` clears it. With `raise`/`lower`, each note's levelling reference is set to the median level of the notes passed (`reference: selection`, the default) or of every note on their tracks (`track`, the UI's Select all; `from`/`to` limit it to a range) and frozen per note, so a later edit of other notes doesn't move it. Refused: a value out of range, not a number or null ("`centre` must be a number from 0 to 100 (%)", "`offset` must be a number of cents from -2400 to 2400", "`gain_db` must be a number of dB from -12 to 12"), both `offset` and `offset_by`, no field at all, an unknown clip or note, a clip not analysed yet ("analysing — try again in a moment") or that can't be ("<track> can't be analysed (missing audio, or longer than 30 min)"), a stretched clip ("tuning can't follow a stretched clip — commit the stretch first": it plays untuned; `vocal.macro`, `vocal.step`, `vocal.reset`, `vocal.split` and `vocal.join` refuse it too). Undoable (Tune Vocal Edit); the UI merges a slider drag into one step. Returns `{notes}`. |
| `vocal.macro` | `{notes, preset?: "First pass (gentle)" \| "Like Melodyne 70 %" \| "Firm", centre?, drift?, raise?, lower?: 0..100, vibrato?: -100..100, reference?, from?, to?}` | Tune Vocal: a macro as one edit: the preset's Centre, Drift, Vibrato, Raise and Lower (values given override it), always with the levelling reference set as `vocal.edit` sets it. Unknown preset names are refused, and so are `snap_to_key`, `transition`, `offset`, `offset_by` and `gain_db` (set those with `vocal.edit`). Undoable (Tune Vocal Macro). Returns `{notes}`. |
| `vocal.step` | `{track, at: position \| notes: [{clip, note}], kind: centre \| drift}` | The mixer's Centre note / Flatten drift buttons: on the note sounding at `at` on the track, `centre` = 1 − (1 − centre)·(1 − 0.40) or `drift` = 1 − (1 − drift)·(1 − 0.30), so a first press is 40 % / 30 % and repeated presses keep pulling. Refused where no note sounds ("no note there") or while the clip is being analysed. Undoable (Centre Note / Flatten Drift). Returns the note `{clip, note}`. With `notes` (the Vocal Studio's Edit All bar) every listed note steps, across tracks, in one undo step; returns `{notes}`. |
| `vocal.reset` | `{notes}` | Tune Vocal: the notes' edits back to none. A clip left with no edit at all (a note that only snaps to key keeps its edit) and no split or join drops its tuning. Undoable (Reset Tuning). Returns `{notes}`. |
| `vocal.split` | `{clip, note, at: position}` | Tune Vocal: splits the note at session time `at` (strictly inside it) in two; both halves keep its edit, the second half's id is its start frame. Saved as a segmentation change and replayed after every re-analysis. Undoable (Split Vocal Note). Returns `{notes: [first, second]}`. |
| `vocal.join` | `{clip, first, second}` | Tune Vocal: joins `second` onto `first`, which must be neighbours in that order; the joined note keeps `first`'s id and edit. Replayed after every re-analysis. Undoable (Join Vocal Notes). Returns `{note}`. |
| `vocal.notes` | `{clip? \| track?, from?, to?}` | Query: a clip's (or a track's audio clips') analysed notes, cut in each clip's note detection mode (`vocal.note_mode`; `clips: [{clip, note_mode}]` lists them), inside each clip's window, default the selected clips; `from`/`to` keep those overlapping a range. Each note: `{clip, note, start, end (session samples), scoop: {start, end} \| null, centre, target, offset, tuned_offset (cents, MIDI note × 100), level_db (as sung), tuned_level_db (with the edit's `gain_db`), edit (the note's edit or null)}`; `target` follows the edit's Snap to key; `tuned_offset` is measured from the target moved by the edit's offset's whole semitones (a note dragged up two rows is judged against the semitone two up), and the summary's `avg_off_after` likewise. Also `unvoiced: [{clip, start, end}]` and `summary` over the notes listed (`{count, start, end, avg_off_before, avg_off_after, drift_before, drift_after, level_spread_db, typical_db}`, null when none), and `messages: [{clip, message}]` for clips whose tuning plays untuned (a phrase over 60 s without a breath; omitted when none). A named clip that is not analysed yet is refused ("analysing — try again in a moment"). |
| `vocal.toggle` | `{clip? \| clips? \| track?}` | AI › Tune Vocal › Tuned / Original: flips the A/B switch of the targeted clips tuned by Tune Vocal (default: the selected clips; `track`: its tuned clips). Original plays the clip untuned (its source with Clean Vocal blended in); the edits and the cached render are kept, so Tuned plays again at once. Refused when no target is tuned ("choose a clip tuned by Tune Vocal"). Undoable (Tuned/Original). Returns `{clips: [{clip, original}…]}`. |
| `vocal.note_mode` | `{clip? \| clips? \| track?, mode?: "classic" \| "musical"}` | AI › Tune Vocal › Notes Classic / Musical, and the Tuning panel's Notes selector: how the targeted audio clips' pitch is cut into notes (default: the selected clips, else the selected tracks' audio clips; no `mode` switches them: Musical unless every one already is). `musical` (new tunings, and what an untuned clip shows): notes by syllable and settled pitch; scoops, slides, falls and wide vibrato stay inside their note as its pitch curve; each note is centred on the stability-weighted median of its pitch; the correction moves each edited note as a whole and blends the move across the glides between touching notes by how far the sung pitch has travelled (no step inside a slide); Drift and Vibrato act on the note's settled middle; a note's Transition shapes the glide into it. `classic` (tunings saved before modes existed, which keep it): every pitch held for 70 ms is a note, a short approach into a long note its scoop. The pitch track is shared, so a switch needs no new analysis; edits move onto the overlapping notes of the new cut and splits/joins replay where they still apply. Saved with the clip's tuning (Classic is not written; a tuning without the field is Classic). Undoable. Returns `{clips: [{clip, note_mode, notes?}…]}` (`notes`: the count, once analysed). |
| `vocal.commit` | `{clip? \| clips?}` | AI › Tune Vocal › Commit: waits for each clip's analysis and render, then renders the clip as it plays (tuning, with Clean Vocal blended in) over its window plus 1 s handles each side into one new source `<clip>-tuned`; the clip plays it (offset and lyrics move with it) and its tuning and Clean Vocal alternate are dropped, so it sounds the same, bit for bit. With Original on, no edit, or a stretched clip (which plays untuned) it only drops the tuning. Refused for an untuned clip and for a tuning that can't be rendered (a phrase over 60 s without a breath). A clip whose audio is missing is refused. Undoable (Commit Tuning; Undo plays the tuning again at once). Returns `{clips: [{clip, source}…]}`. |
| `vocal.remove` | `{clip? \| clips?}` | AI › Tune Vocal › Remove Tuning: drops the targeted clips' tuning (they play untuned). Refused for an untuned clip. Undoable (Remove Tuning; Undo plays the tuning again at once, from the kept render). Returns `{clips}`. |
| `vocal.formant` | `{clip? \| clips? \| track?, semitones?: -12..12 \| by?: -24..24}` | Vocal Studio formant control: the targeted audio clips' clip-wide formant shift (default: the selected clips, else the selected tracks' audio clips), independent of pitch: the resynthesis scales the spectral envelope (the cepstral envelope it keeps when pitch moves) by 2^(st/12), so +3 brightens (formants a minor third up) and −3 darkens, the pitch unchanged. Applied on voiced frames only (breaths and unvoiced sounds are left alone, as pitch shifts are); each note's own `formant` (`vocal.edit`) adds to it, the sum within ±12. Rounded to 0.1. `by` adds to the current value. A clip gets a tuning when it has none; saved with it (only when not 0), part of the render key (renders stay offline, re-rendering only the frames that change); the Tuning panel's Formant row (clip / notes) and the note bar's FORMANT field set it. Undoable (Vocal Formant). Returns `{clips: [{clip, formant}…]}`. |
| `vocal.clashes` | `{track, against?: track}` | Query: the Vocal Studio's clash markers on `track`'s notes (each note judged by the semitone it sounds on after tuning). A note clashes with a note of `against` (default: the session's lead: a listed track named Lead…, else the first listed vocal track; `against` = `track` compares with nothing) sounding with it (overlapping by 60 ms or half the shorter note) when they are a minor 2nd, major 7th or tritone apart, octaves folded; else with the band chord at its middle when it is not a chord tone and is a minor 2nd or tritone from one; else, with a key set, when it is in neither the key's scale nor the chord. The studio draws a red "!" right of each such note, its hover the `text`, and the track list's chip reads "N clash". Returns `{track, against, clashes: [{clip, note, kind: minor_second \| major_seventh \| tritone \| out_of_key, with: track \| null, text: "minor 2nd against Lead Vox (G#4)" \| "minor 2nd against the chord's G#" \| "out of key (E major)"}]}`. |
| `vocal.fit_to_lead` | `{track?, against?: track, notes?: [{clip, note}], align_timing?: false}` | AI › Tune Vocal › Fit to Lead and the studio bar's Fit to lead: each note of `track` (default: the first selected track; `notes`: only those, the studio sends the selection) that clashes as `vocal.clashes` says is moved by whole semitones (its edit's offset, as a drag) to the nearest tone within a fourth that clashes with no note of `against` sounding with it nor with the chord and is in the key: a chord tone first, then the smaller move, then down. Notes that fit already, or have nowhere to go, stay. `align_timing: true` (moving starts and ends to the lead's notes) is refused as unsupported for now. One undo step (Fit to Lead). Returns `{track, against, moved: [{clip, note, semitones}]}`. |
| `vocal.harmony` | `{source?: track, interval?: "3rd" \| "5th" \| "6th" \| "octave", direction?: "up" \| "down", stay_in_key?: true, follow_chords?: true, range?: [low, high], into?: track, name?, formant?: semitones}` | Make harmony (AI › Tune Vocal › Make Harmony…, the studio's form): a harmony part from the source's own voice (default source: the session's lead, as `vocal.clashes` picks it). Each source note (its tuned semitone) gets a harmony note: `interval` (default 3rd) `direction` (default up) counted along the key's scale when `stay_in_key` and a key is set (a diatonic 3rd: E→G#, F#→A in E major), else in fixed semitones (major 3rd, perfect 5th, major 6th); with `follow_chords` and the lead on a tone of the band chord at the note's middle, the chord tone (not the lead's own) nearest that note within the interval's reach instead (3rd up from the root is the chord's 3rd, from its 3rd its 5th, from its 5th the root above); then folded by octaves into `range` (MIDI numbers or names like `"B3"`; a note no octave of which fits keeps the nearest). Each source audio clip is copied (same audio and place) onto the destination: a new audio track `name` (default "Harmony 3rd ↑", "Harmony 5th ↓"…) right under the source with its format, input and output, marked vocal, or the existing audio track `into` (whatever the copies cover there is replaced; not the source itself). Each copy gets its own tuning: the source's edits with every note's offset moved by its harmony interval (so the lead's own inflection and corrections carry over), the clip formant `formant` (±12, default 0: the lead's voice; a few semitones sounds like another singer), rendered offline like any tuning (formant-preserving). Edit it like any part. One undo step (Make Harmony). Refused while the source is analysing, without audio clips, or for bad values. Returns `{track, name, clips, notes}`. |
| `vocal.harmony_preview` | as `vocal.harmony` (`into`, `name`, `formant` ignored) | Query: what `vocal.harmony` would make, changing nothing: `{source, name, notes: [{clip, note, start, end, from, to (MIDI), pitch (cents where the harmony sings)}]}`. The studio draws these as dashed green outlines (Preview). |
| `vocal.tolerance` | `{cents?: 5..25, fix_to?: "centre" \| "edge"}` | The Vocal Studio's Tune all settings, saved with the session (`studio_tuning`, omitted while default): notes already within ±`cents` (default 10) of their target are left alone by Tune all, and unison notes further apart than it get the ≠ marker; `fix_to` centre (default) pulls corrected notes to 0 c, edge only to ±`cents` on their own side. Undoable (Tune All Tolerance). Returns `{tolerance, fix_to}`. |
| `vocal.tune_all` | `{tracks?: [track], tolerance_cents?: 5..25, fix_to?: "centre" \| "edge", drift?: 0..100, vibrato?: -100..100}` | Tune all (the studio bar; the UI sends the shown tracks and the Tuning panel macro's drift and vibrato): every note of the tracks (default: every Vocal Studio track) further than the tolerance (default the session's) from its target gets the edit that puts its held pitch on the target, or at ±tolerance with `edge`: its Centre amount (its own movement kept), else Centre 100 % and the manual offset's part under a semitone; `drift`/`vibrato` are set on the notes it corrects. Notes inside the tolerance are not touched (no edit is added). Refused while a track is being analysed (the UI waits and runs it when the analysis lands). One undo step (Tune All) however many tracks. Returns `{tracks, tolerance, fix_to, tuned: [{track, clip, note, before, after (cents from the target)}], left_alone}`. |
| `vocal.unison` | `{tracks?: [track], tolerance_cents?}` | Query: the unison stacks among the tracks' notes (default: every Vocal Studio track): notes of two or more tracks on the same row (semitone) sounding together (overlapping 60 ms or half the shorter note), chained. Each `{note: "E4", midi, start, end, apart (cents between the highest and lowest held pitch), mismatch (apart > tolerance: the ≠ marker), notes: [{track, clip, note, pitch, offset}]}`. Returns `{tolerance, stacks, mismatches}`. |
| `vocal.match` | `{notes: [{clip, note}], to: {clip, note} \| "centre"}` | Lines up stacked unison notes (the pick list's Match <other> to <lead> and Both to centre): each note's held pitch moves onto `to`'s (its manual offset changed by the difference; `to` stays), or with `"centre"` each onto its own target (0 c). One undo step (Match Unison) across tracks. Returns `{notes: [{track, clip, note, before, after}]}`. |
| `vocal.studio` | `{editing?: track, against?: track}` | Query: the Vocal Studio's tracks (audio tracks marked vocal or holding a live tuning, top to bottom), each `{track, name, color, notes, analysing, avg_offset, status}`: `avg_offset` is the mean distance of its notes (after tuning) from the targets they are judged by, in cents (null without notes), and `status` the Mix pitch box's thresholds on it: `in tune` (≤ 10 c), `drifting` (11–17 c), `off` (> 17 c), else `analysing`, `can't analyse` or `no notes`. `lead`: the track the others are compared with when `editing` is edited (a track named Lead…, else the first other one). With `against?: track`: each track also carries `clashes` (its clash count, `vocal.clashes` against `against`, else the session's lead; the lead itself against the chord and key only) and the reply `against` (the compared track). Also returns `tolerance` (cents) and `fix_to` (`centre` \| `edge`): Tune all's settings (`vocal.tolerance`). |
| `ui.vocal_editor` | `{clip?, track?, at?: position}` | AI › Tune Vocal… and the clip menu's Tune Vocal…: opens the Vocal Editor. With a clip or track it shows that track, scrolls the Edit view so `at` (default: the clip's start, else the playhead) sits a third in, and clears the note selection. Returns `{track, at}` (or `{}` without a target). |
| `window.vocal_editor` | `{value?: bool}` | Window › Vocal Editor: shows or hides the Vocal Editor tab in the lower dock (beside the MIDI Editor). Returns `{value}`. |
| `window.sync_selection` | `{value?: bool}` | Window › Sync › Sync Selection (on by default, saved in ui.json): selecting a track or strip in either window selects it in both. Off, the Mix window keeps its own selection (seeded from the shared one), and its strip menus and shortcuts act on it. Toggles without `value`. Returns `{value}`. |
| `window.sync_scroll` | `{value?: bool}` | Window › Sync › Scroll to Selected Track (on by default): a track that alone joins the selection in one window scrolls into view in the other, without taking focus. Returns `{value}`. |
| `window.sync_folders` | `{value?: bool}` | Window › Sync › Follow Folder Collapse (on by default): a closed folder hides its members' strips in the Mix window (the folder's own strip stays). Returns `{value}`. |
| `window.sync_visibility` | `{value?: bool}` | Window › Sync › Sync Track Visibility (on by default): a track hidden in the Edit window has no strip in the Mix window. Returns `{value}`. |
| `window.vocal_studio` | `{value?: bool}` | Window › Vocal Studio: the Vocal Editor as the Vocal Studio (every vocal track in one pitch grid; spec 2026-11-03-vocal-studio-design.md), shown in the lower dock. Toggles without `value`; off returns to the plain editor (which stays shown). Returns `{value}`. |
| `ui.vocal_studio` | `{value?: bool, edit?: track \| "all", show?: track \| [tracks], hide?: track \| [tracks], all?: bool, add?: track, against?: track \| null, tolerance?: cents, fix_to?: "centre" \| "edge", tune_all?: true, pick?: n \| null}` | The Vocal Studio (on unless `value: false`; the editor's Studio button). `edit`: the editing track (its notes full colour and editable with every Vocal Editor tool; the selection is cleared, the view stays). The other listed tracks (audio tracks marked vocal or holding a tuning, `vocal.studio`) show as faint reference notes, each track in its own colour (blue, orange, purple, green… by place in the list); a double-click on one makes its track the editing track. `hide`/`show`: a track's notes (the eye; the editing track can't be hidden); `all: false` hides every reference track (the bar's All vocals). `add`: puts an audio track in the list by marking it vocal (`track.vocal`, undoable) and edits it. `against`: the track clash markers compare with (`null`: the session's lead; the bar's Against menu); the reply carries `against` and the editing track's `clashes` `[{clip, note, kind, text}]`, and each track its clash count. Unanalysed tracks are analysed in the background as for the Vocal Editor (their chip reads "analysing…"). Returns `{value, editing, all, tracks: [{track, name, editing, hidden, status}]}`. Edit All (spec 2026-11-04-vocal-studio-edit-all-design.md): `edit: "all"` makes every shown track (eye on; with a listed track soloed only the soloed ones) editable at full colour: click, drag, rubber band and keys select across tracks (a note never changes track), and the bar's Centre note / Flatten drift / Snap to scale, the Tuning panel amounts and Formant act on the selection, or with none on every shown track, one undo step each; `edit: <track>` (or a name in the list) returns to single-track editing. Clash markers then compare each track with the compared one, skipping it. `tolerance`/`fix_to` run `vocal.tolerance`; `tune_all: true` runs Tune all on the shown tracks (`{pending: true}` while one is still analysed; it runs when the analysis lands). Notes of several drawn tracks on one row at one time are drawn full height with diagonal stripes in each track's colour, a count badge and, further apart than the tolerance, an amber ≠ (single-track editing stripes a reference under an editing note the same way); a click opens a pick list (a coloured row per note with its offset: select it; Both when all are editable; for a mismatch Match <other> to <lead> and Both to centre, `vocal.match`); hover shows it read-only. `pick: n` opens the n-th stack's list (`null` closes it). Also returns `edit` ("all" or the track), `editable`, `tolerance`, `fix_to`, `tune_pending`, `stacks: [{notes, tracks, apart, mismatch}]` and each track's `chip` text ("N clash", "N ≠" or its status); `tune_all`: what Tune all returned. |
| `ui.vocal_harmony` | `{open?: bool, interval?, direction?, stay_in_key?, follow_chords?, range?: [low, high] (note numbers), formant?, into?: track \| null, preview?: bool, create?: bool}` | The Vocal Studio's Make harmony… form (turns the studio on; `open: false` closes it): sets its choices (defaults 3rd up, in key, following chords, B2–A5, formant 0, a new track, Preview on) and with `create: true` runs `vocal.harmony` from the studio's compared track (the lead) and makes the new part the editing track. Preview draws the planned notes as dashed green outlines in the roll. Returns the form's `vocal.harmony` params with `open`, `preview` and the planned `notes`, or what `vocal.harmony` returned. |
| `ui.vocal_panel` | `{value?: bool}` | The Vocal Editor's Tuning button: shows or hides the Tuning panel (toggles without `value`). Returns `{value}`. |
| `ui.vocal_preset_save` | `{name, centre?, drift?, raise?, lower?: 0..100, vibrato?: -100..100}` | The Tuning panel's Save…: writes a user tuning macro to `<preset folder>/Vocal Tuning/<name>.json` (name sanitised for the file system; an existing file of that name is replaced), listed after the factory macros. Refused without a preset folder (tests, web) or a name. Returns `{name, file}`. |
| `track.view` | `{tracks?, view: "pitch"}` | The track view "waveform + pitch" (audio tracks; offered after "waveform" in the header's view menu): on a Vocal track, each analysed clip's waveform is dimmed and a pitch line drawn over it, one stretch per note, following the tuned curve (the sung one while the clip plays Original), coloured by how far the note is off its target (teal on pitch, through amber, to red at 48 c) and as thick as it is loud; breaths and unvoiced sounds draw no line, stretched clips (which play untuned) none at all. The lane spans the visible notes ±1 semitone (at least 7), the outer whole-note names at its left edge. Hovering a line shows "A4 −22c · 0:03.1" (the note's target, its offset after tuning, its start); a double-click opens the Vocal Editor there with the note selected. A clip keeps its plain waveform with a small hint when it shows no line: "No pitch yet" (not analysed yet), "Not a vocal track" (a track without the Vocal flag), "Stretched: plays untuned". A tuned clip carries a TUNED badge in its name bar, over the line (UNTUNED while it plays the untuned audio, unlike Clean Vocal's ORIGINAL; a click runs `vocal.toggle`), and a header 112 px or taller reads "Tuned · <n> notes edited". |
| `ui.midi_editor` | `{linked?, select?: [n], select_all?, split?: true \| tick, join?, grid?: "edit" \| "off" \| "1/16" \| "1/8t" …, ghosts?: bool, ghost_tracks?: "all" \| [track], zoom?: 0.25 \| 0.5 \| 1 \| 2 \| 4, fit?, cancel?, set?: {pitch \| start \| length \| velocity \| channel: text}}` | The MIDI Editor on the clip it shows (UI command). `linked` sets its Link; `select`/`select_all` pick notes (never ghost notes); `split` cuts the selected notes at the playhead (`true`) or a tick from the clip start, `join` joins them per pitch (one undo step each); `grid` is the editor's own grid (`edit` follows the Edit window's); `ghosts` and `ghost_tracks` choose the ghost notes of other MIDI clips; `zoom` is a factor of the Edit window's zoom; `cancel` cancels a drag (Esc); `set` types into the note bar (pitch and start move the selection relatively, length, velocity and channel set every note; a position reads as the main counter shows it, "34.1.1" in Bars\|Beats, a length as bars.beats.16ths). Returns `{open, linked, clip, selected, grid, grid_label, grid_ticks, ghosts, ghost_tracks, ghost_notes, zoom_factor, dragging, hover, piano_key, note_bar}`; `ui.inspect` reports the same under `midi_editor`. |
| `ui.midi_cleanup` | `{}` | AI › MIDI Cleanup…: opens the MIDI Editor with its Cleanup panel showing (UI command, via `ui.menu.invoke`). |
| `ml.midi` | `{clip? \| clips? \| track? (all its audio clips), model?: "general", onset_threshold?, frame_threshold?, min_note_ms?, pitch_bends?: false, pedal?: true, drum_thresholds?, isolate?: "none" \| "drums" \| "piano" \| "other" \| "vocals" \| "bass", quality?: "fast" \| "high", device?, models_dir?}` | Convert to MIDI. `model`: `general` (Spotify Basic Pitch, any pitched instrument, polyphonic; the default), `piano` (Kong et al. high-resolution piano transcription, with the sustain pedal), `drums` (ADTOF: GM 35 kick, 38 snare, 47 toms, 42 hi-hat, 49 cymbals on channel 10, each hit's velocity from the source's RMS in the 30 ms after it, per class the loudest hit at 120 and the others by the square root of their level ratio; `drum_thresholds?: [kick, snare, toms, hihat, cymbals]` each 0–1 sets the five per-class thresholds, default the published 0.22/0.24/0.32/0.22/0.30 scaled by `onset_threshold`; CC BY-NC-SA 4.0, private use only) or `vocal` (openvpi GAME small: a singing voice, one note at a time). `onset_threshold` 0–1 (lower = more notes; defaults general 0.5, piano 0.3; for drums it scales the five per-drum thresholds, 0.25 = as published; vocal ignores it), `frame_threshold` 0–1 (general, 0.3), `min_note_ms` (general 127.7 = Basic Pitch's 11 frames, else 0), `pitch_bends` (general: on notes that overlap no other note, as Basic Pitch writes them; vocal: each note's fractional pitch at its start; the wheel is centred again before an unbent note and at the clip end), `pedal` (piano: CC64 127/0). `isolate` separates that stem out of the source first and transcribes the stem instead of the mix, from the best installed source (`soundcraft_ml::pipeline::isolation_sources`): `vocals` from Mel-Band RoFormer vocals, `drums` and `bass` from best mode's model (BS-RoFormer SW), `piano` from SW's piano stem, each falling back to HTDemucs (at `quality`; `piano` → its `other`) when not installed; `other` is HTDemucs' other (piano, keys, guitars). A broken install of the preferred model is an error, not a fallback. Its default follows the model: `drums` → the drums stem, `piano` → `piano`, `vocal` → `vocals`, `general` → `none` unless the source track's name says bass or vocal. Makes ONE new instrument track `<track> MIDI` directly under the source track (Subtractive Synth; piano: the synth voiced as keys, preset "Keys", since there is no built-in piano; drums: Drum Synth), output as the source, with one MIDI clip `<clip> MIDI` per source clip at the same position and length. Note times (seconds from the clip start) become ticks on the session tempo map, so they stay where they sound across tempo changes. Notes past the clip's end are dropped or cut at it. Result `made: [{track, name, clip, notes, pedals}]`. The clip exports with `file.export_midi`. The four models' cards say `recommended_ep: cpu`: `device: auto` runs them on the CPU (faster than DirectML for these). |
| `ml.chords` | `{track? \| clip?, grid?: "half_bar" \| "bar" \| "free", mode?: "replace" \| "fill", key?: "auto" \| "keep" \| "C".."B" \| 0-11, scale?: "major" \| "minor", bass?, device?, models_dir?}` | Detect Chords (BTC, large vocabulary: 12 roots × min, maj, dim, aug, min6, maj6, min7, minmaj7, maj7, 7, dim7, hdim7, sus2, sus4). Writes the chords onto the Band's chord lane at the source's position on the tempo map (and shows the Sections and Chords rulers): the source's span is cut into cells of half a bar (default, the lane's own resolution) or a bar; each cell takes the chord heard longest in it (no chord when chords cover less than half of it) and equal neighbours merge. `grid: "free"` ("As played") keeps each change at the time it was heard instead: changes closer than an eighth of a bar merge (the later chord takes the earlier time), equal neighbours merge, and each chord runs to the next change; `replace` then clears the source's span. The band plays such chords on the nearest half bars, each ending where the next begins. The Detect Chords dialog starts on As played when the session's tempo never changes (no tempo map from the recording), else on Half bars; the command's default stays `half_bar`. `mode`: `replace` (default) clears the lane's chords over those cells first; `fill` only fills where the lane is empty. Names (`Em`, `Cmaj7`, `F#m7b5`, `C/E`) become key-relative slots through `chord_match` in the band's key; when the key changes, the chords already on the lane are re-expressed in the new key so they sound the same (a chord the new key can't hold exactly refuses the job, naming its bar); `key: auto` (default) first estimates the key from the detected chords (Krumhansl-Kessler over the chord tones, by length) and sets it on the band when the lane holds no chords outside the written range; `keep` uses the band's key; a note name or 0-11 (with `scale`) sets it. `bass` guesses inversions from the low CQT bins: right on a full mix only (on a single instrument it reports that instrument's lowest voice), so it defaults on for a stereo track not named after one instrument (vocal, bass, drums, guitar, piano, keys, synth…) and off otherwise. Result `made: [{chords: [{chord, name, slot, bass, exact, at, tick, length_ticks}], key, key_set, grid, replace, segments, bass, unreadable}]`. 30 s takes about 0.1 s on the CPU or DirectML. |
| `event.tempo_detect` ("Detect Tempo...") | `{source?: "clips" \| "selection" \| "session", clips?, tracks?, start?, end?, mode?: "constant" \| "follow" \| "smooth", range?: "auto" \| "60-90" \| "80-160" \| "140-200" \| "min-max", set_bar1?: true, apply?: false, beats?, downbeats?}` | Detect Tempo. The source is the given clips, a range on the given (else selected, else all audio) tracks, or the whole session (every audio track's clips summed, no mixer processing); without parameters: the selected clips, else the time selection, else the session. At most 20 minutes. A DSP beat tracker (spectral-flux onsets, autocorrelation tempo, dynamic-programming beats after Ellis 2007, attacks refined to the sample; downbeats from bar accent, low band weighted) finds the beats; `range` limits the beat tempo (half/double; `auto` = 60-200, preferring the middle). `mode`: `follow` (default) a tempo point per bar so every detected downbeat lands on its bar line; `smooth` the same over 5-bar averaged downbeats; `constant` one tempo, the average. `apply: true` writes the map (one undo step): with `set_bar1` (default) bar 1 moves to the first downbeat (the tempo map's song start; clips keep their absolute time; earlier time shows as bars 0, -1…) and the map is rebuilt from there with the meter there; without it the map before the first downbeat is kept and the points replace the tempo events after it. `beats`/`downbeats` (samples) skip the analysis: the dialog passes back what it showed. Result `{bpm, beats, downbeats, points: [{at, bpm}], bars: [per-bar bpm], mode, tracks, start, end, applied}`. |
| `event.tempo_conform` ("Straighten") | `{beats: [samples], downbeats?, bpm?, keep_feel?: 0..1, tracks?, start?, end?, set_tempo?: true}` | Straighten: stretches the audio clips of `tracks` (else the selected, else all audio tracks; inside `start`/`end` when given) so each beat lands on the steady grid at `bpm` (default the beats\' average); `keep_feel` 0 locks to the grid, 1 leaves it, in between each beat moves that share of the way. The first downbeat stays put. Phase-coherent: the span is cut at the beats and each segment is planned once (WSOLA splice choices on the sum of all the tracks) and applied to every track, so a kit\'s mics stay sample-aligned. Each clip keeps its gain, fades and identity, gets `warp` markers (`[position in the original, position in the clip]` from the clip start) and remembers its original: `clip.remove_warp` restores the material, length and start. Then (`set_tempo`) a constant tempo with bar 1 on the first downbeat. One undo step. Result `{bpm, clips, tracks, pairs, max_drift, warnings}`: a bar stretched more than 8 % warns "large drift: … cymbals may sound stretched". At most 30 minutes. |
| `event.tempo_move` | `{tick \| at, to_tick? \| to?, bpm?}` | Moves a tempo change (the Tempo ruler's marker drag; the change at bar 1 stays put) and/or sets its BPM; a ramp moves with it. Refused when no change is at `tick`/`at`. One undo step. Result `{tick, bpm}`. |
| `event.tempo_ramp` | `{tick \| at, ramp?: bool}` | Ramp to Next (the marker's right-click menu): the tempo changes linearly per tick from this change to the next one; the tempo map's conversions integrate it exactly. Without `ramp` it toggles; the last change cannot ramp ("add a tempo change after this one to ramp to"). Saved as `ramp: true` on the tempo event. `event.tempo_remove` also takes `{tick}`. Known limit: MIDI export writes the ramp's start tempo only. |
| `ml.lyrics` | `{track? \| clip?, language?: "en" \| "auto", prompt?, quality?: "fast" \| "best", skip_silence?: false, models_dir?}` | Transcribe Lyrics (Whisper large-v3 in whisper.cpp, through `whisper-rs`; beam 5, temperature 0 without fallback, no context carried between windows, DTW word times with the large-v3 alignment heads, no VAD; words in silence (more than 30 dB under the take's loud parts) and segments holding known caption hallucinations ("transcript", "subtitle", "©"…) are dropped). Feed a vocal track or clip, not a mix. `quality`: `fast` (default: the card's `variants.fast`, large-v3 quantized to q8_0; the same transcript as f16 on the golden verse; falls back to f16 with a note if that file is missing or fails its checksum) or `best` (f16). `skip_silence` leaves silences over 2 s out before decoding (times mapped back; off by default, it did not speed up the test vocal). The words go onto the clips they were sung on (`Clip::lyrics`, in the clip's source time), so they move, nudge, trim (words outside the window hide, not lost), split, delete, undo and duplicate with the audio; a track shows its clips' visible words as a strip along the bottom of its lane. The clips' existing lines that start in the source's range are replaced. `prompt`: names or spellings to expect. Runs on the CPU (no `device`); with whisper.cpp built by clang-cl, a 4-minute vocal takes about 86 s (fast) or 107 s (best) on a 16-core laptop CPU, loading included. Cancel returns at once (whisper.cpp stops on its own thread at its next step). Result `made: [{track, lines, words, language, text: [line texts]}]`. |
| `file.export_lyrics` | `{path, track?}` | A track's lyrics as LRC (`[ti:<track>]`, then `[mm:ss.xx]` per line with `<mm:ss.xx>` per word); `track` defaults to the selected track with lyrics, else the first track with lyrics. File › Export › Lyrics (LRC)… |
| `file.export_lyrics_text` | `{path, track?}` | The same lyrics as plain text, one line per line. File › Export › Lyrics as Text… |
| `lyrics.list` / `lyrics.clear` | `{track?}` / `{track?, start?, end?}` | A track's visible lines and words (samples and seconds); remove its clips' lines (those starting in `start..end` on the timeline when given; without a range, all, hidden ones too; one undo step; also Clear Lyrics on the track's context menu). `file.export_midi` writes each track's visible lyrics as Lyric meta events (FF 05, one per word, at the words' ticks on the tempo map); an audio track with lyrics exports as a lyrics-only track `<track> Lyrics`. `file.import_midi` reads Lyric events back onto the imported track's MIDI clip (a track with lyrics and no notes still comes in); text that is not UTF-8 is read as Windows-1252. Sessions saved with the first build's track-level lyrics move them onto the clips under them when opened. |
| `notes.add` | `{text?, at?, at_playhead?, done?, index?}` | Window › Notes: a new session note (`#word` in the text becomes a tag); `at`/`at_playhead` pins it to a time (a link that locates there, a flag on the Markers ruler while open). Returns `{id, index}`. One undo step. |
| `notes.edit` | `{id, text?, done?, at? (null unpins), at_playhead?, index?}` | Changes a note: text (tags re-read), done, pin, list position (drag to reorder). Typing in the tab is one step per note (`execute_merged`). |
| `notes.delete` / `notes.list` | `{id}` / `{show?: open \| done \| all, tag?}` | Delete a note; the notes in list order with `at` as samples, seconds and the main counter. Notes are saved with the session (`notes`, omitted while empty). |
| `lyrics.sheet_set` | `{text}` or `{lines: [{id?, text}]}` | Window › Lyrics: the lyric sheet, one line per line; `[Verse 1]` lines are section headings, blank lines separate, the rest are numbered lyric lines. Lines keep stable ids across edits (unchanged lines by longest common run, an edited line keeps the id of the line it replaced), so their Lyrics ruler markers follow; a deleted line's marker goes with it (undo restores both). Setting the same text is no undo step. Saved as `lyric_sheet` (omitted while empty). |
| `lyrics.sheet_get` | `{}` | The sheet: `text`, and per line `id`, `text`, `number` (lyric lines only), `heading`, `syllables` (from the bundled CMU Pronouncing Dictionary, as usually sung: "fire" is one; the round-1 spelling heuristic for words it lacks), `rhyme` (letter shared by lines whose end words rhyme perfectly), `rhymes` (shared with another line), `rhyme_word`, `rhyming` (`[{word, start, end, kinds}]`: words that rhyme nearby, `kinds` of `perfect`, `slant`, `multi` for end words within four lines of the same block, `internal` inside a line or mid-line across adjacent lines), `stress` (`"da-DUM-da"`), `fit` (`{partner, section, flag}`: the same line of the section kind's first take, Verse 1 for Verse 2 and the first Chorus for repeats; `flag` says why it does not fit: syllables differ by two or more, a different number of stressed syllables, or the stresses fall elsewhere), `alternates` (`{count, active, versions}` when the line has some), `placed` (`{start, end}` on the Lyrics ruler). The first call loads the dictionary (about a second). |
| `lyrics.rhymes` | `{word, max?: 24}` | Rhymes for `word` from cmudict (offline), most common first: `rhymes` (perfect and multi, plain words), and `perfect`, `multi`, `slant` lists of `{word, syllables, stress}` (at most `max` each); `known` is false for a word the dictionary lacks (then the built-in list answers `rhymes`). Common words only (a WordNet word in lower case). |
| `lyrics.thesaurus` | `{word}` | WordNet 3.1 synonyms and related words of `word` (or its base form: irregular forms and the usual endings): `synonyms`, `similar` (similar to, see also), `broader`, `narrower`, `opposites`, each `[{word, syllables}]`. |
| `lyrics.define` | `{word}` | WordNet senses of `word`, most used first: `senses: [{pos, definition, examples, synonyms}]`, plus its `syllables` and `stress`. |
| `lyrics.replace_word` | `{line (sheet line id) \| number, start, end (byte range), with, word? (refused if the line has something else there)}` | Replaces one word of the sheet (one undo step); the Word Finder's click. |
| `lyrics.alt_add` | `{line \| number, text?}` | Adds an alternate version of a lyric line (a copy, or `text`) and makes it active; at most 16. The active version is the line's text, so the Lyrics ruler marker and the Teleprompter show it; editing the text edits the active version. |
| `lyrics.alt_switch` | `{line \| number, index? \| step? (+1/-1, wraps)}` | Makes another version active. |
| `lyrics.alt_delete` | `{line \| number, index? (default active)}` | Removes a version; one left is no alternates. |
| `lyrics.draft_save` | `{name?}` | Saves the whole sheet (alternates included) as a named draft with the time (`Draft N` by default; at most 50, the oldest goes). Saved in `lyric_sheet.drafts`. |
| `lyrics.draft_restore` / `lyrics.draft_delete` | `{draft (id) \| name}` | Puts a draft's lines back as the sheet (undoable; lines gone take their ruler markers with them) / deletes a draft. |
| `lyrics.drafts` | `{}` | The drafts, oldest first: `id`, `name`, `created`, `lines`, `text`. |
| `lyrics.hook_set` | `{phrase}` | The hook (title) phrase the repetition check counts (empty clears it: the session's name is the hook then); returns `lyrics.repetition`. |
| `lyrics.repetition` | `{}` | `hook`, `marked`, `count` (times the hook is sung, word by word within a line, case and punctuation aside), `hits: [{line, number, start, end}]`, `words: [{word, count}]` (the most repeated words that are not function words, two or more times). |
| `lyrics.ai_prompt` | `{lines?: [numbers], request?}` | Copy for AI: a prompt to paste into any chat with the lyrics and sections, each line's syllables, stress and rhyme letter, the rhyme scheme per section, the selected lines and the request (default: "Suggest 5 alternatives for line 7 that keep 7 syllables and rhyme with 'hill'"); returns `prompt`, `request`, `default_request`. No API call. |
| `lyrics.place` | `{line? (id) \| number?, start?, end?}` | Places a lyric line on the Lyrics ruler (View › Rulers › Lyrics) as a range marker titled "<n> <text>" from the line's current text; placing again moves or retimes it. Without a line: the next unplaced one (after the last placed). Without times: the edit selection; from a point, up to the next placed line, at most 2 bars. In the app: drop a line dragged from the Lyrics tab's gutter, or L with a range selected while the ruler is shown. |
| `lyrics.unplace` | `{line? \| number?}` | Takes a line off the ruler (default: the one at the insertion point). |
| `lyrics.place_lines` | `{count?, start?, end?, from?: "even" \| "transcribed", track?}` | "Place lines on ruler…": the next unplaced lines spread evenly over the range, or (`transcribed`) each line, in sheet order, at the run of Transcribe Lyrics' words it matches on the track (its first or second word starts the run, 60 % of its words in order); returns `placed` and `unmatched`. |
| `lyrics.select_line` | `{line? \| number?}` | Line to time: the placed line's range becomes the edit selection (to loop, punch or comp it). Returns `takes`: the take layers the comp of the vocal track (the first selected comping track, else the transcribed one, else the first) plays over that range; the ruler marker shows them too. |
| `ml.text_midi` | `{prompt, seconds?: 30, start?, continue_from?, instrument?, seed?, temperature?: 1, top_p?: 0.98, max_tokens?: 2046, device?, models_dir?}` | Generate MIDI from Text (MIDI-LLM, Llama 3.2 1B with the Anticipatory Music Transformer's MIDI tokens; 8-bit weights on ONNX Runtime, CPU by default). No source: `prompt` (required) says what to compose (style, instruments, key, meter, tempo, mood). The model writes notes as (onset, duration, instrument × pitch) in 10 ms steps, velocity 72, temperature + top-p over the MIDI tokens, each position limited to the token kind it must be; it stops at `max_tokens` (3 per note, at most 4095) or at the first onset past `seconds` (1–100). One new instrument track per General MIDI instrument in the piece, at the end of the session, named after the instrument (`Drums` for the drums): the Drum Synth on channel 10, else the Subtractive Synth (pianos voiced as keys) with a program change at the clip start; each instrument gets its own channel (1–9, 11–16 in turn) so `file.export_midi` writes a file a General MIDI player plays right. One MIDI clip each at `start` (default the edit cursor), `seconds` long, notes on the session's tempo map. `continue_from` (a MIDI clip id) starts the piece with that clip's first 170 notes, from its start, as one instrument (`instrument`: 0–127 or `"drums"`; default drums when every note is on channel 10, else the clip's first program change, else drums for a track named drum/kit/perc/beat, else piano); the model carries on from them and only what it adds is written, from the clip's start (the default `start` then). The dialog offers it for a selected MIDI clip ("Continue from the clip", on by default). `seed` (a whole number, or its text) repeats a piece with the same prompt and settings on the same device; without it a new seed is chosen and returned. About 19 tokens/s on a 4-core CPU (a full 682-note piece in about two minutes). One undo step. Result `made: [{track, name, clip, program (null for drums), drums, channel, notes}]`, `seed` (text), `prompt`, `notes` (device, counts). |
| `ml.song` | `{engine?: "yue2" \| "acestep", server?, device?, style, lyrics?, instrumental?, score?, plan?: "full" \| "melody" \| "off", seconds?: 60, seed?, bpm?, key?, planner?: true, start?}` | Generate Song: a whole song (vocals and accompaniment, 48 kHz stereo) from YuE2 or ACE-Step 1.5, run by FreeDAWm itself: each engine installs from Setup › ML Models like a model (`yue2`, `acestep`: the MIT server built for this platform, yue2.cpp's `yue-server` or acestep.cpp's `ace-server`, plus its Q8_0 GGUF models fetched from Hugging Face, about 4.4 GB each; see the release playbook, Song engines). With no `server`, the job starts that engine's server on a free port of 127.0.0.1 the first time a song needs it ("Starting …" in the progress window; `device` `cpu` adds the engine's CPU flag, else it picks the best GPU: Vulkan on Windows and Linux, Metal on macOS, the CPU when there is none), waits for it, and reuses it for later songs; it stops after 10 idle minutes, when FreeDAWm quits, or if it crashes (that song fails, FreeDAWm doesn't). A missing engine is refused with where to download it. `server` (an http address) sends the song to a yue2.cpp or acestep.cpp server the person runs instead, on another computer with a bigger GPU for example; both servers speak the same job protocol (`POST` a request, poll `/job?id=N`, fetch `&result=1`, cancel with `&cancel=1`), which is all FreeDAWm uses. Cancel in the progress window cancels the server's job. `style`: YuE2's style tags or ACE-Step's caption; `lyrics` with `[Verse]`/`[Chorus]` tags (empty: YuE2 sings none, ACE-Step's planner writes some unless `instrumental`); `seconds` 10–600 (the model may end sooner); `seed` repeats a song (YuE2 uses it for both its token and noise seeds). YuE2: `plan` is its score planning (melody and chords, melody, none) and `score` an ABC score to sing instead of planning one. ACE-Step: `bpm` (30–300), `key`, and `planner` (the 5 Hz LM plans metadata, lyrics and audio codes first, `/lm`, then `/synth` renders that plan; off renders from the caption alone). The song lands on a new audio track `<engine> Song` at `start` (default the edit cursor). With YuE2, the score it sang (ABC, returned in `score.abc`) also lands as MIDI under it: one instrument track per voice (`YuE2 Score Vocal`, `YuE2 Score Ins`) and `YuE2 Score Chords` (the chord symbols voiced around middle C with the bass below), timed from the score's own tempo and placed on the session's tempo map so they sit under the singing as written (the model's timing drifts from the score; the notes are not fitted to the audio). One undo step. Result `made: [{track, name, clip, source, seconds}, {track, name, clip, notes, voice \| chords}…]`, `seed`, `score: {abc, bpm, meter}`, `song: {bpm, key, lyrics}` (ACE-Step's plan), `request` (what reproduces the song on that server). The audio comes back as 24-bit WAV, normalised by the server. Tested end to end with both servers built for the CPU (4 cores, no GPU) on the same 30 s request: ACE-Step (0.6B planner, turbo DiT, Q8_0) in about 4 minutes; YuE2 (Q8_0, `--max-seq 8192`) in about 38 minutes (its score in under a minute, 750 semantic codes, then 32 flow-matching steps at about a minute each); a GPU is the intended home for both. The weights' licences are the servers' business: YuE2's are CC BY-NC 4.0 with a creator permission (musicians may monetise their songs; companies need a licence), ACE-Step's MIT. |
| `ml.models` | `{models_dir?}` | The model folder and each model: `id`, `name`, `job`, `status` (`installed`, `built in`, `not installed`, `broken`), `problem`, `bytes`, `license` (the card's note), `licence_status` (short: `MIT`, `Apache-2.0`, `CC BY-NC-SA 4.0 — non-commercial, personal use`, `Licence not stated by the author — personal use`; from the card's `license` field once installed), `source`. The Models window and each ML dialog show the same status. |
| `ml.models_available` | `{refresh?: false}` | What the FreeDAWm models release offers against what is installed (UI command; spec 2026-11-04-model-downloads). Fetches the release's `models.json` (`FREEDAWM_MODELS_URL` overrides `https://github.com/s0lidr0ck/FreeDAWm/releases/download/models-v1/models.json`) on the calling thread when it isn't loaded yet, or with `refresh`. Returns `manifest` (`ready`, `failed`, `loading`, `not_loaded`), `error`, `manifest_url`, `folder`, `release`, `downloading`, `queue` and `models`: per model `id`, `name`, `status`, `installed_version` (the card's `exported_utc`), `available` (in the release), `version`, `size` (zip bytes), `license`, `personal_use_only`, `source`, `manual` and `manual_note` (not in the release, e.g. Whisper at 4.75 GB, over GitHub's 2 GB asset limit), `update` (the release's version is newer), `downloading`, `progress` (`{done, total, installing}`), `queued`, `note` (the last failure). |
| `ml.download` | `{ids: [id…] \| id, confirm_personal_use?: false}` | Queues downloads from the release, one at a time on a worker, and returns `{queued, downloading}` at once (poll `ml.models_available`). Each zip goes to `<models>/.downloads/`, its size and sha256 are checked against the manifest, it is unpacked into `.<id>.staging/`, every file is checked against its card, and only then is the folder swapped in: an interrupted or bad download never leaves a model that reads as installed, and the old one stays. Refused: ids not in the release; personal-use models (no licence stated, or non-commercial) without `confirm_personal_use: true` (programmatic calls never open the confirm the window shows). Nothing downloads on its own. |
| `ml.download_cancel` | `{}` | Cancels the running download (cleaned up at its next block) and empties the queue. Returns `{cancelled: id \| null}`. |
| `ml.remove` | `{id}` | Deletes an installed model's folder (renamed aside first). Refused while a job runs or while the model downloads. |

A missing model is refused with how to install it; nothing is changed. In the app: the AI menu's
Separate Stems… / Make Karaoke… / Split Drums… / Clean Vocal… / Convert to MIDI… / Detect Chords… /
Transcribe Lyrics… / Generate MIDI from Text… / Generate Song… (AI › Clean Vocal holds Original / Cleaned, Amount…, Commit and Remove Cleaned
Version: `clip.clean_toggle`, `clip.clean_mix`, `clip.clean_commit`, `clip.clean_remove`) and the same items on
the track and clip context menus open a small dialog (UI command ids `ui.ml.separate`,
`ui.ml.karaoke`, `ui.ml.drums`, `ui.ml.clean`, `ui.ml.midi`, `ui.ml.chords`, `ui.ml.lyrics`, with
optional `track`/`clip`, and `ui.ml.text_midi` and `ui.ml.song`, which need no selection; Convert to MIDI's dialog has the model, onset threshold, shortest note,
pitch bends and pedal; Detect Chords' has the grid (half bars / bars), replace or fill, the key and
the full-mix (bass) switch; Transcribe Lyrics' has the language (English / detect) and an optional
prompt; Generate MIDI from Text's has the prompt, length, randomness and seed, and the status line
names the tracks made and the seed; Generate Song's has the model (YuE2 or ACE-Step), its server's address, the
style, lyrics, instrumental, length and seed, YuE2's score plan and an optional score to sing, and ACE-Step's
tempo, key and planner switch); Setup › ML Models… (`window.ml_models`)
lists the models, their licences and the folder, and chooses GPU/CPU (`ui.set {"ml_device": …}`).
The Convert to MIDI dialog also has **Analyse** (the model runs once, on a worker; every setting change then re-decodes the kept activations in milliseconds and redraws a mini piano roll over the source's waveform), **Play** (loops the edit selection where it overlaps the source, else the clip, with the notes on the instrument track Convert would make, soloed with the source at −12 dB; Source / Notes / Both; stops with the dialog) and **Convert** (commits the notes on screen as one undo step; `Run` without Analyse does both at once). The player is handed a copy of the session, as the Vocal Editor's preview; the document is untouched until Convert.
A running job blocks opening or closing a session, like Bounce. Headless:
`freedawm-cli ml separate|karaoke|drums|clean IN.wav OUTDIR [--device …] [--models DIR] …` (clean: `[--mode vocal|speech] [--mix PCT] [--amount DB] [--no-dereverb]`; the file gets the mix baked in, after the make-up gain),
`freedawm-cli ml midi IN.wav OUT.mid [--model general|piano|drums|vocal] [--onset X] [--frame X]
[--min-ms MS] [--bends] [--no-pedal] [--isolate none|drums|piano|other|vocals|bass] [--quality fast|high] [--bpm 120]` (a format-1 .mid at a constant tempo),
`freedawm-cli ml chords IN.wav OUT.json|OUT.lab [--bass|--no-bass]` (the detector's segments with
times and names; the bass guess defaults on for a stereo file),
`freedawm-cli ml lyrics IN.wav OUT.lrc|OUT.txt|OUT.json [--language en|auto] [--prompt TEXT] [--quality fast|best] [--skip-silence]`,
`freedawm-cli ml text-midi OUT.mid --prompt TEXT [--seconds 30] [--seed N] [--temperature 1] [--bpm 120] [--continue IN.mid [--instrument 0..127|drums]]` (every instrument track in one format-1 .mid; with `--continue` the file's notes start the piece and are written too),
`freedawm-cli ml song OUT.wav --style TEXT [--engine yue2|acestep] [--server URL] [--lyrics FILE] [--instrumental] [--seconds 60] [--seed N] [--score FILE.abc] [--plan full|melody|off] [--bpm N] [--key K] [--no-planner] [--midi OUT.mid]` (the song as 24-bit WAV at 48 kHz; YuE2's score beside it as `OUT.abc`, and as MIDI with `--midi`),
`freedawm-cli ml models`, `freedawm-cli ml bench IN.wav`. `cargo xtask ml-models --copy-from DIR`
installs models another export made (each `DIR/<id>/`, sha256 checked against its card) into the
model folder; Setup › ML Models… has Open folder. Setup › ML Models… also downloads models from the
FreeDAWm models release (`ml.download`): **Download (size)** per missing model, **Update** when the
release's version is newer, **Remove**, **Download all** (with the total), progress (MB of MB,
speed) and Cancel on the row; a personal-use model asks first, naming its licence status and source.
A job dialog whose model is missing shows **Download (size)** under "Not installed: …", with the
progress and Cancel there, and Run once it is in. When the release can't be reached the window says
so and keeps the manual route (the folder and `cargo xtask ml-models`). `cargo xtask ml-publish`
packs the installed models for that release (see `docs/release-playbook.md`).

### Vocal tuning

Tune Vocal (Melodyne-style pitch and level correction) works on audio clips with no model to
install, on every platform. A track's **Vocal flag** (`track.vocal`, Track › Vocal Track) follows its
name until the owner sets it (Vox, Lead Voc, BGV 2…; not Lead Guitar). The app analyses clips on
Vocal tracks, and any clip with a tuning, in the background: pitch track, notes and unvoiced
regions of what each clip sounds like (its source with Clean Vocal blended in at the current
amount), nearest the playhead first. It then renders each
tuned clip's edits; a slider step re-renders only the notes whose curves changed, and is heard within
about a quarter second. The status bar shows "Analysing <track>…" or "Tuning <track>…" while it
works. Analyses and renders are derived: never saved, kept across Undo, made again after a reload
(the saved edits find their notes again by their frames). Until a clip's render matches its tuning
(right after an edit, or after an Undo), playback and bounce play the clip untuned, never an older
or newer render. A tuned phrase that runs over 60 s without a breath cannot be rendered: that clip
plays untuned, and the status bar, `vocal.analyse` and `vocal.notes` say so.

Which commands block: `vocal.analyse` and `vocal.commit` wait for the targeted clips' analysis and
render (for scripts, the CLI and MCP). The note commands (`vocal.edit`, `vocal.macro`, `vocal.step`,
`vocal.reset`, `vocal.split`, `vocal.join`) and `vocal.notes` never wait: on a clip not analysed yet
they answer "analysing — try again in a moment", and on a clip that never will be (its audio is
missing from the pool, it is longer than 30 minutes, or its analysis failed) "<track> can't be
analysed (missing audio, or longer than 30 min)". `vocal.toggle` and `vocal.remove` don't need an
analysis.

Limitations: tuning plays only on clips at their recorded speed (a stretched clip plays untuned, and
the note commands refuse it). Consolidate and Process give a clip a new source, so its tuning
goes inert, as Clean Vocal's does: commit the tuning first (`vocal.commit`). Undo brings the old
source and the tuning back. Renders that write the clip as it plays into a new source, Clean
Vocal's Commit (`clip.clean_commit`) and Elastic/TCE renders, bake the current tuning into it; the
clip then plays that source and its tuning goes inert the same way.

**The Vocal Editor** (Window › Vocal Editor, AI › Tune Vocal…, the clip menu's Tune Vocal…;
`ui.vocal_editor {clip?, track?, at?}`, `window.vocal_editor`) is a lower-dock tab beside the MIDI
Editor. It shows one audio track (the one it was opened on, else the selected audio track, else the
first Vocal track) over the Edit window's visible range: the chord and key strips (read-only: set
key and chords in the timeline), a piano roll of the notes (coloured by how far each sits from its
target: teal on pitch, amber, red at 48 c and beyond; as thick as it is loud), the pitch curve (the
tuned curve in white where an edit moves it, the original as a dashed ghost), hatched unvoiced
blocks ("breath" when 150 ms or longer; never selectable), the scoops with their transition handle
(Keep / Smooth / Shorten, `vocal.edit {transition}`), and the Level lane (each note at its level
after the levelling gain, the typical level dashed). It reads the cached analyses only: a track not
analysed yet shows "Analysing <track>…" (or "<track> can't be analysed (missing audio, or longer than
30 min)" when its clips never will be; the track's pitch lane then says "Can't be analysed"), a non-vocal audio track offers "Mark as vocal"
(`track.vocal`), anything else "Pick an audio track"; a stretched clip shows "Tuning can't follow a
stretched clip". The clip the editor works on (the selected notes', else the one under the playhead)
is analysed and rendered first.

Mouse and keys in the roll: click selects a note, Shift- or Ctrl-click toggles, a drag from empty
space rubber-bands, a click on empty space or a breath clears. A drag that starts on a note moves
pitch (no timing): the note, or the selection when the note is part of it (a note outside it is
selected first, added with Shift or Ctrl). The move is in cents, 50 c per row height of pointer
travel; when a note's tuned centre comes within ±15 c of a semitone it snaps onto it (magnetic: a
long drag sticks briefly at each semitone, a tick on the key column shows while it holds); Alt turns
the snap off; nothing moves until the pointer has gone 4 px up or down, so a click never nudges
pitch. The notes follow the pointer at once, a readout by it shows the live cents and note name
("−5c · A4"), and `vocal.edit` with a per-note `offset` runs at most every 30 ms and on release,
merged into one undo step. A drag released where it began leaves no change and no undo step; Esc mid-drag cancels
the move (the notes go back, nothing to undo, the selection stays); the pointer leaving the window
keeps the last move. A moved note is coloured and labelled by its offset from the semitone it was
moved to (its bar sits at the engine's tuned centre, so a note moved two semitones reads 0 c); its
sung bar stays as a dashed ghost. With notes selected, Alt+Up/Down moves them a semitone
(`vocal.edit {offset_by}`, a step each; Alt+Shift+Up/Down stays the app's Copy Selection to
Target/New Playlist); a note cut by a clip edge shows in both clips and is selected (and edited, by a
drag too, whether or not the other clip is in view) in both. Ctrl+A (and the Select all button) selects every note of the clips the
editor shows, in view or not, without scrolling (the title line reads "Select all · N notes";
levelling then refers to the track's typical level), Ctrl+Shift+A only the notes in view, S splits the note under the
mouse at the mouse (`vocal.split`), J joins two selected neighbours (`vocal.join`), Escape clears;
Delete does nothing (no timing edits). Space over the editor plays the selected notes (from the
first selected note's start to the last one's end, then stops), or from the playhead with no
selection or with Shift; Space while playing stops. A click in the time ruler puts the playhead
there (`transport.locate`), a drag scrubs it. These keys belong to the editor while the pointer is over it
and never while a text field has focus. Wheel scrolls the rows, Shift+wheel the time, Ctrl+wheel
zooms the roll by hand, Ctrl+Shift+wheel the row height (12–40 px).

The rows are shaded by the piano (white-key rows light, black-key rows dark); with a key set, the
out-of-key rows carry a diagonal hatch over the fill. The roll's zoom is relative to the Edit
window: the header's ¼× ½× 1× 2× 4× control (`ui.json` `vocal.zoom_factor`, default 1×;
Ctrl+Alt+Plus/Minus step it) sets the roll's samples per px to the Edit window's ÷ the factor,
the roll moves with the Edit window's scroll and follows its zoom at the factor, and keeps the
first selected note (else the playhead) at the same x when its zoom changes; a hand zoom
(Ctrl+wheel) lights no factor until one is clicked again.

The note bar sits under the selected note (the first selected when several; it follows
scrolling and zoom and hides when that note is off screen): `cents`, `semi`, `drift`, `vibrato`
and `level`. Drag a value sideways (1 c, 0.1 st, 1 %, 1 %, 0.1 dB a pixel; Shift fine ×0.1) or
double-click to type (Enter sets, Esc cancels); each gesture is one undo step (`vocal.edit` at
most every 30 ms and on release; `semi` sets `offset` in whole semitones, `level` sets
`gain_db`); Esc mid-drag puts the values back. With several notes the bar shows their mean and
"N notes", and a drag moves every note by the same delta (one value per note in one call).

The header's Preview toggle (`ui.json` `vocal.preview`, on by default): a clicked note, or one
whose parameters change (a drag, the note bar, the side sliders, Snap to key, a macro, Reset, a
transition, Alt+Up/Down), plays once from its start to its end through the track's normal path
with the current edits (the player is handed a copy of the session with only that track soloed;
the document is untouched), waiting up to 2 s for the clip's tuned render, and the playhead and
edit selection come back afterwards; a new gesture restarts it; nothing while the transport
plays or records on its own; Tuned/Original is respected.

The Tuning panel (the header's Tuning button, `ui.vocal_panel`): Tuned / Original (`vocal.toggle` on
the tuned clips holding a selected note, else all the track's tuned clips), the selection summary
(and "moved +200c" with a × that clears it, `vocal.edit {offset: 0}`, when a selected note was
dragged; "moved mixed (avg …)" when they differ), the Centre, Drift, Vibrato (bipolar), Raise quiet and Lower loud sliders (a drag sends `vocal.edit`
at most every 30 ms, merged into one undo step even across clips; differing notes show "mixed (avg
35%)"), Snap to key (`vocal.edit {snap_to_key}`; greyed out, "Set a key in the timeline first", when the
session sets no key: no key change that parses, no band chords or sections, and the band's default
key; the key strip then reads "No key set · set key & chords in the timeline, not here", the key
line "No key set", and no note is outlined as off key), the macro presets with Apply
(`vocal.macro`; with nothing selected, every note of the track) and Save… (`ui.vocal_preset_save`,
user macros in `<preset folder>/Vocal Tuning/`), the key and chord at the playhead, and Reset
(`vocal.reset`), Remove (`vocal.remove`) and Commit (`vocal.commit`) on the Tuned / Original clips.

## Module chain

Each channel has a dense, reorderable chain of modules (at most 32). A module has a stable numeric
id that survives moves and removals; automation lanes, plug-in state, editors and meters are keyed
by that id, never by position. Session format version 2 stores the chain; opening a version 1
session migrates its inserts, automation lanes and the track's lane view to module ids.
`session.inspect` lists each track's chain, in order, under its `inserts` key: one entry per module,
`{slot: position in the chain, module: id, plugin, bypass, params}` (`params` is null unless
`detail` is `"full"`).

| Command | Params | Result |
|---|---|---|
| `mix.module_add` | `{track?, plugin: id, at?: position in the chain (absent or past the end appends), params?: {id: value}}` | `{module, index}` |
| `mix.module_remove` | `{track?, module: id}` | `{index}` |
| `mix.module_move` | `{track?, module: id, to: position in the chain, to_track?}` | `{index}`; with another `to_track` the module moves there with its automation lanes (one undo step, a new id): `{module, index}` |
| `mix.module_bypass` | `{track?, module: id, value?: bool}` | `{bypass}` |
| `mix.module_param` | `{track?, module: id, param, value}` | `{value}` (clamped) |
| `mix.module_params` | `{track?, module: id, params: {id: value}, preset?: name, memory?: {key: number}}` | `{set}`; unknown ids and non-numeric values are errors. `memory` (at most 256 finite entries) (only for modules with derived parameters, such as Dist and Mod; others refuse it) replaces the module's derived-parameter memory first and the values land as a recall (no Dist knob swap): an editor's A/B or undo snapshot |
| `mix.module_state` | `{track, module: id \| "instrument", state: base64 \| null}` | `{bytes}`; not undoable and not journaled (states can be large) |
| `mix.module_preset` | `{track, module: id \| "instrument", preset: name, state?: base64, params?: {id: value}}` | `{bytes, set}`; Load Preset: a hosted plugin's preset as its state (a VST3 `.vstpreset`, a CLAP discovered preset), or an instrument's preset as its parameters (a built-in's preset, a VST3 program, clamped). A hosted preset clears the module's stored parameter values except those an automation lane drives; the running plugin loads the new state in place (and the old one on undo), and is made again only if it refuses it. Undoable; not journaled (states can be large). The plugin browser's Presets tab uses it |
| `mix.module_state_edit` | `{track?, module: id, state: base64 \| null}` | `{bytes}`; a built-in module's editor changing its stored state (CleanQ's Ceiling panel settings, Conform's bands): undoable ("Edit Module"), at most 64 KiB |
| `mix.module_guides` | `{track?, module: id, guides: [{freq, lo?, hi?, gain?, dynamic?, done?, note?, area?, hint?}] \| null}` or `{track?, module: id, template: id \| name, replace?: bool}` | `{guides: n}` (a template: also `added`, `skipped`, `full`); CleanQ's guides (`uiState/guides`) in its stored state, at most 24, sanitised; undoable. `area` (the area's name, at most 40 characters) and `hint` (the usual move, at most 80) are optional. A template (`male_vocal`, `female_vocal`, `kick_in`, `kick_out`, `snare_top`, `snare_bottom`, `rack_tom`, `floor_tom`, `overheads`, `hi_hat`, `room_mic`, `acoustic_guitar`, `electric_guitar_rhythm`, `lead_guitar`, `bass_guitar`, `piano`) places a source's typical problem areas: added to the guides there, skipping ones already there (within a semitone, or the same area over an overlapping range), or replacing them (`replace: true`); its guides propose no gain and no band is set |
| `mix.module_copy` | `{track?, module: id, to_track, at?: position in the chain}` | `{module, index}`; a new id on `to_track`, automation is not copied |
| `mix.module_knobs` | `{track?, module: id, knobs: [parameter id]}` (at most 4) | `{knobs}` |
| `mix.module_sidechain` | `{track?, module: id, source: track id or name \| null, pre_fader?: bool}` | `{sidechain: {track, pre_fader} \| null}`; the plugin must take a sidechain (`PluginInfo.sidechain`). Refused: the module's own track, a master or VCA track, an aux/folder keying an audio, instrument or MIDI track, and any routing cycle (through busses or other sidechains). A muted, inactive or deleted source gives a silent key; with delay compensation on, the key is aligned with the module's input |
| `mix.module_link` | `{track?, module: id, group: number \| null}` | `{group, members: [{track, module, role, status}]}`; the plugin must link (`PluginInfo.link`). Every module in a group, on any track, shares one value bus; producers render before consumers in the same block. Refused when a consumer would read a block late (a routing cycle, a producer later in the same chain, or one on an aux/master feeding an audio track); edits that cause one later are reported in `routing_warnings` (also `sidechain_warnings`) |
| `mix.module_reference` | `{track?, module: id, slot: 0..3, file: path \| null}` | `{track, module, slot, key, file, frames, channels, sample_rate}`; reference audio for a plugin with reference slots (`PluginInfo.references`), decoded and resampled to the session rate off the audio thread; the plugin plays it at the transport position. A missing, corrupt or too-long file is an error and changes nothing; saved with the session, missing ones reported on open |
| `mix.strip_size` | `{track? \| tracks? \| all?: bool, size: "auto" \| "wide" \| "slim"}` | `{size, tracks: n}`; `auto` follows the Mix window's global wide/slim toggle; without `track`/`tracks`/`all` the selected tracks; one undo step |
| `mix.pitch_expanded` | `{track? \| tracks?, expanded?: bool}` | `{expanded, tracks: n}`; a vocal strip's pitch box: the full Pitch panel or the compact box (flips without `expanded`); saved in the session, not an undo step |

The `mix.insert`, `mix.insert_remove`, `mix.insert_move`, `mix.insert_bypass`, `mix.insert_param`,
`mix.insert_params` and `mix.insert_state` commands remain as aliases that address a module by
chain position (`slot`, `from`, `to`) instead of by id.

Any command's result may carry `routing_warnings`: one line per sidechain the edit silenced, per
linked module it made read a block late, and per module asset (IR, reference) that could not be
remade after a sample-rate change, plus those of a session just loaded. `sidechain_warnings`
carries the same list for older clients.

`StripMeter` (the metering snapshot) carries a `modules` list of `ModuleMeter` entries, one per
module, keyed by module id.

The Mix window shows the chain as tiles with live graphs and knobs, wide (150 px) or slim (80 px).
A module opens in a 280 px in-place drawer with every control; a FireStation module opens instead a
wide drawer that is a translation of its VST editor, at the VST's size (unless `ui.mix.wide_editors`
is off). UI-only commands (use them through `engine.execute`):

| Command | Params | What it does |
|---|---|---|
| `ui.mix.drawer` | `{track, module?, tab?}` | Opens a built-in module's drawer in the Mix window, replacing the strip's open one. `track` is an id or a name; `module` is an id, or a plug-in id or short name (its first such module). Without `module` (or `null`) it folds the strip's drawer. `tab` picks a parameter group in the narrow drawer; in a wide editor, `"Presets"` opens its preset browser and other names pick that editor's own panel or page: `"More"` (Swale, Shale), `"Calibrate"` (Dist, Mod), Delay's and Reverb's page tabs, `"Lane A/B/C"` (Comp), `"Level"` / `"Reference"` / `"Deliver"` (LevelUp), `"Ceiling"` and `"Band N"` (CleanQ). The slim layouts (`ui.mix.slim_editors`) add their own names, which the full editors ignore: `"More"` opens the More section in each of them (Rez and Sprawl included) except Reverb, LevelUp and VintEngine, which have none (Reverb and LevelUp keep their tabs; Shale's slim More is its own, not the full editor's More drawer; ChamberMade's and Swale's are the plugin's own More, saved in its UI state); the many-way picker's menu opens with `"Models"` (Comp, FilterEngine, ModEngine, Amp, VintEngine), `"Source"` (Transient), `"Style"` and `"Interval"` (PitchEngine, Whammy), `"Circuit"` (DistEngine, Sat), `"Mic"` (Shale), `"Machine"` (Tape, Reverb) or `"Styles"` (Delay); the display toggle picks with `"Harmonics"` / `"Meters"` (Sat) and `"Response"` / `"Harmonics"` / `"Meters"` (Tape); Delay's six page tabs open More on that page. `"Calibrate"` works in both of Dist's layouts. |
| `ui.mix.full_editor` | `{track, module, open?: bool}` → `{track, module, open, window}` | A module's full editor in a window of its own (the ⤢ on every insert tile): a built-in's VST-faithful editor at full size, never the slim layout (`window: "editor"`), the generic parameter window of a built-in without one (`"parameters"`), a hosted plugin's own editor (`"hosted"`, else its parameter window). `open: false` closes it. Plugin windows always open wholly on screen, near where they were asked for; one bigger than the screen keeps its top-left in view at the largest size that fits, and one remembered off screen comes back. |
| `ui.mix.instrument_window` | `{track, open?: bool}` → `{track, plugin, open}` | Opens (or with `open: false` closes) an instrument track's instrument window, as double-clicking its instrument slot does: Lift's one-page editor for the built-in sampler, the parameter window of another built-in instrument, a hosted plugin's window. Refused for a track without an instrument. |
| `view.mix_section` | `{section: "pitch"}` → `{mix_views}` | View › Mix Window Views › Pitch (on by default; settings saved before it get it switched on once, `mix_views_offered` records that): toggles the PITCH section on vocal strips, under the inserts and above Sends. While a vocal track's strip is shown the row is reserved on every strip (other strips leave it blank, so Sends, I/O and faders line up); with no vocal strip shown nothing changes. The section shows the last ~3 s of the track's notes (playhead − 2.6 s to + 0.4 s; the edit cursor when stopped), the note under the playhead (else the last that ended within 0.5 s) as "G#4 −22c": its tuned offset (what plays: the sung offset while the clip is untuned or plays Original), coloured by it, and Centre note / Flatten drift, which run `vocal.step` on that note with `at` its middle (disabled with "Analysing…" while a clip in the window is not analysed yet, "Can't be analysed (missing audio, or longer than 30 min)" when it never will be, "no note", or over a stretched clip, which plays untuned and is left out of the graph). A double-click on the graph opens the Vocal Editor there. |
| `window.preferences` | `{value?: bool, tab?: "display"\|"operation"\|"editing"\|"mixing"\|"metering"}` → `{value, tab}` | Setup › Preferences…: opens (a `tab` also picks it) or toggles the window. Its controls run the menus' commands (`options.*`, `view.waveform_*`, `ui.theme`, `window.narrow_mix`, `window.sync_*`, …) |
| `ui.meter_ballistics` | `{release_db_s?: 3..120, hold_secs?: 0..1000}` → `{release_db_s, hold_secs}` | Preferences › Metering: track and master meter release (default 26 dB/s) and peak hold (default 2 s; 0 none; 1000 until a click on the meter clears it). Saved in `ui.json` |
| `view.expanded_send_a`..`view.expanded_send_j`, `view.expanded_sends_all`, `view.expanded_sends_none` | `{value?: bool}` (toggles when omitted); All/None `{}` | View › Expanded Sends (session flags `expanded_send.a`..`j`): each expanded send is drawn on every Mix strip under its slot as a mini fader with its dB (drag; Shift fine; Alt-click 0 dB; double-click opens the Send window), then pan (drag; double-click centres), PRE and M. A strip without that send leaves the rows blank so strips line up |
| `ui.mix.wide_editors` | `{on?: bool}` → `{on}` | FireStation modules open their VST-faithful editor (`true`, the default, saved with the UI settings) or the narrow drawer; without `on` it flips. Also View › Mix Window Views › Plugin Editors as in the VST. |
| `ui.mix.slim_editors` | `{on?: bool}` → `{on}` | A wide editor that has a slim layout (Comp, Transient, PitchEngine, DistEngine, Shale, Sat, Tape, Reverb, CleanQ, FilterEngine, ModEngine, Amp, ChamberMade, Delay, Swale, Twine, LevelUp and VintEngine: every FireStation editor, Manners and Sift included; and Rez and Sprawl) draws it at the narrow drawer's 280 px width instead of the VST's size: the same sections in the same order, stacked (`false` by default, saved with the UI settings; it needs `ui.mix.wide_editors`); without `on` it flips. Also View › Mix Window Views › Slim Plugin Editors. |
| `ui.mix.width.slim`, `ui.mix.width.wide`, `ui.mix.width.auto` | `{tracks?}` → `mix.strip_size`'s result | Channel width for the Mix window's selected channels (or `tracks`), one undo step; refused with nothing selected. Also the Channel width bar at the top of the Mix view, a channel's (or the Tracks list's) right-click menu and View › Channel Width. |
| `ui.mix.width.slim_all`, `ui.mix.width.wide_all`, `ui.mix.width.auto_all` | `{}` → `mix.strip_size`'s result | The same for every channel. The bar's "for Selected / All" choice is `ui.set {"mix_width_all": bool}`. |
| `view.mix_slim` | none | Toggles slim strips for every strip set to `auto` (the same toggle as `ui.set {"narrow_mix": true}`). |

## Scout

Scout measures selected strips (tracks, buses, the master) at two points: PRE (after trim and phase,
before the first module) and POST (after the last module, before the fader). Captures, the
conversation with Claude, the session goal and which answer's guides are shown are saved with the
session, in its `scout` block.

| Command | Params | Result |
|---|---|---|
| `scout.capture` | `{action: "start" \| "stop", strips?: [track id or name], loop?: bool}` | `{capture: id}`. Start asks the host to capture `strips` (default: the selected tracks, in session order; `"Master"` names the master). It reserves the id from `scout.next_id`, and the host arms the capture on the audio thread. With `loop: true` the capture is exactly the loop, once: the host arms it and plays the loop from its start in one step, and a bounded capture feeds only while playing, from the block containing the loop start. Start is refused while a capture runs, with no strips, without a loop (`"Set a loop first"`: no range has been selected yet, or the loop was cleared; Loop Playback need not be on), and for a loop over 30 s (`"Loop is longer than 30 seconds; shorten it to capture it exactly"`). Stop ends the capture (with nothing running it does nothing and returns `{capture: null}`). A capture also ends at 30 s, at the loop end, on a transport Stop and when playback reaches its end. Moving the playhead (Locate, or Play from elsewhere) keeps one capture and starts a new played range. An unbounded capture runs while the transport is stopped too, measuring silence or live input, until 30 s or a stop. Not undoable |
| `scout.store` | `{capture: id, name?, strips: [ScoutStrip], played: [{start, end}], loop_range?: {start, end}, played_truncated?: bool, created?: RFC 3339}` | `{capture, name}`. Appends a finished capture: the host calls it when the rig comes back. The default name is the marker, or a captured strip's clip, starting at the first played sample; otherwise `"Capture N"`. `played_truncated` says more ranges were played than the capture could list. Only an id started by `scout.capture` in this session and not yet stored is taken; any other is refused with `"That capture belongs to another session or was never started"`. Undoable ("Scout Capture") |
| `scout.rename` | `{capture: id, name}` | `{capture, name}` (undoable) |
| `scout.goal` | `{text}` | `{goal}` (undoable; the panel merges typing into one step) |
| `scout.delete` | `{capture: id}` | `{capture}` (undoable) |
| `scout.answer` | `{turn: {question, captures: [id], answer, guides?: [guide], notes?: [text], error?, model?}}` | `{turn: id}`. Appends a question and its answer with a fresh id. Undoable ("Scout Answer") |
| `scout.guides` | `{turn: id \| null}` | `{turn}`. Shows that answer's guides on Clean Q; `null` hides them (undoable) |
| `scout.list` | `{}` | The Scout state `{goal, captures, turns, guides_shown, next_id, capturing}`. Each capture carries `same_as`: the first earlier capture of the same loop range, or `null`. It is computed, not stored. `capturing` is the running capture's id, or `null` |
| `scout.starting_prompt` | `{}` | `{text, chars}`. The FireStation starting prompt (Markdown, under 40 000 characters): what FreeDAWm and Scout are, the `scout-guides` reply format, and every listed module (FireStation, Utilities, Instruments) with each parameter's id, label, range, unit and default; a parameter repeated per band is one line. Generated from the plugin registry. Not undoable (a query) |
| `scout.profile_capture` | `{from: "strip" \| "clip" \| "range" \| "file" \| "stems", name?, role?, tags?, kind?: "track" \| "mix", dir?, save?}` | `{profiles, paths}`. Measures a sound profile (Scout's measurements: spectrum, LUFS/LRA, peak/crest, dynamics, width/correlation, transients) and saves it in the library (`dir`, default `<settings folder>/profiles`, one JSON file per profile; names made unique). `strip`: `{capture, track, position?: "base" \| "post"}` copies a stored capture's numbers (the master defaults to `kind: "mix"`); `clip`: `{clip?}` (default the selected clip) as it plays; `range`: `{track?, start?, end?}` (samples; default the selection) pre-fader; `file`: `{path}` a reference audio file, measured without adding it to the session (default `kind: "mix"`); `stems`: `{track? \| clip? \| path?}` separates the song with the session's stem separation (default model) and saves `<song> · mix` plus `<song> · <stem>` per stem. Never changes the session |
| `scout.profile_list` | `{query?, role?, kind?, dir?}` | `{dir, profiles: [profile + {file}]}`, sorted by name; `query` matches every word in the name, role, tags or source |
| `scout.profile_rename` | `{name, to, dir?}` | `{name}`; refused when `to` is taken |
| `scout.profile_duplicate` | `{name, to?, dir?}` | `{name, file}` (default `<name> copy`) |
| `scout.profile_edit` | `{name, role?, tags?, kind?, dir?}` | `{profile}` |
| `scout.profile_delete` | `{name, dir?}` | `{name}` |
| `scout.profile_export` | `{name, path, dir?}` | `{path}` |
| `scout.profile_import` | `{path, dir?}` | `{name, file}`; the name is made unique |
| `scout.target_set` | `{capture, track, profile: name \| profile \| null, dir?}` | `{capture, track, target}`. Gives a captured strip a target sound profile (a copy from the library, kept in the session; a mix profile on the master is a mastering target); `null` clears it. The package then carries, on that strip's output track, `target` with the profile's measurements (`capture`) and `difference_target_minus_this` (LUFS, LRA, crest, width, correlation, tilt, centroid, transients and the 31-band spectrum shape), and the prompt asks the AI to compare and move the strip toward it. Undoable ("Set Scout Target") |

The Scout window and its packaging are UI commands (in `UI_COMMANDS`, invoked through
`engine.execute` like the other `window.*` and `ui.*` commands):

| Command | Params | Result |
|---|---|---|
| `window.scout` | `{value?: bool}` | `{value}`. Shows or hides the Scout window (*Window > Scout*); without `value` it toggles. |
| `ui.scout.package` | `{captures?: [id], question?}` | The Scout package (goal, session context, the chosen captures and the question) as JSON. `captures` defaults to the ticked captures (initially the newest); `question` defaults to the one typed in the panel. Refused with `"Choose at least one capture"` when no named capture exists. |
| `ui.scout.copy` | `{captures?: [id], question?}` | `{copied: length}`. Puts the prompt plus package on the clipboard, as the panel's Copy button does. Same defaults and refusal as `ui.scout.package`. |
| `ui.scout.send` | `{captures?: [id], question?}` | `{sending: true}`. Asks Claude with the owner's API key (native app only; the web build refuses with `"Use Copy on the web build"`); the answer streams into the panel and is stored as a `scout.answer` turn when it finishes. Refused without an API key (the panel opens its settings), while the last answer is still arriving, with an empty question, and with no capture chosen. |
| `ui.scout.starting_prompt_copy` | `{}` | `{copied: length}`. Puts the starting prompt (`scout.starting_prompt`) on the clipboard (*Scout ▸ ☰ ▸ Copy Starting Prompt*). |
| `ui.scout.starting_prompt_save` | `{path?}` | `{path, chars}`. Writes the starting prompt to `path` as Markdown; without `path` the menu item (*Save Starting Prompt…*) asks for a file, and with no dialog (or a cancelled one) returns `{path: null}`. |
| `ui.scout.import_reference` | `{path?, name?, role?, kind?, tags?}` | As `scout.profile_capture {from: "file"}` in the app's library, then shows Scout ▸ Profiles (*File > Import > Reference…*; without `path` the menu asks for a file). |
| `ui.scout.profile_import` / `ui.scout.profile_export` | `{path?}` / `{name, path?}` | The library commands with the app's folder; without `path` a file dialog asks. |
| `ui.scout.capture_stems` | `{track?, clip?, path?, name?}` | `{started: true}`. *Capture per Stem*: runs `scout.profile_capture {from: "stems"}` on a worker thread over a copy of the session; the status line reports the profiles saved. Refused while one runs. |

## Plugin browser

`window.plugin_browser` (Window > Plugin Browser, Alt+B; a UI command in `UI_COMMANDS`, invoked
through `engine.execute` like the other `window.*` commands) takes `{value?: bool}` and returns
`{value}`: it shows or hides the docked plugin browser, or toggles it without `value`. The panel
has Instruments, Effects, Presets and Library tabs (the Library: see Track presets below); a search field; a Favourites chip and category chips;
and a grid of plugin cards with a picture of the plugin's own window. Drag a card onto a track (or
a track's insert slot) to add it, or double-click it to place it on the selected track. The
Presets tab lists the selected plug-in's presets only: its built-in presets, user presets, VST3
`.vstpreset` files and VST3 programs, and CLAP discovered presets; choosing one runs the undoable
`mix.module_preset` (see the module commands above). Favourites, recent plug-ins and category
overrides are saved in the UI state, not the session.

Card pictures: the built-in and FireStation plugins ship in `crates/ui-egui/assets/browser-thumbs`
(regenerate with `cargo xtask browser-thumbs [plugin id...]`, which also rewrites
`src/browser/thumbs_table.rs`); a third-party plugin's picture is captured from its own window
(Windows) and cached.

VST3 plug-ins are scanned out of process: the helper `freedawm-scan` (`apps/soundcraft-scan`,
crate `crates/plugin-scan`) sits next to the executable and loads each bundle on its own, so a
plugin that crashes or hangs is skipped and listed as a scan failure instead of taking the app
down. Results are cached in `<config>/PluginScan/vst3.json` and rescanned incrementally.

## Band

The Band is MIDI Studio's six-musician band inside the session (`session.band`): six musician
slots, the band settings, the section definitions A–H and two lanes, the **song lane** (section
blocks) and the **chord lane**. A track whose input is a musician (`track.input {input:
"musician:N"}`) plays that musician instead of its clips.

Positions are samples (or any position form above). The lanes keep ticks, snapped to the nearest
**half bar** of the meter the band plays for the session's meter at the start (4/4, 3/4 and 6/8 as
they are; any other meter as 4/4, so a 4/4 half bar is 1920 ticks). Anything laid on a lane cuts
what it covers there: items inside go, items across an edge are trimmed, and an item it lands
inside is split around it (the tail takes a new id). Every edit is one undo step, with the label
given in the table.

The Edit window draws the lanes as the **Sections** and **Chords** rulers (`view.ruler
{ruler: "sections"|"band_chords"}`, View > Rulers). A new session hides them; `band.section.add`,
`band.chord.add`, `band.song.factory` and `band.chords.import` show both.

| Command | Params | Result |
|---|---|---|
| `band.slot` ("Band Musician") | `{slot: 1-6, enabled?, mute?, instrument?: name\|0-12, player?: name\|"Session", target?: name\|"auto", energy?: 1-4, swing?: 0-100, humanize?: 0-100, dynamics?: 0-100, fills?: 0-100, respect?, octave?: -2-2, length?: 10-100, velocity?: 1-127, speed?: half\|normal\|double, voice?: auto\|top\|2nd\|3rd\|bass, swaminst?: name\|0-12, ksoctave?: -2-2\|null, art?: [long, short, pluck, tremolo] 0-15, ample?: 0-2, groove?: 0-100, amplechords?: 0-1, pluckchords?: 0-1}` | `{slot, musician}`. The ranges are MIDI Studio's host parameters; a value out of range is refused with the range in the message. Choices take a name (any case) or an index. Instruments: Piano, Synth, Bass, Electric Keys, Harp & Mallets, Drums, Guitar, Strings, Winds, Brass, Ethnic, Beat, Folk. An instrument change keeps the player if they play the new instrument, else the Library's first player for it takes over (MIDI Studio's rule). `player: "Session"` is the instrument's first player; an unknown player, or one who doesn't play the instrument, is refused by name. A target sets the keyswitch octave to the target's default (`ksoctave` in the same call wins); `"auto"` lets the band pick the target from the instrument; an unknown target is refused by name. `ksoctave: null` = the target's default. |
| `band.settings` ("Band Settings") | `{key?: 0-11\|"C".."B" (sharps or flats), scale?: major\|minor, take?: 1-16, feel?: {natural?, tension?, bass_lock?, fills_yield?, leave_space?, dynamics?}, song_feel?: {on?, time?, density?, pocket?, groove?, touch?}, feel_preset?: name, feel_ignore?: [slot 1-6]}` | `{band}` (the whole state). Song-feel dials take a position or its name: time `normal\|half\|double`, density `medium\|sparse\|light\|busy`, pocket `on\|laidback\|pushing`, groove `straight\|swing\|shuffle\|triplet`, touch `natural\|soft\|hard`. `feel_preset` (a factory feel preset, or one of the user's) sets the five dials and turns the song feel on; `song_feel` in the same call is applied after it. `feel_ignore` lists the musicians that don't follow the song's feel (it replaces the list). |
| `band.preset.load` ("Load Band Preset") | `{name}` | `{band}`. Loads a factory or user band preset into the six slots and names the band, in one undo step. A player the Library lacks is kept by name while the instrument's first player plays (as MIDI Studio does). The song (key, scale, lanes) is untouched. |
| `band.preset.save` | `{name, replace?: false}` | `{name}`. Saves the six musicians as a user band preset (`<user folder>/Bands/<name>.json`). Refused without a user folder (and on the web build), over a factory band's name, and over an existing file unless `replace`. Not undoable. |
| `band.preset.list` | `{}` | `{presets: [{name, factory, tags}], errors}`; `errors` names the user files that were skipped, and why. |
| `band.feel_preset.save` | `{name}` | `{name}`. Saves the song-feel dials as a user feel preset in `<user folder>/FeelPresets.json` (replacing one of that name). A file that isn't a feel presets file is never written over. Not undoable. |
| `band.feel_preset.delete` | `{name}` | `{name}`; refused when there is no such user preset. Not undoable. |
| `band.def` ("Section Definition") | `{id: A-H, name?, energy_shift?: -3-3, shape?: none\|build\|drop, role?: none\|intro\|ending, take?: 0-16 (0 = the band's), follow_idea?, musicians?: [{plays?, energy?: 0-4 (0 = follow)}] (at most six, in slot order), feel?: [time, density, pocket, groove, touch] (-1 = the song's)}` | `{def}`. Creates the definition if it is missing, named as MIDI Studio names a new section (Verse, Chorus, Bridge, Intro, Outro, Pre-chorus, Solo, Tag). A section overrides density, pocket, groove and touch only: Time is the song's, so `feel[0]` must be -1 (anything else is refused). |
| `band.def.delete` ("Delete Section Definition") | `{id: A-H}` | `{id}`. Refused while a block on the song lane uses it, and for the last definition. |
| `band.section.add` ("Add Section") | `{section: A-H, start, end}` | `{block}`. The section must be defined. `end` must come after `start`; a block is at least one half bar long. |
| `band.section.set` ("Edit Section") | `{block, start?, end?, section?: A-H, energy?: -3-3, take?: 0-16}` | `{block}`. `start` alone moves the block (its length kept); `end` alone moves its end. `energy` and `take` are the block's arrangement chip (take 0 = the section's own Take). |
| `band.section.copy` ("Copy Section") | `{block, to}` | `{block}`: the copy at `to`, with the same definition, length and chip. One definition is shared by every block that names it. |
| `band.section.remove` ("Remove Section") | `{block}` | `{block}` |
| `band.chord.add` ("Add Chord") | `{at, end?, chord: "Am7"\|slot 0-111, bass?: -1-11, snap?: true}` | `{chord, slot, bass, exact}`. A chord name is matched to the nearest of MIDI Studio's 112 chord slots in the band's key and scale (`exact` says whether it is one of them); a slash chord gives its bass (a key-relative pitch class; -1 = the root), and `bass` overrides it. A name that can't be read is refused. Without `end` the chord lasts to the next chord or for one bar, whichever is shorter. Positions snap to half bars; `snap: false` keeps their exact ticks (at least an eighth of a bar long). |
| `band.chord.set` ("Edit Chord") | `{chord, at?, end?, chord_text?: name \| slot?: 0-111, bass?: -1-11, snap?: true}` | `{chord, slot, bass, exact}`; `at` alone moves the chord (its length kept). `snap: false` keeps exact ticks, as for `band.chord.add`. |
| `band.chord.remove` ("Remove Chord") | `{chord}` | `{chord}` |
| `band.song.factory` ("Factory Song") | `{name, at?: 0, replace?: true}` | `{song, blocks, chords}`. Lays one of the factory songs from `at`: its section definitions, its arrangement as blocks, and its chords. With `replace` (the default) both lanes are cleared and the definitions replaced first; without it the song's definitions replace those with the same ids, and the song cuts what it covers. It also sets the key, scale, Take, the song feel (its dials, on/off, who ignores it) and the Tension switch. Players and targets are untouched. The lanes then play exactly as MIDI Studio plays that song. |
| `band.chords.import` ("Import Chords") | `{path, at?: 0, key?: 0-11\|"C".."B", scale?: major\|minor}` | `{chords, key, scale, notes}`. Reads a MIDI file's chords onto the chord lane only (no sections, no key change), as MIDI Studio's chord import hears them. Chords are matched in `key`/`scale` (the band's by default); slots are key-relative, so another key transposes them. Native only: the web build refuses it as unsupported. |
| `band.print` ("Print Band") | `{range?: {start, end} \| block?: id \| song?: true, slots?: [1-6]}` | `{clips: [{track, clip, notes}], replaced: [{track, clip}], start, end}`. Prints what the band plays over the song (the default: from the lanes' first item to the song's end), a section `block`, or a `range` (its start snapped to a tick) into MIDI clips: for each MIDI or instrument track whose input is a musician in `slots` (default: every slot some track plays), one clip over the range (laid as `midi.new_clip` lays one; the clips it cuts or removes are listed in `replaced`), then the track's input becomes None, so it plays the printed clip. The band is rendered offline from 0 exactly as playback renders it, with the instruments named as playback names them for the "auto" targets; events inside the range are kept. Notes go on channel 1 and the organ's lower manual keeps its channel; still-sounding notes are cut at the range's end; controllers, pitch bend, pressure and program changes become controller events; channel mode messages (CC 120-127: the band's resets) are left out. Printed clips are marked `midi_channels`: they play each event on its own channel (through the instrument's raw MIDI), controllers before notes on the same tick; other clips play on channel 1 as before. One undo step. Refused with "No track plays a musician" when no track's input is one of `slots`, and with "Nothing to print in that range" when the band plays nothing there (inputs untouched). |
| `midi.snap_to_scale` ("Snap to Scale") | `{clip \| clips?, indices?: [n], key?: "E major", mode?: nearest \| up \| down \| chord}` | `{moved, indices}`. Moves each note's pitch to the nearest pitch class of the scale (ties down; `up`/`down` pick the next scale tone that way). `indices` (one clip only) limits it to those notes, default every note; the scale is `key`, else the Key ruler's key at each note's start (refused with "Set a key first" when there is none). `mode: "chord"` snaps to the chord tones of the Chords ruler at the note's start, the key where no chord sounds. Returns how many notes moved and, for one clip, where the processed notes sit after the re-sort. One undo step "Snap to Scale". |
| `band.library.reload` | `{}` | `{errors}`. Reads the Library again: the embedded players and targets, plus the user folder's. Not undoable. |
| `band.list` | `{}` | `{state, players_by_instrument: {instrument: [player]}, targets: [{slot, name, instruments}], presets: [{name, factory}], factory_songs: [name], feel_presets: [{name, factory}], library_errors, user_folder}` |

**The Studio window** is a UI command (in `UI_COMMANDS`; `engine.execute` runs it through the
app, like the other `window.*` commands), with no undo step. Everything edited inside it is a
`band.*` command above.

| Command | Params | Result |
|---|---|---|
| `window.studio` | `{value?: bool, tab?: song\|sections\|feel\|band, def?: A-H}` | `{value, tab}`. Shows or hides *Window > Studio* (without `value` it toggles), on `tab` (any case). `def` opens that section definition in the Sections tab (the lane's "Edit definition…" does this). An unknown tab or a `def` outside A-H is refused with the allowed values. |

In the Mix window, a strip whose input is a musician has a **Band** key (**B** on slim strips) that
opens that musician's drawer beside the strip; the drawer's edits are `band.slot` (and the Take
`band.settings`). The track input menu lists the six under **Musicians**.

**The user folder.** The desktop app reads players and targets from `<settings dir>/Band/Players`
and `<settings dir>/Band/Targets` (a file there replaces the embedded file of the same name), band
presets from `Band/Bands`, and feel presets from `Band/FeelPresets.json`. Runs with
`FREEDAWM_NO_PREFS`, the headless CLI and the web build have no user folder: they play the
embedded Library only, and saving presets is refused.

**Musicians as track inputs.** `track.input {input: "musician:N"}` (or `"Musician N"`, any case;
N is 1-6) makes the track play musician N; only MIDI and instrument tracks take one (any other track in the call refuses it). It is undoable like any input change. Input, output and bus names that read as a musician (`Musician N`, `musician:N`) are
reserved, so `setup.io` refuses them. A musician is
never an output: `track.output` with a musician name is refused with "A musician can only be a
track input".

An example session (lay a factory song, give a track the bass player, play):

```json
{"id": 1, "method": "engine.execute", "params": {"command": "band.song.factory", "params": {"name": "12-bar Blues"}}}
{"id": 1, "ok": true, "result": {"song": "12-bar Blues", "blocks": 1, "chords": 12}}
{"id": 2, "method": "engine.execute", "params": {"command": "track.new", "params": {"kind": "instrument", "name": "Bass"}}}
{"id": 3, "method": "engine.execute", "params": {"command": "track.input", "params": {"track": "Bass", "input": "musician:2"}}}
{"id": 3, "ok": true, "result": {"input": {"Musician": 1}}}
{"id": 4, "method": "engine.execute", "params": {"command": "transport.play", "params": {}}}
```

## MIDI note editing

The MIDI Editor's edits are `midi.*` commands on one clip's notes, addressed by **index** in the
clip's note list (`midi.notes {clip}` lists them, sorted by start, then pitch). A command that can
re-sort the notes returns where the notes it touched ended up (`indices`), so a selection can
follow them. Every command is one undo step; out-of-range indices are skipped.

| Command | Params | Result |
|---|---|---|
| `midi.notes_move` ("Move Notes") | `{clip, indices: [n], ticks?: 0, semitones?: 0}` | `{moved, indices}`. Moves the notes in time (not before the clip start) and pitch (within 0-127). `indices` is where the moved notes sit after the re-sort. |
| `midi.notes_resize` ("Resize Notes") | `{clip, indices: [n], start?: ticks, length?: ticks}` | `{indices}`. Moves the start edge of each note by `start` (the end stays; never before the clip start, never shorter than a tick) and/or its end edge by `length`. The MIDI Editor's edge drags, on every selected note. Refused without `indices`. |
| `midi.notes_add` ("Add Notes") | `{clip, notes: [{pitch, start, length, velocity?: 100, channel?: 0}]}` | `{indices}`. Adds the notes (ticks from the clip start; `start_ticks`/`length_ticks` read too) and returns where they sit after the sort. At most 100 000 at once; a note without `pitch` or `start` is refused. The Pencil and Paste. |
| `midi.notes_copy` ("Copy Notes") | `{clip, indices: [n], ticks?: 0, semitones?: 0}` | `{indices}`: the copies. Copies the notes, moved in time and pitch; the originals stay. Alt-drag and Duplicate. |
| `midi.notes_set` ("Set Notes") | `{clip, indices: [n], pitch?, start?, length?, velocity?, channel?}` | `{indices}`. Gives every picked note the same value (pitch 0-127, start ≥ 0, length ≥ 1 tick, velocity 1-127, channel 0-15; clamped). The MIDI Editor's note bar for length, velocity and channel. Refused without `indices` or with no value. |
| `midi.notes_split` ("Split Notes") | `{clip, indices: [n], at: tick}` | `{indices}`: both halves of each split note. Each picked note sounding across `at` (ticks from the clip start) becomes two. Refused when none does. The MIDI Editor's Split (at the playhead) and Alt-click with the Scissors. |
| `midi.notes_join` ("Join Notes") | `{clip, indices: [n]}` | `{indices, joined}`. The picked notes of one pitch and channel, taken in time order, become one note from the first start to the last end (the first one's velocity). Refused when no two picked notes share a pitch. The MIDI Editor's Join. |
| `midi.notes_velocity` ("Change Note Velocity") | `{clip, indices?: [n], set? \| add? \| scale? \| from?, to? \| values?: [v]}` | `{changed}`. Changes the velocity of the notes at `indices` (default every note): `set` a value, `add` an offset, `scale` by a factor (0-10), ramp `from` → `to` over the notes' starts, or give one value per index in `values` (needs `indices` of the same length; refused otherwise). Results are kept within 1-127. The notes keep their indices. Refused with no change given. |
| `event.change_velocity` ("Change Velocity...") | `{clips?, set? \| add? \| scale? \| from?, to? \| min?, max?}` | `{notes}`. The Event menu's velocity change on every note of the clips (default: the selected MIDI clips): set, add, scale, a ramp `from` → `to` over each clip's notes, or limit into `min`..`max`. From the menu it opens the Change Velocity dialog (set, add, scale, ramp), which runs `midi.notes_velocity` on the notes selected in the MIDI Editor when it shows them. |

### MIDI controller lanes

The MIDI Editor's CC, pitch bend and aftertouch lanes edit the clip's controller events (they play
on the track's instrument as recorded or imported ones do). A **lane** is `cc0`…`cc127` (also
`cc:7`), `pitchbend` (`pb`) or `aftertouch` (`pressure`); values are 0-127, pitch bend
-8192..8191 (0 = centre), clamped. Events are addressed by index in the clip's controller list
(every lane, sorted by tick); commands that can re-sort it return where their events went. Each
command is one undo step.

| Command | Params | Result |
|---|---|---|
| `midi.ctrls` ("List MIDI Controllers", query) | `{clip, lane?}` | `{events: [{index, tick, channel, lane, value}], lanes}`: the events (of one lane, or all), and the lanes the clip has events on. |
| `midi.ctrl_add` ("Add Controller Event") | `{clip, lane, tick, value, channel?: 0}` | `{index}`. Adds an event, replacing one of the same lane and channel at that tick. Refused for an unknown lane or without `tick` / `value`. |
| `midi.ctrl_edit` ("Edit Controller Event") | `{clip, index, tick?, value?}` | `{index}`. Sets one event's time and/or value. |
| `midi.ctrl_move` ("Move Controller Events") | `{clip, indices: [n], ticks?: 0, value?: 0}` | `{indices}`. Moves the events in time (not before the clip start) and offsets their values. |
| `midi.ctrl_delete` ("Delete Controller Events") | `{clip, indices: [n]}` | `{deleted}`. |
| `midi.ctrl_draw` ("Draw Controller") | `{clip, lane, points: [[tick, value]…], channel?: 0}` | `{events}`. A Pencil pass: the lane's events inside the points' span are replaced by the points (one a tick, the last wins; repeats of the same value dropped). 1 to 100 000 points. |

## Inputs, outputs and input monitoring

The session stores named **inputs** (mono or stereo, on a first device input channel) and named
**outputs** (on a first device output channel; `outputs[0]` is the main mix). Channels are 0-based
and clamped to 0..1023; a channel the open device lacks is kept and shown "(not available)". Session
format version 3 adds `inputs` and the three-state `input_monitor`; version 2 sessions load
unchanged (`input_monitor: true` → `on`, `false` → `auto`).

| Command | Params | Result |
|---|---|---|
| `setup.io` | `{action?: "list"}` | `{busses, inputs, outputs}` |
| `setup.io` | `{action: "create_input", name, format?: "mono" \| "stereo" (default mono), first_channel?: 0}` | `{busses, inputs, outputs}`. Other formats: "inputs are mono or stereo". Names are unique, case-insensitively. Reserved names are refused (see below). |
| `setup.io` | `{action: "rename_input", name, new_name}` | tracks whose input is that input follow the new name. The new name must be unique (case-insensitively) and not reserved. |
| `setup.io` | `{action: "delete_input", name}` | tracks whose input it was get no input |
| `setup.io` | `{action: "set_input_channel", name, first_channel}` | |
| `setup.io` | `{action: "set_input_format", name, format: "mono" \| "stereo"}` | Other formats, or none: "inputs are mono or stereo". Unknown input: "no input `X`". |
| `setup.io` | `{action: "create_output", name, format?: any format (default stereo), first_channel?: 0}` | Names are unique, case-insensitively; reserved names are refused. |
| `setup.io` | `{action: "rename_output", name, new_name}` | tracks routed to that output follow the new name (matched case-insensitively). The new name must be unique (case-insensitively) and not reserved; only the main may take a main name (`Main`, `Master`, `Out 1-2`). |
| `setup.io` | `{action: "delete_output", name}` | The main (`outputs[0]`) can't be deleted: "the main output can't be deleted". |
| `setup.io` | `{action: "set_output_channel", name, first_channel}` | any output, the main (`outputs[0]`) included |
| `setup.io` | `{action: "set_output_format", name, format}` | any format for an output other than the main. `format` is required. The main: "the main output's format is set with setup.main_format"; an unknown output: "no output `X`"; an unknown format: "unknown format `X`". |
| `setup.io` | `{action: "auto_inputs", channels: 1..64}` | adds mono `In 1` … `In N` on channels 0 … N-1, skipping names that exist |
| `setup.io` | `{action: "create_bus" \| "rename_bus" \| "delete_bus", name, new_name?, format?}` | as before; reserved names are refused for busses too |
| `setup.main_format` | `{format}` (2 to 16 channels) | `{format, channels, output, note}`. A default-style main name (`Out 1-N`) follows the new width unless another output already has that name; then it is kept and `note` says so (otherwise `note` is null). |
| `mix.input_monitor` | `{tracks?, value?: bool \| "auto" \| "on" \| "off"}` | `{value: "auto" \| "on" \| "off"}`. `true` is `on`, `false` is `auto`. With no value every track takes the first track's next state (auto → on → off → auto). `auto`: an armed track hears its input while stopped or recording; `on`: always; `off`: never. |
| `track.input` / `track.output` | `{tracks?, input \| output, create_bus?: true}` | `musician:N` or `Musician N` (inputs of MIDI and instrument tracks only, N 1-6; see Band). `module:<track>/<module>` (or a JSON list of them) makes the input a module's MIDI output (see **Module MIDI as a track input** below). A session input name (for `track.input`) or output name (for `track.output`) routes to that path, case-insensitively, before busses are looked up, so a named path never becomes a bus. The main output's name routes to the main mix. Legacy `In N` / `In N-M` names still resolve to device channels. |
| `options.low_latency_monitoring` | `{value?: bool}` (toggles when omitted) | `{flag, value}`. Live: while on, every insert reporting latency on a record-armed or input-monitored audio track, on the busses it feeds and on the master is bypassed in the live path, and the PDC other tracks would add to line up with them is dropped. Sends to effect returns are left alone. Takes are still placed correctly. Not an undo step. The bypassed modules show an "LLM" badge (the strip meter's `llm_bypassed` bits). Also the transport's LLM key and Options › Low Latency Monitoring. |
| `ui.audio_device` | `{system?, output?, input?, sample_rate?: 8000..768000 \| null, buffer_size?: 16..8192 \| null, list?: bool}` | `{config, device: {name, sample_rate, channels, buffer_size, silent} \| null, systems?}`. Setup › Playback Engine for agents: any field given reopens the devices with it (a string, or null/empty for the default); kept with the app preferences, per machine. `list: true` adds the audio systems and their devices. |

**Module MIDI as a track input** (spec `docs/superpowers/specs/2026-10-28-module-midi-out.md`). A
chain module that sends MIDI (`PluginInfo::midi_out`: FireStation Shale, a note per detected hit
on channel 10 at its MIDI Note, velocity from the hit, off a sixteenth later or at the next hit)
can be the input of an instrument or MIDI track: `track.input {input: "module:<track>/<module>"}`,
the track by id or name, the module by its 1-based chain position, `#<module id>`, or its
plugin's name or id (`module:Kick/Shale`, `module:Kick/1`, `module:3/#7`). A JSON list
(`["module:Kick/Shale", "module:Snare/Shale"]`) makes a set: the modules' events are merged per
block in sample order (stable by list order), each keeping its channel and note, so a kit's Shales
play one drum instrument; a single name is a set of one. Stored as `{"Modules": [{track,
module}, …]}` (`Route::Modules`), read back by `track.inspect`. Refused: an audio, aux or other
non-MIDI track ("A module's MIDI plays into a MIDI or instrument track"), as an output ("A
module's MIDI can only be a track input"), a module on the track itself or any loop through
busses, sidechains or other module inputs ("That would feed a track into itself"), a module on
an aux or folder (auxes render after the track), an unknown track or module, a module whose
plugin has no MIDI output ("X has no MIDI output"), a list holding anything but module names, an
empty list. The mix engine renders a source before the track that plays it (the same level
ordering as sidechains); a source that goes missing, inactive or into a cycle plays nothing. With
the transport stopped a monitored source still drives it. Taking a module out of the set sends
note-offs for the notes it started only; leaving the modules (or a seek) sends all notes off.
Freeze refuses a track playing a module live. Shale's MIDI Out switch gates it: off, the route
plays nothing and the strip's input button dims ("Shale's MIDI Out is off"). Hosted (VST3/CLAP)
plugins' MIDI output is not read yet. The input menus (Mix strip I/O, Edit track header) list
**MIDI from** › one checkbox per sending module (`Kick › Shale`); the input button reads `Kick ›
Shale`, or `3 modules` with the list in its tooltip.

| Command | Params | Result |
|---|---|---|
| `track.print_input` ("Print MIDI Input") | `{tracks?, range?: {start, end}}` (default: the selection) | `{clips: [{track, clip, notes}], start, end}`. Records a module MIDI input offline: the session is rendered over the range through the mix engine exactly as playback renders it and what each track's instrument was given becomes one MIDI clip per track (channels kept, `midi_channels`; the events carry the position of the block their source emitted them in), one undo step "Record". A track whose input is not modules' MIDI, or a frozen one, refuses; "Nothing to print in that range" when the modules sent nothing (MIDI Out off?). The UI does the same when a record pass ends for every record-armed instrument/MIDI track with such an input (over the pass, within the punch range; Record starts for such a track without an audio input device), and offers it as **Print to a clip** under MIDI from. |

**Reserved names.** `none`, `no input`, `no output` (and an empty name) always mean no route;
`main`, `master` and `out 1-2` always mean the main mix (case-insensitive). An input, output or bus
can't be created or renamed to one of them ("`X` is a reserved route name"), since a route could
never reach it. Sessions that already hold such names load unchanged.

Every `setup.io` action is one undo step.

Recording: arming a track whose input doesn't resolve, or needs a device input channel the device
lacks, is refused with "{track}: input “{name}” is not available on this device ({n} inputs)". A mono
input feeds both sides of a stereo track. A recording plan carries `latency` (samples, default 0):
take frame `k` is placed at timeline `start + k − latency`, so takes land earlier by the round trip
(loop passes and punch ranges are cut on that shifted timeline; a negative latency places the take later).

## Hardware MIDI input

USB MIDI keyboards and controllers and an audio interface's 5-pin MIDI In are MIDI input ports to
the OS; the desktop app opens them through `midir` (WinMM on Windows, CoreMIDI, ALSA; not in the web
build). Ports are listed and enabled in **Setup › MIDI Input Devices…** (`window.midi_inputs`):
an Enable checkbox and an activity light per port, MIDI Thru (on by default) and Rescan. The choices
are app settings in `ui.json`, not the session: `midi.inputs` (port name → enabled; a port never
chosen is enabled) and `midi.thru`. Ports are rescanned when the window opens and every 2 s: an
unplugged port closes, one plugged back in reopens if enabled.

| Command | Params | Result |
|---|---|---|
| `track.input` | `{tracks?, input: "midi:all" \| "midi:<port name>" (also "MIDI: All Inputs", "MIDI: <port>"), channel?: 0 (Omni) \| 1-16 \| "omni"}` | Instrument and MIDI tracks take live MIDI: every port, or one by name, on every channel or one. Any other track kind refuses it; it is never an output. New instrument tracks take `midi:all`. Stored as `{"Midi": {"device": "<port>", "channel": N}}` (no `device`: all inputs). Returns `{input}`. |
| `window.midi_inputs` | `{value?: bool}` | Setup › MIDI Input Devices…: shows or hides the window. Returns `{value}`. |
| `ui.musical_typing` | `{value?: bool}` | Musical Typing's computer keys (Cmd+Alt+K, Ctrl+Alt+K off macOS): A S D F G H J K L ; play the white keys from C, W E T Y U O P the black keys, Z/X an octave down/up, C/V velocity down/up; while on, those keys don't run shortcuts. Toggles without `value`. Returns `{value}`. |
| `window.midi_editor_separate` | `{value?: bool}` | Window › MIDI Editor in Separate Window (also the editor's Window/Dock button): the MIDI Editor in its own resizable window, remembered in `ui.json` (`midi_editor.separate`, `midi_editor.window`); closing that window docks it back. Toggles without `value`. Returns `{value}`. |

**Musical Typing.** Window › Musical Typing's keys and the computer keys are a port of their own,
"Musical Typing" (`track.input {input: "midi:Musical Typing"}`, or All Inputs). The window's
Play / Step switch: Play sends the keys live, Step enters notes into the selected MIDI clip at the
insertion point.

**Monitoring.** A track plays its live MIDI through its instrument when it is record-armed (with
MIDI Thru on) or its input monitoring is On (the I key; Off never), with the transport stopped
too. Every armed or monitored track whose input matches plays it. Notes, sustain (CC64), mod wheel,
pitch bend, aftertouch and program change pass through raw, channel kept. While playing, an armed
track plays its clips and the live input together. Notes held when the track stops hearing the
input are let go.

**Timing.** Each port's driver callback stamps its messages and pushes them into a preallocated
lock-free ring (one per port, full rings drop and count); the audio thread empties the rings at the
start of each block and places each message one block after its arrival (a message from the start
of the previous block at offset 0, one that just came at the block's end), so the timing within a
block is kept; older (late) messages land at offset 0.

**Recording.** Record-armed instrument and MIDI tracks on a MIDI input record what they heard into
a new MIDI clip over the record range (cut to the punch range), channels and controllers kept; loop
recording makes a clip over the loop per pass with anything in it, the first on the track's playlist
and each later one on a new playlist (comp layers are audio-only). Input Quantize
(`event.input_quantize`) snaps the notes on the session grid when on, keeping the performance to
restore. Latency: each event moves earlier by the output latency the record-offset system reads from
the device (playback − callback), the mix's own delay and one callback block (the wait above); the
audio record offset's manual and measured parts are for the input path and don't apply. One undo
step "Record" for a pass's MIDI (audio takes in the same pass are their own "Record" step).

**Chords from MIDI.** While the Chords ruler's picker is open, keys held on any MIDI input (Musical
Typing too) are named live with the Band's matcher in the band's key (the lowest note the bass:
"Am", "C/E"); letting go leaves the name; Enter or OK adds it, and for a chord played in the picker
moves on to the next chord slot.

## LAN remote: the service remote page and remote recorders

Spec: `docs/superpowers/specs/2026-10-31-lan-remote-recording-design.md`; code: `crates/net`
(`soundcraft-net`), the UI in `crates/ui-egui/src/net_ui.rs`. Desktop only.

**Setup › Network…** (`window.network`) turns it on: *Allow control from this network* (off by
default) makes the app listen on all interfaces at one port, **48100** by default (changeable):
the service remote page and its HTTP API, and a JSON-lines channel for paired PCs, told apart by
the first byte of a connection (`{` is JSON lines). The PC announces itself over mDNS as
`<PC name>._freedawm._tcp.local.` (TXT `role=booth|recorder`, `v=1`). The settings live in
`<config dir>/network.json` (never in a session): `enabled`, `port`, `pc_name`, `paired` (name,
kind, token, follows), `follow` (booth name, last address, token, at launch), `contact_timeout_s`
(7200), `landed` (remote take ids already landed), `remote_ports`, `remote_inputs` (remote audio
inputs seen). Plain traffic, no TLS (LAN only).

**Pairing.** Unpaired clients get only the pairing calls. A request makes this PC show a 4-digit
code on screen (a "Pairing request" window with Deny); typing it on the device returns a token
(128 random bits), remembered on both sides (Setup › Network lists them with Forget). A request
allows 3 wrong codes and expires after 2 minutes; at most 4 wait and 20 are made per 10 minutes.

HTTP (one request per connection; JSON bodies; the token in `X-Token` or `?token=`):

| Request | Body | Result |
|---|---|---|
| `GET /`, `/remote.js`, `/remote.css` | | The page (embedded in the app). |
| `POST /api/pair/request` | `{name}` | `{request, pc}`; this PC shows the code. 429 when rate-limited. |
| `POST /api/pair/confirm` | `{request, code}` | `{token, name, pc}`, or 403 `{error, left}`. |
| `GET /api/events?token=` | | The status feed as server-sent events: `data: <status JSON>` on every change (at most ~15 a second) and every 10 s. 401 `{pair: true}` when unpaired. |
| `GET /api/status` | | The status once. |
| `POST /api/action` | `{action: "record"}`, `{action: "play"}`, `{action: "stop", held?}`, `{action: "arm_all", value}`, `{action: "arm", track, value}`, `{action: "marker", name?}`, `{action: "rename_marker", id, name}` | `{ok, result}`; 409 `{ok: false, error}` when refused. While recording, `stop` needs `held: true` (the page sends it after a 1 s press-and-hold): a tap is refused. Nothing deletes; any other action is refused. A marker is named `Marker h:mm:ss` unless named. |

The **status** (`RigStatus`): `pc`, `session`, `transport` (`stopped|playing|recording`),
`position` (s), `bar`, `record_elapsed` (s), `tracks` (`id, name, armed, level 0..1, remote?`: the
remote PC feeding it), `markers` (`id, number, name, at`), `disk_free` (bytes),
`record_hours_left` (24-bit audio on the armed audio tracks), `remotes` (`name, state, detail,
disk_free?, last_seen?, warning`; state `connected|recording|transferring|waiting to
transfer|take received|not reachable`), `clients` (browsers watching), `warning` (a remote recorder
unreachable while recording).

JSON lines (one request per line, `{"id", "method", "params"}`; replies `{"id", "ok", "result" | "error"}`;
lines without an `id` get no reply):

| Method | Params | Result |
|---|---|---|
| `pair.request` | `{name}` | `{request, pc}` (kind `pc`) |
| `pair.confirm` | `{request, code}` | `{token, name, pc}` |
| `hello` | `{token, name, role?: "follower", report?}` | `{pc, name, transport}`. Unpaired: `{ok: false, pair: true}`. After it, **every control-protocol method** (`engine.execute`, `session.inspect`, `ui.*`…) runs as on the `--control` channel. |
| `sync.ping` | `{t0}` | `{b1, b2}`: the booth's take clock (s) when the ping arrived and when it answered. Refused while the booth is stopped. |
| `follower.report` | `{state, notes, disk_free?, queued, ports, inputs, clips, sending?}` | (notification, about once a second; `inputs`: armed audio inputs, `clips` in the take recording, `sending` 0..1 while an audio take uploads) |
| `feed` | `{port, notes: [{at, pitch, vel}]}` | (notification, every 250 ms while recording: the live sketch, booth timeline seconds, best effort) |
| `feed.audio` | `{input, peaks: [[at, peak]…], clips: [at…], level}` | (notification, every 250 ms while recording audio: a peak (0..1) every ~20 ms and the clips heard, booth timeline seconds, best effort) |
| `take.offer` | `{id, file, bytes, hash}` | `{have}`: bytes of that transfer file the booth has already (a transfer cut off resumes there). |
| `take.chunk` | `{id, offset, data}` (base64, 192 KiB) | `{have}`: appended when `offset` is where the file ends, else ignored (the sender goes back to `have`). At the end the FNV-1a `hash` is checked; a damaged file is dropped and sent again. |
| `take.transfer` | a `TakeEnvelope` | `{landed: id}` once it is in the booth's session (`again: true` when it already was). An audio take's file must have arrived first. |

A follower's link receives `{"event": "transport", "params": {state, take, anchor?, position,
looped?, session, rate, bits}}` whenever the booth's transport changes, and `{"event": "tick"}` every 5 s.
`take` is a new number (Unix milliseconds) at each Record; `anchor` is where that take sits on the
booth's timeline, in seconds: `play_from` (where its transport started: the take clock's zero),
`record_start`, `end` (set at Stop), `looped?` and `punch?` (`[start, end]`). `rate` and `bits`
(16, 24, 32 = float) are the booth project's: a remote audio take is sent at that depth and lands at
that rate.

**Remote recorders** (Setup › Network › Follow a booth: pick a booth found by name, or type its
address, pair with its code, *Follow this booth*, *Start following when FreeDAWm starts*). The
follower records, plays and stops with the booth at the booth's position (seconds on the booth's
timeline; each PC converts to its own samples). With no armed track it arms its MIDI tracks, or
makes and arms a "Piano" instrument track on all inputs (an armed audio track, as on a guitar PC,
is recorded as it is). If it connects while the booth is
already recording, it joins at the current position and its take is marked partial. During a take
it pings the booth (5 times 200 ms apart, then every 2 s); the **TakeEnvelope** it sends at Stop is
`{id: "<PC>-<take>", booth_take, pc, anchor, sync: [{local, booth, rtt}], partial, payload: {kind:
"midi", events: [{t, port, msg}]}}` (`t`: the follower's take clock, seconds, the event as played).
**Remote audio** (spec `docs/superpowers/specs/2026-11-02-remote-audio-recording-design.md`): each
armed audio track's local take (recorded at full quality, as any take) becomes its own envelope
`{id: "<PC>-<take>-a<n>", …, payload: {kind: "audio", input, rate, channels, bits, t0, file, bytes,
hash, frames, clips}}`: `input` is the track's input (the booth offers `Remote: <PC> › <input>` in
its audio tracks' input menus), `t0` the follower's take clock at the take's first frame, `clips`
where it clipped (follower take clock). Its transfer file (`file`: FLAC at the booth project's bit
depth, lossless; 32-bit float WAV when the project records float) is made on a thread in the
follower project's `Remote Takes/`, offered and sent in chunks (`take.offer`, `take.chunk`, four
ahead of the answers; offered again after a reconnect, so it resumes), then the envelope follows.
The booth keeps the file in its project's `Remote Takes/`, decodes it, converts it to the session
rate if needed, moves each sample onto its take clock through the sync points (a windowed-sinc
interpolation corrects the clock drift; within half a sample over the whole take it is copied as
it is, so a take at the session rate lands sample-exact), writes `Audio Files/<PC> <input>_NN.wav`
at the session's bit depth (all on a network thread) and lands it as a local take is placed
(`land_audio`: the booth's loop passes and punch, same playlist and comping rules, one "Record"
undo step) on every audio track whose input is `Remote: <PC> › <input>` (a new track when none).
The booth's own recorder skips those tracks (`io::is_remote_input`); a booth with only remote
tracks armed still records. The transfer file is deleted on the follower once it landed (its WAV
stays in its project).

**Remote mix stream** (`soundcraft_net::stream`, `soundcraft_playback::stream`): a follower with
Setup › Network › *Hear the booth's mix* asks for it on its link (`stream.subscribe {on}`); while at
least one does, the booth turns on a lock-free tap on its main mix after rendering (what it hears,
mute/solo/plugins/locates included) and a network thread reads it every 10 ms, converts it to
48 kHz, encodes 20 ms Opus frames (stereo, 256 kb/s, pure-Rust `rusty-opus`) and sends each as
`{"event": "stream", "params": {seq, epoch, rolled, pos, data}}`: `rolled` and `pos` are the
booth's take clock and timeline (s) at the frame's first input sample; `epoch` changes when the
booth's take clock jumps (Stop, Play, a locate), and the follower drops what it queued at once.
Nothing is sent while the booth is stopped. The follower's decoder keeps a jitter buffer (starts
at 150 ms; +30 ms each time it runs dry, up to 500; −10 ms after 20 s without; lost packets
concealed; a queue 100 ms over target drops a frame for clock drift), converts to its session rate
and queues it for its audio thread, which adds it to its main mix at the *Mix volume* (the
performer hears himself through his own track's input monitoring, no network in that path) and
marks each block with the stream frames played before it and its own take clock. Those marks give
the take's `heard` points (both ends tag their blocks with the mix's plugin delay compensation
taken off, as each take clock does; every 100 ms: his take clock → the booth's take clock of the sample he
heard, Opus's 2.5 ms lookahead taken off); an audio take with `heard` lands through them instead of
the sync points, so it lands where he played it against what he heard, however late the network
delivered it. When the booth stops, a performer still hearing the end keeps recording until the
stream reaches the booth's stop position (3 s at most); the booth keeps only what lies between its
record start and stop (or its punch). Measured: 0 samples off at 48 kHz over loopback with the
real link (rig test, also with plugin delay on either side), 0 samples over a simulated 120 ms ± 80 ms, 2 % loss link, ≤ 0.5 samples from
a 44.1 kHz booth to a 48 kHz performer whose clock runs 120 ppm fast over 250 ms ± 150 ms with 3 %
loss. Driver-reported latency is used; for a driver that reports it wrong, measure the true round
trip with the loopback measure (Setup › Playback Engine, `setup.measure_latency`, see "Record offset and loopback measure") on the performer's PC: a cable from an output to an input.

**Over the internet** (`soundcraft_net::internet`, Setup › Network › *Connect over the internet*).
Transport: iroh 1.4 (MIT OR Apache-2.0; QUIC with TLS 1.3, hole punching through home routers,
relay fallback; builds on Windows with no C toolchain: `tls-ring`, `portmapper`), on its own tokio
runtime (2 threads). Each PC has a key pair (`network.json`: `internet_key`, made once); its public
key is its address. Relays: empty `relay_url` uses n0's free public relays (rate-limited, limits
not published, documented as for development and hobby use: fine for one performer, about
300 kb/s of stream; a take upload may be slowed); a URL there uses that relay (a self-hosted
`iroh-relay` on a small VPS with UDP, never Vercel), with no code change. The LAN protocol runs
unchanged: on the host, *Let a performer join this PC over the internet* (`internet_host`) opens a
second hub listener on loopback only (`Hub::listen_internet`; no web page, and `pair.request` /
`pair.confirm` refused: nobody across the internet sees the code on this screen) and every QUIC
stream from a remote PC is bridged to it; on the performer's PC the follower connects to a local
port bridged to the host (`Internet::bridge_to`), one QUIC connection per host. Pairing: **Invite…**
makes a one-time secret valid 30 minutes (`Hub::new_invite_secret`) and shows the code
`freedawm-invite:<base64 JSON {a: the host's address (key, relay, IP addresses), s: secret, n: its
name}>` with Copy; the performer pastes it in *Join with an invite*, his PC connects and sends
`pair.invite {secret, name}` → `{token, name, pc}` (the same per-device token as the LAN, kept in
both settings; a used or expired invite is refused), then follows that host (`follow.internet`:
the host's address, JSON) and hears its mix. Both sides show the link: "Direct, 38 ms round trip"
or "Through a relay, …", loss, and a warning when it is too weak for the stream (over 400 ms,
over 5 % lost, or under 400 kb/s estimated). Tested on one machine (relay-less, the whole rig and
the stream take over it: 0 samples off) and, `#[ignore]`d, through n0's public relays from Windows
(`two_pcs_meet_through_the_public_relays`: met by key and relay alone, then went direct).

| Method | Params | Result |
|---|---|---|
| `pair.invite` | `{secret, name}` | `{token, name, pc}`; refused once used or after 30 minutes. |
| `stream.subscribe` | `{on}` | `{on}`: this follower's link hears the mix (or stops). |

**Clipping guard rails** (`soundcraft_net::guard`, `net_ui::performer_panel`): on a PC following a
booth with an audio track armed, a "Your level" window shows one big input meter (green up to
-6 dBFS, amber to -1, red above; the target range -12..-6 shaded; a 3-second peak hold), a CLIP lamp
that stays lit until clicked (a sample at -0.01 dBFS or louder, or a clip in the take), and plain
words ("Too loud: turn the knob on your interface down a little", "Too quiet: …", "Nothing is coming
in: …"). **Level check (10 s)**: he plays his loudest part; the verdict is turn down (peak above
-6 dBFS, or clipped), turn up (below -12) or good, and goes to the booth in the follower's report
(`check`, shown with the remote recorder's state). The booth sees the remote input's live level on
the receiving track's meters (Edit and Mix), a red marker on the lane at every clip (live, and on
the landed take), the clip count in the remote recorder's state, and "N clips in this take" when it
lands.
The booth maps `t` through the sync points (piecewise linear between them, the overall rate beyond;
round trips over twice the best are dropped) onto its own take clock, then onto its timeline
through `anchor`, and places the events as local MIDI recording does (`midi_record`), on every
track whose input is `midi:Remote: <PC> › <port>` (armed or not; a port no track listens to gets a
new MIDI track): one "Record" undo step. Takes wait in `<config dir>/Remote Outbox` until the booth
says they landed, and are resent on reconnect; the follower's own session keeps the take (saved
where it is, or as `<PC> – <booth session> – <date>` in `<config dir>/Remote Takes`). Booth away:
the follower keeps recording; it ends the take on the booth's Stop once back, or after
`contact_timeout_s` without contact (Setup › Network, 2 h by default).

Both take clocks are the frames each player rolled since Play (continuous across loop wraps) plus
the time since that block (`soundcraft_playback::TransportProbe`, atomics), less the output latency
and mix delay. The network runs on its own threads; nothing touches the audio thread.

| Command | Params | Result |
|---|---|---|
| `window.network` | `{value?: bool}` | Setup › Network…: shows or hides the window. |

Preview the page without a booth: `cargo run -p soundcraft-net --example remote_demo [port]`.

## Record offset and loopback measure

Takes are placed earlier than they were captured by the record offset. It is kept per machine
(the app preferences, `UiState.record_offsets`), keyed `"{system}|{device}|{buffer or default}|{rate}"`.
The device part is the output, plus ` + {input}` when the input is another device (the chosen
input, else the system default, resolved when the output opens). A `default` buffer is its own key,
apart from an explicit buffer of the same size, and a device's default buffer (WASAPI's) may change
size under it: measure again after changing the device's own settings.
Without a measurement the total is the round trip the devices report plus the manual offset plus
the mix's own delay; a loopback measurement replaces the reported part for its key. These are UI
commands (in `UI_COMMANDS`; `engine.execute` runs them through the app, like the `window.*` and
`ui.*` commands), with no undo step.

| Command | Params | Result |
|---|---|---|
| `setup.record_offset` | `{samples?: number}` | `{manual, measured, total, measured_here, key}`. Sets the current key's manual offset, clamped to ±96 000; without `samples` it only reads. A non-number: "`samples` must be a number". |
| `setup.measure_latency` | `{output?: name, input?: name}` | `{started: true}`. Plays a click out of the output's first channel (default: the main's) and records it on the input's first channel (default: channel 1). The result arrives later: the status reads "Round trip at {buffer}: {n} samples ({ms} ms)" and the value is stored for the key, or the error (no click came back, interrupted, timed out) shows and the previous value stays. Refused without a device, while one runs, while the transport plays or records, and at another device rate (checked before the input opens). |
| `setup.measure_cancel` | `{}` | `{cancelled: bool}` (false when none was running). Abandons the measurement; the previous value stays. Closing the Playback Engine window mid-measure does the same, and reopening the device forgets it. |
| `ui.reset_underruns` | `{}` | `{underruns: 0}`. Resets the underrun count shown in the Playback Engine window and the status bar (as clicking either does). |

## Panel docking

Tracks and Groups, Clips, MIDI Editor, Memory Locations, and Undo History use movable panels. Commands target the active Edit or Mix view; floating panels stay inside the current viewport. Edit and Mix retain separate arrangements; Clips and MIDI Editor belong to Edit. Transport, plug-in, and specialist utility windows retain their existing hosts. The document/session view is protected. Layouts, active tabs, panel visibility, and floating rectangles are saved with the app's existing UI preferences. These operations do not edit document contents.

Panel IDs: `tracks`, `clips`, `midi`, `memory`, `history`. The protected center is `canvas`.

`ui.dock` accepts `operation` and `panel`. Operations are `open`, `close`, `activate`, `float` (with finite positive `rect: [x, y, width, height]`), or `move` (with `target` and `zone: center|left|right|top|bottom`). Center groups with the target; side zones split beside it. `ui.dock.reset` restores the default arrangement. Invalid moves return an error without changing the layout.

Invoke these UI commands through `engine.execute`, for example `{"command":"ui.dock","params":{"operation":"float","panel":"tracks","rect":[100,100,320,440]}}`.

Advanced `ui.dock` operations use the same validated transaction as pointer gestures:
`moveFloating` updates `rect`, `resizeSplit` accepts a boolean child `path` and
`size` (`{"Ratio":0.5}`, `{"FixedFirst":280}`, or `{"FixedSecond":280}`),
`setStackOpen` takes `open`, and `resizeStack` takes a point `height` or null.
A center `move` can include `before` (panel ID or null to append) for tab order.
The equivalent `{"action": ...}` form accepts a serialized shared docking action;
this is also the form emitted by the renderer. Protected-canvas and app-specific panel
rules apply equally to both forms.

## Sticky loop

The edit selection is the loop (`docs/superpowers/specs/2026-10-28-sticky-loop.md`). Whenever a
command leaves the edit selection a non-empty range (`edit.select` with `start` and `end`, Select
All, a clip selection, Double Edit…), the loop becomes that range; a point selection (an insertion)
and `edit.select_none` leave it. The loop is saved with the session (`edit.loop`, `{start, end}`
or absent); it is not an undo step, and setting it by selecting never marks the session modified
(`loop.set` and `loop.clear` do). With Loop Playback on, Play loops the loop when the insertion is
inside it (start inclusive, end exclusive) and runs straight through from outside it; a range
selection plays as before. The main ruler shows the loop (a strip over the selection; dimmed and
dashed once deselected: clicking it selects the range again, dragging an end moves that end,
right-click › Clear Loop) and the transport reads `LOOP start – end`.

| Command | Params | What it does |
|---|---|---|
| `loop.set` | `{start?, end?}` | Sets the loop: both ends, or one (the other end stays; the selection is untouched), or none = the current edit selection, refused with `"make an edit selection first"` when that is a point. Refused when the range is empty. Returns `{start, end}`. Not undoable |
| `loop.clear` | `{}` | Forgets the loop (Edit › Clear Loop); the selection stays. Disabled with no loop |
| `loop.select` | `{}` | Edit › Select Loop Range (Ctrl+Alt+L): `edit.select` with the loop's exact range on the selected tracks (all tracks when none is selected). Refused `"no loop"` without one. Returns `{start, end}` |
| `loop.get` | `{}` | `{start, end}`, or `null` with no loop |
| `edit.select` | as before | Its result gains `loop: {start, end} \| null` |
| `file.bounce_mix` | as before | Without `start`/`end` it renders the loop when one exists, else the whole session (the edit selection is no longer used). The Bounce Mix dialog's Range choice (Loop, Between markers, Whole session) passes `start`/`end` |
| `scout.capture` | as before | `loop: true` captures the loop, whether or not Loop Playback is on |

## Comping with take layers

Each take layer is a playlist of the track; the comp is the active playlist, and each comp clip
records the layer it came from (`from_layer`, a playlist index; absent for clips from no layer, as
in older sessions). `Track.layers_open` shows the layers under the track; `Track.record_layer` is
the layer the next recording pass goes into (used once). Layer indices are playlist indices.

- `comp.swipe {track, layer, start, end, add?: false}`: that range of the layer replaces the comp
  there (silence where the layer has none), one undo step "Comp". Joins between different pieces
  get 10 ms crossfades; continuing pieces of one take are healed into one clip. `add`: only the
  parts where the comp is empty. Refused when the layer has no audio in the range or names the
  comp. Result `{clips, layer}`.
- `comp.cycle {track, start?, end?, dir: up|down}`: the comp range (default: the edit selection)
  switches to the layer above/below the one it plays that has audio there (no wrap; refused at the
  end). Result `{clips, layer}`.
- `comp.layer_new {track?, name?}`: an empty layer "Take N" (the next take colour), the record
  target, layers shown. On a track with audio and no takes yet, that audio first becomes Take 1.
- `comp.layer_delete {track, layer, remove?: false}`: refused with "Remove it from the comp first"
  while the comp plays part of it; `remove` takes its pieces out of the comp first. Later layers
  move up and the comp follows.
- `comp.layer_record_target {track, layer | null}`, `comp.layer_rename {track, layer, name}`,
  `comp.layer_make_comp {track, layer}` (the whole comp becomes that take), `comp.layers {track?,
  open?}` (show/hide; not an undo step).

Recording on a comping track (layers open, a record target, or take layers) puts each pass on a
layer (the record target, else a new one) and into the comp; loop recording makes a layer per pass
and the comp ends on the last. Other tracks record as before (a new playlist per loop pass).
`track.playlist_*` keep working.

## Track presets, deleting markers and chords, templates

**The Library.** What the user saves to use in other sessions lives in one folder, the Library:
the settings folder, or the folder `library_dir` in the UI state names (`ui.set {"library_dir":
"/path"}`, or Folder… in the browser's Library tab; empty: the settings folder). Moving it copies
nothing: what was saved before stays where it was. Under it:

| Folder | Holds | Written by |
|---|---|---|
| `Presets/<plugin>/<name>.json` | plugin presets | a plugin's preset menu |
| `Chains/<name>.json` | plug-in chains | `track.save_chain` |
| `Track Presets/<name>.json` | whole channel strips | `track.save_preset` |
| `Juke Presets/<name>.json` | Juke kits with their sounds | `mix.kit_preset_save` (Save preset in Juke) |
| `Templates/<name>.fdawm` | session templates | `session.save_template` |

A chain or track preset whose modules play audio (the Sampler's sample, Juke's pad samples) keeps
that audio as WAVs in `<name>_samples/` beside its file and lists them under `samples`, so it
loads in any session and the folder can be copied or synced whole. Loading reads them in as new
sources of the session (saved to its `Audio Files/` like any other); a sample whose file is gone
is left out and counted in `missing_samples`. The browser's Library tab (Window › Plugin Browser)
lists all of it with a search and a chip per kind: double-click a chain, track preset, plugin
preset or Juke preset to use it on the selected track, a template to open the New dialog on it.
Juke presets saved before the Library (in `juke_presets/` beside the kits folder) are still listed.

**Track presets.** A track preset is a JSON file (`"format": "soundcraft-track-preset"`) holding a
track's type, width, color, mixer (fader, pan, inserts, sends), instrument, elastic mode and
comments, the busses its sends feed (`busses`) and the audio its modules play (`samples`). Save
Track Preset… (Track menu, a track's context menu in the Edit window, a channel's in the Mix
window) asks for a name and writes into the Library; Load Track Preset… lists them newest first,
with Browse… for a preset file anywhere. Without a Library (the web build) Save asks for a path.

**Plug-in chains.** A chain is a JSON file (`"format": "freedawm-plugin-chain"`) holding a track's
inserts in order, each with its parameters and a hosted plug-in's state, and the audio they play.
Track › Save Plug-in Chain… and Load Plug-in Chain… work as the track preset items do.

| Command | Params | Result |
|---|---|---|
| `track.save_preset` ("Save Track Preset...") | `{path, track?}` (default: the first selected track) | `{path, track}`. Writes the preset file (replacing one already at `path`) and its `<name>_samples/`. Not undoable. |
| `track.load_preset` ("Load Track Preset") | `{path, tracks?}` (default: the selected tracks) | `{tracks, missing_samples}`. Applies the preset's mixer (and, to instrument tracks, its instrument) to each track. Each track keeps its own input, output, VCA, record arm and, when the widths differ, its pan. A send goes to the bus of the same name here, made when there is none (a preset saved before busses were listed keeps its bus ids). A file that is not a track preset is refused and nothing changes. One undo step. |
| `track.save_chain` ("Save Plug-in Chain...") | `{path, track?}` (default: the first selected track) | `{path, track, modules}`. Writes the chain file and its `<name>_samples/`. A track without inserts is refused. Not undoable. |
| `track.load_chain` ("Load Plug-in Chain") | `{path, tracks?, append?: false}` (default: the selected tracks) | `{tracks, modules, missing_samples}`. The chain replaces each track's inserts, or with `append` goes after them (as many as fit). Nothing else of the strip changes. Key inputs and link groups are not carried (they name things in the session the chain came from). A file that is not a chain, an empty chain, or one whose sample paths leave its folder is refused and nothing changes. One undo step. |

**Deleting markers and chords.** In the Edit window a click on a marker (its flag or name) or on
a chord picks it (outlined), and Delete or Backspace removes it. Right-click a chord: Edit Chord…,
Delete Chord, Delete Chords in Selection, Clear All Chords…; a marker: Edit Marker…, Delete Marker,
Delete Markers in Selection, Clear All Markers…; a marker or chord ruler's name: Clear All
Markers… (that ruler only) or Clear All Chords…. Clear All asks first in an in-app dialog. The pick lasts only while the Edit window shows,
the item exists, and neither the edit selection nor the clip selection has changed; otherwise
Delete does what it does without one.
"In Selection" means the items that start inside the edit selection (its end excluded).

**Delete clears clips.** With no ruler item picked, plain Delete or Backspace in the Edit window
runs `edit.clear` (Edit › Clear, Cmd+B, shown as "Ctrl+B  Del" / "Cmd+B  Del" in the menu): the
selected clips go, or the edit selection on the selected tracks is cleared, one undo step. With
nothing to clear the key does nothing. A text field with focus keeps the key (it edits the text),
the MIDI Editor keeps it while it has notes selected (it deletes those), the Vocal Editor and the
plugin editors under the pointer keep theirs, and in the Mix window Delete does nothing.

| Command | Params | Result |
|---|---|---|
| `markers.delete` ("Delete Memory Location") | `{number \| id \| name}`, or `{ids?: [id], range?: {start, end}, all?: bool, ruler?: 1-5}` | `{deleted}`. The first form deletes one memory location, as before. The second deletes every one listed in `ids`, starting inside `range` (`start` ≤ position < `end`; positions take any form above), or all of them with `all: true`, on marker ruler `ruler` only when given (ruler 1 includes markers saved before rulers existed). Matching nothing is refused ("no markers there"), as are a `ruler` outside 1-5 and malformed `ids`, `range` or `all`. One undo step. |
| `band.chords.delete` ("Delete Chords") | `{chords?: [id], range?: {start, end}, all?: bool}` | `{deleted}`. Deletes the chord lane's chords listed, starting inside `range` (in samples or any position form, compared on the tempo map), or all of them. At least one of the three is required; matching nothing is refused ("no chords there"). One undo step. `band.chord.remove {chord}` still deletes one. |

**Editing chords (the Chords ruler).** A double-click on empty Chords ruler space opens an in-app
chord picker at that half bar: a name box (anything `band.chord.add` reads, e.g. "Am7", "F#m7b5",
"G/B") with a live preview of the lane chord it becomes ("exact" or "nearest: …" in the band's
key), and quick buttons for the diatonic triads and sevenths of the key in force there. OK adds
the chord up to the next chord, or to the end of its bar, never cutting into an off-grid neighbour
(`snap: false` with the bounds the ruler chose); Cancel adds nothing. A double-click on a
chord, or right-click › Edit Chord…, opens the picker on it (`band.chord.set`). Dragging a chord's
body moves it and dragging an edge resizes it, on the half-bar grid; with Alt held the chord keeps
the pointer's position. Drags send `snap: false` with the span shown, so a resize moves only the
edge dragged. Picking, Delete and Clear All are as above.

**Key changes (the Key ruler).** A double-click on empty Key ruler space opens an in-app key
picker at that time (12 roots, spelled as the key spells them, × Major / Minor; it starts on the
key in force there, else the band key); OK adds the key change, Cancel adds nothing. A click on a
key change picks it (outlined) for Delete or Backspace, with the marker/chord pick's rules; a
double-click or right-click › Edit Key… opens the picker on it; right-click › Delete Key removes
it; a drag moves it to the nearest grid line (always, whatever the edit mode; a double-click adds where the Edit window would snap), and a drop on another key change is refused; the ruler's name offers Clear All Key Changes…,
confirmed in the app. With no key changes the ruler shows the band key, dimmed, with "(band key)"
when the session sets a key (the Vocal Editor's rule), else "No key set". Key names are what
`vocal::parse_key` reads: a root `A`-`G` with an optional `#`/`b`, then `major`/`maj`/`M`/nothing
or `minor`/`min`/`m`.

| Command | Params | Result |
|---|---|---|
| `event.add_key_change` ("Add Key Change...") | `{key?: "C major", at?: position}` | `{at}`. Adds a key change at `at` (default the session start), replacing one there. An unparsable `key` is refused. One undo step. |
| `event.key_change_edit` ("Edit Key Change") | `{at, key?, to?: position}` | `{at}` (where it is now). Renames the key change at `at` and/or moves it to `to`. Refused: no key change at `at`, another key change already at `to` ("A key change is already there"), neither `key` nor `to`, an unparsable `key`, a negative `to`. A session from before these commands may hold two at one tick: the edit acts on the last. One undo step. |
| `event.key_change_delete` ("Delete Key Change") | `{at}`, `{range: {start, end}}` or `{all: true}` | `{deleted}`. Deletes the key change at `at` (one: the last, should an older session hold two there), those starting inside `range` (end excluded), or all of them. Matching nothing, or none of the three, is refused. One undo step. |

**Templates.** File › Save As Template… writes `<settings dir>/Templates/<session name>.fdawm`
by default (the session without its clips, audio files and videos). File › New… lists Blank,
Demo and then *My templates* from that folder, newest first; choosing one shows its track count,
sample rate and the date it was saved, and Browse… takes a template file from anywhere (typed in
the dialog, or picked when the app has a file picker). File › New from Template… opens the same
dialog with the newest template chosen (Up and Down walk the list). It is a menu-only UI command
(`ui.new_from_template`); agents call `session.new` with the path.

| Command | Params | Result |
|---|---|---|
| `session.new` ("New...") | `{name?: "Untitled", sample_rate?: 48000, bit_depth?: 24, template?: "blank" \| "demo" \| <path of a template file>}` | `{name, sample_rate}`; with a template path also `{tracks, template, missing}`. A template opens as a **new untitled session** named `name`: it has no file and no folder (Save asks where, so the template file is never written over), keeps the template's own sample rate and bit depth (`sample_rate` and `bit_depth` are ignored), and finds any media the template names as the template stored it, paths relative to the template file made absolute (`missing` lists what it could not find). A path with no file, or a file that is not a session, is refused and the open session stays. Not undoable, like opening a session. |
| `session.save_template` ("Save As Template...") | `{path, replace?: false}` | `{path}`. Writes the session without clips, audio files or videos (tracks, mixer, routing, markers, tempo and the Band stay). An existing file needs `replace: true`. |

**Rulers.** The Chord Symbols ruler (`view.ruler {ruler: "chords"}`, a second view of marker
lane 2) is gone: its markers stay on View › Rulers › Markers 2, and `view.ruler` refuses
`chords`. A session saved with it shown opens with Markers 2 shown in its place (it is just
dropped when Markers 2 is shown too), so lane 2's markers stay in view. The Band's chord lane ruler is
labelled **Chords** (View › Rulers › Chords); its id stays `band_chords`.

## Live automation writing

While the transport plays, every track in Write, Touch, Latch, Touch/Latch or Trim mode writes
automation from its controls, every frame the host runs a pass (at most every 20 ms of the
timeline). Any command that changes an automatable value counts as touching that control for half
a second: `mix.volume` (its group's tracks too), `mix.pan` (`pan`, or `pan2` with `index: 1`),
`mix.mute`, `mix.send_level` (`db`, `pan` and `mute` touch `send_x_level`, `send_x_pan`,
`send_x_mute`), `mix.module_param` and `mix.insert_param` (the module lane). So a Mix strip, the
Edit window's keys, a module drawer, a plugin's own window, a MIDI controller and an agent all
write, whatever sent the change. A control that is held without moving (a touch-sensitive fader)
says so with `automation.touch`.

- **Write**: volume, pan, mute and every lane the track already has (not trim), touched or not.
- **Touch**: what is touched; let go, its lane plays its old automation again. A mute press has
  nothing to hold, so in Touch it writes until the transport stops.
- **Latch**: what is touched, then its last value until the transport stops.
- **Touch/Latch**: volume as Touch, everything else as Latch.
- **Trim**: only the trim lane, from `automation.trim` (the Mix fader in Trim mode); everything
  else reads.

Each point replaces the automation since the previous one (a locate while playing starts afresh,
so a jump keeps what it skipped); stopping writes a last point on each lane still writing. A pass
and everything dragged during it are one undo step. Send pan and send mute lanes now play too
(they were written but not heard before).

| Command | Params | What it does |
|---|---|---|
| `automation.touch` | `{tracks?, param, touch?: bool}` | `touch: true` holds the control touched until `touch: false`; without `touch`, touches it once (half a second). `param` as in `automation.set_point`. Not undoable |
| `automation.trim` | `{tracks?, db: -144..24, touch?: bool}` | The trim a Trim-mode fader sets; written to the trim lane while playing, the pass's touched control until `touch: false`. Not undoable |
| `automation.touched` | `{}` | `[{track, param}]`: the controls touched now (held, or changed within half a second) |

## Automation lanes

A track can show several automation lanes under its main lane at once (Pro Tools' automation
lanes); the main lane keeps its own view (`track.view`). They are saved with the session as the
track's `auto_lanes` (param ids, top to bottom; absent when none). A lane whose module is removed
goes with it. The Edit window draws each lane with its name (a picker that changes the lane's
parameter), **+** (show another lane below, from the lane picker) and **−** (hide), and edits its
breakpoints as on the main lane (select, drag, Pencil, curves, rubber band, Set Value…). The small
lane key at the bottom left of the track header shows the lanes with automation (volume when
none has any) or hides them all; right-click it to pick one. The picker marks parameters that
have automation with a dot and the ones on screen with a check.

| Command | Params | What it does |
|---|---|---|
| `track.auto_lane` | `{tracks?, param, show?: bool, after?: param}` | Shows (`true`), hides (`false`) or toggles the lane of `param` (any automation id: `volume`, `pan`, `pan2`, `mute`, `trim`, `send_a_level`, `plugin:m3:outGain`, …); with `after`, it goes right under that lane (a shown lane moves there). Refused for an unknown parameter or a link role. Returns `{shown}`. Undoable |
| `track.auto_lanes` | `{tracks?, show: data \| none}` | `data` adds a lane for every parameter with automation (volume when none has any) below those shown; `none` hides them all. Undoable |

The lane picker (the + of a lane, the right-click of the header's lane key, the track view
selector) lists the fader section (volume, pan, or pan left and pan right on a stereo track,
mute, trim), each assigned send's level, pan and mute (a submenu per send named after its bus;
an empty slot only while it still has automation), then each module's parameters: a built-in's
as its drawer offers them, a hosted VST3, CLAP or AU plugin's with its own names and ranges.
Every one is a lane id the commands take (`pan2`, `trim`, `send_c_pan`, `plugin:m4:7`); a send
lane past the last slot (`send_k_level`) is refused.
