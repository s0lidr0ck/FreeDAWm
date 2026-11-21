//! The reader against real AAF files and a known answer for each, when they are on this machine.
//!
//! `FREEDAWM_AAF_SAMPLES` names a folder holding `aaf/*.aaf` and `expected/<name>.aaf.expected`
//! (a text dump of each file: tracks, clips with positions in samples, fades, crossfades, gains,
//! mute, solo, pan and markers). Unset, the test prints a note and passes: the samples are not part
//! of the repository.
//!
//! Every file must parse; each is then compared with its dump (positions within one sample: the
//! dump rounds some edit-unit conversions differently) and a score is printed. The score may not
//! drop below [`FLOOR`].

use soundcraft_aaf::desc::{Curve, Import};
use std::path::Path;

/// Files that must match their dump fully.
const FLOOR: usize = 62;

#[derive(Debug, Default)]
struct ExpClip {
    /// Timecode text of the start and length, when the dump shows positions as timecode.
    tc: Option<(String, String)>,
    start: i64,
    len: i64,
    offset: i64,
    channels: usize,
    fade_in: Option<(String, i64)>,
    fade_out: Option<(String, i64)>,
    gain: Option<f32>,
    muted: bool,
}

#[derive(Debug, Default)]
struct ExpTrack {
    clips: Vec<ExpClip>,
    xfades: Vec<(String, i64)>,
    mute: bool,
    solo: bool,
    pan: Option<f32>,
}

#[derive(Debug, Default)]
struct Expected {
    /// The timecode the dump counts in: (edit rate, nominal fps, drop frame).
    tc: Option<((i64, i64), i64, bool)>,
    rate: u32,
    start: i64,
    tracks: Vec<ExpTrack>,
    markers: Vec<String>,
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let i = line.find(key)? + key.len();
    line[i..].split_whitespace().next()
}

fn num(line: &str, key: &str) -> Option<i64> {
    field(line, key)?.parse().ok()
}

fn fade(line: &str, key: &str) -> Option<(String, i64)> {
    let i = line.find(key)? + key.len();
    let rest = &line[i..];
    let mut w = rest.split_whitespace();
    let curve = w.next()?.to_string();
    let len = w.next()?.trim_matches(|c| c == '(' || c == ')').parse().ok()?;
    Some((curve, len))
}

fn parse(text: &str) -> Expected {
    let mut e = Expected::default();
    let (mut end_eu, mut end_rate, mut end_samples) = (0i64, (1i64, 1i64), 0i64);
    for line in text.lines() {
        let l = line.trim_start_matches([' ', '│', '├', '└', '─']);
        if l.starts_with("TC EditRrate") {
            if let Some(r) = l.rsplit(':').next() {
                let mut p = r.trim().split('/');
                let rate = (p.next().and_then(|x| x.parse().ok()).unwrap_or(1), p.next().and_then(|x| x.parse().ok()).unwrap_or(1));
                e.tc = Some((rate, 0, false));
            }
        } else if l.starts_with("TC FPS") {
            let v = l.rsplit(':').next().unwrap_or("").trim().to_string();
            if let Some(tc) = &mut e.tc {
                tc.1 = v.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
                tc.2 = v.ends_with(" DF");
            }
        } else if l.starts_with("Composition Start (samples)") {
            e.start = l.rsplit(':').next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        } else if l.starts_with("Composition End (EU)") {
            end_eu = num(l, ": ").unwrap_or(0);
            if let Some(r) = field(l, "EditRate: ") {
                let r = r.trim_end_matches(')');
                let mut p = r.split('/');
                end_rate = (p.next().and_then(|x| x.parse().ok()).unwrap_or(1), p.next().and_then(|x| x.parse().ok()).unwrap_or(1));
            }
        } else if l.starts_with("Composition End (samples)") {
            end_samples = l.rsplit(':').next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        } else if l.starts_with("AudioTrack[") {
            let mut t = ExpTrack { mute: l.contains("Mute: MUTE"), solo: l.contains("Solo: SOLO"), ..ExpTrack::default() };
            if let Some(p) = field(l, "Pan: ") {
                t.pan = p.parse().ok();
            }
            e.tracks.push(t);
        } else if l.starts_with("Clip (") {
            let start_text = field(l, "Start: ").unwrap_or("");
            let tc =
                (start_text.contains(':') || start_text.contains(';')).then(|| (start_text.to_string(), field(l, "Len: ").unwrap_or("").to_string()));
            let c = ExpClip {
                tc,
                start: num(l, "Start: ").unwrap_or(0),
                len: num(l, "Len: ").unwrap_or(0),
                offset: num(l, "SourceOffset: ").unwrap_or(0),
                channels: num(l, "Channels: ").unwrap_or(0) as usize,
                fade_in: fade(l, "FadeIn: "),
                fade_out: fade(l, "FadeOut: "),
                gain: field(l, "Gain: ").and_then(|g| g.parse().ok()),
                muted: l.contains("Mute: MUTE"),
            };
            if let Some(t) = e.tracks.last_mut() {
                t.clips.push(c);
            }
        } else if l.starts_with("X-FADE: ") {
            let curve = field(l, "X-FADE: ").unwrap_or("").to_string();
            if let Some(t) = e.tracks.last_mut() {
                t.xfades.push((curve, num(l, "Length: ").unwrap_or(0)));
            }
        } else if l.starts_with("Marker[") {
            let label = l.find("Label: \"").and_then(|i| l[i + 8..].split('"').next()).unwrap_or("").to_string();
            e.markers.push(label);
        }
    }
    // The rate the dump's samples are in.
    e.rate = if end_eu > 0 {
        let r = end_samples as f64 * end_rate.0 as f64 / (end_eu as f64 * end_rate.1 as f64);
        [8000u32, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000]
            .into_iter()
            .min_by(|a, b| (f64::from(*a) - r).abs().total_cmp(&(f64::from(*b) - r).abs()))
            .unwrap_or(48000)
    } else {
        48000
    };
    e
}

