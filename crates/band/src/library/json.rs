//! `juce::var` and `juce::JSON` as MIDI Studio's loaders use them, so a file reads exactly as MS reads it:
//! - [`Var`]: juce's variant. Object properties keep file order (a repeated key keeps its first place and
//!   takes the last value, as `NamedValueSet::set` does); `null` is void; arrays count as objects for
//!   `isObject()` but have no properties, as in juce.
//! - [`parse`]: a port of juce's `JSONParser` (its leniency and its error text, `"line:col: error: ..."`).
//! - The juce string helpers the loaders call (`getIntValue`, `getTrailingIntValue`, `String (double)`,
//!   `compareIgnoreCase`, `File::createLegalFileName`).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Var {
    #[default]
    Void,
    Bool(bool),
    /// juce's `int` and `int64`.
    Int(i64),
    Double(f64),
    Str(String),
    Arr(Vec<Var>),
    Obj(Vec<(String, Var)>),
}

static VOID: Var = Var::Void;

impl Var {
    pub fn is_void(&self) -> bool {
        matches!(self, Var::Void)
    }
    pub fn is_string(&self) -> bool {
        matches!(self, Var::Str(_))
    }
    /// MS's `isNumber`: `isInt() || isInt64() || isDouble()`.
    pub fn is_number(&self) -> bool {
        matches!(self, Var::Int(_) | Var::Double(_))
    }
    /// juce: arrays are objects too.
    pub fn is_object(&self) -> bool {
        matches!(self, Var::Obj(_) | Var::Arr(_))
    }
    /// `getArray()`
    pub fn array(&self) -> Option<&[Var]> {
        match self {
            Var::Arr(a) => Some(a),
            _ => None,
        }
    }
    /// `getDynamicObject()` (its properties, in order)
    pub fn dyn_obj(&self) -> Option<&[(String, Var)]> {
        match self {
            Var::Obj(o) => Some(o),
            _ => None,
        }
    }
    /// `getProperty (key, var())`
    pub fn prop(&self, key: &str) -> &Var {
        self.dyn_obj().and_then(|o| o.iter().find(|(k, _)| k == key)).map_or(&VOID, |(_, v)| v)
    }
    /// `getProperty (key, def)`: a property that is present (even `null`) wins over the default.
    pub fn prop_or<'a>(&'a self, key: &str, def: &'a Var) -> &'a Var {
        self.dyn_obj().and_then(|o| o.iter().find(|(k, _)| k == key)).map_or(def, |(_, v)| v)
    }
    /// `getProperty (key, def).toString()`
    pub fn prop_str(&self, key: &str, def: &str) -> String {
        match self.dyn_obj().and_then(|o| o.iter().find(|(k, _)| k == key)) {
            Some((_, v)) => v.to_jstring(),
            None => def.to_string(),
        }
    }
    /// `(int) getProperty (key, def)`
    pub fn prop_int(&self, key: &str, def: i32) -> i32 {
        match self.dyn_obj().and_then(|o| o.iter().find(|(k, _)| k == key)) {
            Some((_, v)) => v.to_int(),
            None => def,
        }
    }
    /// `(bool) getProperty (key, def)`
    pub fn prop_bool(&self, key: &str, def: bool) -> bool {
        match self.dyn_obj().and_then(|o| o.iter().find(|(k, _)| k == key)) {
            Some((_, v)) => v.to_bool(),
            None => def,
        }
    }
    /// `hasProperty`
    pub fn has(&self, key: &str) -> bool {
        self.dyn_obj().is_some_and(|o| o.iter().any(|(k, _)| k == key))
    }
    /// `(int) v`: doubles truncate (saturating where C++ is undefined), strings read `getIntValue`.
    pub fn to_int(&self) -> i32 {
        match self {
            Var::Bool(b) => i32::from(*b),
            Var::Int(n) => *n as i32,
            Var::Double(d) => *d as i32,
            Var::Str(s) => get_int_value(s),
            _ => 0,
        }
    }
    /// `(double) v`
    pub fn to_double(&self) -> f64 {
        match self {
            Var::Bool(b) => f64::from(u8::from(*b)),
            Var::Int(n) => *n as f64,
            Var::Double(d) => *d,
            Var::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let mut pos = 0;
                read_double_value(&chars, &mut pos)
            }
            _ => 0.0,
        }
    }
    /// `(bool) v`
    pub fn to_bool(&self) -> bool {
        match self {
            Var::Void => false,
            Var::Bool(b) => *b,
            Var::Int(n) => *n != 0,
            Var::Double(d) => *d != 0.0,
            Var::Str(s) => get_int_value(s) != 0 || s.trim().eq_ignore_ascii_case("true") || s.trim().eq_ignore_ascii_case("yes"),
            Var::Arr(_) | Var::Obj(_) => true,
        }
    }
    /// `v.toString()`
    pub fn to_jstring(&self) -> String {
        match self {
            Var::Void => String::new(),
            Var::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
            Var::Int(n) => n.to_string(),
            Var::Double(d) => serialise_double(*d),
            Var::Str(s) => s.clone(),
            Var::Arr(_) => "[Array]".to_string(),
            Var::Obj(_) => "Object".to_string(),
        }
    }
}

