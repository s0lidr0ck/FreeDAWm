//! A minimal AAF writer for tests (feature `test-writer`): objects with data, strong, weak,
//! vector, set and stream properties stored the way the reader expects, and [`session`], which
//! builds a small composition (tracks of clips, fillers and transitions, clip gain, a start
//! timecode, a marker) over embedded or external PCM media.

use crate::aaf::pid;
use crate::cfb::writer::{Node, write as write_cfb};
use crate::object::{Auid, auid};

/// A property value to write.
#[derive(Debug, Clone)]
pub enum Val {
    Data(Vec<u8>),
    Strong(Obj),
    Vector(Vec<Obj>),
    Set(Vec<Obj>),
    /// A weak reference to a definition by its identification.
    Weak(Auid),
    Stream(Vec<u8>),
}

/// An object to write.
#[derive(Debug, Clone)]
pub struct Obj {
    pub class: Auid,
    pub props: Vec<(u16, Val)>,
}

impl Obj {
    pub fn new(class: Auid) -> Obj {
        Obj { class, props: Vec::new() }
    }
    pub fn with(mut self, pid: u16, v: Val) -> Obj {
        self.props.push((pid, v));
        self
    }
    pub fn data(self, pid: u16, b: impl Into<Vec<u8>>) -> Obj {
        self.with(pid, Val::Data(b.into()))
    }
    pub fn i64(self, pid: u16, v: i64) -> Obj {
        self.data(pid, v.to_le_bytes().to_vec())
    }
    pub fn u32(self, pid: u16, v: u32) -> Obj {
        self.data(pid, v.to_le_bytes().to_vec())
    }
    pub fn rational(self, pid: u16, n: i32, d: i32) -> Obj {
        let mut b = n.to_le_bytes().to_vec();
        b.extend(d.to_le_bytes());
        self.data(pid, b)
    }
    pub fn string(self, pid: u16, s: &str) -> Obj {
        self.data(pid, utf16z(s))
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// The built-in class `{0d010101-0101-XX00-060e-2b3402060101}`.
pub const fn class(x: u8) -> Auid {
    auid(0x0d01_0101, 0x0101, (x as u16) << 8, [0x06, 0x0e, 0x2b, 0x34, 0x02, 0x06, 0x01, 0x01])
}

fn node(o: &Obj, name: String) -> Node {
    let mut children = Vec::new();
    let mut table: Vec<(u16, u16, Vec<u8>)> = Vec::new();
    for (pid, v) in &o.props {
        let sub = format!("p{pid:x}");
        match v {
            Val::Data(b) => table.push((*pid, crate::object::SF_DATA, b.clone())),
            Val::Strong(c) => {
                table.push((*pid, crate::object::SF_STRONG_REF, utf16z(&sub)));
                children.push(node(c, sub));
            }
            Val::Vector(list) | Val::Set(list) => {
                let set = matches!(v, Val::Set(_));
                table.push((*pid, if set { crate::object::SF_STRONG_REF_SET } else { crate::object::SF_STRONG_REF_VECTOR }, utf16z(&sub)));
                let n = list.len() as u32;
                let mut index = Vec::new();
                index.extend(n.to_le_bytes());
                index.extend(n.to_le_bytes());
                index.extend(u32::MAX.to_le_bytes());
                if set {
                    index.extend(0x1b01u16.to_le_bytes());
                    index.push(4);
                }
                for k in 0..n {
                    index.extend(k.to_le_bytes());
                    if set {
                        index.extend(1u32.to_le_bytes());
                        index.extend(k.to_le_bytes());
                    }
                }
                children.push(Node::Stream { name: format!("{sub} index"), data: index });
                for (k, c) in list.iter().enumerate() {
                    children.push(node(c, format!("{sub}{{{k:x}}}")));
                }
            }
            Val::Weak(a) => {
                let mut b = 0u16.to_le_bytes().to_vec();
                b.extend(0x1b01u16.to_le_bytes());
                b.push(16);
                b.extend(a);
                table.push((*pid, crate::object::SF_WEAK_REF, b));
            }
            Val::Stream(data) => {
                let mut b = vec![0x55];
                b.extend(utf16z(&sub));
                table.push((*pid, crate::object::SF_DATA_STREAM, b));
                children.push(Node::Stream { name: sub, data: data.clone() });
            }
        }
    }
    let mut props = vec![0x4C, 0x20];
    props.extend((table.len() as u16).to_le_bytes());
    for (pid, form, v) in &table {
        props.extend(pid.to_le_bytes());
        props.extend(form.to_le_bytes());
        props.extend((v.len() as u16).to_le_bytes());
    }
    for (_, _, v) in &table {
        props.extend(v);
    }
    children.insert(0, Node::Stream { name: "properties".into(), data: props });
    Node::Storage { name, clsid: o.class, children }
}

/// A compound file whose root object holds `header` (as AAF's root holds the header).
pub fn file(header: Obj, sector_size: usize) -> Vec<u8> {
    let root = Obj::new([0; 16]).with(pid::ROOT_HEADER, Val::Strong(header));
    let Node::Storage { children, .. } = node(&root, "Root Entry".into()) else { return Vec::new() };
    write_cfb([0; 16], &children, sector_size, true)
}

/// The sound data definition (the SMPTE label as the files store it).
pub const SOUND: Auid = [0x02, 0x02, 0x03, 0x01, 0x00, 0x02, 0x00, 0x00, 0x06, 0x0e, 0x2b, 0x34, 0x04, 0x01, 0x01, 0x01];
/// The timecode data definition.
pub const TIMECODE: Auid = [0x01, 0x02, 0x03, 0x01, 0x00, 0x01, 0x00, 0x00, 0x06, 0x0e, 0x2b, 0x34, 0x04, 0x01, 0x01, 0x01];
pub const DESCRIPTIVE: Auid = [0x01, 0x02, 0x03, 0x01, 0x00, 0x10, 0x00, 0x00, 0x06, 0x0e, 0x2b, 0x34, 0x04, 0x01, 0x01, 0x01];
pub const OP_GAIN: Auid = auid(0x9d2e_a894, 0x0968, 0x11d3, [0x8a, 0x38, 0x00, 0x50, 0x04, 0x0e, 0xf7, 0xd2]);
pub const OP_DISSOLVE: Auid = auid(0x0c3b_ea41, 0xfc05, 0x11d2, [0x8a, 0x29, 0x00, 0x50, 0x04, 0x0e, 0xf7, 0xd2]);
pub const OP_COMBINER: Auid = auid(0x6b46_dd7a, 0x132d, 0x4856, [0xab, 0x21, 0x8b, 0x75, 0x1d, 0x84, 0x62, 0xec]);
pub const PARAM_AMPLITUDE: Auid = auid(0xe496_2321, 0x2267, 0x11d3, [0x8a, 0x4c, 0x00, 0x50, 0x04, 0x0e, 0xf7, 0xd2]);

fn def(class_byte: u8, id: Auid, name: &str) -> Obj {
    Obj::new(class(class_byte)).data(pid::DEF_IDENTIFICATION, id.to_vec()).string(pid::DEF_NAME, name)
}

fn rational_value(n: i32, d: i32) -> Vec<u8> {
    let mut b = vec![0x4C, 0x00, 0x01, 0x01, 0x03, 0, 0, 0, 0, 0x06, 0x0e, 0x2b, 0x34, 0x01, 0x04, 0x01, 0x01];
    b.extend(n.to_le_bytes());
    b.extend(d.to_le_bytes());
    b
}

/// A piece of a synthetic track.
#[derive(Debug, Clone)]
pub enum Item {
    /// `len` edit units of media `media` from `start` (edit units), at `gain` (a factor) if set.
    Clip {
        media: usize,
        start: i64,
        len: i64,
        gain: Option<f64>,
    },
    /// Two media played as one stereo clip.
    Stereo {
        left: usize,
        right: usize,
        start: i64,
        len: i64,
    },
    Filler(i64),
    Transition(i64),
}

/// A synthetic medium: 16-bit mono PCM at `rate`, embedded or named by `url`.
#[derive(Debug, Clone)]
pub struct MediaSpec {
    pub name: String,
    pub rate: u32,
    pub samples: Vec<i16>,
    pub url: Option<String>,
}

/// A synthetic composition.
#[derive(Debug, Clone)]
pub struct Spec {
    pub name: String,
    /// The tracks' edit rate.
    pub rate: (i32, i32),
    pub tc_start: Option<i64>,
    pub tracks: Vec<(String, Vec<Item>)>,
    pub media: Vec<MediaSpec>,
    /// Markers: (position in edit units, name).
    pub markers: Vec<(i64, String)>,
}

fn mob_id(k: u8) -> Vec<u8> {
    let mut id = vec![0x06, 0x0a, 0x2b, 0x34, 0x01, 0x01, 0x01, 0x05, 0x01, 0x01, 0x0f, 0x10, 0x13, 0, 0, 0];
    id.extend([k; 16]);
    id
}

fn component(class_byte: u8, dd: Auid, len: i64) -> Obj {
    Obj::new(class(class_byte)).with(pid::COMPONENT_DATA_DEF, Val::Weak(dd)).i64(pid::COMPONENT_LENGTH, len)
}

fn source_clip(mob: u8, slot: u32, start: i64, len: i64) -> Obj {
    component(0x11, SOUND, len).data(pid::SOURCEREF_ID, mob_id(mob)).u32(pid::SOURCEREF_SLOT, slot).i64(pid::SOURCECLIP_START, start)
}

/// The AAF file for `spec`, with `sector_size` 512 or 4096.
pub fn session(spec: &Spec, sector_size: usize) -> Vec<u8> {
    let (num, den) = spec.rate;
    let mut mobs = Vec::new();
    let mut essence = Vec::new();
    // Per medium: a file source mob (k = 100 + i) and a master mob (k = 1 + i).
    for (i, m) in spec.media.iter().enumerate() {
        let (sk, mk) = (100 + i as u8, 1 + i as u8);
        // The file mob's slot counts in samples; the master's in the composition's rate.
        let frames = m.samples.len() as i64;
        let mut desc = Obj::new(class(0x48))
            .rational(pid::FILE_SAMPLE_RATE, m.rate as i32, 1)
            .i64(pid::FILE_LENGTH, frames)
            .u32(pid::SOUND_CHANNELS, 1)
            .u32(pid::SOUND_QUANTIZATION, 16)
            .rational(pid::SOUND_SAMPLING_RATE, m.rate as i32, 1);
        if let Some(u) = &m.url {
            desc = desc.with(pid::DESCRIPTOR_LOCATORS, Val::Vector(vec![Obj::new(class(0x32)).string(pid::NETWORK_LOCATOR_URL, u)]));
        } else {
            let pcm: Vec<u8> = m.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            essence.push(Obj::new(class(0x23)).data(pid::ESSENCE_DATA_MOB, mob_id(sk)).with(pid::ESSENCE_DATA_DATA, Val::Stream(pcm)));
        }
        let file_slot = Obj::new(class(0x3B))
            .u32(pid::SLOT_ID, 1)
            .rational(pid::TIMELINE_SLOT_EDIT_RATE, m.rate as i32, 1)
            .i64(pid::TIMELINE_SLOT_ORIGIN, 0)
            .with(pid::SLOT_SEGMENT, Val::Strong(source_clip(0, 0, 0, frames)));
        mobs.push(
            Obj::new(class(0x37))
                .data(pid::MOB_ID, mob_id(sk))
                .string(pid::MOB_NAME, &format!("{}.wav", m.name))
                .with(pid::MOB_SLOTS, Val::Vector(vec![file_slot]))
                .with(pid::SOURCE_MOB_DESCRIPTOR, Val::Strong(desc)),
        );
        let len_eu = frames * i64::from(num) / (i64::from(m.rate) * i64::from(den));
        let master_slot = Obj::new(class(0x3B))
            .u32(pid::SLOT_ID, 1)
            .data(pid::SLOT_PHYSICAL_TRACK, 1u32.to_le_bytes().to_vec())
            .rational(pid::TIMELINE_SLOT_EDIT_RATE, num, den)
            .i64(pid::TIMELINE_SLOT_ORIGIN, 0)
            .with(pid::SLOT_SEGMENT, Val::Strong(source_clip(sk, 1, 0, len_eu)));
        mobs.push(
            Obj::new(class(0x36)).data(pid::MOB_ID, mob_id(mk)).string(pid::MOB_NAME, &m.name).with(pid::MOB_SLOTS, Val::Vector(vec![master_slot])),
        );
    }
    let mut slots = Vec::new();
    if let Some(tc) = spec.tc_start {
        let total: i64 = 1 << 20;
        let seg = component(0x14, TIMECODE, total)
            .i64(pid::TIMECODE_START, tc)
            .data(pid::TIMECODE_FPS, ((num / den.max(1)) as u16).to_le_bytes().to_vec())
            .data(pid::TIMECODE_DROP, vec![0]);
        slots.push(
            Obj::new(class(0x3B)).u32(pid::SLOT_ID, 1).rational(pid::TIMELINE_SLOT_EDIT_RATE, num, den).with(pid::SLOT_SEGMENT, Val::Strong(seg)),
        );
    }
    for (ti, (name, items)) in spec.tracks.iter().enumerate() {
        let mut comps = Vec::new();
        let mut total = 0i64;
        for it in items {
            match it {
                Item::Clip { media, start, len, gain } => {
                    let sc = source_clip(1 + *media as u8, 1, *start, *len);
                    total += len;
                    comps.push(match gain {
                        Some(g) => {
                            let param = Obj::new(class(0x3D))
                                .data(pid::PARAMETER_DEFINITION, PARAM_AMPLITUDE.to_vec())
                                .data(pid::CONSTANT_VALUE, rational_value((g * 65536.0) as i32, 65536));
                            component(0x0A, SOUND, *len)
                                .with(pid::OPGROUP_OPERATION, Val::Weak(OP_GAIN))
                                .with(pid::OPGROUP_INPUTS, Val::Vector(vec![sc]))
                                .with(pid::OPGROUP_PARAMETERS, Val::Set(vec![param]))
                        }
                        None => sc,
                    });
                }
                Item::Stereo { left, right, start, len } => {
                    total += len;
                    comps.push(component(0x0A, SOUND, *len).with(pid::OPGROUP_OPERATION, Val::Weak(OP_COMBINER)).with(
                        pid::OPGROUP_INPUTS,
                        Val::Vector(vec![source_clip(1 + *left as u8, 1, *start, *len), source_clip(1 + *right as u8, 1, *start, *len)]),
                    ));
                }
                Item::Filler(len) => {
                    total += len;
                    comps.push(component(0x09, SOUND, *len));
                }
                Item::Transition(len) => {
                    total -= len;
                    let op = component(0x0A, SOUND, *len).with(pid::OPGROUP_OPERATION, Val::Weak(OP_DISSOLVE));
                    comps.push(component(0x17, SOUND, *len).with(pid::TRANSITION_OPGROUP, Val::Strong(op)).i64(pid::TRANSITION_CUT_POINT, 0));
                }
            }
        }
        let seq = component(0x0F, SOUND, total.max(0)).with(pid::SEQUENCE_COMPONENTS, Val::Vector(comps));
        slots.push(
            Obj::new(class(0x3B))
                .u32(pid::SLOT_ID, 2 + ti as u32)
                .string(pid::SLOT_NAME, name)
                .rational(pid::TIMELINE_SLOT_EDIT_RATE, num, den)
                .i64(pid::TIMELINE_SLOT_ORIGIN, 0)
                .with(pid::SLOT_SEGMENT, Val::Strong(seq)),
        );
    }
    if !spec.markers.is_empty() {
        let events: Vec<Obj> = spec
            .markers
            .iter()
            .map(|(pos, name)| {
                Obj::new(class(0x08))
                    .with(pid::COMPONENT_DATA_DEF, Val::Weak(DESCRIPTIVE))
                    .i64(pid::EVENT_POSITION, *pos)
                    .string(pid::EVENT_COMMENT, name)
            })
            .collect();
        let seq = Obj::new(class(0x0F)).with(pid::COMPONENT_DATA_DEF, Val::Weak(DESCRIPTIVE)).with(pid::SEQUENCE_COMPONENTS, Val::Vector(events));
        slots.push(
            Obj::new(class(0x39)).u32(pid::SLOT_ID, 1000).rational(pid::EVENT_SLOT_EDIT_RATE, num, den).with(pid::SLOT_SEGMENT, Val::Strong(seq)),
        );
    }
    let usage_top = [0x02, 0x01, 0x01, 0x0d, 0x01, 0x01, 0x00, 0x07, 0x06, 0x0e, 0x2b, 0x34, 0x04, 0x01, 0x01, 0x01];
    mobs.push(
        Obj::new(class(0x35))
            .data(pid::MOB_ID, mob_id(200))
            .string(pid::MOB_NAME, &spec.name)
            .data(pid::MOB_USAGE, usage_top.to_vec())
            .with(pid::MOB_SLOTS, Val::Vector(slots)),
    );
    let dict = Obj::new(class(0x22))
        .with(
            pid::DICT_DATA_DEFS,
            Val::Set(vec![def(0x1B, SOUND, "Sound"), def(0x1B, TIMECODE, "Timecode"), def(0x1B, DESCRIPTIVE, "Descriptive Metadata")]),
        )
        .with(
            pid::DICT_OPERATIONS,
            Val::Set(vec![
                def(0x1C, OP_GAIN, "Mono Audio Gain"),
                def(0x1C, OP_DISSOLVE, "Mono Audio Dissolve"),
                def(0x1C, OP_COMBINER, "Audio Channel Combiner"),
            ]),
        )
        .with(pid::DICT_PARAMETERS, Val::Set(vec![def(0x1D, PARAM_AMPLITUDE, "Amplitude")]));
    let ident = Obj::new(class(0x30))
        .string(pid::IDENT_COMPANY, "FreeDAWm tests")
        .string(pid::IDENT_PRODUCT, "testwrite")
        .string(pid::IDENT_VERSION_STRING, "1");
    let content = Obj::new(class(0x18)).with(pid::CONTENT_MOBS, Val::Set(mobs)).with(pid::CONTENT_ESSENCE, Val::Set(essence));
    let header = Obj::new(class(0x2F))
        .with(pid::HEADER_CONTENT, Val::Strong(content))
        .with(pid::HEADER_DICTIONARY, Val::Strong(dict))
        .with(pid::HEADER_IDENTIFICATIONS, Val::Vector(vec![ident]));
    file(header, sector_size)
}

/// A ramp of `n` samples (a recognisable test signal).
pub fn ramp(n: usize) -> Vec<i16> {
    (0..n).map(|i| ((i % 2000) as i32 * 16 - 16000) as i16).collect()
}
