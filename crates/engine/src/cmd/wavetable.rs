//! The Wavetable Synth's user tables: a table made from the user's own audio, stored in the
//! synth's insert (`Insert::state`: the waves themselves, see
//! [`soundcraft_dsp::plugins::wavetable::user`]), so a session never depends on the file it came
//! from. Whenever the state changes, and when a session opens, the table is band-limited here,
//! off the audio thread, into `Insert::prepared` ([`prepared`]); the mix engine hands it to the
//! running synth by reference between blocks.

use soundcraft_dsp::plugins::wavetable::user;
use soundcraft_model::{Insert, PreparedState};
use std::sync::Arc;

/// The Wavetable Synth's plugin id.
pub const WAVETABLE: &str = "wavetable_synth";

/// What a Wavetable Synth insert's stored state makes ready for it: its user table band-limited
/// (`None` when it has none; an error naming what is wrong with a damaged one). Allocates and
/// takes a few milliseconds: never on the audio thread.
pub(crate) fn prepared(ins: &Insert) -> Result<Option<PreparedState>, String> {
    if ins.plugin != WAVETABLE {
        return Ok(None);
    }
    let Some(bytes) = ins.state_bytes() else { return Ok(None) };
    let built = user::prepare(&bytes)?;
    Ok(Some(PreparedState::new_shared(Arc::new(built))))
}

/// [`prepared`] for a module or instrument whose state changed or whose session opened: a
/// damaged table is left out (the synth plays its built-in bank) and logged.
pub(crate) fn prepare_insert(ins: &mut Insert) {
    ins.prepared = prepared(ins).unwrap_or_else(|err| {
        log::warn!("{WAVETABLE}: its user wavetable is damaged and was left out: {err}");
        None
    });
}

/// The user table a Wavetable Synth insert holds, decoded (`None`: none, or another plugin).
pub fn user_table(ins: &Insert) -> Option<Result<user::UserTable, String>> {
    if ins.plugin != WAVETABLE {
        return None;
    }
    ins.state_bytes().map(|b| user::UserTable::decode(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use serde_json::json;
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{Clip, Session, TrackId};
    use soundcraft_time::{Range, TICKS_PER_QUARTER};

    pub(super) fn synth_track(e: &mut Engine) -> TrackId {
        e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": WAVETABLE, "params": {"cutoff": 20000.0}})).unwrap();
        let q = TICKS_PER_QUARTER;
        let mut seq = Sequence::default();
        for (pitch, start) in [(57u8, 0i64), (69, 2)] {
            seq.notes.push(Note {
                probability: 100,
                pitch,
                velocity: 120,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start: start * q,
                length: q,
            });
        }
        let s = e.session_mut();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::midi(cid, "notes", 0, 48_000 * 4, seq));
        t
    }

    pub(super) fn render(s: &Session) -> Vec<f32> {
        soundcraft_mix::render_range(s, Range::new(0, 48_000), 512).swap_remove(0)
    }

    pub(super) fn odd_table() -> user::UserTable {
        let w: Vec<f32> = (0..user::WAVE_LEN).map(|i| (std::f32::consts::TAU * 3.0 * i as f32 / user::WAVE_LEN as f32).sin()).collect();
        user::UserTable::new("third", &[w]).unwrap()
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn a_user_table_in_the_session_plays_saves_and_reopens() {
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let plain = render(e.session());
        // A table whose only wave is the third harmonic: the note sounds an octave and a fifth up.
        let mut s = e.session().clone();
        let ins = s.tracks.iter_mut().find(|x| x.id == t).unwrap().instrument.as_mut().unwrap();
        ins.set_state_bytes(&odd_table().encode());
        let e2 = Engine::new(s);
        assert!(e2.session().track(t).unwrap().instrument.as_ref().unwrap().prepared.is_some());
        let with = render(e2.session());
        let (a, b) = (crossings(&plain[2_400..24_000]), crossings(&with[2_400..24_000]));
        eprintln!("crossings: built-in {a}, user table {b}");
        assert!((b as f32 / a as f32 - 3.0).abs() < 0.1, "{a} {b}");
        // Saved and opened again (JSON): the table travels in the session and sounds the same.
        let text = serde_json::to_string(e2.session()).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        let e3 = Engine::new(back);
        assert_eq!(render(e3.session()), with);
    }

    #[test]
    fn a_damaged_table_is_left_out_and_never_panics() {
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let plain = render(e.session());
        let mut good = odd_table().encode();
        for damage in [vec![0xff, 0x00], good.split_off(100), b"FDWT\x01\x20\x00\x08\x00".to_vec()] {
            let mut s = e.session().clone();
            let ins = s.tracks.iter_mut().find(|x| x.id == t).unwrap().instrument.as_mut().unwrap();
            ins.set_state_bytes(&damage);
            assert!(prepared(ins).is_err());
            assert!(user_table(ins).is_some_and(|r| r.is_err()));
            let e2 = Engine::new(s);
            assert!(e2.session().track(t).unwrap().instrument.as_ref().unwrap().prepared.is_none());
            assert_eq!(render(e2.session()), plain, "the built-in bank plays");
        }
    }
}
