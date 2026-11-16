# Four-DAW parity, section 5: built-in effects

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-16 · **Change:** minor (bug fixes, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). Hardware named below (1176, LA-2A, Pultec and so on) belongs
> to its makers and is named only to say what a model imitates. FreeDAWm is not affiliated with or
> endorsed by any of them. See [NOTICE](../../NOTICE).

The fifth section ([1: playing audio](01-playback-audio.md),
[2: recording audio](02-recording-audio.md), [3: mixing](03-mixing.md),
[4: editing audio](04-editing-audio.md)).

**Scope of this section:** the effects that ship in the box: what there is in each family, how
good the engineering is, and what surrounds them (presets, editors, bypass, sidechain, surround,
racks). Built-in instruments are counted here and compared in the MIDI sections. How the mixer
hosts inserts and routes sidechains is [section 3](03-mixing.md); offline processing of clips is
[section 4](04-editing-audio.md).

## How this was measured

- **FreeDAWm:** read from the code in the main working folder on the working branch (0.9.11 plus
  work in progress on sections 1–4), not from the docs. The headline claims (which effects the
  menu shows, no key input on the compressors, two channels at most, the Pitch latency, the fixed
  Delay tail) were re-read by hand. Nothing was listened to or measured, so "works" means the
  processing is there and wired, not that it sounds as good as the product it is compared with.
- **The four DAWs:** from their published manuals and release notes, as known up to the versions
  above, for each product's top edition and what installs with it. Nothing was run side by side.

**Legend.** Other DAWs: ● has it · ◐ partly or by another route · ○ does not.
FreeDAWm: **Works** · **Partial** · **Flag only** · **Missing**.

## What is in the box

FreeDAWm registers 55 processors: 51 effects and 4 instruments. The insert menu shows 33 of the
effects:

