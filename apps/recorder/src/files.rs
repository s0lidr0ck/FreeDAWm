//! The ideas in the ideas folder. A WAV and a MIDI file with the same stem are one take; an
//! idea's other layers are `<idea> (layer 2).wav` / `.mid` and so on beside it (see
//! [`crate::layers`]), and its sidecar `<idea>.fdidea` remembers how they play together. Files
//! named that way by hand are picked up as layers too.

use crate::layers::{SIDECAR_EXT, layer_stem, split_layer};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One layer of an idea on disk (layer 1 is the idea's own WAV and MIDI file).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerFiles {
    /// 1 for the idea's first take, 2 for `<idea> (layer 2)`…
    pub n: u32,
    pub wav: Option<PathBuf>,
    pub mid: Option<PathBuf>,
    /// What the keys sounded like (`<layer> (keys).wav`), when it was saved.
    pub keys: Option<PathBuf>,
    /// The WAV's length (seconds), read from its header.
    pub seconds: Option<f64>,
}

/// One idea on disk: its first take (`wav`, `mid`) and its other layers.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeFiles {
    pub stem: String,
    pub wav: Option<PathBuf>,
    pub mid: Option<PathBuf>,
    /// What the keys sounded like (`<layer> (keys).wav`), when it was saved.
    pub keys: Option<PathBuf>,
    pub modified: Option<SystemTime>,
    /// The WAV's length (seconds), read from its header.
    pub seconds: Option<f64>,
    /// Layers 2, 3… (in order; numbers can have gaps where one was thrown away).
    pub layers: Vec<LayerFiles>,
    /// `<idea>.fdidea`, when there is one.
    pub sidecar: Option<PathBuf>,
}

impl TakeFiles {
    /// A take of one layer.
    pub fn single(stem: &str, wav: Option<PathBuf>, mid: Option<PathBuf>, seconds: Option<f64>) -> TakeFiles {
        TakeFiles { stem: stem.to_string(), wav, mid, keys: None, modified: None, seconds, layers: Vec::new(), sidecar: None }
    }

    /// Every layer, the first take included (when it still has a file), in order.
    pub fn all_layers(&self) -> Vec<LayerFiles> {
        let first = (self.wav.is_some() || self.mid.is_some() || self.keys.is_some()).then(|| LayerFiles {
            n: 1,
            wav: self.wav.clone(),
            mid: self.mid.clone(),
            keys: self.keys.clone(),
            seconds: self.seconds,
        });
        first.into_iter().chain(self.layers.iter().cloned()).collect()
    }

    /// How many layers it has.
    pub fn layer_count(&self) -> usize {
        self.all_layers().len()
    }

    /// The next free layer number.
    pub fn next_layer(&self) -> u32 {
        self.all_layers().iter().map(|l| l.n).max().unwrap_or(1).saturating_add(1).max(2)
    }

    /// Its length: the first take's, else the longest layer's.
    pub fn length(&self) -> Option<f64> {
        self.seconds.or_else(|| self.layers.iter().filter_map(|l| l.seconds).reduce(f64::max))
    }

    /// Every file it has (layers and sidecar).
    pub fn files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.all_layers().into_iter().flat_map(|l| l.wav.into_iter().chain(l.mid).chain(l.keys)).collect();
        v.extend(self.sidecar.clone());
        v
    }

    /// The folder it is in.
    pub fn folder(&self) -> Option<PathBuf> {
        self.files().first().and_then(|p| p.parent().map(Path::to_path_buf))
    }

    /// The sidecar's path (whether or not it exists yet), beside its files.
    pub fn sidecar_path(&self) -> Option<PathBuf> {
        self.folder().map(|d| d.join(format!("{}.{SIDECAR_EXT}", self.stem)))
    }
}

