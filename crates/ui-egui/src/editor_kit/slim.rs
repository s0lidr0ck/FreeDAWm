//! The pieces every slim editor shares (View > Mix Window Views > Slim Plugin Editors): the
//! classic drawer's width and the laid-out height, the knob grid, the family header folded onto
//! two rows, a header's pill row, the model menu (a full-width dropdown whose entries carry name,
//! subtitle and description), the display toggle, the More disclosure row, wrapped captions and
//! row labels, and a panel painted under controls drawn before it. Each plugin's `slim.rs` draws
//! its own sections with these and with its full editor's own gestures (same commands, param ids
//! and merge keys). Conventions: `docs/superpowers/specs/2026-10-21-slim-editors.md`; Comp's slim
//! layout (`editors/comp/slim.rs`) is the reference.

use super::controls::{PillStyle, SelectionStyle, pill_height, segmented_ex};
use super::family::{self, Family, FamilyState, HeaderRects};
use super::look::{CAPTION_SIZE, LABEL_SIZE, paint_title};
use super::palette::{al, rgb};
use super::{Host, VstPalette};
use crate::SoundApp;
use crate::theme::{bold, regular};
use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

/// The classic drawer's width.
pub const W: f32 = crate::module_drawer::DRAWER_W;
/// The side padding inside a slim editor.
pub const PAD_X: f32 = 10.0;
/// The top (and bottom) padding.
pub const PAD_Y: f32 = 8.0;
/// The gap between sections.
pub const GAP: f32 = 8.0;
/// A one-line row (segmented controls, a header row).
pub const ROW_H: f32 = 22.0;
/// The model menu's closed height.
pub const MODEL_H: f32 = 28.0;
/// The More disclosure row's height.
pub const MORE_H: f32 = 20.0;
/// The primary dials' size.
pub const BIG_KNOB: f32 = 56.0;
/// The secondary dials' size.
pub const SMALL_KNOB: f32 = 36.0;
/// Until the first layout tells.
const FIRST_H: f32 = 700.0;

/// The editor's size for the height last laid out.
pub fn size(laid_out: f32) -> Vec2 {
    vec2(W, if laid_out.is_finite() && laid_out > 100.0 { laid_out } else { FIRST_H })
}

/// Records the height laid out this frame (`bottom` is where the content ends, padding included,
/// `top` the editor's top) and asks for another frame when it changed, so the frame follows.
pub fn settle(ui: &Ui, top: f32, bottom: f32, laid_out: &mut f32) {
    let h = (bottom - top).ceil();
    if (h - *laid_out).abs() > 0.5 {
        *laid_out = h;
        ui.ctx().request_repaint();
    }
}

/// `n` cells in rows of `per_row`, each row centred in `[left, left + w]`.
pub fn grid(left: f32, w: f32, top: f32, n: usize, per_row: usize, cell: Vec2, row_gap: f32) -> Vec<Rect> {
    let per_row = per_row.max(1);
    (0..n)
        .map(|i| {
            let row = i / per_row;
            let in_row = (n - row * per_row).min(per_row);
            let x0 = (left + (w - in_row as f32 * cell.x) / 2.0).round();
            Rect::from_min_size(pos2(x0 + (i % per_row) as f32 * cell.x, top + row as f32 * (cell.y + row_gap)), cell)
        })
        .collect()
}

/// How many knobs to a row for `n` of them: three (then two), two and two for four, four and
/// three (or four) for seven and eight.
pub fn per_row(n: usize) -> usize {
    match n {
        4 => 2,
        7 | 8 => 4,
        _ => 3,
    }
}

/// The bottom of the lowest of `cells` (`top` when there are none).
pub fn bottom_of(cells: &[Rect], top: f32) -> f32 {
    cells.iter().map(|r| r.bottom()).fold(top, f32::max)
}

/// How the family header's second row places A and B.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AbPlace {
    /// Spread with Undo and Bypass over the row, 4 px apart ([`pill_row`], as Comp's header).
    Spread,
    /// Together in the row's middle, `gap` apart; Undo at the left, Bypass at the right.
    Centred(f32),
}

