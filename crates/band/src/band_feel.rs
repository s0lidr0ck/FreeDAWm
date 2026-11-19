//! The band-feel layer: how six musicians who each play their part become one band (not a port of MIDI Studio).
//!
//! Natural gives every player a groove of his own; this layer, on top of Natural, makes them play together the way a
//! band does. Everything here is a pure function of the song position, the musician and the Take, so the same song,
//! settings and Take always give the same performance (print, undo and the tests stay stable), and nothing allocates
//! (it runs on the audio thread).
//!
//! - Time: the band breathes together (one slow drift everyone shares, so they stay locked while the time moves a
//!   little), each player drifts a little on his own around it (correlated, never white noise), the bass rides the
//!   drummer's drift, and each hit scatters by a few ms. Lengths of drift are musical (a beat to a few bars).
//! - Dynamics: four-bar phrases have a shape (they open strong, relax, lean into their end), the band swells into a
//!   section with more Energy and eases into one with less, and lands the downbeat it was heading for.
//! - Chords: a pianist's hands strike nearly together with the top note leading; a section (strings, brass, organ)
//!   never rolls a chord; a deliberate roll is kept for a phrase's first chord.
//! - The ending: the last bar before the final chord lays back, as a band does without a written ritardando.

/// Role codes as the Library has them (`ROLE_NAMES`): keys, bass, drums, guitar, ensemble, melodic.
pub const ROLE_KEYS: i32 = 0;
pub const ROLE_BASS: i32 = 1;
pub const ROLE_DRUMS: i32 = 2;
pub const ROLE_GUITAR: i32 = 3;
pub const ROLE_ENSEMBLE: i32 = 4;

/// Salts that keep the drifts apart.
const SALT_BAND_TIME: u32 = 0x9E37_79B9;
const SALT_DRUM_TIME: u32 = 0x85EB_CA6B;
const SALT_PLAYER_TIME: u32 = 0xC2B2_AE35;
const SALT_PLAYER_VEL: u32 = 0x27D4_EB2F;

fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x
}

/// A lattice value in [-1, 1).
fn lattice(seed: u32, i: i64) -> f32 {
    let h = mix(seed ^ mix((i as u32) ^ mix((i >> 32) as u32 ^ 0x5bd1_e995)));
    (h >> 8) as f32 / 8_388_608.0 - 1.0
}

/// Value noise with a lattice point every `period` quarters, cosine-blended between them: smooth, in [-1, 1].
pub fn smooth(seed: u32, x: f64, period: f64) -> f32 {
    let p = if period > 1.0e-6 { period } else { 1.0 };
    let u = x / p;
    let i = u.floor();
    let t = (u - i) as f32;
    let s = (1.0 - (t * std::f32::consts::PI).cos()) * 0.5;
    let a = lattice(seed, i as i64);
    let b = lattice(seed, i as i64 + 1);
    a + (b - a) * s
}

/// About 1 / sd of [`smooth`] (uniform lattice values, cosine-blended).
const SMOOTH_NORM: f32 = 2.19;

/// Drift with roughly unit spread, built from octaves of [`smooth`] from `shortest` quarters up to 8x that, the
/// longer ones weighing more (a pink-ish spectrum: what human timing and dynamics measure as, unlike white noise).
pub fn drift(seed: u32, x: f64, shortest: f64) -> f32 {
    const W: [f32; 4] = [0.35, 0.5, 0.6, 0.52]; // sum of squares = 1
    let mut v = 0.0f32;
    let mut period = shortest;
    for (k, w) in W.iter().enumerate() {
        v += w * smooth(seed.wrapping_add((k as u32).wrapping_mul(0x632B_E5AB)), x, period);
        period *= 2.0;
    }
    v * SMOOTH_NORM
}

fn take_seed(take: i32, salt: u32) -> u32 {
    mix((take as u32).wrapping_mul(0x9E37_79B1) ^ salt)
}

/// The time every musician of the band shares at song position `x` (quarters), in ms: the band breathing.
pub fn band_time_ms(take: i32, x: f64) -> f32 {
    2.4 * drift(take_seed(take, SALT_BAND_TIME), x, 4.0)
}

/// The drummer's own drift (ms), which the bass follows.
pub fn drummer_time_ms(take: i32, x: f64) -> f32 {
    1.6 * drift(take_seed(take, SALT_DRUM_TIME), x, 1.0)
}

/// A player's own drift around the band (ms), by role. The bass rides the drummer's drift and adds a little of his
/// own; the drummer is the drummer's drift.
pub fn player_time_ms(take: i32, musician: i32, role: i32, x: f64) -> f32 {
    let own = |amp: f32, shortest: f64| amp * drift(take_seed(take, SALT_PLAYER_TIME ^ (musician as u32).wrapping_mul(0x01000193)), x, shortest);
    match role {
        ROLE_DRUMS => drummer_time_ms(take, x),
        ROLE_BASS => drummer_time_ms(take, x) + own(1.3, 1.0),
        ROLE_KEYS | ROLE_GUITAR => 0.6 * drummer_time_ms(take, x) + own(2.6, 1.0),
        _ => 0.4 * drummer_time_ms(take, x) + own(2.8, 2.0),
    }
}

