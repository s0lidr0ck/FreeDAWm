//! The decode cache: an audio file that cannot play from disk as it is (AIFF, FLAC, MP3, AAC,
//! Ogg, a WAV at another rate…) is decoded into memory when a session opens, as before, and the
//! result is also written once to the project's `Decoded` folder as a float WAV at the session
//! rate. The next open plays that file from disk (streamed through the disk cache) instead of
//! decoding the original into memory again. A cache file's name holds the original's size,
//! modified time and the session rate, so a changed original or another rate makes a new one.
//! The folder is safe to delete. A session never saved (no project folder) keeps decoding. Like
//! the waveform cache, it checks the file-access roots itself before it reads or writes.

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The folder in a project that holds the cache.
pub const FOLDER: &str = "Decoded";

/// The audio file's size and modified time (ns since 1970).
fn stamp(file: &Path) -> Option<(u64, u64)> {
    crate::file_access::check_read(file).ok()?;
    let m = std::fs::metadata(file).ok()?;
    let at = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((m.len(), u64::try_from(at.as_nanos()).unwrap_or(u64::MAX)))
}

/// The cache file for `file` decoded at `rate`, in the project folder `project`.
pub fn path_for(project: &Path, file: &Path, rate: u32) -> Option<PathBuf> {
    let name = file.file_name()?.to_str()?;
    let (size, at) = stamp(file)?;
    Some(project.join(FOLDER).join(format!("{name}.{size:x}-{at:x}.{rate}.wav")))
}

/// The cached decode of `file` at `rate`, opened to play from disk, when there is one.
pub fn open(project: &Path, file: &Path, rate: u32) -> Option<Arc<soundcraft_model::disk::DiskFile>> {
    let p = path_for(project, file, rate)?;
    if crate::file_access::check_read(&p).is_err() || !p.is_file() {
        return None;
    }
    crate::io::streamable(&p, rate).filter(|f| f.frames() > 0)
}

/// Writes `buf` (the decode of `file` at `rate`) to the cache, replacing older decodes of the
/// same file name. Nothing is kept when it cannot be written; the open goes on either way.
pub fn store(project: &Path, file: &Path, rate: u32, buf: &AudioBuffer) {
    let Some(p) = path_for(project, file, rate) else { return };
    let Some(dir) = p.parent() else { return };
    if crate::file_access::check_write(&p).is_err() {
        return;
    }
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, ..EncodeOptions::default() };
    let Ok(bytes) = soundcraft_audio_io::encode(buf, &opts) else { return };
    // Written beside, then renamed: a half-written file is never taken for a decode.
    let tmp = p.with_extension("wav.part");
    if std::fs::write(&tmp, bytes).is_err() || std::fs::rename(&tmp, &p).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    // Older decodes of this file (it changed, or the session rate did) go.
    if let (Some(name), Ok(entries)) = (file.file_name().and_then(|n| n.to_str()), std::fs::read_dir(dir)) {
        let prefix = format!("{name}.");
        for e in entries.flatten() {
            let n = e.file_name();
            let n = n.to_string_lossy();
            let theirs = n.strip_prefix(&prefix).is_some_and(|rest| rest.ends_with(".wav") && rest.matches('.').count() == 2);
            if theirs && e.path() != p {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}