/// The family header in two rows from `y` across `[x0, x0 + w]`: the title and the preset strip
/// (◀ name ▶), then Undo, A, B and Bypass. The controls are the full editor's own
/// ([`family::header`]): the same gestures. Returns the y under the header.
#[allow(clippy::too_many_arguments)]
pub(crate) fn family_header(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    pal: &VstPalette,
    st: &mut FamilyState,
    fam: &Family,
    title: (&str, &str),
    (x0, w, y): (f32, f32, f32),
    ab: AbPlace,
) -> f32 {
    let btn_h = pill_height(PillStyle::Small);
    let head = Rect::from_min_size(pos2(x0, y), vec2(w, ROW_H + 2.0));
    let title_w = paint_title(ui.painter(), head, title.0, title.1, pal);
    let (prev, name, next) = preset_strip(head, title_w);
    let y = y + ROW_H + 2.0 + 6.0;
    let widths = [
        family::small_pill_w(ui, "Undo"),
        family::small_pill_w(ui, "A").max(28.0),
        family::small_pill_w(ui, "B").max(28.0),
        family::small_pill_w(ui, "Bypass"),
    ];
    let [undo, a, b, bypass] = match ab {
        AbPlace::Spread => {
            let c = pill_row(x0, w, y, &widths, &[1]);
            [c[0], c[1], c[2], c[3]]
        }
        AbPlace::Centred(gap) => {
            let ab_x = (x0 + (w - (widths[1] + gap + widths[2])) / 2.0).round();
            let a = Rect::from_min_size(pos2(ab_x, y), vec2(widths[1], btn_h));
            [
                Rect::from_min_size(pos2(x0, y), vec2(widths[0], btn_h)),
                a,
                Rect::from_min_size(pos2(a.right() + gap, y), vec2(widths[2], btn_h)),
                Rect::from_min_size(pos2(x0 + w - widths[3], y), vec2(widths[3], btn_h)),
            ]
        }
    };
    family::header(app, ui, host, pal, st, fam, &HeaderRects { prev, name, next, undo, a, b, bypass });
    y + btn_h + GAP + 2.0
}

/// The preset strip ◀ name ▶ in what a title `title_w` wide (and its 10 px gap) leaves of `head`.
/// Returns (prev, name, next).
pub fn preset_strip(head: Rect, title_w: f32) -> (Rect, Rect, Rect) {
    let btn_h = pill_height(PillStyle::Small);
    let arrow_w = 20.0;
    let strip = Rect::from_min_max(pos2(head.left() + title_w + 10.0, head.top()), head.max);
    let mid = strip.center().y;
    let prev = Rect::from_min_size(pos2(strip.left(), mid - btn_h / 2.0), vec2(arrow_w, btn_h));
    let next = Rect::from_min_size(pos2(strip.right() - arrow_w, mid - btn_h / 2.0), vec2(arrow_w, btn_h));
    let name = Rect::from_min_max(pos2(prev.right() + 4.0, mid - (btn_h + 2.0) / 2.0), pos2(next.left() - 4.0, mid + (btn_h + 2.0) / 2.0));
    (prev, name, next)
}

