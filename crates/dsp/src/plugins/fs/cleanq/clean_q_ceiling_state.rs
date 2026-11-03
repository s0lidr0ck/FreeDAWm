//! CleanQCeilingState -- what the Ceiling panel remembers between sessions.
//! Translation of `CleanQ/Source/CleanQCeilingState.h`.
//!
//! None of this is a parameter: a ceiling is a drawing, not something a host should automate. The
//! C++ keeps it in the plugin state as the `ceiling` child of `uiState` (a JUCE `ValueTree`, saved
//! as XML). Here it is the module's state (`Insert::state`): the same `<ceiling …>` element with
//! the same properties, in the same order, and one `<point hz db/>` child per node.
//!
//! The capture itself is NOT saved: it is a measurement of whatever was playing.
//!
//! Translation notes (JUCE semantics reproduced deliberately):
//! - `writeTo` stores each property as the `var` would print it: bools as `1`/`0`, ints in decimal,
//!   doubles round-trippable (JUCE's `serialiseDouble` keeps 15 significant digits; Rust's shortest
//!   round-trip form is used, with a `.0` on whole numbers as JUCE writes them), and the noise
//!   levels with two decimals (`String (level, 2)`).
//! - `readFrom` starts from the defaults, finds the `ceiling` element (`getChildWithName`; here the
//!   first `<ceiling` element anywhere in the text, so a whole C++ `uiState` works too), and reads
//!   each property the way the `var` converts a string: `getDoubleValue` (`readDoubleValue`) reads
//!   a leading number, and "inf" / "-inf" as ±infinity, which `jlimit` then holds at the bound
//!   (`maxDb="inf"` is 24, `db="-inf"` is -120); `getIntValue` reads a leading `-` and digits (no
//!   `+`: "+5" is 0); anything else is 0. A bool is a non-zero integer, "true" or "yes"
//!   (`stringToBool`). Then `jlimit` to the same ranges, sort, and a shape that is not exactly 16
//!   nodes is dropped.
//! - Never-crash guard, not in the C++: "nan" reads as 0, not NaN (JUCE would return NaN, which
//!   `jlimit` passes through into the shape and the audio).
//! - Hostile input (never-crash, not in the C++): text over [`MAX_STATE_BYTES`] or that is not UTF-8
//!   is refused (`None`), the point scan stops once it has seen more than 16 (the shape is dropped
//!   either way), and `generated` keeps at most [`MAX_GENERATED`] entries.

use super::clean_q_ceiling::{Ceiling, GRID_HI, GRID_LO, NUM_NODES, NUM_NOISES, Point};
use super::clean_q_guides::Guides;

/// The largest state text read (a 16-node ceiling with every property is about 1 KB).
pub const MAX_STATE_BYTES: usize = 1 << 20;
/// Most `generated` slots kept (there are 24 bands).
pub const MAX_GENERATED: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct CeilingState {
    pub shape: Ceiling,

    pub noise_on: [bool; NUM_NOISES],
    /// Dragged: stop auto-levelling.
    pub noise_manual: [bool; NUM_NOISES],
    pub noise_level: [f64; NUM_NOISES],

    /// 0: everything played, averaged.
    pub avg_seconds: f64,
    /// 1/N octave: third-octave is the tonal-balance standard.
    pub smoothing: i32,
    pub snap: bool,
    /// Overlay stays up with the panel closed.
    pub pinned: bool,

    pub amount: f64,
    pub max_db: f64,
    /// Conform's bell budget (it uses at most 12; a state from before the fixed bells may say 16).
    pub max_bands: i32,
    pub cuts_only: bool,

    /// The slots Conform wrote.
    pub generated: Vec<i32>,
    /// The make-up gain the last Conform put on the output (dB), so the next Conform or Remove
    /// takes back only its own and leaves the user's trim. A state from before make-up reads 0.
    pub makeup_db: f64,

    /// The guides (`uiState/guides`, `CleanQGuides.h`): written after the `<ceiling>` element as
    /// the C++ `uiState` holds them side by side; a state from before guides were carried has
    /// none.
    pub guides: Guides,
}

impl Default for CeilingState {
    fn default() -> Self {
        CeilingState {
            shape: Ceiling::default(),
            noise_on: [false; NUM_NOISES],
            noise_manual: [false; NUM_NOISES],
            noise_level: [-50.0; NUM_NOISES],
            avg_seconds: 0.0,
            smoothing: 3,
            snap: true,
            pinned: false,
            amount: 1.0,
            max_db: 12.0,
            max_bands: 12,
            cuts_only: false,
            generated: Vec::new(),
            makeup_db: 0.0,
            guides: Guides::default(),
        }
    }
}

