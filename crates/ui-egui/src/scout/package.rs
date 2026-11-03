//! The Scout package: Scout2's `ScoutPackage` layout (`ScoutPackage.cpp`, Full detail, the 31-band
//! curve included) versioned forward as "0.6.0-daw", with the strips' real identity, chain,
//! routing and the session around them. The derived values (tilt, resonances) and the flags are a
//! line-by-line translation of `derive` and `flagsFor`; the rounding is the C++ `r2`/`r1`.
//!
//! Built only on request (Copy, Send, `ui.scout.package`), never per frame.

use crate::module_drawer::{dial_name, drawer_shows, value_text};
use serde_json::{Value, json};
use soundcraft_dsp::ParamInfo;
use soundcraft_dsp::plugins::fs::cleanq::{F_DYN, F_ON, F_RANGE, F_SLOPE, F_THRESHOLD, F_TYPE, NUM_BAND_FIELDS, NUM_GLOBAL_PARAMS};
use soundcraft_dsp::scout::iso_centre;
use soundcraft_model::{Insert, Mixer, Route, ScoutCapture, ScoutMetrics, ScoutProfile, ScoutStrip, Session};

/// `scout_payload_version` of every package this build writes.
pub const PAYLOAD_VERSION: &str = "0.6.0-daw";

/// Two decimal places, as the original's `r2()`. A non-finite value becomes `null`.
fn r2(v: f64) -> Value {
    num((v * 100.0).round() / 100.0)
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn num(v: f64) -> Value {
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}

/// `combiningNote`, verbatim.
const COMBINING_NOTE: &str = "Several Scout packages may be pasted together. Match them by track.name: a Base and a Post with the same name are one processing chain (Post minus Base = what the effects did); different names are different tracks of the same mix. Analyze whatever set is present.";

/// `knownLimitations`, verbatim, then what the in-DAW capture adds.
const KNOWN_LIMITATIONS: [&str; 10] = [
    "Spectrum uses 31 one-third-octave IIR bandpass filters (not FFT) - coarse but zero-latency",
    "LUFS/LRA use a close BS.1770 approximation (biquad K-weighting), not a certified meter",
    "True peak uses 4x Hermite interpolation - an estimate, not the BS.1770 FIR",
    "Spectral centroid is band-energy weighted",
    "Transient detection uses fixed 10ms-window RMS comparison, not sample-accurate onset detection",
    "Single capture window - does not represent full song dynamics",
    "Stereo width is correlation-based estimate, not full spatial analysis",
    "Surround strips are measured on the ITU stereo fold-down",
    "Captures are at most 30 seconds",
    "Frames captured while the transport was stopped before Play are included (silence or live input)",
];

// ---------------------------------------------------------------- derived values

/// `Derived`: the spectrum's tilt and the bands standing above it.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub tilt_db_per_octave: f64,
    /// (centre Hz, dB above the trend), largest first, at most 3.
    pub resonances: Vec<(f64, f64)>,
}

/// `derive`: least-squares slope of the spectrum against log2(frequency) over bands 5..27
/// (63 Hz .. 10 kHz), and the bands at least 2 dB above that line and above both neighbours.
pub fn derive(m: &ScoutMetrics) -> Derived {
    const LO: usize = 5;
    const HI: usize = 27;
    let n = (HI - LO + 1) as f64;
    let sp = |i: usize| f64::from(m.spectrum31.get(i).copied().unwrap_or(0.0));
    let x = |i: usize| f64::from(iso_centre(i)).log2();

    let (mut mx, mut my) = (0.0, 0.0);
    for i in LO..=HI {
        mx += x(i);
        my += sp(i);
    }
    mx /= n;
    my /= n;

    let (mut sxy, mut sxx) = (0.0, 0.0);
    for i in LO..=HI {
        let dx = x(i) - mx;
        sxy += dx * (sp(i) - my);
        sxx += dx * dx;
    }
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let icpt = my - slope * mx;
    let tilt = (slope * 100.0).round() / 100.0;

    let mut resonances = Vec::new();
    for i in 3..=28 {
        let above = sp(i) - (icpt + slope * x(i));
        if above >= 2.0 && sp(i) >= sp(i - 1) && sp(i) >= sp(i + 1) {
            resonances.push((f64::from(iso_centre(i)), r1(above)));
        }
    }
    resonances.sort_by(|a, b| b.1.total_cmp(&a.1));
    resonances.truncate(3);
    Derived { tilt_db_per_octave: tilt, resonances }
}

