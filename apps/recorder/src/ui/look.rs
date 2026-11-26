//! The Recorder's look: the approved design's colours and sizes (a dark window, one red button),
//! and its font, Inter (the copy the DAW already bundles for its splash, `assets/fonts/`,
//! OFL-1.1). Only the Regular weight is bundled: heavier weights are drawn by
//! [`super::paint`] as the regular face thickened (see `paint::Weight`).

use egui::Color32;

/// The window (idle, saved, studio).
pub const BG: Color32 = Color32::from_rgb(0x1b, 0x1c, 0x1f);
/// The window while recording: tinted red.
pub const BG_REC: Color32 = Color32::from_rgb(0x24, 0x1a, 0x1b);
/// Cards.
pub const CARD: Color32 = Color32::from_rgb(0x26, 0x28, 0x2d);
/// Text boxes.
pub const FIELD: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);
/// A meter's empty part.
pub const TRACK: Color32 = Color32::from_rgb(0x12, 0x13, 0x16);
/// Outlines of buttons.
pub const LINE: Color32 = Color32::from_rgb(0x3a, 0x3d, 0x44);
/// Outlines of text boxes, the dashed preview card.
pub const LINE_FIELD: Color32 = Color32::from_rgb(0x4a, 0x4e, 0x57);
/// The idea list's round Play buttons.
pub const ROUND: Color32 = Color32::from_rgb(0x33, 0x36, 0x3d);
pub const TEXT: Color32 = Color32::from_rgb(0xf3, 0xf4, 0xf6);
pub const DIM: Color32 = Color32::from_rgb(0xa9, 0xad, 0xb6);
/// Dim text on the recording screen.
pub const DIM_REC: Color32 = Color32::from_rgb(0xc9, 0xb4, 0xb5);
/// The gear's lines.
pub const ICON: Color32 = Color32::from_rgb(0xd7, 0xda, 0xe0);
/// Record.
pub const RED: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
/// Record's ring.
pub const RED_RING: Color32 = Color32::from_rgb(0x3a, 0x23, 0x26);
/// The recording screen's dot and bars.
pub const RED_LIVE: Color32 = Color32::from_rgb(0xff, 0x6b, 0x70);
/// "Recording".
pub const RED_SOFT: Color32 = Color32::from_rgb(0xff, 0xb3, 0xb5);
/// Stop's square.
pub const RED_STOP: Color32 = Color32::from_rgb(0xc2, 0x34, 0x3a);
/// Stop's ring.
pub const STOP_RING: Color32 = Color32::from_rgb(0x5a, 0x2a, 0x2d);
/// Throw away.
pub const RED_TEXT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x8e);
/// Levels, the check mark, a studio found.
pub const GREEN: Color32 = Color32::from_rgb(0x3e, 0xcf, 0x8e);
/// A saved take's shape.
pub const SHAPE: Color32 = Color32::from_rgb(0x6b, 0x70, 0x80);
/// Links ("All my ideas", "Back to my ideas").
pub const LINK: Color32 = Color32::from_rgb(0x8f, 0xb8, 0xff);
/// Problems (said in words, never alarming red).
pub const AMBER: Color32 = Color32::from_rgb(0xf5, 0xb5, 0x4a);
/// Record, greyed (nothing to record from).
pub const OFF: Color32 = Color32::from_rgb(0x3a, 0x3d, 0x44);

/// The design's window, in points.
pub const WIDTH: f32 = 480.0;
pub const HEIGHT: f32 = 640.0;
/// Its padding (sides, top and bottom).
pub const PAD_X: f32 = 28.0;
pub const PAD_Y: f32 = 24.0;
/// Every button and row is at least this tall (a finger's size).
pub const TOUCH: f32 = 44.0;

/// Inter Regular 4.1, the file the DAW bundles (`assets/fonts/Inter-Regular.ttf`, OFL-1.1).
static INTER: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");

/// The Recorder's look on `ctx`: its dark visuals, Inter as the text font.
pub fn apply(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    ctx.add_font(FontInsert::new(
        "inter-regular",
        egui::FontData::from_static(INTER),
        vec![InsertFontFamily { family: egui::FontFamily::Proportional, priority: FontPriority::Highest }],
    ));
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = FIELD;
    v.faint_bg_color = CARD;
    v.selection.bg_fill = Color32::from_rgb(0x2f, 0x5f, 0xb0);
    v.selection.stroke.color = TEXT;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.hovered.weak_bg_fill = ROUND;
    v.widgets.hovered.bg_fill = ROUND;
    v.widgets.noninteractive.fg_stroke.color = TEXT;
    v.widgets.inactive.fg_stroke.color = TEXT;
    v.widgets.hovered.fg_stroke.color = TEXT;
    v.widgets.active.fg_stroke.color = TEXT;
    v.text_cursor.stroke = egui::Stroke::new(2.0, TEXT);
    v.window_corner_radius = egui::CornerRadius::same(12);
    v.menu_corner_radius = egui::CornerRadius::same(10);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = egui::CornerRadius::same(10);
    }
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 8.0);
        s.spacing.interact_size.y = TOUCH;
        s.spacing.combo_height = 320.0;
        s.interaction.tooltip_delay = 0.4;
        for (style, size) in
            [(egui::TextStyle::Body, 15.0), (egui::TextStyle::Button, 15.0), (egui::TextStyle::Small, 12.0), (egui::TextStyle::Heading, 24.0)]
        {
            s.text_styles.insert(style, egui::FontId::proportional(size));
        }
    });
}
