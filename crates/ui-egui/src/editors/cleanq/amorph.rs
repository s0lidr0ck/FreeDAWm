//! `CleanQAmorphEq.h` and the editor's Copy / Paste: the Amorph build's readable "amorph-eq"
//! text both ways (Paste also takes FireStation's preset text), and a dropped `.json` file.

use super::*;
use serde_json::Map;

/// A cut's default Q in the Amorph text (`CleanQAmorphEq.h`'s literal, not exactly 1/sqrt 2).
#[allow(clippy::approx_constant)]
const CUT_Q: f64 = 0.7071;

// ============================================================================ CleanQAmorphEq.h
const TYPE_KEYS: [&str; 7] = ["bell", "lowshelf", "highshelf", "lowcut", "highcut", "notch", "bandpass"];
const PLACE_KEYS: [&str; 5] = ["stereo", "left", "right", "mid", "side"];
const SLOPES_DB: [i32; 8] = [6, 12, 18, 24, 36, 48, 72, 96];

fn round_to(v: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (v * m).round() / m
}

/// Copy: the Amorph build's readable text (`amorph::write`).
pub(super) fn copy_text(ui: &Ui, cx: &Cx, st: &mut Editor) {
    let ins = cx.ins();
    let mut o = Map::new();
    o.insert("format".into(), json!("amorph-eq"));
    o.insert("version".into(), json!(1));
    if !ins.preset.is_empty() {
        o.insert("name".into(), json!(ins.preset));
    }
    o.insert("output_gain_db".into(), json!(round_to(q::knob_to_out_db(pv(ins, "outGain")), 2)));
    o.insert("phase_invert".into(), json!(pv(ins, "invert") > 0.5));
    let att = q::knob_to_attack_ms(pv(ins, "dynAttack"));
    let rel = q::knob_to_release_ms(pv(ins, "dynRelease"));
    let mut list = Vec::new();
    for b in cx.bands.iter().filter(|b| b.used()) {
        let mut x = Map::new();
        x.insert("band".into(), json!(b.slot + 1));
        x.insert("on".into(), json!(b.active()));
        x.insert("type".into(), json!(TYPE_KEYS[b.kind.clamp(0, 6) as usize]));
        x.insert("freq".into(), json!(round_to(b.freq, 2)));
        if b.place != q::PLACE_STEREO {
            x.insert("channel".into(), json!(PLACE_KEYS[b.place.clamp(0, 4) as usize]));
        }
        if b.has_gain() {
            x.insert("gain".into(), json!(round_to(b.gain, 2)));
        }
        if b.kind == q::TYPE_LOW_CUT || b.kind == q::TYPE_HIGH_CUT {
            x.insert("slope".into(), json!(SLOPES_DB[b.slope.clamp(0, 7) as usize]));
        }
        x.insert("q".into(), json!(round_to(b.q, 3)));
        if b.dyn_ && b.has_gain() {
            x.insert(
                "dynamic".into(),
                json!({"on": true, "threshold": round_to(f64::from(pv(ins, bp(b.slot, cq::F_THRESHOLD))), 1), "range": round_to(f64::from(pv(ins, bp(b.slot, cq::F_RANGE))), 1), "attack_ms": round_to(att, 2), "release_ms": round_to(rel, 1)}),
            );
        }
        list.push(Value::Object(x));
    }
    o.insert("bands".into(), Value::Array(list));
    let marks: Vec<Value> = cx
        .guides()
        .iter()
        .map(|g| json!({"freq": round_to(g.freq_hz, 1), "lo": round_to(g.lo_hz, 1), "hi": round_to(g.hi_hz, 1), "gain": round_to(g.gain_db, 1), "label": g.note, "dynamic": g.dynamic}))
        .collect();
    o.insert("markers".into(), Value::Array(marks));
    ui.ctx().copy_text(serde_json::to_string_pretty(&Value::Object(o)).unwrap_or_default());
    st.message = Some(("Copied".into(), 0.0));
}

/// `extractJson`: a ```json fence (the last), else the outermost braces.
fn extract_json(text: &str) -> String {
    let t = text.trim().trim_start_matches('\u{FEFF}');
    if let Some(open) = t.rfind("```json")
        && let Some(nl) = t[open..].find('\n').map(|n| open + n)
        && let Some(end) = t[nl..].find("```").map(|e| nl + e)
    {
        return t[nl + 1..end].trim().to_string();
    }
    match (t.find('{'), t.rfind('}')) {
        (Some(a), Some(b)) if b > a => t[a..=b].to_string(),
        _ => t.to_string(),
    }
}

