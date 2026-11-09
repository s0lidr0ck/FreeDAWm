//! Turning drum recordings into a Juke pad's samples: each hit found in the takes (a file can
//! hold one hit or a run of them), trimmed to its attack and its ring, then sorted by how hard it
//! was played into velocity layers (1 the softest) with round-robins inside each layer, and the
//! pad's trim that levels it with the other pads (the same targets the recorded kits use).

/// Hits a pad keeps: four velocity layers of four round-robins.
pub const LAYERS: usize = 4;
pub const ROBINS: usize = 4;

/// Loudness targets for a pad's loudest layer (its loudest 50 ms, dB of mean energy): drums at
/// the kick's level, hats and cymbals 4 dB under it.
pub const DRUM_TARGET: f32 = -10.0;
pub const CYMBAL_TARGET: f32 = -14.0;
/// A trim never lifts a pad's peak above this (dBFS; Juke's kit level is -6 dB, so the output
/// keeps its headroom), and stays within ±`TRIM_LIMIT` dB.
pub const PEAK_CEILING: f32 = 6.0;
pub const TRIM_LIMIT: f32 = 12.0;

/// Takes with layers closer together than this (dB) were played at one strength: one layer of
/// round-robins, not layers that would only differ by chance.
const ONE_STRENGTH_DB: f32 = 4.0;

/// A hit cut from a take: its audio (planar) and how loud it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub channels: Vec<Vec<f32>>,
    /// Its loudest 50 ms: mean energy over the channels, in dB.
    pub punch_db: f32,
    /// Absolute peak.
    pub peak: f32,
}

/// The hits in a take (planar, at `sample_rate`): each from just before its attack to where it
/// has rung out (or the next hit starts), with a short fade at the end. Noise and bleed much
/// quieter than the take's loudest hit are left out.
pub fn split(channels: &[Vec<f32>], sample_rate: f32) -> Vec<Hit> {
    let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    if frames == 0 {
        return Vec::new();
    }
    let n = channels.len() as f32;
    let mono: Vec<f32> =
        (0..frames).map(|i| channels.iter().map(|c| c.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0)).sum::<f32>() / n).collect();
    let loudest = mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if loudest <= 1e-6 {
        return Vec::new();
    }
    let mut starts = onsets(&mono, sr, loudest);
    starts.push(frames);
    let pre = (sr * 0.001) as usize;
    let fade_out = (sr * 0.01) as usize;
    let min_len = (sr * 0.01) as usize;
    let mut out = Vec::new();
    for w in starts.windows(2) {
        let (a, b) = (w[0], w[1].min(frames));
        let Some(seg) = mono.get(a..b) else { continue };
        let peak = seg.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        // Bleed, a breath, the room: 40 dB under the take's loudest hit.
        if peak < loudest * 0.01 {
            continue;
        }
        let Some(attack) = seg.iter().position(|v| v.abs() >= peak * 0.1) else { continue };
        let start = (a + attack).saturating_sub(pre).max(a);
        let tail = seg.iter().rposition(|v| v.abs() >= peak * 0.001).map_or(b, |i| a + i + 1);
        let end = tail.max(start).min(b);
        if end - start < min_len {
            continue;
        }
        let mut hit: Vec<Vec<f32>> = channels.iter().map(|c| c.get(start..end).map(<[f32]>::to_vec).unwrap_or_default()).collect();
        let len = end - start;
        let fin = (start..a + attack).len().clamp(1, len);
        let fout = fade_out.min(len / 4).max(1);
        for c in &mut hit {
            for (i, v) in c.iter_mut().enumerate() {
                if !v.is_finite() {
                    *v = 0.0;
                }
                if i < fin {
                    *v *= i as f32 / fin as f32;
                }
                if i + fout > len {
                    *v *= (len - i) as f32 / fout as f32;
                }
            }
        }
        out.push(Hit { punch_db: punch_db(&hit, sr), peak: hit.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs())), channels: hit });
    }
    out
}

