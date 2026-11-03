//! Fidelity of the FireStation engine ports against the unmodified C++ render tools.
//!
//! Each engine's `tests/fidelity/<engine>/cases.json` lists cases; `cargo xtask fidelity-inputs`
//! and `cargo xtask fidelity-refs <engine>` make the (local, git-ignored) input and reference audio.
//! Every case must come within -60 dB RMS of its reference, with no non-finite output. A case may
//! carry a written-down `"exception": {"pass_db", "reason"}`; it is judged by that bar instead and
//! the reason is printed with the table. An exception's bar must be finite and no looser than
//! -40 dB, and a case that passes the normal -60 dB bar while it has an exception fails as a STALE
//! EXCEPTION (so an exception cannot linger and later hide a regression).
//!
//! The render mirrors `FireStation/Tools/renderfile.cpp`: prepare at the case's rate (a plain tag:
//! the same input samples at every rate), THEN set the case's values, then process in the case's
//! block size. Values are set in the plugin's parameter order, the order the C++ wrapper pushes
//! them to its engine at the start of the first block.

use std::path::{Path, PathBuf};

const PASS_DB: f32 = -60.0;

/// The difference between a reference and a render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Diff {
    /// RMS of the difference relative to the reference RMS, in dB.
    pub rms_rel_db: f32,
    /// Peak of the difference relative to the reference RMS, in dB.
    pub peak_rel_db: f32,
    /// Non-finite samples in `actual`.
    pub nonfinite: usize,
    /// Channels missing on either side plus, per channel, the length difference.
    pub len_mismatch: usize,
}

/// Compares over the common length of each channel, relative to the reference RMS over all
/// channels (floored at 1e-9).
pub fn compare(reference: &[Vec<f32>], actual: &[Vec<f32>]) -> Diff {
    let mut ref_sq = 0.0f64;
    let mut ref_n = 0usize;
    let mut diff_sq = 0.0f64;
    let mut diff_n = 0usize;
    let mut peak = 0.0f64;
    let mut nonfinite = 0usize;
    let mut len_mismatch = reference.len().abs_diff(actual.len());
    for (r, a) in reference.iter().zip(actual) {
        len_mismatch += r.len().abs_diff(a.len());
        for (&x, &y) in r.iter().zip(a) {
            let (x, y) = (f64::from(x), f64::from(y));
            ref_sq += x * x;
            ref_n += 1;
            if !y.is_finite() {
                nonfinite += 1;
                continue;
            }
            let d = y - x;
            diff_sq += d * d;
            diff_n += 1;
            peak = peak.max(d.abs());
        }
    }
    let ref_rms = (ref_sq / ref_n.max(1) as f64).sqrt().max(1e-9);
    let diff_rms = (diff_sq / diff_n.max(1) as f64).sqrt();
    let db = |v: f64| (20.0 * (v / ref_rms).max(1e-30).log10()) as f32;
    Diff { rms_rel_db: db(diff_rms), peak_rel_db: db(peak), nonfinite, len_mismatch }
}

struct Case {
    /// The case's JSON entry as written, for its hash.
    raw: serde_json::Value,
    name: String,
    input: String,
    sr: u32,
    block: usize,
    values: Vec<(String, f32)>,
    /// Values set after `values` have settled (`"after"`; the render tool's `--editor --after`).
    after: Vec<(String, f32)>,
    /// State properties (`"props"`; the render tool's `--prop`): AmpEngine's `irPath<Amp>` names a
    /// bundled IR, `assets/amp-irs/<name>`, handed to the plugin as an asset under that key.
    props: Vec<(String, String)>,
    /// A written-down exception to the pass bar, `"exception": {"pass_db": <dB>, "reason": <why>}`
    /// (not part of the case hash: it says how the render is judged, not how it is made).
    exception: Option<(f32, String)>,
}

/// The loosest bar an exception may set: anything above it is not a port, it is a different sound.
const EXCEPTION_LIMIT_DB: f32 = -40.0;