- **21 FireStation engines** (ported from the owner's C++ suite): Comp, CleanQ, CopyQ, Vint,
  Sat, Tape, Dist, Amp, Twine, Transient, Manners, Sift, Shale, Swale, Filter, Mod, Delay,
  Reverb, ChamberMade, Pitch, LevelUp. They carry 783 factory presets between them.
- **5 FreeDAWm designs:** Rez (resonance suppressor, 111 presets), Lull (gate, 26), Sprawl
  (imager, 16), Vault (convolution reverb), Mend (voice cleaning).
- **7 utilities:** Gain, Trim, Invert, DC Offset Removal, Signal Generator, Time Shift, Imprint
  (impulse-response capture).

**18 older stock effects are hidden from the menu on purpose**, because an engine covers each
one: EQ 1-Band, EQ 7-Band, Compressor/Limiter, Expander/Gate, De-Esser, Maximizer, Channel Strip,
Room and Plate Reverb, Mod Delay, Chorus, Flanger, Phaser, Saturator, Lo-Fi, Rectifier, Pitch
Shifter, Dither. They still load from old sessions. The tables below judge what a user can reach
from the menu, and say so where a hidden effect would have filled a gap.

## Summary

| Area | FreeDAWm today | Distance from the four |
|---|---|---|
| A. Dynamics | Ten modelled compressor styles, multiband, true-peak limiter, transient shaper, two gates | Medium: **no compressor takes a key input**, so no ducking; no upward compression |
| B. EQ and filters | 24-band dynamic EQ with mid/side, five modelled vintage EQs, match EQ, nine circuit filters | Small: no linear-phase mode on the main EQ, no simple channel EQ in the menu |
| C. Saturation and amps | Tape, five saturation circuits, twelve pedals, seven guitar amps with IR cabinets | Small to medium: no bass amp, no tuner, no pedalboard |
| D. Reverb | Six algorithmic machines, a geometry-built chamber, zero-latency convolution | Medium: **no bundled impulse responses**, nothing beyond stereo |
| E. Delay and modulation | Six delay voicings, fourteen modelled modulation circuits | Small to medium: no rotary speaker, auto-pan, ring modulator or frequency shifter |
| F. Pitch and voice | Octaver, doubler, whammy; machine-learning de-noise and de-reverb on an insert | Medium: no real-time pitch correction, no vocoder |
| G. Creative and mastering | Rhythmic swells, step-sequenced filter, loudness matching with references | **Large**: no beat repeat, gater, glitch or multi-effect; no mastering assistant |
| H. Utility and metering | Gain, polarity, DC, tone generator, delay, imager | Medium: **no meter plug-ins** (spectrum, loudness, phase), no dither in the menu, no tuner |
| I. Presets | 936 factory presets, a searchable browser, chain and track presets | Small: two save paths that disagree, no rename, no default preset |
| J. Editors | Custom editors for 24 effects, live graphs on mixer tiles | Small: fixed size, no value type-in on knobs |
| K. Common behaviour | Latency and tails reported, every parameter automatable, every channel processed | **Large**: no controller mapping, no racks or macros |

Three things every one of the four does and FreeDAWm does not: **a compressor you can key from
another track**; **effects that process every channel of a surround track**; and **meter
plug-ins** (spectrum, loudness, phase) you can put anywhere in a chain.

## A. Dynamics

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Compressor with knee, mix, sidechain filter, gain-reduction meter | ● | ● | ● | ● | **Works** (Comp) |
| Modelled characters (FET, opto, VCA, vari-mu) | ● | ● | ● | ● | **Works**: ten styles, three variants each |
| Key input from another track | ● | ● | ● | ● | **Missing** on Comp; only Lull, Rez and Shale take a key |
| Look-ahead | ● | ● | ● | ● | **Missing** on Comp (the hidden stock compressor has it) |
| Auto make-up gain | ◐ | ● | ● | ● | **Works** |
| Mid/side and stereo link amount | ◐ | ● | ● | ● | **Works** |
| Multiband compressor | ● | ● | ● | ● | **Works**: three bands inside Comp; crossovers are not linear-phase |
| Upward compression or expansion | ◐ | ◐ | ◐ | ● | **Missing** |
| Brickwall limiter aware of inter-sample peaks | ● | ● | ● | ● | **Works** (Twine: 8× true-peak detection) |
| Clipper | ◐ | ◐ | ◐ | ● | **Works** (Twine: hard, soft, tape) |
| Gate and expander with hysteresis, hold, look-ahead, key | ● | ● | ● | ● | **Works** (Lull; Shale for drums with bleed removal) |
| De-esser | ● | ● | ● | ● | **Works** (Sift: split or wide, plus plosives) |
| Transient shaper | ◐ | ◐ | ● | ● | **Works** |
| Dynamic resonance suppressor | ○ | ○ | ○ | ○ | **Works** (Rez), with a key input |
| Ducker (music under voice) | ● | ● | ● | ● | **Missing**: needs the compressor key |
| Loudness leveller to a LUFS target | ○ | ○ | ◐ | ○ | **Works** (LevelUp) |

## B. EQ and filters

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Parametric EQ, eight or more bands, steep cuts | ● | ● | ● | ● | **Works** (CleanQ: 24 bands, cuts to 96 dB/oct) |
| Simple channel EQ that opens instantly | ● | ● | ● | ● | **Missing** from the menu (EQ 1-Band and 7-Band are hidden) |
| Dynamic bands | ◐ | ● | ○ | ● | **Works** |
| Mid/side or left/right per band | ◐ | ● | ◐ | ● | **Works** |
| Spectrum behind the curve | ● | ● | ● | ● | **Works** |
| Linear-phase mode | ◐ | ◐ | ● | ● | **Partial**: in the match EQ only |
| Correct bell shapes near the top of the range at 44.1/48 kHz | ● | ● | ● | ● | **Works**: matched to the analog shape (within about 2 dB) |
| Tilt band | ○ | ○ | ◐ | ◐ | **Missing** |
| Match EQ | ○ | ○ | ● | ◐ | **Works** (CopyQ: up to four weighted profiles) |
| Modelled vintage EQs | ● | ● | ● | ◐ | **Works** (Vint: two Pultecs, 1073, 550A, SSL E) |
| Graphic EQ | ◐ | ○ | ● | ● | **Missing** |
| Automatic balance EQ | ○ | ○ | ○ | ○ | **Works** (Manners) |
| Resonant filter with envelope, LFO and step modulation | ● | ● | ● | ● | **Works** (Filter: nine circuits, 16 steps) |
| Wah | ● | ● | ● | ● | **Works** |
| Formant / vowel filter | ◐ | ○ | ◐ | ◐ | **Missing** |

## C. Saturation, distortion and amps

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Tape machine | ◐ | ◐ | ● | ● | **Works**: four machines, wow, flutter, hiss |
| Tube, transformer and console saturation | ● | ● | ● | ● | **Works** (Sat: five circuits) |
| Overdrive, distortion and fuzz pedals | ● | ● | ● | ● | **Works** (Dist: twelve) |
| Oversampling to control aliasing | ● | ● | ● | ● | **Partial**: good in Sat, Tape, Vint, Twine and Amp; Dist and Filter use a weak filter (bug 8) |
| Bit and sample-rate reduction | ● | ● | ● | ● | **Partial**: inside Vint's Sampler circuit; the Lo-Fi effect is hidden |
| Exciter / enhancer | ◐ | ◐ | ● | ◐ | **Works** (Vint Enhancer) |
| Sub-bass generator | ● | ○ | ● | ◐ | **Missing** |
| Guitar amps | ● | ● | ● | ● | **Works**: seven, solved tube by tube; mono out |
| Bass amp | ● | ● | ● | ● | **Missing** |
| Cabinets from impulse responses | ● | ● | ● | ● | **Works** |
| Pedalboard in one window | ◐ | ● | ● | ● | **Missing**: pedals are separate inserts |
| Tuner | ● | ● | ● | ● | **Missing** |

## D. Reverb

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Room, hall, plate | ● | ● | ● | ● | **Works** (Reverb) |
| Chamber, spring | ◐ | ◐ | ● | ◐ | **Works**; ChamberMade builds the room from its shape, materials and microphones |
| Shimmer | ○ | ○ | ◐ | ● | **Works** |
| Gated / non-linear | ● | ◐ | ● | ◐ | **Missing** |
| Convolution with your own impulse responses | ● | ● | ● | ● | **Works** (Vault): zero latency, true stereo, up to 10 s |
| Bundled impulse-response library | ● | ● | ● | ● | **Missing**: one generated hall |
| Edit the impulse response (reverse, trim, stretch, envelope) | ◐ | ● | ● | ● | **Partial**: a decay envelope only |
| Capture your own impulse responses | ○ | ● | ● | ○ | **Works** (Imprint) |
| Pre-delay synced to tempo | ◐ | ● | ● | ● | **Works** in Reverb; not in Vault or ChamberMade |
| Freeze, ducking, modulation, damping | ◐ | ◐ | ● | ● | **Works** in Reverb; the duck follows the reverb's own input only |
| Surround or immersive reverb | ● | ◐ | ● | ● | **Missing** |

## E. Delay and modulation

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Stereo, ping-pong, dual delay with tempo sync | ● | ● | ● | ● | **Works** |
| Tape and bucket-brigade models | ● | ● | ● | ● | **Works**: six voicings |
| Multi-tap and rhythm patterns | ● | ● | ● | ● | **Works** |
| Reverse, freeze, pitch in the loop, ducking | ◐ | ◐ | ● | ● | **Works** |
| Granular delay | ○ | ○ | ◐ | ◐ | **Missing** |
| Long synced delays at high sample rates | ● | ● | ● | ● | **Works** up to 10.9 s at every rate |
| Chorus, ensemble, flanger, phaser | ● | ● | ● | ● | **Works** (Mod: fourteen modelled circuits) |
| Tremolo, vibrato | ● | ● | ● | ● | **Works** |
| Auto-pan | ◐ | ● | ● | ● | **Missing** |
| Rotary speaker | ◐ | ● | ● | ● | **Missing** |
| Ring modulator, frequency shifter | ● | ○ | ● | ● | **Missing** |
| Modulation locked to the bar | ● | ● | ● | ● | **Works** in Filter, Swale and Mod |

## F. Pitch and voice

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Real-time pitch shifter | ● | ◐ | ● | ● | **Works** (Pitch); no formant preservation, and its delay is not compensated (bug 5) |
| Octaver, doubler, harmony at fixed intervals | ● | ◐ | ● | ● | **Works** |
| Harmony that follows a key | ○ | ○ | ◐ | ◐ | **Missing** |
| Real-time automatic pitch correction | ○ | ○ | ● | ● | **Missing**; the vocal editor is offline (section 4) |
| Vocoder | ◐ | ● | ● | ● | **Missing** |
| De-noise on an insert | ○ | ○ | ○ | ◐ | **Works** (Mend Live): about 50 ms delay, 44.1 and 48 kHz only |
| De-reverb on an insert | ○ | ○ | ○ | ○ | **Works** (Mend Studio): about 2.9 s delay, so for mixing, not tracking |
| De-click, de-hum | ○ | ○ | ○ | ○ | **Missing** |
| Stem separation as a live insert | ○ | ○ | ○ | ○ | **Missing**; offline only, as in the four |

## G. Creative effects and mastering

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Beat repeat / stutter | ◐ | ◐ | ● | ● | **Missing** |
| Gater or drawn volume shaper | ◐ | ● | ● | ● | **Partial**: Swale repeats one attack-hold-release shape per beat |
| Step-sequenced multi-effect | ○ | ◐ | ● | ● | **Partial**: the Filter's 16-step cutoff only |
| Glitch, granular or spectral effects | ◐ | ○ | ● | ◐ | **Missing** |
| Lo-fi and vinyl | ● | ◐ | ● | ● | **Partial**: cassette and 12-bit sampler models; no vinyl |
| Mastering assistant | ○ | ○ | ● | ○ | **Missing** |
| Loudness match against reference tracks | ○ | ◐ | ◐ | ◐ | **Works** (LevelUp: four references, platform preview) |

## H. Utility and metering

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Gain, trim, polarity | ● | ● | ● | ● | **Works**; polarity flips all channels together |
| Swap channels, mono switch, mid/side encode | ◐ | ● | ● | ● | **Missing** as a utility |
| Stereo width and imaging | ◐ | ● | ● | ● | **Works** (Sprawl: four bands, mono-safe) |
| DC removal, tone generator, sample delay | ● | ● | ● | ● | **Works**; no sweep in the generator |
| Dither with noise shaping | ● | ● | ● | ● | **Partial**: inside LevelUp only; the Dither effect is hidden |
| Spectrum analyser plug-in | ◐ | ● | ● | ● | **Missing**: spectra appear inside editors only |
| Loudness and true-peak meter plug-in | ◐ | ● | ● | ● | **Partial**: inside LevelUp; the meter code is also used at bounce |
| Phase / correlation meter plug-in | ● | ● | ● | ● | **Partial**: inside Sprawl |
| Phase alignment between microphones | ◐ | ◐ | ◐ | ◐ | **Partial**: inside Shale, for drums |
| Tuner | ● | ● | ● | ● | **Missing** |

## I. Presets

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Factory presets | ● | ● | ● | ● | **Works**: 936, on 24 of the 33 listed effects |
| Save and load your own | ● | ● | ● | ● | **Works**; both save paths write the same file |
| Rename and delete your own | ● | ● | ● | ● | **Partial**: delete in the wide editors; no rename |
| Browser with search, tags and favourites | ◐ | ● | ● | ● | **Works** |
| Step to next and previous | ● | ● | ● | ● | **Works** in the wide editors |
| Default preset for new instances | ● | ● | ● | ● | **Missing** |
| A/B compare | ● | ● | ● | ● | **Partial**: missing in five editors and all narrow drawers |
| Copy settings to another instance | ● | ● | ● | ● | **Partial**: by clipboard, wide editors only |
| Chain presets and track presets | ● | ● | ● | ● | **Works** |
| Favourites and recents in the insert menu | ◐ | ● | ◐ | ● | **Partial**: in the browser panel only |
| Hide effects you do not use | ● | ● | ● | ● | **Missing**: the hidden list is fixed in code |

## J. Editors

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Purpose-built editor per effect | ● | ● | ● | ● | **Works** for 24; Vault and Mend have a drawer with a graph |
| EQ curve with draggable points; compressor curve and history | ● | ● | ● | ● | **Works** |
| Resizable or scalable windows | ◐ | ● | ● | ● | **Missing**: fixed size, with a slim layout as the alternative |
| Several editors open; detach to their own windows | ● | ● | ● | ● | **Works** |
| Pin on top; follow the selected track | ● | ● | ◐ | ● | **Missing** |
| Type a value into any control | ● | ● | ● | ● | **Partial**: CleanQ and a few fields |
| Fine adjust; reset by double-click | ● | ● | ● | ● | **Works** |
| Undo inside the editor | ◐ | ● | ● | ● | **Works**, and every move is also a session undo step |
| Small views on the mixer (EQ curve, gain reduction, pinned knobs) | ◐ | ● | ● | ● | **Works** |

## K. Common behaviour

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Bypass without a click | ● | ● | ● | ● | **Works**: a crossfade; a plugin switched back on starts clean |
| Bypass keeps other tracks in time | ● | ● | ● | ● | **Works**: a bypassed insert keeps its latency |
| Latency reported for delay compensation | ● | ● | ● | ● | **Works**, except Pitch, Sat and Vint (bug 5) |
| Tails included in bounce and freeze | ● | ● | ● | ● | **Partial**: Filter still reports a fixed 1 s (bug 6) |
| Every parameter automatable | ● | ● | ● | ● | **Works** |
| Wet/dry mix on every effect | ◐ | ● | ◐ | ◐ | **Partial**: missing on eight engines; no mix on the insert slot |
| Process every channel of a surround track | ● | ● | ● | ● | **Works**: the front pair as stereo, every other channel through its own mono instance; dynamics are not linked across them |
| Map a hardware controller to a parameter | ● | ● | ● | ● | **Missing** (section 3) |
| Modulators (LFO, envelope) for any parameter | ○ | ◐ | ○ | ● | **Missing** |
| Rack with parallel, mid/side or band splits; macro knobs | ○ | ● | ○ | ◐ | **Missing**: chains are a straight line of up to 32 |
| Console channel strip | ● | ● | ○ | ● | **Missing** from the menu (Channel Strip is hidden) |
| Processor load shown per effect | ◐ | ● | ◐ | ◐ | **Missing** |
| Works in the browser build | — | — | — | — | **Partial**: Mend passes audio through; no user presets |

## Bugs found on the way

Found while reading; none was reproduced by running the program. Paths are under `crates/`.
Three rows are marked *by design*: the port matches the C++ original on purpose, so changing
them is the owner's decision, not a repair.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed.** Saving a user preset from a wide editor stored the parameters the editor keeps out of presets (LevelUp's reference, link group and learn state; CopyQ capture; ChamberMade's Mix and Level; others) and loading restored them. The file now leaves them and Bypass out, and a load ignores them in older files (`editor_kit/presets.rs`: `user_preset_body`, `decode`). Test: `ui-egui/tests/editor_kit.rs` | `ui-egui/src/editor_kit/presets.rs:221-230, 693` |
| 2 | **Fixed.** On a track wider than stereo, 24 effects (every engine, Mend, Vault) processed left and right only; centre, LFE and surrounds passed unprocessed. They now run one instance on the first two channels and a mono instance on each other channel (`dsp/src/plugins/per_channel.rs`), with the same parameters, transport, assets and state. Detection is not linked across them. Test: `per_channel.rs` (a 5.1 surround channel sounds as on a mono track) | `dsp/src/plugins/fs/*/mod.rs` (`channels.min(2)`) |
| 3 | **Fixed.** Saving from the narrow drawer wrote no name or tags, every stored parameter, no CleanQ guides, and let a user preset take a factory preset's name (after which the factory one won). It now writes the wide editor's file (`user_preset_body`: name, tags, the same parameters, CleanQ's guides) and refuses a factory name; its loads skip what the editors keep out of presets (`editors::not_in_presets`). **Claim corrected:** the Dist knob bank and the Amp and Vault loaded files are not in the wide editor's presets either, so the two paths did not differ there. Test: `editor_kit/presets.rs` | `ui-egui/src/panels.rs:971-1029` |
| 4 | **Fixed.** Bypass was an instant switch with no reset, and the track's delay compensation changed in the same block. A bypass now crossfades (the mixer's 10 ms), a bypassed insert keeps its latency (its input goes past it through a delay of that length, so no track moves), and a plugin switched back on is reset and heard once its latency has passed (`insert_step` in `mix/src/lib.rs`). Test: `mix/tests/insert_bypass.rs` | `mix/src/lib.rs:1939, 2721, 2770` |
| 5 | *By design.* Pitch reports no latency though its shifted voices run 6–20 ms late; Sat and Vint also report none | `dsp/src/plugins/fs/pitch/mod.rs:14-16`; `fs/sat/mod.rs:16`; `fs/vint/mod.rs:40` |
| 6 | **Partly fixed.** Delay reported an 8 s tail whatever the feedback or freeze, Mod 0.25 s, ChamberMade four tape repeats, so a bounce or freeze could cut the tail. Delay now reports at least 8 s and as long as its echoes take to fall 80 dB (60 s for Freeze or a runaway loop); Mod adds its flangers' Regen ring; ChamberMade counts tape repeats to -80 dB. Filter still reports 1 s: left for now (its file is being changed for bug 8); a resonant setting low down can ring longer. Tests: delay, mod and chamber `tests.rs` | `dsp/src/plugins/fs/delay/mod.rs:251-254`; `fs/mod_engine/mod.rs:398-401`; `fs/filter/mod.rs:303`; `fs/chamber/mod.rs:409-414` |
| 7 | **Fixed.** The Delay buffer was a fixed number of samples (10.9 s at 48 kHz, 2.7 s at 192 kHz), so long synced times were shortened and lost sync at high rates. A line now holds the same 10.9 s at every rate (`line_len`; unchanged at 44.1 and 48 kHz). Test: delay `tests.rs` (a 4 s whole note at 60 BPM keeps its time at 192 kHz) | `dsp/src/plugins/fs/delay/delay_engine_dsp.rs:59-60, 966` |
| 8 | *By design.* Dist and Filter oversample four times behind a filter that rejects only about 16 dB at the original Nyquist, so high-gain settings alias | `dsp/src/plugins/fs/dist/dist_engine_dsp.rs:1191-1200`; `fs/filter/filter_engine_dsp.rs:920-929` |
| 9 | *By design.* Reverb's output cuts and colour filters keep coefficients computed for 48 kHz; at 96 kHz they sit at about twice the labelled frequency | `dsp/src/plugins/fs/reverb/reverb_engine_dsp.rs:838-845, 2139-2146` |
| 10 | **Fixed.** A Comp factory preset from before Multiband loaded in Parallel at the default crossovers from the wide editor, but kept the module's split from the drawer menu and the browser. Every recall path now fills those in (`fs_presets::fill_missing`). Test: `fs_presets.rs` | `ui-egui/src/fs_presets.rs:20-23, 77`; `ui-egui/src/browser/presets.rs:260, 277` |
| 11 | **Fixed** at the plugin. After a rate change Vault kept playing an impulse response made for the old rate until the host sent a new one; it now plays its built-in space meanwhile. The mixer never prepares an instance again at a new rate with an old response, so nothing audible changes in the app. Test: `vault.rs` | `dsp/src/plugins/vault.rs:169-171` |
| 12 | **Fixed.** Vault's pre-delay moved in whole-sample steps with no smoothing, so automating it clicked. A change now crossfades between the two delays over 20 ms. Test: `vault.rs` | `dsp/src/plugins/vault.rs:228, 236` |
| 13 | **Fixed.** Vault trimmed the silence before the first reflection of a loaded impulse response; it now keeps the start and trims only the quiet end. Test: `vault.rs`, `vault/ir.rs` | `dsp/src/plugins/vault/ir.rs:143-145` |
| 14 | **Fixed.** Loading a chain preset cleared the inserts directly and left the old modules' automation lanes behind; it now removes the modules one by one with their lanes. Test: `engine/src/cmd/chain.rs` | `engine/src/cmd/chain.rs:228-229` |
| 15 | **Fixed.** Mod's tempo-synced LFO took its rate from the tempo but not its phase; while the host plays with Sync on, its phase now comes from the song position (a cycle starts on every multiple of the Division). Test: mod `tests.rs` | `dsp/src/plugins/fs/mod_engine/mod.rs:45` |
| 16 | **Fixed.** IIR bells and shelves (CleanQ and the hidden EQs) narrowed and lost gain near the top of the range at 44.1 and 48 kHz. They are now matched to their analog shape up to Nyquist (`biquad::matched`, after Vicanek): at most half the bilinear design's error and under 2 dB in the tested cases; low down they equal the old design, and a shelf whose poles would pass Nyquist keeps it. CleanQ's Ceiling fit keeps the C++ maths. Tests: `biquad.rs`, `clean_q_dsp.rs` | `dsp/src/biquad.rs:47-99`; `dsp/src/plugins/fs/cleanq/clean_q_dsp.rs:422-433` |
| 17 | **Fixed.** Hidden effects that old sessions still load: the stock Limit switch is a brickwall now; a look-ahead change crossfades; the Maximizer limits the true peak (4x interpolated, within about 0.5 dB of the ceiling); the stock gate and compressor link the front pair only (each other channel on its own), so a loud LFE no longer ducks the rest; Lo-Fi's top rate means no reduction at any session rate; Channel Strip resets a section switched back on. Test: `dsp/tests/plugins.rs` | `dsp/src/plugins/dynamics.rs:120-195, 648`; `harmonic.rs:132`; `strip.rs:177-182` |
| 18 | **Fixed.** Time Shift clicked if its delay changed again during a crossfade; a change now waits for the crossfade before it. Test: `dsp/tests/plugins.rs` | `dsp/src/plugins/utility.rs:86-94` |

## Suggested order of work

Estimates are agent-hours for one implementer with tests, as in the earlier sections. No estimate
includes listening tests, which this section needs more than any other.

**1. Fix what is wrong (≈ 14–22 h).** Bugs 1–4, 6, 7 and 10–18. Bugs 5, 8 and 9 wait for the
owner's decision, because fixing them makes the port differ from the original. Bug 2 first, in its
cheapest form: until effects handle every channel, refuse or warn when one is put on a surround
track. Then bug 4.

**2. What all four have (≈ 85–135 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Key input on Comp, with a ducking preset | 4–8 | The routing side is in section 3's list; this is the engine side |
| Effects on surround tracks: run stereo engines per channel pair or per channel, with linked detection for dynamics | 10–16 | **Partly done** with bug 2: per channel (front pair stereo); linked detection for dynamics remains |
| Click-free bypass that keeps delay compensation steady | 4–6 | **Done** with bug 4 |
| A simple channel EQ and compressor in the menu (unhide and tidy the stock ones, or presets of CleanQ and Comp) | 3–6 | Owner's call which |
| Meter plug-ins: spectrum, loudness with true peak, correlation | 8–12 | The measuring code exists |
| Tuner; channel utility (swap, mono, mid/side, per-channel polarity); dither in the menu | 6–10 | |
| Bundled impulse responses for Vault, and impulse-response editing | 6–10 | Imprint can capture; licensing for any third-party set must be checked |
| Rotary speaker, auto-pan, ring modulator and frequency shifter | 10–16 | |
| Vocoder | 8–12 | Needs a key input |
| Bass amp; a pedalboard that holds Dist, Mod and Filter in one window | 10–16 | |
| Beat repeat and a drawn volume shaper / gater | 8–12 | |
| Preset housekeeping: one save path, rename, default preset, A/B everywhere, favourites in the insert menu, hide effects | 8–12 | Fixes bugs 1, 3 and 10 properly |
| Editor basics: scalable windows, type-in on every control, pin on top | 6–10 | |

**3. What most have (≈ 60–95 h).** Real-time pitch correction (Logic, Cubase), 10–16; linear-phase
mode and uncramped bells in CleanQ, 8–12; upward compression, 4–6; gated reverb and a surround
reverb, 10–16; a rack with parallel and band splits and macro knobs (Studio Pro, partly Cubase),
14–22; modulators for any parameter (Cubase, partly Studio Pro), 10–16; a sub-bass generator and
a graphic EQ, 4–7. A mastering assistant is Logic's alone and belongs in the last section.

**Where FreeDAWm is already level or ahead:** ten compressor models with a multiband mode in one
plug-in, a 24-band dynamic EQ with mid/side per band, circuit-level models of vintage EQs, pedals
and amps, a reverb built from a room's geometry, impulse-response capture, match EQ with several
profiles, a resonance suppressor and an automatic balance EQ (none of the four ships either),
de-noise and de-reverb on an insert, and loudness matching against reference tracks.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Feature entries for the four come from
their manuals as known, not from a side-by-side run; treat a ◐ as "check before relying on it".
What counts as "in the box" differs by edition and bundle, most of all for Pro Tools.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | major | First version: built-in effects against Pro Tools, Studio Pro, Logic Pro and Cubase |
| 2026-11-16 | minor | Bug 2 fixed: stereo-only effects process every channel of a surround track |
| 2026-11-16 | minor | Bug 4 fixed: bypass crossfades, keeps the insert's latency and resets the plugin when it comes back |
| 2026-11-16 | minor | Bug 6 mostly fixed: Delay, Mod and ChamberMade report tails that follow their feedback (Filter still to do) |
| 2026-11-16 | minor | Bug 7 fixed: the Delay holds 10.9 s at every sample rate |
| 2026-11-16 | minor | Bug 11 fixed: Vault never plays a response made for another rate |
| 2026-11-16 | minor | Bug 12 fixed: Vault's pre-delay crossfades |
| 2026-11-16 | minor | Bug 13 fixed: Vault keeps the gap before an impulse response's first reflection |
| 2026-11-16 | minor | Bug 15 fixed: Mod's synced LFO locks to the song position |
| 2026-11-16 | minor | Bug 16 fixed: bells and shelves keep their shape near 20 kHz |
| 2026-11-16 | minor | Bug 17 fixed: the hidden stock dynamics, Lo-Fi and Channel Strip |
| 2026-11-16 | minor | Bug 18 fixed: Time Shift changes during a crossfade do not click |
| 2026-11-16 | minor | Bug 14 fixed: replacing a chain removes the old modules' automation lanes |
| 2026-11-16 | minor | Bug 10 fixed: a partial Comp preset sounds the same wherever it is loaded |
| 2026-11-16 | minor | Bug 1 fixed: a user preset saved from a wide editor leaves out what the editor keeps out of presets |
| 2026-11-16 | minor | Bug 3 fixed: the narrow drawer saves the wide editor's preset file and refuses factory names; the claim about the Dist bank and loaded files corrected |
