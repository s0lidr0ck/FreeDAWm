# Four-DAW parity, section 5: built-in effects

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-18 · **Change:** minor (bug fixes and tier-2 items, see the revision history) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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

FreeDAWm registers 60 processors: 56 effects and 4 instruments. The insert menu shows 39 of the
effects:

- **21 FireStation engines** (ported from the owner's C++ suite): Comp, CleanQ, CopyQ, Vint,
  Sat, Tape, Dist, Amp, Twine, Transient, Manners, Sift, Shale, Swale, Filter, Mod, Delay,
  Reverb, ChamberMade, Pitch, LevelUp. They carry 783 factory presets between them.
- **7 FreeDAWm designs:** Rez (resonance suppressor, 111 presets), Lull (gate, 26), Sprawl
  (imager, 16), Vault (convolution reverb), Mend (voice cleaning), Keel (bass amp, 12) and Heft
  (bass enhancer, 7).
- **8 utilities:** Gain, Trim, Invert, DC Offset Removal, Signal Generator, Time Shift, Imprint
  (impulse-response capture), Channel Tool (swap, mono, mid/side, polarity per channel), and
  Dither, back in the menu.
- **4 meters:** Spectrum, Loudness Meter (with true peak, range and a ten-minute history), Phase
  Meter (correlation and goniometer), Tuner.

The insert menu offers the engines and the FreeDAWm designs first (by name), then one Utilities
group: the utilities, Dither and the four meters (the owner's call; Dither has no group of its
own).

**17 older stock effects are hidden from the menu on purpose**, because an engine covers each
one: EQ 1-Band, EQ 7-Band, Compressor/Limiter, Expander/Gate, De-Esser, Maximizer, Channel Strip,
Room and Plate Reverb, Mod Delay, Chorus, Flanger, Phaser, Saturator, Lo-Fi, Rectifier, Pitch
Shifter. They still load from old sessions. The tables below judge what a user can reach
from the menu, and say so where a hidden effect would have filled a gap.

## Summary

| Area | FreeDAWm today | Distance from the four |
|---|---|---|
| A. Dynamics | Ten modelled compressor styles with a key input and look-ahead, multiband, true-peak limiter, transient shaper, two gates | Small (upward compression dropped by the owner) |
| B. EQ and filters | 24-band dynamic EQ with mid/side, five modelled vintage EQs, match EQ, nine circuit filters | Small: no simple channel EQ in the menu (linear-phase CleanQ and a graphic EQ dropped by the owner) |
| C. Saturation and amps | Tape, five saturation circuits, twelve pedals, seven guitar amps with IR cabinets, a bass amp, a bass enhancer | Small (pedalboard dropped by the owner) |
| D. Reverb | Six algorithmic machines, a geometry-built chamber, zero-latency convolution | Medium: **no bundled impulse responses** (the owner is sourcing them); no reverb that spreads a source across surround speakers |
| E. Delay and modulation | Six delay voicings, fourteen modelled modulation circuits | Small: rotary speaker, auto-pan, ring modulator and frequency shifter dropped by the owner |
| F. Pitch and voice | Octaver, doubler, whammy; machine-learning de-noise and de-reverb on an insert | Small: real-time pitch correction and a vocoder dropped by the owner |
| G. Creative and mastering | Rhythmic swells, step-sequenced filter, loudness matching with references | Medium: no glitch or multi-effect; no mastering assistant (beat repeat and a gater dropped by the owner) |
| H. Utility and metering | Gain, polarity, DC, tone generator, delay, imager, channel tool, dither, spectrum, loudness, phase meter, tuner | Small: no phase alignment utility outside Shale |
| I. Presets | 955 factory presets, a searchable browser, chain and track presets, one save path, rename, a default per effect, A/B everywhere | Small |
| J. Editors | Custom editors for 26 effects, live graphs on mixer tiles, sizes 75–200 %, typed values, pinned windows that can follow the selection | Small |
| K. Common behaviour | Latency and tails reported, every parameter automatable and modulatable, every channel processed, a wet/dry mix on every insert | Medium: no racks or macros |

Three things every one of the four did and FreeDAWm did not, at the first review: a compressor
you can key from another track (now Comp); effects that process every channel of a surround
track (now done, bug 2); and meter plug-ins you can put anywhere in a chain (now Spectrum,
Loudness Meter and Phase Meter). All three are in place.

## A. Dynamics

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Compressor with knee, mix, sidechain filter, gain-reduction meter | ● | ● | ● | ● | **Works** (Comp) |
| Modelled characters (FET, opto, VCA, vari-mu) | ● | ● | ● | ● | **Works**: ten styles, three variants each |
| Key input from another track | ● | ● | ● | ● | **Works**: Comp, Lull, Rez and Shale take a key; Comp's Listen plays what its detector hears |
| Look-ahead | ● | ● | ● | ● | **Works** on Comp: Look-ahead 0–10 ms (the header's Ahead pill steps Off, 1, 2, 5, 10 ms): the audio is delayed and reported as latency, the detectors hear the signal as it comes (feed-forward, as from a key; in Multiband each band its own band, in Serial the first lane). Off by default: the C++ path untouched. Test: comp `look_ahead_catches_a_burst_before_it_is_heard` |
| Auto make-up gain | ◐ | ● | ● | ● | **Works** |
| Mid/side and stereo link amount | ◐ | ● | ● | ● | **Works** |
| Multiband compressor | ● | ● | ● | ● | **Works**: three bands inside Comp; crossovers are not linear-phase |
| Upward compression or expansion | ◐ | ◐ | ◐ | ● | **Dropped by the owner** |
| Brickwall limiter aware of inter-sample peaks | ● | ● | ● | ● | **Works** (Twine: 8× true-peak detection) |
| Clipper | ◐ | ◐ | ◐ | ● | **Works** (Twine: hard, soft, tape) |
| Gate and expander with hysteresis, hold, look-ahead, key | ● | ● | ● | ● | **Works** (Lull; Shale for drums with bleed removal) |
| De-esser | ● | ● | ● | ● | **Works** (Sift: split or wide, plus plosives) |
| Transient shaper | ◐ | ◐ | ● | ● | **Works** |
| Dynamic resonance suppressor | ○ | ○ | ○ | ○ | **Works** (Rez), with a key input |
| Ducker (music under voice) | ● | ● | ● | ● | **Works**: Comp keyed from the voice, factory preset "Duck Under Voice" |
| Loudness leveller to a LUFS target | ○ | ○ | ◐ | ○ | **Works** (LevelUp) |

## B. EQ and filters

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Parametric EQ, eight or more bands, steep cuts | ● | ● | ● | ● | **Works** (CleanQ: 24 bands, cuts to 96 dB/oct) |
| Simple channel EQ that opens instantly | ● | ● | ● | ● | **Not planned here**: the owner keeps the hidden stock EQs hidden (EQ 1-Band and 7-Band still load from old sessions); CleanQ is the menu's EQ |
| Dynamic bands | ◐ | ● | ○ | ● | **Works** |
| Mid/side or left/right per band | ◐ | ● | ◐ | ● | **Works** |
| Spectrum behind the curve | ● | ● | ● | ● | **Works** |
| Linear-phase mode | ◐ | ◐ | ● | ● | **Partial**: in the match EQ only; the rest, a linear-phase CleanQ, was dropped by the owner |
| Correct bell shapes near the top of the range at 44.1/48 kHz | ● | ● | ● | ● | **Works**: matched to the analog shape (within about 2 dB) |
| Tilt band | ○ | ○ | ◐ | ◐ | **Partial**: the CleanQ factory preset Tilt (Mix Bus): two gentle shelves at 1 kHz, -3 dB below and +3 dB above; move both gains to tilt. A single tilt control is left out: a new band type would change CleanQ's packed band mode, which every saved CleanQ setting uses. Test: cleanq `the_tilt_preset_tilts` |
| Match EQ | ○ | ○ | ● | ◐ | **Works** (CopyQ: up to four weighted profiles) |
| Modelled vintage EQs | ● | ● | ● | ◐ | **Works** (Vint: two Pultecs, 1073, 550A, SSL E) |
| Graphic EQ | ◐ | ○ | ● | ● | **Dropped by the owner** |
| Automatic balance EQ | ○ | ○ | ○ | ○ | **Works** (Manners) |
| Resonant filter with envelope, LFO and step modulation | ● | ● | ● | ● | **Works** (Filter: nine circuits, 16 steps) |
| Wah | ● | ● | ● | ● | **Works** |
| Formant / vowel filter | ◐ | ○ | ◐ | ◐ | **Not planned here**: a new effect, and the owner builds only Keel and Heft of the new effects in this section |

## C. Saturation, distortion and amps

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Tape machine | ◐ | ◐ | ● | ● | **Works**: four machines, wow, flutter, hiss |
| Tube, transformer and console saturation | ● | ● | ● | ● | **Works** (Sat: five circuits) |
| Overdrive, distortion and fuzz pedals | ● | ● | ● | ● | **Works** (Dist: twelve) |
| Oversampling to control aliasing | ● | ● | ● | ● | **Works**: good in Sat, Tape, Vint, Twine and Amp, and in Dist and Filter since bug 8 (half-band stages). Dist's Oversampling choice runs its circuits at 8x (a third half-band stage, one more sample of latency, twice the processor cost) for the hard clippers (RAT, Fuzz Face, Big Muff) at full drive; it is 4x by default, so saved sessions play the same samples. A test measures what folds back from a 3.66 kHz tone at full drive: at least 6 dB less at 8x for each of the three |
| Bit and sample-rate reduction | ● | ● | ● | ● | **Works**: Lo-Fi (bit depth 1–24, sample rate down to 500 Hz, noise, mix) is offered under Utilities again, the one stock effect the lists show; Vint's Sampler circuit is a fixed 12-bit model |
| Exciter / enhancer | ◐ | ◐ | ● | ◐ | **Works** (Vint Enhancer) |
| Sub-bass generator | ● | ○ | ● | ◐ | **Dropped by the owner** (a plain sub-bass generator) |
| Bass enhancer (harmonics so low notes are heard on small speakers) | ◐ | ○ | ◐ | ○ | **Works** (Heft): harmonics made only from the band below Frequency (40–250 Hz) and added only above it, Character from even to odd, Original Bass keeps, reduces or removes the lows; transparent at zero, no latency, no aliasing (cubic at most, from a band under 250 Hz); every channel. Tests: `heft/tests.rs` |
| Guitar amps | ● | ● | ● | ● | **Works**: seven, solved tube by tube; mono out |
| Bass amp | ● | ● | ● | ● | **Works** (Keel): Tube (the Amp engine's 12AX7 stage), Solid State and Grind preamps at 4x through half-bands, the Amp engine's tone stack with Keel's own parts per voicing, Bright, three cabinets or none blended with a latency-matched DI, Squeeze limiter; 5 samples latency at every rate. Tests: `keel/tests.rs` (latency measured, no allocation, 44.1/48/96 kHz, aliasing at full drive, silence in gives silence out: the tube stage starts from where it settles with no input, which had made a -62 dBFS thump from silence) |
| Cabinets from impulse responses | ● | ● | ● | ● | **Works** |
| Pedalboard in one window | ◐ | ● | ● | ● | **Dropped by the owner**: pedals stay separate inserts |
| Tuner | ● | ● | ● | ● | **Works** (Tuner) |

## D. Reverb

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Room, hall, plate | ● | ● | ● | ● | **Works** (Reverb) |
| Chamber, spring | ◐ | ◐ | ● | ◐ | **Works**; ChamberMade builds the room from its shape, materials and microphones |
| Shimmer | ○ | ○ | ◐ | ● | **Works** |
| Gated / non-linear | ● | ◐ | ● | ◐ | **Dropped by the owner** |
| Convolution with your own impulse responses | ● | ● | ● | ● | **Works** (Vault): zero latency, true stereo, up to 10 s |
| Bundled impulse-response library | ● | ● | ● | ● | **Not planned here**: the owner is sourcing the impulse responses; until then one generated hall |
| Edit the impulse response (reverse, trim, stretch, envelope) | ◐ | ● | ● | ● | **Works**: a decay envelope, and under Vault's response in its drawer Reverse, the trim (from and to, seconds of the file) and the stretch (half to twice as long, read at a changed rate, so longer is also darker) (`mix.module_ir_edit`, one undo step per change). The file is untouched: the edit is stored on the module (`Insert::space_edit`), the response made ready per file and edit, and remade when the session opens. Loading another file starts unedited |
| Capture your own impulse responses | ○ | ● | ● | ○ | **Works** (Imprint) |
| Pre-delay synced to tempo | ◐ | ● | ● | ● | **Works** in Reverb; not in Vault or ChamberMade |
| Freeze, ducking, modulation, damping | ◐ | ◐ | ● | ● | **Works** in Reverb; the duck follows the reverb's own input only |
| Surround or immersive reverb | ● | ◐ | ● | ● | **Partial**: on a surround track Reverb, ChamberMade and Vault reverberate every channel (the front pair in stereo, each other channel through its own instance, bug 2); a reverb that spreads one source across the speakers is a new effect, not planned here (the owner builds only Keel and Heft of the new effects in this section) |

## E. Delay and modulation

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Stereo, ping-pong, dual delay with tempo sync | ● | ● | ● | ● | **Works** |
| Tape and bucket-brigade models | ● | ● | ● | ● | **Works**: six voicings |
| Multi-tap and rhythm patterns | ● | ● | ● | ● | **Works** |
| Reverse, freeze, pitch in the loop, ducking | ◐ | ◐ | ● | ● | **Works** |
| Granular delay | ○ | ○ | ◐ | ◐ | **Not planned here**: a new effect, and the owner builds only Keel and Heft of the new effects in this section |
| Long synced delays at high sample rates | ● | ● | ● | ● | **Works** up to 10.9 s at every rate |
| Chorus, ensemble, flanger, phaser | ● | ● | ● | ● | **Works** (Mod: fourteen modelled circuits) |
| Tremolo, vibrato | ● | ● | ● | ● | **Works** |
| Auto-pan | ◐ | ● | ● | ● | **Dropped by the owner** |
| Rotary speaker | ◐ | ● | ● | ● | **Dropped by the owner** |
| Ring modulator, frequency shifter | ● | ○ | ● | ● | **Dropped by the owner** |
| Modulation locked to the bar | ● | ● | ● | ● | **Works** in Filter, Swale and Mod |

## F. Pitch and voice

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Real-time pitch shifter | ● | ◐ | ● | ● | **Works** (Pitch); no formant preservation, and its delay is not compensated (bug 5) |
| Octaver, doubler, harmony at fixed intervals | ● | ◐ | ● | ● | **Works** |
| Harmony that follows a key | ○ | ○ | ◐ | ◐ | **Not planned here**: needs pitch tracking in Pitch, close to the real-time pitch correction the owner dropped; the owner builds only Keel and Heft of the new effects in this section |
| Real-time automatic pitch correction | ○ | ○ | ● | ● | **Dropped by the owner**; the vocal editor is offline (section 4) |
| Vocoder | ◐ | ● | ● | ● | **Dropped by the owner** |
| De-noise on an insert | ○ | ○ | ○ | ◐ | **Works** (Mend Live): about 50 ms delay, 44.1 and 48 kHz only |
| De-reverb on an insert | ○ | ○ | ○ | ○ | **Works** (Mend Studio): about 2.9 s delay, so for mixing, not tracking |
| De-click, de-hum | ○ | ○ | ○ | ○ | **Not planned here**: none of the four has them on an insert; a new effect, and the owner builds only Keel and Heft of the new effects in this section |
| Stem separation as a live insert | ○ | ○ | ○ | ○ | **Not planned here**: offline only, as in the four (the models need seconds of audio at a time; section 4 has it as a clip process) |

## G. Creative effects and mastering

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Beat repeat / stutter | ◐ | ◐ | ● | ● | **Dropped by the owner** |
| Gater or drawn volume shaper | ◐ | ● | ● | ● | **Partial**: Swale repeats one attack-hold-release shape per beat; the rest, a gater or drawn shaper, was dropped by the owner |
| Step-sequenced multi-effect | ○ | ◐ | ● | ● | **Partial**: the Filter's 16-step cutoff only; a multi-effect is a new effect, not planned here (the owner builds only Keel and Heft of the new effects in this section) |
| Glitch, granular or spectral effects | ◐ | ○ | ● | ◐ | **Not planned here**: new effects (beat repeat dropped by the owner), and the owner builds only Keel and Heft of the new effects in this section |
| Lo-fi and vinyl | ● | ◐ | ● | ● | **Partial**: cassette and 12-bit sampler models, and Lo-Fi under Utilities; a vinyl model is a new effect, not planned here (the owner builds only Keel and Heft of the new effects in this section) |
| Mastering assistant | ○ | ○ | ● | ○ | **Not planned here**: the owner has tabled the mastering page; LevelUp does loudness to a target |
| Loudness match against reference tracks | ○ | ◐ | ◐ | ◐ | **Works** (LevelUp: four references, platform preview) |

## H. Utility and metering

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Gain, trim, polarity | ● | ● | ● | ● | **Works**; polarity flips all channels together |
| Swap channels, mono switch, mid/side encode | ◐ | ● | ● | ● | **Works** (Channel Tool: swap, mono, left or right only, mid/side encode and decode, polarity per channel, gain, DC filter) |
| Stereo width and imaging | ◐ | ● | ● | ● | **Works** (Sprawl: four bands, mono-safe) |
| DC removal, tone generator, sample delay | ● | ● | ● | ● | **Works**; no sweep in the generator |
| Dither with noise shaping | ● | ● | ● | ● | **Works**: Dither is in the menu again (16, 20, 24 bits, first-order shaping), and inside LevelUp |
| Spectrum analyser plug-in | ◐ | ● | ● | ● | **Works** (Spectrum: sum, left, right, mid, side or both sides; slope, speed, peak hold) |
| Loudness and true-peak meter plug-in | ◐ | ● | ● | ● | **Works** (Loudness Meter: the bounce's BS.1770 meter, with loudness range and a short-term history) |
| Phase / correlation meter plug-in | ● | ● | ● | ● | **Works** (Phase Meter: correlation with its lowest held, balance, Lissajous or polar scope) |
| Phase alignment between microphones | ◐ | ◐ | ◐ | ◐ | **Partial**: inside Shale, for drums; a utility of its own is a new effect, not planned here (the owner builds only Keel and Heft of the new effects in this section) |
| Tuner | ● | ● | ● | ● | **Works** (Tuner: A4 415–466 Hz, mute while tuning) |

## I. Presets

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Factory presets | ● | ● | ● | ● | **Works**: 936, on 24 of the 33 listed effects |
| Save and load your own | ● | ● | ● | ● | **Works**: one save path (`preset_house::write_user`) for the wide editors, the drawers and the plug-in windows |
| Rename and delete your own | ● | ● | ● | ● | **Works**: delete in the wide editors; rename from the Presets menu (the file, its star, a default naming it and the modules showing it follow) |
| Browser with search, tags and favourites | ◐ | ● | ● | ● | **Works** |
| Step to next and previous | ● | ● | ● | ● | **Works** in the wide editors |
| Default preset for new instances | ● | ● | ● | ● | **Works**: Presets menu › Start New Inserts From (a factory or user preset), loaded when the insert menu adds the plug-in |
| A/B compare | ● | ● | ● | ● | **Works**: the wide editors' A/B, and Compare in every narrow drawer's and plug-in window's Presets menu |
| Copy settings to another instance | ● | ● | ● | ● | **Works**: the wide editors' Copy and Paste (clipboard), and Copy Settings / Paste Settings in every narrow drawer's and plug-in window's Presets menu (onto the same plug-in on any track, one undo step; what a preset holds). Test: `preset_house` `settings_copy_to_another_instance` |
| Chain presets and track presets | ● | ● | ● | ● | **Works** |
| Favourites and recents in the insert menu | ◐ | ● | ◐ | ● | **Works**: the browser's stars and the last inserted, first in the insert menu |
| Hide effects you do not use | ● | ● | ● | ● | **Works**: insert menu › Show or Hide Effects (search still finds a hidden one) |

## J. Editors

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Purpose-built editor per effect | ● | ● | ● | ● | **Works** for 26 (Keel and Heft among them); Vault and Mend have a drawer with a graph |
| EQ curve with draggable points; compressor curve and history | ● | ● | ● | ● | **Works** |
| Resizable or scalable windows | ◐ | ● | ● | ● | **Works**: View › Plug-in Editors › Plugin Editors Larger / Smaller / at Actual Size (75–200 %; the editor drawn scaled, gestures mapped back), and the slim layout |
| Several editors open; detach to their own windows | ● | ● | ● | ● | **Works** |
| Pin on top; follow the selected track | ● | ● | ◐ | ● | **Works**: a plug-in window's Pin keeps it on top (a detached one at the system's level); its Follow moves it to the same plug-in on the track selected next (its first of that plug-in), pin and detach going with it; a track without it leaves the window where it is. Test: `panels` `a_following_plugin_window_moves_with_the_selection` |
| Type a value into any control | ● | ● | ● | ● | **Works**: Ctrl+click (Cmd+click) a knob or dial in any editor, drawer or tile and type the value as shown (with or without its unit, a choice's name, On/Off) |
| Fine adjust; reset by double-click | ● | ● | ● | ● | **Works** |
| Undo inside the editor | ◐ | ● | ● | ● | **Works**, and every move is also a session undo step |
| Small views on the mixer (EQ curve, gain reduction, pinned knobs) | ◐ | ● | ● | ● | **Works** |

## K. Common behaviour

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Bypass without a click | ● | ● | ● | ● | **Works**: a crossfade; a plugin switched back on starts clean |
| Bypass keeps other tracks in time | ● | ● | ● | ● | **Works**: a bypassed insert keeps its latency |
| Latency reported for delay compensation | ● | ● | ● | ● | **Works**, except Pitch, Sat and Vint (bug 5) |
| Tails included in bounce and freeze | ● | ● | ● | ● | **Works** (bug 6) |
| Every parameter automatable | ● | ● | ● | ● | **Works** |
| Wet/dry mix on every effect | ◐ | ● | ◐ | ◐ | **Works**: every insert slot has a Mix (the slot's menu, 0–100 % wet, `mix.module_mix`): the module's output blended with its input, the input delayed by the module's latency so there is no comb, a change gliding over one block; automatable (a Mix lane under the module in the lane picker, `plugin:m<id>:@mix`, read where the module's input is, written when the Mix moves in a pass). With a key from a later track (the main input waiting for it) the dry side is not delayed by that wait. Tests: `mix/tests/insert_mix.rs` |
| Process every channel of a surround track | ● | ● | ● | ● | **Works**: the front pair as stereo, every other channel through its own mono instance; Comp can link its detection across all of them (Link All Channels), leaving the LFE out |
| Map a hardware controller to a parameter | ● | ● | ● | ● | **Works** (built in section 6, K): right-click a plug-in knob on the chain › Learn MIDI Controller; Setup › MIDI Mappings lists them. Learning from a plug-in's own window or the drawer's knobs is section 6's leftover |
| Modulators (LFO, envelope) for any parameter | ○ | ◐ | ○ | ● | **Works**: up to 8 per track (an insert's menu › Modulators…): an LFO (sine, triangle, saw, square, random; 0.01–40 Hz or locked to the bar, 4 bars to 1/32) or an envelope follower (attack, release; the track's signal entering the inserts), each moving up to 8 plug-in parameters by -100 to 100 % of their range in the plug-in's own taper, around the stored or automated value. The plug-ins that take sample-accurate changes get the movement every 64 samples, others once a block. Not verified by listening. Tests: `mix/tests/modulators.rs`, `engine` `modulators_are_added_set_aimed_and_removed` |
| Rack with parallel, mid/side or band splits; macro knobs | ○ | ● | ○ | ◐ | **Partial**: every insert's wet/dry mix runs one effect in parallel with its own dry signal; a rack with parallel chains, mid/side or band splits and macro knobs is not built (a module hosting chains; design below, about 14–22 h). Chains are a straight line of up to 32 |
| Console channel strip | ● | ● | ○ | ● | **Not planned here**: the owner keeps the hidden stock EQ and compressor hidden, and Channel Strip is the two together (it still loads from old sessions) |
| Processor load shown per effect | ◐ | ● | ◐ | ◐ | **Works**: each module's share of a processor core in the last block (`ModuleMeter::load`), shown on hovering its chain tile or its insert slot; none on the web build. Test: `module_meters` `a_running_module_reports_its_processor_load` |
| Works in the browser build | — | — | — | — | **Partial**: Mend passes audio through; no user presets. The rest is not planned here: the owner has stopped work on the browser build |

## Bugs found on the way

Found while reading; none was reproduced by running the program. Paths are under `crates/`.
Three rows were marked *by design* (5, 8 and 9): the port matched the C++ original on purpose.
The owner has decided they should be fixed; where a fix makes an engine differ from the C++, the
fidelity test renders that engine's C++-faithful build (`plugins::fs::cpp_reference`).

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed.** Saving a user preset from a wide editor stored the parameters the editor keeps out of presets (LevelUp's reference, link group and learn state; CopyQ capture; ChamberMade's Mix and Level; others) and loading restored them. The file now leaves them and Bypass out, and a load ignores them in older files (`editor_kit/presets.rs`: `user_preset_body`, `decode`). Test: `ui-egui/tests/editor_kit.rs` | `ui-egui/src/editor_kit/presets.rs:221-230, 693` |
| 2 | **Fixed.** On a track wider than stereo, 24 effects (every engine, Mend, Vault) processed left and right only; centre, LFE and surrounds passed unprocessed. They now run one instance on the first two channels and a mono instance on each other channel (`dsp/src/plugins/per_channel.rs`), with the same parameters, transport, assets and state. Detection is not linked across them. Test: `per_channel.rs` (a 5.1 surround channel sounds as on a mono track) | `dsp/src/plugins/fs/*/mod.rs` (`channels.min(2)`) |
| 3 | **Fixed.** Saving from the narrow drawer wrote no name or tags, every stored parameter, no CleanQ guides, and let a user preset take a factory preset's name (after which the factory one won). It now writes the wide editor's file (`user_preset_body`: name, tags, the same parameters, CleanQ's guides) and refuses a factory name; its loads skip what the editors keep out of presets (`editors::not_in_presets`). **Claim corrected:** the Dist knob bank and the Amp and Vault loaded files are not in the wide editor's presets either, so the two paths did not differ there. Test: `editor_kit/presets.rs` | `ui-egui/src/panels.rs:971-1029` |
| 4 | **Fixed.** Bypass was an instant switch with no reset, and the track's delay compensation changed in the same block. A bypass now crossfades (the mixer's 10 ms), a bypassed insert keeps its latency (its input goes past it through a delay of that length, so no track moves), and a plugin switched back on is reset and heard once its latency has passed (`insert_step` in `mix/src/lib.rs`). Test: `mix/tests/insert_bypass.rs` | `mix/src/lib.rs:1939, 2721, 2770` |
| 5 | **Fixed.** Pitch reported no latency though its shifted voices ran 6–20 ms late; Sat and Vint also reported none though their oversamplers delay everything. Pitch now delays its dry to the middle of the voices' sweep and reports that (17.5 ms for POG at the default Grain, following Style, Grain and the rate; OC-2 0); Sat and Vint report their half-bands' 4, 5 or 6 samples (Eco, Normal, Insane). Tests measure the delay at 44.1, 48 and 96 kHz (commit 3eeb3a5) | `dsp/src/plugins/fs/pitch/pitch_engine_dsp.rs` (`dry_delay_samples`); `fs/sat/sat_halfband.rs` (`round_trip_latency`); `fs/sat/mod.rs`, `fs/vint/mod.rs` (`latency`) |
| 6 | **Fixed.** Delay reported an 8 s tail whatever the feedback or freeze, Mod 0.25 s, ChamberMade four tape repeats, Filter 1 s, so a bounce or freeze could cut the tail. Delay now reports at least 8 s and as long as its echoes take to fall 80 dB (60 s for Freeze or a runaway loop); Mod adds its flangers' Regen ring; ChamberMade counts tape repeats to -80 dB. Filter reports at least 1 s and as long as its circuit rings to -80 dB at its Reso and the lowest corner its modulation can reach (a Moog at 90% Reso near the bottom: about 6 s; a self-oscillating Moog or MS-20: 60 s), from each circuit's small-signal poles (`ring_seconds`). Tests: delay, mod and chamber `tests.rs`; filter `the_tail_covers_the_ring_at_the_lowest_corner` (the reported tail against the rendered ring) (commit ebfe3a8) | `dsp/src/plugins/fs/delay/mod.rs:251-254`; `fs/mod_engine/mod.rs:398-401`; `fs/filter/mod.rs:303`; `fs/chamber/mod.rs:409-414` |
| 7 | **Fixed.** The Delay buffer was a fixed number of samples (10.9 s at 48 kHz, 2.7 s at 192 kHz), so long synced times were shortened and lost sync at high rates. A line now holds the same 10.9 s at every rate (`line_len`; unchanged at 44.1 and 48 kHz). Test: delay `tests.rs` (a 4 s whole note at 60 BPM keeps its time at 192 kHz) | `dsp/src/plugins/fs/delay/delay_engine_dsp.rs:59-60, 966` |
| 8 | **Fixed (the filter; partly the aliasing).** Dist and Filter oversampled four times behind a Butterworth that rejects only about 16 dB at the original Nyquist and 24–28 dB at 0.55–0.6 fs. They now use the half-band stages Sat and Vint have: flat to 0.45 fs, and anything that would fold below 0.45 fs rejected by more than 100 dB, for a reported 5-sample latency (the old filters delayed about 3 samples, unreported). Gentle settings lose their aliasing (20–30 dB less on the Blues Breaker at full drive and the Klon at low drive); hard clippers at full drive (RAT, Fuzz Face, Big Muff) are unchanged, because their aliasing is made above the 4x Nyquist and only a higher factor would remove it (commit a38e3fc) | `dsp/src/plugins/fs/dist/dist_engine_dsp.rs` (`oversampled`); `fs/filter/filter_engine_dsp.rs` (`process_channel_halfband`); `fs/sat/sat_halfband.rs` (`up4`, `down4`) |
| 9 | **Fixed (the output cuts; the colour part of the claim was wrong).** Reverb's output low and high cuts kept the coefficients the constructor computed for 48 kHz while at their defaults, so at 96 kHz they sat at about twice the labelled frequency (8% low at 44.1 kHz). `prepare` now redesigns them at its rate; at 48 kHz nothing changes. The colour converters were never affected in this port: Color's default (Now) has none, so they are first designed when Color is chosen, at the rate then in use; they were left unchanged (commit a96a792) | `dsp/src/plugins/fs/reverb/reverb_engine_dsp.rs` (`prepare_impl`, `design_low_cut`, `design_high_cut`) |
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

**1. Fix what is wrong (≈ 14–22 h).** Bugs 1–4, 6, 7 and 10–18. Bugs 5, 8 and 9, once by design,
are being fixed at the owner's decision. Bug 2 first, in its
cheapest form: until effects handle every channel, refuse or warn when one is put on a surround
track. Then bug 4.

**2. What all four have (≈ 85–135 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Key input on Comp, with a ducking preset | 4–8 | **Done**: Comp takes the mixer's existing sidechain key (the same picker as Lull, Rez and Shale), with Listen and "Duck Under Voice" (commits 8d6303b, 773ac66) |
| Effects on surround tracks: run stereo engines per channel pair or per channel, with linked detection for dynamics | 10–16 | **Done**: per channel with bug 2; Comp's Link All Channels keys every channel from the loudest one (off by default). The LFE, as the track's channel format names it (the mixer tells each insert, `Plugin::set_lfe_channels`), neither feeds the link nor is turned down by it (the owner's decision; test `comp_link_leaves_the_lfe_of_a_5_1_track_out`). The other dynamics engines (Twine, Transient, Sift, LevelUp) still detect per channel (commits 8d6303b, LFE: see the revision history) |
| Click-free bypass that keeps delay compensation steady | 4–6 | **Done** with bug 4 |
| A simple channel EQ and compressor in the menu (unhide and tidy the stock ones, or presets of CleanQ and Comp) | 3–6 | Owner's call which |
| Meter plug-ins: spectrum, loudness with true peak, correlation | 8–12 | **Done**: Spectrum, Loudness Meter, Phase Meter (commit 5f123a2) |
| Tuner; channel utility (swap, mono, mid/side, per-channel polarity); dither in the menu | 6–10 | **Done** (commit 5f123a2) |
| Bundled impulse responses for Vault, and impulse-response editing | 6–10 | Imprint can capture; licensing for any third-party set must be checked |
| Rotary speaker, auto-pan, ring modulator and frequency shifter | 10–16 | Dropped by the owner |
| Vocoder | 8–12 | Dropped by the owner |
| Bass amp; a pedalboard that holds Dist, Mod and Filter in one window | 10–16 | **Done**: the bass amp (Keel); the pedalboard dropped by the owner. A bass enhancer (Heft) added at the owner's request |
| Beat repeat and a drawn volume shaper / gater | 8–12 | Dropped by the owner |
| Preset housekeeping: one save path, rename, default preset, A/B everywhere, favourites in the insert menu, hide effects | 8–12 | **Done** (see section I) |
| Editor basics: scalable windows, type-in on every control, pin on top | 6–10 | **Done** (see section J) |

**3. What most have (≈ 60–95 h).** Real-time pitch correction (Logic, Cubase), 10–16; linear-phase
mode and uncramped bells in CleanQ, 8–12; upward compression, 4–6; gated reverb and a surround
reverb, 10–16; a rack with parallel and band splits and macro knobs (Studio Pro, partly Cubase),
14–22; modulators for any parameter (Cubase, partly Studio Pro), 10–16; a sub-bass generator and
a graphic EQ, 4–7. A mastering assistant is Logic's alone and belongs in the last section.
**Dropped by the owner:** real-time pitch correction, a linear-phase CleanQ, upward compression,
gated reverb, a plain sub-bass generator and a graphic EQ (with, from tier 2, the rotary speaker,
auto-pan, ring modulator, frequency shifter, vocoder, beat repeat, volume shaper / gater and the
pedalboard). Of the new effects only a bass amp and a bass enhancer (harmonics above a chosen
frequency, so low notes are heard on small speakers) are to be built.

**Where FreeDAWm is already level or ahead:** ten compressor models with a multiband mode in one
plug-in, a 24-band dynamic EQ with mid/side per band, circuit-level models of vintage EQs, pedals
and amps, a reverb built from a room's geometry, impulse-response capture, match EQ with several
profiles, a resonance suppressor and an automatic balance EQ (none of the four ships either),
de-noise and de-reverb on an insert, and loudness matching against reference tracks.

## Designs

### Modulators (round three, built)

- **Model.** `Track::modulators` (at most 8): `Modulator { id, kind: Lfo | Envelope, bypass,
  shape, rate_hz, sync, attack_ms, release_ms, targets }`, each target a chain module, one of its
  parameter ids and a depth of -1 to 1. Saved with the session; an old session has none.
- **Engine.** Each strip keeps a fixed `ModRun` (phase, envelope level, the values at the
  block's start and end). Before the chain runs, an LFO's phase advances (free, or from the song
  position when locked and playing) and an envelope follows the strip's input peak (0 at -60 dBFS,
  1 at 0 dBFS). After a module's block-start values and lanes are applied, every targeted
  parameter is set to its stored or laned value moved by the sum of `depth × value` in the
  plug-in's normalised taper; a plug-in that takes timed changes gets the value every 64 samples
  between the block's two values. The moved parameter is marked unsent, so when a modulator goes
  the plain value is sent again. No allocation.
- **Commands and UI.** `mix.modulator_add`, `_set`, `_target`, `_remove`, each one undo step; the
  Modulators window from an insert's menu.
- **Left.** Modulating send levels, pan or the fader (only plug-in parameters); showing the
  movement on the knobs; a modulator keyed from another track.

### Editing Vault's impulse response (built in round four)

Vault's response is made ready off the audio thread from its file path by the asset system
(`mix.module_ir` → `make_space_ir`), and that path is what a session saves, collects with its
media and caches assets by. Reverse, trim and stretch therefore need: an edit stored beside the
path (`Insert::assets["vaultIrEdit"]`, e.g. `rev=1,start=0.1,end=2.5,stretch=1.2`), applied to
the decoded channels before `SpaceIr::new` (trim in seconds, reverse, stretch as a change of the
file's rate); the asset cache keyed by path and edit together wherever a Vault response is made
or looked up (the engine's command and session loading, the mix's hand-over, the drawer's
picture); and a `mix.module_ir_edit` command with the drawer's controls. About 6–8 h.

### A rack: parallel chains, splits and macros (not built)

A module that hosts up to four chains of up to eight modules each (Juke already hosts effects
on its buses through `Plugin::set_child`), split by: parallel (each chain hears the input), mid
and side, or up to four bands (the Comp's crossovers, linear-phase later); each chain with a
level, pan, mute and solo, summed at the output; eight macro knobs, each mapped to any of its
modules' parameters with a range (the modulators' targets are the same shape). Latency: chains
padded to the longest. The work is mostly in the model (a module with modules inside), the mix
(slots inside slots, their automation and assets), the chain view and presets. About 14–22 h;
until then every insert's wet/dry mix gives one effect in parallel with its own dry signal.

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
| 2026-11-16 | minor | Bug 5 fixed: Pitch delays its dry and reports it as latency; Sat and Vint report their oversampling delay |
| 2026-11-16 | minor | Bug 8 fixed: Dist and Filter oversample through half-band filters and report the latency; hard clippers still alias at 4x |
| 2026-11-16 | minor | Bug 9 fixed: Reverb's output cuts are designed at the rate in use (the colour filters were never stale) |
| 2026-11-16 | minor | Bugs 5, 8 and 9: the commits that fixed them noted in their rows |
| 2026-11-16 | minor | Bug 6 fixed: Filter's tail follows its circuit, Reso and lowest corner |
| 2026-11-16 | minor | Tier 2: Comp takes a key (Listen, "Duck Under Voice") and can link detection across a surround track |
| 2026-11-16 | minor | Tier 2: Spectrum, Loudness Meter, Phase Meter, Tuner and Channel Tool added; Dither back in the menu |
| 2026-11-16 | minor | Bug 6 and the tier-2 rows above: the commits that did them noted |
| 2026-11-16 | minor | The effects the owner dropped marked "dropped by the owner" (rotary speaker, auto-pan, ring modulator, frequency shifter, vocoder, beat repeat, volume shaper / gater, pedalboard, upward compression, linear-phase CleanQ, gated reverb, plain sub-bass generator, graphic EQ, real-time pitch correction) |
| 2026-11-16 | minor | Comp's Link All Channels leaves the LFE out, told by the mixer from the track's channel format; the meters and Dither listed under Utilities in the insert menu |
| 2026-11-16 | minor | Keel (bass amp) and Heft (bass enhancer) added; preset housekeeping and editor basics done |
| 2026-11-16 | minor | Keel: silence in gives silence out (the tube stage's idle point is where it settles, not its exact DC solution) |
| 2026-11-17 | minor | Round three reconciled: controller mapping already built (section 6); the new effects, the bundled impulse responses, the mastering assistant, the hidden stock EQ and channel strip and the browser build marked not planned here, with the owner's reasons |
| 2026-11-17 | minor | Comp gets a look-ahead |
| 2026-11-17 | minor | Lo-Fi offered under Utilities: bit and sample-rate reduction in the menu |
| 2026-11-17 | minor | CleanQ factory preset Tilt |
| 2026-11-17 | minor | Copy Settings and Paste Settings in every drawer and plug-in window |
| 2026-11-17 | minor | Plug-in windows can follow the selected track |
| 2026-11-17 | minor | A wet/dry mix on every insert slot |
| 2026-11-17 | minor | Each module shows its processor load |
| 2026-11-17 | minor | Modulators: LFOs and envelope followers for any plug-in parameter |
| 2026-11-17 | minor | Designs for the modulators, impulse-response editing and a rack; what is left of oversampling, IR editing and the rack stated |
| 2026-11-18 | minor | Round four: Vault's impulse response reversed, trimmed and stretched |
| 2026-11-18 | minor | Round four: Dist's 8x oversampling for the hard clippers, off by default |
