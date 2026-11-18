//! Auditioning audio that is not playing on the timeline: an audio file in the Workspace browser
//! (decoded on a worker thread, so a long file does not hold up the window) and a clip in the
//! Clip List, through the player's preview voice over the mix, at the preview level
//! (`UiState::preview_db`) on the chosen output pair. Clicking what is playing again stops it.
//!
//! A file can be heard at the session tempo (`UiState::preview_tempo`: a loop that says its
//! tempo, see `soundcraft_audio_io::loop_info`, is stretched with its pitch kept on the worker)
//! and in sync (`UiState::preview_sync`: while the session plays, it repeats in time with it from
//! the bar the playhead was in).
//!
//! A MIDI file in the browser is heard too: its notes rendered on a worker at the session tempo
//! where the playhead is (`midi_file`) and played through the same preview voice; a click on it
//! again stops it. It plays on a copy of the selected track's own instrument, with its settings,
//! when that is a built-in one a worker can make ([`track_voice`]: the Subtractive Synth or the
//! Drum Synth), every channel; else on the Subtractive Synth, the drum channel (10) left out.
//!
//! An instrument preset is heard the same way ([`preset`]): a copy of the instrument with the
//! preset's settings plays a short phrase (a rising arpeggio and a held chord; a drum instrument a
//! beat), made on a worker, while the track's own instrument stays as it is.

use crate::SoundApp;
use soundcraft_model::{ClipContent, ClipId, Session, TrackId};
use std::sync::mpsc::{Receiver, channel};

/// The longest a preview plays, in seconds (a file is decoded whole, then cut).
pub const MAX_SECONDS: f64 = 120.0;

/// Audition state on the app.
#[derive(Default)]
pub struct Audition {
    /// A file being decoded: its path and the worker's answer (the audio, its rate, and the
    /// tempo it was stretched from, if it was).
    pending: Option<(String, Receiver<Result<Decoded, String>>)>,
    /// What is playing (a file's path, or `clip:<id>`), until another or Stop.
    pub playing: Option<String>,
}

/// What the worker hands back: planar audio, its rate, and the file's own tempo when it was
/// stretched to the session's.
type Decoded = (Vec<Vec<f32>>, u32, Option<f64>);

/// `channels` (at `rate`) stretched, pitch kept, from the tempo `info` says to `target` bpm;
/// unchanged (and `None`) when it says none or it is already there.
pub fn at_tempo(
    channels: Vec<Vec<f32>>,
    rate: u32,
    info: &soundcraft_audio_io::loop_info::LoopInfo,
    target: Option<f64>,
) -> (Vec<Vec<f32>>, Option<f64>) {
    match (info.bpm, target.filter(|t| t.is_finite() && *t > 0.0)) {
        (Some(bpm), Some(t)) if (bpm - t).abs() > 0.01 => {
            let out = soundcraft_dsp::offline::time_stretch(&channels, bpm / t, rate as f32);
            (out, Some(bpm))
        }
        _ => (channels, None),
    }
}

/// Where a synced preview of `frames` (at `rate`) sits: from the start of the bar at `at`,
/// repeating every its length in session samples.
pub fn sync_at(s: &soundcraft_model::Session, at: soundcraft_time::Samples, frames: usize, rate: u32) -> Option<soundcraft_playback::PreviewSync> {
    let period = (frames as f64 * s.sample_rate.as_f64() / f64::from(rate.max(1))).round() as i64;
    let bar = s.tempo.bar_beat_at(at.max(0), s.sample_rate).bar;
    let origin = s.tempo.tick_to_samples(s.tempo.bar_start_tick(bar), s.sample_rate);
    (period > 0).then_some(soundcraft_playback::PreviewSync { origin, period })
}

/// The planar audio at `rate` cut to [`MAX_SECONDS`] and set to the preview level.
fn prepared(mut channels: Vec<Vec<f32>>, rate: u32, db: f32) -> Vec<Vec<f32>> {
    let max = (MAX_SECONDS * f64::from(rate.max(1))) as usize;
    let g = if db.is_finite() { 10f32.powf(db.clamp(-60.0, 12.0) / 20.0) } else { 1.0 };
    for ch in &mut channels {
        ch.truncate(max);
        ch.iter_mut().for_each(|x| *x *= g);
    }
    channels
}

fn play(app: &mut SoundApp, what: String, channels: Vec<Vec<f32>>, rate: u32, synced: bool) {
    let channels = prepared(channels, rate, app.ui.preview_db);
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    let sync = if synced && app.ui.preview_sync { sync_at(app.engine.session(), app.position(), frames, rate) } else { None };
    if let Some(p) = &app.player {
        p.preview_synced(channels, rate, app.ui.preview_output, sync);
    }
    app.audition.playing = Some(what);
}