/// One `scout_flags` entry: severity, id, message.
pub type Flag = (&'static str, &'static str, String);

/// `flagsFor`, in its order and with its thresholds and wording.
pub fn flags_for(b: &ScoutMetrics) -> Vec<Flag> {
    let mut f: Vec<Flag> = Vec::new();
    if b.clipped_samples > 0 {
        f.push(("warning", "clipping_in_source", format!("{} clipped samples in signal", b.clipped_samples)));
    }
    if b.activity_ratio < 0.3 {
        f.push(("info", "low_activity", "Capture has significant silence - metrics may not be representative".into()));
    }
    if b.crest_factor_db > 20.0 {
        f.push(("info", "high_dynamic_range", format!("Crest factor {:.1} dB - may benefit from compression", b.crest_factor_db)));
    }
    if b.crest_factor_db < 4.0 {
        f.push(("warning", "possible_overcompression", format!("Crest factor {:.1} dB suggests heavy compression", b.crest_factor_db)));
    }
    if b.rms_dbfs < -30.0 {
        f.push(("info", "low_level", format!("RMS {:.1} dBFS - signal may need gain staging", b.rms_dbfs)));
    }
    if b.dc_offset > 0.01 {
        f.push(("warning", "dc_offset_present", format!("DC offset {:.3} detected - consider a high-pass filter", b.dc_offset)));
    }
    if b.true_peak_dbtp > -1.0 {
        f.push(("warning", "true_peak_over", format!("True peak {:.1} dBTP exceeds -1 dBTP streaming target", b.true_peak_dbtp)));
    }
    if b.stereo_correlation < 0.3 {
        f.push(("warning", "low_mono_compatibility", format!("Stereo correlation {:.2} may cause mono compatibility issues", b.stereo_correlation)));
    }
    if b.low_freq_correlation < 0.0 {
        f.push((
            "warning",
            "low_freq_phase",
            format!("Low-frequency correlation {:.2} indicates bass phase issues below 150 Hz", b.low_freq_correlation),
        ));
    }
    if b.lr_balance_db.abs() > 3.0 {
        f.push(("info", "channel_imbalance", format!("L/R balance {:.1} dB favors one channel", b.lr_balance_db)));
    }
    let d = derive(b);
    if d.tilt_db_per_octave < -4.5 {
        f.push(("info", "dark_balance", format!("Spectral tilt {:.2} dB/oct - darker than typical (0 = pink)", d.tilt_db_per_octave)));
    }
    if d.tilt_db_per_octave > 0.5 {
        f.push(("info", "bright_balance", format!("Spectral tilt {:.2} dB/oct - brighter than pink", d.tilt_db_per_octave)));
    }
    if let Some(&(hz, above)) = d.resonances.first()
        && above >= 5.0
    {
        f.push(("info", "resonance", format!("{} Hz stands {above:.1} dB above the trend - possible resonance", hz.round() as i64)));
    }
    if b.mid_side_ratio_db > -60.0 && b.side_low_ratio_db > -6.0 {
        f.push((
            "warning",
            "wide_low_end",
            format!("Much of the side energy is below 150 Hz ({:.1} dB) - consider mono-ing the lows", b.side_low_ratio_db),
        ));
    }
    f
}

