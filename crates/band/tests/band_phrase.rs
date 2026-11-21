//! Phrases ("As a band"): measured on the factory song (four-bar sections, and the same song with eight-bar sections)
//! played by the Factory Band. Each musician plays a part with statement, answer, restatement and turnaround instead
//! of one bar over and over; the part varies, but it stays a part (not a coin toss every bar); downbeats stay; a
//! chorus repeat is the same chorus; only one player is busy at a time. With "As a band" off nothing here plays (the
//! goldens and the fidelity suite pin Classic and Natural).

mod feel_support;

use feel_support::{
    Mode, Take, bar_of, bar_shapes, distinct_bars, factory_song, grid_offset, is_drums, kit, mean, notes, passes_alike, play, section_len,
    section_passes,
};

const TAKES: [i32; 3] = [1, 2, 3];

/// Mean distinct bars per eight-bar stretch, per musician (silent stretches left out), over the takes.
fn distinct_per_eight(mode: Mode, double: bool) -> Vec<(String, f64, usize)> {
    let s = factory_song("Factory Band", double, &[]);
    let mut out: Vec<(String, Vec<f64>, usize)> = Vec::new();
    for take in TAKES {
        let t = play(&s, mode, take);
        for k in (0..6).filter(|&k| t.enabled[k]) {
            let shapes = bar_shapes(&t, k, false);
            if out.iter().all(|o| o.0 != t.names[k]) {
                out.push((t.names[k].clone(), Vec::new(), 0));
            }
            let Some(o) = out.iter_mut().find(|o| o.0 == t.names[k]) else { continue };
            for w in (0..shapes.len()).step_by(8) {
                let (d, played) = distinct_bars(&shapes, w, (w + 8).min(shapes.len()));
                if played >= 6 {
                    o.1.push(d as f64);
                    o.2 = o.2.max(d);
                }
            }
        }
    }
    out.into_iter().map(|(n, v, most)| (n, mean(&v), most)).collect()
}

#[test]
fn a_take_plays_its_phrases_the_same_every_time() {
    let s = factory_song("Factory Band", true, &[]);
    let a = play(&s, Mode::Band, 1);
    assert_eq!(a.events, play(&s, Mode::Band, 1).events, "the same song, settings and Take must give the same performance");
    assert_ne!(a.events, play(&s, Mode::Band, 2).events, "another Take plays differently");
}

#[test]
fn distinct_bars_per_eight_bar_section_rise_but_stay_a_part() {
    for double in [false, true] {
        let nat = distinct_per_eight(Mode::Natural, double);
        let band = distinct_per_eight(Mode::Band, double);
        for ((name, n, _), (_, b, most)) in nat.iter().zip(&band) {
            if *n < 6.0 {
                // A part that repeats: it varies now.
                assert!(*b >= n + 0.1, "{name} (8-bar sections {double}): {b:.2} distinct bars in eight, {n:.2} without phrases");
            } else {
                // A part that already changes nearly every bar by chance (the brass at Energy 4): as varied, but now
                // as phrases (see `the_second_half_of_a_phrase_opens_like_the_first`).
                assert!(*b >= n - 0.5, "{name} (8-bar sections {double}): {b:.2} distinct bars in eight, {n:.2} without phrases");
            }
            // The ceiling: variation, not a new bar every bar.
            assert!(*b <= 6.9, "{name} (8-bar sections {double}): {b:.2} distinct bars in eight is no longer a part");
            assert!(*most <= 8);
        }
        // The drummer, the most repetitive of them, varies the most.
        let drums = |v: &[(String, f64, usize)]| v.iter().find(|x| x.0.contains("Drums")).map_or(0.0, |x| x.1);
        assert!(drums(&band) >= drums(&nat) + 1.0, "drums {:.2} against {:.2}", drums(&band), drums(&nat));
    }
}