/// Where hits start in `mono` (the first always 0): where the signal jumps well above the ring
/// of what came before. The ring is a peak follower that holds through a bass note's cycles
/// (a sub kick's 40 Hz is one swing every 25 ms), so a long 808 tail stays one hit; a new hit
/// must rise 9 dB over the ring as it was 10 ms earlier (an attack takes a few ms to rise), be no
/// quieter than 40 dB under `loudest`, and come 50 ms after the last.
fn onsets(mono: &[f32], sr: f32, loudest: f32) -> Vec<usize> {
    let release = (-1.0 / (sr * 0.08)).exp();
    let gap = (sr * 0.05) as usize;
    let delay = ((sr * 0.01) as usize).max(1);
    let floor = loudest * 0.01;
    let mut out = vec![0usize];
    let mut ring = 0.0f32;
    // The ring over the last `delay` frames, oldest at `i % delay`.
    let mut past = vec![0.0f32; delay];
    let mut last: Option<usize> = None;
    for (i, v) in mono.iter().enumerate() {
        let a = v.abs();
        let slot = i % delay;
        let before = past.get(slot).copied().unwrap_or(0.0);
        if a >= floor && a > before * 2.8 && last.is_none_or(|l| i - l >= gap) {
            if i > 0 {
                out.push(i);
            }
            last = Some(i);
        }
        ring = (ring * release).max(a);
        if let Some(p) = past.get_mut(slot) {
            *p = ring;
        }
    }
    out
}

/// The loudest 50 ms of `channels`: mean energy over the channels, in dB.
pub fn punch_db(channels: &[Vec<f32>], sample_rate: f32) -> f32 {
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    if frames == 0 || channels.is_empty() {
        return -120.0;
    }
    let n = channels.len() as f32;
    let e: Vec<f32> = (0..frames).map(|i| channels.iter().map(|c| c.get(i).map_or(0.0, |v| v * v)).sum::<f32>() / n).collect();
    let w = ((sample_rate * 0.05) as usize).clamp(1, frames);
    let mut acc: f32 = e.iter().take(w).sum();
    let mut best = acc;
    for k in w..frames {
        acc += e.get(k).copied().unwrap_or(0.0) - e.get(k - w).copied().unwrap_or(0.0);
        best = best.max(acc);
    }
    10.0 * (best.max(0.0) / w as f32).max(1e-12).log10()
}

/// The note a hit (mono) rings at, fractional MIDI: the median of 100 ms windows from 60 ms in
/// (past the click and an 808's pitch drop) to 1 s, down to 30 Hz (B0, an 808's lowest), when
/// most agree. `None` for
/// noise (a snare, a hat).
pub fn hit_pitch(mono: &[f32], sample_rate: f32) -> Option<f32> {
    let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { return None };
    let win = (sr * 0.1) as usize;
    let hop = (sr * 0.05) as usize;
    let peak = mono.iter().filter(|x| x.is_finite()).fold(0.0f32, |m, x| m.max(x.abs()));
    if peak < 1e-4 || win < 64 || hop == 0 {
        return None;
    }
    let mut found = Vec::new();
    let mut pos = (sr * 0.06) as usize;
    let end = mono.len().min(sr as usize);
    while pos + win <= end {
        if let Some(frame) = mono.get(pos..pos + win) {
            let rms = (frame.iter().map(|x| if x.is_finite() { x * x } else { 0.0 }).sum::<f32>() / win as f32).sqrt();
            if rms > peak * 0.05
                && let Some(f) = crate::pitch_detect::yin(frame, sr, 30.0, 1000.0, 0.15)
            {
                let m = 69.0 + 12.0 * (f / 440.0).log2();
                if m.is_finite() {
                    found.push(m);
                }
            }
        }
        pos += hop;
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by(f32::total_cmp);
    let mid = *found.get(found.len() / 2)?;
    let near = found.iter().filter(|m| (*m - mid).abs() < 0.5).count();
    (near * 2 >= found.len()).then_some(mid)
}

/// The note (fractional MIDI, the middle of its semitone's hits) most hits sound at, when the
/// drum is pitched: at least 40% of the hits have a pitch, and that semitone holds at least a
/// quarter of them (and two). `None` for unpitched drums (snares, hats) and scattered pitches.
pub fn common_note(pitches: &[Option<f32>]) -> Option<f32> {
    let found: Vec<f32> = pitches.iter().flatten().copied().filter(|m| m.is_finite()).collect();
    if found.is_empty() || (found.len() as f32) < pitches.len() as f32 * 0.4 {
        return None;
    }
    let mut best: Option<(usize, i32)> = None;
    for m in &found {
        let n = m.round() as i32;
        let count = found.iter().filter(|x| x.round() as i32 == n).count();
        if best.is_none_or(|(c, _)| count > c) {
            best = Some((count, n));
        }
    }
    let (count, n) = best?;
    if count < 2 || (count as f32) < found.len() as f32 * 0.25 {
        return None;
    }
    let same: Vec<f32> = found.iter().copied().filter(|x| x.round() as i32 == n).collect();
    Some(same.iter().sum::<f32>() / same.len() as f32)
}

/// Where the hits go: `slots[i]` is the (layer, robin) of hit `hits[i]` of [`Plan::hits`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    /// (hit index, layer, robin), each from 0.
    pub slots: Vec<(usize, usize, usize)>,
    /// Layers used.
    pub layers: usize,
    /// Hits left out (more than a layer holds).
    pub dropped: usize,
}

