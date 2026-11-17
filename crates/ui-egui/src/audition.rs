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
//! A MIDI file in the browser is heard too: its notes rendered on a worker with the Subtractive
//! Synth at the session tempo where the playhead is (`midi_file`), every track and channel but the
//! drum channel (10), and played through the same preview voice; a click on it again stops it.

use crate::SoundApp;
use soundcraft_model::{ClipContent, ClipId};
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

/// Auditions the MIDI file at `path` on the Subtractive Synth at the session tempo, or stops it
/// when it is what plays.
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
    let spawned = std::thread::Builder::new().name("audition-midi".into()).spawn(move || {
        let got = std::fs::read(&owned).map_err(|e| e.to_string()).and_then(|bytes| render_midi(&bytes, bpm, rate));
        let _ = tx.send(got.map(|ch| (ch, rate, None)));
    });
    match spawned {
        Ok(_) => app.audition.pending = Some((path.to_string(), rx)),
        Err(e) => app.ui.status = format!("Cannot audition: {e}"),
    }
}

/// A Standard MIDI File's notes played by the Subtractive Synth at `bpm`, as stereo at `rate`, cut
/// to [`MAX_SECONDS`] (and a second of release after the last note). The drum channel (10) is
/// left out: the synth would play its kit as pitches.
pub fn render_midi(bytes: &[u8], bpm: f64, rate: u32) -> Result<Vec<Vec<f32>>, String> {
    use soundcraft_dsp::Plugin;
    let smf = soundcraft_midi::read_smf(bytes).map_err(|e| e.to_string())?;
    let bpm = if bpm.is_finite() && bpm > 1.0 { bpm } else { 120.0 };
    let per_tick = f64::from(rate) * 60.0 / bpm / f64::from(soundcraft_midi::PPQ);
    let max = (MAX_SECONDS * f64::from(rate)) as usize;
    // (sample, on, pitch, velocity), offs before ons at the same sample.
    let mut events: Vec<(usize, bool, u8, u8)> = Vec::new();
    for n in smf.tracks.iter().flat_map(|t| t.sequence.notes.iter()).filter(|n| n.channel != 9 && !n.muted && n.start >= 0) {
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
    if events.is_empty() {
        return Err("no notes to play".into());
    }
    events.sort_by_key(|e| (e.0, e.1));
    let len = events.iter().map(|e| e.0).max().unwrap_or(0).saturating_add(rate as usize).min(max + rate as usize);
    let mut synth = soundcraft_dsp::plugins::synth::SubtractiveSynth::new();
    const BLOCK: usize = 256;
    synth.prepare(rate as f32, BLOCK, 2);
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
                synth.note_on(t - at, pitch, vel);
            } else {
                synth.note_off(t - at, pitch);
            }
            next += 1;
        }
        io.iter_mut().for_each(|c| c.iter_mut().for_each(|x| *x = 0.0));
        synth.process(&mut io, n);
        for (o, c) in out.iter_mut().zip(&io) {
            o.extend_from_slice(c.get(..n).unwrap_or(&[]));
        }
        at += n;
    }
    Ok(out)
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
            let t = SmfTrack { name: "T".into(), sequence: Sequence { notes, ctrls: Vec::new() }, ..Default::default() };
            write_smf(&Smf { format: 1, tracks: vec![t], ..Default::default() }).unwrap()
        };
        let bytes = file(vec![n(60, 0, 0), n(64, 0, 1920)]);
        // Three beats at 120 bpm, then a second of release.
        let at120 = super::render_midi(&bytes, 120.0, 48_000).unwrap();
        assert_eq!(at120[0].len(), 72_000 + 48_000);
        let at60 = super::render_midi(&bytes, 60.0, 48_000).unwrap();
        assert_eq!(at60[0].len(), 144_000 + 48_000);
        let energy = |c: &[f32]| c.iter().map(|x| x * x).sum::<f32>();
        assert!(energy(&at120[0][..24_000]) > 0.01, "the first note sounds");
        assert!(energy(&at120[0][60_000..72_000]) > 0.01, "the second note, two beats later");
        assert!(super::render_midi(&file(vec![n(36, 9, 0)]), 120.0, 48_000).is_err(), "drums only: nothing to play");
        assert!(super::render_midi(b"not midi", 120.0, 48_000).is_err());
    }

    #[test]
    fn previews_are_cut_and_set_to_the_preview_level() {
        let out = super::prepared(vec![vec![1.0; 300]], 1, -6.0);
        assert_eq!(out[0].len(), 120);
        assert!((out[0][0] - 0.501).abs() < 0.01);
    }
}
