//! Tap Tempo (the Tempo Operations window's Tap button): the tempo from the times of taps.

use super::*;
use crate::cmd;
use serde_json::json;

/// A pause longer than this between taps starts a new count.
pub const TAP_RESET_SECONDS: f64 = 2.0;
/// How many of the latest taps are averaged.
pub const TAP_KEEP: usize = 8;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "event.tap_tempo",
        "Tap Tempo",
        [],
        None,
        "{taps: [seconds, ascending], at?} — the tempo the taps beat (the last 8, after the last pause of 2 s or more), set on the tempo in effect at `at` (default the insertion point) -> {bpm, tick}",
        always,
        tap
    )]
}

/// The tempo `taps` (times in seconds) beat: the mean interval of the latest run of taps, or
/// `None` with fewer than two.
pub fn tapped_bpm(taps: &[f64]) -> Option<f64> {
    let taps: Vec<f64> = taps.iter().copied().filter(|t| t.is_finite()).collect();
    let run_start = taps.windows(2).rposition(|w| w[1] - w[0] >= TAP_RESET_SECONDS || w[1] <= w[0]).map_or(0, |i| i + 1);
    let run = taps.get(run_start..)?;
    let run = run.get(run.len().saturating_sub(TAP_KEEP)..)?;
    let (first, last) = (run.first()?, run.last()?);
    let beats = run.len().checked_sub(1).filter(|n| *n > 0)?;
    let bpm = 60.0 * beats as f64 / (last - first);
    bpm.is_finite().then_some(bpm.clamp(5.0, 999.0))
}

fn tap(e: &mut Engine, p: &Value) -> Result<Value> {
    let taps: Vec<f64> = p
        .get("taps")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("event.tap_tempo", "`taps` is a list of times in seconds"))?
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    let bpm = tapped_bpm(&taps).ok_or_else(|| bad("event.tap_tempo", "tap at least twice"))?;
    let at = position_param(e, "event.tap_tempo", p, "at")?.unwrap_or(e.session().edit.selection.start);
    let s = e.session_mut();
    let here = s.tempo.samples_to_ticks(at.max(0), s.sample_rate);
    // The tempo change in effect there takes the new tempo; the map's first one when none is earlier.
    let tick = s.tempo.tempos().iter().map(|t| t.tick).filter(|t| *t <= here).max().unwrap_or(0);
    let bpm = (bpm * 100.0).round() / 100.0;
    s.tempo.set_tempo(tick, bpm).map_err(|err| bad("event.tap_tempo", err.to_string()))?;
    Ok(json!({"bpm": bpm, "tick": tick}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_taps_since_the_last_pause_set_the_tempo() {
        let tapped_bpm = |t: &[f64]| tapped_bpm(t).map(|b| (b * 1000.0).round() / 1000.0);
        assert_eq!(tapped_bpm(&[1.0]), None);
        assert_eq!(tapped_bpm(&[0.0, 0.5, 1.0, 1.5]), Some(120.0));
        // A pause starts over: only the last run counts.
        assert_eq!(tapped_bpm(&[0.0, 0.3, 5.0, 6.0, 7.0]), Some(60.0));
        assert_eq!(tapped_bpm(&[0.0, 0.3, 5.0]), None);
        // Only the last eight are averaged.
        let taps: Vec<f64> = (0..20).map(|i| if i < 12 { f64::from(i) * 0.6 } else { 6.6 + f64::from(i - 11) * 0.5 }).collect();
        assert_eq!(tapped_bpm(&taps), Some(120.0));
        assert_eq!(tapped_bpm(&[0.0, f64::NAN, 0.5]), Some(120.0));
    }

    #[test]
    fn tapping_changes_the_tempo_in_effect_at_the_insertion_point() {
        let mut e = Engine::default();
        let sr = e.session().sample_rate;
        let tick_bar_3 = e.session().tempo.bar_start_tick(3);
        e.execute("event.tempo", &json!({"bpm": 100.0, "at": e.session().tempo.tick_to_samples(tick_bar_3, sr)})).unwrap();
        let later = e.session().tempo.tick_to_samples(tick_bar_3 + 960, sr);
        e.execute("edit.select", &json!({"start": later})).unwrap();
        let v = e.execute("event.tap_tempo", &json!({"taps": [10.0, 10.4, 10.8]})).unwrap();
        assert_eq!(v["bpm"], 150.0);
        let tempos: Vec<(i64, f64)> = e.session().tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect();
        assert_eq!(tempos.len(), 2);
        assert_eq!(tempos[1], (tick_bar_3, 150.0));
        assert!(e.execute("event.tap_tempo", &json!({"taps": [1.0]})).is_err());
    }
}