/// A header's pill row: `widths` spread over `[x0, x0 + w]` at `y`, 4 px apart after the indices
/// in `tight_after` (A next to B) and evenly spread elsewhere; they narrow alike when they do not
/// fit.
pub fn pill_row(x0: f32, w: f32, y: f32, widths: &[f32], tight_after: &[usize]) -> Vec<Rect> {
    const TIGHT: f32 = 4.0;
    const MIN_SPARE: f32 = 4.0;
    let btn_h = pill_height(PillStyle::Small);
    let n = widths.len();
    if n == 0 {
        return vec![];
    }
    let tight_n = tight_after.iter().filter(|i| **i + 1 < n).count();
    let loose_gaps = (n - 1).saturating_sub(tight_n) as f32;
    let tight_total = TIGHT * tight_n as f32;
    let room = w - tight_total - loose_gaps * MIN_SPARE;
    let mut ws = widths.to_vec();
    let sum: f32 = ws.iter().sum();
    if sum > room && sum > 0.0 {
        let k = room / sum;
        for bw in &mut ws {
            *bw = (*bw * k).floor();
        }
    }
    let spare = if loose_gaps > 0.0 { ((w - ws.iter().sum::<f32>() - tight_total) / loose_gaps).max(MIN_SPARE) } else { 0.0 };
    let mut x = x0;
    let mut out = Vec::with_capacity(n);
    for (i, bw) in ws.iter().enumerate() {
        out.push(Rect::from_min_size(pos2(x.round(), y), vec2(*bw, btn_h)));
        x += bw + if tight_after.contains(&i) { TIGHT } else { spare };
    }
    out
}

/// The More section's disclosure row: a chevron, "More", and (closed) what it holds. True when
/// clicked.
pub fn more_header(ui: &mut Ui, r: Rect, salt: &str, pal: &VstPalette, open: bool, holds: &str) -> bool {
    let resp = ui.interact(r, ui.id().with(("slim-more", salt)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, "More"));
    let p = ui.painter();
    let hot = resp.hovered();
    p.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, al(Color32::WHITE, 0.07)));
    let c = pos2(r.left() + 6.0, r.center().y + 1.0);
    let col = if hot { pal.accent } else { pal.text_dim };
    let tri = if open {
        vec![pos2(c.x - 4.0, c.y - 2.0), pos2(c.x + 4.0, c.y - 2.0), pos2(c.x, c.y + 2.5)]
    } else {
        vec![pos2(c.x - 2.0, c.y - 4.0), pos2(c.x + 2.5, c.y), pos2(c.x - 2.0, c.y + 4.0)]
    };
    p.add(egui::Shape::convex_polygon(tri, col, Stroke::NONE));
    let mf = bold(11.0);
    let mw = p.layout_no_wrap("More".into(), mf.clone(), Color32::WHITE).size().x;
    p.text(pos2(r.left() + 16.0, r.center().y + 1.0), Align2::LEFT_CENTER, "More", mf, if hot { pal.accent } else { pal.text_bright });
    if !open && !holds.is_empty() {
        let at = pos2(r.left() + 16.0 + mw + 8.0, r.center().y + 1.0);
        let clip = Rect::from_min_max(pos2(at.x, r.top()), r.max);
        let mut job = egui::text::LayoutJob::simple_singleline(holds.to_string(), regular(9.5), pal.text_faint);
        job.wrap = egui::text::TextWrapping::truncate_at_width(clip.width());
        let g = ui.fonts_mut(|f| f.layout_job(job));
        p.with_clip_rect(clip).galley(pos2(at.x, at.y - g.size().y / 2.0), g, pal.text_faint);
    }
    resp.clicked()
}

/// One entry of a [`model_menu`].
#[derive(Clone, Copy, Debug)]
pub struct MenuEntry<'a> {
    pub name: &'a str,
    /// A short subtitle after the name ("Distress"); may be empty. A tag every entry shares names
    /// the picker: the closed selector shows it and the open menu skips it.
    pub tag: &'a str,
    /// The description under the name in the open menu (and the closed selector's hover).
    pub blurb: &'a str,
    /// The hardware credit line (`crate::hardware_credit`), shown on hover in the open menu and
    /// after the blurb in the closed selector's hover; empty for our own designs.
    pub credit: &'a str,
}

/// How a [`model_menu_ex`] draws its text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuLook {
    /// The closed selector's text inset from its left edge.
    pub inset: f32,
    /// Text centred on its row (and a smaller type in a selector lower than [`MODEL_H`], inset 8)
    /// instead of set on a baseline.
    pub centred: bool,
    /// The open menu's height before it scrolls.
    pub max_height: f32,
}

