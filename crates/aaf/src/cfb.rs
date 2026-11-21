//! A reader for Microsoft compound files ("structured storage", the [MS-CFB] binary format), the
//! container AAF files are stored in: a little file system of storages (folders) and streams
//! (files) inside one file, laid out in fixed-size sectors chained through a file allocation table.
//!
//! Written from the published [MS-CFB] specification. Everything is read from a byte slice the
//! caller holds; nothing is trusted: every sector number, chain, size and directory link is
//! checked, chains that loop are refused, and no allocation is larger than the file itself.

use crate::Error;

const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const HEADER_LEN: usize = 512;
/// Sector numbers at and above this are markers, not sectors.
const MAXREGSECT: u32 = 0xFFFF_FFFA;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
/// "No entry" in a directory link.
pub const NOSTREAM: u32 = 0xFFFF_FFFF;
const DIR_ENTRY_LEN: usize = 128;
/// Deepest storage nesting walked (AAF files nest a few dozen levels at most).
const MAX_DEPTH: usize = 512;

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Empty,
    Storage,
    Stream,
    Root,
}

/// One directory entry: a storage or a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    left: u32,
    right: u32,
    child: u32,
    /// The class id a storage was given (AAF: the class of the object stored in it).
    pub clsid: [u8; 16],
    start: u32,
    pub size: u64,
}

/// A parsed compound file over the bytes it was read from.
#[derive(Debug)]
pub struct Cfb<'a> {
    data: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    mini_cutoff: u64,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    entries: Vec<Entry>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn bad(what: impl Into<String>) -> Error {
    Error::new(format!("not a readable compound file: {}", what.into()))
}

