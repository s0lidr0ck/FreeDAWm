//! `tables_match_source`: re-parses the C++ table headers (with its own small parser, not the
//! converter's) and checks, for every array, its element count and an FNV-1a checksum of its
//! values (each parsed as its C++ element type), plus every int constant and every `StageModel`
//! aggregate (its scalars, and that each table pointer names the right array). Every array in the
//! headers must have a Rust counterpart here, so a table the converter missed fails too.
//!
//! Needs the FireStation tree (`FIRESTATION_ROOT`, default `C:\projects\A18\FireStation`);
//! without it the test prints a skip, as the fidelity test does (`FIDELITY_STRICT=1` fails instead).

use super::vint_british_tables as bt;
use super::vint_gain_table as gt;
use super::vint_neve_curves as nc;
use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn header(name: &str) -> Option<String> {
    let root = std::env::var_os("FIRESTATION_ROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\projects\A18\FireStation"));
    let path = root.join("Engine 11 - Vint").join("Source").join(name);
    match std::fs::read_to_string(&path) {
        Ok(s) => Some(s),
        Err(e) if std::env::var("FIDELITY_STRICT").as_deref() == Ok("1") => panic!("{}: {e} (FIDELITY_STRICT=1)", path.display()),
        Err(e) => {
            eprintln!("SKIPPED: {}: {e} (no FireStation tree; set FIDELITY_STRICT=1 to fail instead)", path.display());
            None
        }
    }
}

fn tokens(src: &str) -> Vec<String> {
    let c: Vec<char> = src.chars().collect();
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = at(i);
        if ch == '/' && at(i + 1) == '/' {
            while i < c.len() && at(i) != '\n' {
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == '*' {
            i += 2;
            while i < c.len() && !(at(i) == '*' && at(i + 1) == '/') {
                i += 1;
            }
            i += 2;
        } else if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            let s = i;
            while at(i).is_ascii_alphanumeric()
                || at(i) == '_'
                || at(i) == '.'
                || (matches!(at(i), '-' | '+') && matches!(at(i - 1), 'e' | 'E') && at(s).is_ascii_digit())
            {
                i += 1;
            }
            out.push(c[s..i].iter().collect());
        } else {
            out.push(ch.to_string());
            i += 1;
        }
    }
    out
}

/// A C++ number's value as its element type: `float` rounds to f32 first.
fn value(text: &str, float: bool) -> f64 {
    if let Some(h) = text.strip_prefix("0x") {
        return u64::from_str_radix(h.trim_end_matches(['u', 'l', 'U', 'L']), 16).unwrap() as f64;
    }
    let t = text.trim_end_matches(['f', 'F', 'l', 'L', 'u', 'U']);
    if float { f64::from(t.parse::<f32>().unwrap()) } else { t.parse::<f64>().unwrap() }
}