// ---- Parsing (juce_JSON.cpp, JSONParser) ------------------------------------------------------

/// Deeper nesting is an error rather than juce's stack overflow.
const MAX_DEPTH: usize = 256;

struct Parser {
    s: Vec<char>,
    pos: usize,
    depth: usize,
}

type PResult<T> = Result<T, String>;

impl Parser {
    fn at(&self, i: usize) -> char {
        self.s.get(i).copied().unwrap_or('\0')
    }
    fn throw_error<T>(&self, message: &str, location: usize) -> PResult<T> {
        let (mut line, mut column) = (1, 1);
        let mut i = 0;
        while i < location && self.at(i) != '\0' {
            column += 1;
            if self.at(i) == '\n' {
                column = 1;
                line += 1;
            }
            i += 1;
        }
        Err(format!("{line}:{column}: error: {message}"))
    }
    fn skip_whitespace(&mut self) {
        while self.at(self.pos) != '\0' && self.at(self.pos).is_whitespace() {
            self.pos += 1;
        }
    }
    fn read_char(&mut self) -> char {
        let c = self.at(self.pos);
        self.pos += 1;
        c
    }
    fn peek_char(&self) -> char {
        self.at(self.pos)
    }
    fn match_if(&mut self, c: char) -> bool {
        if self.peek_char() == c {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn is_eof(&self) -> bool {
        self.peek_char() == '\0'
    }
    fn match_string(&mut self, t: &str) -> bool {
        t.chars().all(|c| self.match_if(c))
    }

    fn parse_object_or_array(&mut self) -> PResult<Var> {
        self.skip_whitespace();
        if self.match_if('{') {
            return self.parse_object();
        }
        if self.match_if('[') {
            return self.parse_array();
        }
        if !self.is_eof() {
            return self.throw_error("Expected '{' or '['", self.pos);
        }
        Ok(Var::Void)
    }

    fn parse_hex_digit(&mut self) -> PResult<u32> {
        match self.read_char().to_digit(16) {
            Some(d) => Ok(d),
            None => self.throw_error("Invalid hex character", self.pos.saturating_sub(1)),
        }
    }
    fn parse_code_unit(&mut self) -> PResult<u32> {
        Ok((self.parse_hex_digit()? << 12) | (self.parse_hex_digit()? << 8) | (self.parse_hex_digit()? << 4) | self.parse_hex_digit()?)
    }
    fn parse_low_surrogate_code_unit(&mut self) -> PResult<u32> {
        let error_location = self.pos;
        if self.read_char() != '\\' || self.read_char() != 'u' {
            return self.throw_error("Expected UTF-16 low surrogate", error_location);
        }
        let low = self.parse_code_unit()?;
        if !(0xDC00..=0xDFFF).contains(&low) {
            return self.throw_error("Expected UTF-16 low surrogate", error_location);
        }
        Ok(low)
    }
    fn parse_escape_sequence(&mut self) -> PResult<char> {
        let error_location = self.pos.saturating_sub(2);
        let first = self.parse_code_unit()?;
        if !(0xD800..=0xDFFF).contains(&first) {
            return Ok(char::from_u32(first).unwrap_or('\u{FFFD}'));
        }
        if !(0xD800..=0xDBFF).contains(&first) {
            return self.throw_error("Invalid UTF-16 escape sequence", error_location);
        }
        let low = self.parse_low_surrogate_code_unit()?;
        Ok(char::from_u32(0x10000 + ((first - 0xD800) << 10) + (low - 0xDC00)).unwrap_or('\u{FFFD}'))
    }

    fn parse_string(&mut self, quote_char: char) -> PResult<String> {
        let mut buffer = String::new();
        loop {
            let mut c = self.read_char();
            if c == quote_char {
                break;
            }
            if c == '\\' {
                c = self.read_char();
                match c {
                    'a' => c = '\u{7}',
                    'b' => c = '\u{8}',
                    'f' => c = '\u{c}',
                    'n' => c = '\n',
                    'r' => c = '\r',
                    't' => c = '\t',
                    'u' => c = self.parse_escape_sequence()?,
                    _ => {}
                }
            }
            if c == '\0' {
                return self.throw_error("Unexpected EOF in string constant", self.pos);
            }
            buffer.push(c);
        }
        Ok(buffer)
    }

    fn parse_any(&mut self) -> PResult<Var> {
        self.skip_whitespace();
        let original_location = self.pos;
        match self.read_char() {
            '{' => return self.parse_object(),
            '[' => return self.parse_array(),
            '"' => return self.parse_string('"').map(Var::Str),
            '\'' => return self.parse_string('\'').map(Var::Str),
            '-' => {
                self.skip_whitespace();
                return self.parse_number(true);
            }
            '0'..='9' => {
                self.pos = original_location;
                return self.parse_number(false);
            }
            't' if self.match_string("rue") => return Ok(Var::Bool(true)),
            'f' if self.match_string("alse") => return Ok(Var::Bool(false)),
            'n' if self.match_string("ull") => return Ok(Var::Void),
            _ => {}
        }
        self.throw_error("Syntax error", original_location)
    }

    fn parse_number(&mut self, is_negative: bool) -> PResult<Var> {
        let original_pos = self.pos;
        let mut int_value: i64 = i64::from(u32::from(self.read_char())).wrapping_sub(i64::from(b'0'));
        loop {
            let last_pos = self.pos;
            let c = self.read_char();
            if c.is_ascii_digit() {
                int_value = int_value.wrapping_mul(10).wrapping_add(i64::from(u32::from(c) - u32::from('0')));
                continue;
            }
            if c == 'e' || c == 'E' || c == '.' {
                self.pos = original_pos;
                let as_double = read_double_value(&self.s, &mut self.pos);
                return Ok(Var::Double(if is_negative { -as_double } else { as_double }));
            }
            if c.is_whitespace() || c == ',' || c == '}' || c == ']' || c == '\0' {
                self.pos = last_pos;
                break;
            }
            return self.throw_error("Syntax error in number", last_pos);
        }
        Ok(Var::Int(if is_negative { int_value.wrapping_neg() } else { int_value }))
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.throw_error("Too deeply nested", self.pos);
        }
        Ok(())
    }

    fn parse_object(&mut self) -> PResult<Var> {
        self.enter()?;
        let mut props: Vec<(String, Var)> = Vec::new();
        // name -> index into props: a repeated key keeps its first place and takes the last value, in O(1).
        let mut index: HashMap<String, usize> = HashMap::new();
        let start_of_object_decl = self.pos;
        loop {
            self.skip_whitespace();
            let mut error_location = self.pos;
            let c = self.read_char();
            if c == '}' {
                break;
            }
            if c == '\0' {
                return self.throw_error("Unexpected EOF in object declaration", start_of_object_decl);
            }
            if c != '"' {
                return self.throw_error("Expected a property name in double-quotes", error_location);
            }
            error_location = self.pos;
            let property_name = self.parse_string('"')?;
            if property_name.is_empty() {
                return self.throw_error("Invalid property name", error_location);
            }
            self.skip_whitespace();
            error_location = self.pos;
            if self.read_char() != ':' {
                return self.throw_error("Expected ':'", error_location);
            }
            let value = self.parse_any()?;
            match index.get(&property_name).and_then(|&i| props.get_mut(i)) {
                Some(slot) => slot.1 = value,
                None => {
                    index.insert(property_name.clone(), props.len());
                    props.push((property_name, value));
                }
            }
            self.skip_whitespace();
            if self.match_if(',') {
                continue;
            }
            if self.match_if('}') {
                break;
            }
            return self.throw_error("Expected ',' or '}'", self.pos);
        }
        self.depth -= 1;
        Ok(Var::Obj(props))
    }

    fn parse_array(&mut self) -> PResult<Var> {
        self.enter()?;
        let mut items = Vec::new();
        let start_of_array_decl = self.pos;
        loop {
            self.skip_whitespace();
            if self.match_if(']') {
                break;
            }
            if self.is_eof() {
                return self.throw_error("Unexpected EOF in array declaration", start_of_array_decl);
            }
            items.push(self.parse_any()?);
            self.skip_whitespace();
            if self.match_if(',') {
                continue;
            }
            if self.match_if(']') {
                break;
            }
            return self.throw_error("Expected ',' or ']'", self.pos);
        }
        self.depth -= 1;
        Ok(Var::Arr(items))
    }
}

/// `JSON::parse (text, result)`: `Err` holds juce's description (`"3:7: error: Syntax error"`). An empty text
/// parses to void; anything after the top-level object or array is ignored, as in juce.
pub fn parse(text: &str) -> Result<Var, String> {
    let mut p = Parser { s: text.chars().collect(), pos: 0, depth: 0 };
    p.parse_object_or_array()
}

/// `JSON::parse (text)`: void when the text does not parse.
pub fn parse_or_void(text: &str) -> Var {
    parse(text).unwrap_or_default()
}

/// File bytes as `File::loadFileAsString` reads them (UTF-8, its BOM dropped; UTF-16 with a BOM).
pub fn text_from_bytes(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> =
            rest.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    String::from_utf8_lossy(b).into_owned()
}

/// `CharacterFunctions::readDoubleValue`: whitespace, a sign, NaN / Inf, at most 18 significant digits, an exponent.
/// Leaves `pos` after the number (or where it started when there is none).
pub fn read_double_value(s: &[char], pos: &mut usize) -> f64 {
    const MAX_SIG: i32 = 18;
    let at = |i: usize| s.get(i).copied().unwrap_or('\0');
    while at(*pos) != '\0' && at(*pos).is_whitespace() {
        *pos += 1;
    }
    let end_of_whitespace = *pos;
    let mut buffer = String::new();
    let mut is_negative = false;
    let mut c = at(*pos);
    if c == '-' || c == '+' {
        if c == '-' {
            is_negative = true;
            buffer.push('-');
        }
        *pos += 1;
        c = at(*pos);
    }
    if c == 'n' || c == 'N' {
        if matches!(at(*pos + 1), 'a' | 'A') && matches!(at(*pos + 2), 'n' | 'N') {
            *pos += 3;
            return f64::NAN;
        }
        *pos = end_of_whitespace;
        return 0.0;
    }
    if c == 'i' || c == 'I' {
        if matches!(at(*pos + 1), 'n' | 'N') && matches!(at(*pos + 2), 'f' | 'F') {
            *pos += 3;
            return if is_negative { f64::NEG_INFINITY } else { f64::INFINITY };
        }
        *pos = end_of_whitespace;
        return 0.0;
    }
    let (mut num_sig_figs, mut extra_exponent) = (0, 0);
    let (mut decimal_point_found, mut leading_zeros) = (false, false);
    loop {
        let d = at(*pos);
        if d.is_ascii_digit() {
            *pos += 1;
            if decimal_point_found {
                if num_sig_figs >= MAX_SIG {
                    continue;
                }
            } else {
                if num_sig_figs >= MAX_SIG {
                    extra_exponent = i32::saturating_add(extra_exponent, 1);
                    continue;
                }
                if num_sig_figs == 0 && d == '0' {
                    leading_zeros = true;
                    continue;
                }
            }
            buffer.push(d);
            num_sig_figs += 1;
        } else if !decimal_point_found && d == '.' {
            *pos += 1;
            buffer.push('.');
            decimal_point_found = true;
        } else {
            break;
        }
    }
    if !leading_zeros && num_sig_figs == 0 {
        *pos = end_of_whitespace;
        return 0.0;
    }
    let c = at(*pos);
    if c == 'e' || c == 'E' {
        let start_of_exponent = *pos;
        let mut positive = true;
        *pos += 1;
        match at(*pos) {
            '-' => {
                positive = false;
                *pos += 1;
            }
            '+' => *pos += 1,
            _ => {}
        }
        let mut exponent: i32 = 0;
        let start_of_exponent_digits = *pos;
        while at(*pos).is_ascii_digit() {
            let digit = at(*pos) as i32 - '0' as i32;
            *pos += 1;
            if digit != 0 || exponent != 0 {
                exponent = exponent.saturating_mul(10).saturating_add(digit);
            }
        }
        if *pos == start_of_exponent_digits {
            *pos = start_of_exponent;
        }
        exponent = extra_exponent.saturating_add(if positive { exponent } else { exponent.saturating_neg() });
        if exponent < f64::MIN_10_EXP - 1 {
            return if is_negative { -0.0 } else { 0.0 };
        }
        if exponent > f64::MAX_10_EXP + 1 {
            return if is_negative { f64::NEG_INFINITY } else { f64::INFINITY };
        }
        return strtod(&buffer, exponent);
    }
    strtod(&buffer, extra_exponent)
}

/// `strtod` on juce's buffer: a signed mantissa ("1.", ".5", "" are numbers to strtod) and an exponent.
fn strtod(mantissa: &str, exponent: i32) -> f64 {
    let (sign, body) = match mantissa.strip_prefix('-') {
        Some(b) => ("-", b),
        None => ("", mantissa),
    };
    if !body.contains(|c: char| c.is_ascii_digit()) {
        return 0.0;
    }
    let body = if body.starts_with('.') { format!("0{body}") } else { body.to_string() };
    let body = if body.ends_with('.') { format!("{body}0") } else { body };
    format!("{sign}{body}e{exponent}").parse::<f64>().unwrap_or(0.0)
}

// ---- juce::String helpers --------------------------------------------------------------------------

/// `String::getIntValue`: whitespace, an optional '-', digits (wrapping).
pub fn get_int_value(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let mut v: u32 = 0;
    for c in digits.chars() {
        match c.to_digit(10) {
            Some(d) if c.is_ascii_digit() => v = v.wrapping_mul(10).wrapping_add(d),
            _ => break,
        }
    }
    if neg { (v as i32).wrapping_neg() } else { v as i32 }
}

/// `String::getTrailingIntValue`: the digits at the end ("lane3" -> 3, "laneA" -> 0), negated after a '-'.
pub fn get_trailing_int_value(s: &str) -> i32 {
    let (mut n, mut mult): (i32, i32) = (0, 1);
    for c in s.chars().rev() {
        if !c.is_ascii_digit() {
            if c == '-' {
                n = n.wrapping_neg();
            }
            break;
        }
        n = n.wrapping_add(mult.wrapping_mul(c as i32 - '0' as i32));
        mult = mult.wrapping_mul(10);
    }
    n
}

/// `String::containsOnly ("0123456789")` (true for "").
pub fn contains_only_digits(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit())
}

