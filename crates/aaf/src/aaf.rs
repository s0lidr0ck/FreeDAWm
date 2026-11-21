//! The AAF object model, as far as an audio timeline needs it: the header, the dictionary and
//! the meta dictionary (class names and extension properties), and the mobs walked from a top
//! composition through its slots, sequences, clips, fillers, transitions and operation groups to
//! master mobs, source mobs and their essence (embedded essence data or a file locator).
//!
//! Class and property identities follow the AAF Object Specification v1.1; class names are read
//! from the file's own meta dictionary where it has one (so extended classes are recognised by
//! their base class), with the specification's built-in identifiers as the fallback.

use crate::Error;
use crate::object::{Auid, Object, Store, auid_text, utf16};
use std::collections::HashMap;

/// A mob id (SMPTE UMID, 32 bytes).
pub type MobId = [u8; 32];

/// Property ids of the AAF Object Specification used here.
pub mod pid {
    pub const ROOT_META_DICTIONARY: u16 = 0x0001;
    pub const ROOT_HEADER: u16 = 0x0002;
    pub const META_CLASSES: u16 = 0x0003;
    pub const META_IDENTIFICATION: u16 = 0x0005;
    pub const META_NAME: u16 = 0x0006;
    pub const CLASS_PARENT: u16 = 0x0008;
    pub const CLASS_PROPERTIES: u16 = 0x0009;
    pub const PROPERTY_LOCAL_ID: u16 = 0x000D;