/// `formatCapture` (curves included).
fn format_capture(r: &ScoutMetrics) -> Value {
    let f = |v: f32| r2(f64::from(v));
    // `! (midSideRatioDb > -60)`: NaN counts as mono.
    let mono = r.mid_side_ratio_db.partial_cmp(&-60.0) != Some(std::cmp::Ordering::Greater);
    let d = derive(r);
    json!({
        "capture_quality": {
            "valid": r.valid,
            "activity_ratio": f(r.activity_ratio),
            "clipped_samples": r.clipped_samples,
            "peak_hold_dbfs": f(r.peak_hold_dbfs),
            "noise_floor_p10_dbfs": f(r.noise_floor_p10_dbfs),
            "dc_offset": f(r.dc_offset),
        },
        "level": {
            "peak_dbfs": f(r.peak_dbfs),
            "true_peak_dbtp": f(r.true_peak_dbtp),
            "rms_dbfs": f(r.rms_dbfs),
            "lufs_integrated": f(r.lufs_integrated),
        },
        "dynamics": {
            "crest_factor_db": f(r.crest_factor_db),
            "dynamic_range_db": f(r.dynamic_range_db),
            "loudness_range_lra": f(r.lra),
            "peak_to_lufs_db": f(r.peak_to_lufs_db),
            "transient_density": f(r.transient_density),
        },
        "loudness_distribution": {
            "p10_lufs": f(r.lufs_p10),
            "p50_lufs": f(r.lufs_p50),
            "p90_lufs": f(r.lufs_p90),
            "momentary_max_lufs": f(r.lufs_momentary_max),
            "short_term_max_lufs": f(r.lufs_short_term_max),
            "note": "percentiles of 400 ms K-weighted blocks above the -70 LUFS gate; p90 minus p10 is the working dynamic span",
        },
        "spectrum_31_band": r.spectrum31.iter().map(|&s| f(s)).collect::<Vec<_>>(),
        "broad_zones": {
            "sub_20_60": f(r.zone_sub),
            "low_60_150": f(r.zone_low),
            "low_mid_150_500": f(r.zone_low_mid),
            "mid_500_2000": f(r.zone_mid),
            "high_mid_2000_6000": f(r.zone_high_mid),
            "high_6000_10000": f(r.zone_high),
            "air_10000_20000": f(r.zone_air),
        },
        "stereo": {
            "correlation": f(r.stereo_correlation),
            "width": f(r.stereo_width),
            "low_freq_correlation": f(r.low_freq_correlation),
            "lr_balance_db": f(r.lr_balance_db),
            "mono_source": mono,
            "side_to_mid_db": if mono { Value::Null } else { f(r.mid_side_ratio_db) },
            "side_energy_below_150hz_db": if mono { Value::Null } else { f(r.side_low_ratio_db) },
        },
        "tonal": {
            "spectral_centroid_hz": f(r.spectral_centroid_hz),
            "tilt_db_per_octave": num(d.tilt_db_per_octave),
            "resonances_above_trend": d.resonances.iter().map(|&(hz, above)| json!({"hz": hz.round() as i64, "above_trend_db": num(above)})).collect::<Vec<_>>(),
        },
    })
}

// ---------------------------------------------------------------- targets

/// How far a strip is from its target: target minus strip. Spectrum bands compare shape (each
/// spectrum less its mean over 63 Hz..10 kHz), since loudness is its own number.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetDiff {
    pub bands: [f32; soundcraft_model::SCOUT_BANDS],
    pub lufs: f32,
    pub lra: f32,
    pub crest: f32,
    pub width: f32,
    pub correlation: f32,
    pub tilt: f64,
    pub centroid_hz: f32,
    pub transient_density: f32,
}

fn shape_of(m: &ScoutMetrics) -> [f32; soundcraft_model::SCOUT_BANDS] {
    let mid: Vec<f32> = m.spectrum31.iter().copied().skip(5).take(23).collect();
    let mean = if mid.is_empty() { 0.0 } else { mid.iter().sum::<f32>() / mid.len() as f32 };
    let mut out = m.spectrum31;
    out.iter_mut().for_each(|v| *v -= mean);
    out
}

