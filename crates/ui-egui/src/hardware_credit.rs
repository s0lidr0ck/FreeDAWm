//! The credit line shown with a mode, circuit, preset or impulse response named after a piece of
//! hardware (NOTICE has the full trademark notice, and the About box a short one).
//!
//! The wording is fixed (owner's decision, 2026-11-02): "Component-level model of the <hardware>."
//! only where the DSP solves that circuit's own parts (from its schematic: part values, tubes,
//! transistors, the timing network); "Modelled after the <hardware>." for a behavioural or
//! curve approximation. Then "No affiliation; <brand> is a trademark of its owner."
//!
//! Where it shows: on hover over a full editor's mode description and over each entry of a slim
//! editor's model menu (and after the description in the closed menu's hover), on hover over the
//! amp cabinet IR picker, and after a factory preset's description in the preset browser.

/// `blurb` and its credit line, as one description.
pub fn with_credit(blurb: &str, credit: &str) -> String {
    match (blurb.is_empty(), credit.is_empty()) {
        (_, true) => blurb.to_string(),
        (true, false) => credit.to_string(),
        (false, false) => format!("{blurb} {credit}"),
    }
}

/// The description in `rect` shows itself and its credit on hover (nothing when there is no
/// credit).
pub fn hover(ui: &egui::Ui, rect: egui::Rect, id_salt: &str, blurb: &str, credit: &str) {
    if credit.is_empty() || !rect.is_positive() {
        return;
    }
    let resp = ui.interact(rect, ui.id().with(("hardware_credit", id_salt)), egui::Sense::hover());
    resp.on_hover_text(with_credit(blurb, credit));
}

/// The bundled amp cabinet impulse responses' folders (`assets/amp-irs/<folder>/`).
pub fn ir_folder_credit(folder: &str) -> &'static str {
    match folder {
        "Fender" => "Measured from a 1971 Fender Twin Reverb 2x12 cabinet. No affiliation; Fender and Twin Reverb are trademarks of their owner.",
        "Marshall" => "Measured from a Marshall-style 4x12 cabinet. No affiliation; Marshall is a trademark of its owner.",
        _ => "",
    }
}

/// The credit lines a FireStation factory preset's description gets: one per piece of hardware
/// its name names (word for word: "Vocal Chain 1176 into LA-2A" gets both), for the engines
/// whose modes are named after hardware.
pub fn preset_credit(plugin: &str, name: &str) -> String {
    let padded = format!(" {name} ");
    let has = |word: &str| padded.contains(&format!(" {word} "));
    let mut out: Vec<&'static str> = Vec::new();
    match plugin {
        "fs_comp" => {
            for style in ["1176", "LA-2A", "160", "G Bus", "Vari-Mu", "EL8", "2500", "33609"] {
                if has(style) {
                    out.push(crate::editors::comp::style_credit(style));
                }
            }
            if has("Dyna") || has("Ross") {
                out.push(crate::editors::comp::style_credit("Ross"));
            }
        }
        "fs_amp" => {
            for (i, amp) in soundcraft_dsp::plugins::fs::amp::AMP_NAMES.iter().enumerate() {
                if has(amp) {
                    out.push(crate::editors::amp::amp_credit(i));
                }
            }
        }
        "fs_vint" => {
            for (word, circuit) in
                [("Pultec", "Passive"), ("Dolby", "Air"), ("Neve", "Console"), ("1073", "Console"), ("SSL", "British"), ("API", "Proportional")]
            {
                let credit = crate::editors::vint::circuit_credit(circuit);
                if has(word) && !out.contains(&credit) {
                    out.push(credit);
                }
            }
        }
        _ => {}
    }
    out.retain(|c| !c.is_empty());
    out.dedup();
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_follows_the_blurb() {
        assert_eq!(with_credit("Blurb.", ""), "Blurb.");
        assert_eq!(with_credit("", "Credit."), "Credit.");
        assert_eq!(with_credit("Blurb.", "Credit."), "Blurb. Credit.");
    }

    #[test]
    fn ir_folders_have_credits() {
        assert!(ir_folder_credit("Fender").contains("No affiliation"));
        assert!(ir_folder_credit("Marshall").contains("No affiliation"));
        assert_eq!(ir_folder_credit("Mine"), "");
    }

    #[test]
    fn presets_named_after_hardware_get_its_credit() {
        let both = preset_credit("fs_comp", "Vocal Chain 1176 into LA-2A");
        assert!(both.contains("UREI 1176") && both.contains("Teletronix LA-2A"), "{both}");
        assert!(preset_credit("fs_comp", "Dyna Squash").contains("Ross"));
        assert_eq!(preset_credit("fs_comp", "Dynamic Solo"), "", "whole words only");
        assert!(preset_credit("fs_amp", "Twin Surf Drip").contains("Fender Twin Reverb"));
        assert!(preset_credit("fs_amp", "JTM45 Full Tilt").contains("Marshall"));
        assert!(preset_credit("fs_vint", "Kick Pultec Trick").contains("Pultec"));
        assert!(preset_credit("fs_vint", "Dolby Air").starts_with("Modelled after"));
        assert_eq!(preset_credit("fs_chamber", "Gold Plaster Twin"), "", "only the amp's presets name amps");
    }
}
