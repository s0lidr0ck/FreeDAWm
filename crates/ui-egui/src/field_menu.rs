//! The clip menu's Field Recording items, on an audio clip whose file has a timecode or a field
//! recorder's iXML: Expand to Location Channels (from the file's folder, or one chosen) and
//! Alternate Takes of its scene.

use crate::SoundApp;
use serde_json::json;
use soundcraft_model::ClipId;

/// The alternate takes listed for a clip, kept while its menu is open (they are found by reading
/// a folder's file headers, too slow for every frame): the clip, the session's revision, and
/// the takes or why there are none.
#[derive(Clone)]
struct Listed {
    clip: ClipId,
    revision: u64,
    takes: Result<Vec<(String, String)>, String>,
}

/// Whether clip `id` is from a location recording.
pub(crate) fn is_field_clip(app: &SoundApp, id: ClipId) -> bool {
    let s = app.engine.session();
    s.find_clip(id)
        .and_then(|(_, c)| c.source().filter(|_| c.is_audio()))
        .and_then(|src| s.source(src))
        .is_some_and(|src| src.time_reference > 0 || s.field_info.contains_key(&src.id.0))
}

fn report(app: &mut SoundApp, r: Result<serde_json::Value, String>) {
    match r {
        Ok(v) => {
            if let Some(n) = v["tracks"].as_array().map(Vec::len) {
                app.ui.status = format!("{n} location channel{} on new tracks", if n == 1 { "" } else { "s" });
            }
        }
        Err(e) => app.ui.status = e,
    }
}

pub(crate) fn clip_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: ClipId) {
    if !is_field_clip(app, id) {
        return;
    }
    ui.separator();
    ui.menu_button("Field Recording", |ui| {
        if ui
            .button("Expand to Location Channels")
            .on_hover_text("Each channel recorded at this clip's timecode, scene and take on a new track below (from its file's folder)")
            .clicked()
        {
            let r = app.run("clip.expand_channels", json!({"clip": id.0}));
            report(app, r);
            ui.close();
        }
        if app.services.pick_folder.is_some() && ui.button("Expand from Folder…").on_hover_text("The same, looking in another folder").clicked() {
            if let Some(dir) = app.services.pick_folder.as_ref().and_then(|pick| pick("field.folder")) {
                let r = app.run("clip.expand_channels", json!({"clip": id.0, "folder": dir}));
                report(app, r);
            }
            ui.close();
        }
        ui.menu_button("Alternate Takes", |ui| {
            let key = egui::Id::new("field_alternate_takes");
            let revision = app.engine.revision;
            let listed = match ui.data(|d| d.get_temp::<Listed>(key)) {
                Some(l) if l.clip == id && l.revision == revision => l,
                _ => {
                    let takes = app.run("clip.alternate_takes", json!({"clip": id.0})).map(|v| {
                        v["takes"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .map(|t| (t["take"].as_str().unwrap_or("").to_string(), t["name"].as_str().unwrap_or("").to_string()))
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                    let l = Listed { clip: id, revision, takes };
                    ui.data_mut(|d| d.insert_temp(key, l.clone()));
                    l
                }
            };
            match listed.takes {
                Err(why) => {
                    ui.label(why);
                }
                Ok(takes) if takes.is_empty() => {
                    ui.label("No other takes of this scene found");
                }
                Ok(takes) => {
                    for (take, name) in takes {
                        if ui.button(format!("Take {take}")).on_hover_text(name).clicked() {
                            let r = app.run("clip.use_alternate_take", json!({"clip": id.0, "take": take}));
                            report(app, r);
                            ui.close();
                        }
                    }
                }
            }
        });
    });
}