/// `target` minus `m`.
pub fn target_diff(m: &ScoutMetrics, target: &ScoutMetrics) -> TargetDiff {
    let (a, b) = (shape_of(m), shape_of(target));
    let mut bands = [0.0; soundcraft_model::SCOUT_BANDS];
    for ((d, x), y) in bands.iter_mut().zip(&a).zip(&b) {
        *d = y - x;
    }
    TargetDiff {
        bands,
        lufs: target.lufs_integrated - m.lufs_integrated,
        lra: target.lra - m.lra,
        crest: target.crest_factor_db - m.crest_factor_db,
        width: target.stereo_width - m.stereo_width,
        correlation: target.stereo_correlation - m.stereo_correlation,
        tilt: derive(target).tilt_db_per_octave - derive(m).tilt_db_per_octave,
        centroid_hz: target.spectral_centroid_hz - m.spectral_centroid_hz,
        transient_density: target.transient_density - m.transient_density,
    }
}

/// The metrics a strip's target is compared with: its output (post, or base without a chain).
pub fn compared(st: &ScoutStrip) -> &ScoutMetrics {
    if has_chain(st) { &st.post } else { &st.base }
}

/// A track's `target` block: the profile, its numbers and the differences.
fn target_block(m: &ScoutMetrics, t: &ScoutProfile) -> Value {
    let d = target_diff(m, &t.metrics);
    let f = |v: f32| r2(f64::from(v));
    json!({
        "name": t.name,
        "kind": t.kind.as_str(),
        "role": t.role,
        "source": t.source_text(),
        "spectrum_bands_version": t.bands_version,
        "capture": format_capture(&t.metrics),
        "difference_target_minus_this": {
            "lufs_integrated": f(d.lufs),
            "loudness_range_lra": f(d.lra),
            "crest_factor_db": f(d.crest),
            "stereo_width": f(d.width),
            "stereo_correlation": f(d.correlation),
            "tilt_db_per_octave": r2(d.tilt),
            "spectral_centroid_hz": f(d.centroid_hz),
            "transient_density": f(d.transient_density),
            "spectrum_shape_31_band": d.bands.iter().map(|&v| f(v)).collect::<Vec<_>>(),
        },
    })
}

// ---------------------------------------------------------------- chain and routing

/// A parameter's current value, as the drawer reads it.
fn param_value(ins: &Insert, p: &ParamInfo) -> f32 {
    p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default))
}

/// Parameters the drawer shows that do nothing as the module stands, so the package leaves them
/// out: a Clean Q band that is off, a band's slope unless it is a cut (Low Cut 3, High Cut 4), its
/// threshold and range while it isn't dynamic, and the dynamic ceiling's settings while it is off.
fn inert(ins: &Insert, p: &ParamInfo) -> bool {
    if ins.plugin != "fs_cleanq" {
        return false;
    }
    let info = soundcraft_dsp::plugin_info("fs_cleanq");
    let at = |idx: usize| info.and_then(|i| i.params.get(idx)).map_or(0.0, |q| param_value(ins, q));
    if p.group == "Ceiling" {
        return p.id != "ceilOn" && ins.params.get("ceilOn").copied().unwrap_or(0.0) < 0.5;
    }
    let Some(idx) = info.and_then(|i| i.params.iter().position(|q| q.id == p.id)) else { return false };
    if idx < NUM_GLOBAL_PARAMS {
        return false;
    }
    let (slot, field) = ((idx - NUM_GLOBAL_PARAMS) / NUM_BAND_FIELDS, (idx - NUM_GLOBAL_PARAMS) % NUM_BAND_FIELDS);
    let base = NUM_GLOBAL_PARAMS + slot * NUM_BAND_FIELDS;
    if at(base + F_ON) < 0.5 {
        return true;
    }
    let cut = matches!(at(base + F_TYPE).round() as i32, 3 | 4);
    (field == F_SLOPE && !cut) || ((field == F_THRESHOLD || field == F_RANGE) && at(base + F_DYN) < 0.5)
}

/// The module's displayed name and its plugin's full name.
fn module_names(ins: &Insert) -> (String, String) {
    match soundcraft_dsp::plugin_info(&ins.plugin) {
        Some(i) => (i.short_name.to_string(), i.name.to_string()),
        None => (ins.plugin.clone(), ins.plugin.clone()),
    }
}

