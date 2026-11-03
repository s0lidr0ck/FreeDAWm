//! Windows only: embed the app icon and version info (VERSIONINFO) into `freedawm.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `FREEDAWM_REQUIRE_WINRES=1` (or the older `SOUNDCRAFT_REQUIRE_WINRES=1`) turns it
//! into an error (for release builds).

/// Lists the launch splash's frames (`assets/splash/frames/f000.jpg`…, made by
/// `packaging/splash.py`) as `include_bytes!` in `$OUT_DIR/splash_frames.rs`, in name order, so
/// `splash.rs` embeds them without naming each one. The manifest directory is read at run time
/// (sccache may serve this script compiled in another worktree).
fn splash_frames() {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let dir = manifest.join("../../assets/splash/frames");
    let mut frames: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jpg")).collect())
        .unwrap_or_default();
    if frames.is_empty() {
        // Not a silent build without a splash (and no rerun-if-changed on a missing path, which
        // would rerun this script on every build).
        eprintln!("no splash frames in {}: run `python packaging/splash.py` (needs ffmpeg) to make them", dir.display());
        std::process::exit(1);
    }
    println!("cargo:rerun-if-changed={}", dir.display());
    frames.sort();
    let mut out = String::from("pub static FRAMES: &[&[u8]] = &[\n");
    for f in &frames {
        println!("cargo:rerun-if-changed={}", f.display());
        out.push_str(&format!("    include_bytes!({:?}),\n", f.display().to_string()));
    }
    out.push_str("];\n");
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("splash_frames.rs");
    if let Err(e) = std::fs::write(&path, out) {
        println!("cargo:warning=splash frame list not written: {e}");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/freedawm.ico");
    splash_frames();
    println!("cargo:rerun-if-env-changed=FREEDAWM_REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/freedawm.ico")
        .set("ProductName", "FreeDAWm")
        .set("FileDescription", "FreeDAWm digital audio workstation")
        .set("CompanyName", "Learning Machines LLC")
        .set("LegalCopyright", "Copyright (c) the SoundCraft authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "freedawm.exe")
        .set("InternalName", "freedawm");
    if let Err(e) = res.compile() {
        if std::env::var_os("FREEDAWM_REQUIRE_WINRES").or_else(|| std::env::var_os("SOUNDCRAFT_REQUIRE_WINRES")).is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=freedawm.exe built without icon/version resources: {e}");
    }
}