/// Stops what is auditioning.
pub fn stop(app: &mut SoundApp) {
    if app.audition.playing.take().is_some()
        && let Some(p) = &app.player
    {
        p.preview(Vec::new(), 48_000);
    }
    app.audition.pending = None;
}

/// Auditions the audio file at `path`, or stops it when it is what plays.
pub fn file(app: &mut SoundApp, path: &str) {
    if app.audition.playing.as_deref() == Some(path) || app.audition.pending.as_ref().is_some_and(|(p, _)| p == path) {
        stop(app);
        return;
    }
    stop(app);
    let (tx, rx) = channel();
    let owned = path.to_string();
    // The session tempo where the playhead is, for a file heard in tempo.
    let target = app.ui.preview_tempo.then(|| {
        let s = app.engine.session();
        s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(app.position().max(0), s.sample_rate))
    });
    let spawned = std::thread::Builder::new().name("audition".into()).spawn(move || {
        let ext = std::path::Path::new(&owned).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let got = std::fs::read(&owned).map_err(|e| e.to_string()).and_then(|bytes| {
            let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|e| e.to_string())?;
            let info = soundcraft_audio_io::loop_info::loop_info(&bytes, &owned);
            let rate = buf.sample_rate;
            let mut ch = buf.channels;
            // Cut before stretching: only what can play is worked on.
            let max = (MAX_SECONDS * f64::from(rate.max(1))) as usize;
            ch.iter_mut().for_each(|c| c.truncate(max));
            let (ch, from) = at_tempo(ch, rate, &info, target);
            Ok((ch, rate, from))
        });
        let _ = tx.send(got);
    });
    match spawned {
        Ok(_) => app.audition.pending = Some((path.to_string(), rx)),
        Err(e) => app.ui.status = format!("Cannot audition: {e}"),
    }
}

/// A built-in instrument a preview makes on a worker: its plugin id and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    pub plugin: String,
    pub params: Vec<(String, f32)>,
    /// A built-in synth's modulation patch, compiled (`Plugin::set_prepared_state`); empty: none.
    pub prepared: Vec<f64>,
}

/// The built-in instruments a preview can copy: they need no samples or files loaded.
pub const COPYABLE: [&str; 2] = ["subtractive_synth", "drum_synth"];

/// The instrument the preview falls back on.
const FALLBACK: &str = "subtractive_synth";

/// A copy of `track`'s instrument for a preview, when it is one of [`COPYABLE`].
pub fn track_voice(s: &Session, track: TrackId) -> Option<Voice> {
    let ins = s.track(track)?.instrument.as_ref()?;
    COPYABLE.contains(&ins.plugin.as_str()).then(|| Voice {
        plugin: ins.plugin.clone(),
        params: ins.params.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        // Its patch too: what it was made ready as for the audio thread.
        prepared: ins.prepared.as_ref().map(|p| p.data.to_vec()).unwrap_or_default(),
    })
}

/// The selected track's voice: the first selected track with a copyable instrument.
fn selected_voice(app: &SoundApp) -> Option<Voice> {
    let s = app.engine.session();
    s.edit.selected_tracks.iter().find_map(|t| track_voice(s, *t))
}

/// Auditions the MIDI file at `path` at the session tempo on the selected track's instrument (or
/// the Subtractive Synth), or stops it when it is what plays.
pub fn midi_file(app: &mut SoundApp, path: &str) {
    if app.audition.playing.as_deref() == Some(path) || app.audition.pending.as_ref().is_some_and(|(p, _)| p == path) {
        stop(app);
        return;
    }
    stop(app);
    let (tx, rx) = channel();
    let owned = path.to_string();
    let s = app.engine.session();
    let bpm = s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(app.position().max(0), s.sample_rate));
    let rate = s.sample_rate.hz();
    let voice = selected_voice(app);
    let spawned = std::thread::Builder::new().name("audition-midi".into()).spawn(move || {
        let got = std::fs::read(&owned).map_err(|e| e.to_string()).and_then(|bytes| render_midi(&bytes, bpm, rate, voice.as_ref()));
        let _ = tx.send(got.map(|ch| (ch, rate, None)));
    });
    match spawned {
        Ok(_) => app.audition.pending = Some((path.to_string(), rx)),
        Err(e) => app.ui.status = format!("Cannot audition: {e}"),
    }
}

/// Note events for [`render`]: (sample, on, pitch, velocity), offs before ons at one sample.
type Events = Vec<(usize, bool, u8, u8)>;

