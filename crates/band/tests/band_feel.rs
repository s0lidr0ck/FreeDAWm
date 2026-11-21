//! The band-feel layer ("As a band", on top of Natural), measured on the lab's test songs: the pulse is shared and
//! breathes, phrases have a shape, the drummer marks section changes and the others answer, chords strike like hands,
//! notes stay playable, and a Take always plays the same. Classic and Natural are pinned elsewhere (goldens,
//! fidelity); here: the switch leaves them alone.

mod feel_support;

use std::collections::BTreeMap;

use feel_support::{
    Mode, Note, Take, bar_of, bar_quarters, bass_vs_kick, fill_bars, grid_offset, is_drums, mean, ms_per_quarter, notes, play, repetition, sd,
    section_starts, settings_for, spec_for, strikes, test_songs,
};
use soundcraft_band::band_feel::playable_range;
use soundcraft_band::library::load::embedded_library;
use soundcraft_band::offline::render;

fn song(name: &str) -> feel_support::TestSong {
    match test_songs().into_iter().find(|s| s.name == name) {
        Some(s) => s,
        None => panic!("no test song {name}"),
    }
}

#[test]
fn a_take_plays_the_same_every_time_and_another_take_differently() {
    let s = song("pop-verse-chorus-bridge");
    let a = play(&s, Mode::Band, 1);
    let b = play(&s, Mode::Band, 1);
    assert_eq!(a.events, b.events, "the same song, settings and Take must give the same performance");
    let c = play(&s, Mode::Band, 2);
    assert_ne!(a.events, c.events, "another Take plays differently");
}

#[test]
fn the_switch_leaves_classic_alone() {
    let s = song("pop-verse-chorus-bridge");
    let lib = embedded_library();
    let mut on = settings_for(&s, Mode::Classic, 1, &lib);
    on.feel.band = true;
    let off = settings_for(&s, Mode::Classic, 1, &lib);
    assert!(!off.feel.band);
    assert_eq!(render(on, &spec_for(&s)), render(off, &spec_for(&s)), "Classic plays exactly as before with the switch on");
}

/// Spread of a musician's onsets at each 16th position that has at least 8 of them (ms), averaged.
fn position_spread(t: &Take, k: usize) -> f64 {
    let all = notes(&t.events);
    let mut by: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for n in all.iter().filter(|n| n.musician == k) {
        let (o, p) = grid_offset(t, n);
        by.entry(p).or_default().push(o);
    }
    let v: Vec<f64> = by.values().filter(|x| x.len() >= 8).map(|x| sd(x)).collect();
    mean(&v)
}

#[test]
fn the_pulse_breathes_and_bass_sits_with_the_kick() {
    for name in ["pop-verse-chorus-bridge", "country-two-beat"] {
        let s = song(name);
        let nat = play(&s, Mode::Natural, 1);
        let band = play(&s, Mode::Band, 1);
        let d = (0..6).find(|&k| is_drums(&band, k) && band.enabled[k]).unwrap_or(0);
        let (sn, sb) = (position_spread(&nat, d), position_spread(&band, d));
        // Natural: a template (about 1 ms at a position). As a band: a few ms, a human drummer's spread.
        assert!(sn < 2.5, "{name}: Natural drummer spread {sn:.2} ms");
        assert!((2.5..=7.0).contains(&sb), "{name}: drummer spread as a band {sb:.2} ms");
        let bk = bass_vs_kick(&band);
        assert!(bk.len() >= 15, "{name}: shared bass / kick hits {}", bk.len());
        let within = bk.iter().filter(|x| x.abs() <= 8.0).count() as f64 / bk.len() as f64;
        assert!(mean(&bk).abs() <= 5.0, "{name}: bass {:+.1} ms from the kick on average", mean(&bk));
        assert!((1.5..=6.0).contains(&sd(&bk)), "{name}: bass against kick spread {:.2} ms", sd(&bk));
        assert!(within >= 0.85, "{name}: only {:.0} % of shared hits within 8 ms", within * 100.0);
    }
}

/// Matched notes (musician, pitch, 16th) of two takes: the band take's velocity over the natural one's, by bar.
fn velocity_ratio_by_bar(nat: &Take, band: &Take) -> Vec<f64> {
    let q = ms_per_quarter(nat) / 4.0;
    let key = |n: &Note| (n.musician, n.pitch, (n.ms / q).round() as i64);
    let mut nv: BTreeMap<(usize, u8, i64), u8> = BTreeMap::new();
    for n in notes(&nat.events).iter().filter(|n| !is_drums(nat, n.musician)) {
        nv.insert(key(n), n.vel);
    }
    let mut by_bar: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for n in notes(&band.events).iter().filter(|n| !is_drums(band, n.musician)) {
        if let Some(&v) = nv.get(&key(n)) {
            by_bar.entry(bar_of(band, n)).or_default().push(f64::from(n.vel) / f64::from(v.max(1)));
        }
    }
    (0..i64::from(band.song.bars)).map(|b| by_bar.get(&b).map_or(1.0, |v| mean(v))).collect()
}