impl MenuLook {
    /// Comp's selector: baseline text 12 px in.
    pub const BASELINE: Self = Self { inset: 12.0, centred: false, max_height: 560.0 };
    /// Baseline text 10 px in.
    pub const BASELINE_10: Self = Self { inset: 10.0, centred: false, max_height: 560.0 };
    /// Centred text; a selector lower than [`MODEL_H`] (a switch row's) gets the smaller type.
    pub const CENTRED: Self = Self { inset: 12.0, centred: true, max_height: 560.0 };
}

/// `text` with its baseline at `at.y`, from `at.x`.
fn baseline_text(p: &Painter, at: Pos2, text: &str, f: egui::FontId, c: Color32) {
    let size = f.size;
    p.text(pos2(at.x, at.y - size * 0.95), Align2::LEFT_TOP, text, f, c);
}

/// The model selector with Comp's look ([`MenuLook::BASELINE`]); see [`model_menu_ex`].
#[allow(clippy::too_many_arguments)]
pub fn model_menu(
    ui: &mut Ui,
    r: Rect,
    salt: &str,
    label: &str,
    pal: &VstPalette,
    entries: &[MenuEntry],
    selected: usize,
    open_now: bool,
) -> Option<usize> {
    model_menu_ex(ui, r, salt, label, pal, entries, selected, open_now, MenuLook::BASELINE)
}