    pub const COMPONENT_DATA_DEF: u16 = 0x0201;
    pub const COMPONENT_LENGTH: u16 = 0x0202;
    pub const COMPONENT_USER_COMMENTS: u16 = 0x0204;
    pub const COMPONENT_ATTRIBUTES: u16 = 0x0205;
    pub const OPGROUP_OPERATION: u16 = 0x0B01;
    pub const OPGROUP_INPUTS: u16 = 0x0B02;
    pub const OPGROUP_PARAMETERS: u16 = 0x0B03;
    pub const SEQUENCE_COMPONENTS: u16 = 0x1001;
    pub const SELECTOR_SELECTED: u16 = 0x0F01;
    pub const SELECTOR_ALTERNATES: u16 = 0x0F02;
    pub const SOURCEREF_ID: u16 = 0x1101;
    pub const SOURCEREF_SLOT: u16 = 0x1102;
    pub const SOURCECLIP_START: u16 = 0x1201;
    pub const SOURCECLIP_FADE_IN_LEN: u16 = 0x1202;
    pub const SOURCECLIP_FADE_IN_TYPE: u16 = 0x1203;
    pub const SOURCECLIP_FADE_OUT_LEN: u16 = 0x1204;
    pub const SOURCECLIP_FADE_OUT_TYPE: u16 = 0x1205;
    pub const TIMECODE_START: u16 = 0x1501;
    pub const TIMECODE_FPS: u16 = 0x1502;
    pub const TIMECODE_DROP: u16 = 0x1503;
    pub const TRANSITION_OPGROUP: u16 = 0x1801;
    pub const TRANSITION_CUT_POINT: u16 = 0x1802;
    pub const CONTENT_MOBS: u16 = 0x1901;
    pub const CONTENT_ESSENCE: u16 = 0x1902;
    pub const CONTROL_POINT_VALUE: u16 = 0x1A02;
    pub const CONTROL_POINT_TIME: u16 = 0x1A03;
    pub const DEF_IDENTIFICATION: u16 = 0x1B01;
    pub const DEF_NAME: u16 = 0x1B02;
    pub const DICT_OPERATIONS: u16 = 0x2603;
    pub const DICT_PARAMETERS: u16 = 0x2604;
    pub const DICT_DATA_DEFS: u16 = 0x2605;
    pub const DICT_INTERPOLATIONS: u16 = 0x2609;
    pub const ESSENCE_DATA_MOB: u16 = 0x2701;
    pub const ESSENCE_DATA_DATA: u16 = 0x2702;
    pub const DESCRIPTOR_LOCATORS: u16 = 0x2F01;
    pub const FILE_SAMPLE_RATE: u16 = 0x3001;
    pub const FILE_LENGTH: u16 = 0x3002;
    pub const FILE_CONTAINER: u16 = 0x3004;
    pub const AIFC_SUMMARY: u16 = 0x3101;
    pub const WAVE_SUMMARY: u16 = 0x3801;
    pub const HEADER_CONTENT: u16 = 0x3B03;
    pub const HEADER_DICTIONARY: u16 = 0x3B04;
    pub const HEADER_IDENTIFICATIONS: u16 = 0x3B06;
    pub const SOUND_SAMPLING_RATE: u16 = 0x3D03;
    pub const SOUND_CHANNELS: u16 = 0x3D07;
    pub const SOUND_QUANTIZATION: u16 = 0x3D01;
    pub const IDENT_COMPANY: u16 = 0x3C01;
    pub const IDENT_PRODUCT: u16 = 0x3C02;
    pub const IDENT_VERSION_STRING: u16 = 0x3C04;
    pub const NETWORK_LOCATOR_URL: u16 = 0x4001;
    pub const TEXT_LOCATOR_NAME: u16 = 0x4101;
    pub const MOB_ID: u16 = 0x4401;
    pub const MOB_NAME: u16 = 0x4402;
    pub const MOB_SLOTS: u16 = 0x4403;
    pub const MOB_USER_COMMENTS: u16 = 0x4406;
    pub const MOB_USAGE: u16 = 0x4408;
    pub const MOB_ATTRIBUTES: u16 = 0x4409;
    pub const SOURCE_MOB_DESCRIPTOR: u16 = 0x4701;
    pub const SLOT_ID: u16 = 0x4801;
    pub const SLOT_NAME: u16 = 0x4802;
    pub const SLOT_SEGMENT: u16 = 0x4803;
    pub const SLOT_PHYSICAL_TRACK: u16 = 0x4804;
    pub const EVENT_SLOT_EDIT_RATE: u16 = 0x4901;
    pub const TIMELINE_SLOT_EDIT_RATE: u16 = 0x4B01;
    pub const TIMELINE_SLOT_ORIGIN: u16 = 0x4B02;
    pub const PARAMETER_DEFINITION: u16 = 0x4C01;
    pub const CONSTANT_VALUE: u16 = 0x4D01;
    pub const VARYING_INTERPOLATION: u16 = 0x4E01;
    pub const VARYING_POINTS: u16 = 0x4E02;
    pub const TAGGED_NAME: u16 = 0x5001;
    pub const TAGGED_VALUE: u16 = 0x5003;
    pub const EVENT_POSITION: u16 = 0x0601;
    pub const EVENT_COMMENT: u16 = 0x0602;
    pub const DESC_MARKER_SLOTS: u16 = 0x6102;
}

