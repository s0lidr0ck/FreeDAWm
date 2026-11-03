//! A small XML tree for DAWproject's `project.xml` and `metadata.xml`: parsed with `quick-xml`
//! (the file is hostile input: depth and size are capped, and any malformation is an error, never
//! a panic) and written back with escaping.

use quick_xml::events::Event;

/// Deepest element nesting read (DAWproject nests a few levels; a hostile file may nest forever).
const MAX_DEPTH: usize = 256;
/// Most elements read from one document.
const MAX_ELEMENTS: usize = 4_000_000;

/// An element: name, attributes in file order, children, and its text (concatenated).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Element { name: name.into(), ..Element::default() }
    }

    /// Builder: adds an attribute.
    pub fn with(mut self, key: &str, value: impl ToString) -> Self {
        self.set(key, value);
        self
    }

    /// Builder: adds a child.
    pub fn child_with(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    /// Sets (or replaces) an attribute.
    pub fn set(&mut self, key: &str, value: impl ToString) {
        let value = value.to_string();
        match self.attrs.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.attrs.push((key.to_string(), value)),
        }
    }

    pub fn push(&mut self, child: Element) {
        self.children.push(child);
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// A numeric attribute (`inf`/`-inf` as DAWproject writes them; NaN refused).
    pub fn num(&self, key: &str) -> Option<f64> {
        parse_num(self.attr(key)?)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.attr(key)?.trim().parse().ok()
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.attr(key)?.trim() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }

    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut Element> {
        self.children.iter_mut().find(|c| c.name == name)
    }

    /// Every child named `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The document with this element as its root, with an XML declaration.
    pub fn to_document(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
        self.write(&mut out, 0);
        out
    }

    /// This element alone, without a declaration (one line per element, indented).
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        for _ in 0..depth.min(64) {
            out.push_str("  ");
        }
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            escape_into(out, v, true);
            out.push('"');
        }
        if self.children.is_empty() && self.text.is_empty() {
            out.push_str("/>\n");
            return;
        }
        out.push('>');
        if self.children.is_empty() {
            escape_into(out, &self.text, false);
        } else {
            out.push('\n');
            if !self.text.is_empty() {
                escape_into(out, &self.text, false);
                out.push('\n');
            }
            for c in &self.children {
                c.write(out, depth + 1);
            }
            for _ in 0..depth.min(64) {
                out.push_str("  ");
            }
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push_str(">\n");
    }

    /// Parses a document; the root element is returned.
    pub fn parse(text: &str) -> Result<Element, String> {
        let mut reader = quick_xml::Reader::from_str(text);
        reader.config_mut().check_end_names = true;
        let mut stack: Vec<Element> = Vec::new();
        let mut root: Option<Element> = None;
        let mut count = 0usize;
        loop {
            let ev = reader.read_event().map_err(|e| format!("XML error at byte {}: {e}", reader.buffer_position()))?;
            match ev {
                Event::Start(e) | Event::Empty(e) if root.is_some() => {
                    let _ = e;
                    return Err("XML has content after its root element".into());
                }
                Event::Start(e) => {
                    count += 1;
                    if count > MAX_ELEMENTS || stack.len() >= MAX_DEPTH {
                        return Err("XML is too large or nested too deeply".into());
                    }
                    stack.push(open(&e)?);
                }
                Event::Empty(e) => {
                    count += 1;
                    if count > MAX_ELEMENTS || stack.len() >= MAX_DEPTH {
                        return Err("XML is too large or nested too deeply".into());
                    }
                    let el = open(&e)?;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(el),
                        None => root = Some(el),
                    }
                }
                Event::End(_) => {
                    let mut el = stack.pop().ok_or("XML closes an element it never opened")?;
                    // Layout whitespace around children and the text goes; inner spaces stay.
                    if el.text.starts_with(char::is_whitespace) || el.text.ends_with(char::is_whitespace) {
                        el.text = el.text.trim().to_string();
                    }
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(el),
                        None => root = Some(el),
                    }
                }
                Event::Text(t) => {
                    if let Some(top) = stack.last_mut() {
                        let s = t.decode().map_err(|e| format!("XML text: {e}"))?;
                        top.text.push_str(&s);
                    }
                }
                Event::CData(t) => {
                    if let Some(top) = stack.last_mut() {
                        top.text.push_str(&String::from_utf8_lossy(&t));
                    }
                }
                Event::GeneralRef(r) => {
                    if let Some(top) = stack.last_mut() {
                        if let Ok(Some(c)) = r.resolve_char_ref() {
                            top.text.push(c);
                        } else {
                            match &*r.decode().map_err(|e| format!("XML entity: {e}"))? {
                                "lt" => top.text.push('<'),
                                "gt" => top.text.push('>'),
                                "amp" => top.text.push('&'),
                                "quot" => top.text.push('"'),
                                "apos" => top.text.push('\''),
                                other => return Err(format!("XML uses an unknown entity &{other};")),
                            }
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if !stack.is_empty() {
            return Err("XML ends inside an element".into());
        }
        root.ok_or_else(|| "XML has no root element".into())
    }
}

fn open(e: &quick_xml::events::BytesStart<'_>) -> Result<Element, String> {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut el = Element::new(name);
    for a in e.attributes() {
        let a = a.map_err(|err| format!("XML attribute: {err}"))?;
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let value = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|err| format!("XML attribute `{key}`: {err}"))?.into_owned();
        el.attrs.push((key, value));
    }
    Ok(el)
}

/// A DAWproject number: decimal, or `inf`/`-inf`. NaN and garbage are `None`.
pub fn parse_num(s: &str) -> Option<f64> {
    let s = s.trim();
    let v = match s {
        "inf" | "+inf" | "Infinity" => f64::INFINITY,
        "-inf" | "-Infinity" => f64::NEG_INFINITY,
        _ => s.parse::<f64>().ok()?,
    };
    (!v.is_nan()).then_some(v)
}

/// A number as DAWproject writes it: shortest round-trip decimal, `inf`/`-inf`.
pub fn fmt_num(v: f64) -> String {
    if v.is_nan() {
        "0".into()
    } else if v == f64::INFINITY {
        "inf".into()
    } else if v == f64::NEG_INFINITY {
        "-inf".into()
    } else if v == v.trunc() && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

fn escape_into(out: &mut String, s: &str, attr: bool) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' if attr => out.push_str("&quot;"),
            '\n' if attr => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' if attr => out.push_str("&#9;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_writes_and_parses_back() {
        let doc = r#"<?xml version="1.0"?><Project version="1.0"><Application name="A &amp; B" version="1"/><Note>x &lt; y&#33;</Note></Project>"#;
        let p = Element::parse(doc).unwrap();
        assert_eq!(p.name, "Project");
        assert_eq!(p.child("Application").and_then(|a| a.attr("name")), Some("A & B"));
        assert_eq!(p.child("Note").map(|n| n.text.as_str()), Some("x < y!"));
        let back = Element::parse(&p.to_document()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn malformed_documents_are_errors() {
        for bad in ["", "<a>", "<a></b>", "</a>", "<a/><b/>", "<a x=\"1></a>", "<a>&nope;</a>", "\u{0}<<<"] {
            assert!(Element::parse(bad).is_err(), "{bad:?}");
        }
        let deep = "<a>".repeat(MAX_DEPTH + 2);
        assert!(Element::parse(&deep).is_err());
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_num("inf"), Some(f64::INFINITY));
        assert_eq!(parse_num(" 1.5 "), Some(1.5));
        assert_eq!(parse_num("NaN"), None);
        assert_eq!(parse_num("x"), None);
        assert_eq!(fmt_num(2.0), "2.0");
        assert_eq!(fmt_num(0.1), "0.1");
        assert_eq!(parse_num(&fmt_num(1.0 / 3.0)), Some(1.0 / 3.0));
    }
}