#[test]
fn phrases_have_a_shape_and_the_band_swells_into_a_bigger_section() {
    let s = song("pop-verse-chorus-bridge");
    let nat = play(&s, Mode::Natural, 1);
    let band = play(&s, Mode::Band, 1);
    let r = velocity_ratio_by_bar(&nat, &band);
    // Verse (0) -> Chorus (+1) at bars 4 and 12, Bridge (-1) -> Chorus at 20: the bar before swells.
    for b in [3usize, 11, 19] {
        let before = mean(&r[b - 3..b]);
        assert!(r[b] > before * 1.03, "bar {b}: {:.3} against {:.3} before it", r[b], before);
    }
    // Chorus -> Bridge (bar 16): it eases instead.
    assert!(r[15] < r[11], "into the smaller bridge {:.3} vs into the chorus {:.3}", r[15], r[11]);
    // Inside a phrase: it opens stronger than its middle.
    let opening = mean(&[r[0], r[4], r[8], r[12], r[16], r[20]]);
    let middle = mean(&[r[1], r[2], r[5], r[6], r[9], r[10], r[13], r[14], r[17], r[18], r[21], r[22]]);
    assert!(opening > middle + 0.01, "phrase opening {opening:.3} vs middle {middle:.3}");
}

#[test]
fn no_two_four_bar_phrases_in_a_row_are_the_same() {
    for s in test_songs() {
        let t = play(&s, Mode::Band, 1);
        for k in (0..6).filter(|&k| t.enabled[k]) {
            let r = repetition(&t, k);
            assert_eq!(r.same_phrase.0, 0, "{} m{}: {} of {} phrases repeat the one before", s.name, k + 1, r.same_phrase.0, r.same_phrase.1);
        }
    }
}

/// Onsets of the non-drummers in the last two beats of `bar`, off the beat.
fn off_beat_onsets(t: &Take, bar: i64) -> usize {
    let bar_ms = ms_per_quarter(t) * bar_quarters(t);
    let from = bar as f64 * bar_ms + bar_ms * 0.5;
    notes(&t.events)
        .iter()
        .filter(|n| !is_drums(t, n.musician) && n.ms >= from - 5.0 && n.ms < (bar + 1) as f64 * bar_ms - 30.0)
        .filter(|n| {
            let (_, p) = grid_offset(t, n);
            p % 4 != 0
        })
        .count()
}

#[test]
fn the_drummer_marks_section_changes_and_the_others_answer() {
    let s = song("pop-verse-chorus-bridge");
    let mut marked = 0;
    let mut changes = 0;
    for take in 1..=3 {
        let band = play(&s, Mode::Band, take);
        let nat = play(&s, Mode::Natural, take);
        let fills = fill_bars(&band);
        for b in section_starts(&band).into_iter().filter(|&b| b > 0) {
            changes += 1;
            if fills.contains(&(b - 1)) {
                marked += 1;
                // While he fills, the others keep to the beats (they answer the fill instead of playing through it).
                let (ob, on) = (off_beat_onsets(&band, b - 1), off_beat_onsets(&nat, b - 1));
                assert!(ob * 2 <= on.max(1), "take {take}, bar {}: {ob} off-beat notes under the fill (Natural {on})", b - 1);
            }
        }
    }
    assert!(marked * 10 >= changes * 8, "the drummer marked {marked} of {changes} section changes");
}

#[test]
fn every_note_is_playable() {
    for s in test_songs() {
        let band = play(&s, Mode::Band, 1);
        for n in notes(&band.events).iter().filter(|n| !is_drums(&band, n.musician)) {
            let (lo, hi) = playable_range(band.instruments[n.musician]);
            assert!((lo..=hi).contains(&i32::from(n.pitch)), "{} m{} plays {} outside {lo}..{hi}", s.name, n.musician + 1, n.pitch);
        }
    }
}

#[test]
fn chords_strike_like_hands_and_sections_do_not_roll() {
    let s = song("pop-verse-chorus-bridge");
    let nat = play(&s, Mode::Natural, 1);
    let band = play(&s, Mode::Band, 1);
    let lowest_first = |t: &Take, k: usize| {
        let st = strikes(t, k);
        (st.iter().filter(|x| x.2.first() < x.2.last()).count() as f64 / st.len().max(1) as f64, st.len())
    };
    // m1 the piano, m4 the strings, m5 the brass.
    for k in [0usize, 3, 4] {
        let (n, _) = lowest_first(&nat, k);
        let (b, count) = lowest_first(&band, k);
        assert!(count >= 20, "m{} struck {count} chords", k + 1);
        assert!(n > 0.95, "Natural m{} rolls up {:.0} % (the gap this layer closes)", k + 1, n * 100.0);
        assert!(b < 0.7, "as a band m{} still rolls up {:.0} % of its chords", k + 1, b * 100.0);
    }
    // The ballad's pianist still rolls on purpose sometimes: not never, not always.
    let ballad = play(&song("slow-ballad"), Mode::Band, 1);
    let (b, _) = lowest_first(&ballad, 0);
    assert!((0.15..0.8).contains(&b), "the ballad's piano rolls up {:.0} %", b * 100.0);
}

#[test]
fn the_band_lays_back_into_the_final_chord() {
    let s = song("slow-ballad");
    let nat = play(&s, Mode::Natural, 1);
    let band = play(&s, Mode::Band, 1);
    // The Ending's final chord starts at bar 20.
    let bar_ms = ms_per_quarter(&nat) * bar_quarters(&nat);
    let at = 20.0 * bar_ms;
    let first = |t: &Take| notes(&t.events).iter().filter(|n| (n.ms - at).abs() < 120.0).map(|n| n.ms - at).collect::<Vec<f64>>();
    let (n, b) = (first(&nat), first(&band));
    assert!(!n.is_empty() && !b.is_empty());
    assert!(mean(&b) > mean(&n) + 20.0, "final chord {:+.1} ms as a band vs {:+.1} ms", mean(&b), mean(&n));
    // ... and together: every player within 40 ms of the others.
    let spread = b.iter().copied().fold(f64::MIN, f64::max) - b.iter().copied().fold(f64::MAX, f64::min);
    assert!(spread < 40.0, "final chord spread {spread:.1} ms");
}