/// A Standard MIDI File's notes played at `bpm` by `voice` (else by the Subtractive Synth, the
/// drum channel (10) left out: the synth would play its kit as pitches), as stereo at `rate`, cut
/// to [`MAX_SECONDS`] (and a second of release after the last note).
pub fn render_midi(bytes: &[u8], bpm: f64, rate: u32, voice: Option<&Voice>) -> Result<Vec<Vec<f32>>, String> {
    let smf = soundcraft_midi::read_smf(bytes).map_err(|e| e.to_string())?;
    let bpm = if bpm.is_finite() && bpm > 1.0 { bpm } else { 120.0 };
    let per_tick = f64::from(rate) * 60.0 / bpm / f64::from(soundcraft_midi::PPQ);
    let max = (MAX_SECONDS * f64::from(rate)) as usize;
    let mut events: Events = Vec::new();
    let drums = voice.is_some();
    for n in smf.tracks.iter().flat_map(|t| t.sequence.notes.iter()).filter(|n| (drums || n.channel != 9) && !n.muted && n.start >= 0) {
        let a = (n.start as f64 * per_tick) as usize;
        if a >= max {
            continue;
        }
        let b = ((n.end() as f64 * per_tick) as usize).clamp(a + 1, max);
        events.push((a, true, n.pitch, n.velocity.max(1)));
        events.push((b, false, n.pitch, 0));
        if events.len() > 400_000 {
            break;
        }
    }
    render(events, rate, max, voice)
}

/// `events` played by `voice` (else the Subtractive Synth) at `rate`, a second of release after
/// the last.
fn render(mut events: Events, rate: u32, max: usize, voice: Option<&Voice>) -> Result<Vec<Vec<f32>>, String> {
    if events.is_empty() {
        return Err("no notes to play".into());
    }
    events.sort_by_key(|e| (e.0, e.1));
    let len = events.iter().map(|e| e.0).max().unwrap_or(0).saturating_add(rate as usize).min(max + rate as usize);
    const BLOCK: usize = 256;
    let id = voice.map_or(FALLBACK, |v| v.plugin.as_str());
    let mut inst = soundcraft_dsp::create(id).ok_or_else(|| format!("cannot make {id}"))?;
    inst.prepare(rate as f32, BLOCK, 2);
    for (k, v) in voice.map(|v| v.params.as_slice()).unwrap_or_default() {
        inst.set_param(k, *v);
    }
    if let Some(v) = voice.filter(|v| !v.prepared.is_empty()) {
        inst.set_prepared_state(&v.prepared);
    }
    inst.set_offline(true);
    let mut out = vec![Vec::with_capacity(len), Vec::with_capacity(len)];
    let mut io = vec![vec![0.0f32; BLOCK]; 2];
    let mut next = 0;
    let mut at = 0;
    while at < len {
        let n = BLOCK.min(len - at);
        while let Some(&(t, on, pitch, vel)) = events.get(next) {
            if t >= at + n {
                break;
            }
            if on {
                inst.note_on(t - at, pitch, vel);
            } else {
                inst.note_off(t - at, pitch);
            }
            next += 1;
        }
        io.iter_mut().for_each(|c| c.iter_mut().for_each(|x| *x = 0.0));
        inst.process(&mut io, n);
        for (o, c) in out.iter_mut().zip(&io) {
            o.extend_from_slice(c.get(..n).unwrap_or(&[]));
        }
        at += n;
    }
    Ok(out)
}

/// The phrase a preset preview plays at `rate`: a rising arpeggio and a held chord around middle
/// C at 120 bpm (a drum instrument: a beat of kick, snare and hats).
pub fn phrase(plugin: &str, rate: u32) -> Events {
    let beat = rate as usize / 2;
    let mut ev = Events::new();
    let mut note = |start: usize, len: usize, pitch: u8, vel: u8| {
        ev.push((start, true, pitch, vel));
        ev.push((start + len.max(1), false, pitch, 0));
    };
    if plugin == "drum_synth" {
        for i in 0..8 {
            note(i * beat / 2, beat / 4, 42, if i % 2 == 0 { 100 } else { 70 });
        }
        for i in [0, 4] {
            note(i * beat / 2, beat / 4, 36, 110);
        }
        for i in [2, 6] {
            note(i * beat / 2, beat / 4, 38, 105);
        }
    } else {
        for (i, p) in [48u8, 52, 55, 60].into_iter().enumerate() {
            note(i * beat / 2, beat / 2, p, 100);
        }
        for p in [60u8, 64, 67] {
            note(2 * beat, 2 * beat, p, 90);
        }
    }
    ev
}

