//! The Link calibration wizard DistEngine and ModEngine share (`calStart`, `calRender`,
//! `calToggleAB`, `calNext`, `calEnd` and the panel `paint()` draws): four steps that set Input,
//! play the dry tone, then store the Level set points at the drive knob's two Link anchors.
//!
//! The C++ suspends Link while the wizard runs (`processor.setCalibrationActive`), so Level can be
//! set by hand. In the DAW, Link is derived by the command layer, so the wizard takes `link` off
//! while it runs and puts it back after (on when it completes, as `calEnd (true)`); the editor
//! shows the Link pill in the state it will return to ([`Cal::saved_link`]).

use super::controls::{PillLook, PillStyle, pill_ex, pill_height, pill_width_ex};
use super::look::{TINY_SIZE, font, glow_rect};
use super::palette::VstPalette;
use super::{Host, end_gesture, one_step, param, set_params_merged};
use crate::SoundApp;
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// One engine's wizard (`currentCircuit()`'s Link facts and `kCalInputText`).
#[derive(Clone, Debug)]
pub struct CalSpec<'a> {
    /// The drive-role parameter ("drive"; ModEngine: "depth").
    pub drive: &'a str,
    /// What the drive knob is called here (`driveName`).
    pub drive_name: &'a str,
    /// The Drive positions the Link curve is pinned at.
    pub anchor_lo: f32,
    pub anchor_hi: f32,
    /// The parameters holding the two Level set points.
    pub link_lo: &'a str,
    pub link_hi: &'a str,
    /// Step 1's body.
    pub input_text: String,
}

/// `CalState`, plus the Link state the wizard returns to.
#[derive(Clone, Copy, Default, Debug)]
pub struct Cal {
    pub active: bool,
    pub ab: bool,
    pub step: i32,
    saved_drive: f32,
    saved_bypass: f32,
    lo_level: f32,
    hi_temp: f32,
    has_hi_temp: bool,
    pub saved_link: f32,
}

impl Cal {
    /// `inputDial.setCalFocus (step == 1)`.
    pub fn input_focus(&self) -> bool {
        self.active && self.step == 1
    }
    /// `levelDial.setCalFocus (step >= 3)`.
    pub fn level_focus(&self) -> bool {
        self.active && self.step >= 3
    }
}

/// `setParam`: one change, outside the editor's undo stack.
fn put(app: &mut SoundApp, host: &Host, list: &[(&str, f32)]) {
    one_step(app, |app| set_params_merged(app, host, list, "calibrate"));
}

/// `calStart`.
pub(crate) fn start(app: &mut SoundApp, host: &Host, cal: &mut Cal, spec: &CalSpec) {
    *cal = Cal {
        active: true,
        ab: false,
        step: 1,
        saved_drive: param(host, spec.drive),
        saved_bypass: param(host, "bypass"),
        lo_level: param(host, spec.link_lo),
        hi_temp: 0.5,
        has_hi_temp: false,
        saved_link: param(host, "link"),
    };
    // `setCalibrationActive (true)`: Link stops driving Level while the wizard runs.
    put(app, host, &[("link", 0.0)]);
    render(app, host, cal, spec);
}

/// `calRender`: what each step sets.
fn render(app: &mut SoundApp, host: &Host, cal: &mut Cal, spec: &CalSpec) {
    cal.ab = false;
    match cal.step {
        1 => put(app, host, &[("bypass", 0.0), (spec.drive, 0.5)]),
        2 => put(app, host, &[("bypass", 1.0)]),
        3 => put(app, host, &[("bypass", 0.0), (spec.drive, spec.anchor_lo)]),
        _ => put(app, host, &[("bypass", 0.0), (spec.drive, spec.anchor_hi)]),
    }
}

/// The step's title, body, A/B label (steps 3 and 4) and Next label.
pub fn texts(step: i32, spec: &CalSpec) -> (String, String, Option<String>, &'static str) {
    let n = spec.drive_name;
    let lo = (spec.anchor_lo * 10.0).round() as i32;
    let hi = (spec.anchor_hi * 10.0).round() as i32;
    match step {
        1 => ("Set Input for your guitar".into(), spec.input_text.clone(), None, "Next"),
        2 => (
            "Hear your dry tone".into(),
            "The pedal is bypassed. Play the part you'll be using so you know what you're matching against, then hit Next.".into(),
            None,
            "Next",
        ),
        3 => (
            format!("Set your {n} {lo} level"),
            format!(
                "{n} is at {lo} and the pedal is on. Turn Level until it sits where you want it. Use A / B to flip between the pedal and your dry tone while you play."
            ),
            Some("A / B  vs dry".into()),
            "Next",
        ),
        _ => (
            format!("Set your {n} {hi} level"),
            format!(
                "{n} is now at {hi}. Turn Level until it matches the {n} {lo} level you just set. A / B flips between your {n} {lo} setting and this one."
            ),
            Some(format!("A / B  vs {n} {lo}")),
            "Done",
        ),
    }
}