#[test]
fn the_second_half_of_a_phrase_opens_like_the_first() {
    // Eight-bar sections: bar 5 is bar 1 again (a statement restated), more often than without phrases.
    let s = factory_song("Factory Band", true, &[]);
    let restated = |mode: Mode| {
        let mut same = 0;
        let mut all = 0;
        for take in TAKES {
            let t = play(&s, mode, take);
            for k in (0..6).filter(|&k| t.enabled[k]) {
                let shapes = bar_shapes(&t, k, false);
                for (id, starts) in section_passes(&t) {
                    if section_len(&t, id) != 8 {
                        continue;
                    }
                    for &b in &starts {
                        let (Some(a), Some(c)) = (shapes.get(b as usize), shapes.get(b as usize + 4)) else { continue };
                        if !a.is_empty() {
                            all += 1;
                            same += usize::from(a == c);
                        }
                    }
                }
            }
        }
        same as f64 / all.max(1) as f64
    };
    let (n, b) = (restated(Mode::Natural), restated(Mode::Band));
    assert!(b >= n, "bar 5 restates bar 1 in {:.0} % of phrases, {:.0} % without phrases", b * 100.0, n * 100.0);
    assert!(b >= 0.5, "bar 5 restates bar 1 in only {:.0} % of phrases", b * 100.0);
}

#[test]
fn downbeats_keep_the_kick_and_the_root() {
    for double in [false, true] {
        let s = factory_song("Factory Band", double, &[]);
        for take in TAKES {
            let nat = play(&s, Mode::Natural, take);
            let band = play(&s, Mode::Band, take);
            let d = (0..6).find(|&k| is_drums(&band, k) && band.enabled[k]).unwrap_or(0);
            let b = (0..6).find(|&k| band.instruments[k] == 2 && band.enabled[k]).unwrap_or(0);
            let downbeat = |t: &Take, k: usize, bar: i64, want: &dyn Fn(u8) -> bool| {
                notes(&t.events).iter().any(|n| n.musician == k && bar_of(t, n) == bar && grid_offset(t, n).1 == 0 && want(n.pitch))
            };
            for bar in 0..i64::from(s.bars) {
                let kick = |p: u8| kit(p) == feel_support::Kit::Kick;
                if downbeat(&nat, d, bar, &kick) {
                    assert!(downbeat(&band, d, bar, &kick), "take {take}, bar {}: the downbeat kick is gone", bar + 1);
                }
                // The bass's downbeat note: the same note as without phrases.
                let root = |t: &Take| {
                    notes(&t.events).iter().filter(|n| n.musician == b && bar_of(t, n) == bar && grid_offset(t, n).1 == 0).map(|n| n.pitch % 12).min()
                };
                if let Some(r) = root(&nat) {
                    assert_eq!(root(&band), Some(r), "take {take}, bar {}: the bass's downbeat changed", bar + 1);
                }
            }
        }
    }
}

#[test]
fn a_chorus_repeat_is_the_same_chorus_a_little_different() {
    let s = factory_song("Factory Band", false, &[]);
    let mut differs = 0;
    for take in TAKES {
        let nat = play(&s, Mode::Natural, take);
        let t = play(&s, Mode::Band, take);
        for k in (0..6).filter(|&k| t.enabled[k]) {
            // About as alike as the passes are without phrases (the piano's pushes and the strings' coin tosses make
            // them differ there too; as a band the others also make room for the drummer's fills, which fall
            // differently before a chorus than before the bridge).
            let (n, b) = (passes_alike(&nat, k), passes_alike(&t, k));
            assert!(
                b >= n - 0.2 && b >= 0.3,
                "take {take}, {}: passes of a section {:.0} % alike, {:.0} % without phrases",
                t.names[k],
                b * 100.0,
                n * 100.0
            );
            differs += usize::from(b < 0.98);
        }
    }
    assert!(differs >= 6, "the second pass of a section should differ a little ({differs} players did)");
}