/// The specification's built-in class identifiers `{0d010101-0101-XX00-060e-2b3402060101}`, by
/// their `XX` byte: the fallback when a file has no meta dictionary entry for a class.
const BUILTIN_CLASSES: &[(u8, &str, Option<u8>)] = &[
    (0x02, "Component", None),
    (0x03, "Segment", Some(0x02)),
    (0x04, "EdgeCode", Some(0x03)),
    (0x05, "EssenceGroup", Some(0x03)),
    (0x06, "Event", Some(0x03)),
    (0x07, "GPITrigger", Some(0x06)),
    (0x08, "CommentMarker", Some(0x06)),
    (0x09, "Filler", Some(0x03)),
    (0x0A, "OperationGroup", Some(0x03)),
    (0x0B, "NestedScope", Some(0x03)),
    (0x0C, "Pulldown", Some(0x03)),
    (0x0D, "ScopeReference", Some(0x03)),
    (0x0E, "Selector", Some(0x03)),
    (0x0F, "Sequence", Some(0x03)),
    (0x10, "SourceReference", Some(0x03)),
    (0x11, "SourceClip", Some(0x10)),
    (0x14, "Timecode", Some(0x03)),
    (0x17, "Transition", Some(0x02)),
    (0x19, "ControlPoint", None),
    (0x23, "EssenceData", None),
    (0x24, "EssenceDescriptor", None),
    (0x25, "FileDescriptor", Some(0x24)),
    (0x26, "AIFCDescriptor", Some(0x25)),
    (0x2C, "WAVEDescriptor", Some(0x25)),
    (0x31, "Locator", None),
    (0x32, "NetworkLocator", Some(0x31)),
    (0x33, "TextLocator", Some(0x31)),
    (0x34, "Mob", None),
    (0x35, "CompositionMob", Some(0x34)),
    (0x36, "MasterMob", Some(0x34)),
    (0x37, "SourceMob", Some(0x34)),
    (0x38, "MobSlot", None),
    (0x39, "EventMobSlot", Some(0x38)),
    (0x3A, "StaticMobSlot", Some(0x38)),
    (0x3B, "TimelineMobSlot", Some(0x38)),
    (0x3C, "Parameter", None),
    (0x3D, "ConstantValue", Some(0x3C)),
    (0x3E, "VaryingValue", Some(0x3C)),
    (0x3F, "TaggedValue", None),
    (0x41, "DescriptiveMarker", Some(0x08)),
    (0x42, "SoundDescriptor", Some(0x25)),
    (0x48, "PCMDescriptor", Some(0x42)),
];

/// Deepest class inheritance followed.
const MAX_CLASS_DEPTH: usize = 32;

/// A decoded indirect value (`aafIndirect`: a byte order, a type id, the value).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Rational(i32, i32),
    Text(String),
    Other(Vec<u8>),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Rational(n, d) if *d != 0 => Some(f64::from(*n) / f64::from(*d)),
            _ => None,
        }
    }
}

/// Decodes an indirect value.
pub fn indirect(b: &[u8]) -> Value {
    let (Some(t), Some(v)) = (b.get(1..17), b.get(17..)) else { return Value::Other(b.to_vec()) };
    let le = b.first() != Some(&0x42);
    let int = |n: usize, signed: bool| -> Option<i64> {
        let raw = v.get(..n)?;
        let mut a = [0u8; 8];
        if le {
            a[..n].copy_from_slice(raw);
        } else {
            for (i, x) in raw.iter().rev().enumerate() {
                a[i] = *x;
            }
        }
        let u = u64::from_le_bytes(a);
        Some(if signed && n < 8 && (u >> (n * 8 - 1)) & 1 == 1 { (u | (!0u64 << (n * 8))) as i64 } else { u as i64 })
    };
    if t.get(8..) != Some(&[0x06, 0x0e, 0x2b, 0x34, 0x01, 0x04, 0x01, 0x01][..]) {
        return Value::Other(v.to_vec());
    }
    match (t[0], t[1], t[2], t[3]) {
        (0x00, k @ 0x01..=0x08, 0x01, 0x01) => {
            let n = [1usize, 2, 4, 8][usize::from((k - 1) % 4)];
            int(n, k >= 5).map(Value::Int).unwrap_or(Value::Other(v.to_vec()))
        }
        (0x00, 0x01, 0x01, 0x03) => match (
            int(4, true),
            v.get(4..).and_then(|d| {
                if le { d.get(..4)?.try_into().ok().map(i32::from_le_bytes) } else { d.get(..4)?.try_into().ok().map(i32::from_be_bytes) }
            }),
        ) {
            (Some(n), Some(d)) => Value::Rational(n as i32, d),
            _ => Value::Other(v.to_vec()),
        },
        (0x00, 0x02, 0x10, 0x01) => Value::Text(utf16(v)),
        _ => Value::Other(v.to_vec()),
    }
}