/// `calToggleAB`.
fn toggle_ab(app: &mut SoundApp, host: &Host, cal: &mut Cal, spec: &CalSpec) {
    if !cal.active {
        return;
    }
    cal.ab = !cal.ab;
    if cal.step == 3 {
        put(app, host, &[("bypass", if cal.ab { 1.0 } else { 0.0 })]);
    } else if cal.step == 4 {
        if cal.ab {
            cal.hi_temp = param(host, "level");
            cal.has_hi_temp = true;
            put(app, host, &[(spec.drive, spec.anchor_lo), ("level", cal.lo_level)]);
        } else {
            let level = if cal.has_hi_temp { cal.hi_temp } else { param(host, "level") };
            put(app, host, &[(spec.drive, spec.anchor_hi), ("level", level)]);
        }
    }
}

/// `calNext`. (`host` is this frame's: a value written by a toggle just now is read from what
/// the toggle wrote, not from `host`.)
fn next(app: &mut SoundApp, host: &Host, cal: &mut Cal, spec: &CalSpec) {
    if !cal.active {
        return;
    }
    if cal.step == 3 {
        if cal.ab {
            toggle_ab(app, host, cal, spec);
        }
        cal.lo_level = param(host, "level");
        put(app, host, &[(spec.link_lo, cal.lo_level)]);
        cal.step = 4;
        render(app, host, cal, spec);
        return;
    }
    if cal.step == 4 {
        let hi = if cal.ab && cal.has_hi_temp { cal.hi_temp } else { param(host, "level") };
        if cal.ab {
            toggle_ab(app, host, cal, spec);
        }
        put(app, host, &[(spec.link_hi, hi)]);
        end(app, host, cal, spec, true);
        return;
    }
    cal.step += 1;
    render(app, host, cal, spec);
}

/// `calEnd`: bypass and the drive knob back, Link on when completed (else as it was), and Link
/// takes over Level again.
fn end(app: &mut SoundApp, host: &Host, cal: &mut Cal, spec: &CalSpec, completed: bool) {
    if cal.ab {
        toggle_ab(app, host, cal, spec);
    }
    cal.active = false;
    let link = if completed { 1.0 } else { cal.saved_link };
    end_gesture(app);
    put(app, host, &[("bypass", cal.saved_bypass), (spec.drive, cal.saved_drive), ("link", link)]);
}

/// Below this width the panel stacks its pills under the text.
pub(crate) const NARROW_PANEL: f32 = 420.0;
/// The body text's size (a wide panel shrinks it until it fits its box).
const BODY_SIZE: f32 = 11.0;

/// A narrow panel's pills: whether A / B goes on a row of its own above Cancel and Next (when the
/// three do not fit one row of `inner_w`).
fn ab_own_row(ui: &Ui, ab_text: Option<&str>, next_text: &str, inner_w: f32) -> bool {
    let w = |t: &str| pill_width_ex(ui, t, PillStyle::Normal, false).trunc() + 8.0;
    ab_text.is_some_and(|t| w(t) + w("Cancel") + w(next_text) > inner_w)
}

/// The height a narrow [`panel`] (the slim editors') `w` wide needs for this step: the step, the
/// title, the body wrapped at its full size, and the pills (A / B on a row of its own when the
/// three do not fit one).
pub(crate) fn panel_height(ui: &Ui, cal: &Cal, spec: &CalSpec, w: f32) -> f32 {
    let (_, body, ab_text, next_text) = texts(cal.step, spec);
    let inner_w = (w - 32.0).max(1.0);
    let body_h = ui.painter().layout(body, font(BODY_SIZE, false), egui::Color32::WHITE, inner_w).size().y;
    let h = pill_height(PillStyle::Normal);
    let pills = if ab_own_row(ui, ab_text.as_deref(), next_text, inner_w) { 2.0 * h + 6.0 } else { h };
    (12.0 + 16.0 + 19.0 + 3.0 + body_h + 8.0 + pills + 12.0).ceil()
}