/// A case's `"exception"` entry: its pass bar (finite, at or below [`EXCEPTION_LIMIT_DB`]) and its
/// written reason (not empty).
fn parse_exception(e: &serde_json::Value) -> Result<(f32, String), String> {
    let db = e["pass_db"].as_f64().ok_or("exception: `pass_db` must be a number")? as f32;
    if !db.is_finite() || db > EXCEPTION_LIMIT_DB {
        return Err(format!("exception: `pass_db` {db} must be finite and at most {EXCEPTION_LIMIT_DB} dB"));
    }
    let why = e["reason"].as_str().filter(|r| !r.trim().is_empty()).ok_or("exception: `reason` must be written down")?;
    Ok((db, why.to_string()))
}

/// How a case did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Ok,
    /// Passed its exception's bar (and not the normal one).
    OkException,
    Fail,
    /// Has an exception but passes the normal bar: the exception must go.
    StaleException,
}

fn judge(d: &Diff, exception: Option<f32>) -> Verdict {
    let clean = d.nonfinite == 0 && d.len_mismatch == 0;
    let passes = |bar: f32| clean && d.rms_rel_db <= bar;
    match exception {
        None if passes(PASS_DB) => Verdict::Ok,
        Some(_) if passes(PASS_DB) => Verdict::StaleException,
        Some(bar) if passes(bar) => Verdict::OkException,
        _ => Verdict::Fail,
    }
}

fn fidelity_dir() -> PathBuf {
    Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())).join("tests/fidelity")
}

fn load_cases(path: &Path) -> Vec<Case> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let arr = json.as_array().unwrap_or_else(|| panic!("{}: expected an array of cases", path.display()));
    arr.iter()
        .map(|c| Case {
            raw: c.clone(),
            name: c["name"].as_str().expect("case name").to_string(),
            input: c["input"].as_str().expect("case input").to_string(),
            sr: c["sr"].as_u64().expect("case sr") as u32,
            block: c["block"].as_u64().expect("case block") as usize,
            values: c["values"]
                .as_object()
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_f64().expect("numeric value") as f32)).collect())
                .unwrap_or_default(),
            after: c
                .get("after")
                .and_then(|a| a.as_object())
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_f64().expect("numeric value") as f32)).collect())
                .unwrap_or_default(),
            props: c
                .get("props")
                .and_then(|a| a.as_object())
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().expect("string prop").to_string())).collect())
                .unwrap_or_default(),
            exception: c
                .get("exception")
                .map(|e| parse_exception(e).unwrap_or_else(|err| panic!("{}: case {}: {err}", path.display(), c["name"].as_str().unwrap_or("?")))),
        })
        .collect()
}

fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (_, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    buf.channels
}

fn render(id: &str, case: &Case, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let mut p = soundcraft_dsp::create(id).unwrap_or_else(|| panic!("unknown plugin {id}"));
    let block = case.block.max(1);
    p.prepare(case.sr as f32, block, 2);
    for info in p.info().params {
        if let Some((_, v)) = case.values.iter().find(|(k, _)| k == info.id) {
            assert!(p.set_param(info.id, *v), "{}: set_param {} failed", case.name, info.id);
        }
    }
    // `after` values land once the first ones have settled; a port pushes whatever it holds at
    // the start of the first block, so they simply override.
    for (k, v) in &case.after {
        assert!(p.set_param(k, *v), "{}: set_param {k} (after) failed", case.name);
    }
    for (k, name) in &case.props {
        // the IR as the C++ loads it: the file's first channel, made ready for this rate
        let path = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into()))
            .join("../../assets/amp-irs")
            .join(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let ir = soundcraft_dsp::plugins::fs::amp::ImpulseAsset::new(&buf.channels[0], f64::from(info.sample_rate), f64::from(case.sr))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(p.set_asset(k, Some(std::sync::Arc::new(ir))), "{}: {id} takes no asset {k}", case.name);
    }
    for (k, _) in case.values.iter().chain(&case.after) {
        assert!(p.info().param(k).is_some(), "{}: {id} has no parameter {k}", case.name);
    }
    let len = input.iter().map(Vec::len).min().unwrap_or(0);
    let mut out: Vec<Vec<f32>> = (0..2).map(|_| Vec::with_capacity(len)).collect();
    let mut buf = vec![vec![0.0f32; block]; 2];
    let mut pos = 0;
    while pos < len {
        let n = block.min(len - pos);
        for (b, src) in buf.iter_mut().zip(input.iter().cycle()) {
            b[..n].copy_from_slice(&src[pos..pos + n]);
        }
        p.process(&mut buf, n);
        for (o, b) in out.iter_mut().zip(&buf) {
            o.extend_from_slice(&b[..n]);
        }
        pos += n;
    }
    // The reference went through a 24-bit WAV writer, which clamps to [-1, 1]; the render does too.
    for c in &mut out {
        for s in c.iter_mut() {
            if s.is_finite() {
                *s = s.clamp(-1.0, 1.0);
            }
        }
    }
    out
}

