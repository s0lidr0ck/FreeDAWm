//! The takes in the ideas folder: a WAV and a MIDI file with the same stem are one take.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One take on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeFiles {
    pub stem: String,
    pub wav: Option<PathBuf>,
    pub mid: Option<PathBuf>,
    pub modified: Option<SystemTime>,
    /// The WAV's length (seconds), read from its header.
    pub seconds: Option<f64>,
}

/// The newest `max` takes in `dir`, newest first.
pub fn recent(dir: &Path, max: usize) -> Vec<TakeFiles> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut takes: Vec<TakeFiles> = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let (Some(ext), Some(stem)) = (ext, path.file_stem().and_then(|s| s.to_str()).map(str::to_string)) else { continue };
        if ext != "wav" && ext != "mid" {
            continue;
        }
        let modified = e.metadata().ok().and_then(|m| m.modified().ok());
        if !takes.iter().any(|t| t.stem == stem) {
            takes.push(TakeFiles { stem: stem.clone(), wav: None, mid: None, modified: None, seconds: None });
        }
        let Some(t) = takes.iter_mut().find(|t| t.stem == stem) else { continue };
        t.modified = t.modified.max(modified);
        if ext == "wav" {
            t.seconds = wav_seconds(&path);
            t.wav = Some(path);
        } else {
            t.mid = Some(path);
        }
    }
    takes.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.stem.cmp(&a.stem)));
    takes.truncate(max);
    takes
}

/// A WAV file's length from its header (a header being rewritten mid-take reads as far as it
/// says).
pub fn wav_seconds(path: &Path) -> Option<f64> {
    use std::io::Read;
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    head.truncate(n);
    wav_header_seconds(&head)
}

/// The length a WAV header says: `fmt ` gives the bytes per second, `data` the size.
fn wav_header_seconds(b: &[u8]) -> Option<f64> {
    let u32_at = |i: usize| b.get(i..i.checked_add(4)?).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
    if b.get(0..4) != Some(b"RIFF") || b.get(8..12) != Some(b"WAVE") {
        return None;
    }
    let (mut at, mut per_sec) = (12usize, None);
    while let (Some(id), Some(size)) = (b.get(at..at.checked_add(4)?), u32_at(at.checked_add(4)?)) {
        let body = at.checked_add(8)?;
        match id {
            b"fmt " => per_sec = u32_at(body.checked_add(8)?).filter(|r| *r > 0),
            b"data" => return per_sec.map(|r| f64::from(size) / f64::from(r)),
            _ => {}
        }
        at = body.checked_add(size as usize)?.checked_add(size as usize % 2)?;
    }
    None
}