/// An open AAF file.
pub struct Reader<'a> {
    pub store: Store<'a>,
    pub header: Object,
    content: Option<Object>,
    /// Class id → (name, parent class id), from the meta dictionary.
    classes: HashMap<Auid, (String, Option<Auid>)>,
    /// Extension property name → its local property id in this file.
    ext: HashMap<String, u16>,
    /// Dictionary definition id → name (data, operation, parameter, interpolation definitions).
    defs: HashMap<Auid, String>,
    mobs: HashMap<MobId, Object>,
    mob_order: Vec<MobId>,
    essence: HashMap<MobId, Object>,
}

impl<'a> Reader<'a> {
    pub fn open(data: &'a [u8]) -> Result<Reader<'a>, Error> {
        let store = Store::new(data)?;
        let root = store.root()?;
        let header = store.strong(&root, pid::ROOT_HEADER).ok_or_else(|| Error::new("not an AAF file: no header"))?;
        let mut r = Reader {
            store,
            header,
            content: None,
            classes: HashMap::new(),
            ext: HashMap::new(),
            defs: HashMap::new(),
            mobs: HashMap::new(),
            mob_order: Vec::new(),
            essence: HashMap::new(),
        };
        if let Some(meta) = r.store.strong(&root, pid::ROOT_META_DICTIONARY) {
            for c in r.store.strong_list(&meta, pid::META_CLASSES) {
                let (Some(id), Some(name)) = (c.auid(pid::META_IDENTIFICATION), c.string(pid::META_NAME)) else { continue };
                let parent = c.weak_auid(pid::CLASS_PARENT).filter(|p| *p != id);
                for p in r.store.strong_list(&c, pid::CLASS_PROPERTIES) {
                    if let (Some(local), Some(pname)) = (p.u16(pid::PROPERTY_LOCAL_ID), p.string(pid::META_NAME))
                        && local >= 0x8000
                    {
                        r.ext.entry(pname).or_insert(local);
                    }
                }
                r.classes.insert(id, (name, parent));
            }
        }
        if let Some(dict) = r.store.strong(&r.header, pid::HEADER_DICTIONARY) {
            for p in [pid::DICT_DATA_DEFS, pid::DICT_OPERATIONS, pid::DICT_PARAMETERS, pid::DICT_INTERPOLATIONS] {
                for d in r.store.strong_list(&dict, p) {
                    if let (Some(id), Some(name)) = (d.auid(pid::DEF_IDENTIFICATION), d.string(pid::DEF_NAME)) {
                        r.defs.insert(id, name);
                    }
                }
            }
        }
        r.content = r.store.strong(&r.header, pid::HEADER_CONTENT);
        if let Some(content) = &r.content {
            for m in r.store.strong_list(content, pid::CONTENT_MOBS) {
                if let Some(id) = mob_id(&m, pid::MOB_ID)
                    && !r.mobs.contains_key(&id)
                {
                    r.mob_order.push(id);
                    r.mobs.insert(id, m);
                }
            }
            for e in r.store.strong_list(content, pid::CONTENT_ESSENCE) {
                if let Some(id) = mob_id(&e, pid::ESSENCE_DATA_MOB) {
                    r.essence.entry(id).or_insert(e);
                }
            }
        }
        Ok(r)
    }

    /// The class names of `class` from the most derived up.
    pub fn class_chain(&self, class: &Auid) -> Vec<String> {
        let mut out = Vec::new();
        let mut c = Some(*class);
        while let Some(id) = c {
            if out.len() >= MAX_CLASS_DEPTH {
                break;
            }
            if let Some((name, parent)) = self.classes.get(&id) {
                out.push(name.clone());
                c = *parent;
            } else if let Some(x) = builtin_byte(&id) {
                let mut b = BUILTIN_CLASSES.iter().find(|(k, _, _)| *k == x);
                while let Some((_, name, parent)) = b {
                    if out.len() >= MAX_CLASS_DEPTH {
                        break;
                    }
                    out.push((*name).to_string());
                    b = parent.and_then(|p| BUILTIN_CLASSES.iter().find(|(k, _, _)| *k == p));
                }
                break;
            } else {
                break;
            }
        }
        out
    }

    /// Whether `o` is of class `name` or derived from it.
    pub fn is(&self, o: &Object, name: &str) -> bool {
        self.class_chain(&o.class).iter().any(|c| c == name)
    }

    /// The most derived class name of `o`.
    pub fn class_name(&self, o: &Object) -> String {
        self.class_chain(&o.class).into_iter().next().unwrap_or_else(|| auid_text(&o.class))
    }

    /// The local id of an extension property by name.
    pub fn ext_pid(&self, name: &str) -> Option<u16> {
        self.ext.get(name).copied()
    }

    /// The name of the definition a weak reference (or an AUID) names.
    pub fn def_name(&self, id: &Auid) -> Option<&str> {
        self.defs.get(id).map(String::as_str)
    }

    pub fn mob(&self, id: &MobId) -> Option<&Object> {
        self.mobs.get(id)
    }

    /// Every mob in the file, in its stored order.
    pub fn mobs(&self) -> impl Iterator<Item = &Object> {
        self.mob_order.iter().filter_map(|id| self.mobs.get(id))
    }

    pub fn essence_data(&self, id: &MobId) -> Option<&Object> {
        self.essence.get(id)
    }

    /// The identification of the application that wrote the file last (company, product,
    /// version text).
    pub fn writer(&self) -> (String, String, String) {
        let ids = self.store.strong_list(&self.header, pid::HEADER_IDENTIFICATIONS);
        let Some(i) = ids.last() else { return Default::default() };
        (
            i.string(pid::IDENT_COMPANY).unwrap_or_default(),
            i.string(pid::IDENT_PRODUCT).unwrap_or_default(),
            i.string(pid::IDENT_VERSION_STRING).unwrap_or_default(),
        )
    }

    /// The tagged values of a strong vector of TaggedValue (comments, attributes), by name.
    pub fn tagged(&self, of: &Object, pid: u16) -> Vec<(String, Value)> {
        self.store
            .strong_list(of, pid)
            .into_iter()
            .filter_map(|t| Some((t.string(pid::TAGGED_NAME)?, indirect(t.data(pid::TAGGED_VALUE).unwrap_or(&[])))))
            .collect()
    }

    /// An outline of an object tree (class names and key properties), for diagnostics.
    pub fn outline(&self, o: &Object, depth: usize, out: &mut String) {
        if depth > 40 || out.len() > 4 << 20 {
            return;
        }
        let pad = "  ".repeat(depth);
        let mut line = format!("{pad}{}", self.class_name(o));
        if let Some(n) = o.string(pid::MOB_NAME) {
            line.push_str(&format!(" name={n:?}"));
        }
        if let Some(n) = o.string(pid::SLOT_NAME) {
            line.push_str(&format!(" slotname={n:?}"));
        }
        for (k, label) in [
            (pid::SLOT_ID, "id"),
            (pid::SLOT_PHYSICAL_TRACK, "phys"),
            (pid::SOURCEREF_SLOT, "srcslot"),
            (pid::SOUND_CHANNELS, "ch"),
            (pid::SOUND_QUANTIZATION, "bits"),
        ] {
            if let Some(v) = o.u32(k) {
                line.push_str(&format!(" {label}={v}"));
            }
        }
        for (k, label) in [
            (pid::COMPONENT_LENGTH, "len"),
            (pid::SOURCECLIP_START, "start"),
            (pid::TRANSITION_CUT_POINT, "cut"),
            (pid::TIMECODE_START, "tc"),
            (pid::FILE_LENGTH, "flen"),
            (pid::TIMELINE_SLOT_ORIGIN, "origin"),
            (pid::EVENT_POSITION, "pos"),
        ] {
            if let Some(v) = o.i64(k) {
                line.push_str(&format!(" {label}={v}"));
            }
        }
        for (k, label) in [
            (pid::TIMELINE_SLOT_EDIT_RATE, "rate"),
            (pid::FILE_SAMPLE_RATE, "srate"),
            (pid::EVENT_SLOT_EDIT_RATE, "erate"),
            (pid::CONTROL_POINT_TIME, "t"),
        ] {
            if let Some((n, d)) = o.rational(k) {
                line.push_str(&format!(" {label}={n}/{d}"));
            }
        }
        for k in [pid::COMPONENT_DATA_DEF, pid::OPGROUP_OPERATION, pid::VARYING_INTERPOLATION] {
            if let Some(a) = o.weak_auid(k) {
                line.push_str(&format!(" [{}]", self.def_name(&a).map(str::to_string).unwrap_or_else(|| auid_text(&a))));
            }
        }
        if let Some(a) = o.auid(pid::PARAMETER_DEFINITION) {
            line.push_str(&format!(" param={}", self.def_name(&a).map(str::to_string).unwrap_or_else(|| auid_text(&a))));
        }
        for k in [pid::CONSTANT_VALUE, pid::CONTROL_POINT_VALUE, pid::TAGGED_VALUE] {
            if let Some(v) = o.data(k) {
                line.push_str(&format!(" value={:?}", indirect(v)));
            }
        }
        if let Some(n) = o.string(pid::TAGGED_NAME) {
            line.push_str(&format!(" tag={n:?}"));
        }
        if let Some(n) = o.string(pid::NETWORK_LOCATOR_URL) {
            line.push_str(&format!(" url={n:?}"));
        }
        if let Some(n) = o.string(pid::EVENT_COMMENT) {
            line.push_str(&format!(" comment={n:?}"));
        }
        if let Some(id) = mob_id(o, pid::SOURCEREF_ID) {
            let target = self.mob(&id).map(|m| format!("{}:{:?}", self.class_name(m), m.string(pid::MOB_NAME).unwrap_or_default()));
            line.push_str(&format!(" -> {}", target.unwrap_or_else(|| "(none)".into())));
        }
        for p in &o.props {
            if p.pid >= 0x8000 {
                let name = self.ext.iter().find(|(_, v)| **v == p.pid).map(|(k, _)| k.as_str()).unwrap_or("?");
                let shown = if p.form == crate::object::SF_DATA {
                    format!("{:02x?}", &p.value[..p.value.len().min(24)])
                } else {
                    format!("form {:#x}", p.form)
                };
                line.push_str(&format!(" {name}={shown}"));
            }
        }
        out.push_str(&line);
        out.push('\n');
        for p in &o.props {
            match p.form {
                crate::object::SF_STRONG_REF => {
                    if let Some(c) = self.store.strong(o, p.pid) {
                        self.outline(&c, depth + 1, out);
                    }
                }
                crate::object::SF_STRONG_REF_VECTOR | crate::object::SF_STRONG_REF_SET => {
                    for c in self.store.strong_list(o, p.pid) {
                        self.outline(&c, depth + 1, out);
                    }
                }
                _ => {}
            }
        }
    }
}

fn builtin_byte(id: &Auid) -> Option<u8> {
    (id[..6] == [0x01, 0x01, 0x01, 0x0d, 0x01, 0x01] && id[6] == 0 && id[8..] == [0x06, 0x0e, 0x2b, 0x34, 0x02, 0x06, 0x01, 0x01]).then_some(id[7])
}

/// A mob id property.
pub fn mob_id(o: &Object, pid: u16) -> Option<MobId> {
    o.data(pid)?.get(..32)?.try_into().ok()
}

#[cfg(test)]
mod explore {
    use super::*;

    #[test]
    #[ignore]
    fn outline() {
        for path in std::env::var("AAF_DUMP").unwrap().split(',') {
            let bytes = std::fs::read(path).unwrap();
            let r = Reader::open(&bytes).unwrap();
            let mut out = String::new();
            for m in r.mobs() {
                r.outline(m, 0, &mut out);
            }
            let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
            std::fs::write(format!("{}/{name}.outline", std::env::var("AAF_OUT").unwrap()), out).unwrap();
        }
    }
}