fn upper(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// `String::compareIgnoreCase` (char by char, upper-cased).
pub fn compare_ignore_case(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars(), b.chars());
    loop {
        let (c1, c2) = (x.next().unwrap_or('\0'), y.next().unwrap_or('\0'));
        if c1 != c2 {
            let o = upper(c1).cmp(&upper(c2));
            if o != Ordering::Equal {
                return o;
            }
        }
        if c1 == '\0' {
            return Ordering::Equal;
        }
    }
}

/// `File::createLegalFileName`
#[cfg_attr(target_arch = "wasm32", allow(dead_code))] // saving is native only
pub fn create_legal_file_name(original: &str) -> String {
    let s: Vec<char> = original.chars().filter(|c| !"\"#@,;:<>*^|?\\/".contains(*c)).collect();
    const MAX_LENGTH: usize = 128;
    let len = s.len();
    if len <= MAX_LENGTH {
        return s.into_iter().collect();
    }
    let last_dot = s.iter().rposition(|&c| c == '.');
    match last_dot {
        Some(d) if d as i64 > (len as i64 - 12).max(0) => {
            let keep = MAX_LENGTH.saturating_sub(len - d);
            s[..keep.min(len)].iter().chain(s[d..].iter()).collect()
        }
        _ => s[..MAX_LENGTH].iter().collect(),
    }
}

