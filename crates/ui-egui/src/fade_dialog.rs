//! Edit › Fades › Create…: a shape and bend for fade-ins and for fade-outs (a crossfade's
//! incoming and outgoing sides), where a crossfade sits against the cut (as selected, before,
//! across or after it), batch fades on clips standing inside the selection, a preview
//! of both curves, and presets (built-in ones and the user's, kept in `ui.json`).

use serde_json::{Value, json};
use soundcraft_model::{Fade, FadeShape};

/// A saved fade setting.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FadePreset {
    pub name: String,
    pub in_shape: FadeShape,
    pub in_curve: f32,
    pub out_shape: FadeShape,
    pub out_curve: f32,
}

/// The presets every session offers.
pub fn built_in() -> Vec<FadePreset> {
    let p = |name: &str, in_shape, in_curve, out_shape, out_curve| FadePreset { name: name.into(), in_shape, in_curve, out_shape, out_curve };
    vec![
        p("Equal power", FadeShape::EqualPower, 0.0, FadeShape::EqualPower, 0.0),
        p("Linear (equal gain)", FadeShape::Linear, 0.0, FadeShape::Linear, 0.0),
        p("S-curve", FadeShape::SCurve, 0.0, FadeShape::SCurve, 0.0),
        p("Fast in, slow out", FadeShape::Logarithmic, 0.0, FadeShape::Exponential, 0.0),
        p("Slow in, fast out", FadeShape::Exponential, 0.0, FadeShape::Logarithmic, 0.0),
    ]
}

/// The dialog's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct FadeDialog {
    pub in_shape: FadeShape,
    pub in_curve: f32,
    pub out_shape: FadeShape,
    pub out_curve: f32,
    /// Fade both ends of the clips that lie wholly inside the selection.
    pub batch: bool,
    pub batch_ms: f64,
    /// Where a crossfade sits against the cut: `selection` (as the selection lies over it),
    /// `before`, `centered` or `after` (the selection's length on that side).
    pub placement: &'static str,
    /// The name a new preset is saved under.
    pub name: String,
}

impl Default for FadeDialog {
    fn default() -> FadeDialog {
        FadeDialog {
            in_shape: FadeShape::EqualPower,
            in_curve: 0.0,
            out_shape: FadeShape::EqualPower,
            out_curve: 0.0,
            batch: false,
            batch_ms: 10.0,
            placement: "selection",
            name: String::new(),
        }
    }
}

impl FadeDialog {
    pub fn apply(&mut self, p: &FadePreset) {
        (self.in_shape, self.in_curve, self.out_shape, self.out_curve) = (p.in_shape, p.in_curve, p.out_shape, p.out_curve);
    }

    pub fn preset(&self, name: &str) -> FadePreset {
        FadePreset { name: name.into(), in_shape: self.in_shape, in_curve: self.in_curve, out_shape: self.out_shape, out_curve: self.out_curve }
    }

    /// The `edit.fades_create` parameters (on the current selection) at the session rate `sr`.
    pub fn params(&self, sr: f64) -> Value {
        let id = |s: FadeShape| s.label().to_lowercase();
        let mut v = json!({
            "shape": id(self.in_shape),
            "in_shape": id(self.in_shape),
            "out_shape": id(self.out_shape),
            "in_curvature": self.in_curve.clamp(-1.0, 1.0),
            "out_curvature": self.out_curve.clamp(-1.0, 1.0),
        });
        if self.placement != "selection" {
            v["placement"] = json!(self.placement);
        }
        if self.batch {
            v["batch_length"] = json!((self.batch_ms.clamp(0.0, 60_000.0) / 1000.0 * sr).round() as i64);
        }
        v
    }
}

/// The crossfade placements the dialog offers: id, label, hover text.
pub const PLACEMENTS: [(&str, &str, &str); 4] = [
    ("selection", "As Selected", "The crossfade covers the selection, wherever it lies over the cut"),
    ("before", "Before Cut", "The selection's length, ending at the cut (pre-crossfade)"),
    ("centered", "Across Cut", "The selection's length, centred on the cut"),
    ("after", "After Cut", "The selection's length, starting at the cut (post-crossfade)"),
];

