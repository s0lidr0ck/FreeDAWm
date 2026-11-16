//! The VST-faithful plugin editors, one module per FireStation plugin, each a translation of that
//! plugin's JUCE editor built on [`crate::editor_kit`]. An engine registers its editor in its own
//! hooks file (`src/engines/<engine>.rs`: `wide_editor: Some(crate::editors::<engine>::draw)`);
//! one without opens the narrow drawer. This module list is written once; each plugin's task
//! fills only its own module.

pub mod amp;
pub mod chamber;
pub mod cleanq;
pub mod comp;
pub mod copyq;
pub mod delay;
pub mod dist;
pub mod filter;
pub mod levelup;
pub mod lull;
pub mod manners;
pub mod meters;
pub mod mod_engine;
pub mod pitch;
pub mod reverb;
pub mod rez;
pub mod sat;
pub mod shale;
pub mod sift;
pub mod sprawl;
pub mod swale;
pub mod tape;
pub mod transient;
pub mod twine;
pub mod vint;

use crate::SoundApp;
use egui::Ui;
use soundcraft_mix::StripMeter;
use soundcraft_model::{ModuleId, Track};

/// A wide editor's draw: the editor of `module` on `track` (see `editor_kit::run`).
pub type WideEditor = fn(&mut SoundApp, &mut Ui, &Track, ModuleId, Option<&StripMeter>);

/// Whether `plugin` opens a wide, VST-faithful editor (its engine's `EngineUi::wide_editor`).
pub fn has_wide(plugin: &str) -> bool {
    crate::engine_ui::engine_ui(plugin).is_some_and(|u| u.wide_editor.is_some())
}

/// Draws `module`'s wide editor and returns true, or returns false (drawing nothing) when its
/// plugin has none: the caller then draws the narrow drawer.
pub(crate) fn drawer(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) -> bool {
    let Some(ins) = track.module(module) else { return false };
    match crate::engine_ui::engine_ui(&ins.plugin).and_then(|u| u.wide_editor) {
        Some(draw) => {
            draw(app, ui, track, module, meters);
            true
        }
        None => false,
    }
}

/// What `plugin`'s editor keeps out of its presets besides Bypass (each editor's
/// `getPresetParams`): every save path leaves these out of a user preset and every load ignores
/// them, so a preset saved from the narrow drawer is the one the wide editor would write.
pub(crate) fn not_in_presets(plugin: &str) -> &'static [&'static str] {
    match plugin {
        "fs_amp" => &amp::NOT_IN_PRESETS,
        "fs_chamber" => chamber::FAMILY.not_in_presets,
        "fs_comp" => &comp::NOT_IN_PRESETS,
        "fs_copyq" => copyq::FAMILY.not_in_presets,
        "fs_delay" => delay::FAM.not_in_presets,
        "fs_dist" => dist::FAMILY.not_in_presets,
        "fs_filter" => filter::FAMILY.not_in_presets,
        "fs_levelup" => &levelup::NOT_IN_PRESETS,
        "lull" => lull::FAMILY.not_in_presets,
        "fs_manners" => manners::FAMILY.not_in_presets,
        "fs_mod" => mod_engine::FAMILY.not_in_presets,
        "fs_pitch" => pitch::FAMILY.not_in_presets,
        "fs_reverb" => reverb::FAM.not_in_presets,
        "rez" => rez::FAMILY.not_in_presets,
        "fs_sat" => sat::FAMILY.not_in_presets,
        "fs_shale" => shale::FAM.not_in_presets,
        "fs_sift" => sift::FAMILY.not_in_presets,
        "sprawl" => sprawl::FAMILY.not_in_presets,
        "fs_swale" => swale::FAM.not_in_presets,
        "fs_tape" => tape::FAMILY.not_in_presets,
        "fs_transient" => transient::FAMILY.not_in_presets,
        "fs_twine" => &twine::NOT_IN_PRESETS,
        "fs_vint" => vint::FAMILY.not_in_presets,
        _ => &[],
    }
}