/// How far one hit scatters (ms, the spread of a bell of width 1) by role and, for drums, by kit piece: the kick and
/// the bass are the steadiest, hats and hands a little looser, a section looser again.
pub fn hit_scatter_ms(role: i32, drum_note: i32) -> f32 {
    match role {
        ROLE_DRUMS => match drum_note {
            35 | 36 => 2.0,
            37..=40 => 2.6,
            _ => 3.4,
        },
        ROLE_BASS => 2.6,
        ROLE_KEYS | ROLE_GUITAR => 3.4,
        _ => 3.6,
    }
}

/// The notes an instrument (the Library's instrument index: Piano, Synth, Bass, Electric Keys, Harp & Mallets, Drums,
/// Guitar, Strings, Winds, Brass, Ethnic, Beat, Folk) can really play, as MIDI notes.
pub fn playable_range(instrument: i32) -> (i32, i32) {
    match instrument {
        0 => (21, 108),
        1 => (24, 108),
        2 => (23, 67),
        3 => (28, 100),
        4 => (36, 103),
        6 => (40, 88),
        7 => (28, 100),
        8 => (34, 98),
        9 => (34, 84),
        10 => (36, 96),
        12 => (40, 93),
        _ => (0, 127),
    }
}

/// `note` moved by octaves into `lo..=hi` (unchanged when it is there already or the range is narrower than an
/// octave).
pub fn fold_into(note: i32, lo: i32, hi: i32) -> i32 {
    if hi - lo < 11 {
        return note;
    }
    let mut n = note;
    while n > hi {
        n -= 12;
    }
    while n < lo {
        n += 12;
    }
    n
}

/// A player's slow velocity drift (velocity units with spread `amount`): phrases breathe, not single notes.
pub fn player_vel_drift(take: i32, musician: i32, x: f64, amount: f32) -> f32 {
    amount * drift(take_seed(take, SALT_PLAYER_VEL ^ (musician as u32).wrapping_mul(0x0100_0193)), x, 1.0)
}

/// The shape of a four-bar phrase at `t` (0 = its first downbeat, 1 = the next phrase's): opens strong, relaxes in
/// the middle, leans into its end; a two-bar sub-phrase on top. A multiplier around 1 (`depth` 1 = about +-6 %).
pub fn phrase_shape(t: f32, depth: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let t = t.clamp(0.0, 1.0);
    1.0 + depth * (0.045 * (tau * t).cos() + 0.018 * (2.0 * tau * t).cos() + 0.02 * (t - 0.5))
}

/// What the band does at one step of the song (computed once a step, shared by everyone at that position).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandStep {
    pub on: bool,
    /// Song position of the step (quarters): what the drifts are keyed on.
    pub song_ppq: f64,
    /// ms everyone adds (the shared breath, the ending's lay-back).
    pub shared_ms: f32,
    /// Velocity multiplier (phrase shape, swell into / ease out of a section, the landing).
    pub vel_mul: f32,
    /// The first chord of a four-bar phrase (a deliberate roll may stay there).
    pub phrase_start: bool,
    /// This step is inside the last bar before a section with more Energy.
    pub building: bool,
}

impl Default for BandStep {
    fn default() -> Self {
        BandStep { on: false, song_ppq: 0.0, shared_ms: 0.0, vel_mul: 1.0, phrase_start: false, building: false }
    }
}

/// The section's arc at one step: where the step sits and what comes next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionPlace {
    /// Bar inside the section (0-based) and the section's length in bars.
    pub bar: i32,
    pub bars: i32,
    /// 0..1 inside the bar.
    pub in_bar: f32,
    /// Energy of the next section minus this one's (0 when the song loops into the same).
    pub energy_step: i32,
    /// The next section is a different one (or the same with another Energy).
    pub change_next: bool,
    /// This is the bar before the Ending's final chord.
    pub before_final: bool,
    /// This is the Ending's final chord.
    pub final_chord: bool,
    /// The section's first bar.
    pub first_bar: bool,
    /// The section before had less Energy (the band lands this one).
    pub landed_up: bool,
    /// Quarters per bar.
    pub bar_quarters: f32,
    /// Milliseconds per quarter.
    pub ms_per_quarter: f32,
}