/// Auditions `voice` (an instrument preset's settings) playing [`phrase`], or stops it when it is
/// what plays. `name` tells previews apart.
pub fn preset(app: &mut SoundApp, voice: Voice, name: &str) {
    let what = format!("preset:{}:{name}", voice.plugin);
    if app.audition.playing.as_deref() == Some(what.as_str()) || app.audition.pending.as_ref().is_some_and(|(p, _)| *p == what) {
        stop(app);
        return;
    }
    stop(app);
    let (tx, rx) = channel();
    let rate = app.engine.session().sample_rate.hz();
    let spawned = std::thread::Builder::new().name("audition-preset".into()).spawn(move || {
        let events = phrase(&voice.plugin, rate);
        let got = render(events, rate, (MAX_SECONDS * f64::from(rate)) as usize, Some(&voice));
        let _ = tx.send(got.map(|ch| (ch, rate, None)));
    });
    match spawned {
        Ok(_) => app.audition.pending = Some((what, rx)),
        Err(e) => app.ui.status = format!("Cannot audition: {e}"),
    }
}

/// Auditions `clip` as it sounds (its material with clip gain), or stops it when it is what plays.
pub fn clip(app: &mut SoundApp, id: ClipId) {
    let what = format!("clip:{}", id.0);
    if app.audition.playing.as_deref() == Some(what.as_str()) {
        stop(app);
        return;
    }
    stop(app);
    let s = app.engine.session_arc();
    let Some((_, c)) = s.find_clip(id) else { return };
    let ClipContent::Audio { offset, .. } = c.content else {
        app.ui.status = "Only audio clips can be auditioned".into();
        return;
    };
    let rate = s.sample_rate.hz();
    let n = usize::try_from(c.source_span(c.length)).unwrap_or(0).min((MAX_SECONDS * f64::from(rate)) as usize);
    let Some(mut channels) = soundcraft_mix::clip_source_window(&s, c, offset, n) else { return };
    for ch in &mut channels {
        for (k, x) in ch.iter_mut().enumerate() {
            *x *= c.gain_at(c.timeline_span(k as i64));
        }
    }
    play(app, what, channels, rate, false);
}