/// Sorts hits of loudness `punch` (dB) into velocity layers, softest first: as many layers as
/// the playing spans (about one per 6 dB, up to four; one if every hit was played alike), the
/// hits split evenly among them by loudness, up to four round-robins a layer (spread across the
/// layer's range when it has more).
pub fn plan(punch: &[f32]) -> Plan {
    let n = punch.len();
    if n == 0 {
        return Plan::default();
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| punch.get(*a).copied().unwrap_or(0.0).total_cmp(&punch.get(*b).copied().unwrap_or(0.0)));
    let lo = order.first().and_then(|i| punch.get(*i)).copied().unwrap_or(0.0);
    let hi = order.last().and_then(|i| punch.get(*i)).copied().unwrap_or(0.0);
    let spread = hi - lo;
    let layers = if n == 1 || spread < ONE_STRENGTH_DB { 1 } else { (((spread / 6.0).round() as usize) + 1).clamp(2, LAYERS).min(n) };
    let mut slots = Vec::new();
    for layer in 0..layers {
        let group = order.get(layer * n / layers..(layer + 1) * n / layers).unwrap_or(&[]);
        let take = group.len().min(ROBINS);
        for robin in 0..take {
            // Evenly through the group, from its middle out when it has more than four.
            let at = if group.len() <= ROBINS { robin } else { (robin * group.len() + group.len() / 2) / ROBINS };
            if let Some(h) = group.get(at.min(group.len() - 1)) {
                slots.push((*h, layer, robin));
            }
        }
    }
    Plan { dropped: n - slots.len(), slots, layers }
}