/// The band's step from where it is in the section.
pub fn band_step(take: i32, song_ppq: f64, p: &SectionPlace, dynamics: f32) -> BandStep {
    let bars = p.bars.max(1);
    let phrase_len = if bars % 4 == 0 || bars > 4 { 4 } else { bars };
    let pb = p.bar.rem_euclid(phrase_len.max(1));
    let t = (pb as f32 + p.in_bar) / phrase_len.max(1) as f32;
    let depth = dynamics.clamp(0.0, 2.0);
    let mut vel = phrase_shape(t, depth);
    let last_bar = p.bar == bars - 1;
    let building = last_bar && p.energy_step > 0;
    if building {
        // Swell into the bigger section: up to +12 % over the bar, more for a bigger step.
        vel *= 1.0 + (0.08 + 0.02 * (p.energy_step.min(2) as f32)) * p.in_bar * depth.min(1.0);
    } else if last_bar && p.energy_step < 0 {
        vel *= 1.0 - 0.06 * p.in_bar * depth.min(1.0); // ease into the smaller one
    }
    if p.first_bar && p.landed_up && p.in_bar < 0.26 {
        vel *= 1.0 + 0.05 * depth.min(1.0); // land it
    }
    let mut ms = band_time_ms(take, song_ppq);
    if building {
        ms -= 2.0 * p.in_bar; // a band leans forward into a bigger chorus
    }
    if p.before_final {
        // The last bar before the final chord lays back, more and more (a ritardando nobody wrote).
        let q = p.ms_per_quarter.clamp(200.0, 1500.0);
        ms += p.in_bar * p.in_bar * 0.06 * q;
    }
    if p.final_chord {
        ms += 0.07 * p.ms_per_quarter.clamp(200.0, 1500.0);
    }
    BandStep { on: true, song_ppq, shared_ms: ms, vel_mul: vel, phrase_start: pb == 0 && p.in_bar < 0.01, building }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spread(f: impl Fn(f64) -> f32) -> (f32, f32) {
        let v: Vec<f32> = (0..4000).map(|i| f(f64::from(i) * 0.25)).collect();
        let m = v.iter().sum::<f32>() / v.len() as f32;
        let sd = (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt();
        (m, sd)
    }

    #[test]
    fn drift_has_unit_spread_and_is_smooth() {
        for seed in [1u32, 7, 99, 12345] {
            let (m, sd) = spread(|x| drift(seed, x, 1.0));
            assert!(m.abs() < 0.25, "mean {m}");
            assert!((0.7..1.3).contains(&sd), "sd {sd}");
        }
        // Smooth: a 16th apart, the drift moves far less than its spread (not white noise).
        let steps: Vec<f32> = (0..2000).map(|i| (drift(5, f64::from(i) * 0.25 + 0.25, 1.0) - drift(5, f64::from(i) * 0.25, 1.0)).abs()).collect();
        let mean_step = steps.iter().sum::<f32>() / steps.len() as f32;
        assert!(mean_step < 0.45, "mean 16th-to-16th change {mean_step}");
    }

    #[test]
    fn the_same_take_drifts_the_same_and_another_take_differently() {
        assert_eq!(band_time_ms(3, 17.25), band_time_ms(3, 17.25));
        let a: Vec<f32> = (0..64).map(|i| player_time_ms(1, 2, ROLE_KEYS, f64::from(i))).collect();
        let b: Vec<f32> = (0..64).map(|i| player_time_ms(2, 2, ROLE_KEYS, f64::from(i))).collect();
        assert_ne!(a, b);
    }

    #[test]
    fn a_phrase_opens_relaxes_and_leans_into_its_end() {
        let s = |t| phrase_shape(t, 1.0);
        assert!(s(0.0) > s(0.5) && s(0.95) > s(0.5));
        assert!((0.9..1.1).contains(&s(0.3)));
        assert_eq!(phrase_shape(0.4, 0.0), 1.0);
    }

    #[test]
    fn the_band_swells_into_a_bigger_section_and_lays_back_before_the_final_chord() {
        let place = |bar, in_bar, energy_step| SectionPlace {
            bar,
            bars: 4,
            in_bar,
            energy_step,
            change_next: true,
            before_final: false,
            final_chord: false,
            first_bar: bar == 0,
            landed_up: false,
            bar_quarters: 4.0,
            ms_per_quarter: 500.0,
        };
        let early = band_step(1, 12.0, &place(3, 0.0, 1), 1.0);
        let late = band_step(1, 15.75, &place(3, 0.95, 1), 1.0);
        let flat = band_step(1, 15.75, &place(3, 0.95, 0), 1.0);
        assert!(late.vel_mul > flat.vel_mul * 1.07, "{} vs {}", late.vel_mul, flat.vel_mul);
        assert!(early.building && late.building && !flat.building);
        let mut p = place(2, 0.9, 0);
        p.before_final = true;
        let rit = band_step(1, 11.6, &p, 1.0);
        p.before_final = false;
        let plain = band_step(1, 11.6, &p, 1.0);
        assert!(rit.shared_ms > plain.shared_ms + 20.0);
    }
}
