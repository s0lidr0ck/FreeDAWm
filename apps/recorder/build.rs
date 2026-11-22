//! Windows only: embed the app icon and version info (VERSIONINFO) into `freedawm-recorder.exe`,
//! as `apps/soundcraft/build.rs` does for `freedawm.exe`. On every other target this does
//! nothing. A missing resource compiler is a warning unless `FREEDAWM_REQUIRE_WINRES=1` (release
//! builds) makes it an error.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/freedawm.ico");
    println!("cargo:rerun-if-env-changed=FREEDAWM_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/freedawm.ico")
        .set("ProductName", "FreeDAWm Recorder")
        .set("FileDescription", "FreeDAWm Recorder")
        .set("CompanyName", "Learning Machines LLC")
        .set("LegalCopyright", "Copyright (c) the SoundCraft authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "freedawm-recorder.exe")
        .set("InternalName", "freedawm-recorder");
    if let Err(e) = res.compile() {
        if std::env::var_os("FREEDAWM_REQUIRE_WINRES").is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=freedawm-recorder.exe built without icon/version resources: {e}");
    }
}
