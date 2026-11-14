//! The plugin browser's catalog: categories, filters and scan failures.

use soundcraft_au_host::AuDescriptor;
use soundcraft_clap_host::ClapDescriptor;
use soundcraft_dsp::{Category, PluginInfo};
use soundcraft_ui_egui::browser::catalog::{
    BrowserCategory, BrowserItem, Format, Kind, Query, ScanFailure, build_catalog, category_for_builtin, category_for_clap, category_for_vst3, filter,
};
use soundcraft_vst3_host::Vst3Descriptor;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

#[test]
fn vst3_subcategories_map() {
    use BrowserCategory as C;
    let cases: &[(&[&str], bool, C)] = &[
        (&["Fx", "Reverb"], false, C::Reverb),
        (&["Instrument", "Synth"], true, C::Synth),
        (&["Instrument", "Piano"], true, C::Keys),
        (&["Instrument", "Drum"], true, C::Drums),
        (&["Instrument", "Sampler"], true, C::Sampler),
        (&["Fx", "Dynamics"], false, C::Dynamics),
        (&["Fx", "EQ"], false, C::Eq),
        (&["Fx", "Distortion"], false, C::Distortion),
        (&["Fx", "Analyzer"], false, C::Analyzer),
        (&["Fx", "Whatever"], false, C::Other),
        (&["Instrument", "Whatever"], true, C::Synth),
    ];
    for (subs, inst, want) in cases {
        assert_eq!(category_for_vst3(&s(subs), *inst), *want, "{subs:?}");
    }
}

#[test]
fn clap_features_map() {
    use BrowserCategory as C;
    assert_eq!(category_for_clap(&s(&["audio-effect", "reverb"]), false), C::Reverb);
    assert_eq!(category_for_clap(&s(&["instrument", "synthesizer"]), true), C::Synth);
    assert_eq!(category_for_clap(&s(&["instrument", "piano"]), true), C::Keys);
    assert_eq!(category_for_clap(&s(&["audio-effect", "compressor"]), false), C::Dynamics);
}

#[test]
fn builtins_and_firestation() {
    let builtins = soundcraft_dsp::plugins();
    let cat = build_catalog(builtins, &[], &[], &[], Vec::new());
    // The hidden stock effects are not offered; every listed built-in is, once.
    assert_eq!(cat.items.len(), builtins.iter().filter(|p| p.listed()).count());
    assert!(cat.items.iter().all(|i| !["compressor", "eq_7band", "room_reverb", "dither"].contains(&i.id.as_str())));
    for info in builtins.iter().filter(|p| p.listed()) {
        let found: Vec<&BrowserItem> = cat.items.iter().filter(|i| i.id == info.id).collect();
        assert_eq!(found.len(), 1, "{}", info.id);
        let item = found[0];
        if info.category == Category::FireStation {
            assert_eq!(item.format, Format::FireStation, "{}", info.id);
            assert_eq!(item.vendor, "FireStation");
        } else {
            assert_eq!(item.format, Format::BuiltIn, "{}", info.id);
            assert_eq!(item.vendor, "FreeDAWm");
        }
        let want = if info.is_instrument { Kind::Instrument } else { Kind::Effect };
        assert_eq!(item.kind, want, "{}", info.id);
        assert_eq!(item.category, category_for_builtin(info));
    }
    let by_id = |id: &str| soundcraft_dsp::plugin_info(id).map(category_for_builtin);
    // Every FireStation engine, explicitly: a new engine or a changed mapping fails here.
    use BrowserCategory as C;
    let fs: &[(&str, C)] = &[
        ("fs_amp", C::Distortion),
        ("fs_chamber", C::Reverb),
        ("fs_cleanq", C::Eq),
        ("fs_comp", C::Dynamics),
        ("fs_delay", C::Delay),
        ("fs_dist", C::Distortion),
        ("fs_filter", C::Modulation),
        ("fs_levelup", C::Utility),
        ("fs_manners", C::Eq),
        ("fs_mod", C::Modulation),
        ("fs_pitch", C::Pitch),
        ("fs_reverb", C::Reverb),
        ("fs_sat", C::Distortion),
        ("fs_shale", C::Dynamics),
        ("fs_swale", C::Dynamics),
        ("fs_tape", C::Distortion),
        ("fs_transient", C::Dynamics),
        ("fs_twine", C::Dynamics),
        ("fs_vint", C::Eq),
    ];
    for (id, want) in fs {
        assert_eq!(by_id(id), Some(*want), "{id}");
    }
    let mut registered: Vec<&str> = builtins.iter().filter(|i| i.category == Category::FireStation).map(|i| i.id).collect();
    registered.sort_unstable();
    let listed: Vec<&str> = fs.iter().map(|(id, _)| *id).collect();
    assert_eq!(registered, listed, "every FireStation engine needs a row above");
    assert_eq!(by_id("sampler"), Some(BrowserCategory::Sampler));
    assert_eq!(by_id("drum_synth"), Some(BrowserCategory::Drums));
    assert_eq!(by_id("subtractive_synth"), Some(BrowserCategory::Synth));
    assert_eq!(by_id("dither"), Some(BrowserCategory::Utility));
    assert_eq!(by_id("pitch_shifter"), Some(BrowserCategory::Pitch));
}

fn item(id: &str, name: &str, vendor: &str, category: BrowserCategory) -> BrowserItem {
    BrowserItem {
        id: id.into(),
        name: name.into(),
        vendor: vendor.into(),
        version: String::new(),
        format: Format::Vst3,
        kind: Kind::Effect,
        category,
    }
}

