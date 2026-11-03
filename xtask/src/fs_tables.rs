//! `cargo xtask fs-tables <engine>`: converts a FireStation engine's generated C++ table headers to
//! Rust mechanically, so no table is ever retyped.
//!
//! It understands:
//! - integer constants `[static] const|constexpr int NAME = 17;` → `pub const NAME: usize = 17;`;
//! - float/double arrays of any rank, `[static] const|constexpr float|double NAME[dims]... = {...};`,
//!   with numeric, symbolic (an int constant above) or empty (`[]`, counted) dimensions →
//!   `pub static NAME: [[f32; N]; M] = [...];`.
//!
//! Names go camelCase → SCREAMING_SNAKE (`kPoints` → `K_POINTS`, `ironTrim` → `IRON_TRIM`).
//! Number text is copied verbatim minus its `f`/`F`/`L` suffix. `/** */` doc comments before a
//! declaration and a `//` comment after an int constant become `///` docs; `//` comments inside an
//! array body (row labels) are kept beside their rows. Element counts are checked against the dims.

use std::path::Path;

/// Engine → (header under the engine's directory, Rust file under the repo root), from each
/// engine's own registration (`soundcraft_dsp::plugins::fs::Fidelity::tables`).
fn tables() -> Vec<(&'static str, &'static str, &'static str)> {
    soundcraft_dsp::plugins::fs::engines()
        .iter()
        .filter_map(|e| e.fidelity)
        .flat_map(|f| f.tables.iter().map(move |(header, out)| (f.name, *header, *out)))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(String),
    Punct(char),
    Line(String),
    Block(String),
}