/// Draws the dialog's fields; true when OK was pressed.
pub fn body(ui: &mut egui::Ui, d: &mut FadeDialog, presets: &mut Vec<FadePreset>, ok: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    ui.horizontal(|ui| {
        ui.label("Preset");
        egui::ComboBox::from_id_salt("fade_preset").selected_text("Choose…").show_ui(ui, |ui| {
            for p in built_in().iter().chain(presets.iter()) {
                if ui.selectable_label(false, &p.name).clicked() {
                    d.apply(p);
                }
            }
        });
    });
    for (label, shape, curve, salt) in
        [("Fade in", &mut d.in_shape, &mut d.in_curve, "fade_in_shape"), ("Fade out", &mut d.out_shape, &mut d.out_curve, "fade_out_shape")]
    {
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(salt).selected_text(shape.label()).show_ui(ui, |ui| {
                for s in FadeShape::ALL {
                    ui.selectable_value(shape, s, s.label());
                }
            });
            ui.add(egui::Slider::new(curve, -1.0..=1.0).text("bend"));
        });
    }
    preview(ui, d);
    ui.horizontal(|ui| {
        ui.label("Crossfade");
        for (id, label, tip) in PLACEMENTS {
            ui.selectable_value(&mut d.placement, id, label).on_hover_text(tip);
        }
    });
    ui.checkbox(&mut d.batch, "Also fade both ends of clips inside the selection");
    if d.batch {
        ui.add(egui::Slider::new(&mut d.batch_ms, 1.0..=1000.0).logarithmic(true).text("ms"));
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("Preset name").desired_width(140.0));
        let name = d.name.trim().to_string();
        if ui.add_enabled(!name.is_empty(), egui::Button::new("Save Preset")).clicked() {
            presets.retain(|p| p.name != name);
            presets.push(d.preset(&name));
        }
        if presets.iter().any(|p| p.name == name) && ui.button("Delete Preset").clicked() {
            presets.retain(|p| p.name != name);
        }
    });
    ok(ui)
}

/// Both curves as a crossfade would draw them: the fade-out falling, the fade-in rising.
fn preview(ui: &mut egui::Ui, d: &FadeDialog) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(240.0, 80.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_stroke(r, 2.0, egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
    let curve = |fade: Fade, rising: bool| -> Vec<egui::Pos2> {
        (0..=60)
            .map(|i| {
                let t = i as f32 / 60.0;
                let g = if rising { fade.gain(t) } else { fade.gain(1.0 - t) };
                egui::pos2(r.min.x + t * r.width(), r.max.y - g.clamp(0.0, 1.0) * r.height())
            })
            .collect()
    };
    let fin = Fade { len: 1, shape: d.in_shape, curvature: d.in_curve };
    let fout = Fade { len: 1, shape: d.out_shape, curvature: d.out_curve };
    p.add(egui::Shape::line(curve(fout, false), egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 150, 90))));
    p.add(egui::Shape::line(curve(fin, true), egui::Stroke::new(1.5, egui::Color32::from_rgb(110, 190, 240))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_asks_for_each_side_and_batch_fades() {
        let mut d = FadeDialog::default();
        d.apply(&built_in()[3]);
        d.batch = true;
        d.batch_ms = 10.0;
        let v = d.params(48_000.0);
        assert_eq!(v["in_shape"], "logarithmic");
        assert_eq!(v["out_shape"], "exponential");
        assert_eq!(v["batch_length"], 480);
        assert!(FadeShape::from_id(v["in_shape"].as_str().unwrap()).is_some());
        assert!(FadeShape::from_id(&FadeShape::EqualPower.label().to_lowercase()).is_some());
    }

    #[test]
    fn the_crossfade_placement_is_sent_only_when_chosen() {
        let mut d = FadeDialog::default();
        assert!(d.params(48_000.0).get("placement").is_none(), "as selected: the old request");
        d.placement = "before";
        assert_eq!(d.params(48_000.0)["placement"], "before");
        assert!(PLACEMENTS.iter().any(|p| p.0 == "centered") && PLACEMENTS.iter().any(|p| p.0 == "after"));
    }
}