fn run_engine(engine: &str) {
    let id = format!("fs_{engine}");
    let dir = fidelity_dir().join(engine);
    let cases_path = dir.join("cases.json");
    let registered = soundcraft_dsp::plugin_info(&id).is_some();
    if !cases_path.exists() {
        assert!(!registered, "{id} is registered but {} is missing", cases_path.display());
        println!("fidelity {engine}: not ported yet (no cases.json, no {id})");
        return;
    }
    let cases = load_cases(&cases_path);
    let mut missing: Vec<String> = Vec::new();
    for c in &cases {
        let refs = dir.join("refs");
        for p in [fidelity_dir().join("inputs").join(&c.input), refs.join(format!("{}.wav", c.name)), refs.join(format!("{}.hash", c.name))] {
            if !p.exists() && !missing.contains(&p.display().to_string()) {
                missing.push(p.display().to_string());
            }
        }
    }
    if !missing.is_empty() {
        let msg = format!(
            "fidelity {engine}: {} audio file(s) missing (local only, git-ignored), e.g. {}.\n\
             Run `cargo xtask fidelity-inputs` and `cargo xtask fidelity-refs {engine}` \
             (set FIDELITY_STRICT=1 to fail instead of skipping).",
            missing.len(),
            missing[0]
        );
        // The audio is local only, so a fresh worktree has none: skip with a note unless strict.
        if std::env::var("FIDELITY_STRICT").is_ok_and(|v| v == "1") {
            panic!("{msg}");
        }
        eprintln!("SKIPPED: {msg}");
        return;
    }

    let mut rows = Vec::new();
    let mut notes = Vec::new();
    let mut failed = false;
    for c in &cases {
        let stored = std::fs::read_to_string(dir.join("refs").join(format!("{}.hash", c.name))).ok();
        if let Err(e) = check_ref_hash(engine, &c.raw, stored.as_deref()) {
            failed = true;
            rows.push(format!("{:<28} STALE: {e}", c.name));
            continue;
        }
        let input = read_wav(&fidelity_dir().join("inputs").join(&c.input));
        let reference = read_wav(&dir.join("refs").join(format!("{}.wav", c.name)));
        let actual = render(&id, c, &input);
        let d = compare(&reference, &actual);
        let verdict = judge(&d, c.exception.as_ref().map(|e| e.0));
        failed |= matches!(verdict, Verdict::Fail | Verdict::StaleException);
        if let Some((db, why)) = &c.exception {
            notes.push(format!("  {}: EXCEPTION, pass <= {db} dB: {why}", c.name));
        }
        rows.push(format!(
            "{:<28} {:>9.1} {:>9.1} {:>6} {:>6}  {}",
            c.name,
            d.rms_rel_db,
            d.peak_rel_db,
            d.nonfinite,
            d.len_mismatch,
            match verdict {
                Verdict::Ok => "ok",
                Verdict::OkException => "ok (exception)",
                Verdict::Fail => "FAIL",
                Verdict::StaleException => "STALE EXCEPTION: passes the normal bar; remove its exception",
            }
        ));
    }
    let table = format!(
        "fidelity {engine} ({} cases, pass <= {PASS_DB} dB)\n{:<28} {:>9} {:>9} {:>6} {:>6}\n{}",
        cases.len(),
        "case",
        "rms dB",
        "peak dB",
        "nonfin",
        "len",
        rows.join("\n")
    );
    let table = if notes.is_empty() { table } else { format!("{table}\n{}", notes.join("\n")) };
    println!("{table}");
    assert!(!failed, "{table}");
}

