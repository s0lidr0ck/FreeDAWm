//! VST3 preset files (`.vstpreset`, the VST3 SDK's `PresetFile` layout) and the standard folders
//! they live in. Pure Rust (no plugin is loaded): reading a preset gives the component and
//! controller states, which SoundCraft stores as a module's state ([`pack_state`]) and restores
//! into a new instance (component `setState`, controller `setComponentState` + `setState`).
//!
//! Layout (little-endian): `VST3`, version (`i32`), the class id as 32 ASCII characters, the
//! chunk list's offset (`i64`); the chunk list is `List`, an entry count (`i32`) and per entry a
//! 4-character id (`Comp`, `Cont`, `Info`, …), an offset and a size (`i64` each).

use crate::Vst3Descriptor;
use std::path::{Path, PathBuf};

/// The header's magic.
const MAGIC: &[u8; 4] = b"VST3";
/// The chunk list's magic.
const LIST: &[u8; 4] = b"List";
/// Header: magic, version, class id, list offset.
const HEADER: usize = 4 + 4 + 32 + 8;
/// Most chunks a list may declare (the SDK writes three or four).
const MAX_CHUNKS: usize = 64;
/// Largest preset file read (a sampler's preset can embed a few MB).
pub const MAX_PRESET_BYTES: u64 = 64 << 20;
/// SoundCraft's VST3 state container (`STATE_MAGIC` in the host): component, then controller.
const STATE_MAGIC: &[u8; 4] = b"SCV3";
/// Most preset files listed for one plugin.
const MAX_FILES: usize = 5000;
/// How deep the vendor/plugin folder is searched for preset sub-folders.
const MAX_DEPTH: usize = 4;

/// One `.vstpreset`'s content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VstPreset {
    /// The class id the preset was saved for (32 hex digits, as written).
    pub class_id: String,
    /// The component (processor) state.
    pub component: Vec<u8>,
    /// The edit controller's state, when the preset has one.
    pub controller: Option<Vec<u8>>,
}

impl VstPreset {
    /// The preset as a SoundCraft VST3 state blob (what `Plugin::load_state` takes).
    pub fn state_blob(&self) -> Option<Vec<u8>> {
        pack_state(&self.component, self.controller.as_deref().unwrap_or_default())
    }
}

/// SoundCraft's VST3 state container: `SCV3`, then the component state and the controller state,
/// each after its length (`u32`, little-endian). `None` when a part is over 4 GiB.
pub fn pack_state(component: &[u8], controller: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(12 + component.len() + controller.len());
    out.extend_from_slice(STATE_MAGIC);
    out.extend_from_slice(&u32::try_from(component.len()).ok()?.to_le_bytes());
    out.extend_from_slice(component);
    out.extend_from_slice(&u32::try_from(controller.len()).ok()?.to_le_bytes());
    out.extend_from_slice(controller);
    Some(out)
}

fn take<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn i64_at(b: &[u8], at: usize) -> Option<i64> {
    take::<8>(b, at).map(i64::from_le_bytes)
}

/// A chunk's bytes: `offset` and `size` must lie inside the file.
fn chunk(b: &[u8], offset: i64, size: i64) -> Option<&[u8]> {
    let start = usize::try_from(offset).ok()?;
    let len = usize::try_from(size).ok()?;
    b.get(start..start.checked_add(len)?)
}