/// A double as the JUCE `var` prints it into XML: whole numbers keep a `.0`.
fn var_double(v: f64) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1.0e15 { format!("{v:.1}") } else { format!("{v}") }
}

fn bit(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

impl CeilingState {
    /// `writeTo`: the `<ceiling>` element, properties in the C++ order, then the points.
    pub fn write(&self) -> Vec<u8> {
        let mut noise_on = String::new();
        let mut manual = String::new();
        let mut level = String::new();
        for i in 0..NUM_NOISES {
            noise_on.push_str(bit(self.noise_on[i]));
            manual.push_str(bit(self.noise_manual[i]));
            if i > 0 {
                level.push(' ');
            }
            level.push_str(&format!("{:.2}", self.noise_level[i]));
        }
        let generated = self.generated.iter().map(i32::to_string).collect::<Vec<_>>().join(" ");

        let mut out = format!(
            "<ceiling noiseOn=\"{noise_on}\" noiseManual=\"{manual}\" noiseLevel=\"{level}\" avg=\"{}\" smooth=\"{}\" snap=\"{}\" pinned=\"{}\" amount=\"{}\" maxDb=\"{}\" maxBands=\"{}\" cutsOnly=\"{}\" generated=\"{generated}\"",
            var_double(self.avg_seconds),
            self.smoothing,
            bit(self.snap),
            bit(self.pinned),
            var_double(self.amount),
            var_double(self.max_db),
            self.max_bands,
            bit(self.cuts_only),
        );
        // Only when Conform set one, so a state from before make-up writes back as it was.
        if self.makeup_db != 0.0 {
            out.push_str(&format!(" makeupDb=\"{}\"", var_double(self.makeup_db)));
        }
        if self.shape.pts.is_empty() {
            out.push_str("/>");
        } else {
            out.push('>');
            for p in &self.shape.pts {
                out.push_str(&format!("<point hz=\"{}\" db=\"{}\"/>", var_double(p.hz), var_double(p.db)));
            }
            out.push_str("</ceiling>");
        }
        self.guides.write_xml(&mut out);
        out.into_bytes()
    }

    /// `readFrom`. `None` only for input the C++ could never have written (too big, not UTF-8);
    /// anything else reads as the C++ reads it: no `ceiling` element gives the defaults (no shape).
    pub fn read(data: &[u8]) -> Option<CeilingState> {
        if data.len() > MAX_STATE_BYTES {
            return None;
        }
        let text = std::str::from_utf8(data).ok()?;
        let mut s = CeilingState { guides: Guides::read_xml(text), ..CeilingState::default() };
        let Some(el) = find_element(text, "ceiling") else { return Some(s) };
        let attr = |name: &str| attribute(el.attrs, name);

        let on = attr("noiseOn").unwrap_or_default();
        let man = attr("noiseManual").unwrap_or_default();
        let lv: Vec<&str> = attr("noiseLevel").unwrap_or_default().split(' ').filter(|t| !t.is_empty()).collect();
        for i in 0..NUM_NOISES {
            // guarded: a state from an older build may carry a shorter string
            s.noise_on[i] = on.as_bytes().get(i) == Some(&b'1');
            s.noise_manual[i] = man.as_bytes().get(i) == Some(&b'1');
            if let Some(t) = lv.get(i) {
                s.noise_level[i] = jlimit(-120.0, 0.0, juce_double(t));
            }
        }

        s.avg_seconds = jlimit(0.0, 30.0, attr("avg").map_or(0.0, juce_double));
        s.smoothing = attr("smooth").map_or(3, juce_int).clamp(3, 12);
        s.snap = attr("snap").is_none_or(juce_bool);
        s.pinned = attr("pinned").is_some_and(juce_bool);
        s.amount = jlimit(0.0, 1.0, attr("amount").map_or(1.0, juce_double));
        s.max_db = jlimit(1.0, 24.0, attr("maxDb").map_or(12.0, juce_double));
        s.max_bands = attr("maxBands").map_or(12, juce_int).clamp(1, 24);
        s.cuts_only = attr("cutsOnly").is_some_and(juce_bool);

        s.makeup_db = jlimit(-24.0, 24.0, attr("makeupDb").map_or(0.0, juce_double));
        for t in attr("generated").unwrap_or_default().split(' ').filter(|t| !t.is_empty()).take(MAX_GENERATED) {
            s.generated.push(juce_int(t).clamp(0, 23));
        }

        // The points: every `point` child, as long as there could still be exactly 16.
        let mut rest = el.body;
        while s.shape.pts.len() <= NUM_NODES {
            let Some(p) = find_element(rest, "point") else { break };
            rest = p.after;
            let hz = attribute(p.attrs, "hz").map_or(1000.0, juce_double);
            let db = attribute(p.attrs, "db").map_or(-40.0, juce_double);
            s.shape.pts.push(Point { hz: jlimit(GRID_LO, GRID_HI, hz), db: jlimit(-120.0, 0.0, db) });
        }
        s.shape.sort();
        // a ceiling is always 16 nodes; one saved by the earlier build (a few long runs) is
        // dropped rather than stretched into something nobody drew
        if s.shape.pts.len() != NUM_NODES {
            s.shape.pts.clear();
        }
        Some(s)
    }
}

/// `juce::jlimit (lo, hi, v)`: `v < lo ? lo : (hi < v ? hi : v)`.
fn jlimit(lo: f64, hi: f64, v: f64) -> f64 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// The leading number of `t`, as `String::getDoubleValue` (`readDoubleValue`) reads it: optional
/// whitespace and sign, then "inf" (±infinity) or digits, a fraction, an exponent; nothing numeric
/// reads 0. Never-crash guard: "nan" reads 0, not NaN.
pub fn juce_double(t: &str) -> f64 {
    let t = t.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    let negative = b.first() == Some(&b'-');
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let word = |w: &str| t.get(i..i + w.len()).is_some_and(|x| x.eq_ignore_ascii_case(w));
    if word("inf") {
        return if negative { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    if word("nan") {
        return 0.0; // the guard: JUCE gives NaN
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > s
    };
    let mut any = digits(&mut i);
    if b.get(i) == Some(&b'.') {
        i += 1;
        any |= digits(&mut i);
    }
    if !any {
        return 0.0;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if digits(&mut j) {
            i = j;
        }
    }
    t.get(..i).and_then(|n| n.parse::<f64>().ok()).filter(|v| !v.is_nan()).unwrap_or(0.0)
}

/// The leading integer of `t`, as `String::getIntValue` (`readIntValue`) reads it: a leading `-`
/// is a sign, a `+` is not (so "+5" is 0). Saturating where JUCE would overflow.
pub fn juce_int(t: &str) -> i32 {
    let t = t.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, t.get(1..).unwrap_or_default()),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for c in digits.bytes().take_while(u8::is_ascii_digit) {
        v = v.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// A string `var` as a bool (`stringToBool`): a non-zero integer, "true" or "yes".
pub fn juce_bool(t: &str) -> bool {
    let w = t.trim();
    juce_int(t) != 0 || w.eq_ignore_ascii_case("true") || w.eq_ignore_ascii_case("yes")
}

/// One element found in XML text: its attribute text, the text up to its end tag (empty when it
/// closes itself or has none), and what follows its start tag.
pub(crate) struct Element<'a> {
    pub(crate) attrs: &'a str,
    pub(crate) body: &'a str,
    pub(crate) after: &'a str,
}

/// The first `<name …>` element in `text` (not a longer name that starts the same).
pub(crate) fn find_element<'a>(text: &'a str, name: &str) -> Option<Element<'a>> {
    let open = format!("<{name}");
    let mut from = 0;
    loop {
        let at = from + text.get(from..)?.find(&open)?;
        let after_name = at + open.len();
        let next = text.as_bytes().get(after_name).copied();
        if !matches!(next, Some(b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>')) {
            from = after_name;
            continue;
        }
        let rest = text.get(after_name..)?;
        let end = tag_end(rest)?;
        let self_closing = end > 0 && rest.as_bytes().get(end - 1) == Some(&b'/');
        let attrs = rest.get(..if self_closing { end - 1 } else { end })?;
        let after = rest.get(end + 1..)?;
        let body = if self_closing {
            ""
        } else {
            let close = format!("</{name}");
            after.find(&close).and_then(|c| after.get(..c)).unwrap_or(after)
        };
        return Some(Element { attrs, body, after });
    }
}

/// The index of the `>` closing a start tag, skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, c) in s.bytes().enumerate() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return Some(i),
            None => {}
        }
    }
    None
}

