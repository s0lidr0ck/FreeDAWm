//! Rez's editor in the Mix window: its size, the graph's gestures (bands and range handles) and
//! the controls through commands, each one undo step, and hostile parameters.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, vec2};
use soundcraft_ui_egui::editors::rez::{SIZE, band_pos, plot_rect};
use soundcraft_ui_egui::module_graphs::freq_to_x;

const SCREEN: egui::Vec2 = vec2(1800.0, 1000.0);

#[test]
fn rez_opens_its_own_editor_at_its_size() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("rez", SCREEN, theme);
        assert_eq!(frame(&h, t, m).size(), SIZE);
    }
}

#[test]
fn dragging_a_band_sets_its_frequency_and_sensitivity_in_one_undo_step() {
    let (mut h, t, m) = open("rez", SCREEN, "dark");
    let f = frame(&h, t, m);
    let from = band_pos(f, 2500.0, 0.0);
    let to = band_pos(f, 5000.0, 6.0);
    drag(&mut h, from, to, Modifiers::NONE);
    assert!((raw(&h, t, m, "b3_freq") / 5000.0).log2().abs() < 0.03, "{}", raw(&h, t, m, "b3_freq"));
    assert!((raw(&h, t, m, "b3_gain") - 6.0).abs() < 0.3, "{}", raw(&h, t, m, "b3_gain"));
    // The other bands did not move.
    assert_eq!(raw(&h, t, m, "b2_freq"), 600.0);
    undo(&mut h);
    assert_eq!((raw(&h, t, m, "b3_freq"), raw(&h, t, m, "b3_gain")), (2500.0, 0.0), "one undo step");
}

#[test]
fn alt_drag_changes_a_bands_width_only() {
    let (mut h, t, m) = open("rez", SCREEN, "dark");
    let f = frame(&h, t, m);
    let at = band_pos(f, 600.0, 0.0);
    drag(&mut h, at, at + vec2(0.0, 48.0), Modifiers::ALT);
    assert!(raw(&h, t, m, "b2_q") > 1.5, "down narrows the band: Q {}", raw(&h, t, m, "b2_q"));
    assert_eq!((raw(&h, t, m, "b2_freq"), raw(&h, t, m, "b2_gain")), (600.0, 0.0));
}

#[test]
fn a_band_switches_off_with_a_right_click_and_goes_flat_on_a_double_click() {
    let (mut h, t, m) = open("rez", SCREEN, "dark");
    let f = frame(&h, t, m);
    right_click(&mut h, band_pos(f, 150.0, 0.0));
    assert_eq!(raw(&h, t, m, "b1_on"), 0.0);
    right_click(&mut h, band_pos(f, 150.0, 0.0));
    assert_eq!(raw(&h, t, m, "b1_on"), 1.0);

    drag(&mut h, band_pos(f, 8000.0, 0.0), band_pos(f, 8000.0, -12.0), Modifiers::NONE);
    assert!((raw(&h, t, m, "b4_gain") + 12.0).abs() < 0.3);
    let at = band_pos(f, raw(&h, t, m, "b4_freq"), raw(&h, t, m, "b4_gain"));
    // (Half a second on, so the drag's release is not taken for the first of the two clicks.)
    h.run_steps(30);
    double_click(&mut h, at);
    assert_eq!(raw(&h, t, m, "b4_gain"), 0.0);
}

#[test]
fn the_range_handles_set_low_and_high() {
    let (mut h, t, m) = open("rez", SCREEN, "dark");
    let plot = plot_rect(frame(&h, t, m));
    // Low starts at 60 Hz: its handle, grabbed near the bottom (clear of the bands).
    let y = plot.bottom() - 30.0;
    drag(&mut h, egui::pos2(freq_to_x(plot, 60.0), y), egui::pos2(freq_to_x(plot, 300.0), y), Modifiers::NONE);
    assert!((raw(&h, t, m, "low") / 300.0).log2().abs() < 0.05, "{}", raw(&h, t, m, "low"));
    // High starts at 20 kHz, the plot's right edge.
    drag(&mut h, egui::pos2(plot.right() - 1.0, y), egui::pos2(freq_to_x(plot, 6000.0), y), Modifiers::NONE);
    assert!((raw(&h, t, m, "high") / 6000.0).log2().abs() < 0.05, "{}", raw(&h, t, m, "high"));
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "high"), 20_000.0);
}

#[test]
fn the_controls_set_their_parameters() {
    let (mut h, t, m) = open("rez", SCREEN, "dark");
    let f = frame(&h, t, m);
    // Depth: the 104 px knob in the first group; 25 px up is +0.2 of its travel.
    let depth = f.min + vec2(24.0 + 75.0, 357.0 + 18.0 + 52.0);
    let before = param(&h, t, m, "depth");
    drag(&mut h, depth, depth - vec2(0.0, 25.0), Modifiers::NONE);
    assert!((param(&h, t, m, "depth") - (before + 0.2)).abs() < 0.01, "{}", param(&h, t, m, "depth"));
    // Hard: the right half of the mode switch under it.
    click(&mut h, f.min + vec2(24.0 + 14.0 + 122.0 * 0.75, 529.0));
    assert_eq!(raw(&h, t, m, "mode"), 1.0);
    // M/S: the right half of the stereo switch.
    click(&mut h, f.min + vec2(512.0 + 142.0 * 0.75, 529.0));
    assert_eq!(raw(&h, t, m, "stereo"), 1.0);
    // Delta, at the header's right.
    click(&mut h, f.min + vec2(836.0 - 29.0, 28.0));
    assert_eq!(raw(&h, t, m, "delta"), 1.0);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "delta"), 0.0);
}

#[test]
fn hostile_parameters_draw_without_panicking() {
    let (mut h, t, m) = open("rez", SCREEN, "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), SIZE);
}