impl<'a> Cfb<'a> {
    /// Reads the header, the allocation tables, the directory and the mini stream.
    pub fn parse(data: &'a [u8]) -> Result<Cfb<'a>, Error> {
        if data.len() < HEADER_LEN || data.get(..8) != Some(&SIGNATURE[..]) {
            return Err(bad("no compound-file signature"));
        }
        let h = |at| u16_at(data, at).ok_or_else(|| bad("short header"));
        let w = |at| u32_at(data, at).ok_or_else(|| bad("short header"));
        if h(28)? != 0xFFFE {
            return Err(bad("byte order mark is not little-endian"));
        }
        let major = h(26)?;
        let shift = h(30)?;
        let sector_size = match (major, shift) {
            (3, 9) => 512,
            (4, 12) => 4096,
            // Some writers pair the shift with the other version number; the shift is what counts.
            (_, 9) => 512,
            (_, 12) => 4096,
            _ => return Err(bad(format!("sector shift {shift} (version {major})"))),
        };
        let mini_shift = h(32)?;
        if mini_shift != 6 {
            return Err(bad(format!("mini sector shift {mini_shift}")));
        }
        let n_fat = w(44)? as usize;
        let first_dir = w(48)?;
        let mini_cutoff = u64::from(w(56)?);
        let first_mini_fat = w(60)?;
        let n_mini_fat = w(64)? as usize;
        let first_difat = w(68)?;
        let n_difat = w(72)? as usize;
        let sectors = data.len().saturating_sub(sector_size) / sector_size + 1;
        if n_fat > sectors || n_difat > sectors || n_mini_fat > sectors {
            return Err(bad("allocation table larger than the file"));
        }
        let mut cfb = Cfb {
            data,
            sector_size,
            mini_sector_size: 64,
            mini_cutoff,
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
            entries: Vec::new(),
        };
        // The DIFAT: 109 entries in the header, then a chain of DIFAT sectors.
        let mut fat_sectors: Vec<u32> = Vec::with_capacity(n_fat.min(sectors));
        for i in 0..109 {
            let s = w(76 + i * 4)?;
            if s <= MAXREGSECT && fat_sectors.len() < n_fat {
                fat_sectors.push(s);
            }
        }
        let per = sector_size / 4 - 1;
        let mut next = first_difat;
        let mut seen = 0usize;
        while next <= MAXREGSECT && fat_sectors.len() < n_fat {
            seen += 1;
            if seen > n_difat.max(1) + 1 || seen > sectors {
                return Err(bad("DIFAT chain loops"));
            }
            let sec = cfb.sector(next)?;
            for i in 0..per {
                let s = u32_at(sec, i * 4).unwrap_or(FREESECT);
                if s <= MAXREGSECT && fat_sectors.len() < n_fat {
                    fat_sectors.push(s);
                }
            }
            next = u32_at(sec, per * 4).unwrap_or(ENDOFCHAIN);
        }
        let mut fat = Vec::with_capacity(fat_sectors.len().saturating_mul(sector_size / 4));
        for s in fat_sectors {
            let sec = cfb.sector(s)?;
            fat.extend(sec.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
        }
        cfb.fat = fat;
        // The directory.
        let dir = cfb.chain_bytes(first_dir, None)?;
        let mut entries = Vec::with_capacity(dir.len() / DIR_ENTRY_LEN);
        for e in dir.chunks_exact(DIR_ENTRY_LEN) {
            entries.push(parse_entry(e, sector_size == 512));
        }
        if entries.first().map(|e| e.kind) != Some(Kind::Root) {
            return Err(bad("the first directory entry is not the root"));
        }
        cfb.entries = entries;
        // The mini FAT and the mini stream (held by the root entry).
        if n_mini_fat > 0 && first_mini_fat <= MAXREGSECT {
            let b = cfb.chain_bytes(first_mini_fat, None)?;
            cfb.mini_fat = b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        }
        let root = cfb.entries[0].clone();
        if root.start <= MAXREGSECT && root.size > 0 {
            cfb.mini_stream = cfb.chain_bytes(root.start, Some(root.size))?;
        }
        Ok(cfb)
    }

    /// The bytes of regular sector `n`.
    fn sector(&self, n: u32) -> Result<&'a [u8], Error> {
        if n > MAXREGSECT {
            return Err(bad(format!("sector number {n:#x}")));
        }
        let at = (n as usize).checked_add(1).and_then(|k| k.checked_mul(self.sector_size)).ok_or_else(|| bad("sector out of range"))?;
        // The last sector of a file may be cut short; what is there is read.
        let data: &'a [u8] = self.data;
        let end = at.saturating_add(self.sector_size).min(data.len());
        data.get(at..end).filter(|s| !s.is_empty()).ok_or_else(|| bad(format!("sector {n} is past the end of the file")))
    }

    /// The bytes of the regular-sector chain starting at `start`, cut to `size` when given.
    fn chain_bytes(&self, start: u32, size: Option<u64>) -> Result<Vec<u8>, Error> {
        let limit = self.data.len() as u64;
        let want = size.unwrap_or(limit).min(limit) as usize;
        let mut out = Vec::with_capacity(want.min(self.data.len()));
        let mut s = start;
        let mut seen = vec![false; self.fat.len()];
        while s <= MAXREGSECT {
            match seen.get_mut(s as usize) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err(bad("sector chain loops")),
                None => return Err(bad(format!("sector {s} has no FAT entry"))),
            }
            out.extend_from_slice(self.sector(s)?);
            if size.is_some() && out.len() >= want {
                break;
            }
            s = *self.fat.get(s as usize).ok_or_else(|| bad(format!("sector {s} has no FAT entry")))?;
        }
        if let Some(n) = size {
            if (out.len() as u64) < n {
                return Err(bad(format!("stream is {} bytes short", n - out.len() as u64)));
            }
            out.truncate(n as usize);
        }
        Ok(out)
    }

