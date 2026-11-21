//! The AAF stored-object format over a compound file: every object is a storage holding a
//! `properties` stream (a table of `(property id, stored form, length)` then the values), its
//! strongly referenced objects as child storages, and for each set or vector of them an
//! `<name> index` stream naming the element storages `<name>{<key>}`.
//!
//! Written from the AAF Low-Level Container Specification's description of the structured-storage
//! format. Weak references (to dictionary definitions, mostly) are read as their key bytes: the
//! caller resolves them by identification.

use crate::Error;
use crate::cfb::{Cfb, Kind};

/// A property value as stored.
pub const SF_DATA: u16 = 0x82;
pub const SF_DATA_STREAM: u16 = 0x42;
pub const SF_STRONG_REF: u16 = 0x22;
pub const SF_STRONG_REF_VECTOR: u16 = 0x32;
pub const SF_STRONG_REF_SET: u16 = 0x3A;
pub const SF_WEAK_REF: u16 = 0x02;
pub const SF_WEAK_REF_VECTOR: u16 = 0x12;
pub const SF_WEAK_REF_SET: u16 = 0x1A;

/// Largest `properties` stream read (real ones are a few kilobytes).
const MAX_PROPERTIES: u64 = 4 << 20;
/// Largest index stream read.
const MAX_INDEX: u64 = 16 << 20;
/// Most elements in one set or vector.
const MAX_ELEMENTS: usize = 1 << 20;

/// An AUID / class id as 16 bytes in the order the file stores them (little-endian `Data1`,
/// `Data2`, `Data3`, then the eight bytes of `Data4`).
pub type Auid = [u8; 16];

/// Builds an [`Auid`] from its textual parts (`{d1-d2-d3-d4}`).
pub const fn auid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Auid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
}

/// An [`Auid`] as `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`.
pub fn auid_text(a: &Auid) -> String {
    let d1 = u32::from_le_bytes([a[0], a[1], a[2], a[3]]);
    let d2 = u16::from_le_bytes([a[4], a[5]]);
    let d3 = u16::from_le_bytes([a[6], a[7]]);
    format!("{{{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}", a[8], a[9], a[10], a[11], a[12], a[13], a[14], a[15])
}

/// One property of an object.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub pid: u16,
    pub form: u16,
    pub value: Vec<u8>,
}

/// One stored object: the storage it lives in, its class id and its properties.
#[derive(Debug, Clone)]
pub struct Object {
    pub entry: usize,
    pub class: Auid,
    pub props: Vec<Prop>,
}

impl Object {
    pub fn prop(&self, pid: u16) -> Option<&Prop> {
        self.props.iter().find(|p| p.pid == pid)
    }
    /// A data property's bytes.
    pub fn data(&self, pid: u16) -> Option<&[u8]> {
        self.prop(pid).filter(|p| p.form == SF_DATA).map(|p| p.value.as_slice())
    }
    pub fn u8(&self, pid: u16) -> Option<u8> {
        self.data(pid)?.first().copied()
    }
    pub fn u16(&self, pid: u16) -> Option<u16> {
        let d = self.data(pid)?;
        Some(u16::from_le_bytes(d.get(..2)?.try_into().ok()?))
    }
    pub fn u32(&self, pid: u16) -> Option<u32> {
        let d = self.data(pid)?;
        Some(u32::from_le_bytes(d.get(..4)?.try_into().ok()?))
    }
    pub fn i32(&self, pid: u16) -> Option<i32> {
        self.u32(pid).map(|v| v as i32)
    }
    pub fn i64(&self, pid: u16) -> Option<i64> {
        let d = self.data(pid)?;
        match d.len() {
            8.. => Some(i64::from_le_bytes(d.get(..8)?.try_into().ok()?)),
            4 => Some(i64::from(i32::from_le_bytes(d.get(..4)?.try_into().ok()?))),
            _ => None,
        }
    }
    /// A rational (`i32 / i32`).
    pub fn rational(&self, pid: u16) -> Option<(i32, i32)> {
        let d = self.data(pid)?;
        Some((i32::from_le_bytes(d.get(..4)?.try_into().ok()?), i32::from_le_bytes(d.get(4..8)?.try_into().ok()?)))
    }
    pub fn auid(&self, pid: u16) -> Option<Auid> {
        self.data(pid)?.get(..16)?.try_into().ok()
    }
    /// A UTF-16 string property (null-terminated, little-endian).
    pub fn string(&self, pid: u16) -> Option<String> {
        self.data(pid).map(utf16)
    }
    /// A weak reference's key (the referenced definition's identification, for AUID keys).
    pub fn weak_key(&self, pid: u16) -> Option<Vec<u8>> {
        let p = self.prop(pid).filter(|p| p.form == SF_WEAK_REF)?;
        let size = usize::from(*p.value.get(4)?);
        p.value.get(5..5 + size).map(<[u8]>::to_vec)
    }
    pub fn weak_auid(&self, pid: u16) -> Option<Auid> {
        self.weak_key(pid)?.get(..16)?.try_into().ok()
    }
}