/// The newest `max` ideas in `dir`, newest first; an idea with layers is one idea.
pub fn recent(dir: &Path, max: usize) -> Vec<TakeFiles> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut takes: Vec<TakeFiles> = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let (Some(ext), Some(stem)) = (ext, path.file_stem().and_then(|s| s.to_str()).map(str::to_string)) else { continue };
        if ext != "wav" && ext != "mid" && ext != SIDECAR_EXT {
            continue;
        }
        let heard = stem.strip_suffix(" (keys)").filter(|_| ext == "wav");
        let (idea, n) = if ext == SIDECAR_EXT { (stem.clone(), 0) } else { split_layer(heard.unwrap_or(&stem)) };
        let modified = e.metadata().ok().and_then(|m| m.modified().ok());
        if !takes.iter().any(|t| t.stem == idea) {
            takes.push(TakeFiles::single(&idea, None, None, None));
        }
        let Some(t) = takes.iter_mut().find(|t| t.stem == idea) else { continue };
        if ext == SIDECAR_EXT {
            t.sidecar = Some(path);
            continue;
        }
        t.modified = t.modified.max(modified);
        if n <= 1 {
            if heard.is_some() {
                t.keys = Some(path);
            } else if ext == "wav" {
                t.seconds = wav_seconds(&path);
                t.wav = Some(path);
            } else {
                t.mid = Some(path);
            }
            continue;
        }
        if !t.layers.iter().any(|l| l.n == n) {
            t.layers.push(LayerFiles { n, wav: None, mid: None, keys: None, seconds: None });
        }
        if let Some(l) = t.layers.iter_mut().find(|l| l.n == n) {
            if heard.is_some() {
                l.keys = Some(path);
            } else if ext == "wav" {
                l.seconds = wav_seconds(&path);
                l.wav = Some(path);
            } else {
                l.mid = Some(path);
            }
        }
    }
    // A sidecar alone is not an idea.
    takes.retain(|t| t.layer_count() > 0);
    for t in &mut takes {
        t.layers.sort_by_key(|l| l.n);
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

/// Renames an idea's files (every layer's and its sidecar) to `new`: all, or none (a name taken
/// by another idea is refused).
pub fn rename(take: &TakeFiles, new: &str) -> Result<TakeFiles, String> {
    let new = crate::settings::file_safe(new);
    if new.is_empty() {
        return Err("A take needs a name".into());
    }
    if new == take.stem {
        return Ok(take.clone());
    }
    let moved = |p: &PathBuf, n: u32| {
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("wav").to_string();
        p.with_file_name(format!("{}.{ext}", layer_stem(&new, n)))
    };
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut out = TakeFiles { stem: new.clone(), layers: Vec::new(), ..take.clone() };
    for l in take.all_layers() {
        let (wav, mid) = (l.wav.as_ref().map(|p| moved(p, l.n)), l.mid.as_ref().map(|p| moved(p, l.n)));
        plan.extend(l.wav.iter().cloned().zip(wav.clone()));
        plan.extend(l.mid.iter().cloned().zip(mid.clone()));
        let keys = l.keys.as_ref().map(|p| p.with_file_name(format!("{}.wav", crate::keys::keys_stem(&layer_stem(&new, l.n)))));
        plan.extend(l.keys.iter().cloned().zip(keys.clone()));
        if l.n == 1 {
            (out.wav, out.mid, out.keys) = (wav, mid, keys);
        } else {
            out.layers.push(LayerFiles { wav, mid, keys, ..l });
        }
    }
    if let Some(s) = &take.sidecar {
        let to = s.with_file_name(format!("{new}.{SIDECAR_EXT}"));
        plan.push((s.clone(), to.clone()));
        out.sidecar = Some(to);
    }
    // The new names must all be free (any file of another idea with that name counts).
    let taken = plan.iter().any(|(_, to)| to.exists()) || take.folder().is_some_and(|dir| recent(&dir, usize::MAX).iter().any(|t| t.stem == new));
    if taken {
        return Err(format!("Another take is already called “{new}”"));
    }
    let mut done: Vec<&(PathBuf, PathBuf)> = Vec::new();
    for step in &plan {
        if let Err(e) = std::fs::rename(&step.0, &step.1) {
            // Put back what moved: the idea keeps one name.
            for (from, to) in done.iter().rev() {
                let _ = std::fs::rename(to, from);
            }
            return Err(format!("Cannot rename: {e}"));
        }
        done.push(step);
    }
    Ok(out)
}

/// Deletes an idea's files for good, every layer and its sidecar (the recorder has no recycle
/// bin of its own; the window asks first).
pub fn delete(take: &TakeFiles) -> Result<(), String> {
    for p in take.files() {
        std::fs::remove_file(&p).map_err(|e| format!("Cannot delete {}: {e}", p.display()))?;
    }
    Ok(())
}

/// Deletes one layer's files for good (layer 1 is the idea's own WAV and MIDI file); the
/// idea's other layers stay as they are.
pub fn delete_layer(take: &TakeFiles, n: u32) -> Result<TakeFiles, String> {
    let Some(l) = take.all_layers().into_iter().find(|l| l.n == n) else { return Err(format!("There is no layer {n}")) };
    for p in l.wav.iter().chain(l.mid.iter()).chain(l.keys.iter()) {
        std::fs::remove_file(p).map_err(|e| format!("Cannot delete {}: {e}", p.display()))?;
    }
    let mut out = take.clone();
    if n == 1 {
        (out.wav, out.mid, out.keys, out.seconds) = (None, None, None, None);
    }
    out.layers.retain(|l| l.n != n);
    Ok(out)
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