/// The panel (`resized()`'s `calPanelBounds` = `panel` here) and its three pills, floating over
/// what is under it and taking its clicks. A panel narrower than [`NARROW_PANEL`] (the slim
/// editors, sized by [`panel_height`]) puts the pills under the text, right-aligned, with A / B
/// on a row of its own when the three do not fit one.
pub(crate) fn panel(app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, cal: &mut Cal, spec: &CalSpec, panel: Rect) {
    let _ = ui.interact(panel, ui.id().with("cal-swallow"), Sense::click_and_drag());
    let p = ui.painter().clone();
    p.rect_filled(panel, CornerRadius::same(4), pal.panel_alt);
    glow_rect(&p, panel, 4.0, pal.accent, 1.0);
    p.rect_stroke(panel.shrink(0.5), CornerRadius::same(4), Stroke::new(1.0, pal.accent), StrokeKind::Middle);
    let (title, body, ab_text, next_text) = texts(cal.step, spec);

    let inner = panel.shrink2(vec2(16.0, 12.0));
    let h = pill_height(PillStyle::Normal);
    let narrow = panel.width() < NARROW_PANEL;
    let own_row = narrow && ab_own_row(ui, ab_text.as_deref(), next_text, inner.width());
    let buttons = if narrow {
        Rect::from_min_max(pos2(inner.left(), inner.bottom() - h), inner.max)
    } else {
        Rect::from_min_max(pos2(inner.right() - (inner.width() / 2.0).min(330.0), inner.top()), inner.max)
    };
    let mut x = buttons.right();
    let mut place = |ui: &Ui, text: &str, cy: f32| {
        x -= 8.0;
        let w = pill_width_ex(ui, text, PillStyle::Normal, false).trunc();
        let r = Rect::from_min_size(pos2(x - w, cy - h * 0.5), vec2(w, h));
        x -= w;
        r
    };
    let next_r = place(ui, next_text, buttons.center().y);
    let cancel_r = place(ui, "Cancel", buttons.center().y);
    let ab_r = ab_text.as_ref().map(|t| {
        if own_row {
            let w = pill_width_ex(ui, t, PillStyle::Normal, false).trunc().min(inner.width());
            Rect::from_min_size(pos2(buttons.right() - 8.0 - w, buttons.top() - 6.0 - h), vec2(w, h))
        } else {
            place(ui, t, buttons.center().y)
        }
    });

    let mut text = panel.shrink2(vec2(16.0, 12.0));
    if narrow {
        text.max.y -= h + 8.0 + if own_row { h + 6.0 } else { 0.0 };
    } else {
        text.max.x -= (text.width() * 0.5).min(330.0) + 18.0;
    }
    p.text(text.left_top(), Align2::LEFT_TOP, format!("Step {} of 4", cal.step), font(TINY_SIZE, true), pal.accent);
    let title_r = Rect::from_min_size(pos2(text.left(), text.top() + 16.0), vec2(text.width(), 19.0));
    p.with_clip_rect(title_r).text(title_r.left_top(), Align2::LEFT_TOP, title, font(15.0, false), pal.text_bright);
    let body_r = Rect::from_min_max(pos2(text.left(), title_r.bottom() + 3.0), text.max);
    // `drawFittedText (.., 3)`: the type shrinks until the text fits the box.
    let mut size = BODY_SIZE;
    let mut galley = p.layout(body.clone(), font(size, false), pal.text_body, body_r.width().max(1.0));
    while galley.size().y > body_r.height() + 1.0 && size > 8.0 {
        size -= 0.5;
        galley = p.layout(body.clone(), font(size, false), pal.text_body, body_r.width().max(1.0));
    }
    p.with_clip_rect(body_r.intersect(p.clip_rect())).galley(body_r.left_top(), galley, pal.text_body);

    let plain = PillLook { tracked: false, ..PillLook::default() };
    if pill_ex(ui, next_r, "cal-next", pal, next_text, false, PillStyle::Normal, plain).clicked() {
        next(app, host, cal, spec);
    } else if pill_ex(ui, cancel_r, "cal-cancel", pal, "Cancel", false, PillStyle::Normal, plain).clicked() {
        end(app, host, cal, spec, false);
    } else if let (Some(r), Some(t)) = (ab_r, ab_text.as_deref())
        && pill_ex(ui, r, "cal-ab", pal, t, cal.ab, PillStyle::Normal, plain).clicked()
    {
        toggle_ab(app, host, cal, spec);
    }
}
