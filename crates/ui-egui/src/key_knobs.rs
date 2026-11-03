//! Which parameters a module tile shows as knobs by default.

use crate::engine_ui::engine_ui;
use soundcraft_dsp::{ParamInfo, PluginInfo, Unit};
use soundcraft_model::Insert;

/// The name a knob wears on a module tile: an explicit short label, else the parameter's name
/// without a lane letter ("A Amount" in Lane A reads "Amount"), shortened to fit.
///
/// An engine's explicit labels and own shortening come from its `EngineUi` (`tile_labels`,
/// `tile_label`): labels the general shortening would get wrong (two knobs read the same, or a
/// name cut to a word that says nothing), Amp's names without their amp.
pub fn tile_label(plugin: &str, p: &ParamInfo) -> String {
    let ui = engine_ui(plugin);
    match ui.and_then(|u| u.tile_labels.iter().find(|(id, _)| *id == p.id)) {
        Some((_, label)) => (*label).to_string(),
        None => match ui.and_then(|u| u.tile_label) {
            Some(f) => f(p),
            None => crate::chain_view::short_param(crate::module_drawer::hero_name(p)),
        },
    }
}

/// The name a tile's knob wears for this module: the name its engine's current model gives the
/// knob ([`crate::module_drawer::dial_name`]: Pitch's Dry is "Direct" on the OC-2, the RAT's Drive
/// "Distortion"), else [`tile_label`].
pub fn tile_knob_label(ins: &Insert, p: &ParamInfo) -> String {
    match crate::module_drawer::dial_name(ins, p.id) {
        Some(label) => crate::chain_view::short_param(label),
        None => tile_label(&ins.plugin, p),
    }
}

/// Whether a tile's knob does anything in the engine's current model: one the model hides in the
/// drawer (as the engine's editor does) is drawn dimmed and takes no input.
pub fn tile_knob_applies(ins: &Insert, id: &str) -> bool {
    crate::module_drawer::model_switch(ins, id) != Some(crate::module_drawer::Switch::Hide)
}

/// The key knobs of this module until the user picks others: [`default_key_knobs`], or where they
/// follow the engine's model, the model's own (Station's `kAmpModels`: each amp has its four).
pub fn key_knobs_for(ins: &Insert, info: &PluginInfo) -> Vec<&'static str> {
    match engine_ui(info.id).and_then(|u| u.model_key_knobs) {
        Some(f) => f(ins),
        None => default_key_knobs(info),
    }
}