/// Reads a `.vstpreset`. The bytes are hostile: any inconsistency is an error, never a panic.
pub fn read_vstpreset(bytes: &[u8]) -> Result<VstPreset, String> {
    let broken = |why: &str| format!("not a usable VST3 preset: {why}");
    if bytes.len() < HEADER || bytes.get(..4) != Some(&MAGIC[..]) {
        return Err(broken("no VST3 preset header"));
    }
    let version = take::<4>(bytes, 4).map(i32::from_le_bytes).ok_or_else(|| broken("no version"))?;
    if version < 1 {
        return Err(broken("unknown version"));
    }
    let class = bytes.get(8..40).ok_or_else(|| broken("no class id"))?;
    if !class.iter().all(u8::is_ascii_hexdigit) {
        return Err(broken("the class id is not 32 hex digits"));
    }
    let class_id = String::from_utf8_lossy(class).into_owned();
    let list = i64_at(bytes, 40).and_then(|o| usize::try_from(o).ok()).ok_or_else(|| broken("bad chunk list offset"))?;
    if list < HEADER || take::<4>(bytes, list) != Some(*LIST) {
        return Err(broken("no chunk list"));
    }
    let count = list
        .checked_add(4)
        .and_then(|at| take::<4>(bytes, at))
        .map(i32::from_le_bytes)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| broken("bad chunk count"))?;
    if count > MAX_CHUNKS {
        return Err(broken("too many chunks"));
    }
    let (mut component, mut controller) = (None, None);
    for i in 0..count {
        let at = i.checked_mul(20).and_then(|o| o.checked_add(8)).and_then(|o| list.checked_add(o)).ok_or_else(|| broken("bad chunk list offset"))?;
        let id = take::<4>(bytes, at).ok_or_else(|| broken("the chunk list is cut short"))?;
        let (offset, size) = (at.checked_add(4).and_then(|a| i64_at(bytes, a)), at.checked_add(12).and_then(|a| i64_at(bytes, a)));
        let (Some(offset), Some(size)) = (offset, size) else { return Err(broken("the chunk list is cut short")) };
        let data = chunk(bytes, offset, size).ok_or_else(|| broken("a chunk lies outside the file"))?;
        match &id {
            b"Comp" => component = Some(data.to_vec()),
            b"Cont" => controller = Some(data.to_vec()),
            _ => {}
        }
    }
    let component = component.ok_or_else(|| broken("no component state"))?;
    Ok(VstPreset { class_id, component, controller })
}

/// The class id's COM (GUID) text form: Windows writes and reads `Data1`, `Data2` and `Data3` of
/// the 16 bytes as little-endian numbers, so their bytes swap.
fn guid_form(hex: &str) -> Option<String> {
    let h = hex.as_bytes();
    if h.len() != 32 {
        return None;
    }
    let pair = |i: usize| h.get(i * 2..i * 2 + 2);
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut out = Vec::with_capacity(32);
    for i in order {
        out.extend_from_slice(pair(i)?);
    }
    String::from_utf8(out).ok()
}

/// Whether a preset's class id names the plugin of `class_id` (as the host writes it): the same
/// hex digits, or the same id in the other byte order (a preset saved on another platform).
pub fn same_class(preset: &str, class_id: &str) -> bool {
    preset.eq_ignore_ascii_case(class_id) || guid_form(class_id).is_some_and(|g| g.eq_ignore_ascii_case(preset))
}

/// Reads a `.vstpreset` for the plugin of `class_id`; one saved for another plugin is refused.
pub fn load_vstpreset(bytes: &[u8], class_id: &str) -> Result<VstPreset, String> {
    let p = read_vstpreset(bytes)?;
    if !same_class(&p.class_id, class_id) {
        return Err("this preset was made for a different plugin".into());
    }
    Ok(p)
}

/// Reads the file at `path` for the plugin of `class_id` (see [`load_vstpreset`]).
pub fn load_vstpreset_file(path: &Path, class_id: &str) -> Result<VstPreset, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > MAX_PRESET_BYTES {
        return Err(format!("{}: the preset is too large", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    load_vstpreset(&bytes, class_id)
}

/// The parts of a SoundCraft VST3 state blob ([`pack_state`]): the component state and the
/// controller state. A blob without the container is a bare component state. `None` when the
/// container is malformed.
pub fn unpack_state(data: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let Some(rest) = data.strip_prefix(STATE_MAGIC) else { return (!data.is_empty()).then(|| (data.to_vec(), Vec::new())) };
    let part = |b: &[u8]| -> Option<(Vec<u8>, usize)> {
        let n = usize::try_from(u32::from_le_bytes(take::<4>(b, 0)?)).ok()?;
        Some((b.get(4..4usize.checked_add(n)?)?.to_vec(), 4 + n))
    };
    let (component, used) = part(rest)?;
    let rest = rest.get(used..)?;
    let (controller, used) = part(rest)?;
    (used == rest.len()).then_some((component, controller))
}

/// A `.vstpreset` file for the plugin of `class_id` (32 hex digits, written as given) holding
/// `component` and, when not empty, `controller` (the layout [`read_vstpreset`] reads). `None`
/// when the class id is not 32 hex digits.
pub fn write_vstpreset(class_id: &str, component: &[u8], controller: &[u8]) -> Option<Vec<u8>> {
    if class_id.len() != 32 || !class_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER + component.len() + controller.len() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(class_id.as_bytes());
    out.extend_from_slice(&0i64.to_le_bytes());
    let mut chunks: Vec<(&[u8; 4], i64, i64)> = Vec::new();
    let comp_at = i64::try_from(out.len()).ok()?;
    out.extend_from_slice(component);
    chunks.push((b"Comp", comp_at, i64::try_from(component.len()).ok()?));
    if !controller.is_empty() {
        let at = i64::try_from(out.len()).ok()?;
        out.extend_from_slice(controller);
        chunks.push((b"Cont", at, i64::try_from(controller.len()).ok()?));
    }
    let list_at = i64::try_from(out.len()).ok()?;
    out.extend_from_slice(LIST);
    out.extend_from_slice(&i32::try_from(chunks.len()).ok()?.to_le_bytes());
    for (id, at, size) in chunks {
        out.extend_from_slice(id);
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
    }
    out.get_mut(40..48)?.copy_from_slice(&list_at.to_le_bytes());
    Some(out)
}

/// The class id's other text form (see [`same_class`]): Windows' GUID byte order for the plain
/// hex digits, and back.
pub fn other_class_form(class_id: &str) -> Option<String> {
    guid_form(class_id)
}

/// The standard VST3 preset folders (each holds `<vendor>/<plugin name>/…`), user folders first.
pub fn preset_roots() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(home) = env("USERPROFILE") {
            out.push(home.join("Documents").join("VST3 Presets"));
        }
        if let Some(data) = env("PROGRAMDATA") {
            out.push(data.join("VST3 Presets"));
        }
        let common = env("CommonProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Common Files"));
        out.push(common.join("VST3 Presets"));
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            out.push(home.join("Library/Audio/Presets"));
        }
        out.push(PathBuf::from("/Library/Audio/Presets"));
        out.push(PathBuf::from("/Network/Library/Audio/Presets"));
    } else {
        if let Some(home) = env("HOME") {
            out.push(home.join(".vst3/presets"));
        }
        out.push(PathBuf::from("/usr/share/vst3/presets"));
        out.push(PathBuf::from("/usr/local/share/vst3/presets"));
    }
    out
}

