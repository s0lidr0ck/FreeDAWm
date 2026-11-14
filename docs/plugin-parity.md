# Plugin parity: built-in processors and plugin hosting

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-11-06 · **Change:** minor (Lift - Sampler: cleanup, Slice mode, Warp, LFO sync; previously minor: Lift - Sampler: filter, LFO, glide, mono, From Selection, one-page editor; previously minor: modules emit MIDI: Shale's MIDI Out into instrument tracks; previously minor: the stock effects hidden from the plugin lists; one flat plugin menu by vendor; previously trivial: floating CLAP editors documented as not repainting during playback; VST3 controller messages delivered both ways; previously minor: Bounce, Freeze and Commit render through the live plugin instances; VST3 host messaging, restart flags, UI-thread teardown, offline mode; previously trivial: editors keep drawing during playback, CLAP host timers; previously trivial: hosted plugins open their own editor first; Capture Thumbnail Now; previously trivial: renamed to FreeDAWm, sessions saved as .fdawm; previously minor: plugin browser, out-of-process VST3 scanning; previously minor: VST-faithful editors for the FireStation engines; previously trivial: CleanQ's dynamic ceiling deferred and inert; previously trivial: the Vint strip graph; Vint's 64 parameters; previously minor: FireStation engines: six mixing-core ports; previously trivial: Sampler ping-pong loop phase (#463); previously trivial: mix.instrument sets built-in instrument parameters in params (#265); previously minor: plugin editors on Windows) · **Compared with:** Avid Pro Tools Ultimate 2026.4.1

> Pro Tools, AudioSuite and Avid are trademarks of Avid Technology, Inc. They are named here only to
> compare features (a workflow many users already know); FreeDAWm is not affiliated with or endorsed
> by Avid. See [NOTICE](../NOTICE).

Pro Tools users live in plugins: the bundled set covers everyday mixing, and third-party plugins
cover the rest. This doc tracks both. Our plugins have our own names and are written from
scratch (clean-room rule): Avid plugin names are not used here, only categories.

**Built-in plugins and Process: breadth ~40 %, ready ~35 %** (80–120 h).
**Third-party hosting: breadth ~70 %, ready ~45 %** (40–60 h). Both estimated.

## Built-in processors (measured from `crates/dsp/src/plugins`)

| Category | Pro Tools bundles (approx., by category) | FreeDAWm (ids) | Gap |
|---|---|---|---|
| EQ | 1-band and 7-band EQ, channel strip, vintage EQ models | `eq_1band`, `eq_7band`, `channel_strip` | Vintage models, linear phase, dynamic EQ; parametric UI (#139) |
| Dynamics | compressor/limiter, expander/gate, de-esser, maximizer, vintage compressors, multiband | `compressor`, `expander_gate`, `de_esser`, `maximizer`, `rez` (dynamic resonance suppressor: cuts only the frequencies that stick out of the sound's own spectrum; four sensitivity bands and a range on a live graph, L/R or M/S with link and balance, delta, a key input, its own editor, 111 factory presets of its own), `lull` (gate and expander: Expand or Gate, knee, hysteresis, hold, lookahead, filtered detector with Listen, a key that guards against bleed or triggers, its own editor and 26 factory presets) | Vintage/opto models, multiband, sidechain input (no key input anywhere) |
| Reverb | algorithmic room/plate/hall; convolution (paid tier) | `room_reverb`, `plate_reverb` | Hall/chamber, convolution, early-reflection editor |
| Delay | modulated delay, tape/analog delays | `mod_delay` | Tape, ping-pong, multi-tap, tempo-sync depth |
| Modulation | chorus, flanger, phaser, tremolo, ring/frequency shift | `chorus`, `flanger`, `phaser` | Tremolo, auto-pan, ring modulation |
| Harmonic | lo-fi, rectifier, amp simulation, saturation, tape | `lofi`, `rectifier`, `saturator` | Amp and cabinet models, tape |
| Pitch and time | pitch shift, varispeed effects, time shift | `pitch_shifter`, `time_shift` | Formant control, varispeed effect, pitch correction |
| Utility | trim, gain, invert, DC offset, signal generator, dither, time adjuster | `trim`, `gain`, `invert`, `dc_offset_removal`, `signal_generator`, `dither`, `sprawl` (stereo imager: four bands between three movable splits, each made narrower or wider by scaling only its side, so the mono sum never changes; Stereoize for mono sources, Mono Safe, a vectorscope and correlation meter in its own editor, 16 factory presets of its own) | Time adjuster, meter/analyzer plugins, noise shaping options |
| Instruments | several virtual instruments (multi-sampler, piano, drum machine, organ, synths, sample player; Massive X Player added in 2026.4) | `subtractive_synth`, `drum_synth`; `mix.instrument` takes the instrument's parameters in `params` and refuses unknown fields (#265). The sampler's ping-pong loop keeps its phase when a step is wider than the loop (#463). `sampler` (Lift - Sampler): one sample across the keyboard with start/end, loops with a crossfade, a resonant LP/HP/BP filter with its own envelope, an LFO (tempo sync), glide, mono legato, pan, From Selection, root and tempo detection, Trim/Normalize/Reverse, Slice mode (hits, beat grid, equal; slices to MIDI; one slice across the keys) and Warp (pitch apart from tempo), in a one-page knob editor. | multi-sample zones, piano, organ, wavetable, content library |
| **Total** | **≈ 80 processors and instruments** (estimate from Avid's bundle documentation) | **29 processors + 2 instruments** | |

Process (offline processing of clips; command `process.run`, old id `audiosuite.process`): every
leaf of the incumbent's AudioSuite menu in the catalog maps to one of our processes (`AUDIOSUITE` in
`crates/ui-egui/src/menus.rs`), so the menu is 100 % present. Depth: no preview,
handles, "clip by clip / entire selection" and multi-input modes beyond the basics; hosted
plugins can't be used offline from Process yet.

Fidelity: **unmeasured.** Nothing compares a FreeDAWm processor's frequency response, gain
reduction curve or reverb tail with the Pro Tools equivalent (G-07 in [`gaps.md`](gaps.md)).
User report #152 (a saved instrument level did not reach the bounce) is closed by rendering through
the live instances (see Third-party plugin hosting).

## FireStation engines

Six mixing engines are native Rust ports of the owner's C++ FireStation effects (private fork;
they are the owner's own code, so the clean-room rule for Avid names does not apply to them). Each
is a line-by-line translation of the C++ DSP, registered in the `Category::FireStation` category and
verified against renders of the C++ engine. A case passes at an RMS difference of -60 dB or better,
relative to the reference.

| Engine | Id | Cases | Worst RMS (rel.) | Notes |
|---|---|---:|---|---|
| Sat | `fs_sat` | 45 | about -117.5 dB | |
| Tape | `fs_tape` | 50 | about -119.7 dB | Hiss and flutter are deterministic and match exactly (A/B) |
| Comp | `fs_comp` | 55 | 54 cases below -60 dB | `preset_blue_stripe_trash` is -50.6 dB under a written per-case exception (-45 dB bar), backed by a 1-ulp self-sensitivity check of -49.5 dB |
| CleanQ | `fs_cleanq` | 55 | about -107 dB | 254 host parameters; ceiling cutting is unit-tested only. The dynamic ceiling is inert: it needs a drawn or learned ceiling shape, not ported yet (its drawer tab is greyed with a note, `ceilOn` cannot be switched on from the UI, the tile shows Output only) |
| Twine | `fs_twine` | 26 | about -118 dB | |
| Vint | `fs_vint` | 62 | about -106.7 dB | 64 parameters: the C++ appended `conPre`, the 1073's gain switch. Code paths unreachable with `VINT_MODELLED_PASSIVE=0` are not ported |

Infrastructure that came with the ports:

- **Fidelity harness** (`crates/dsp/tests/fidelity.rs`): runs each case against the C++ render. The
  audio (inputs and references) is local only and git-ignored; only `cases.json` is committed.
  `cargo xtask fidelity-inputs`, `fidelity-refs <engine>` and `fs-tables <engine>` regenerate them.
  A per-case exception needs a written reason.
- **Analysis rings** (`Plugin::analysis`): plugins publish data for the strip graphs.
- **Latency changes**: when a plugin's reported latency changes, the chain re-aligns with a 10 ms
  crossfade.
- **Parameters**: `ParamInfo.display` and `ParamInfo.group` carry display text and grouping.
- **Strip UI**: per-engine graphs, Station accents and key knobs, drawer tabs, a FireStation-first
  plug-in picker, factory presets, and the `presets.import_firestation` command. Vint's graph is
  its measured frequency response, computed off the UI thread, cached, and asked for at most once
  per 100 ms per module.
- **Editors** (2026-10-20): each of the 18 FireStation modules opens, in the Mix window, a wide
  drawer translated from its JUCE editor (`crates/ui-egui/src/editors/`, on the kit in
  `editor_kit/`): the VST's size and palette, every gesture, the preset browser with favourites,
  Copy / Paste of FireStation preset text, the editor's own Undo and A/B, the dirty star, the
  tooltips, and the live meters the editors read (`Analysis::live`). Interaction inventories:
  `docs/superpowers/editors/`. The narrow generic drawer stays behind `ui.mix.wide_editors`.

Deferred: CleanQ's dynamic-ceiling feature (drawing or learning the ceiling shape; until then the
ceiling is inert), and the Plan 3 and Plan 4 engines (Dist, Pitch, Mod, Filter, Delay, Reverb,
Transient, Amp, ChamberMade, Swale; then Shale, LevelUp, Scout2).

## Third-party plugin hosting

| Feature | Pro Tools | FreeDAWm | Notes |
|---|---|---|---|
| Plugin format | AAX only (Native and DSP) | CLAP, VST3, Audio Units (macOS) | Users' libraries mostly ship VST3/AU too. AAX (#109) needs Avid's SDK licence and PACE signing: owner decision, likely never |
| VST2, LV2, LADSPA | — | ❌ (#30) | Beyond Pro Tools; VST2 SDK is no longer licensable |
| Scan, vendor menus, instrument picker | ✅ | ✅ | #23. One flat menu: FireStation, Utilities, then one submenu per vendor with its CLAP, VST3 and Audio Units plugins together (a plugin in several formats listed once, CLAP first; the search lists every format). Spec 2026-10-28-plugin-menu |
| Audio, parameters, automation, latency (PDC) | ✅ | ✅ | |
| Notes / MIDI to instruments | ✅ | ✅ | But no live MIDI input (G-02) |
| A plugin's MIDI output into another track | ✅ (MIDI nodes) | 🟡 | Built-in modules: `Plugin::midi_out` (a fixed 64-event ring per slot, read by the mix engine after `process`); Shale sends a note per hit, and any instrument or MIDI track can take one or several modules as its input (`track.input module:…`, the input menu's MIDI from; recorded as MIDI clips). Hosted VST3/CLAP plugins that output MIDI are not read yet: their `midi_out` is the default (nothing) |
| State saved in sessions, presets | ✅ | ✅ | |
| Create/destroy off the audio thread | ✅ | ✅ | |
| Editors: CLAP | — | 🟡 floating windows; embedded editors on Windows; `clap.timer-support` host timers | Embedded on macOS/Linux missing. A floating (plugin-owned) editor on Windows gets its timers from the per-frame service but not its paints: the host has no HWND to `RedrawWindow`, so it stops repainting while the transport plays (the embedded case is covered; enumerating the thread's windows would close this) |
| Editors: VST3 | — | 🟡 macOS (verified with a commercial pitch editor) and Windows (verified with a free JUCE compressor) | Linux views missing |
| Processor ↔ controller messaging (VST3) | ✅ | ✅ `IMessage`/`IAttributeList` from the host, connection proxies that deliver audio-thread messages on the UI thread, `restartComponent` flags honoured (a preset loaded in the plugin's GUI updates the session's parameters) | Was: `createInstance` refused, sides connected directly (the controller's handler ran on the audio thread), restart flags dropped: the crash candidates for an IK instrument |
| Instances terminated off the audio thread | ✅ | ✅ a replaced player hands its instances back to the UI thread first (`Cmd::Shutdown`) | |
| Offline renders in offline mode | ✅ | ✅ VST3 `kOffline`, CLAP `clap.render`: instances made for a headless render are prepared offline, and the live instances a Bounce/Freeze/Commit borrows are re-prepared offline on lend (deactivate, `setupProcessing`/`clap.render`, activate, on the main thread; state kept) and realtime again on return | Disk-streaming instruments wait for their data instead of dropping it; a plugin that refuses the mode logs and renders on as it is |
| Hosted instrument edits stored | ✅ | ✅ edits in a hosted instrument's own editor and its restart reports update the session (`mix.instrument_param` takes hosted ids); a render from the stored state starts in step with it | Report #152 (bounce level) |
| Editors keep drawing during playback | ✅ | 🟡 Windows: each UI frame dispatches the editor's `WM_TIMER`s and runs its pending paints (at most 60 Hz), which Win32 otherwise starves while the app's own window repaints continuously; VST3 views and embedded CLAP editors (both in a host window). A floating CLAP editor gets the timers only, not the paints (see Editors: CLAP) | Reported with a Waves VST3 on a bus: the knob moved in the audio but not on screen until Pause |
| Editors: AU | — | ❌ | Cocoa view hosting missing |
| Multiple editor pages | ✅ | 🟡 | #6 |
| macOS hardened-runtime entitlement for third-party code | ✅ | ❌ | #153, #149 — signed builds refuse third-party VST3 |
| Sidechain / key input | ✅ | ❌ | All three hosts |
| ARA 2 (pitch/time editors in the timeline) | ✅ | ❌ | Menu leaves Track/Clip/Window ▸ ARA still missing |
| Crash isolation | in-process (DSP on HDX) | 🟡 VST3 scanning out of process (`freedawm-scan`, cached; a crashing plugin is skipped); hosting still in-process | Out-of-process hosting would beat Pro Tools |
| Plugin browser | ✅ | ✅ docked panel: thumbnail grid, tabs, search, favourites, categories, drag/double-click to place, presets tab | `window.plugin_browser` |
| Hosted editor first, browser pictures | ✅ | ✅ clicking a hosted instrument slot or insert tile opens the plugin's own editor (the generic window when it has none: right-click › Show Parameters…); a card's picture is taken from the open editor after 1.5 s (10 s for instruments, which may show a loading screen first), or at once with right-click › Capture Thumbnail Now | UI actions |
| Plugin delay reporting changes on the fly | ✅ | 🟡 | Aux insert latency bug #103 (PR #133) |
| Offline bounce, freeze and commit through the live instances | ✅ (offline bounce) | ✅ Bounce Mix, Freeze, Freeze up to here, Commit and Track › Bounce stop the transport, borrow the player's engine (every hosted instance in it: `Cmd::Lend`/`Cmd::Return`, the callback plays silence meanwhile) and render through it on the main thread a slice per frame, at the live sample rate and block size; each instance's state is put back as it was (built-in modules are not snapshotted: they are reset on return, as a Stop resets them) and the engine returns. No copy of a hosted plugin is made, so the preset chosen in a plugin's own editor (which the session's stored params never see) is what gets printed: a fake hosted instrument bounces and freezes within 0.5 dB of playback. The transport cannot start during a render ("Rendering…"). Headless (CLI, MCP) renders still make instances of their own from the stored state | Reported with IK Pianoverse: the bounce came out 14 dB quieter than playback, and Freeze/Print crashed |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-06 | minor | Lift - Sampler: Trim/Normalize/Reverse, loop crossfade, Slice mode, slices to MIDI, Warp, tempo detection, LFO sync |
| 2026-11-06 | minor | Lift - Sampler: filter, LFO, glide, mono, pan, From Selection, root detection, one-page editor |
| 2026-10-28 | minor | Modules can emit MIDI (`Plugin::midi_out`, `PluginInfo::midi_out`, a preallocated ring per slot): Shale's MIDI Out sends a note per hit (channel 10, MIDI Note, velocity from the hit, off a sixteenth later or at the next hit) and feeds any instrument/MIDI track whose input is MIDI from it (one or several modules, merged); hosted plugins' MIDI output still unread |
| 2026-10-28 | minor | The stock effects (EQ, dynamics, reverb, delay, modulation, harmonic, pitch, dither: 18 modules) are hidden from the picker, its search and the plugin browser (`soundcraft_dsp::HIDDEN_STOCK`, `PluginInfo::listed`); they still load, play and open from sessions and `mix.module_add` takes them by id. The picker is one flat menu: FireStation, Utilities, then vendors with CLAP, VST3 and AU merged. Estimates unchanged |
| 2026-10-28 | trivial | Live instances lent to a render are re-prepared in offline mode for it (realtime again on return); a lend cancelled before the audio thread answers gets its engine back; a returned engine never re-enters a shut audio state; the audio device cannot change during a render |
| 2026-10-28 | trivial | Floating CLAP editors: timers serviced, paints not (documented gap); VST3 controller→processor messages sent off the main thread are delivered too |
| 2026-10-28 | minor | VST3 host holes closed: host `IMessage`/`IAttributeList`, connection proxies, restart flags; instances terminated on the UI thread; offline renders in offline mode; hosted instrument edits stored |
| 2026-10-28 | minor | Bounce Mix, Freeze, Freeze up to here, Commit and Track › Bounce render through the live plugin instances (the player lends its engine; per-frame jobs; state restored; transport refused meanwhile) |
| 2026-10-27 | trivial | Hosted plugin editors keep drawing while the transport plays (Windows: timer and paint service from the UI frame); CLAP host timers |
| 2026-10-27 | trivial | Hosted instruments open their own editor like inserts; Show Parameters… and Capture Thumbnail Now on the slot and tile menus; instruments wait 10 s before their picture is taken |
| 2026-10-26 | trivial | The product is renamed FreeDAWm (was SoundCraft); sessions are saved as `.fdawm` and `.scraft` still opens. Estimates unchanged |
| 2026-10-23 | minor | Plugin browser panel, out-of-process VST3 scanning, hosted presets (`mix.module_preset`) |
| 2026-10-20 | minor | VST-faithful editors for the 18 FireStation modules, the Mix drawer's default |
| 2026-10-19 | trivial | CleanQ: the dynamic ceiling (draw/learn shape) is deferred and inert until then; its drawer tab is greyed, the tile shows Output only |
| 2026-10-19 | trivial | The Vint strip graph (frequency response); Vint has 64 parameters (the C++ appended `conPre`) |
| 2026-10-19 | minor | FireStation engines: six mixing-core ports (fs_sat, fs_tape, fs_comp, fs_cleanq, fs_twine, fs_vint) with fidelity status |
| 2026-10-13 | trivial | Sampler ping-pong loop phase (#463) |
| 2026-10-13 | trivial | mix.instrument sets built-in instrument parameters in params (#265) |
| 2026-10-12 | minor | Editors: VST3 and embedded CLAP editors on Windows |
| 2026-10-10 | major | Created: built-in processor inventory by category and third-party hosting checklist |