/// A many-way picker as one full-width selector in `r` showing the current choice
/// ("EL8 · Distress") and a menu of every choice, each with its description under it. `label`
/// names it for accessibility and tests ("Model", "Circuit"); `open_now` opens the menu this
/// frame (the scripts' drawer tabs). Returns the entry picked (never the current one).
#[allow(clippy::too_many_arguments)]
pub fn model_menu_ex(
    ui: &mut Ui,
    r: Rect,
    salt: &str,
    label: &str,
    pal: &VstPalette,
    entries: &[MenuEntry],
    selected: usize,
    open_now: bool,
    look: MenuLook,
) -> Option<usize> {
    let resp = ui.interact(r, ui.id().with(("slim-model-menu", salt)), Sense::click());
    let cur = entries.get(selected);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, label));
    let p = ui.painter();
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let b = r.shrink(0.5);
    p.rect_filled(b, CornerRadius::same(5), al(Color32::BLACK, 0.22));
    let edge = if open || resp.hovered() { pal.accent } else { al(Color32::WHITE, 0.12) };
    p.rect_stroke(b, CornerRadius::same(5), Stroke::new(1.0, edge), StrokeKind::Middle);
    if let Some(s) = cur {
        let clip = Rect::from_min_max(b.min, pos2(b.right() - 26.0, b.bottom()));
        let pc = p.with_clip_rect(clip.intersect(p.clip_rect()));
        if look.centred {
            let big = r.height() >= MODEL_H;
            let (nf, tf) = if big { (bold(13.0), regular(10.5)) } else { (bold(11.5), regular(10.0)) };
            let x = b.left() + if big { look.inset } else { 8.0 };
            let nw = pc.layout_no_wrap(s.name.to_string(), nf.clone(), Color32::WHITE).size().x;
            pc.text(pos2(x, b.center().y), Align2::LEFT_CENTER, s.name, nf, pal.accent);
            if !s.tag.is_empty() {
                pc.text(pos2(x + nw + 6.0, b.center().y + 0.5), Align2::LEFT_CENTER, format!("\u{00B7}  {}", s.tag), tf, pal.text_dim);
            }
        } else {
            let (nf, tf) = (bold(13.0), regular(10.5));
            let nw = p.layout_no_wrap(s.name.to_string(), nf.clone(), Color32::WHITE).size().x;
            let x = b.left() + look.inset;
            let base = (b.center().y + 4.5).round();
            baseline_text(&pc, pos2(x, base), s.name, nf, pal.accent);
            if !s.tag.is_empty() {
                baseline_text(&pc, pos2(x + nw + 6.0, base), &format!("\u{00B7}  {}", s.tag), tf, pal.text_dim);
            }
        }
    }
    let c = pos2(b.right() - 14.0, b.center().y);
    let a = if open { -1.0 } else { 1.0 };
    p.add(egui::Shape::convex_polygon(
        vec![pos2(c.x - 4.5, c.y - 2.0 * a), pos2(c.x + 4.5, c.y - 2.0 * a), pos2(c.x, c.y + 3.0 * a)],
        pal.text_dim,
        Stroke::NONE,
    ));
    let resp = match cur {
        Some(s) if !open && !(s.blurb.is_empty() && s.credit.is_empty()) => {
            resp.on_hover_text(crate::hardware_credit::with_credit(s.blurb, s.credit))
        }
        _ => resp,
    };

    let mut picked = None;
    let mut popup = egui::Popup::menu(&resp)
        .width(r.width())
        .gap(4.0)
        .frame(
            egui::Frame::new()
                .fill(rgb(0x1C1D20))
                .stroke(Stroke::new(1.0, al(Color32::WHITE, 0.12)))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(4.0),
        )
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    if open_now {
        popup = popup.open_memory(Some(egui::SetOpenCommand::Bool(true)));
    }
    let shared_tag = entries.len() > 1 && entries.windows(2).all(|p| p[0].tag == p[1].tag);
    popup.show(|ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let w = r.width() - 8.0;
        egui::ScrollArea::vertical().max_height(look.max_height).show(ui, |ui| {
            for (i, s) in entries.iter().enumerate() {
                let blurb = (!s.blurb.is_empty()).then(|| {
                    let job = egui::text::LayoutJob::simple(s.blurb.to_string(), regular(9.5), pal.text_faint, w - 20.0);
                    ui.fonts_mut(|f| f.layout_job(job))
                });
                let h = 6.0 + 15.0 + blurb.as_ref().map_or(0.0, |g| 2.0 + g.size().y) + 7.0;
                let (cell, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                let show_tag = !s.tag.is_empty() && !shared_tag;
                resp.widget_info(|| {
                    let name = if show_tag { format!("{} {}", s.name, s.tag) } else { s.name.to_string() };
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
                });
                let resp = if s.credit.is_empty() { resp } else { resp.on_hover_text(s.credit) };
                let on = i == selected;
                let p = ui.painter();
                if on {
                    p.rect_filled(cell, CornerRadius::same(4), al(pal.accent, 0.16));
                    p.rect_filled(Rect::from_min_size(cell.min + vec2(0.0, 4.0), vec2(3.0, cell.height() - 8.0)), CornerRadius::same(1), pal.accent);
                } else if resp.hovered() {
                    p.rect_filled(cell, CornerRadius::same(4), al(Color32::WHITE, 0.05));
                }
                let (nf, tf) = (bold(12.0), regular(9.0));
                let nw = p.layout_no_wrap(s.name.to_string(), nf.clone(), Color32::WHITE).size().x;
                let x = cell.left() + 10.0;
                let name_c = if on { pal.accent } else { pal.text_bright };
                let pc = p.with_clip_rect(cell.intersect(p.clip_rect()));
                if look.centred {
                    let mid = cell.top() + 6.0 + 7.5;
                    p.text(pos2(x, mid), Align2::LEFT_CENTER, s.name, nf, name_c);
                    if show_tag {
                        pc.text(pos2(x + nw + 5.0, mid + 0.5), Align2::LEFT_CENTER, s.tag, tf, pal.text_faint);
                    }
                } else {
                    let base = cell.top() + 6.0 + 12.0;
                    baseline_text(p, pos2(x, base), s.name, nf, name_c);
                    if show_tag {
                        baseline_text(&pc, pos2(x + nw + 5.0, base), s.tag, tf, pal.text_faint);
                    }
                }
                if let Some(g) = blurb {
                    p.galley(pos2(x, cell.top() + 6.0 + 15.0 + 2.0), g, pal.text_faint);
                }
                if i + 1 < entries.len() {
                    p.hline(cell.x_range().shrink(6.0), cell.bottom() - 0.5, Stroke::new(1.0, al(Color32::WHITE, 0.05)));
                }
                if resp.clicked() {
                    picked = Some(i);
                    ui.close();
                }
            }
        });
    });
    picked.filter(|i| *i != selected)
}

