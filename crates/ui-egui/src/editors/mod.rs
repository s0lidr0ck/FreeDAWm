//! The VST-faithful plugin editors, one module per FireStation plugin, each a translation of that
//! plugin's JUCE editor built on [`crate::editor_kit`]. An engine registers its editor in its own
//! hooks file (`src/engines/<engine>.rs`: `wide_editor: Some(crate::editors::<engine>::draw)`);
//! one without opens the narrow drawer. This module list is written once; each plugin's task
//! fills only its own module.

pub mod amp;
pub mod chamber;
pub mod cleanq;
pub mod comp;
pub mod delay;
pub mod dist;
pub mod filter;
pub mod levelup;
pub mod manners;
pub mod mod_engine;
pub mod pitch;
pub mod reverb;
pub mod rez;
pub mod sat;
pub mod shale;
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