fn fnv(vals: impl IntoIterator<Item = f64>) -> (usize, u64) {
    let mut n = 0;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in vals {
        n += 1;
        for b in v.to_bits().to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    (n, h)
}

#[derive(Default)]
struct Parsed {
    ints: BTreeMap<String, usize>,
    /// name -> (count, checksum)
    arrays: BTreeMap<String, (usize, u64)>,
    /// name -> (scalars in order, identifiers in order)
    aggregates: BTreeMap<String, (Vec<f64>, Vec<String>)>,
}

fn parse(src: &str) -> Parsed {
    let t = tokens(src);
    let mut p = Parsed::default();
    let mut i = 0;
    while i + 3 < t.len() {
        let qual = t[i] == "const" || t[i] == "constexpr";
        if !qual {
            i += 1;
            continue;
        }
        let (ty, name) = (t[i + 1].as_str(), t[i + 2].clone());
        if ty == "int" && t[i + 3] == "=" {
            // int A = 1, B = 2;
            let mut j = i + 2;
            loop {
                p.ints.insert(t[j].clone(), t[j + 2].parse().unwrap());
                if t[j + 3] == "," {
                    j += 4;
                } else {
                    break;
                }
            }
            i = j + 4;
        } else if (ty == "float" || ty == "double") && t[i + 3] == "[" {
            let mut j = i + 3;
            while t[j] != "{" {
                j += 1;
            }
            let mut depth = 0;
            let mut vals = Vec::new();
            loop {
                let tok = &t[j];
                if tok == "{" {
                    depth += 1;
                } else if tok == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else if tok == "-" {
                    j += 1;
                    vals.push(-value(&t[j], ty == "float"));
                } else if tok != "," {
                    vals.push(value(tok, ty == "float"));
                }
                j += 1;
            }
            p.arrays.insert(name, fnv(vals));
            i = j + 1;
        } else if ty == "StageModel" && t[i + 3] == "=" {
            let mut j = i + 4;
            let mut depth = 0;
            let (mut scalars, mut idents) = (Vec::new(), Vec::new());
            loop {
                let tok = &t[j];
                if tok == "{" {
                    depth += 1;
                } else if tok == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else if tok == "-" {
                    j += 1;
                    scalars.push(-value(&t[j], false));
                } else if tok.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
                    scalars.push(value(tok, false));
                } else if tok != "," {
                    idents.push(tok.clone());
                }
                j += 1;
            }
            p.aggregates.insert(name, (scalars, idents));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    p
}

fn check_arrays(header_name: &str, p: &Parsed, rust: &[(&str, Vec<f64>)]) {
    let names: Vec<&str> = rust.iter().map(|r| r.0).collect();
    let cpp: Vec<&str> = p.arrays.keys().map(String::as_str).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(cpp, sorted, "{header_name}: the arrays in the header and in Rust");
    for (name, vals) in rust {
        let got = fnv(vals.iter().copied());
        assert_eq!(p.arrays[*name], got, "{header_name}: {name} (count, checksum)");
    }
}

fn flat1<const N: usize>(a: &[f32; N]) -> Vec<f64> {
    a.iter().map(|&v| f64::from(v)).collect()
}

#[test]
fn tables_match_source() {
    let Some(gain) = header("VintGainTable.h") else { return };
    let p = parse(&gain);
    assert_eq!(p.ints["kStages"], gt::K_STAGES);
    assert_eq!(p.ints["kPoints"], gt::K_POINTS);
    check_arrays("VintGainTable.h", &p, &[("ironTrim", gt::IRON_TRIM.to_vec()), ("table", gt::TABLE.iter().flat_map(flat1).collect())]);
    assert!(gain.contains("kCalibrated = true") && gt::K_CALIBRATED);

    let Some(bri) = header("VintBritishTables.h") else { return };
    let p = parse(&bri);
    assert_eq!(p.ints["kTablePoints"], bt::K_TABLE_POINTS);
    check_arrays(
        "VintBritishTables.h",
        &p,
        &[
            ("kQ", bt::K_Q.iter().flatten().flatten().copied().collect()),
            ("kGainDb", bt::K_GAIN_DB.iter().flatten().flatten().flatten().copied().collect()),
        ],
    );

    let Some(neve) = header("VintNeveCurves.h") else { return };
    let p = parse(&neve);
    assert_eq!(p.ints["kCurvePoints"], nc::K_CURVE_POINTS);
    assert_eq!(p.ints["kMaxSlow"], nc::K_MAX_SLOW);
    assert_eq!(p.ints["kTwoStride"], nc::K_TWO_STRIDE);
    assert_eq!(p.ints["kTwoPoints"], nc::K_TWO_POINTS);
    assert_eq!(p.ints["kAxisPoints"], nc::K_AXIS_POINTS);
    macro_rules! stage_arrays {
        ($($cpp:literal => $rs:ident),* $(,)?) => { vec![$(($cpp, nc::$rs.to_vec())),*] };
    }
    let arrays: Vec<(&str, Vec<f64>)> = stage_arrays![
        "stageOpenOut" => STAGE_OPEN_OUT, "stageOpenF0" => STAGE_OPEN_F0, "stageOpenF1" => STAGE_OPEN_F1,
        "stageOpenF2" => STAGE_OPEN_F2, "stageOpenF3" => STAGE_OPEN_F3, "stageOpenROut" => STAGE_OPEN_ROUT,
        "stageOpenRFlow" => STAGE_OPEN_RFLOW, "stageOpenOut2" => STAGE_OPEN_OUT2, "stageOpenFlow2" => STAGE_OPEN_FLOW2,
        "stage330Out" => STAGE330_OUT, "stage330F0" => STAGE330_F0, "stage330F1" => STAGE330_F1,
        "stage330F2" => STAGE330_F2, "stage330F3" => STAGE330_F3, "stage330ROut" => STAGE330_ROUT,
        "stage330RFlow" => STAGE330_RFLOW, "stage330Out2" => STAGE330_OUT2, "stage330Flow2" => STAGE330_FLOW2,
        "stage120Out" => STAGE120_OUT, "stage120F0" => STAGE120_F0, "stage120F1" => STAGE120_F1,
        "stage120F2" => STAGE120_F2, "stage120F3" => STAGE120_F3, "stage120ROut" => STAGE120_ROUT,
        "stage120RFlow" => STAGE120_RFLOW, "stage120Out2" => STAGE120_OUT2, "stage120Flow2" => STAGE120_FLOW2,
        "stage91Out" => STAGE91_OUT, "stage91F0" => STAGE91_F0, "stage91F1" => STAGE91_F1,
        "stage91F2" => STAGE91_F2, "stage91F3" => STAGE91_F3, "stage91ROut" => STAGE91_ROUT,
        "stage91RFlow" => STAGE91_RFLOW, "stage91Out2" => STAGE91_OUT2, "stage91Flow2" => STAGE91_FLOW2,
        "stageOutputOut" => STAGE_OUTPUT_OUT, "stageOutputF0" => STAGE_OUTPUT_F0, "stageOutputF1" => STAGE_OUTPUT_F1,
        "stageOutputF2" => STAGE_OUTPUT_F2, "stageOutputF3" => STAGE_OUTPUT_F3, "stageOutputF4" => STAGE_OUTPUT_F4,
        "stageOutputF5" => STAGE_OUTPUT_F5, "stageOutputROut" => STAGE_OUTPUT_ROUT, "stageOutputRFlow" => STAGE_OUTPUT_RFLOW,
        "stageOutputOut2" => STAGE_OUTPUT_OUT2, "stageOutputFlow2" => STAGE_OUTPUT_FLOW2,
    ];
    check_arrays("VintNeveCurves.h", &p, &arrays);

    // The aggregates: scalars in C++ order, and each table pointer at the array it names.
    let ptr_of = |name: &str| arrays.iter().find(|a| a.0 == name).map(|a| a.1.len());
    let models: [(&str, &StageModel); 5] = [
        ("stageOpen", &nc::STAGE_OPEN),
        ("stage330", &nc::STAGE330),
        ("stage120", &nc::STAGE120),
        ("stage91", &nc::STAGE91),
        ("stageOutput", &nc::STAGE_OUTPUT),
    ];
    assert_eq!(p.aggregates.len(), models.len());
    for (name, m) in models {
        let (scalars, idents) = &p.aggregates[name];
        let mut rs = vec![m.n as f64, m.xm, m.x0, m.inv_a];
        rs.extend(m.inertia);
        rs.extend(m.in_ref);
        rs.extend([m.varies as f64, f64::from(m.two), m.two_lo, m.two_step, f64::from(m.two_n)]);
        rs.extend([f64::from(m.three), m.three_lo, m.three_step, f64::from(m.three_n), m.three_c, m.three_w]);
        assert_eq!(scalars, &rs, "{name}: scalars");
        let mut ptrs: Vec<Option<&[f64]>> = vec![Some(m.out)];
        ptrs.extend(m.flow);
        ptrs.extend([Some(m.r_out), Some(m.r_flow), Some(m.out2), Some(m.flow2)]);
        assert_eq!(idents.len(), ptrs.len(), "{name}: pointers");
        for (id, ptr) in idents.iter().zip(ptrs) {
            match ptr {
                None => assert_eq!(id, "nullptr", "{name}"),
                Some(slice) => {
                    let want = arrays.iter().find(|a| a.0 == id.as_str()).map(|a| a.1.as_slice());
                    assert_eq!(Some(slice), want, "{name}: {id}");
                    assert_eq!(Some(slice.len()), ptr_of(id));
                }
            }
        }
    }
}