/// The value of attribute `name` in a start tag's attribute text (entities left as they are: the
/// values this reads are numbers and flags).
pub(crate) fn attribute<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut s = attrs;
    loop {
        s = s.trim_start();
        let eq = s.find('=')?;
        let key = s.get(..eq)?.trim();
        let rest = s.get(eq + 1..)?.trim_start();
        let q = rest.chars().next()?;
        if q != '"' && q != '\'' {
            return None;
        }
        let val_end = rest.get(1..)?.find(q)?;
        let val = rest.get(1..1 + val_end)?;
        if key == name {
            return Some(val);
        }
        s = rest.get(val_end + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::super::clean_q_ceiling::{auto_ceiling, flat_ceiling};
    use super::*;

    #[test]
    fn defaults_are_the_cpp_defaults() {
        let s = CeilingState::default();
        assert_eq!((s.avg_seconds, s.smoothing, s.snap, s.pinned), (0.0, 3, true, false));
        assert_eq!((s.amount, s.max_db, s.max_bands, s.cuts_only, s.makeup_db), (1.0, 12.0, 12, false, 0.0));
        assert_eq!(s.noise_level, [-50.0; 5]);
        assert!(s.shape.pts.is_empty() && s.generated.is_empty());
    }

    #[test]
    fn writes_the_cpp_layout_and_reads_it_back() {
        let mut s = CeilingState { shape: flat_ceiling(-30.0), ..CeilingState::default() };
        s.shape.pts[3].db = -27.25;
        s.noise_on[1] = true;
        s.noise_level[1] = -41.237;
        s.avg_seconds = 10.0;
        s.smoothing = 6;
        s.generated = vec![0, 4];
        s.makeup_db = -2.5;
        let text = String::from_utf8(s.write()).unwrap();
        assert!(
            text.starts_with(
                "<ceiling noiseOn=\"01000\" noiseManual=\"00000\" noiseLevel=\"-50.00 -41.24 -50.00 -50.00 -50.00\" avg=\"10.0\" smooth=\"6\" snap=\"1\" pinned=\"0\" amount=\"1.0\" maxDb=\"12.0\" maxBands=\"12\" cutsOnly=\"0\" generated=\"0 4\" makeupDb=\"-2.5\"><point hz=\"20.0\" db=\"-30.0\"/>"
            ),
            "{text}"
        );
        assert!(text.ends_with("</ceiling>"));
        let back = CeilingState::read(text.as_bytes()).unwrap();
        assert_eq!(back.shape, s.shape, "the points round-trip exactly");
        assert_eq!(back.noise_level[1], -41.24, "two decimals, as String (level, 2)");
        assert_eq!((back.avg_seconds, back.smoothing, back.generated.clone(), back.makeup_db), (10.0, 6, vec![0, 4], -2.5));
        // A learned (irregular) shape round-trips bit for bit.
        let grid: Vec<f64> = (0..super::super::clean_q_ceiling::GRID_N).map(|i| -40.0 + 7.0 * (i as f64 * 0.11).sin()).collect();
        let learned = CeilingState { shape: auto_ceiling(&grid), ..CeilingState::default() };
        assert_eq!(CeilingState::read(&learned.write()).unwrap().shape, learned.shape);
    }

    #[test]
    fn reads_a_whole_cpp_ui_state_and_juce_conversions() {
        let mut pts = String::new();
        // out of order, and out of range: sorted and limited as the C++ does
        for k in (0..16).rev() {
            pts.push_str(&format!("<point hz=\"{}\" db=\"{}\"/>", 10.0 + 2000.0 * k as f64, -10.0 * k as f64 + 5.0));
        }
        let xml = format!(
            "<?xml version=\"1.0\"?><state><uiState dbRange=\"18\"><guides/><ceiling avg=\"99\" smooth=\"2.9\" snap=\"true\" pinned=\"yes\" maxBands=\"0\" amount=\"abc\" maxDb=\"inf\">{pts}</ceiling></uiState></state>"
        );
        let s = CeilingState::read(xml.as_bytes()).unwrap();
        assert_eq!(s.avg_seconds, 30.0);
        assert_eq!(s.smoothing, 3, "getIntValue reads 2, limited to 3");
        assert!(s.snap && s.pinned, "\"true\" and \"yes\" are true");
        assert_eq!(s.max_bands, 1);
        assert_eq!(s.amount, 0.0, "garbage reads 0");
        assert_eq!(s.max_db, 24.0, "\"inf\" is infinity to JUCE, limited to 24");
        assert_eq!(s.shape.pts.len(), 16);
        assert_eq!(s.shape.pts[0], Point { hz: 20.0, db: 0.0 });
        assert!(s.shape.pts.windows(2).all(|w| w[0].hz <= w[1].hz));
        assert_eq!(s.shape.pts[15].hz, 20000.0);
    }

    #[test]
    fn corrupt_states_fall_back_without_crashing() {
        // garbage bytes, not UTF-8, too big: refused; text with no ceiling: the defaults
        assert_eq!(CeilingState::read(&[0xff, 0xfe, 0x00, 0x80]), None);
        assert_eq!(CeilingState::read(&vec![b'<'; MAX_STATE_BYTES + 1]), None);
        for junk in ["", "hello", "<ceiling", "<ceiling noiseOn=\"1", "<ceilingX/>", "<<<>>>\"'", "<ceiling a=b/>"] {
            let s = CeilingState::read(junk.as_bytes()).unwrap();
            assert!(s.shape.pts.is_empty(), "{junk}");
        }
        // the wrong number of points: dropped
        let few = "<ceiling><point hz=\"100\" db=\"-3\"/><point hz=\"200\" db=\"-3\"/></ceiling>";
        assert!(CeilingState::read(few.as_bytes()).unwrap().shape.pts.is_empty());
        // absurd counts: a million points read no further than 17, and drop the shape
        let many = format!("<ceiling>{}</ceiling>", "<point hz=\"100\" db=\"-3\"/>".repeat(30_000));
        assert!(CeilingState::read(many.as_bytes()).unwrap().shape.pts.is_empty());
        // NaN points read 0 (the guard), then are limited; -inf is held at the floor
        let nan: String =
            (0..16).map(|k| format!("<point hz=\"{}\" db=\"nan\"/>", if k == 3 { "NaN".to_string() } else { (100 * (k + 1)).to_string() })).collect();
        let s = CeilingState::read(format!("<ceiling>{nan}</ceiling>").as_bytes()).unwrap();
        assert_eq!(s.shape.pts.len(), 16);
        assert!(s.shape.pts.iter().all(|p| p.hz.is_finite() && p.db == 0.0));
        assert!(s.shape.to_grid().iter().all(|v| v.is_finite()));
        let inf: String = (0..16).map(|k| format!("<point hz=\"{}\" db=\"-inf\"/>", 100 * (k + 1))).collect();
        let s = CeilingState::read(format!("<ceiling>{inf}</ceiling>").as_bytes()).unwrap();
        assert!(s.shape.pts.iter().all(|p| p.db == -120.0));
        let huge = "<ceiling generated=\"99999999999999 -5 7\"/>";
        assert_eq!(CeilingState::read(huge.as_bytes()).unwrap().generated, vec![23, 0, 7]);
    }

    #[test]
    fn guides_round_trip_through_module_state() {
        use super::super::clean_q_guides::Guide;
        let mut s = CeilingState { shape: super::super::clean_q_ceiling::flat_ceiling(-40.0), ..CeilingState::default() };
        s.guides.add(Guide { dynamic: true, ..Guide::around(250.0, -4.5) });
        s.guides.add(Guide { done: true, ..Guide::around(2500.0, 3.0) });
        s.guides.add(Guide { note: "harsh <here> & \"there\"".into(), ..Guide::around(6000.0, -2.0) });
        let back = CeilingState::read(&s.write()).unwrap();
        assert_eq!(back, s, "every field, guides included");
        assert_eq!(back.guides.list.len(), 3);
        assert!(back.guides.list[0].dynamic && back.guides.list[1].done);
        assert_eq!(back.guides.list[2].note, "harsh <here> & \"there\"");
    }

    /// A state written before guides were carried (`feat/remaining`'s `write`).
    const OLD_STATE: &str = "<ceiling noiseOn=\"10000\" noiseManual=\"00000\" noiseLevel=\"-50.00 -50.00 -50.00 -50.00 -50.00\" avg=\"0.0\" smooth=\"3\" snap=\"1\" pinned=\"0\" amount=\"1.0\" maxDb=\"12.0\" maxBands=\"16\" cutsOnly=\"0\" generated=\"2 5\"/>";

    #[test]
    fn old_ceiling_state_loads_with_no_guides() {
        let s = CeilingState::read(OLD_STATE.as_bytes()).unwrap();
        assert!(s.guides.list.is_empty());
        assert!(s.noise_on[0] && !s.noise_on[1]);
        assert_eq!(s.generated, [2, 5]);
        // Its Conform bands stay as they are (they are parameters); its budget of 16 is kept
        // (Conform now uses at most 12) and it had no make-up.
        assert_eq!((s.max_bands, s.makeup_db), (16, 0.0));
        // With no guides, it writes back byte for byte as the older build wrote it.
        assert_eq!(s.write(), OLD_STATE.as_bytes());
    }

    #[test]
    fn hostile_guides_blob_is_trimmed() {
        let mut text = String::from("<ceiling/><guides>");
        for i in 0..1000 {
            text.push_str(&format!("<guide freq=\"{}\" lo=\"{}\" hi=\"{}\" gain=\"-3\"/>", 100 + i, 90 + i, 120 + i));
        }
        text.push_str("</guides>");
        assert_eq!(CeilingState::read(text.as_bytes()).unwrap().guides.list.len(), super::super::clean_q_guides::MAX_GUIDES);

        let bad = "<ceiling/><guides>\
            <guide freq=\"nan\" lo=\"nan\" hi=\"nan\"/>\
            <guide freq=\"1000\" lo=\"1200\" hi=\"900\"/>\
            <guide freq=\"1000\" lo=\"1000\" hi=\"1000\"/>\
            <guide freq=\"50000\" lo=\"40000\" hi=\"60000\"/>\
            <guide freq=\"5\" lo=\"2\" hi=\"8\"/>\
            <guide freq=\"inf\" lo=\"100\" hi=\"200\"/>\
            <guide freq=\"1000\" lo=\"800\" hi=\"1250\" gain=\"99\" note=\"ok\"/>\
            </guides>";
        let g = CeilingState::read(bad.as_bytes()).unwrap().guides;
        assert_eq!(g.list.len(), 1, "{g:?}");
        assert_eq!((g.list[0].gain_db, g.list[0].note.as_str()), (30.0, "ok"));

        // A 1 MiB note of '&' (and one of "&amp;") reads fast: the unescaping is linear and cut.
        for unit in ["&", "&amp;", "&#"] {
            let note = unit.repeat(((1 << 20) - 200) / unit.len());
            let blob = format!("<ceiling/><guides><guide freq=\"1000\" lo=\"800\" hi=\"1250\" note=\"{note}\"/></guides>");
            let t0 = std::time::Instant::now();
            let g = CeilingState::read(blob.as_bytes()).unwrap().guides;
            assert!(t0.elapsed() < std::time::Duration::from_millis(500), "{unit}: {:?}", t0.elapsed());
            assert_eq!(g.list.len(), 1);
            assert!(g.list[0].note.chars().count() <= 80);
        }

        // Random bytes never panic.
        let mut seed = 12345u32;
        for _ in 0..50 {
            let bytes: Vec<u8> = (0..4096)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect();
            let _ = CeilingState::read(&bytes);
            let ascii: String = bytes.iter().map(|b| b"<guide freq=\"1 \"/>s"[usize::from(*b) % 19] as char).collect();
            let _ = CeilingState::read(ascii.as_bytes());
        }
    }

    #[test]
    fn juce_number_reading() {
        assert_eq!(juce_double(" -12.5dB"), -12.5);
        assert_eq!(juce_double("1e3x"), 1000.0);
        assert_eq!(juce_double("1e"), 1.0);
        assert_eq!(juce_double(".5"), 0.5);
        assert_eq!(juce_double("nan"), 0.0, "the never-crash guard");
        assert_eq!(juce_double(" -INF"), f64::NEG_INFINITY);
        assert_eq!(juce_double("+inf"), f64::INFINITY);
        assert_eq!(juce_double("-"), 0.0);
        assert_eq!(juce_double("1e999"), f64::INFINITY);
        assert_eq!(juce_int("12.9"), 12);
        assert_eq!(juce_int("-99999999999"), i32::MIN);
        assert_eq!(juce_int("+5"), 0, "no leading + in readIntValue");
        assert!(juce_bool("2") && juce_bool(" TRUE ") && juce_bool("Yes") && !juce_bool("0") && !juce_bool("x") && !juce_bool("+1"));
    }
}