/// The plugin's `.vstpreset` files in the standard folders ([`preset_roots`]).
pub fn preset_files(desc: &Vst3Descriptor) -> Vec<PathBuf> {
    preset_files_in(&preset_roots(), desc)
}

/// The plugin's `.vstpreset` files under `<root>/<vendor>/<plugin name>/` (and its sub-folders,
/// a few levels deep) of each root in turn; within a root, sorted by path (case-insensitive).
pub fn preset_files_in(roots: &[PathBuf], desc: &Vst3Descriptor) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let dir = root.join(&desc.vendor).join(&desc.name);
        let mut found = Vec::new();
        collect(&dir, 0, &mut found);
        found.sort_by_cached_key(|p| p.to_string_lossy().to_lowercase());
        for p in found {
            if out.len() >= MAX_FILES {
                return out;
            }
            out.push(p);
        }
    }
    out
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let p = e.path();
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if is_dir {
            if depth < MAX_DEPTH {
                collect(&p, depth + 1, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vstpreset")) {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_form_swaps_the_first_three_fields() {
        assert_eq!(guid_form("0123456789ABCDEF0123456789ABCDEF").as_deref(), Some("67452301AB89EFCD0123456789ABCDEF"));
        assert_eq!(guid_form("short"), None);
        assert!(same_class("67452301ab89efcd0123456789abcdef", "0123456789ABCDEF0123456789ABCDEF"));
    }

    #[test]
    fn written_presets_read_back_and_states_unpack() {
        let id = "0123456789ABCDEF0123456789ABCDEF";
        let file = write_vstpreset(id, b"comp", b"ctrl").unwrap();
        let p = read_vstpreset(&file).unwrap();
        assert_eq!((p.class_id.as_str(), p.component.as_slice(), p.controller.as_deref()), (id, &b"comp"[..], Some(&b"ctrl"[..])));
        let bare = write_vstpreset(id, b"only", b"").unwrap();
        assert_eq!(read_vstpreset(&bare).unwrap().controller, None);
        assert!(write_vstpreset("xyz", b"c", b"").is_none());
        let blob = pack_state(b"comp", b"ctrl").unwrap();
        assert_eq!(unpack_state(&blob), Some((b"comp".to_vec(), b"ctrl".to_vec())));
        assert_eq!(unpack_state(b"raw"), Some((b"raw".to_vec(), Vec::new())));
        assert_eq!(unpack_state(&[b'S', b'C', b'V', b'3', 0xff, 0xff, 0xff, 0xff]), None);
        assert_eq!(unpack_state(b""), None);
        assert_eq!(other_class_form(&other_class_form(id).unwrap()).as_deref(), Some(id));
    }
}
