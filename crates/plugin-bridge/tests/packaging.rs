//! The helper ships with the app: every package build builds it and puts it next to the app,
//! where `helper_path` looks for it (the owner: the helper ships together with its option).

use std::path::Path;

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn every_package_builds_and_carries_the_helper() {
    for script in [
        "packaging/linux/package.sh",
        "packaging/linux/flatpak/io.github.s0lidr0ck.FreeDAWm.yml",
        "packaging/macos/package.sh",
        "packaging/freebsd/package.sh",
        "packaging/windows/package.ps1",
    ] {
        let s = read(script);
        assert!(s.contains("-p soundcraft-plugin-bridge"), "{script} does not build the helper");
        assert!(s.contains("freedawm-plugin-host"), "{script} does not package the helper");
    }
    for manifest in ["packaging/linux/nfpm.yaml", "packaging/linux/flatpak/io.github.s0lidr0ck.FreeDAWm.bundle.yml", "packaging/windows/freedawm.wxs"]
    {
        assert!(read(manifest).contains("freedawm-plugin-host"), "{manifest} leaves the helper out");
    }
    // Signed with the others on macOS and Windows.
    assert!(
        read("packaging/macos/package.sh")
            .contains("sign --options runtime --entitlements \"$HERE/entitlements.plist\" \"$APP/Contents/MacOS/freedawm-plugin-host\"")
    );
    assert!(read("packaging/windows/package.ps1").contains("(Join-Path $Stage 'freedawm-plugin-host.exe')"));
}

#[test]
fn the_helper_is_named_as_the_app_looks_for_it() {
    assert_eq!(soundcraft_plugin_bridge::HELPER_NAME, "freedawm-plugin-host");
    assert!(env!("CARGO_BIN_EXE_freedawm-plugin-host").contains(soundcraft_plugin_bridge::HELPER_NAME));
}