/// The parameter ids a module tile shows as key knobs until the user picks others (a plugin whose
/// knobs follow its model gives its first model's: see [`key_knobs_for`]).
///
/// Third-party plugins (ids like `vst3:...`) have no static parameter list here: `[]`.
pub fn default_key_knobs(info: &PluginInfo) -> Vec<&'static str> {
    if info.id.contains(':') {
        return Vec::new();
    }
    match info.id {
        "compressor" => vec!["threshold", "ratio", "makeup"],
        // The curve is the main control; the output trim is the one knob.
        "eq_7band" => vec!["output_gain"],
        "channel_strip" => vec!["comp_threshold", "output_gain"],
        // FireStation engines: Station's own key knobs (`EngineUi::key_knobs`).
        _ => match engine_ui(info.id).map(|u| u.key_knobs).filter(|k| !k.is_empty()) {
            Some(keys) => keys.to_vec(),
            None => info.params.iter().filter(|p| p.unit != Unit::Toggle).take(3).map(|p| p.id).collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_graphs::{GraphKind, graph_kind};

    #[test]
    fn graph_kinds_and_key_knobs() {
        assert_eq!(graph_kind("eq_7band"), GraphKind::EqCurve);
        assert_eq!(graph_kind("eq_1band"), GraphKind::EqCurve);
        assert_eq!(graph_kind("channel_strip"), GraphKind::Compressor);
        assert_eq!(graph_kind("vst3:abc"), GraphKind::Level);
        let comp = soundcraft_dsp::plugin_info("compressor").unwrap();
        assert_eq!(default_key_knobs(comp), ["threshold", "ratio", "makeup"]);
        // every default key knob names a real param of every built-in
        for p in soundcraft_dsp::plugins() {
            for k in default_key_knobs(p) {
                assert!(p.params.iter().any(|x| x.id == k), "{} {k}", p.id);
            }
        }
    }

    #[test]
    fn firestation_engines_get_station_knobs_and_graphs() {
        let fs: Vec<_> = soundcraft_dsp::plugins().iter().filter(|p| p.category == soundcraft_dsp::Category::FireStation).collect();
        assert!(fs.len() >= 5, "the ported engines are registered");
        for p in fs {
            let knobs = default_key_knobs(p);
            assert!(knobs.len() >= 2, "{}: Station gives at least two", p.id);
            for k in &knobs {
                assert!(p.param(k).is_some(), "{} {k}", p.id);
            }
            assert_ne!(graph_kind(p.id), GraphKind::Level, "{} has its own graph", p.id);
        }
        assert_eq!(graph_kind("fs_cleanq"), GraphKind::CleanQ);
        assert_eq!(graph_kind("fs_comp"), GraphKind::Comp);
        assert_eq!(graph_kind("fs_sat"), GraphKind::Console);
        assert_eq!(graph_kind("fs_tape"), GraphKind::Console);
        assert_eq!(graph_kind("fs_twine"), GraphKind::Twine);
        assert_eq!(graph_kind("fs_vint"), GraphKind::Vint);
        assert_eq!(graph_kind("fs_pitch"), GraphKind::Pitch);
        let pitch = soundcraft_dsp::plugin_info("fs_pitch").unwrap();
        assert_eq!(default_key_knobs(pitch), ["dry", "octDown", "octUp", "level"]);
        assert_eq!(graph_kind("fs_mod"), GraphKind::Mod);
        let modi = soundcraft_dsp::plugin_info("fs_mod").unwrap();
        assert_eq!(default_key_knobs(modi), ["rate", "depth", "mix", "level"]);
        assert_eq!(graph_kind("fs_transient"), GraphKind::Transient);
        let transient = soundcraft_dsp::plugin_info("fs_transient").unwrap();
        assert_eq!(default_key_knobs(transient), ["attack", "sustain", "mix", "output"]);
        let labels: Vec<String> =
            default_key_knobs(transient).iter().filter_map(|k| transient.param(k)).map(|p| tile_label("fs_transient", p)).collect();
        assert_eq!(labels, ["Attack", "Sustain", "Mix", "Output"]);
        let vint = soundcraft_dsp::plugin_info("fs_vint").unwrap();
        assert_eq!(default_key_knobs(vint), ["drive", "mix", "output"]);
        assert_eq!(graph_kind("fs_dist"), GraphKind::Dist);
        let dist = soundcraft_dsp::plugin_info("fs_dist").unwrap();
        assert_eq!(default_key_knobs(dist), ["drive", "tone", "level"]);
        assert_eq!(graph_kind("fs_delay"), GraphKind::Delay);
        let delay = soundcraft_dsp::plugin_info("fs_delay").unwrap();
        assert_eq!(default_key_knobs(delay), ["time", "feedback", "mix", "level"]);
        assert_eq!(graph_kind("fs_reverb"), GraphKind::Reverb);
        let reverb = soundcraft_dsp::plugin_info("fs_reverb").unwrap();
        assert_eq!(default_key_knobs(reverb), ["decay", "size", "preDelay", "mix"]);
        let comp = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        assert_eq!(default_key_knobs(comp), ["amount", "ratio", "makeup", "dry"]);
        let cq = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
        // Station's three: the output trim and the dynamic ceiling's threshold and amount.
        assert_eq!(default_key_knobs(cq), ["outGain", "ceilThr", "ceilAmount"]);
        assert_eq!(default_key_knobs(cq), soundcraft_dsp::plugins::fs::cleanq::KEY_KNOBS);
    }

    #[test]
    fn tile_labels_are_distinct_for_every_plugin() {
        for p in soundcraft_dsp::plugins() {
            let labels: Vec<String> = default_key_knobs(p).iter().filter_map(|k| p.param(k)).map(|x| tile_label(p.id, x)).collect();
            for (i, a) in labels.iter().enumerate() {
                assert!(!labels[i + 1..].contains(a), "{}: two knobs read {a:?} ({labels:?})", p.id);
            }
        }
        let cq = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
        let tape = soundcraft_dsp::plugin_info("fs_tape").unwrap();
        assert_eq!(tile_label("fs_cleanq", cq.param("outGain").unwrap()), "Output");
        assert_eq!(tile_label("fs_cleanq", cq.param("ceilThr").unwrap()), "Thresh");
        assert_eq!(tile_label("fs_cleanq", cq.param("ceilAmount").unwrap()), "Amount");
        assert_eq!(tile_label("fs_tape", tape.param("flutter").unwrap()), "W&F");
        let comp = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        assert_eq!(tile_label("fs_comp", comp.param("amount").unwrap()), "Amount");
    }

    #[test]
    fn tile_knobs_follow_the_model() {
        let pitch = soundcraft_dsp::plugin_info("fs_pitch").unwrap();
        let mut ins = Insert::new("fs_pitch");
        let dry = pitch.param("dry").unwrap();
        assert_eq!(tile_knob_label(&ins, dry), "Dry");
        assert!(default_key_knobs(pitch).iter().all(|k| tile_knob_applies(&ins, k)), "POG uses all four");
        ins.params.insert("style".into(), 2.0);
        assert_eq!(tile_knob_label(&ins, dry), "Direct");
        assert!(!tile_knob_applies(&ins, "octUp"), "the OC-2 has no octave up");
        assert!(tile_knob_applies(&ins, "octDown"));
        ins.params.insert("style".into(), 4.0);
        assert!(!tile_knob_applies(&ins, "octDown") && !tile_knob_applies(&ins, "octUp"));
        assert!(tile_knob_applies(&ins, "dry") && tile_knob_applies(&ins, "level"));
        // Engines without model switches are untouched.
        let comp = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        let c = Insert::new("fs_comp");
        assert_eq!(tile_knob_label(&c, comp.param("amount").unwrap()), "Amount");
        assert!(tile_knob_applies(&c, "amount"));
    }

    #[test]
    fn filter_tile_knobs_follow_the_circuit() {
        assert_eq!(graph_kind("fs_filter"), GraphKind::Filter);
        let filter = soundcraft_dsp::plugin_info("fs_filter").unwrap();
        assert_eq!(default_key_knobs(filter), ["cutoff", "reso", "drive", "mix"]);
        let labels =
            |ins: &Insert| -> Vec<String> { default_key_knobs(filter).iter().map(|k| tile_knob_label(ins, filter.param(k).unwrap())).collect() };
        let mut ins = Insert::new("fs_filter");
        // FilterConfig.h's dial names: the Q-Tron's Sweep and Peak, the wahs' Pedal and Q.
        assert_eq!(labels(&ins), ["Sweep", "Peak", "Drive", "Mix"]);
        ins.params.insert("circuit".into(), 2.0);
        assert_eq!(labels(&ins), ["Pedal", "Q", "Drive", "Mix"]);
        for circuit in 0..9 {
            ins.params.insert("circuit".into(), circuit as f32);
            let l = labels(&ins);
            for (i, a) in l.iter().enumerate() {
                assert!(!l[i + 1..].contains(a), "circuit {circuit}: {l:?}");
            }
            // Station's four always apply, whatever the circuit or source.
            for src in 0..4 {
                ins.params.insert("modSrc".into(), src as f32);
                assert!(default_key_knobs(filter).iter().all(|k| tile_knob_applies(&ins, k)), "circuit {circuit}, source {src}");
            }
        }
    }

    #[test]
    fn chamber_tile_knobs_hero_and_graph() {
        assert_eq!(graph_kind("fs_chamber"), GraphKind::Chamber);
        let chamber = soundcraft_dsp::plugin_info("fs_chamber").unwrap();
        // Station's `list ("mix,decay,predelay,level")`.
        assert_eq!(default_key_knobs(chamber), ["mix", "decay", "predelay", "level"]);
        let mut ins = Insert::new("fs_chamber");
        let labels: Vec<String> = default_key_knobs(chamber).iter().map(|k| tile_knob_label(&ins, chamber.param(k).unwrap())).collect();
        assert_eq!(labels, ["Mix", "Decay", "Pre-Delay", "Level"]);
        assert!(default_key_knobs(chamber).iter().all(|k| tile_knob_applies(&ins, k)));
        assert_eq!(crate::module_drawer::hero_label(&ins, chamber.param("decay").unwrap()), "DECAY");
        assert_eq!(crate::theme::station_accent("fs_chamber"), Some(egui::Color32::from_rgb(0xFF, 0x9A, 0x3D)));
        // An empty object slot's position, size and rotate are hidden; a placed pot shows its
        // position and size but not Rotate (it has no face); a panel shows Rotate too.
        use crate::module_drawer::{Switch, model_switch};
        for id in ["o3x", "o3y", "o3s", "o3r"] {
            assert_eq!(model_switch(&ins, id), Some(Switch::Hide), "{id}");
        }
        assert_eq!(model_switch(&ins, "o3t"), None, "the type is always there");
        assert_eq!(model_switch(&ins, "mix"), None);
        ins.params.insert("o3t".into(), 2.0);
        assert_eq!(model_switch(&ins, "o3x"), None);
        assert_eq!(model_switch(&ins, "o3s"), None);
        assert_eq!(model_switch(&ins, "o3r"), Some(Switch::Hide));
        ins.params.insert("o3t".into(), 3.0);
        assert_eq!(model_switch(&ins, "o3r"), None);
        assert_eq!(model_switch(&ins, "o4x"), Some(Switch::Hide));
    }

    #[test]
    fn amp_knobs_follow_the_amp() {
        use soundcraft_dsp::plugins::fs::amp::{KEY_KNOBS, NUM_AMPS};
        assert_eq!(graph_kind("fs_amp"), GraphKind::Amp);
        let amp = soundcraft_dsp::plugin_info("fs_amp").unwrap();
        let mut ins = Insert::new("fs_amp");
        assert_eq!(key_knobs_for(&ins, amp), ["volume", "treble", "middle", "bass"]);
        let want: [&[&str]; NUM_AMPS] = [
            &["Volume", "Treble", "Middle", "Bass"],
            &["Volume I", "Treble", "Middle", "Bass"],
            &["Volume", "Treble", "Bass", "Reverb"],
            &["Volume", "Treble", "Middle", "Bass"],
            &["Volume", "Treble", "Bass", "Reverb"],
            &["Brilliant", "Treble", "Bass", "Cut"],
            &["Gain", "Bass", "Treble", "HF Drive"],
        ];
        for (a, labels) in want.iter().enumerate() {
            ins.params.insert("amp".into(), a as f32);
            let knobs = key_knobs_for(&ins, amp);
            assert_eq!(knobs, KEY_KNOBS[a]);
            let got: Vec<String> = knobs.iter().map(|k| tile_knob_label(&ins, amp.param(k).unwrap())).collect();
            assert_eq!(got, *labels, "amp {a}");
            assert!(knobs.iter().all(|k| tile_knob_applies(&ins, k)), "amp {a}: its own knobs are live");
            // another amp's knobs are dimmed
            let other = KEY_KNOBS[(a + 1) % NUM_AMPS];
            assert!(other.iter().filter(|k| !knobs.contains(k)).all(|k| !tile_knob_applies(&ins, k)), "amp {a}");
            let tiles: Vec<String> = knobs.iter().map(|k| tile_label("fs_amp", amp.param(k).unwrap())).collect();
            for (i, x) in tiles.iter().enumerate() {
                assert!(!tiles[i + 1..].contains(x), "amp {a}: {tiles:?}");
            }
        }
    }

    #[test]
    fn third_party_has_no_default_knobs() {
        let fake = PluginInfo {
            id: "vst3:abc",
            name: "X",
            short_name: "X",
            category: soundcraft_dsp::Category::Other,
            params: &[],
            is_instrument: false,
            audiosuite: false,
            sidechain: false,
            link: None,
            references: 0,
            midi_out: false,
        };
        assert!(default_key_knobs(&fake).is_empty());
    }
}
