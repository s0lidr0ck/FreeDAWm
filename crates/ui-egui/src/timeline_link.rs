//! What the Edit window, the Vocal Editor and the MIDI Editor share about time (spec
//! 2026-11-04-editor-timeline-link): following the playhead as Options › Scrolling asks (Page,
//! Continuous, Centre), the editors' Link (on: the editor follows the playhead itself), and the
//! timebase ruler drawn in the main counter's format through any time-to-x mapping.

use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, Rect, Stroke, Ui, pos2, vec2};
use soundcraft_model::Session;
use soundcraft_time::{FrameRate, GridValue, NoteValue, Range, Samples, TimeFormat, format_position};

/// How a view follows the playhead while playing (Options › Scrolling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// `none` and `after_playback`: the view never moves by itself while playing.
    Off,
    /// The playhead passing [`PAGE_TURN`] of the width turns the page: it lands at [`PAGE_LAND`].
    Page,
    /// The view scrolls with the playhead, which stays [`CONTINUOUS_AT`] of the width in.
    Continuous,
    /// The playhead stays at the centre.
    Centre,
}

/// Where (share of the width) the playhead turns the page in [`Follow::Page`].
pub const PAGE_TURN: f64 = 0.9;
/// Where (share of the width) the playhead lands after a page turn, and after a locate reveals it.
pub const PAGE_LAND: f64 = 0.1;
/// Where (share of the width) [`Follow::Continuous`] holds the playhead.
pub const CONTINUOUS_AT: f64 = 1.0 / 3.0;

impl Follow {
    /// The mode for an `options.scrolling` value.
    pub fn from_mode(mode: &str) -> Follow {
        match mode {
            "page" => Follow::Page,
            "continuous" => Follow::Continuous,
            "center" | "centre" => Follow::Centre,
            _ => Follow::Off,
        }
    }

    /// The session's mode (Options › Scrolling).
    pub fn of(s: &Session) -> Follow {
        Follow::from_mode(&s.edit.scrolling)
    }

    pub fn follows(self) -> bool {
        self != Follow::Off
    }

    /// Where (share of the width) the playhead sits after this mode moves the view.
    fn anchor(self) -> f64 {
        match self {
            Follow::Off | Follow::Page => PAGE_LAND,
            Follow::Continuous => CONTINUOUS_AT,
            Follow::Centre => 0.5,
        }
    }
}

fn at_share(pos: Samples, span: Samples, share: f64) -> Samples {
    pos.saturating_sub((span.max(0) as f64 * share) as Samples).max(0)
}

/// The left edge a view `span` samples wide starting at `left` moves to while playing at `pos`
/// (`None`: it stays). Never before 0.
pub fn follow_target(mode: Follow, left: Samples, span: Samples, pos: Samples) -> Option<Samples> {
    let to = match mode {
        Follow::Off => return None,
        Follow::Page => {
            let turn = left.saturating_add((span.max(0) as f64 * PAGE_TURN) as Samples);
            if pos >= left && pos <= turn {
                return None;
            }
            at_share(pos, span, PAGE_LAND)
        }
        Follow::Continuous | Follow::Centre => at_share(pos, span, mode.anchor()),
    };
    (to != left).then_some(to)
}

/// The left edge that brings `pos` into a view `span` wide at `left` (`None` when it shows):
/// where `mode` would put it, [`PAGE_LAND`] in without one.
pub fn reveal(mode: Follow, left: Samples, span: Samples, pos: Samples) -> Option<Samples> {
    if pos >= left && pos <= left.saturating_add(span) {
        return None;
    }
    Some(at_share(pos, span, mode.anchor()))
}

/// One editor's follow state (the Vocal Editor's, the MIDI Editor's).
///
/// Linked, each frame the editor is drawn: a locate (a stopped playhead that moved, or a jump
/// while playing: back, or ahead by more than playback moves it), the editor's opening and the
/// start of playback bring the playhead into view; while playing it follows as the mode asks. A
/// pan or zoom by hand while playing holds the following until the next locate or play start.
/// Unlinked, the editor never moves by itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditorFollow {
    last_pos: Option<Samples>,
    was_playing: bool,
    /// Following is held (a pan or zoom by hand while playing).
    pub hold: bool,
    /// Set by the editor when the user panned or zoomed it this frame.
    pub user_moved: bool,
    /// The editor was just opened at a place of its own (Tune Vocal… on a note): its opening
    /// does not reveal the playhead.
    skip_open: bool,
}