/// A WAV file's shape: the loudest sample (0..1) near each of `bars` evenly spaced points, read
/// from up to 4096 frames at each (a few hundred kilobytes, whatever the take's length). PCM
/// (16, 24 or 32 bit) and 32-bit float files; `None` for anything else or a damaged file.
pub fn shape(path: &Path, bars: usize) -> Option<Vec<f32>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; 65_536];
    let n = f.read(&mut head).ok()?;
    head.truncate(n);
    let u16_at = |i: usize| head.get(i..i.checked_add(2)?).and_then(|s| s.try_into().ok()).map(u16::from_le_bytes);
    let u32_at = |i: usize| head.get(i..i.checked_add(4)?).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
    if head.get(0..4) != Some(b"RIFF") || head.get(8..12) != Some(b"WAVE") {
        return None;
    }
    let (mut at, mut fmt, mut data) = (12usize, None, None);
    while let (Some(id), Some(size)) = (head.get(at..at.checked_add(4)?), u32_at(at.checked_add(4)?)) {
        let body = at.checked_add(8)?;
        match id {
            b"fmt " => {
                let tag = u16_at(body)?;
                let tag = if tag == 0xFFFE { u16_at(body.checked_add(24)?)? } else { tag };
                fmt = Some((tag, usize::from(u16_at(body.checked_add(2)?)?), usize::from(u16_at(body.checked_add(14)?)?)));
            }
            b"data" => {
                data = Some((body, size as usize));
                break;
            }
            _ => {}
        }
        at = body.checked_add(size as usize)?.checked_add(size as usize % 2)?;
    }
    let ((tag, channels, bits), (start, size)) = (fmt?, data?);
    let width = bits / 8;
    if channels == 0 || !matches!((tag, bits), (1, 16) | (1, 24) | (1, 32) | (3, 32)) {
        return None;
    }
    let frame = channels.checked_mul(width)?;
    // A take cut short leaves the header's size behind the file's: the file says how far it goes.
    let in_file = (f.metadata().ok()?.len() as usize).saturating_sub(start);
    let frames = size.min(in_file) / frame;
    let bars = bars.max(1);
    let each = (frames / bars).clamp(1, 4096);
    let mut buf = vec![0u8; each * frame];
    let mut out = Vec::with_capacity(bars);
    for i in 0..bars {
        let first = i.checked_mul(frames)? / bars;
        if frames == 0 || first >= frames {
            out.push(0.0);
            continue;
        }
        f.seek(SeekFrom::Start(u64::try_from(start.checked_add(first.checked_mul(frame)?)?).ok()?)).ok()?;
        let got = f.read(&mut buf).ok()?;
        let peak = buf
            .get(..got - got % width)
            .unwrap_or_default()
            .chunks_exact(width)
            .map(|s| match (tag, bits) {
                (1, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32_768.0,
                (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                (1, _) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                _ => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
            })
            .fold(0f32, |m, x| if x.is_finite() { m.max(x.abs()) } else { m });
        out.push(peak.min(1.0));
    }
    Some(out)
}

/// Renames a take's files to `new` (both, or neither: a name taken by another take is refused).
pub fn rename(take: &TakeFiles, new: &str) -> Result<TakeFiles, String> {
    let new = crate::settings::file_safe(new);
    if new.is_empty() {
        return Err("A take needs a name".into());
    }
    if new == take.stem {
        return Ok(take.clone());
    }
    let moved =
        |p: &Option<PathBuf>| p.as_ref().map(|p| p.with_file_name(format!("{new}.{}", p.extension().and_then(|x| x.to_str()).unwrap_or("wav"))));
    let (wav, mid) = (moved(&take.wav), moved(&take.mid));
    if wav.iter().chain(mid.iter()).any(|p| p.exists()) {
        return Err(format!("Another take is already called “{new}”"));
    }
    if let (Some(from), Some(to)) = (&take.wav, &wav) {
        std::fs::rename(from, to).map_err(|e| format!("Cannot rename: {e}"))?;
    }
    if let (Some(from), Some(to)) = (&take.mid, &mid)
        && let Err(e) = std::fs::rename(from, to)
    {
        // Put the WAV back: the pair keeps one name.
        if let (Some(from), Some(to)) = (&take.wav, &wav) {
            let _ = std::fs::rename(to, from);
        }
        return Err(format!("Cannot rename: {e}"));
    }
    Ok(TakeFiles { stem: new, wav, mid, ..take.clone() })
}

/// Deletes a take's files for good (the recorder has no recycle bin of its own; the window asks
/// first).
pub fn delete(take: &TakeFiles) -> Result<(), String> {
    for p in take.wav.iter().chain(take.mid.iter()) {
        std::fs::remove_file(p).map_err(|e| format!("Cannot delete {}: {e}", p.display()))?;
    }
    Ok(())
}

/// Shows `path` in the system's file manager (selected, where the system can).
pub fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let mut cmd = {
        // Explorer reads `/select,"<path>"` itself, so the argument goes as written.
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        c
    };
    #[cfg(not(windows))]
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| format!("Cannot open the folder: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_rename_and_delete() {
        let dir = std::env::temp_dir().join(format!("freedawm-recorder-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Idea A.wav"), b"x").unwrap();
        std::fs::write(dir.join("Idea A.mid"), b"x").unwrap();
        std::fs::write(dir.join("Idea B.mid"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let list = recent(&dir, 10);
        assert_eq!(list.len(), 2);
        let a = list.iter().find(|t| t.stem == "Idea A").unwrap();
        assert!(a.wav.is_some() && a.mid.is_some());
        assert!(rename(a, "Idea B").is_err(), "a name taken is refused");
        let renamed = rename(a, "Chorus").unwrap();
        assert!(dir.join("Chorus.wav").exists() && dir.join("Chorus.mid").exists());
        delete(&renamed).unwrap();
        assert_eq!(recent(&dir, 10).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