/// Onsets of musician `k` in `bar`.
fn onsets(t: &Take, k: usize, bar: i64) -> usize {
    notes(&t.events)
        .iter()
        .filter(|n| n.musician == k && bar_of(t, n) == bar)
        .map(|n| grid_offset(t, n).1)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

#[test]
fn when_the_drummer_fills_the_others_leave_room() {
    // Bars where the drummer plays a fill (a turn into the next section): the others play fewer onsets there than in
    // the bars of their phrases the drummer leaves alone.
    let s = factory_song("Factory Band", true, &[]);
    let (mut in_fill, mut fills) = (0usize, 0usize);
    let (mut elsewhere, mut others) = (0usize, 0usize);
    for take in TAKES {
        let t = play(&s, Mode::Band, take);
        let fill = feel_support::fill_bars(&t);
        for k in (0..6).filter(|&k| t.enabled[k] && !is_drums(&t, k)) {
            for bar in 0..i64::from(s.bars) {
                let n = onsets(&t, k, bar);
                if fill.contains(&bar) {
                    in_fill += n;
                    fills += 1;
                } else {
                    elsewhere += n;
                    others += 1;
                }
            }
        }
    }
    let (f, e) = (in_fill as f64 / fills.max(1) as f64, elsewhere as f64 / others.max(1) as f64);
    assert!(fills > 0 && f < e, "{f:.2} onsets a bar under a drum fill, {e:.2} elsewhere");
}

#[test]
fn only_one_player_is_busy_in_a_bar_and_the_spotlight_moves() {
    use soundcraft_band::band_phrase::{GROUP_BASS, GROUP_CHORDS, GROUP_DRUMS, GROUP_SECTION, PhraseAt, device, has_spotlight};
    // The Factory Band as the phrase layer sees it: piano, drums, bass, strings, brass in slots 1 to 5.
    let band = [(0, GROUP_CHORDS), (1, GROUP_DRUMS), (2, GROUP_BASS), (3, GROUP_SECTION), (4, GROUP_SECTION)];
    for take in 1..=12 {
        let mut spot_bars = [0usize; 5];
        for section in 0..3 {
            for occurrence in 0..3 {
                for bars in [4, 8] {
                    for bar in 0..bars {
                        let mut busy = 0;
                        for (i, &(musician, group)) in band.iter().enumerate() {
                            for density in [1, 3, 8] {
                                let p = PhraseAt { take, musician, group, section, occurrence, bar, bars, density };
                                if device(&p).0.busy() {
                                    assert!(has_spotlight(&p), "take {take}: slot {musician} busy without the spotlight");
                                    busy += usize::from(density == 3);
                                }
                                spot_bars[i] += usize::from(density == 3 && has_spotlight(&p));
                            }
                        }
                        assert!(busy <= 1, "take {take}: {busy} players busy in bar {bar} of {bars}");
                    }
                }
            }
        }
        assert!(spot_bars.iter().all(|&n| n >= 6), "take {take}: the spotlight stays with a few: {spot_bars:?}");
    }
}

#[test]
fn golden_factory_song_as_a_band() {
    // The factory song played by the Factory Band as a band, Take 1, bar by bar (onsets and pitches from the chord's
    // root, octaves counted; velocity and timing are the band-feel layer's and pinned by its own tests). Regenerate
    // with FREEDAWM_BLESS_GOLDENS=1 after a deliberate change, and listen to it.
    let s = factory_song("Factory Band", false, &[]);
    let t = play(&s, Mode::Band, 1);
    let mut text = String::new();
    for k in (0..6).filter(|&k| t.enabled[k]) {
        for (b, shape) in bar_shapes(&t, k, true).iter().enumerate() {
            let cells: Vec<String> = shape.iter().map(|(p, n)| format!("{p}:{n}")).collect();
            text += &format!("m{} bar {:2}: {}\n", k + 1, b + 1, cells.join(" "));
        }
    }
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/band-phrase-factory.txt");
    if std::env::var_os("FREEDAWM_BLESS_GOLDENS").is_some() {
        assert!(std::fs::write(&path, &text).is_ok(), "could not write {}", path.display());
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    let first_diff = want.lines().zip(text.lines()).position(|(a, b)| a != b).map_or(want.lines().count().min(text.lines().count()), |i| i);
    assert!(want == text, "the factory song as a band changed near line {} (FREEDAWM_BLESS_GOLDENS=1 to accept)", first_diff + 1);
}