/// Starts a decoded file once it is ready (each frame), and forgets a preview that ended.
pub fn poll(app: &mut SoundApp) {
    let Some((path, rx)) = &app.audition.pending else { return };
    let path = path.clone();
    match rx.try_recv() {
        Ok(Ok((channels, rate, from))) => {
            app.audition.pending = None;
            if let Some(bpm) = from {
                app.ui.status = format!("Previewing at the session tempo (the file's is {bpm:.1} bpm)");
            }
            play(app, path, channels, rate, true);
        }
        Ok(Err(e)) => {
            app.audition.pending = None;
            app.ui.status = format!("Cannot audition {path}: {e}");
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => app.audition.pending = None,
    }
}

/// Whether a file is still being read for audition.
pub fn loading(app: &SoundApp) -> bool {
    app.audition.pending.is_some()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_loop_is_heard_at_the_session_tempo_from_the_bar() {
        use soundcraft_audio_io::loop_info::LoopInfo;
        let ch = vec![vec![0.1f32; 48_000]];
        let at60 = LoopInfo { bpm: Some(60.0), ..LoopInfo::default() };
        let (out, from) = super::at_tempo(ch.clone(), 48_000, &at60, Some(120.0));
        assert_eq!(from, Some(60.0));
        assert!((out[0].len() as i64 - 24_000).abs() < 64, "{}", out[0].len());
        // No tempo said, or no target: as it is.
        assert_eq!(super::at_tempo(ch.clone(), 48_000, &LoopInfo::default(), Some(120.0)).0[0].len(), 48_000);
        assert_eq!(super::at_tempo(ch, 48_000, &at60, None).1, None);
        // In sync: from the start of the playhead's bar, every the loop's length.
        let s = soundcraft_model::Session::default();
        let bar = s.tempo.tick_to_samples(s.tempo.bar_start_tick(3), s.sample_rate);
        let sync = super::sync_at(&s, bar + 1000, 24_000, 48_000).unwrap();
        assert_eq!((sync.origin, sync.period), (bar, 24_000 * i64::from(s.sample_rate.hz()) / 48_000));
    }

    #[test]
    fn a_midi_file_is_rendered_on_the_synth_at_the_tempo_without_its_drums() {
        use soundcraft_midi::{Note, Sequence, Smf, SmfTrack, write_smf};
        let n =
            |pitch, channel, start| Note { probability: 100, pitch, velocity: 100, muted: false, release_velocity: 64, channel, start, length: 960 };
        let file = |notes| {
            let t = SmfTrack { name: "T".into(), sequence: Sequence { notes, ctrls: Vec::new(), sysex: Vec::new() } };
            write_smf(&Smf { format: 1, tracks: vec![t], ..Default::default() }).unwrap()
        };
        let bytes = file(vec![n(60, 0, 0), n(64, 0, 1920)]);
        // Three beats at 120 bpm, then a second of release.
        let at120 = super::render_midi(&bytes, 120.0, 48_000, None).unwrap();
        assert_eq!(at120[0].len(), 72_000 + 48_000);
        let at60 = super::render_midi(&bytes, 60.0, 48_000, None).unwrap();
        assert_eq!(at60[0].len(), 144_000 + 48_000);
        let energy = |c: &[f32]| c.iter().map(|x| x * x).sum::<f32>();
        assert!(energy(&at120[0][..24_000]) > 0.01, "the first note sounds");
        assert!(energy(&at120[0][60_000..72_000]) > 0.01, "the second note, two beats later");
        assert!(super::render_midi(&file(vec![n(36, 9, 0)]), 120.0, 48_000, None).is_err(), "drums only: nothing to play");
        assert!(super::render_midi(b"not midi", 120.0, 48_000, None).is_err());
        // On the track's own instrument, with its settings: the Drum Synth plays the drum channel.
        let drums = super::Voice { plugin: "drum_synth".into(), params: Vec::new(), prepared: Vec::new() };
        let kit = super::render_midi(&file(vec![n(36, 9, 0)]), 120.0, 48_000, Some(&drums)).unwrap();
        assert!(energy(&kit[0][..24_000]) > 0.01, "the kick sounds");
        // A synth copy with a closed filter is darker than the default synth: the settings reach it.
        let dark = super::Voice { plugin: "subtractive_synth".into(), params: vec![("cutoff".into(), 120.0)], prepared: Vec::new() };
        let dull = super::render_midi(&bytes, 120.0, 48_000, Some(&dark)).unwrap();
        let hf = |c: &[f32]| c.windows(2).map(|w| (w[1] - w[0]) * (w[1] - w[0])).sum::<f32>() / energy(c).max(1e-9);
        let same = super::Voice { plugin: "subtractive_synth".into(), params: Vec::new(), prepared: Vec::new() };
        assert_eq!(super::render_midi(&bytes, 120.0, 48_000, Some(&same)).unwrap(), at120, "a copy at its defaults is the synth");
        let (a, b) = (hf(&dull[0][..24_000]), hf(&at120[0][..24_000]));
        assert!(a < b * 0.8, "the copy plays with the track's cutoff: {a} against {b}");
    }

    #[test]
    fn a_preset_preview_plays_a_phrase_on_a_copy_with_the_presets_settings() {
        let energy = |c: &[f32]| c.iter().map(|x| x * x).sum::<f32>();
        for plugin in super::COPYABLE {
            let voice = super::Voice { plugin: plugin.into(), params: Vec::new(), prepared: Vec::new() };
            let out = super::render(super::phrase(plugin, 48_000), 48_000, 48_000 * 10, Some(&voice)).unwrap();
            assert!(out[0].len() > 48_000 * 2 && out[0].len() < 48_000 * 5, "{plugin}: a short phrase");
            assert!(energy(&out[0]) > 0.1, "{plugin} sounds");
        }
        // The same phrase sounds different with another preset's settings.
        let info = soundcraft_dsp::plugin_info("subtractive_synth").unwrap();
        let presets = soundcraft_dsp::plugins::fs::factory_presets("subtractive_synth");
        assert!(!presets.is_empty());
        let plain = super::render(super::phrase("subtractive_synth", 48_000), 48_000, 480_000, None).unwrap();
        let mut differs = 0;
        for (name, json) in presets.iter().take(3) {
            let voice = crate::fs_presets::preset_voice(info, name, json).unwrap();
            assert!(!voice.params.is_empty(), "{name}");
            let got = super::render(super::phrase("subtractive_synth", 48_000), 48_000, 480_000, Some(&voice)).unwrap();
            if got[0].len() != plain[0].len() || got[0].iter().zip(&plain[0]).any(|(a, b)| (a - b).abs() > 1e-4) {
                differs += 1;
            }
        }
        assert_eq!(differs, 3);
    }

    #[test]
    fn previews_are_cut_and_set_to_the_preview_level() {
        let out = super::prepared(vec![vec![1.0; 300]], 1, -6.0);
        assert_eq!(out[0].len(), 120);
        assert!((out[0][0] - 0.501).abs() < 0.01);
    }
}
