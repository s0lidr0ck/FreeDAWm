//! Lists the built-in programs (`IUnitInfo`) of chosen installed VST3 plugins, loading only those
//! bundles in this process. Run with
//! `FREEDAWM_PROGRAM_BUNDLES="C:\...\A.vst3;C:\...\B.vst3" cargo test -p soundcraft-vst3-host
//! --test real_programs -- --ignored --nocapture`.

#[test]
#[ignore = "loads the installed plugins named in FREEDAWM_PROGRAM_BUNDLES"]
fn installed_plugins_programs() {
    let list = std::env::var("FREEDAWM_PROGRAM_BUNDLES").or_else(|_| std::env::var("SOUNDCRAFT_PROGRAM_BUNDLES")).unwrap_or_default();
    for path in list.split(';').filter(|p| !p.trim().is_empty()) {
        // Only this bundle is scanned (in process): a folder holding just a link to it (a copy of a
        // single-file plugin).
        let src = std::path::Path::new(path.trim());
        let dir = std::env::temp_dir().join(format!("sc-programs-{}-{}", std::process::id(), src.file_stem().unwrap().to_string_lossy()));
        let _ = std::fs::create_dir_all(&dir);
        let link = dir.join(src.file_name().unwrap());
        if !link.exists() {
            let made = if src.is_dir() {
                std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(src).status().is_ok_and(|s| s.success())
            } else {
                std::fs::copy(src, &link).is_ok()
            };
            if !made {
                println!("{path}: could not link it");
                continue;
            }
        }
        soundcraft_vst3_host::add_search_dir(&dir);
        let descs = soundcraft_vst3_host::load_bundle(&link).unwrap_or_default();
        for d in descs {
            if let Err(e) = soundcraft_vst3_host::instantiate(&d.id) {
                println!("  instantiate: {e}");
            }
            let info = soundcraft_vst3_host::plugin_info(&d.id);
            let p = soundcraft_vst3_host::programs(&d.id);
            let shown = p.as_ref().map(|p| {
                (
                    p.param.clone(),
                    p.names.len(),
                    p.names.iter().take(5).cloned().collect::<Vec<_>>(),
                    p.values.iter().take(5).copied().collect::<Vec<_>>(),
                )
            });
            println!("{} ({}): info {}, programs {shown:?}", d.name, d.id, info.is_some());
        }
    }
}