impl EditorFollow {
    /// A follow state for an editor opened at a place of its own (`open_at`).
    pub fn opened_at() -> EditorFollow {
        EditorFollow { skip_open: true, ..EditorFollow::default() }
    }

    /// The left edge the editor's view (`left`, `span` samples wide) moves to this frame, if any.
    /// `jump`: the most the playhead moves between two frames while playing (more is a locate).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        linked: bool,
        mode: Follow,
        playing: bool,
        pos: Samples,
        left: Samples,
        span: Samples,
        jump: Samples,
    ) -> Option<Samples> {
        let user = std::mem::take(&mut self.user_moved);
        let opened = self.last_pos.is_none();
        let started = playing && !self.was_playing;
        let located = match self.last_pos {
            None => true,
            Some(last) if playing && self.was_playing => pos < last || pos.saturating_sub(last) > jump.max(1),
            Some(last) => pos != last,
        };
        self.last_pos = Some(pos);
        self.was_playing = playing;
        if !linked {
            self.hold = false;
            self.skip_open = false;
            return None;
        }
        if opened && std::mem::take(&mut self.skip_open) {
            return None;
        }
        if started || located {
            self.hold = false;
        }
        if user && playing && !located {
            self.hold = true;
        }
        if (started || located)
            && let Some(to) = reveal(mode, left, span, pos)
        {
            return Some(to);
        }
        if playing && !self.hold {
            return follow_target(mode, left, span, pos);
        }
        None
    }
}

/// The most the playhead moves in one frame while playing, at `rate` Hz: a quarter second
/// (more is a locate).
pub fn frame_jump(rate: u32) -> Samples {
    Samples::from(rate.max(1)) / 4
}

// ---- the Link button ----------------------------------------------------------------------------

/// The 🔗 Link toggle (an editor's toolbar): green "Linked" when on, "Link" when off.
pub fn link_button(ui: &mut Ui, linked: bool) -> egui::Response {
    let t = Tokens::current();
    let dark = t.panel_bg.r() < 128;
    let (fill, text) = match (linked, dark) {
        (true, true) => (Color32::from_rgb(33, 74, 46), Color32::from_rgb(150, 228, 166)),
        (true, false) => (Color32::from_rgb(198, 236, 206), Color32::from_rgb(22, 96, 44)),
        (false, _) => (t.button, t.text),
    };
    let label = egui::RichText::new(if linked { "     Linked" } else { "     Link" }).color(text);
    let b = ui.add(egui::Button::new(label).fill(fill));
    let icon = Rect::from_center_size(pos2(b.rect.left() + 13.0, b.rect.center().y), vec2(13.0, 13.0));
    crate::icons::draw(ui.painter(), icon, "link", text);
    b.on_hover_text(if linked {
        "Linked: follows the playhead itself (Page / Continuous / Centre, as Options \u{25b8} Scrolling), even when the Edit window is hidden. Click to unlink."
    } else {
        "Unlinked: stays where you put it. Click to follow the playhead (Options \u{25b8} Scrolling)."
    })
}

// ---- the timebase ruler -------------------------------------------------------------------------

/// A "nice" step in samples for time labels at least `min_px` apart at `spp` samples per px.
pub fn nice_step(spp: f64, sr: f64, min_px: f64) -> i64 {
    let min = spp * min_px;
    let steps_s = [0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0];
    for st in steps_s {
        if st * sr >= min {
            return (st * sr) as i64;
        }
    }
    (3600.0 * sr) as i64
}

