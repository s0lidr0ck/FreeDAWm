//! Local ML audio commands (desktop only): stem separation, karaoke, drum split, vocal cleanup,
//! and the model list. See `crate::ml`.

use super::*;
use crate::cmd;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "ml.separate",
            "Separate Stems",
            ["Track"],
            None,
            "{track? | clip?, stems?: [\"vocals\", \"drums\", \"bass\", \"other\"] (default all), mute_original?: bool (default false), quality?: fast (default, one pass) | high (2 shifted passes averaged at overlap 0.5: smoother stems, about twice the time), device?, models_dir?} — HTDemucs v4: new tracks `<track> <Stem>` directly under the source at its position, one undo step. See the shared source params: track? (id or name; default: the selected clip, else the first selected track), clip?, device?: auto | gpu | cpu, models_dir?",
            has_selection,
            |e, p| crate::ml::run_command(e, "ml.separate", p)
        ),
        cmd!(
            "ml.karaoke",
            "Make Karaoke",
            ["Track"],
            None,
            "{track? | clip?, keep_backing?: bool (default false: backing vocals get their own track; true: they stay in the Instrumental), mute_original?: bool (default true), quality?: fast | high (the HTDemucs step), device?, models_dir?} — HTDemucs, then the Mel-Band RoFormer karaoke model on the vocals: `Lead Vocal` and `Instrumental` tracks (+ `Backing Vocals`) under the source",
            has_selection,
            |e, p| crate::ml::run_command(e, "ml.karaoke", p)
        ),
        cmd!(
            "ml.drums",
            "Split Drums",
            ["Track"],
            None,
            "{track? | clip?, from_mix?: bool (default false: the source is a drum track; true: separate the drums out of a mix first), mute_original?: bool (default false), quality?: fast | high (the HTDemucs step, with from_mix), device?, models_dir?} — MDX23C DrumSep: Kick, Snare, Toms, Hi-Hat, Ride, Crash tracks under the source",
            has_selection,
            |e, p| crate::ml::run_command(e, "ml.drums", p)
        ),
        cmd!(
            "ml.clean",
            "Clean Vocal",
            ["Track"],
            None,
            "{clip? | clips? | track? (every audio clip of it), mode?: vocal (default: Smule Renaissance Small alone, noise and reverb together) | speech (DeepFilterNet 3 de-noise, then Renaissance de-reverb), mix?: Vocal's Amount, wet % 0..100 (default 100), amount_db?: Speech's de-noise attenuation limit 0..100 dB (default 24; 100 = unlimited), dereverb?: bool (Speech; default true), denoise?: bool (old: without `mode`, dereverb → vocal, denoise alone → speech without de-reverb), device?, models_dir?} — one fixed make-up gain (gated loudness matched, ±12 dB, output peak at most -1 dBFS) whenever Renaissance runs; old params without `mode`: denoise+dereverb or dereverb alone → vocal, denoise alone → speech without de-reverb; replaces each clip with its cleaned audio, like Process (the clip keeps its gain and fades; Undo restores the original)",
            has_selection,
            |e, p| crate::ml::run_command(e, "ml.clean", p)
        ),
        cmd!(
            "ml.midi",
            "Convert to MIDI",
            ["Track"],
            None,
            "{clip? | clips? | track? (every audio clip of it), model?: general (Basic Pitch, default) | piano (Kong, with sustain pedal) | drums (ADTOF: GM 35 kick, 38 snare, 47 toms, 42 hi-hat, 49 cymbals) | vocal (GAME, one note at a time), onset_threshold?: 0..1 (lower = more notes; default the model's own: general 0.5, piano 0.3; drums scales its five thresholds, 0.25 = as published), frame_threshold?: 0..1 (general, default 0.3), min_note_ms?: 0..10000 (default: general 127.7, else 0), pitch_bends?: bool (default false; general and vocal), pedal?: bool (default true; piano: CC64), drum_thresholds?: [kick, snare, toms, hihat, cymbals] each 0..1 (drums; default the published five scaled by onset_threshold), isolate?: none | drums | other | vocals | bass (separate that HTDemucs stem out first, at `quality`, and transcribe it; default from the model: drums → drums, piano → other, vocal → vocals, general → none unless the track name says bass or vocal), quality?: fast | high (the isolation step), device?, models_dir?} — a new instrument track `<track> MIDI` directly under the source track (Subtractive Synth; piano: voiced as keys; drums: Drum Synth on channel 10, each hit's velocity from the source's level in the 30 ms after it, the loudest of its class at 120), one MIDI clip per source clip at its position, notes on the session's tempo map; one undo step",
            has_selection,
            |e, p| crate::ml::run_command(e, "ml.midi", p)
        ),
        cmd!(
            "ml.chords",
            "Detect Chords",
            ["Track"],
            None,
            "{track? | clip?, grid?: half_bar (default) | bar, mode?: replace (default: the lane's chords over the source's range go) | fill (only where the lane is empty), key?: auto (default: estimated from the chords and set on the band when the lane holds no other chords) | keep (the band's key) | C..B | 0-11, scale?: major | minor (with a key), bass?: bool (guess inversions; default on for a stereo track not named after one instrument, i.e. a full mix), device?, models_dir?} — BTC chord recognition: the chords onto the Band's chord lane at the source's position on the tempo map, snapped to the grid with repeats merged; shows the Chords ruler; one undo step",
            has_selection,
            |e, p| crate::ml::run_command(e, "ml.chords", p)
        ),
        cmd!(
            "ml.lyrics",
            "Transcribe Lyrics",
            ["Track"],
            None,
            "{track? | clip?, language?: en (default) | auto, prompt?: text (names and words to expect), models_dir?} — Whisper large-v3 (whisper.cpp, CPU): timed lyric lines and words on the source track at the source's position (replacing its lines in that range); feed a vocal track, not a mix; one undo step. Export with file.export_lyrics / file.export_lyrics_text; MIDI export writes them as lyric events",
            has_selection,
            |e, p| crate::ml::run_command(e, "ml.lyrics", p)
        ),
        cmd!(
            "ml.text_midi",
            "Generate MIDI from Text",
            ["Track"],
            None,
            "{prompt: text (what to compose: style, instruments, key, meter, tempo, mood — e.g. \"upbeat jazz with saxophones and drums, B-flat major, 4/4\"), seconds?: 1..100 (default 30; the piece stops at the first note after), start?: position (default the edit cursor), continue_from?: MIDI clip id (the piece starts with that clip's first 170 notes, from the clip's start, and the model carries on; they are not copied, only what it adds is written; start defaults to the clip's start), instrument?: 0..127 | \"drums\" (what the continued notes are played on: default drums when they are all on channel 10, else the clip's first program change, else drums for a track named drum/kit/perc/beat, else 0 piano), seed?: integer (default random; the same seed, prompt and settings give the same notes), temperature?: 0.1..2 (default 1), top_p?: 0.05..1 (default 0.98), max_tokens?: 3..4095 (default 2046 = 682 notes), device?, models_dir?} — MIDI-LLM (Llama 3.2 1B): one new instrument track per General MIDI instrument in the piece, named after it (drums: Drum Synth on channel 10; the rest: Subtractive Synth, a program change on each clip keeps the General MIDI instrument for MIDI export and GM plugins), one MIDI clip each at `start`, notes on the session's tempo map; one undo step. Result: made (track, name, clip, program, notes), seed",
            always,
            |e, p| crate::ml::run_command(e, "ml.text_midi", p)
        ),
        cmd!(
            "ml.song",
            "Generate Song",
            ["Track"],
            None,
            "{engine?: yue2 (default) | acestep, server?: http address (default http://127.0.0.1:8087 for yue2, http://127.0.0.1:8080 for acestep), style: text (YuE2's style tags, ACE-Step's caption), lyrics?: text with [Verse]/[Chorus] tags, instrumental?: bool, score?: ABC (YuE2: sing this score instead of writing one), plan?: full | melody | off (YuE2's score planning, default full), seconds?: 10..600 (default 60; the model may end sooner), seed?: integer, bpm?: 30..300 (ACE-Step; default the planner's), key?: \"C major\" (ACE-Step), planner?: bool (ACE-Step's 5 Hz planner first, default true), start?: position (default the edit cursor)} — a whole song (vocals and accompaniment, 48 kHz stereo) from YuE2 (yue2.cpp's yue-server) or ACE-Step 1.5 (acestep.cpp's ace-server), running on this computer; FreeDAWm only talks to the server. The song lands on a new audio track `<engine> Song` at `start`; with YuE2 the score it wrote (returned as `score.abc`) also lands as MIDI: one instrument track per voice (`YuE2 Score Vocal`, `YuE2 Score Ins`) and `YuE2 Score Chords`, timed from the score's tempo onto the session's tempo map. One undo step. Result: made, seed, score {abc, bpm, meter}, song {bpm, key, lyrics} (ACE-Step's planner fills these), request (what reproduces the song)",
            always,
            |e, p| crate::ml::run_command(e, "ml.song", p)
        ),
        cmd!(
            query "ml.models",
            "ML Models",
            [],
            None,
            "{models_dir?} — the model folder and each model: id, name, job, status (installed | built in | not installed | broken), bytes, licence (the card's note), licence_status (short: \"MIT\", \"Licence not stated by the author — personal use\"…), source",
            always,
            crate::ml::models_command
        ),
    ]
}