fn num(o: &Value, keys: &[&str], fallback: f64) -> Option<f64> {
    for k in keys {
        match o.get(*k) {
            Some(Value::Number(n)) => return n.as_f64(),
            Some(Value::String(s)) if s.chars().any(|c| c.is_ascii_digit()) => {
                return Some(s.trim().trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.').parse::<f64>().unwrap_or(fallback));
            }
            _ => {}
        }
    }
    None
}

/// Paste: FireStation preset text first, else the Amorph text (`amorph::parse` + `loadAmorphText`).
pub(super) fn paste_text(app: &mut SoundApp, cx: &Cx, st: &mut Editor, text: &str) {
    let info = cx.host.info;
    // FireStation's own preset text first -- unless it is plainly the Amorph text (its `bands` /
    // `markers` arrays), which the FireStation reader would take as an empty preset.
    let amorph = serde_json::from_str::<Value>(&extract_json(text))
        .is_ok_and(|v| v.get("bands").is_some() || v.get("markers").is_some() || v.get("guides").is_some());
    if !amorph && let Some((name, map)) = crate::fs_presets::factory_recall(info, "Pasted", text).filter(|(_, m)| !m.is_empty()) {
        let list: Vec<(String, f32)> = map.iter().filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f as f32))).collect();
        let refs: Vec<(&str, f32)> = list.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        params(app, cx, &refs);
        select(st, None);
        st.message = Some((format!("Pasted {name} \u{2014} Save to keep"), 0.0));
        return;
    }
    let msg = match load_amorph(app, cx, st, text) {
        Ok(m) | Err(m) => m,
    };
    st.message = Some((msg, 0.0));
}