/// The ruler's ticks across `view` at `spp` samples per px in format `f`: (time, labelled).
pub fn marks(s: &Session, view: Range, spp: f64, f: TimeFormat, rate: FrameRate) -> Vec<(Samples, bool)> {
    let sr = s.sample_rate;
    let spp = if spp.is_finite() && spp > 0.0 { spp } else { 1.0 };
    let mut marks: Vec<(Samples, bool)> = Vec::new();
    match f {
        TimeFormat::BarsBeats => {
            // Bars every n so labels are ≥ 60 px apart.
            let bar_px = {
                let b = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
                b.step_samples(view.start.max(0), sr, &s.tempo, rate) as f64 / spp
            };
            let every = [1i64, 2, 4, 8, 16, 32, 64, 128].into_iter().find(|n| bar_px * *n as f64 >= 60.0).unwrap_or(256);
            let first = s.tempo.bar_beat_at(view.start.max(0), sr).bar;
            let mut bar = first - (first - 1).rem_euclid(every);
            let mut guard = 0;
            loop {
                let at = s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar, beat: 1, tick: 0 }, sr);
                if at > view.end || guard > 400 {
                    break;
                }
                marks.push((at, true));
                if bar_px * every as f64 >= 240.0 {
                    let m = s.tempo.meter_at_tick(s.tempo.samples_to_ticks(at, sr));
                    for beat in 2..=i64::from(m.numerator) {
                        marks.push((s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar, beat, tick: 0 }, sr), false));
                    }
                }
                bar += every;
                guard += 1;
            }
        }
        TimeFormat::Samples => {
            let step = {
                let min = spp * 90.0;
                let mut st = 1i64;
                while (st as f64) < min {
                    st *= if st.to_string().starts_with('2') { 5 } else { 2 };
                    if st > 10_000_000_000 {
                        break;
                    }
                }
                st
            };
            let mut at = view.start.max(0).div_euclid(step) * step;
            while at <= view.end && marks.len() < 400 {
                marks.push((at, true));
                at += step;
            }
        }
        _ => {
            let step = nice_step(spp, sr.as_f64(), 70.0).max(1);
            let mut at = view.start.max(0).div_euclid(step) * step;
            while at <= view.end && marks.len() < 400 {
                marks.push((at, true));
                let sub = step / 4;
                if sub > 0 && (sub as f64 / spp) > 8.0 {
                    for k in 1..4 {
                        marks.push((at + sub * k, false));
                    }
                }
                at += step;
            }
        }
    }
    marks
}

/// A labelled tick's text at `at` in format `f`.
pub fn label(s: &Session, at: Samples, f: TimeFormat, rate: FrameRate) -> String {
    let sr = s.sample_rate;
    match f {
        TimeFormat::BarsBeats => s.tempo.bar_beat_at(at, sr).bar.to_string(),
        TimeFormat::MinSecs => {
            let secs = sr.seconds(at);
            if secs.fract().abs() < 1e-9 {
                format!("{}:{:02}", (secs / 60.0) as i64, (secs as i64) % 60)
            } else {
                format_position(at, f, sr, &s.tempo, rate, s.timecode_start, s.feet_frames_start())
            }
        }
        _ => format_position(at, f, sr, &s.tempo, rate, s.timecode_start, s.feet_frames_start()),
    }
}

/// The labels a ruler across `view` at `spp` shows in format `f`, in time order.
pub fn labels(s: &Session, view: Range, spp: f64, f: TimeFormat, rate: FrameRate) -> Vec<(Samples, String)> {
    marks(s, view, spp, f, rate).into_iter().filter(|m| m.1).map(|(at, _)| (at, label(s, at, f, rate))).collect()
}

/// Draws a timebase ruler in `row` across `view`, each time at `x_of(time)`, `spp` samples per
/// px (the Edit window's rulers and both editors'); `bottom` closes it with a line. Returns the
/// labels drawn.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    painter: &egui::Painter,
    s: &Session,
    row: Rect,
    view: Range,
    spp: f64,
    x_of: &dyn Fn(Samples) -> f32,
    f: TimeFormat,
    rate: FrameRate,
    bottom: bool,
) -> Vec<(Samples, String)> {
    let t = Tokens::current();
    let mut drawn = Vec::new();
    for (at, major) in marks(s, view, spp, f, rate) {
        let x = x_of(at);
        if major {
            painter.line_segment([pos2(x, row.min.y + 2.0), pos2(x, row.max.y)], Stroke::new(1.0, t.ruler_tick));
            let text = label(s, at, f, rate);
            painter.text(pos2(x + 3.0, row.center().y), Align2::LEFT_CENTER, &text, regular(11.0), t.ruler_text);
            drawn.push((at, text));
        } else {
            painter.line_segment([pos2(x, row.max.y - 4.0), pos2(x, row.max.y)], Stroke::new(1.0, t.ruler_tick));
        }
    }
    if bottom {
        painter.line_segment([pos2(row.min.x, row.max.y), pos2(row.max.x, row.max.y)], Stroke::new(1.0, t.border));
    }
    drawn
}

