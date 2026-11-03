//! The DAWproject container: a zip holding `project.xml`, `metadata.xml` and the embedded files
//! (audio, plugin states). Entries may be stored or deflated; the workspace's `zip` has no
//! codecs, so deflated entries are read raw and inflated with `miniz_oxide`. Written archives
//! store every entry (audio does not compress, and every reader handles stored entries).

use std::io::{Read, Seek, Write};

/// Most entries read from one archive.
const MAX_ENTRIES: usize = 100_000;

/// An open archive.
pub struct Archive<R: Read + Seek> {
    zip: zip::ZipArchive<R>,
    /// Entry names (normalised: forward slashes, no leading `./` or `/`) by index.
    names: Vec<String>,
}

/// An archive path as stored in `project.xml` or the zip: forward slashes, no leading `./`/`/`.
pub fn normalise(path: &str) -> String {
    let p = path.replace('\\', "/");
    let mut s = p.as_str();
    loop {
        if let Some(rest) = s.strip_prefix("./") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix('/') {
            s = rest;
        } else {
            break;
        }
    }
    s.to_string()
}

impl<R: Read + Seek> Archive<R> {
    pub fn new(reader: R) -> Result<Self, String> {
        let zip = zip::ZipArchive::new(reader).map_err(|e| format!("not a DAWproject file (not a zip archive: {e})"))?;
        if zip.len() > MAX_ENTRIES {
            return Err("the archive has too many entries".into());
        }
        let names = zip.file_names().map(|n| n.map(|x| normalise(&x)).unwrap_or_default()).collect::<Vec<_>>();
        Ok(Archive { zip, names })
    }

    /// The entry names (normalised).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        let want = normalise(name);
        self.names.iter().position(|n| *n == want).or_else(|| self.names.iter().position(|n| n.eq_ignore_ascii_case(&want)))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.index_of(name).is_some()
    }

    /// The entry's uncompressed size as declared (hostile: only a hint).
    pub fn size(&mut self, name: &str) -> Option<u64> {
        let i = self.index_of(name)?;
        self.zip.by_index_raw(i).ok().map(|f| f.size())
    }

    /// The bytes of entry `name`, at most `max` of them (stored or deflated entries).
    pub fn read(&mut self, name: &str, max: u64) -> Result<Vec<u8>, String> {
        let i = self.index_of(name).ok_or_else(|| format!("{name}: not in the archive"))?;
        let mut f = self.zip.by_index_raw(i).map_err(|e| format!("{name}: {e}"))?;
        let method = f.compression();
        let packed = f.compressed_size();
        let size = f.size();
        if size > max || packed > max {
            return Err(format!("{name}: too large ({size} bytes)"));
        }
        let mut raw = Vec::new();
        f.by_ref().take(packed).read_to_end(&mut raw).map_err(|e| format!("{name}: {e}"))?;
        let cap = usize::try_from(max).unwrap_or(usize::MAX);
        // By constant: which `CompressionMethod` variants exist depends on features.
        if method == zip::CompressionMethod::Stored {
            Ok(raw)
        } else if method == zip::CompressionMethod::DEFLATE {
            miniz_oxide::inflate::decompress_to_vec_with_limit(&raw, cap).map_err(|e| format!("{name}: damaged compressed data ({:?})", e.status))
        } else {
            Err(format!("{name}: compressed with {method}, which is not supported"))
        }
    }
}

/// Writes an archive to `w`: each entry stored, in the order given.
pub struct ArchiveWriter<W: Write + Seek> {
    zip: zip::ZipWriter<W>,
    written: Vec<String>,
}

impl<W: Write + Seek> ArchiveWriter<W> {
    pub fn new(w: W) -> Self {
        ArchiveWriter { zip: zip::ZipWriter::new(w), written: Vec::new() }
    }

    /// Adds `name` (an archive path) with `bytes`. A name already written is an error.
    pub fn add(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let name = normalise(name);
        if self.written.contains(&name) {
            return Err(format!("{name}: written twice"));
        }
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(bytes.len() as u64 >= u32::MAX as u64);
        self.zip.start_file(name.as_str(), opts).map_err(|e| format!("{name}: {e}"))?;
        self.zip.write_all(bytes).map_err(|e| format!("{name}: {e}"))?;
        self.written.push(name);
        Ok(())
    }

    pub fn finish(self) -> Result<W, String> {
        self.zip.finish().map_err(|e| format!("archive: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_and_deflated_entries_read_back() {
        let mut w = ArchiveWriter::new(std::io::Cursor::new(Vec::new()));
        w.add("project.xml", b"<Project/>").unwrap();
        w.add("./audio/a.wav", b"RIFF").unwrap();
        assert!(w.add("audio/a.wav", b"again").is_err());
        let bytes = w.finish().unwrap().into_inner();
        let mut a = Archive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(a.read("project.xml", 100).unwrap(), b"<Project/>");
        assert_eq!(a.read("AUDIO/A.WAV", 100).unwrap(), b"RIFF");
        assert!(a.read("project.xml", 3).is_err(), "over the cap");
        assert!(a.read("missing", 100).is_err());

        // A deflated entry, as Bitwig and Studio One write them.
        let data = b"hello hello hello hello hello".repeat(20);
        let packed = miniz_oxide::deflate::compress_to_vec(&data, 6);
        let mut raw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        raw.start_file("x.txt", opts).unwrap();
        raw.write_all(&data).unwrap();
        let stored = raw.finish().unwrap().into_inner();
        // Rewrite the entry as deflated by hand: patch method, sizes and data.
        let deflated = deflate_entry(&stored, &data, &packed);
        let mut a = Archive::new(std::io::Cursor::new(deflated)).unwrap();
        assert_eq!(a.read("x.txt", 10_000).unwrap(), data);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(Archive::new(std::io::Cursor::new(b"not a zip".to_vec())).is_err());
        assert!(Archive::new(std::io::Cursor::new(Vec::new())).is_err());
    }

    /// A one-entry zip (`stored`, holding `data`) rebuilt with the entry deflated (`packed`).
    pub(crate) fn deflate_entry(stored: &[u8], data: &[u8], packed: &[u8]) -> Vec<u8> {
        let name_len = u16::from_le_bytes([stored[26], stored[27]]) as usize;
        let extra_len = u16::from_le_bytes([stored[28], stored[29]]) as usize;
        let name = &stored[30..30 + name_len];
        let crc = &stored[14..18];
        let mut out = Vec::new();
        // Local header.
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        out.extend_from_slice(crc);
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_len as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(packed);
        let _ = extra_len;
        let cd = out.len();
        // Central directory.
        out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        out.extend_from_slice(crc);
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_len as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(name);
        let cd_len = out.len() - cd;
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0]);
        out.extend_from_slice(&(cd_len as u32).to_le_bytes());
        out.extend_from_slice(&(cd as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}