fn chain(ctx: &egui::Context, m: &Mixer) -> Value {
    m.inserts
        .iter()
        .enumerate()
        .map(|(i, ins)| {
            let (short, full) = module_names(ins);
            let params: Vec<Value> = soundcraft_dsp::plugin_info(&ins.plugin)
                .map(|info| {
                    info.params
                        .iter()
                        .filter(|p| drawer_shows(ins, p) && !inert(ins, p))
                        .map(|p| json!({"name": dial_name(ins, p.id).unwrap_or(p.name), "value": value_text(ctx, ins, p, param_value(ins, p))}))
                        .collect()
                })
                .unwrap_or_default();
            json!({"position": i + 1, "plugin": full, "name": short, "bypassed": ins.bypass || !ins.active, "params": params})
        })
        .collect()
}

fn route_name(s: &Session, r: &Route) -> Value {
    match r {
        Route::None | Route::Musician(_) | Route::Modules(_) | Route::Midi(_) => Value::Null,
        Route::Main => json!(s.outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone())),
        Route::Bus(b) => json!(s.bus(*b).map_or_else(|| "missing bus".to_string(), |b| b.name.clone())),
        Route::Hardware(h) => json!(h),
    }
}

fn routing(s: &Session, m: &Mixer) -> Value {
    let sends: Vec<Value> = m
        .sends
        .iter()
        .flatten()
        .map(|x| json!({"bus": route_name(s, &x.target), "level_db": r2(f64::from(x.level_db)), "pre_fader": x.pre_fader, "muted": x.mute}))
        .collect();
    let sidechain = m.inserts.iter().find_map(|ins| {
        let sc = ins.sidechain?;
        let source = s.track(sc.track).map_or_else(|| "missing track".to_string(), |t| t.name.clone());
        Some(json!({"module": module_names(ins).0, "source": source, "pre_fader": sc.pre_fader}))
    });
    json!({"output": route_name(s, &m.output), "sends": sends, "sidechain": sidechain})
}

