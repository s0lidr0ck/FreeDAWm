//! The waveform cache: each audio file's waveform overview ([`Peaks`], one per channel) kept in
//! the project's `Waveforms` folder (`Waveforms/<file name>.peaks`), so reopening a long
//! recording doesn't scan all of its audio again for its waveform. A cache file names the audio
//! file's size and modified time and the session rate the overview was made at; one that doesn't
//! match (the file changed) or doesn't read is made again. The folder is safe to delete. A
//! session never saved keeps its overviews in memory only.

use soundcraft_audio_io::AudioBuffer;
use soundcraft_audio_io::peaks::Peaks;
use std::path::{Path, PathBuf};

/// The folder in a project that holds the cache.
pub const FOLDER: &str = "Waveforms";
const MAGIC: &[u8; 8] = b"FDWPEAK1";
const HEADER: usize = 8 + 8 + 8 + 4 + 4;

/// The cache file for the audio file `file` in the project folder `project`.
pub fn path_for(project: &Path, file: &Path) -> Option<PathBuf> {
    let name = file.file_name()?.to_str()?;
    Some(project.join(FOLDER).join(format!("{name}.peaks")))
}

/// The audio file's size and modified time (ns since 1970), what a cache file is good for.
fn stamp(file: &Path) -> Option<(u64, u64)> {
    crate::file_access::check_read(file).ok()?;
    let m = std::fs::metadata(file).ok()?;
    let at = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((m.len(), u64::try_from(at.as_nanos()).unwrap_or(u64::MAX)))
}

fn header(stamp: (u64, u64), rate: u32, channels: usize) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEADER);
    h.extend_from_slice(MAGIC);
    h.extend_from_slice(&stamp.0.to_le_bytes());
    h.extend_from_slice(&stamp.1.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&u32::try_from(channels).unwrap_or(u32::MAX).to_le_bytes());
    h
}

/// The header the cache file for `file` must start with, or `None` when `file` can't be read.
fn expected(file: &Path, rate: u32, channels: usize) -> Option<Vec<u8>> {
    stamp(file).map(|s| header(s, rate, channels))
}

/// The cached overviews of `file` (in `project`) for `buf`, its audio decoded at `rate`; `None`
/// when there is no cache file for it as it is now.
pub fn load(project: &Path, file: &Path, rate: u32, buf: &AudioBuffer) -> Option<Vec<Peaks>> {
    let out = load_sized(project, file, rate, buf.num_channels(), buf.frames())?;
    out.iter().zip(&buf.channels).all(|(p, c)| p.len_samples() == c.len()).then_some(out)
}

/// [`load`] for audio of `channels` channels of `frames` frames each, not decoded (a file played
/// from disk).
pub fn load_sized(project: &Path, file: &Path, rate: u32, channels: usize, frames: usize) -> Option<Vec<Peaks>> {
    let cache = path_for(project, file)?;
    crate::file_access::check_read(&cache).ok()?;
    let want = expected(file, rate, channels)?;
    let bytes = std::fs::read(&cache).ok()?;
    if bytes.get(..HEADER)? != want.as_slice() {
        return None;
    }
    let mut at = HEADER;
    let mut out = Vec::with_capacity(channels);
    for _ in 0..channels {
        let (p, used) = Peaks::from_bytes(bytes.get(at..)?)?;
        if p.len_samples() != frames {
            return None;
        }
        out.push(p);
        at = at.checked_add(used)?;
    }
    Some(out)
}

/// Whether `project` holds a cache file for `file` as it is now (its header alone is read).
pub fn fresh(project: &Path, file: &Path, rate: u32, channels: usize) -> bool {
    use std::io::Read;
    let (Some(cache), Some(want)) = (path_for(project, file), expected(file, rate, channels)) else { return false };
    if crate::file_access::check_read(&cache).is_err() {
        return false;
    }
    let mut got = [0u8; HEADER];
    std::fs::File::open(cache).and_then(|mut f| f.read_exact(&mut got)).is_ok() && got.as_slice() == want.as_slice()
}

/// Keeps `peaks` (the overviews of `file` decoded at `rate`) in `project`'s cache. Best effort:
/// a cache that can't be written is made again next time.
pub fn store(project: &Path, file: &Path, rate: u32, peaks: &[Peaks]) {
    let (Some(cache), Some(mut bytes)) = (path_for(project, file), expected(file, rate, peaks.len())) else { return };
    if crate::file_access::check_write(&cache).is_err() {
        return;
    }
    for p in peaks {
        bytes.extend_from_slice(&p.to_bytes());
    }
    let tmp = cache.with_extension("peaks.tmp");
    let written = cache
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, &bytes))
        .and_then(|()| std::fs::rename(&tmp, &cache));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        log::debug!("waveform cache {}: {e}", cache.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("freedawm-peaks-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Audio Files")).unwrap();
        d
    }

    #[test]
    fn a_cache_is_used_while_its_file_is_unchanged() {
        let d = dir("unchanged");
        let file = d.join("Audio Files").join("Vox.wav");
        std::fs::write(&file, b"pretend audio").unwrap();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![(0..1000).map(|i| (i as f32 * 0.01).sin()).collect(), vec![0.25; 1000]] };
        let peaks: Vec<Peaks> = buf.channels.iter().map(|c| Peaks::build(c)).collect();
        assert!(load(&d, &file, 48_000, &buf).is_none());
        store(&d, &file, 48_000, &peaks);
        assert!(d.join("Waveforms").join("Vox.wav.peaks").is_file());
        assert!(fresh(&d, &file, 48_000, 2));
        assert_eq!(load(&d, &file, 48_000, &buf), Some(peaks.clone()));
        // Another session rate, or another length: not this cache.
        assert!(load(&d, &file, 44_100, &buf).is_none());
        let short = AudioBuffer::new(48_000, 2, 10);
        assert!(load(&d, &file, 48_000, &short).is_none());
        // The file changed: stale.
        std::fs::write(&file, b"other audio, longer").unwrap();
        assert!(!fresh(&d, &file, 48_000, 2));
        assert!(load(&d, &file, 48_000, &buf).is_none());
        // Garbage in the cache is not read.
        store(&d, &file, 48_000, &peaks);
        let cache = d.join("Waveforms").join("Vox.wav.peaks");
        let mut b = std::fs::read(&cache).unwrap();
        b.truncate(b.len() / 2);
        std::fs::write(&cache, b).unwrap();
        assert!(load(&d, &file, 48_000, &buf).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