fn curve_name(c: Curve) -> &'static str {
    match c {
        Curve::Linear => "CURV_LIN",
        Curve::Power => "CURV_PWR",
        Curve::Log => "CURV_LOG",
        Curve::Exp => "CURV_EXP",
    }
}

/// `frames` as timecode text at `fps`, drop frame counted the SMPTE way (`;` before the frames).
fn tc_text(frames: i64, fps: i64, drop: bool) -> String {
    let fps = fps.max(1);
    let mut f = frames.max(0);
    if drop {
        let d = fps / 15; // 2 at 30, 4 at 60, 8 at 120
        let per10 = fps * 600 - 9 * d;
        let per1 = fps * 60 - d;
        let (tens, rem) = (f / per10, f % per10);
        f += 9 * d * tens + if rem > d { d * ((rem - d) / per1) } else { 0 };
    }
    let (h, m, sec, fr) = (f / (fps * 3600), (f / (fps * 60)) % 60, (f / fps) % 60, f % fps);
    format!("{h:02}:{m:02}:{sec:02}{}{fr:02}", if drop { ';' } else { ':' })
}

fn near(a: i64, b: i64) -> bool {
    (a - b).abs() <= 1
}

/// The differences between what was read and the dump.
fn compare(got: &Import, want: &Expected) -> Vec<String> {
    let mut out = Vec::new();
    if got.tracks.len() != want.tracks.len() {
        out.push(format!("tracks: {} read, {} expected", got.tracks.len(), want.tracks.len()));
    }
    let names: Vec<&str> = got.markers.iter().map(|m| m.name.as_str()).collect();
    if names != want.markers.iter().map(String::as_str).collect::<Vec<_>>() {
        out.push(format!("markers: {names:?} read, {:?} expected", want.markers));
    }
    for (ti, (g, w)) in got.tracks.iter().zip(&want.tracks).enumerate() {
        let t = ti + 1;
        if g.clips.len() != w.clips.len() {
            out.push(format!("track {t}: {} clips read, {} expected", g.clips.len(), w.clips.len()));
        }
        if g.mute != w.mute || g.solo != w.solo {
            out.push(format!("track {t}: mute/solo {}/{} read, {}/{} expected", g.mute, g.solo, w.mute, w.solo));
        }
        if let Some(p) = w.pan {
            let gp = g.pan.map(|x| (f64::from(x) + 1.0) / 2.0);
            if gp.is_none_or(|x| (x - f64::from(p)).abs() > 0.01) {
                out.push(format!("track {t}: pan {gp:?} read, {p} expected"));
            }
        }
        for (ci, (gc, wc)) in g.clips.iter().zip(&w.clips).enumerate() {
            let c = ci + 1;
            let start = gc.start + got.start;
            if let (Some((ws, wl)), Some(((num, den), fps, drop))) = (&wc.tc, want.tc) {
                // Positions as timecode: frames at the timecode's edit rate.
                let frames = |x: i64| ((x as f64) * num as f64 / (got.sample_rate as f64 * den as f64)).round() as i64;
                let tc_start = got.timecode.map_or(0, |t| t.frames);
                let (gs, gl) = (tc_text(tc_start + frames(gc.start), fps, drop), tc_text(frames(gc.len), fps, drop));
                if gs != *ws || gl != *wl {
                    out.push(format!("track {t} clip {c}: at {gs}+{gl} read, {ws}+{wl} expected"));
                }
                continue;
            }
            if !near(start, wc.start) || !near(gc.len, wc.len) {
                out.push(format!("track {t} clip {c}: at {start}+{} read, {}+{} expected", gc.len, wc.start, wc.len));
            }
            if !near(gc.offset, wc.offset) {
                out.push(format!("track {t} clip {c}: source offset {} read, {} expected", gc.offset, wc.offset));
            }
            if gc.channels.len() != wc.channels {
                out.push(format!("track {t} clip {c}: {} channels read, {} expected", gc.channels.len(), wc.channels));
            }
            let mine = |f: Option<soundcraft_aaf::desc::Fade>| f.filter(|f| !f.cross).map(|f| (curve_name(f.curve).to_string(), f.len));
            for (what, g, w) in [("fade in", mine(gc.fade_in), &wc.fade_in), ("fade out", mine(gc.fade_out), &wc.fade_out)] {
                let same = match (&g, w) {
                    (None, None) => true,
                    (Some((gc, gl)), Some((wc, wl))) => gc == wc && near(*gl, *wl),
                    _ => false,
                };
                if !same {
                    out.push(format!("track {t} clip {c}: {what} {g:?} read, {w:?} expected"));
                }
            }
            if let Some(wg) = wc.gain
                && gc.gain_db.is_none_or(|g| (g - wg).abs() > 0.06)
            {
                out.push(format!("track {t} clip {c}: gain {:?} read, {wg} expected", gc.gain_db));
            }
            if gc.muted != wc.muted {
                out.push(format!("track {t} clip {c}: muted {} read, {} expected", gc.muted, wc.muted));
            }
        }
        let xf: Vec<(String, i64)> =
            g.clips.iter().filter_map(|c| c.fade_in.filter(|f| f.cross)).map(|f| (curve_name(f.curve).to_string(), f.len)).collect();
        let same = xf.len() == w.xfades.len() && xf.iter().zip(&w.xfades).all(|(a, b)| a.0 == b.0 && near(a.1, b.1));
        if !same {
            out.push(format!("track {t}: crossfades {xf:?} read, {:?} expected", w.xfades));
        }
    }
    out
}