    /// The bytes of the mini-sector chain starting at `start`, `size` long.
    fn mini_chain_bytes(&self, start: u32, size: u64) -> Result<Vec<u8>, Error> {
        let want = size.min(self.mini_stream.len() as u64) as usize;
        if (want as u64) < size {
            return Err(bad("mini stream shorter than a stream in it"));
        }
        let mut out = Vec::with_capacity(want);
        let mut s = start;
        let mut seen = vec![false; self.mini_fat.len()];
        while s <= MAXREGSECT && out.len() < want {
            match seen.get_mut(s as usize) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err(bad("mini sector chain loops")),
                None => return Err(bad(format!("mini sector {s} has no entry"))),
            }
            let at = (s as usize).checked_mul(self.mini_sector_size).ok_or_else(|| bad("mini sector out of range"))?;
            let end = at.saturating_add(self.mini_sector_size).min(self.mini_stream.len());
            let b = self.mini_stream.get(at..end).filter(|b| !b.is_empty()).ok_or_else(|| bad(format!("mini sector {s} out of range")))?;
            out.extend_from_slice(b);
            s = *self.mini_fat.get(s as usize).ok_or_else(|| bad(format!("mini sector {s} has no entry")))?;
        }
        if out.len() < want {
            return Err(bad("stream is short"));
        }
        out.truncate(want);
        Ok(out)
    }

    /// The directory entries (index 0 is the root).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, i: usize) -> Option<&Entry> {
        self.entries.get(i)
    }

    /// The root storage's index (always 0).
    pub fn root(&self) -> usize {
        0
    }

    /// The entries directly inside storage `i`, in name order (the red-black tree walked in
    /// order). Links that loop or point outside the directory are ignored.
    pub fn children(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(e) = self.entries.get(i) else { return out };
        if !matches!(e.kind, Kind::Storage | Kind::Root) {
            return out;
        }
        let mut seen = vec![false; self.entries.len()];
        let mut stack: Vec<(u32, bool)> = vec![(e.child, false)];
        while let Some((n, expanded)) = stack.pop() {
            if n == NOSTREAM {
                continue;
            }
            let Some(c) = self.entries.get(n as usize) else { continue };
            if expanded {
                out.push(n as usize);
                continue;
            }
            if seen[n as usize] || stack.len() > MAX_DEPTH * 64 {
                continue;
            }
            seen[n as usize] = true;
            stack.push((c.right, false));
            stack.push((n, true));
            stack.push((c.left, false));
        }
        out.retain(|&k| k != i && self.entries[k].kind != Kind::Empty);
        out
    }

    /// The child of storage `i` called `name`.
    pub fn child(&self, i: usize, name: &str) -> Option<usize> {
        self.children(i).into_iter().find(|&k| self.entries[k].name == name)
    }

    /// The whole of stream `i`, when it is no longer than `max` bytes.
    pub fn read(&self, i: usize, max: u64) -> Result<Vec<u8>, Error> {
        let e = self.entries.get(i).ok_or_else(|| bad("no such entry"))?;
        if e.kind != Kind::Stream {
            return Err(bad(format!("`{}` is not a stream", e.name)));
        }
        if e.size > max {
            return Err(Error::new(format!("stream `{}` is too large ({} bytes; at most {max})", e.name, e.size)));
        }
        if e.size == 0 {
            return Ok(Vec::new());
        }
        if e.size < self.mini_cutoff { self.mini_chain_bytes(e.start, e.size) } else { self.chain_bytes(e.start, Some(e.size)) }
    }
}

fn parse_entry(b: &[u8], v3: bool) -> Entry {
    let name_len = usize::from(u16_at(b, 64).unwrap_or(0)).min(64);
    let units: Vec<u16> = b.get(..name_len).unwrap_or(&[]).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let units: Vec<u16> = units.into_iter().take_while(|&u| u != 0).collect();
    let name = String::from_utf16_lossy(&units);
    let kind = match b.get(66) {
        Some(1) => Kind::Storage,
        Some(2) => Kind::Stream,
        Some(5) => Kind::Root,
        _ => Kind::Empty,
    };
    let mut clsid = [0u8; 16];
    if let Some(c) = b.get(80..96) {
        clsid.copy_from_slice(c);
    }
    let mut size = u64_at(b, 120).unwrap_or(0);
    if v3 {
        size &= 0xFFFF_FFFF;
    }
    Entry {
        name,
        kind,
        left: u32_at(b, 68).unwrap_or(NOSTREAM),
        right: u32_at(b, 72).unwrap_or(NOSTREAM),
        child: u32_at(b, 76).unwrap_or(NOSTREAM),
        clsid,
        start: u32_at(b, 116).unwrap_or(ENDOFCHAIN),
        size,
    }
}

/// A minimal compound-file writer for tests: every stream in regular sectors when
/// `mini == false`, or short streams in the mini stream; the directory as a right-leaning list.
#[cfg(any(test, feature = "test-writer"))]
pub mod writer {
    /// A node to write: a storage with children or a stream with bytes.
    #[derive(Debug, Clone)]
    pub enum Node {
        Storage { name: String, clsid: [u8; 16], children: Vec<Node> },
        Stream { name: String, data: Vec<u8> },
    }