/// Canonical text of one case entry: name, input, sr, block, then the values sorted by id.
/// MUST stay identical to `case_key` in `xtask/src/fidelity.rs` (both pin the same vector).
fn case_key(case: &serde_json::Value) -> String {
    let mut s = format!(
        "name={}
input={}
sr={}
block={}
",
        case["name"].as_str().unwrap_or(""),
        case["input"].as_str().unwrap_or(""),
        case["sr"].as_u64().unwrap_or(0),
        case["block"].as_u64().unwrap_or(0)
    );
    let mut values: Vec<(&String, &serde_json::Value)> = case["values"].as_object().map(|o| o.iter().collect()).unwrap_or_default();
    values.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in values {
        let v = match v {
            serde_json::Value::Number(n) => n.as_f64().map(|x| format!("{x:?}")).unwrap_or_default(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => "?".to_string(),
        };
        s.push_str(&format!(
            "{k}={v}
"
        ));
    }
    // Only cases that have `after` values carry this section, so every other key is unchanged.
    if let Some(after) = case.get("after").and_then(|a| a.as_object()) {
        let mut after: Vec<(&String, &serde_json::Value)> = after.iter().collect();
        after.sort_by(|a, b| a.0.cmp(b.0));
        s.push_str(
            "after
",
        );
        for (k, v) in after {
            let v = v.as_f64().map(|x| format!("{x:?}")).unwrap_or_else(|| "?".to_string());
            s.push_str(&format!(
                "{k}={v}
"
            ));
        }
    }
    // The same for `props` (state properties: AmpEngine's IRs).
    if let Some(props) = case.get("props").and_then(|a| a.as_object()) {
        let mut props: Vec<(&String, &serde_json::Value)> = props.iter().collect();
        props.sort_by(|a, b| a.0.cmp(b.0));
        s.push_str(
            "props
",
        );
        for (k, v) in props {
            s.push_str(&format!(
                "{k}={}
",
                v.as_str().unwrap_or("?")
            ));
        }
    }
    s
}

/// FNV-1a, 64-bit: stable across toolchains (unlike `DefaultHasher`).
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Checks that a reference was rendered from this exact case entry: `fidelity-refs` stores
/// `case <hash>` in `refs/<case>.hash` next to the WAV.
fn check_ref_hash(engine: &str, case: &serde_json::Value, stored: Option<&str>) -> Result<(), String> {
    let want = format!("{:016x}", fnv1a64(case_key(case).as_bytes()));
    let got = stored.and_then(|t| t.lines().find_map(|l| l.strip_prefix("case ")).map(str::trim));
    match got {
        Some(h) if h == want => Ok(()),
        _ => Err(format!(
            "reference does not match the current case entry (stored {}, now {want}); re-run `cargo xtask fidelity-refs {engine}`",
            got.unwrap_or("nothing")
        )),
    }
}

/// One test per engine module, named after it (`mod_engine` for Mod), run against the engine's
/// fidelity name (`tests/fidelity/<name>/`).
macro_rules! fidelity_engine {
    ($($engine:ident),*) => {$(
        #[test]
        fn $engine() {
            run_module(stringify!($engine));
        }
    )*};
}

// Every engine folder under `src/plugins/fs` (written by `build.rs`).
include!(concat!(env!("OUT_DIR"), "/fidelity_engines.rs"));

fn run_module(module: &str) {
    let reg = soundcraft_dsp::plugins::fs::engine_modules().iter().find(|(m, _)| *m == module).map(|(_, r)| *r);
    match reg.and_then(|r| r.fidelity) {
        Some(f) => run_engine(f.name),
        None => println!("fidelity {module}: the engine has no C++ original to compare with"),
    }
}

#[cfg(test)]
mod compare_tests {
    use super::*;

    fn tone(len: usize) -> Vec<f32> {
        (0..len).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()
    }

    #[test]
    fn identical_is_far_below_the_bar() {
        let r = vec![tone(4800), tone(4800)];
        let d = compare(&r, &r.clone());
        assert!(d.rms_rel_db < -200.0, "{d:?}");
        assert_eq!((d.nonfinite, d.len_mismatch), (0, 0));
    }

    #[test]
    fn one_percent_noise_is_about_minus_40() {
        let r = vec![tone(48_000), tone(48_000)];
        // Deterministic noise with exactly 1% of the reference RMS.
        let mut seed = 0x1234_5678u32;
        let mut noise: Vec<Vec<f32>> = r
            .iter()
            .map(|c| {
                c.iter()
                    .map(|_| {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
                    })
                    .collect()
            })
            .collect();
        let rms = |x: &[Vec<f32>]| (x.iter().flatten().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (x.len() * x[0].len()) as f64).sqrt();
        let k = (0.01 * rms(&r) / rms(&noise)) as f32;
        for c in &mut noise {
            c.iter_mut().for_each(|v| *v *= k);
        }
        let a: Vec<Vec<f32>> = r.iter().zip(&noise).map(|(c, n)| c.iter().zip(n).map(|(x, y)| x + y).collect()).collect();
        let d = compare(&r, &a);
        assert!((d.rms_rel_db + 40.0).abs() < 0.5, "{d:?}");
        assert!(d.peak_rel_db > d.rms_rel_db);
    }

    #[test]
    fn length_mismatch_is_counted() {
        let r = vec![tone(100), tone(100)];
        let a = vec![tone(90), tone(100)];
        assert_eq!(compare(&r, &a).len_mismatch, 10);
        assert_eq!(compare(&r, &r[..1]).len_mismatch, 1, "a missing channel");
        assert!(compare(&r, &a).rms_rel_db < -200.0, "compared over the common length");
    }

    #[test]
    fn nan_is_counted() {
        let r = vec![tone(100)];
        let mut a = r.clone();
        a[0][3] = f32::NAN;
        a[0][7] = f32::INFINITY;
        let d = compare(&r, &a);
        assert_eq!(d.nonfinite, 2);
        assert!(d.rms_rel_db.is_finite());
    }

    const PINNED_CASE: &str = r#"{"name":"x","input":"song.wav","sr":48000,"block":512,"values":{"mix":0.5,"circuit":2}}"#;

    #[test]
    fn case_hash_is_pinned() {
        // The same vector is pinned in xtask/src/fidelity.rs, so the two sides agree.
        let case: serde_json::Value = serde_json::from_str(PINNED_CASE).unwrap();
        assert_eq!(
            case_key(&case),
            "name=x
input=song.wav
sr=48000
block=512
circuit=2.0
mix=0.5
"
        );
        assert_eq!(format!("{:016x}", fnv1a64(case_key(&case).as_bytes())), PINNED);
    }

    const PINNED: &str = "ba4cd375db776a93";

    #[test]
    fn after_values_extend_the_key() {
        // The same vector is pinned in xtask/src/fidelity.rs.
        let case: serde_json::Value =
            serde_json::from_str(r#"{"name":"x","input":"song.wav","sr":48000,"block":512,"values":{"circuit":2},"after":{"tone":0.25,"drive":1}}"#)
                .unwrap();
        assert!(
            case_key(&case).ends_with(
                "circuit=2.0
after
drive=1.0
tone=0.25
"
            ),
            "{}",
            case_key(&case)
        );
        assert_eq!(format!("{:016x}", fnv1a64(case_key(&case).as_bytes())), "4723c66fb471e523");
    }

    #[test]
    fn stale_or_missing_ref_hash_fails_the_case() {
        let case: serde_json::Value = serde_json::from_str(PINNED_CASE).unwrap();
        let good = format!(
            "case {PINNED}
exe-mtime 123
"
        );
        assert!(check_ref_hash("sat", &case, Some(&good)).is_ok());
        // The case changed after its reference was rendered.
        let edited: serde_json::Value = serde_json::from_str(&PINNED_CASE.replace("0.5", "0.6")).unwrap();
        let err = check_ref_hash("sat", &edited, Some(&good)).unwrap_err();
        assert!(err.contains("cargo xtask fidelity-refs sat"), "{err}");
        assert!(check_ref_hash("sat", &case, None).is_err());
        assert!(check_ref_hash("sat", &case, Some("garbage")).is_err());
    }

    fn diff(db: f32) -> Diff {
        Diff { rms_rel_db: db, peak_rel_db: db + 10.0, nonfinite: 0, len_mismatch: 0 }
    }

    #[test]
    fn exceptions_must_be_finite_and_no_looser_than_minus_40() {
        let e = |v: serde_json::Value| parse_exception(&v);
        assert_eq!(e(serde_json::json!({"pass_db": -45.0, "reason": "1-ulp"})), Ok((-45.0, "1-ulp".to_string())));
        assert_eq!(e(serde_json::json!({"pass_db": -40.0, "reason": "edge"})).map(|x| x.0), Ok(-40.0));
        for bad in [-39.9, -10.0, 0.0, 20.0] {
            let r = e(serde_json::json!({"pass_db": bad, "reason": "why"}));
            assert!(r.as_ref().is_err_and(|m| m.contains("-40")), "{bad}: {r:?}");
        }
        // Non-finite: JSON has no NaN, so a huge number (inf as f32) or a non-number.
        let r = e(serde_json::json!({"pass_db": -1e300, "reason": "why"}));
        assert!(r.is_err(), "-1e300 is -inf as f32: {r:?}");
        assert!(e(serde_json::json!({"pass_db": "x", "reason": "why"})).is_err());
        assert!(e(serde_json::json!({"pass_db": -50.0, "reason": " "})).is_err());
    }

    #[test]
    fn an_exception_that_passes_the_normal_bar_is_stale() {
        assert_eq!(judge(&diff(-70.0), None), Verdict::Ok);
        assert_eq!(judge(&diff(-50.0), None), Verdict::Fail);
        // Comp's `preset_blue_stripe_trash`: -50.6 under its -45 dB exception.
        assert_eq!(judge(&diff(-50.6), Some(-45.0)), Verdict::OkException);
        assert_eq!(judge(&diff(-44.0), Some(-45.0)), Verdict::Fail);
        // Fixed since: the exception would hide a later regression back to -45.
        assert_eq!(judge(&diff(-60.0), Some(-45.0)), Verdict::StaleException);
        assert_eq!(judge(&diff(-90.0), Some(-45.0)), Verdict::StaleException);
        // Non-finite output or a length mismatch fails whatever the bar.
        let mut d = diff(-90.0);
        d.nonfinite = 1;
        assert_eq!(judge(&d, Some(-45.0)), Verdict::Fail);
        assert_eq!(judge(&d, None), Verdict::Fail);
    }

    #[test]
    fn silent_reference_is_floored() {
        let d = compare(&[vec![0.0; 10]], &[vec![1e-6; 10]]);
        assert!((d.rms_rel_db - 60.0).abs() < 0.01, "{d:?}");
    }
}