#[test]
fn every_sample_parses_and_matches_its_dump() {
    let Some(dir) = std::env::var_os("FREEDAWM_AAF_SAMPLES") else {
        println!("FREEDAWM_AAF_SAMPLES is not set: the AAF sample comparison is skipped");
        return;
    };
    let dir = Path::new(&dir);
    let mut files: Vec<_> = std::fs::read_dir(dir.join("aaf")).map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|x| x == "aaf"));
    files.sort();
    assert!(!files.is_empty(), "no .aaf files in {}", dir.join("aaf").display());
    let (mut full, mut failed_parse) = (0usize, Vec::new());
    let mut report = String::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(f).unwrap();
        let expected = std::fs::read_to_string(dir.join("expected").join(format!("{name}.expected"))).ok().map(|t| parse(&t));
        let rate = expected.as_ref().map_or(48000, |e| e.rate);
        // The options the dumps were made with (Pro Tools fade handling).
        let pt_true_fades = name == "PT_Fades_B.aaf" || name.starts_with("PT_lang_");
        let pt_remove_sae = name == "PT_Fades_A.aaf" || name.starts_with("PT_Multichannel_");
        let opts = soundcraft_aaf::Options { sample_rate: rate, pt_true_fades, pt_remove_sae, ..Default::default() };
        let got = match soundcraft_aaf::read(&bytes, &opts) {
            Ok(g) => g,
            Err(e) => {
                failed_parse.push(format!("{name}: {e}"));
                continue;
            }
        };
        let Some(want) = expected else {
            report.push_str(&format!("{name}: no dump\n"));
            continue;
        };
        let diffs = compare(&got, &want);
        if diffs.is_empty() {
            full += 1;
            report.push_str(&format!("{name}: ok ({} tracks, {} clips)\n", got.tracks.len(), got.clip_count()));
        } else {
            report.push_str(&format!("{name}: {} differences\n", diffs.len()));
            for d in diffs.iter().take(8) {
                report.push_str(&format!("    {d}\n"));
            }
        }
    }
    println!("{report}");
    println!("{full} of {} files match their dump fully", files.len());
    assert!(failed_parse.is_empty(), "files that did not parse: {failed_parse:#?}");
    assert!(full >= FLOOR, "{full} files match, fewer than {FLOOR}");
}