fn automated(ctx: &egui::Context, st: &ScoutStrip) -> Value {
    st.automated
        .iter()
        .map(|a| {
            let ins = st.mixer.inserts.iter().find(|i| i.id == a.module);
            let p = ins.and_then(|i| soundcraft_dsp::plugin_info(&i.plugin)).and_then(|info| info.params.iter().find(|p| p.id == a.param));
            match (ins, p) {
                (Some(ins), Some(p)) => json!({
                    "module": module_names(ins).0,
                    "param": dial_name(ins, p.id).unwrap_or(p.name),
                    "start": value_text(ctx, ins, p, a.start),
                    "end": value_text(ctx, ins, p, a.end),
                }),
                _ => json!({"module": ins.map(|i| module_names(i).0), "param": a.param, "start": num(f64::from(a.start)), "end": num(f64::from(a.end))}),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- captures

/// Whether a strip's POST differs from its PRE: it has a module in play.
fn has_chain(st: &ScoutStrip) -> bool {
    st.mixer.inserts.iter().any(|i| i.active && !i.bypass)
}

fn secs(c: &ScoutCapture, at: i64) -> Value {
    if c.sample_rate > 0.0 { r2(at as f64 / c.sample_rate) } else { Value::Null }
}

/// The captured length in seconds: the played ranges, else (a capture while stopped) the meter's.
fn duration(c: &ScoutCapture, m: &ScoutMetrics) -> Value {
    let played: i64 = c.played.iter().map(|r| r.end.saturating_sub(r.start).max(0)).fold(0, i64::saturating_add);
    if played > 0 && c.sample_rate > 0.0 { r2(played as f64 / c.sample_rate) } else { r2(f64::from(m.duration_sec)) }
}

fn capture_info(s: &Session, c: &ScoutCapture, same: Option<u32>) -> Value {
    let same = same.and_then(|id| s.scout.captures.iter().find(|x| x.id == id)).map(|x| json!({"id": x.id, "name": x.name}));
    json!({
        "id": c.id,
        "name": c.name,
        "created": c.created,
        "sample_rate": r2(c.sample_rate),
        "tempo": r2(c.tempo),
        "played_ranges_s": c.played.iter().map(|r| json!([secs(c, r.start), secs(c, r.end)])).collect::<Vec<_>>(),
        "played_truncated": c.played_truncated,
        "loop": c.loop_range.map(|r| json!({"start_s": secs(c, r.start), "end_s": secs(c, r.end)})),
        "same_section_as": same,
        "spectrum_bands_version": c.bands_version,
    })
}

/// `buildTrack`: one strip at one capture position.
fn build_track(ctx: &egui::Context, s: &Session, c: &ScoutCapture, st: &ScoutStrip, post: bool) -> Value {
    let m = if post { &st.post } else { &st.base };
    let name = st.name.trim();
    let color = s.track(st.track).map(|t| format!("#{:02x}{:02x}{:02x}", t.color[0], t.color[1], t.color[2]));
    let flags: Vec<Value> = flags_for(m).into_iter().map(|(sev, id, msg)| json!({"severity": sev, "id": id, "message": msg})).collect();
    let mut t = json!({
        "name": if name.is_empty() { "Unnamed track" } else { name },
        "role": st.role,
        "channel_type": st.channel_type,
        "capture_position": if post { "post" } else { "base" },
        "capture_duration_s": duration(c, m),
        "capture_id": c.id,
        "track_id": st.track.0,
        "color": color,
        "chain": chain(ctx, &st.mixer),
        "routing": routing(s, &st.mixer),
        "automated_during_capture": automated(ctx, st),
        "capture": format_capture(m),
        "scout_flags": flags,
    });
    if let (Some(note), Some(o)) = (&st.note, t.as_object_mut()) {
        o.insert("note".into(), json!(note));
    }
    t
}

fn interpretation_guide() -> Value {
    json!({
        "dbfs_reference": "0 dBFS = digital full scale",
        "lufs_reference": "EBU R128, target varies by genre (-14 LUFS streaming, -16 broadcast)",
        "delta_meaning": "post minus base; positive = boost, negative = cut",
        "crest_factor": "Peak minus RMS in dB; lower = more compressed",
        "true_peak": "Inter-sample peak in dBTP; streaming target <= -1 dBTP",
        "loudness_range": "EBU LRA in LU; macro-dynamics across the capture, higher = more dynamic",
        "spectral_centroid": "Brightness center of mass in Hz (band-energy weighted); higher = brighter",
        "low_freq_correlation": "Phase coherence below 150 Hz; negative = bass phase issues",
        "tilt_db_per_octave": "Slope of the 63 Hz-10 kHz spectrum in 1/3-octave bands: 0 = pink-noise balance, typical mixed material -1 to -3, more negative = darker, positive = brighter than pink",
        "resonances_above_trend": "Bands standing above the tilt line; candidates for narrow EQ cuts",
        "spectrum_bands_version": "Per capture. 2 = 6th-order Butterworth 1/3-octave bands (steep skirts; deep cuts read at their real depth). 1 = older 2nd-order bands: a loud neighbour leaks in, so cuts read shallower; broadband material reads about 1.5 dB higher per band below 5 kHz, about the same near 10 kHz and lower in the top three bands (12.5 kHz about -1, 16 kHz about -2 to -3, 20 kHz about -6 to -7 dB), and tilt about 0.1 dB/oct darker. Compare spectra only between captures of the same version",
        "side_to_mid_db": "Side energy relative to mid; null = mono, around -6 to -12 dB is moderate width",
        "noise_floor_p10_dbfs": "RMS of the quietest 10% of 10 ms windows",
        "base_without_post": "A track sent with a base capture only has no module in play: its post would be identical",
        "target": "A sound profile the engineer is shooting for (measured from a reference sound, stem or mix): its numbers and difference_target_minus_this; spectrum_shape_31_band compares each spectrum less its 63 Hz-10 kHz mean",
        "chain": "The track's modules in order with their settings as the engineer sees them; parameters that do nothing as set are left out",
    })
}

/// `addResponseInstructions` (Full detail), with the Clean Q preset replaced by guides.
fn response_instructions(multi: bool, goal: bool) -> Value {
    let mut constraints = vec![
        "Reference specific numeric values from the data",
        "Suggest specific parameter ranges when recommending adjustments",
        "Consider the track role when evaluating if processing is appropriate",
        "If a Base and Post package share a name, evaluate what the chain did and whether it helps or hurts",
        "If several tracks are present, consider how they interact; if asked for rough-mix levels or pans, give a per-track table: fader offset in dB relative to the loudest track, pan, one-line reason",
    ];
    if multi {
        constraints.push(
            "The tracks in this package were captured from one session; pairs sharing a name with capture_position base and post are the two ends of one chain",
        );
    }
    if goal {
        constraints.push("analysis_request.goal carries the engineer's goal for this session - prioritize it");
    }
    constraints.push("Never tell the engineer to apply a preset; give ranges and targets the engineer will set by hand");
    json!({
        "format": "detailed_analysis",
        "style": "Thorough analysis with reasoning. Reference specific measurements, explain trade-offs, suggest parameter ranges.",
        "sections": "summary,observations,suggestions,warnings",
        "constraints": constraints,
        "avoid": [
            "Do not recommend specific plugin brands unless asked",
            "Do not repeat back raw numbers without interpretation",
            "Do not explain what measurements mean - the user is an audio engineer",
        ],
        "guides": soundcraft_engine::scout_prompt::guides_format(),
    })
}

/// The package for `captures` (ids, in that order; unknown ids are skipped), the question and
/// the session goal. One strip at one position is a `single_capture` with `track`; anything more
/// is a `multi_capture` with `tracks`, sorted by capture, then name, base before post.
pub fn build_package(ctx: &egui::Context, s: &Session, captures: &[u32], question: &str, goal: &str) -> Value {
    let same = soundcraft_engine::cmd::scout_same_as(&s.scout);
    let mut caps: Vec<(&ScoutCapture, Option<u32>)> = Vec::new();
    for id in captures {
        if let Some(i) = s.scout.captures.iter().position(|c| c.id == *id)
            && !caps.iter().any(|(c, _)| c.id == *id)
            && let Some(c) = s.scout.captures.get(i)
        {
            caps.push((c, same.get(i).copied().flatten()));
        }
    }
    let mut tracks: Vec<Value> = Vec::new();
    for (c, _) in &caps {
        let mut strips: Vec<&ScoutStrip> = c.strips.iter().collect();
        strips.sort_by_key(|st| st.name.to_lowercase());
        for st in strips {
            tracks.push(build_track(ctx, s, c, st, false));
            if has_chain(st) {
                tracks.push(build_track(ctx, s, c, st, true));
            }
            // The target goes with the strip's output (its post, or its base without a chain).
            if let (Some(t), Some(o)) = (&st.target, tracks.last_mut().and_then(Value::as_object_mut)) {
                o.insert("target".into(), target_block(compared(st), t));
            }
        }
    }
    let multi = tracks.len() != 1;
    let first = caps.first().map(|(c, _)| *c);
    let sample_rate = first.map_or(s.sample_rate.as_f64(), |c| c.sample_rate);
    let post_only = !multi && tracks.first().is_some_and(|t| t["capture_position"] == "post");
    let question = question.trim();
    let user_question = if !question.is_empty() {
        question.to_string()
    } else if multi {
        "Evaluate these captures together: judge what any base/post chains did, and how the tracks sit against each other.".into()
    } else if post_only {
        "Evaluate this processed signal; if a Base package with the same name is present, judge what the chain did.".into()
    } else {
        "What processing would you suggest for this track?".into()
    };
    let focus = if multi {
        "multi_track_evaluation"
    } else if post_only {
        "effect_chain_evaluation"
    } else {
        "processing_recommendations"
    };
    let goal = goal.trim();
    let mut p = serde_json::Map::new();
    p.insert("scout_payload_version".into(), json!(PAYLOAD_VERSION));
    p.insert("payload_type".into(), json!(if multi { "multi_capture" } else { "single_capture" }));
    p.insert("created_at".into(), json!(caps.iter().map(|(c, _)| c.created.as_str()).max().unwrap_or("")));
    p.insert("combining_packages".into(), json!(COMBINING_NOTE));
    if multi {
        let one = caps.len() == 1;
        p.insert(
            "capture_sync".into(),
            json!({
                "simultaneous": one,
                "group_id": if one { first.map_or(0, |c| c.id) } else { 0 },
                "note": if one {
                    "Every track here was captured from the same moment of the same playback, started together on the host timeline. Differences between them are differences in the signal, not in what was playing. A base and a post sharing a name are the two ends of one chain over identical audio, so post minus base is exactly what the chain did."
                } else {
                    "These captures were taken separately, at different times. Treat differences between tracks with care: they may reflect different passages rather than different processing."
                },
            }),
        );
    }
    p.insert("session".into(), json!({"name": s.name, "sample_rate": r2(sample_rate), "tempo": first.map_or(Value::Null, |c| r2(c.tempo))}));
    let mut project = json!({"sample_rate": r2(sample_rate)});
    if multi && let Some(o) = project.as_object_mut() {
        o.insert("track_count".into(), json!(tracks.len()));
    }
    p.insert("project".into(), project);
    match caps.as_slice() {
        [(c, same)] => {
            p.insert("capture".into(), capture_info(s, c, *same));
        }
        _ => {
            p.insert("captures".into(), caps.iter().map(|(c, same)| capture_info(s, c, *same)).collect());
        }
    }
    p.insert("analysis_request".into(), json!({"user_question": user_question, "focus": focus, "goal": goal}));
    p.insert("interpretation_guide".into(), interpretation_guide());
    if multi {
        p.insert("tracks".into(), Value::Array(tracks));
    } else if let Some(t) = tracks.pop() {
        p.insert("track".into(), t);
    }
    p.insert("known_limitations".into(), json!(KNOWN_LIMITATIONS));
    let targeted = caps.iter().any(|(c, _)| c.strips.iter().any(|st| st.target.is_some()));
    let mut instructions = response_instructions(multi, !goal.is_empty());
    if targeted && let Some(a) = instructions["constraints"].as_array_mut() {
        a.push(json!("A track with a target: compare its numbers with target.capture (the same measurements, side by side), say where it differs most, and suggest how to move it toward the target (difference_target_minus_this: positive = the target has more); spectrum differences are of shape, loudness is lufs_integrated. A mix target on the master is a mastering target"));
    }
    p.insert("ai_response_instructions".into(), instructions);
    Value::Object(p)
}

/// The exact text sent to Claude and put on the clipboard by Copy: the question, then the package.
pub fn prompt_text(package: &Value, question: &str) -> String {
    let q = question.trim();
    let q = if q.is_empty() { package["analysis_request"]["user_question"].as_str().unwrap_or("") } else { q };
    let body = serde_json::to_string_pretty(package).unwrap_or_default();
    let targets = target_lines(package);
    let q = if targets.is_empty() { q.to_string() } else { format!("{q}\n\n{targets}") };
    format!(
        "{q}\n\nThe Scout package for this question follows (measurements, each track's chain and routing). Follow its ai_response_instructions.\n\n```json\n{body}\n```\n"
    )
}

/// "Move <track> toward the target; the target is <name> (<source>)." per track with a target.
fn target_lines(package: &Value) -> String {
    let tracks: Vec<&Value> = match package.get("tracks").and_then(Value::as_array) {
        Some(a) => a.iter().collect(),
        None => package.get("track").into_iter().collect(),
    };
    let mut out: Vec<String> = Vec::new();
    for t in tracks {
        if let Some(tg) = t.get("target") {
            let name = t["name"].as_str().unwrap_or("");
            let tname = tg["name"].as_str().unwrap_or("");
            let src = tg["source"].as_str().unwrap_or("");
            let src = if src.is_empty() { String::new() } else { format!(" ({src})") };
            let what = if tg["kind"] == "mix" { "mastering target" } else { "target" };
            out.push(format!("Compare {name} with its {what} and move it toward the target; the target is {tname}{src}."));
        }
    }
    out.join("\n")
}