    struct Flat {
        name: String,
        kind: u8,
        clsid: [u8; 16],
        child: u32,
        right: u32,
        data: Vec<u8>,
    }

    fn flatten(nodes: &[Node], out: &mut Vec<Flat>) -> u32 {
        // Siblings chained through `right`, so the in-order walk lists them in order.
        let mut first = super::NOSTREAM;
        let mut prev: Option<usize> = None;
        for n in nodes {
            let idx = out.len();
            match n {
                Node::Storage { name, clsid, children } => {
                    out.push(Flat { name: name.clone(), kind: 1, clsid: *clsid, child: super::NOSTREAM, right: super::NOSTREAM, data: Vec::new() });
                    let c = flatten(children, out);
                    out[idx].child = c;
                }
                Node::Stream { name, data } => {
                    out.push(Flat { name: name.clone(), kind: 2, clsid: [0; 16], child: super::NOSTREAM, right: super::NOSTREAM, data: data.clone() })
                }
            }
            match prev {
                Some(p) => out[p].right = idx as u32,
                None => first = idx as u32,
            }
            prev = Some(idx);
        }
        first
    }

    /// The compound file holding `root`'s children, with `sector_size` 512 or 4096.
    pub fn write(root_clsid: [u8; 16], children: &[Node], sector_size: usize, use_mini: bool) -> Vec<u8> {
        let mut flat =
            vec![Flat { name: "Root Entry".into(), kind: 5, clsid: root_clsid, child: super::NOSTREAM, right: super::NOSTREAM, data: Vec::new() }];
        let c = flatten(children, &mut flat);
        flat[0].child = c;
        let per_fat = sector_size / 4;
        // Lay out: streams, mini stream, mini FAT, directory, then FAT sectors.
        let mut sectors: Vec<Vec<u8>> = Vec::new();
        let mut fat: Vec<u32> = Vec::new();
        let chain = |bytes: &[u8], sectors: &mut Vec<Vec<u8>>, fat: &mut Vec<u32>| -> u32 {
            if bytes.is_empty() {
                return 0xFFFF_FFFE;
            }
            let start = sectors.len() as u32;
            let n = bytes.len().div_ceil(sector_size);
            for i in 0..n {
                let mut s = bytes[i * sector_size..((i + 1) * sector_size).min(bytes.len())].to_vec();
                s.resize(sector_size, 0);
                sectors.push(s);
                fat.push(if i + 1 == n { 0xFFFF_FFFE } else { start + i as u32 + 1 });
            }
            start
        };
        let mut starts = vec![0xFFFF_FFFEu32; flat.len()];
        let mut mini = Vec::new();
        let mut mini_fat: Vec<u32> = Vec::new();
        for (i, f) in flat.iter().enumerate() {
            if f.kind != 2 || f.data.is_empty() {
                continue;
            }
            if use_mini && f.data.len() < 4096 {
                let start = mini_fat.len() as u32;
                let n = f.data.len().div_ceil(64);
                for k in 0..n {
                    let mut s = f.data[k * 64..((k + 1) * 64).min(f.data.len())].to_vec();
                    s.resize(64, 0);
                    mini.extend(s);
                    mini_fat.push(if k + 1 == n { 0xFFFF_FFFE } else { start + k as u32 + 1 });
                }
                starts[i] = start;
            } else {
                starts[i] = chain(&f.data, &mut sectors, &mut fat);
            }
        }
        let mini_len = mini.len();
        let root_start = chain(&mini, &mut sectors, &mut fat);
        let mf: Vec<u8> = mini_fat.iter().flat_map(|x| x.to_le_bytes()).collect();
        let mini_fat_start = chain(&mf, &mut sectors, &mut fat);
        let mini_fat_n = mf.len().div_ceil(sector_size);
        let mut dir = Vec::new();
        for (i, f) in flat.iter().enumerate() {
            let mut e = vec![0u8; 128];
            let units: Vec<u16> = f.name.encode_utf16().take(31).collect();
            for (k, u) in units.iter().enumerate() {
                e[k * 2..k * 2 + 2].copy_from_slice(&u.to_le_bytes());
            }
            e[64..66].copy_from_slice(&(((units.len() + 1) * 2) as u16).to_le_bytes());
            e[66] = f.kind;
            e[67] = 1;
            e[68..72].copy_from_slice(&super::NOSTREAM.to_le_bytes());
            e[72..76].copy_from_slice(&f.right.to_le_bytes());
            e[76..80].copy_from_slice(&f.child.to_le_bytes());
            e[80..96].copy_from_slice(&f.clsid);
            let (start, size) = if i == 0 { (root_start, mini_len as u64) } else { (starts[i], f.data.len() as u64) };
            e[116..120].copy_from_slice(&start.to_le_bytes());
            e[120..128].copy_from_slice(&size.to_le_bytes());
            dir.extend(e);
        }
        let dir_start = chain(&dir, &mut sectors, &mut fat);
        // FAT sectors: enough to cover every sector including themselves.
        let mut n_fat = 1;
        while (sectors.len() + n_fat) > n_fat * per_fat {
            n_fat += 1;
        }
        let fat_start = sectors.len() as u32;
        for _ in 0..n_fat {
            fat.push(0xFFFF_FFFD);
            sectors.push(vec![0; sector_size]);
        }
        fat.resize(n_fat * per_fat, 0xFFFF_FFFF);
        for k in 0..n_fat {
            let s = &mut sectors[fat_start as usize + k];
            for j in 0..per_fat {
                s[j * 4..j * 4 + 4].copy_from_slice(&fat[k * per_fat + j].to_le_bytes());
            }
        }
        let mut h = vec![0u8; 512];
        h[..8].copy_from_slice(&super::SIGNATURE);
        h[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
        h[26..28].copy_from_slice(&(if sector_size == 512 { 3u16 } else { 4 }).to_le_bytes());
        h[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
        h[30..32].copy_from_slice(&(if sector_size == 512 { 9u16 } else { 12 }).to_le_bytes());
        h[32..34].copy_from_slice(&6u16.to_le_bytes());
        h[44..48].copy_from_slice(&(n_fat as u32).to_le_bytes());
        h[48..52].copy_from_slice(&dir_start.to_le_bytes());
        // Without the mini stream every stream is in regular sectors: a cutoff of 0 says so.
        h[56..60].copy_from_slice(&(if use_mini { 4096u32 } else { 0 }).to_le_bytes());
        h[60..64].copy_from_slice(&(if mini_fat_n == 0 { 0xFFFF_FFFE } else { mini_fat_start }).to_le_bytes());
        h[64..68].copy_from_slice(&(mini_fat_n as u32).to_le_bytes());
        h[68..72].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
        for k in 0..109 {
            let v = if k < n_fat { fat_start + k as u32 } else { 0xFFFF_FFFF };
            h[76 + k * 4..80 + k * 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut out = h;
        out.resize(sector_size, 0);
        for s in sectors {
            out.extend(s);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::writer::{Node, write};
    use super::*;

    fn sample() -> Vec<Node> {
        vec![
            Node::Stream { name: "small".into(), data: (0..100u8).collect() },
            Node::Storage {
                name: "folder".into(),
                clsid: [7; 16],
                children: vec![
                    Node::Stream { name: "big".into(), data: (0..20_000u32).map(|x| (x % 251) as u8).collect() },
                    Node::Stream { name: "empty".into(), data: Vec::new() },
                    Node::Storage { name: "deeper".into(), clsid: [9; 16], children: vec![Node::Stream { name: "x".into(), data: vec![1, 2, 3] }] },
                ],
            },
        ]
    }

    fn check(bytes: &[u8]) {
        let c = Cfb::parse(bytes).unwrap();
        let root = c.root();
        let kids: Vec<String> = c.children(root).iter().map(|&k| c.entry(k).unwrap().name.clone()).collect();
        assert_eq!(kids, ["small", "folder"]);
        let small = c.child(root, "small").unwrap();
        assert_eq!(c.read(small, 1 << 20).unwrap(), (0..100u8).collect::<Vec<_>>());
        let folder = c.child(root, "folder").unwrap();
        assert_eq!(c.entry(folder).unwrap().clsid, [7; 16]);
        let big = c.child(folder, "big").unwrap();
        let want: Vec<u8> = (0..20_000u32).map(|x| (x % 251) as u8).collect();
        assert_eq!(c.read(big, 1 << 20).unwrap(), want);
        assert!(c.read(big, 100).is_err(), "a stream larger than the limit is refused");
        assert_eq!(c.read(c.child(folder, "empty").unwrap(), 10).unwrap(), Vec::<u8>::new());
        let deeper = c.child(folder, "deeper").unwrap();
        assert_eq!(c.read(c.child(deeper, "x").unwrap(), 10).unwrap(), vec![1, 2, 3]);
        assert!(c.read(folder, 10).is_err(), "a storage is not a stream");
        assert!(c.child(root, "nope").is_none());
    }

    #[test]
    fn reads_512_and_4096_byte_sectors_with_and_without_the_mini_stream() {
        for size in [512, 4096] {
            for mini in [false, true] {
                check(&write([1; 16], &sample(), size, mini));
            }
        }
    }

    #[test]
    fn many_fat_sectors_are_found() {
        // Enough streams that the FAT needs several sectors.
        let kids: Vec<Node> = (0..300).map(|i| Node::Stream { name: format!("s{i}"), data: vec![i as u8; 5000] }).collect();
        let bytes = write([0; 16], &kids, 512, false);
        let c = Cfb::parse(&bytes).unwrap();
        assert_eq!(c.children(0).len(), 300);
        let k = c.child(0, "s299").unwrap();
        assert_eq!(c.read(k, 1 << 20).unwrap(), vec![(299 % 256) as u8; 5000]);
    }

    #[test]
    fn truncated_and_corrupted_files_are_errors_not_panics() {
        for (size, mini) in [(512, true), (4096, false)] {
            let good = write([1; 16], &sample(), size, mini);
            for cut in (0..good.len()).step_by(97) {
                let bytes = &good[..cut];
                if let Ok(c) = Cfb::parse(bytes) {
                    for i in 0..c.entries().len() {
                        let _ = c.read(i, 1 << 20);
                        let _ = c.children(i);
                    }
                }
            }
            // Flip bytes everywhere (deterministic pseudo-random).
            let mut x: u32 = 12345;
            for _ in 0..2000 {
                let mut bytes = good.clone();
                for _ in 0..4 {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    let at = (x as usize >> 3) % bytes.len();
                    bytes[at] ^= (x >> 24) as u8 | 1;
                }
                if let Ok(c) = Cfb::parse(&bytes) {
                    for i in 0..c.entries().len() {
                        let _ = c.read(i, 1 << 20);
                        let _ = c.children(i);
                    }
                }
            }
        }
    }

    #[test]
    fn looping_chains_are_refused() {
        let mut bytes = write([1; 16], &[Node::Stream { name: "big".into(), data: vec![5; 8000] }], 512, false);
        let fat_at = (u32_at(&bytes, 76).unwrap() as usize + 1) * 512;
        // Sector 1 points back to sector 0.
        bytes[fat_at + 4..fat_at + 8].copy_from_slice(&0u32.to_le_bytes());
        let c = Cfb::parse(&bytes).unwrap();
        let k = c.child(0, "big").unwrap();
        assert!(c.read(k, 1 << 20).is_err());
        // A directory whose sibling links loop still lists each entry once.
        let mut bytes = write([1; 16], &sample(), 512, true);
        let dir_at = (u32_at(&bytes, 48).unwrap() as usize + 1) * 512;
        // Entry 2 ("folder") right-links back to entry 1 ("small").
        bytes[dir_at + 2 * 128 + 72..dir_at + 2 * 128 + 76].copy_from_slice(&1u32.to_le_bytes());
        let c = Cfb::parse(&bytes).unwrap();
        assert_eq!(c.children(0).len(), 2);
    }

    #[test]
    fn not_a_compound_file() {
        assert!(Cfb::parse(b"").is_err());
        assert!(Cfb::parse(&[0u8; 4096]).is_err());
        let mut h = write([1; 16], &sample(), 512, true);
        h[30] = 10;
        assert!(Cfb::parse(&h).is_err(), "unknown sector size");
    }
}
