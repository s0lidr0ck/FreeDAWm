//! `.vstpreset` files: reading the chunk layout, refusing presets of other plugins and broken
//! files, finding a plugin's presets in the standard folders.

use soundcraft_dsp::Category;
use soundcraft_vst3_host::Vst3Descriptor;
use soundcraft_vst3_host::vstpreset::{load_vstpreset, pack_state, preset_files_in, read_vstpreset};
use std::path::{Path, PathBuf};

const CLASS: &str = "0123456789ABCDEF0123456789ABCDEF";

/// A `.vstpreset` as the VST3 SDK lays it out (little-endian): `VST3`, version 1, the 32-char
/// class id and the chunk list's offset (48 bytes), the chunks' data, then `List`, the entry count
/// and per entry its 4-char id, offset and size.
fn build(class: &str, chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"VST3");
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(class.as_bytes());
    let offset_at = out.len();
    out.extend_from_slice(&0i64.to_le_bytes());
    let mut entries = Vec::new();
    for (id, data) in chunks {
        entries.push((**id, out.len() as i64, data.len() as i64));
        out.extend_from_slice(data);
    }
    let list = out.len() as i64;
    out[offset_at..offset_at + 8].copy_from_slice(&list.to_le_bytes());
    out.extend_from_slice(b"List");
    out.extend_from_slice(&(entries.len() as i32).to_le_bytes());
    for (id, offset, size) in entries {
        out.extend_from_slice(&id);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
    }
    out
}

fn fixture() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("tests/fixture/presets/Minimal.vstpreset")
}

fn fixture_bytes() -> Vec<u8> {
    build(CLASS, &[(b"Comp", b"component state"), (b"Cont", b"ctrl"), (b"Info", b"<MetaInfo/>")])
}

#[test]
fn reads_a_vstpreset_fixture() {
    let built = fixture_bytes();
    // The committed fixture is exactly what the layout above makes.
    let committed = std::fs::read(fixture()).unwrap();
    assert_eq!(committed, built, "tests/fixture/presets/Minimal.vstpreset is stale");
    let p = read_vstpreset(&committed).unwrap();
    assert_eq!(p.class_id, CLASS);
    assert_eq!(p.component, b"component state");
    assert_eq!(p.controller.as_deref(), Some(&b"ctrl"[..]));
    // Without a controller chunk.
    let p = read_vstpreset(&build(CLASS, &[(b"Comp", b"abc")])).unwrap();
    assert_eq!((p.component.as_slice(), p.controller), (&b"abc"[..], None));
    // Into SoundCraft's state container: `SCV3`, then each part with its length.
    let p = read_vstpreset(&built).unwrap();
    let blob = p.state_blob().unwrap();
    assert_eq!(blob, pack_state(b"component state", b"ctrl").unwrap());
    assert_eq!(&blob[..4], b"SCV3");
    assert_eq!(&blob[4..8], &15u32.to_le_bytes());
}

#[test]
fn rejects_wrong_class_and_corrupt() {
    let bytes = fixture_bytes();
    // The class must be the plugin's (as written raw, or in the GUID text form Windows uses).
    assert!(load_vstpreset(&bytes, CLASS).is_ok());
    assert!(load_vstpreset(&bytes, &CLASS.to_lowercase()).is_ok());
    let guid_form = "67452301AB89EFCD0123456789ABCDEF";
    assert!(load_vstpreset(&bytes, guid_form).is_ok(), "the COM byte order of the same id");
    let err = load_vstpreset(&bytes, "FFFFFFFF89ABCDEF0123456789ABCDEF").unwrap_err();
    assert!(err.contains("made for a different plugin"), "{err}");
    // Truncated anywhere: an error, never a panic.
    for n in 0..bytes.len() {
        assert!(read_vstpreset(&bytes[..n]).is_err(), "truncated at {n}");
    }
    // Not a preset, a bad version, offsets out of the file, no component chunk.
    assert!(read_vstpreset(b"RIFF....").is_err());
    let mut bad = bytes.clone();
    bad[40..48].copy_from_slice(&i64::MAX.to_le_bytes());
    assert!(read_vstpreset(&bad).is_err());
    let mut bad = bytes.clone();
    let list = bytes.len() - 3 * 20 - 8;
    bad[list + 8 + 4..list + 8 + 12].copy_from_slice(&(-5i64).to_le_bytes());
    assert!(read_vstpreset(&bad).is_err());
    let mut bad = bytes.clone();
    bad[list + 4..list + 8].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(read_vstpreset(&bad).is_err());
    assert!(read_vstpreset(&build(CLASS, &[(b"Cont", b"x")])).unwrap_err().contains("no component state"));
    let mut bad = bytes.clone();
    bad[8] = 0xFF;
    assert!(read_vstpreset(&bad).is_err(), "a class id that is not ASCII hex");
}

fn desc(vendor: &str, name: &str) -> Vst3Descriptor {
    Vst3Descriptor {
        id: format!("vst3:{CLASS}"),
        class_id: CLASS.into(),
        name: name.into(),
        vendor: vendor.into(),
        version: String::new(),
        sdk_version: String::new(),
        sub_categories: Vec::new(),
        category: Category::Other,
        is_instrument: false,
        path: PathBuf::new(),
    }
}

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"x").unwrap();
}

#[test]
fn finds_presets_under_vendor_and_name() {
    let tmp = std::env::temp_dir().join(format!("sc-vstpreset-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (user, shared) = (tmp.join("user"), tmp.join("shared"));
    touch(&user.join("Acme/Squash/Gentle.vstpreset"));
    touch(&user.join("Acme/Squash/Drums/Kick Punch.VSTPRESET"));
    touch(&user.join("Acme/Squash/notes.txt"));
    touch(&user.join("Acme/Other/Nope.vstpreset"));
    touch(&shared.join("Acme/Squash/Factory.vstpreset"));
    let found = preset_files_in(&[user.clone(), shared.clone(), tmp.join("missing")], &desc("Acme", "Squash"));
    let names: Vec<String> = found.iter().map(|p| p.strip_prefix(&tmp).unwrap().to_string_lossy().replace('\\', "/")).collect();
    assert_eq!(names, ["user/Acme/Squash/Drums/Kick Punch.VSTPRESET", "user/Acme/Squash/Gentle.vstpreset", "shared/Acme/Squash/Factory.vstpreset"]);
    assert!(preset_files_in(&[user], &desc("Nobody", "Squash")).is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Reads one real installed `.vstpreset` (the first found under the standard folders) and checks
/// it parses. Run with `-- --ignored --nocapture`.
#[test]
#[ignore = "reads presets installed on this machine"]
fn reads_an_installed_vstpreset() {
    let roots = soundcraft_vst3_host::vstpreset::preset_roots();
    let mut stack: Vec<PathBuf> = roots.clone();
    let mut tried = 0;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vstpreset")) {
                let bytes = std::fs::read(&p).unwrap();
                let r = read_vstpreset(&bytes);
                println!("{}: {:?}", p.display(), r.as_ref().map(|v| (v.class_id.clone(), v.component.len(), v.controller.as_ref().map(Vec::len))));
                r.unwrap();
                tried += 1;
                if tried >= 5 {
                    return;
                }
            }
        }
    }
    assert!(tried > 0, "no .vstpreset under {roots:?}");
}