/// The display toggle in `r` (the display's top-left corner): which of the full editor's
/// side-by-side displays is shown ("GR | Curve"). Returns the one picked.
pub fn display_toggle(ui: &mut Ui, r: Rect, salt: &str, pal: &VstPalette, options: &[&str], shown: usize) -> Option<usize> {
    segmented_ex(ui, r, ("slim-display", salt), pal, options, 1, Some(shown), SelectionStyle::Filled, false)
}

/// Where a [`display_toggle`] of `labels` goes in `display`: its top-left corner, sized to the
/// labels (at least 70 px).
pub fn toggle_rect(display: Rect, labels: &[&str]) -> Rect {
    let w = 10.0 + labels.iter().map(|l| 8.0 + 6.0 * l.chars().count() as f32).sum::<f32>();
    Rect::from_min_size(display.left_top() + vec2(6.0, 6.0), vec2(w.max(70.0), 16.0))
}

/// Names what the display in `r` shows, for the accessibility tree (tests).
pub fn display_label(ui: &Ui, r: Rect, salt: &str, shown: &str) {
    ui.interact(r, ui.id().with(("slim-display-label", salt)), Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, shown));
}

/// `text` wrapped to `r`'s width from its top-left, in the regular face at `size`, at most
/// `max_rows` lines (the last cut with an ellipsis). Returns its height (0 when empty).
pub fn wrapped(ui: &Ui, r: Rect, text: &str, size: f32, colour: Color32, max_rows: usize) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let mut job = egui::text::LayoutJob::simple(text.to_string(), regular(size), colour, r.width().max(1.0));
    job.wrap.max_rows = max_rows.max(1);
    let g = ui.fonts_mut(|f| f.layout_job(job));
    let h = g.size().y;
    ui.painter().galley(r.min, g, colour);
    h
}

/// A caption (a blurb) at the kit's caption size: [`wrapped`] at [`CAPTION_SIZE`].
pub fn caption(ui: &Ui, r: Rect, text: &str, colour: Color32, max_rows: usize) -> f32 {
    wrapped(ui, r, text, CAPTION_SIZE, colour, max_rows)
}

/// A label at the left of a row (the full editor's "Circuit", "Variant" captions), clipped to
/// `r`; faint when `dim`.
pub fn row_label(ui: &Ui, r: Rect, text: &str, pal: &VstPalette, dim: bool) {
    ui.painter().with_clip_rect(r.intersect(ui.clip_rect())).text(
        pos2(r.left(), r.center().y),
        Align2::LEFT_CENTER,
        text,
        regular(LABEL_SIZE),
        if dim { pal.text_faint } else { pal.text_body },
    );
}

/// A switch row's name over its control, a little smaller than a [`row_label`].
pub fn switch_label(ui: &Ui, r: Rect, text: &str, pal: &VstPalette) {
    ui.painter().text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, regular(10.5), pal.text_body);
}

/// A slot for a panel painted under controls drawn after it: [`panel_under`] fills it once the
/// panel's extent is known.
pub fn panel_slot(p: &Painter) -> egui::layers::ShapeIdx {
    p.add(egui::Shape::Noop)
}

/// The family's panel (`paintPanel`) in `slot`, behind what was drawn since.
pub fn panel_under(p: &Painter, slot: egui::layers::ShapeIdx, r: Rect, corner: f32) {
    let c = CornerRadius::same(corner.clamp(0.0, 255.0) as u8);
    p.set(
        slot,
        egui::Shape::Vec(vec![
            egui::Shape::rect_filled(r, c, al(Color32::BLACK, 0.4)),
            egui::Shape::rect_stroke(r.shrink(0.5), c, Stroke::new(1.0, al(Color32::WHITE, 0.07)), StrokeKind::Middle),
        ]),
    );
}