/// An editor's ruler in the main counter's format, with the playhead when given (a line and a flag): `row`
/// across `view`, times at `x_of`. Stores the labels as `soundcraft-ruler-<who>` (tests read
/// them) and returns them.
pub fn editor_ruler(
    ui: &Ui,
    s: &Session,
    who: &str,
    row: Rect,
    view: Range,
    x_of: &dyn Fn(Samples) -> f32,
    playhead: Option<Samples>,
) -> Vec<(Samples, String)> {
    let t = Tokens::current();
    let painter = ui.painter().with_clip_rect(row);
    painter.rect_filled(row, 0.0, t.ruler_bg);
    let spp = view.len().max(1) as f64 / f64::from(row.width().max(1.0));
    let drawn = draw(&painter, s, row, view, spp, x_of, s.edit.main_counter, s.frame_rate, true);
    if let Some(ph) = playhead.filter(|p| *p >= view.start && *p <= view.end) {
        let x = x_of(ph);
        painter.add(egui::Shape::convex_polygon(
            vec![pos2(x - 6.0, row.top()), pos2(x + 6.0, row.top()), pos2(x, row.top() + 7.0)],
            t.playhead,
            Stroke::NONE,
        ));
        painter.line_segment([pos2(x, row.top()), pos2(x, row.bottom())], Stroke::new(1.5, t.playhead));
    }
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(format!("soundcraft-ruler-{who}")), drawn.clone()));
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPAN: Samples = 100_000;

    #[test]
    fn page_turns_at_ninety_percent_and_lands_at_ten() {
        assert_eq!(follow_target(Follow::Page, 0, SPAN, 50_000), None);
        assert_eq!(follow_target(Follow::Page, 0, SPAN, 90_000), None);
        assert_eq!(follow_target(Follow::Page, 0, SPAN, 90_001), Some(80_001));
        // Behind the view (a loop wrap): back to it.
        assert_eq!(follow_target(Follow::Page, 200_000, SPAN, 150_000), Some(140_000));
    }

    #[test]
    fn continuous_holds_a_third_in_and_centre_the_middle() {
        assert_eq!(follow_target(Follow::Continuous, 0, 90_000, 60_000), Some(30_000));
        assert_eq!(follow_target(Follow::Continuous, 30_000, 90_000, 60_000), None);
        assert_eq!(follow_target(Follow::Centre, 0, SPAN, 80_000), Some(30_000));
        // Never before the session start.
        assert_eq!(follow_target(Follow::Centre, 0, SPAN, 20_000), None);
        assert_eq!(follow_target(Follow::Off, 0, SPAN, 500_000), None);
    }

    #[test]
    fn reveal_only_moves_a_hidden_playhead() {
        assert_eq!(reveal(Follow::Off, 0, SPAN, 50_000), None);
        assert_eq!(reveal(Follow::Off, 0, SPAN, 500_000), Some(490_000));
        assert_eq!(reveal(Follow::Centre, 0, SPAN, 500_000), Some(450_000));
    }

    #[test]
    fn an_unlinked_editor_never_moves() {
        let mut f = EditorFollow::default();
        for pos in [0, 500_000, 1_000_000] {
            assert_eq!(f.update(false, Follow::Page, true, pos, 0, SPAN, 12_000), None);
        }
    }

    #[test]
    fn a_linked_editor_reveals_a_locate_and_follows_while_playing() {
        let mut f = EditorFollow::default();
        assert_eq!(f.update(true, Follow::Page, false, 0, 0, SPAN, 12_000), None, "open: the playhead shows");
        assert_eq!(f.update(true, Follow::Page, false, 1_000_000, 0, SPAN, 12_000), Some(990_000), "a locate far away");
        // Playing, it turns the page.
        assert_eq!(f.update(true, Follow::Page, true, 1_000_000, 990_000, SPAN, 100_000), None);
        assert_eq!(f.update(true, Follow::Page, true, 1_080_000, 990_000, SPAN, 100_000), None);
        assert_eq!(f.update(true, Follow::Page, true, 1_080_001, 990_000, SPAN, 100_000), Some(1_070_001));
    }

    #[test]
    fn a_pan_by_hand_while_playing_holds_until_the_next_locate() {
        let mut f = EditorFollow::default();
        f.update(true, Follow::Continuous, true, 0, 0, SPAN, 12_000);
        f.user_moved = true;
        assert_eq!(f.update(true, Follow::Continuous, true, 5_000, 400_000, SPAN, 12_000), None);
        assert!(f.hold);
        assert_eq!(f.update(true, Follow::Continuous, true, 10_000, 400_000, SPAN, 12_000), None, "held");
        // A jump back (a loop wrap or a locate) resumes.
        assert_eq!(f.update(true, Follow::Continuous, true, 0, 400_000, SPAN, 12_000), Some(0));
        assert!(!f.hold);
    }

    #[test]
    fn opened_at_a_place_of_its_own_does_not_reveal() {
        let mut f = EditorFollow::opened_at();
        assert_eq!(f.update(true, Follow::Page, false, 0, 1_000_000, SPAN, 12_000), None);
        assert_eq!(f.update(true, Follow::Page, false, 0, 1_000_000, SPAN, 12_000), None, "no locate since");
    }
}