/// A null-terminated UTF-16LE string (lossy; stops at the first null).
pub fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// The objects of a compound file, read on demand.
pub struct Store<'a> {
    pub cfb: Cfb<'a>,
}

impl<'a> Store<'a> {
    pub fn new(data: &'a [u8]) -> Result<Store<'a>, Error> {
        Ok(Store { cfb: Cfb::parse(data)? })
    }

    /// The object stored in directory entry `entry`.
    pub fn object(&self, entry: usize) -> Result<Object, Error> {
        let e = self.cfb.entry(entry).ok_or_else(|| Error::new("no such storage"))?;
        if !matches!(e.kind, Kind::Storage | Kind::Root) {
            return Err(Error::new(format!("`{}` is not an object", e.name)));
        }
        let class = e.clsid;
        let props = match self.cfb.child(entry, "properties") {
            Some(p) => parse_properties(&self.cfb.read(p, MAX_PROPERTIES)?)?,
            None => Vec::new(),
        };
        Ok(Object { entry, class, props })
    }

    /// The root object (meta dictionary and header).
    pub fn root(&self) -> Result<Object, Error> {
        self.object(self.cfb.root())
    }

    fn named(&self, of: &Object, name: &str) -> Option<usize> {
        self.cfb.child(of.entry, name)
    }

    /// The object a strong reference property points at.
    pub fn strong(&self, of: &Object, pid: u16) -> Option<Object> {
        let p = of.prop(pid).filter(|p| p.form == SF_STRONG_REF)?;
        let e = self.named(of, &utf16(&p.value))?;
        self.object(e).ok()
    }

    /// The objects of a strong reference vector or set property, in order. Elements that cannot
    /// be read are skipped.
    pub fn strong_list(&self, of: &Object, pid: u16) -> Vec<Object> {
        let Some(p) = of.prop(pid).filter(|p| p.form == SF_STRONG_REF_VECTOR || p.form == SF_STRONG_REF_SET) else { return Vec::new() };
        let name = utf16(&p.value);
        let Some(ix) = self.named(of, &format!("{name} index")) else { return Vec::new() };
        let Ok(index) = self.cfb.read(ix, MAX_INDEX) else { return Vec::new() };
        let keys = if p.form == SF_STRONG_REF_VECTOR { vector_keys(&index) } else { set_keys(&index) };
        // Element storages are `name{key}` with the key in hex; find them by a lowercase match.
        let kids: Vec<(String, usize)> =
            self.cfb.children(of.entry).into_iter().filter_map(|k| self.cfb.entry(k).map(|e| (e.name.to_ascii_lowercase(), k))).collect();
        let mut out = Vec::with_capacity(keys.len().min(4096));
        for key in keys {
            let want = format!("{name}{{{key:x}}}").to_ascii_lowercase();
            if let Some((_, k)) = kids.iter().find(|(n, _)| *n == want)
                && let Ok(o) = self.object(*k)
            {
                out.push(o);
            }
        }
        out
    }

    /// The keys of a weak reference vector or set property (each the referenced object's key).
    pub fn weak_list(&self, of: &Object, pid: u16) -> Vec<Vec<u8>> {
        let Some(p) = of.prop(pid).filter(|p| p.form == SF_WEAK_REF_VECTOR || p.form == SF_WEAK_REF_SET) else { return Vec::new() };
        let name = utf16(&p.value);
        let Some(ix) = self.named(of, &format!("{name} index")) else { return Vec::new() };
        let Ok(b) = self.cfb.read(ix, MAX_INDEX) else { return Vec::new() };
        // count u32, tag u16, key pid u16, key size u8, then the keys.
        let (Some(count), Some(&size)) = (b.get(..4).and_then(|x| x.try_into().ok()).map(u32::from_le_bytes), b.get(8)) else { return Vec::new() };
        let size = usize::from(size);
        if size == 0 {
            return Vec::new();
        }
        b.get(9..).unwrap_or(&[]).chunks_exact(size).take((count as usize).min(MAX_ELEMENTS)).map(<[u8]>::to_vec).collect()
    }

    /// The bytes of a data-stream property's stream, when no longer than `max`.
    pub fn stream(&self, of: &Object, pid: u16, max: u64) -> Result<Vec<u8>, Error> {
        let p = of.prop(pid).filter(|p| p.form == SF_DATA_STREAM).ok_or_else(|| Error::new("no data stream"))?;
        // A byte-order byte, then the stream's name.
        let name = utf16(p.value.get(1..).unwrap_or(&[]));
        let e = self.named(of, &name).ok_or_else(|| Error::new(format!("data stream `{name}` is missing")))?;
        self.cfb.read(e, max)
    }

    /// The size of a data-stream property's stream.
    pub fn stream_len(&self, of: &Object, pid: u16) -> Option<u64> {
        let p = of.prop(pid).filter(|p| p.form == SF_DATA_STREAM)?;
        let name = utf16(p.value.get(1..).unwrap_or(&[]));
        let e = self.named(of, &name)?;
        self.cfb.entry(e).map(|e| e.size)
    }
}

/// The local keys of a strong reference vector's index: count, first free key, last free key,
/// then a key per element.
fn vector_keys(b: &[u8]) -> Vec<u32> {
    let Some(count) = b.get(..4).and_then(|x| x.try_into().ok()).map(u32::from_le_bytes) else { return Vec::new() };
    b.get(12..)
        .unwrap_or(&[])
        .chunks_exact(4)
        .take((count as usize).min(MAX_ELEMENTS))
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// The local keys of a strong reference set's index: count, first free key, last free key, the
/// identification property id (u16), the key size (u8), then per element a local key, a
/// reference count and the key.
fn set_keys(b: &[u8]) -> Vec<u32> {
    let Some(count) = b.get(..4).and_then(|x| x.try_into().ok()).map(u32::from_le_bytes) else { return Vec::new() };
    let Some(&size) = b.get(14) else { return Vec::new() };
    let stride = 8 + usize::from(size);
    b.get(15..)
        .unwrap_or(&[])
        .chunks_exact(stride)
        .take((count as usize).min(MAX_ELEMENTS))
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// A `properties` stream: byte order (`0x4C` little-endian), format version, entry count, then
/// `(pid, stored form, length)` per entry, then the values one after another.
pub fn parse_properties(b: &[u8]) -> Result<Vec<Prop>, Error> {
    let bad = |w: &str| Error::new(format!("unreadable properties stream: {w}"));
    let order = *b.first().ok_or_else(|| bad("empty"))?;
    if order != 0x4C {
        return Err(bad("not little-endian (big-endian AAF files are not read)"));
    }
    let count = usize::from(u16::from_le_bytes([*b.get(2).ok_or_else(|| bad("short"))?, *b.get(3).ok_or_else(|| bad("short"))?]));
    let mut at = 4 + count * 6;
    if at > b.len() {
        return Err(bad("table longer than the stream"));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = b.get(4 + i * 6..10 + i * 6).ok_or_else(|| bad("short table"))?;
        let pid = u16::from_le_bytes([e[0], e[1]]);
        let form = u16::from_le_bytes([e[2], e[3]]);
        let len = usize::from(u16::from_le_bytes([e[4], e[5]]));
        let value = b.get(at..at + len).ok_or_else(|| bad("value past the end"))?.to_vec();
        at += len;
        out.push(Prop { pid, form, value });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auids_print_like_the_specification() {
        let a = auid(0x0d010101, 0x0101, 0x3400, [0x06, 0x0e, 0x2b, 0x34, 0x02, 0x7f, 0x01, 0x01]);
        assert_eq!(auid_text(&a), "{0d010101-0101-3400-060e-2b34027f0101}");
    }

    #[test]
    fn property_streams_parse_and_refuse_garbage() {
        let mut b = vec![0x4C, 0x20, 2, 0];
        b.extend([1, 0x44, 0x82, 0, 4, 0]);
        b.extend([2, 0x44, 0x22, 0, 4, 0]);
        b.extend([1, 2, 3, 4]);
        b.extend([b'A', 0, 0, 0]);
        let p = parse_properties(&b).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].pid, 0x4401);
        assert_eq!(p[0].value, vec![1, 2, 3, 4]);
        assert_eq!(utf16(&p[1].value), "A");
        assert!(parse_properties(&b[..b.len() - 1]).is_err());
        assert!(parse_properties(&[]).is_err());
        assert!(parse_properties(&[0x42, 0, 0, 0]).is_err());
        for cut in 0..b.len() {
            let _ = parse_properties(&b[..cut]);
        }
    }
}