/// `juce::String (double)` (and `(float)`): `std::ostream << d` with its defaults, i.e. printf `%g`.
pub fn fmt_g(d: f64) -> String {
    if d.is_nan() {
        return "nan".to_string();
    }
    if d.is_infinite() {
        return if d < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if d == 0.0 {
        return if d.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    const P: i32 = 6;
    let sci = format!("{:.*e}", (P - 1) as usize, d);
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    let strip = |s: String| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if !(-4..P).contains(&x) {
        let m = strip(mant.to_string());
        format!("{m}e{}{:02}", if x < 0 { '-' } else { '+' }, x.abs())
    } else {
        strip(format!("{:.*}", (P - 1 - x) as usize, d))
    }
}

/// `var (double).toString()` (juce's `serialiseDouble`), close enough for the error texts it appears in.
fn serialise_double(d: f64) -> String {
    if d.is_finite() && d == d.trunc() && d.abs() < 1.0e6 { format!("{d:.1}") } else { format!("{d}") }
}

// ---- Writing ------------------------------------------------------------------------------------

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_var(out: &mut String, v: &Var, indent: usize) {
    match v {
        Var::Void => out.push_str("null"),
        Var::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Var::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Var::Double(d) => {
            if d.is_finite() && d.abs() >= 1.0e15 {
                // with an exponent: plain digits would read back as a (wrapping) integer
                let _ = write!(out, "{d:e}");
            } else if d.is_finite() {
                let _ = write!(out, "{d}");
            } else {
                out.push_str("null");
            }
        }
        Var::Str(s) => write_string(out, s),
        Var::Arr(a) if a.is_empty() => out.push_str("[]"),
        Var::Obj(o) if o.is_empty() => out.push_str("{}"),
        Var::Arr(a) => {
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                out.push_str(&" ".repeat(indent + 2));
                write_var(out, x, indent + 2);
                out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&" ".repeat(indent));
            out.push(']');
        }
        Var::Obj(o) => {
            out.push_str("{\n");
            for (i, (k, x)) in o.iter().enumerate() {
                out.push_str(&" ".repeat(indent + 2));
                write_string(out, k);
                out.push_str(": ");
                write_var(out, x, indent + 2);
                out.push_str(if i + 1 < o.len() { ",\n" } else { "\n" });
            }
            out.push_str(&" ".repeat(indent));
            out.push('}');
        }
    }
}

/// `JSON::toString (v)`: multi-line, two-space indent. Doubles are written so they read back exactly.
pub fn to_string(v: &Var) -> String {
    let mut out = String::new();
    write_var(&mut out, v, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_juce() {
        let v = parse(r#"{ "a": 1, "b": [1.5, -2, true, null, 'x'], "a": 3, "c": { } } trailing"#).expect("parses");
        let o = v.dyn_obj().expect("object");
        assert_eq!(o.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(v.prop("a"), &Var::Int(3));
        let b = v.prop("b").array().expect("array");
        assert_eq!(b, [Var::Double(1.5), Var::Int(-2), Var::Bool(true), Var::Void, Var::Str("x".into())]);
        assert!(v.has("c") && v.prop("c").is_object() && !v.has("d"));
        assert_eq!(parse(""), Ok(Var::Void));
        assert_eq!(parse("5"), Err("1:1: error: Expected '{' or '['".into()));
        assert_eq!(parse("{\n  \"x\": NaN }"), Err("2:8: error: Syntax error".into()));
        assert_eq!(parse("[1, 2"), Err("1:6: error: Expected ',' or ']'".into()));
        assert_eq!(parse("{ \"a\" 1 }"), Err("1:7: error: Expected ':'".into()));
        assert_eq!(parse("{ \"\": 1 }"), Err("1:4: error: Invalid property name".into()));
        assert_eq!(parse("[1x]"), Err("1:3: error: Syntax error in number".into()));
        assert!(parse(&"[".repeat(10_000)).is_err());
        assert_eq!(parse(r#"["é🎸"]"#), Ok(Var::Arr(vec![Var::Str("é🎸".into())])));
        assert_eq!(
            parse("[0.1, 1e3, 2E-2, 12345678901234567890123]").ok().and_then(|v| v.array().map(<[Var]>::to_vec)),
            Some(vec![Var::Double(0.1), Var::Double(1000.0), Var::Double(0.02), Var::Int(12345678901234567890123_i128 as i64)])
        );
    }

    #[test]
    fn huge_exponents_do_not_overflow() {
        assert_eq!(parse("[1234567890123456789e99999999999]"), Ok(Var::Arr(vec![Var::Double(f64::INFINITY)])));
        assert_eq!(parse("[1234567890123456789e-99999999999]"), Ok(Var::Arr(vec![Var::Double(0.0)])));
        let digits = format!("[{}e2147483647]", "9".repeat(40));
        assert_eq!(parse(&digits), Ok(Var::Arr(vec![Var::Double(f64::INFINITY)])));
    }

    #[test]
    fn many_keys_parse_in_linear_time() {
        let mut text = String::from("{");
        for i in 0..200_000 {
            let _ = write!(text, "\"k{i}\": {i},");
        }
        text.push_str("\"k5\": -1}");
        let t = std::time::Instant::now();
        let v = parse(&text).expect("parses");
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "took {:?}", t.elapsed());
        let o = v.dyn_obj().expect("object");
        assert_eq!(o.len(), 200_000);
        assert_eq!(o.get(5), Some(&("k5".to_string(), Var::Int(-1))), "a repeated key keeps its place, last value wins");
    }

    #[test]
    fn var_conversions_match_juce() {
        assert_eq!(Var::Double(2.9).to_int(), 2);
        assert_eq!(Var::Double(-2.9).to_int(), -2);
        assert_eq!(Var::Str(" 42abc".into()).to_int(), 42);
        assert_eq!(Var::Bool(true).to_jstring(), "1");
        assert!(Var::Str("yes".into()).to_bool() && !Var::Str("no".into()).to_bool());
        assert!(Var::Arr(vec![]).is_object() && Var::Arr(vec![]).dyn_obj().is_none());
        assert_eq!(Var::Arr(vec![]).prop_str("x", "def"), "def");
    }

    #[test]
    fn juce_string_helpers() {
        assert_eq!(get_trailing_int_value("lane3"), 3);
        assert_eq!(get_trailing_int_value("laneA"), 0);
        assert_eq!(get_trailing_int_value("E12"), 12);
        assert_eq!(get_int_value("-36x"), -36);
        assert_eq!(get_int_value("abc"), 0);
        assert_eq!(fmt_g(999.0), "999");
        assert_eq!(fmt_g(0.5), "0.5");
        assert_eq!(fmt_g(-100.0), "-100");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.0001), "0.0001");
        assert_eq!(fmt_g(0.00001), "1e-05");
        assert_eq!(fmt_g(2.25), "2.25");
        assert_eq!(compare_ignore_case("abc", "ABD"), Ordering::Less);
        assert_eq!(compare_ignore_case("Zed", "alton"), Ordering::Greater);
        assert_eq!(compare_ignore_case("ab", "AB"), Ordering::Equal);
        assert_eq!(compare_ignore_case("a", "ab"), Ordering::Less);
        assert_eq!(create_legal_file_name("My: Band?"), "My Band");
    }

    #[test]
    fn writes_and_reads_back() {
        let v = Var::Obj(vec![
            ("s".into(), Var::Str("a\"b\n".into())),
            ("d".into(), Var::Double(f64::from(0.1_f32))),
            ("a".into(), Var::Arr(vec![Var::Int(1), Var::Bool(false)])),
            ("e".into(), Var::Obj(vec![])),
            ("big".into(), Var::Double(1.0e20)),
            ("neg".into(), Var::Double(-123456789012345680.0)),
            ("f32".into(), Var::Double(f64::from(3.4e38_f32))),
        ]);
        assert_eq!(parse(&to_string(&v)), Ok(v));
    }
}