fn load_amorph(app: &mut SoundApp, cx: &Cx, st: &mut Editor, text: &str) -> Result<String, String> {
    let json: Value = serde_json::from_str(&extract_json(text)).map_err(|_| "Not a preset: not JSON".to_string())?;
    if !json.is_object() {
        return Err("Not a preset: not JSON".into());
    }
    let bands = json.get("bands").and_then(Value::as_array);
    let markers = json.get("markers").and_then(Value::as_array).or_else(|| json.get("guides").and_then(Value::as_array));
    if bands.is_none() && markers.is_none() {
        return Err("Not a preset: needs a \"bands\" or \"markers\" array".into());
    }
    let mut problems: Vec<String> = Vec::new();
    let mut list: Vec<(&'static str, f32)> = Vec::new();
    let mut n_bands = 0;
    if let Some(arr) = bands {
        let mut timing_done = false;
        let mut slot = 0;
        for (n, pb) in arr.iter().enumerate() {
            let label = format!("band {}", num(pb, &["band"], (n + 1) as f64).unwrap_or((n + 1) as f64) as i64);
            let tv = pb.get("type").cloned().unwrap_or(json!("bell"));
            let kind = match &tv {
                Value::Number(x) => x.as_f64().map_or(-1, |v| v as i32),
                Value::String(s) => {
                    let k: String = s.to_lowercase().chars().filter(|c| !matches!(c, ' ' | '_' | '-')).collect();
                    TYPE_KEYS.iter().position(|t| *t == k).map_or(-1, |i| i as i32)
                }
                _ => -1,
            };
            if !(0..q::NUM_TYPES).contains(&kind) {
                problems.push(format!("{label}: unknown type '{}'", tv.as_str().unwrap_or("?")));
                continue;
            }
            let Some(freq) = num(pb, &["freq", "frequency", "hz"], 0.0).filter(|f| *f > 0.0) else {
                problems.push(format!("{label}: missing freq"));
                continue;
            };
            if slot >= q::NUM_BANDS {
                problems.push("only the first 24 bands were loaded".into());
                break;
            }
            let freq = freq.clamp(FREQ_MIN, FREQ_MAX);
            let gain = num(pb, &["gain", "gain_db"], 0.0).unwrap_or(0.0).clamp(-30.0, 30.0);
            let is_cut = kind == q::TYPE_LOW_CUT || kind == q::TYPE_HIGH_CUT;
            let qv = num(pb, &["q"], 1.0).unwrap_or(if is_cut { CUT_Q } else { 1.0 }).clamp(0.025, 40.0);
            let slope_db = num(pb, &["slope", "slope_db"], 24.0).unwrap_or(24.0) as i32;
            let slope = SLOPES_DB.iter().position(|s| *s == slope_db).unwrap_or(3);
            let active = !matches!(pb.get("on"), Some(Value::Bool(false)));
            let place_s = pb.get("channel").or_else(|| pb.get("place")).and_then(Value::as_str).unwrap_or("stereo").to_lowercase();
            let place = PLACE_KEYS.iter().position(|p| *p == place_s).unwrap_or(0);
            let (mut dyn_, mut thr, mut range) = (false, -20.0, 0.0);
            match pb.get("dynamic") {
                Some(d @ Value::Object(_)) => {
                    dyn_ = !matches!(d.get("on"), Some(Value::Bool(false)));
                    thr = num(d, &["threshold"], -20.0).unwrap_or(-20.0).clamp(-60.0, 0.0);
                    range = num(d, &["range"], 0.0).unwrap_or(0.0).clamp(-30.0, 30.0);
                    if dyn_ && !timing_done {
                        timing_done = true;
                        let a = num(d, &["attack_ms", "attack"], 10.0).unwrap_or(10.0).clamp(0.1, 500.0);
                        let r = num(d, &["release_ms", "release"], 100.0).unwrap_or(100.0).clamp(5.0, 5000.0);
                        list.push(("dynAttack", q::attack_to_knob(a).clamp(0.0, 1.0)));
                        list.push(("dynRelease", q::release_to_knob(r).clamp(0.0, 1.0)));
                    }
                }
                Some(Value::Bool(b)) => dyn_ = *b,
                _ => {}
            }
            if kind > q::TYPE_HIGH_SHELF {
                dyn_ = false;
            }
            list.extend([
                (bp(slot, cq::F_FREQ), q::freq_to_knob(freq).clamp(0.0, 1.0)),
                (bp(slot, cq::F_GAIN), q::gain_to_knob(gain).clamp(0.0, 1.0)),
                (bp(slot, cq::F_Q), q::q_to_knob(qv).clamp(0.0, 1.0)),
                (bp(slot, cq::F_TYPE), kind as f32),
                (bp(slot, cq::F_PLACE), place as f32),
                (bp(slot, cq::F_SLOPE), slope as f32),
                (bp(slot, cq::F_DYN), if dyn_ { 1.0 } else { 0.0 }),
                (bp(slot, cq::F_THRESHOLD), thr as f32),
                (bp(slot, cq::F_RANGE), range as f32),
                (bp(slot, cq::F_ON), if active { q::ON_ACTIVE as f32 } else { q::ON_BYPASSED as f32 }),
            ]);
            slot += 1;
        }
        n_bands = slot;
        for s in slot..q::NUM_BANDS {
            list.push((bp(s, cq::F_ON), q::ON_FREE as f32));
        }
    }
    if let Some(v) = num(&json, &["output_gain_db"], 0.0) {
        list.push(("outGain", q::out_db_to_knob(v.clamp(-24.0, 24.0)).clamp(0.0, 1.0)));
    }
    if let Some(Value::Bool(b)) = json.get("phase_invert") {
        list.push(("invert", if *b { 1.0 } else { 0.0 }));
    }
    let mut n_guides = 0;
    let mut guides = Guides::default();
    if let Some(arr) = markers {
        for m in arr {
            let Some(f) = num(m, &["freq", "frequency", "hz"], 0.0).filter(|f| *f > 0.0) else { continue };
            let (mut lo, mut hi) = (num(m, &["lo", "low"], 0.0).unwrap_or(0.0), num(m, &["hi", "high"], 0.0).unwrap_or(0.0));
            if lo <= 0.0 || hi <= 0.0 {
                let bw = num(m, &["bandwidth_oct", "width_oct"], 0.25).unwrap_or(0.25);
                let half = if bw > 0.0 { bw } else { 0.25 } * 0.5;
                lo = f / 2f64.powf(half);
                hi = f * 2f64.powf(half);
            }
            let note = m.get("label").or_else(|| m.get("reason")).or_else(|| m.get("note")).and_then(Value::as_str).unwrap_or("").to_string();
            let g = Guide {
                freq_hz: f,
                lo_hz: lo,
                hi_hz: hi,
                gain_db: num(m, &["gain", "gain_db"], -3.0).unwrap_or(-3.0),
                dynamic: m.get("dynamic").and_then(Value::as_bool).unwrap_or(false),
                note,
                ..Guide::default()
            };
            let _ = guides.add(g);
        }
        n_guides = guides.list.len();
        if n_guides > 0 {
            st.guides_on = true;
        }
    }
    // The bands and the guides as one undo step.
    let with_guides = markers.is_some();
    crate::editor_kit::one_step(app, |app| {
        set_params_merged(app, cx.host, &list, "paste");
        if with_guides {
            write_guides_merged(app, cx.host, &guides, "paste");
        }
    });
    select(st, None);
    st.guide_edit = None;
    let mut msg = "Loaded ".to_string();
    if bands.is_some() {
        msg += &format!("{n_bands} {}", if n_bands == 1 { "band" } else { "bands" });
    }
    if bands.is_some() && markers.is_some() {
        msg += ", ";
    }
    if markers.is_some() {
        msg += &format!("{n_guides} {}", if n_guides == 1 { "guide" } else { "guides" });
    }
    if !problems.is_empty() {
        msg += &format!(" ({})", problems.join("; "));
    }
    Ok(msg)
}

// ============================================================================ file drop
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn file_drop(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, ed: Rect) {
    let (hovering, dropped, pointer) = ui.input(|i| {
        (
            i.raw.hovered_files.iter().any(|f| f.path.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))),
            i.raw.dropped_files.clone(),
            i.pointer.hover_pos(),
        )
    });
    let over = pointer.is_none_or(|p| ed.contains(p));
    st.file_hover = hovering && over;
    if !over {
        return;
    }
    for f in dropped {
        let path = f.path().to_path_buf();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            paste_text(app, cx, st, &text);
            if let Some((m, _)) = st.message.as_mut() {
                *m = m.replace("Pasted", "Dropped");
            }
        }
        return;
    }
}

/// No file system on the web: nothing to drop.
#[cfg(target_arch = "wasm32")]
pub(super) fn file_drop(_app: &mut SoundApp, _ui: &mut Ui, _cx: &Cx, st: &mut Editor, _ed: Rect) {
    st.file_hover = false;
}