fn query() -> Query {
    Query { kind: Kind::Effect, text: String::new(), favourites: false, recent: false, category: None, vendor: None }
}

#[test]
fn filter_search_and_chips() {
    use BrowserCategory as C;
    let items = vec![
        item("a", "Big Hall", "Acme", C::Reverb),
        item("b", "Plateverb", "Zed", C::Other),
        item("c", "Squash", "Acme", C::Dynamics),
        item("d", "Echo", "Zed", C::Delay),
        BrowserItem { kind: Kind::Instrument, ..item("e", "Verbose Synth", "Acme", C::Synth) },
    ];
    let none = BTreeMap::new();
    let ids = |v: Vec<usize>| v.into_iter().map(|i| items[i].id.as_str()).collect::<Vec<_>>();

    // Text: by name ("Plateverb") and by category label ("Reverb"); the instrument tab is separate.
    let q = Query { text: "VERB".into(), ..query() };
    assert_eq!(ids(filter(&items, &q, &none, &[], &[])), vec!["a", "b"]);
    // Vendor in the text.
    let q = Query { text: "zed".into(), ..query() };
    assert_eq!(ids(filter(&items, &q, &none, &[], &[])), vec!["d", "b"]);

    // No filter: favourites first, then by name.
    let favs = vec!["c".to_string()];
    assert_eq!(ids(filter(&items, &query(), &none, &favs, &[])), vec!["c", "a", "d", "b"]);
    // Favourites chip.
    let q = Query { favourites: true, ..query() };
    assert_eq!(ids(filter(&items, &q, &none, &favs, &[])), vec!["c"]);
    // Vendor chip.
    let q = Query { vendor: Some("Acme".into()), ..query() };
    assert_eq!(ids(filter(&items, &q, &none, &[], &[])), vec!["a", "c"]);
    // Category chip with an override recategorising "b" as a reverb.
    let mut over = BTreeMap::new();
    over.insert("b".to_string(), C::Reverb);
    let q = Query { category: Some(C::Reverb), ..query() };
    assert_eq!(ids(filter(&items, &q, &none, &[], &[])), vec!["a"]);
    assert_eq!(ids(filter(&items, &q, &over, &[], &[])), vec!["a", "b"]);
    // Recent: newest first (index 0 of `recent` is the newest), only the recent ones.
    let recent = vec!["d".to_string(), "a".to_string(), "e".to_string()];
    let q = Query { recent: true, ..query() };
    assert_eq!(ids(filter(&items, &q, &none, &favs, &recent)), vec!["d", "a"]);
    // Instrument tab.
    let q = Query { kind: Kind::Instrument, ..query() };
    assert_eq!(ids(filter(&items, &q, &none, &[], &[])), vec!["e"]);
}

#[test]
fn failures_counted() {
    let clap = ClapDescriptor {
        id: "clap:com.acme.verb".into(),
        plugin_id: "com.acme.verb".into(),
        name: "Acme Verb".into(),
        vendor: "Acme".into(),
        version: "1.0".into(),
        description: String::new(),
        features: s(&["audio-effect", "reverb"]),
        category: Category::Reverb,
        is_instrument: false,
        path: PathBuf::from("acme.clap"),
    };
    let vst3 = Vst3Descriptor {
        id: "vst3:00000000000000000000000000000001".into(),
        class_id: "00000000000000000000000000000001".into(),
        name: "Keys One".into(),
        vendor: "Zed".into(),
        version: "2.0".into(),
        sdk_version: String::new(),
        sub_categories: s(&["Instrument", "Piano"]),
        category: Category::Instrument,
        is_instrument: true,
        path: PathBuf::from("keys.vst3"),
    };
    let au = AuDescriptor {
        id: "au:aufx:dely:Acme".into(),
        name: "Tape Echo".into(),
        vendor: "Acme".into(),
        version: "1.0.0".into(),
        component_type: "aufx".into(),
        subtype: "dely".into(),
        manufacturer: "Acme".into(),
        category: Category::Delay,
        is_instrument: false,
    };
    let failures = vec![ScanFailure { path: "broken.vst3".into(), reason: "cannot load".into() }];
    let builtins: &[&PluginInfo] = &[];
    let cat = build_catalog(builtins, &[clap], &[vst3], &[au], failures.clone());
    assert_eq!(cat.failures, failures);
    assert_eq!(cat.items.len(), 3);
    assert!(cat.items.iter().all(|i| i.id != "broken.vst3"));
    let get = |id: &str| cat.items.iter().find(|i| i.id == id).cloned();
    let c = get("clap:com.acme.verb").map(|i| (i.format, i.kind, i.category, i.vendor, i.version));
    assert_eq!(c, Some((Format::Clap, Kind::Effect, BrowserCategory::Reverb, "Acme".into(), "1.0".into())));
    let v = get("vst3:00000000000000000000000000000001").map(|i| (i.format, i.kind, i.category));
    assert_eq!(v, Some((Format::Vst3, Kind::Instrument, BrowserCategory::Keys)));
    let a = get("au:aufx:dely:Acme").map(|i| (i.format, i.kind, i.category));
    assert_eq!(a, Some((Format::Au, Kind::Effect, BrowserCategory::Delay)));
}

#[test]
fn category_labels_and_serde() {
    assert_eq!(BrowserCategory::ALL.len(), 14);
    assert_eq!(BrowserCategory::Eq.label(), "EQ");
    for c in BrowserCategory::ALL {
        let j = serde_json::to_string(&c).unwrap_or_default();
        let back: Option<BrowserCategory> = serde_json::from_str(&j).ok();
        assert_eq!(back, Some(c));
    }
}