/// The pad's trim (dB) that brings its loudest layer (mean `punch` dB of that layer's hits) to
/// its family's target, without lifting `peak` past the ceiling.
pub fn trim(cymbal: bool, loudest_layer_punch: f32, peak: f32) -> f32 {
    let target = if cymbal { CYMBAL_TARGET } else { DRUM_TARGET };
    let headroom = PEAK_CEILING - 20.0 * peak.max(1e-6).log10();
    let t = (target - loudest_layer_punch).min(headroom).clamp(-TRIM_LIMIT, TRIM_LIMIT);
    if t.is_finite() { (t * 10.0).round() / 10.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decaying 120 Hz hit of amplitude `amp`, `len` seconds.
    fn hit(amp: f32, sr: f32, len: f32) -> Vec<f32> {
        (0..(sr * len) as usize).map(|i| amp * (-(i as f32) / (sr * 0.05)).exp() * (i as f32 * 120.0 * std::f32::consts::TAU / sr).sin()).collect()
    }

    #[test]
    fn a_run_of_hits_splits_into_trimmed_hits() {
        let sr = 48_000.0;
        let mut take = vec![0.0; 9_600]; // 200 ms of silence first
        for amp in [0.2, 0.5, 0.9] {
            take.extend(hit(amp, sr, 0.5));
            take.extend(vec![0.0; 4_800]);
        }
        let hits = split(&[take.clone(), take], sr);
        assert_eq!(hits.len(), 3, "{:?}", hits.iter().map(|h| h.peak).collect::<Vec<_>>());
        for h in &hits {
            // Starts within a couple of ms of its attack: no silence before it.
            let first = h.channels[0].iter().position(|v| v.abs() > h.peak * 0.1).unwrap();
            assert!(first < 200, "attack at {first}");
            assert!(h.channels[0].len() < 30_000, "rings out, no trailing silence: {}", h.channels[0].len());
            assert_eq!(h.channels.len(), 2);
        }
        assert!(hits[0].punch_db < hits[1].punch_db && hits[1].punch_db < hits[2].punch_db);
    }

    #[test]
    fn a_long_sub_kick_stays_one_hit() {
        // 808-style: 45 Hz falling to 40 Hz, ringing 2 s, three of them 3 s apart.
        let sr = 48_000.0;
        let one: Vec<f32> = (0..(sr * 2.0) as usize)
            .map(|i| {
                let t = i as f32 / sr;
                let f = 40.0 + 5.0 * (-t / 0.1).exp();
                0.8 * (-t / 0.7).exp() * (t * f * std::f32::consts::TAU).sin()
            })
            .collect();
        let mut take = vec![0.0; 4_800];
        for _ in 0..3 {
            take.extend(&one);
            take.extend(vec![0.0; 48_000]);
        }
        let hits = split(&[take], sr);
        assert_eq!(hits.len(), 3, "{:?}", hits.iter().map(|h| h.channels[0].len()).collect::<Vec<_>>());
        assert!(hits.iter().all(|h| h.channels[0].len() as f32 > sr * 1.5), "each keeps its ring");
    }

    #[test]
    fn bleed_far_under_the_hits_is_left_out() {
        let sr = 48_000.0;
        let mut take = hit(0.9, sr, 0.4);
        take.extend(vec![0.0; 4_800]);
        take.extend(hit(0.002, sr, 0.3));
        assert_eq!(split(&[take], sr).len(), 1);
        assert!(split(&[vec![0.0; 1000]], sr).is_empty());
    }

    #[test]
    fn hits_played_alike_are_round_robins_of_one_layer() {
        let p = plan(&[-12.0, -11.0, -12.5, -11.5, -12.2]);
        assert_eq!(p.layers, 1);
        assert_eq!(p.slots.len(), 4);
        assert_eq!(p.dropped, 1);
        assert!(p.slots.iter().all(|s| s.1 == 0));
    }

    #[test]
    fn hits_played_soft_to_hard_fill_layers_softest_first() {
        // Sixteen hits from -40 to -10 dB, recorded in any order.
        let punch: Vec<f32> = (0..16).map(|i| -40.0 + ((i * 7) % 16) as f32 * 2.0).collect();
        let p = plan(&punch);
        assert_eq!(p.layers, 4);
        assert_eq!(p.slots.len(), 16);
        for w in 0..3 {
            let max_lo = p.slots.iter().filter(|s| s.1 == w).map(|s| punch[s.0]).fold(f32::MIN, f32::max);
            let min_hi = p.slots.iter().filter(|s| s.1 == w + 1).map(|s| punch[s.0]).fold(f32::MAX, f32::min);
            assert!(max_lo < min_hi, "layer {w} is softer than layer {}", w + 1);
        }
        // Two hits a world apart: two layers.
        assert_eq!(plan(&[-30.0, -10.0]).layers, 2);
        assert_eq!(plan(&[-10.0]).layers, 1);
        assert_eq!(plan(&[]), Plan::default());
    }

    #[test]
    fn a_sub_kicks_pitch_is_found() {
        let sr = 48_000.0;
        let kick: Vec<f32> = (0..(sr * 0.8) as usize)
            .map(|i| {
                let t = i as f32 / sr;
                0.8 * (-t / 0.5).exp() * (t * 49.0 * std::f32::consts::TAU).sin()
            })
            .collect();
        let m = hit_pitch(&kick, sr).unwrap();
        assert!((m - 31.0).abs() < 0.3, "49 Hz is G1 (31): {m}");
        let tom: Vec<f32> = (0..(sr * 0.6) as usize)
            .map(|i| {
                let t = i as f32 / sr;
                0.7 * (-t / 0.25).exp() * (t * 98.0 * std::f32::consts::TAU).sin()
            })
            .collect();
        let m = hit_pitch(&tom, sr).unwrap();
        assert!((m - 43.0).abs() < 0.3, "98 Hz is G2 (43): {m}");
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..24_000)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / 16_777_216.0 * (-(i as f32) / 4_000.0).exp() - 0.5 * (-(i as f32) / 4_000.0).exp()
            })
            .collect();
        assert_eq!(hit_pitch(&noise, sr), None);
    }

    #[test]
    fn a_pitched_drum_keeps_its_most_common_note() {
        // An 808 line: mostly G1 (31), some F1 and A1.
        let line = [Some(31.1), Some(30.9), Some(29.0), Some(31.0), Some(33.2), Some(31.2), None, Some(29.1)];
        let note = common_note(&line).unwrap();
        assert!((note - 31.05).abs() < 0.1, "{note}");
        // Unpitched (a snare's noise): no note, nothing left out.
        assert_eq!(common_note(&[None, None, Some(60.0), None]), None);
        // Every hit a different note: no common one.
        assert_eq!(common_note(&[Some(40.0), Some(42.0), Some(44.0), Some(46.0), Some(48.0)]), None);
    }

    #[test]
    fn trim_levels_to_the_target_within_the_ceiling_and_limit() {
        assert_eq!(trim(false, -16.0, 0.3), 6.0);
        assert_eq!(trim(true, -16.0, 0.3), 2.0);
        // A hot peak caps the lift: peak 1.0 leaves 6 dB.
        assert_eq!(trim(false, -30.0, 1.0), 6.0);
        assert_eq!(trim(false, 10.0, 0.5), -12.0);
    }
}