fn tokenize(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    while i < chars.len() {
        let c = at(i);
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == '/' {
            let start = i + 2;
            while i < chars.len() && at(i) != '\n' {
                i += 1;
            }
            out.push(Tok::Line(chars[start..i].iter().collect::<String>().trim().to_string()));
        } else if c == '/' && at(i + 1) == '*' {
            let start = i + 2;
            i += 2;
            while i < chars.len() && !(at(i) == '*' && at(i + 1) == '/') {
                i += 1;
            }
            out.push(Tok::Block(chars[start..i.min(chars.len())].iter().collect()));
            i += 2;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while at(i).is_ascii_alphanumeric() || at(i) == '_' {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit() || (c == '.' && at(i + 1).is_ascii_digit()) {
            let start = i;
            while at(i).is_ascii_alphanumeric() || at(i) == '.' || ((at(i) == '-' || at(i) == '+') && matches!(at(i - 1), 'e' | 'E')) {
                i += 1;
            }
            out.push(Tok::Num(chars[start..i].iter().collect()));
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

/// `kPoints` → `K_POINTS`, `ironTrim` → `IRON_TRIM`, `table` → `TABLE`.
pub fn screaming(name: &str) -> String {
    let mut s = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            s.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        s.push(c.to_ascii_uppercase());
    }
    s
}

/// A number literal without its C suffix, checked to parse; an integer gets `.0` (Rust float).
fn number(text: &str, neg: bool) -> Result<String, String> {
    let t = text.trim_end_matches(['f', 'F', 'l', 'L']);
    t.parse::<f64>().map_err(|_| format!("not a number: {text}"))?;
    let t = if t.contains(['.', 'e', 'E']) { t.to_string() } else { format!("{t}.0") };
    Ok(if neg { format!("-{t}") } else { t })
}

/// A number inside an aggregate, as Rust source: its C suffix dropped, its kind kept (`15` stays an
/// integer, `0x30c3ull` a hex integer): the struct's own macro gives each field its type.
fn aggregate_number(text: &str, neg: bool) -> Result<String, String> {
    let t = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        let digits = hex.trim_end_matches(['u', 'U', 'l', 'L']);
        u64::from_str_radix(digits, 16).map_err(|_| format!("not a number: {text}"))?;
        format!("0x{digits}")
    } else {
        let t = text.trim_end_matches(['f', 'F', 'l', 'L', 'u', 'U']);
        t.parse::<f64>().map_err(|_| format!("not a number: {text}"))?;
        t.to_string()
    };
    Ok(if neg { format!("-{t}") } else { t })
}

/// The items of an aggregate initializer `{ ... }` starting at `toks[i]`, as a Rust macro's
/// argument list: braces become `[...]`, names SCREAMING_SNAKE (`nullptr` kept), numbers as
/// `aggregate_number`. Returns the text and the index after the closing `}`.
fn aggregate(toks: &[Tok], i: usize) -> Result<(String, usize), String> {
    if toks.get(i) != Some(&Tok::Punct('{')) {
        return Err("expected `{`".into());
    }
    let mut j = i + 1;
    let mut items = Vec::new();
    loop {
        match toks.get(j) {
            Some(Tok::Punct('}')) => return Ok((items.join(", "), j + 1)),
            Some(Tok::Punct('{')) => {
                let (inner, k) = aggregate(toks, j)?;
                items.push(format!("[{inner}]"));
                j = k;
            }
            Some(Tok::Punct('-')) => match toks.get(j + 1) {
                Some(Tok::Num(t)) => {
                    items.push(aggregate_number(t, true)?);
                    j += 2;
                }
                _ => return Err("`-` without a number".into()),
            },
            Some(Tok::Num(t)) => {
                items.push(aggregate_number(t, false)?);
                j += 1;
            }
            Some(Tok::Ident(n)) => {
                items.push(if n == "nullptr" { n.clone() } else { screaming(n) });
                j += 1;
            }
            Some(Tok::Punct(',')) | Some(Tok::Block(_)) | Some(Tok::Line(_)) => j += 1,
            other => return Err(format!("unexpected {other:?} in an aggregate")),
        }
    }
}

#[derive(Debug)]
enum Node {
    Num(String),
    List { items: Vec<(Node, Option<String>)>, open: Option<String> },
}

/// Parses `{ ... }` starting at `toks[i]` (which must be `{`); returns the node and the index after `}`.
fn parse_list(toks: &[Tok], mut i: usize) -> Result<(Node, usize), String> {
    if toks.get(i) != Some(&Tok::Punct('{')) {
        return Err("expected `{`".into());
    }
    i += 1;
    let mut open = None;
    while let Some(Tok::Line(c)) = toks.get(i) {
        open = Some(c.clone());
        i += 1;
    }
    let mut items: Vec<(Node, Option<String>)> = Vec::new();
    loop {
        match toks.get(i) {
            Some(Tok::Punct('}')) => return Ok((Node::List { items, open }, i + 1)),
            Some(Tok::Punct('{')) => {
                let (n, j) = parse_list(toks, i)?;
                items.push((n, None));
                i = j;
            }
            Some(Tok::Punct('-')) => match toks.get(i + 1) {
                Some(Tok::Num(t)) => {
                    items.push((Node::Num(number(t, true)?), None));
                    i += 2;
                }
                _ => return Err("`-` without a number".into()),
            },
            Some(Tok::Num(t)) => {
                items.push((Node::Num(number(t, false)?), None));
                i += 1;
            }
            Some(Tok::Punct(',')) | Some(Tok::Block(_)) => i += 1,
            Some(Tok::Line(c)) => {
                // A row label: belongs to the item just closed.
                if let Some(last) = items.last_mut() {
                    last.1 = Some(c.clone());
                }
                i += 1;
            }
            other => return Err(format!("unexpected {other:?} in an array body")),
        }
    }
}

fn shape(n: &Node, depth: usize, dims: &mut Vec<usize>) -> Result<(), String> {
    if let Node::List { items, .. } = n {
        match dims.get(depth) {
            Some(&d) if d != items.len() => return Err(format!("dimension {depth}: {} items, expected {d}", items.len())),
            None => dims.push(items.len()),
            _ => {}
        }
        for (child, _) in items {
            shape(child, depth + 1, dims)?;
        }
    }
    Ok(())
}

fn emit(n: &Node, indent: usize, out: &mut String) {
    match n {
        Node::Num(t) => out.push_str(t),
        Node::List { items, open } => {
            if items.iter().all(|(c, _)| matches!(c, Node::Num(_))) {
                let nums: Vec<&str> = items.iter().map(|(c, _)| if let Node::Num(t) = c { t.as_str() } else { "" }).collect();
                out.push('[');
                out.push_str(&nums.join(", "));
                out.push(']');
                return;
            }
            out.push('[');
            if let Some(c) = open {
                out.push_str(&format!("   // {c}"));
            }
            out.push('\n');
            let pad = "    ".repeat(indent + 1);
            for (child, label) in items {
                out.push_str(&pad);
                emit(child, indent + 1, out);
                out.push(',');
                if let Some(c) = label {
                    out.push_str(&format!("   // {c}"));
                }
                out.push('\n');
            }
            out.push_str(&"    ".repeat(indent));
            out.push(']');
        }
    }
}

fn doc(text: &str, out: &mut String) {
    for line in text.lines() {
        let l = line.trim().trim_start_matches('*').trim();
        if !l.is_empty() {
            out.push_str(&format!("/// {l}\n"));
        }
    }
}

/// Converts a C++ table header to Rust source (the body after the file's header comment).
pub fn convert(src: &str) -> Result<String, String> {
    let toks = tokenize(src);
    let mut out = String::new();
    let mut consts: Vec<(String, usize)> = Vec::new();
    // Struct types of aggregates: declared in the generated file's parent module.
    let mut struct_types: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        // A declaration: const|constexpr TYPE NAME ...
        let is_qual = matches!(toks.get(i), Some(Tok::Ident(q)) if q == "const" || q == "constexpr");
        // A struct aggregate: const|constexpr TYPE NAME = { ... };
        if let (true, Some(Tok::Ident(t)), Some(Tok::Ident(name)), Some(Tok::Punct('=')), Some(Tok::Punct('{'))) =
            (is_qual, toks.get(i + 1), toks.get(i + 2), toks.get(i + 3), toks.get(i + 4))
            && !matches!(t.as_str(), "int" | "float" | "double" | "bool")
        {
            let (items, after) = aggregate(&toks, i + 4).map_err(|e| format!("{name}: {e}"))?;
            if toks.get(after) != Some(&Tok::Punct(';')) {
                return Err(format!("{name}: expected `;` after the aggregate"));
            }
            out.push_str(&format!("\npub static {}: {t} = {}!({items});\n", screaming(name), screaming(t).to_lowercase()));
            if !struct_types.contains(t) {
                struct_types.push(t.clone());
            }
            i = after + 1;
            continue;
        }
        let ty = match (is_qual, toks.get(i + 1)) {
            (true, Some(Tok::Ident(t))) if t == "int" || t == "float" || t == "double" || t == "bool" => t.clone(),
            _ => {
                i += 1;
                continue;
            }
        };
        let Some(Tok::Ident(name)) = toks.get(i + 2) else {
            i += 1;
            continue;
        };
        // A `/** */` just before the declaration (skipping `static` / `inline`) is its doc.
        let mut k = i;
        while k > 0 && matches!(toks.get(k - 1), Some(Tok::Ident(q)) if q == "static" || q == "inline") {
            k -= 1;
        }
        let pre_doc = match k.checked_sub(1).and_then(|k| toks.get(k)) {
            Some(Tok::Block(b)) => Some(b.clone()),
            _ => None,
        };
        let mut j = i + 3;
        if ty == "bool" {
            let v = match (toks.get(j), toks.get(j + 1), toks.get(j + 2)) {
                (Some(Tok::Punct('=')), Some(Tok::Ident(v)), Some(Tok::Punct(';'))) if v == "true" || v == "false" => v.clone(),
                _ => return Err(format!("{name}: expected `= true|false;`")),
            };
            if let Some(b) = &pre_doc {
                doc(b, &mut out);
            }
            out.push_str(&format!("pub const {}: bool = {v};\n", screaming(name)));
            i = j + 3;
            continue;
        }
        if ty == "int" && toks.get(j) == Some(&Tok::Punct('=')) {
            // `int A = 1, B = 2;`: one or more constants in one declaration.
            let mut list = vec![(name.clone(), j)];
            loop {
                let (nm, at) = list[list.len() - 1].clone();
                let Some(Tok::Num(v)) = toks.get(at + 1) else { return Err(format!("{nm}: expected an integer")) };
                let v: usize = v.parse().map_err(|_| format!("{nm}: {v} is not an integer"))?;
                consts.push((nm, v));
                j = at + 2;
                match (toks.get(j), toks.get(j + 1), toks.get(j + 2)) {
                    (Some(Tok::Punct(',')), Some(Tok::Ident(n2)), Some(Tok::Punct('='))) => list.push((n2.clone(), j + 2)),
                    (Some(Tok::Punct(';')), _, _) => break,
                    _ => return Err(format!("{}: expected `;`", list[list.len() - 1].0)),
                }
            }
            j += 1;
            if let Some(b) = &pre_doc {
                doc(b, &mut out);
            }
            if let Some(Tok::Line(c)) = toks.get(j) {
                doc(c, &mut out);
                j += 1;
            }
            for (nm, _) in &list {
                let v = consts.iter().rev().find(|(n, _)| n == nm).map(|(_, v)| *v).unwrap_or(0);
                out.push_str(&format!("pub const {}: usize = {v};\n", screaming(nm)));
            }
            i = j;
            continue;
        }
        if ty == "int" || toks.get(j) != Some(&Tok::Punct('[')) {
            i += 1;
            continue;
        }
        // Dimensions.
        let mut dims: Vec<Option<(String, usize)>> = Vec::new();
        while toks.get(j) == Some(&Tok::Punct('[')) {
            if toks.get(j + 1) == Some(&Tok::Punct(']')) {
                dims.push(None);
                j += 2;
                continue;
            }
            // A product of numbers and int constants: `17`, `kN`, `4 * kTwoPoints`.
            j += 1;
            let (mut text, mut value) = (Vec::new(), 1usize);
            loop {
                let (t, v) = match toks.get(j) {
                    Some(Tok::Num(n)) => (n.clone(), n.parse::<usize>().map_err(|_| format!("{name}: bad dimension {n}"))?),
                    Some(Tok::Ident(c)) => {
                        let v = consts.iter().find(|(n, _)| n == c).map(|(_, v)| *v).ok_or_else(|| format!("{name}: unknown dimension {c}"))?;
                        (screaming(c), v)
                    }
                    _ => return Err(format!("{name}: unsupported dimension")),
                };
                text.push(t);
                value = value.checked_mul(v).ok_or_else(|| format!("{name}: dimension overflows"))?;
                match toks.get(j + 1) {
                    Some(Tok::Punct('*')) => j += 2,
                    Some(Tok::Punct(']')) => {
                        j += 2;
                        break;
                    }
                    _ => return Err(format!("{name}: unsupported dimension")),
                }
            }
            dims.push(Some((text.join(" * "), value)));
        }
        if toks.get(j) != Some(&Tok::Punct('=')) {
            return Err(format!("{name}: expected `=`"));
        }
        j += 1;
        while matches!(toks.get(j), Some(Tok::Line(_)) | Some(Tok::Block(_))) {
            j += 1;
        }
        let (node, after) = parse_list(&toks, j)?;
        if toks.get(after) != Some(&Tok::Punct(';')) {
            return Err(format!("{name}: expected `;` after the array"));
        }
        let mut counted: Vec<usize> = dims.iter().map(|d| d.as_ref().map(|(_, v)| *v).unwrap_or(0)).collect();
        // Unknown (`[]`) dims are taken from the data; known ones are checked against it.
        let mut measured = Vec::new();
        shape(&node, 0, &mut measured)?;
        if measured.len() != dims.len() {
            return Err(format!("{name}: {} dimensions declared, {} in the data", dims.len(), measured.len()));
        }
        for (d, (c, m)) in dims.iter().zip(counted.iter_mut().zip(&measured)) {
            if d.is_none() {
                *c = *m;
            } else if c != m {
                return Err(format!("{name}: declared {c}, data has {m}"));
            }
        }
        let elem = if ty == "double" { "f64" } else { "f32" };
        let mut rust_ty = elem.to_string();
        for (d, c) in dims.iter().zip(&counted).rev() {
            let n = d.as_ref().map(|(s, _)| s.clone()).unwrap_or_else(|| c.to_string());
            rust_ty = format!("[{rust_ty}; {n}]");
        }
        out.push('\n');
        if let Some(b) = &pre_doc {
            doc(b, &mut out);
        }
        if dims.len() > 1 {
            out.push_str("#[rustfmt::skip]\n");
        }
        out.push_str(&format!("pub static {}: {rust_ty} = ", screaming(name)));
        emit(&node, 0, &mut out);
        out.push_str(";\n");
        i = after + 1;
    }
    if out.is_empty() {
        return Err("no tables found".into());
    }
    if !struct_types.is_empty() {
        out = format!("use super::{{{}}};\n\n{out}", struct_types.join(", "));
    }
    Ok(out)
}

pub fn run(repo: &Path, args: &[String]) -> Result<(), String> {
    let tables = tables();
    let engines: Vec<&str> = tables.iter().map(|t| t.0).collect();
    let engine = args.first().ok_or_else(|| format!("usage: cargo xtask fs-tables <{}>", engines.join("|")))?;
    let mut any = false;
    for (_, header, out) in tables.iter().filter(|t| t.0 == engine) {
        let dir = crate::fidelity::engines()
            .into_iter()
            .find(|e| e.0 == engine)
            .map(|e| e.1)
            .ok_or_else(|| format!("{engine}: not in the engine table"))?;
        let src_path = crate::fidelity::firestation_root().join(dir).join(header);
        let src = std::fs::read_to_string(&src_path).map_err(|e| format!("{}: {e}", src_path.display()))?;
        let body = convert(&src).map_err(|e| format!("{}: {e}", src_path.display()))?;
        let text = format!("//! GENERATED by `cargo xtask fs-tables {engine}` from `{dir}/{header}`. Do not edit by hand.\n\n{body}");
        let out_path = repo.join(out);
        std::fs::write(&out_path, text).map_err(|e| format!("{}: {e}", out_path.display()))?;
        println!("{} <- {}", out_path.display(), src_path.display());
        any = true;
    }
    if any { Ok(()) } else { Err(format!("no tables for {engine}; one of {}", engines.join(", "))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_constants_and_arrays() {
        let src = r#"
            static constexpr int kN = 3;   // three of them
            /** Gains. */
            static const float gains[] = { 1.0f, -0.5f, 2.5e-3f };
            static constexpr double grid[2][kN] = {
                { 1.0, 2.0, 3.0 },   // first
                { -4, 5, 6.5 },      // second
            };
        "#;
        let rs = convert(src).unwrap();
        assert!(rs.contains("/// three of them\npub const K_N: usize = 3;"), "{rs}");
        assert!(rs.contains("/// Gains.\npub static GAINS: [f32; 3] = [1.0, -0.5, 2.5e-3];"), "{rs}");
        assert!(rs.contains("pub static GRID: [[f64; K_N]; 2] = [\n    [1.0, 2.0, 3.0],   // first\n    [-4.0, 5.0, 6.5],   // second\n];"), "{rs}");
    }

    #[test]
    fn converts_int_lists_and_bools() {
        let src = r#"
            static constexpr int kA = 4, kB = 3;
            static constexpr bool kOn = true;
            static constexpr double t[kA][kB] = { {1,2,3}, {4,5,6}, {7,8,9}, {0,0,0} };
        "#;
        let rs = convert(src).unwrap();
        assert!(rs.contains("pub const K_A: usize = 4;\npub const K_B: usize = 3;\n"), "{rs}");
        assert!(rs.contains("pub const K_ON: bool = true;\n"), "{rs}");
        assert!(rs.contains("pub static T: [[f64; K_B]; K_A] = "), "{rs}");
    }

    #[test]
    fn converts_product_dimensions() {
        let src = r#"
            constexpr int kTwo = 3;
            constexpr double rows[2 * kTwo] = { 1, 2, 3, 4, 5, 6 };
        "#;
        let rs = convert(src).unwrap();
        assert!(rs.contains("pub static ROWS: [f64; 2 * K_TWO] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];"), "{rs}");
        assert!(convert("constexpr int kTwo = 3; constexpr double r[2 * kTwo] = { 1, 2 };").is_err());
        assert!(convert("constexpr double r[2 * kNope] = { 1, 2 };").is_err());
    }

    #[test]
    fn converts_struct_aggregates_to_a_macro_call() {
        let src = r#"
            constexpr double curveA[2] = { 1.5, -2 };
            constexpr Model modelA = { 4, 15, 0.25, curveA, { curveA, nullptr }, { 1e-05, 0 },
                0x30c3ull, -2, -0.5f };
        "#;
        let rs = convert(src).unwrap();
        assert!(rs.contains("pub static MODEL_A: Model = model!(4, 15, 0.25, CURVE_A, [CURVE_A, nullptr], [1e-05, 0], 0x30c3, -2, -0.5);"), "{rs}");
        assert!(rs.starts_with("use super::{Model};\n"), "the struct is the parent module's: {rs}");
        assert!(convert("constexpr Model m = { 1, oops( };").is_err());
    }

    #[test]
    fn rejects_counts_that_do_not_match() {
        assert!(convert("static const float a[2] = { 1.0f };").is_err());
        assert!(convert("static const float a[2][2] = { {1,2}, {3} };").is_err());
        assert!(convert("static const float a[] = { 1.0f, oops };").is_err());
        assert!(convert("nothing here").is_err());
    }

    #[test]
    fn screaming_names() {
        assert_eq!(screaming("kPoints"), "K_POINTS");
        assert_eq!(screaming("ironTrim"), "IRON_TRIM");
        assert_eq!(screaming("table"), "TABLE");
    }
}

#[cfg(test)]
mod engine_table_tests {
    /// The engine tables as they stood before they were read from the engines' own registrations
    /// (`soundcraft_dsp::plugins::fs`), pinned: (engine, FireStation folder, render tool), sorted.
    const ENGINES_PINNED: &[(&str, &str, &str)] = &[
        ("amp", "Engine 12 - Amp", "AmpEngine"),
        ("chamber", "ChamberMade", "ChamberMade"),
        ("cleanq", "CleanQ", "CleanQ"),
        ("comp", "Engine 1 - Comp", "CompEngine"),
        ("delay", "Engine 6 - Delay", "DelayEngine"),
        ("dist", "Engine 2 - Dist", "DistEngine"),
        ("filter", "Engine 5 - Filter", "FilterEngine"),
        ("levelup", "LevelUp", "LevelUp"),
        ("mod", "Engine 4 - Mod", "ModEngine"),
        ("pitch", "Engine 3 - Pitch", "PitchEngine"),
        ("reverb", "Engine 7 - Reverb", "ReverbEngine"),
        ("sat", "Engine 8 - Sat", "SatEngine"),
        ("shale", "Shale", "Shale"),
        ("swale", "Swale", "Swale"),
        ("tape", "Engine 9 - Tape", "TapeEngine"),
        ("transient", "Engine 10 - Transient", "TransientEngine"),
        ("twine", "Twine", "Twine"),
        ("vint", "Engine 11 - Vint", "VintEngine"),
    ];

    /// (engine, C++ header, Rust file), sorted.
    const TABLES_PINNED: &[(&str, &str, &str)] = &[
        ("sat", "Source/SatGainTable.h", "crates/dsp/src/plugins/fs/sat/sat_gain_table.rs"),
        ("tape", "Source/TapeGainTable.h", "crates/dsp/src/plugins/fs/tape/tape_gain_table.rs"),
        ("vint", "Source/VintBritishTables.h", "crates/dsp/src/plugins/fs/vint/tables/vint_british_tables.rs"),
        ("vint", "Source/VintGainTable.h", "crates/dsp/src/plugins/fs/vint/tables/vint_gain_table.rs"),
        ("vint", "Source/VintNeveCurves.h", "crates/dsp/src/plugins/fs/vint/tables/vint_neve_curves.rs"),
    ];

    #[test]
    fn engine_tables_are_unchanged() {
        let mut engines: Vec<(&str, &str, &str)> = crate::fidelity::engines();
        engines.sort();
        assert_eq!(engines, ENGINES_PINNED);
        let mut tables = super::tables();
        tables.sort();
        assert_eq!(tables, TABLES_PINNED);
    }
}
