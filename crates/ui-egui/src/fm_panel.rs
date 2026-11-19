//! The FM synth's algorithm laid out for drawing (Steeple's algorithm display): each operator's
//! row and the centre of its box.

use egui::{Pos2, Rect, pos2};
use soundcraft_dsp::plugins::fm::MODULATORS;

/// The FM Synth's plugin id.
pub const FM: &str = "fm_synth";

/// Each operator's row in the diagram (0 the bottom: the operators heard) for algorithm `alg`:
/// a modulator sits one row above the highest operator it modulates.
pub fn rows(alg: usize) -> [usize; 4] {
    let mods = MODULATORS.get(alg).copied().unwrap_or([0; 4]);
    let mut row = [0usize; 4];
    // Modulators always sit above (higher numbered than) what they modulate: fill from 1 up.
    for n in 0..4 {
        for (target, mask) in mods.iter().enumerate() {
            if mask & (1 << n) != 0 {
                row[n] = row[n].max(row[target] + 1);
            }
        }
    }
    row
}

/// The centre of each operator's box in `r` for algorithm `alg`.
pub fn layout(alg: usize, r: Rect) -> [Pos2; 4] {
    let row = rows(alg);
    let top = row.iter().copied().max().unwrap_or(0);
    let mut out = [r.center(); 4];
    for (n, p) in out.iter_mut().enumerate() {
        let same: Vec<usize> = (0..4).filter(|m| row[*m] == row[n]).collect();
        let i = same.iter().position(|m| *m == n).unwrap_or(0);
        let x = r.min.x + r.width() * (i as f32 + 1.0) / (same.len() as f32 + 1.0);
        let y = r.max.y - 14.0 - (r.height() - 28.0) * row[n] as f32 / (top.max(1) as f32);
        *p = pos2(x, if top == 0 { r.max.y - 14.0 } else { y });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn modulators_are_drawn_above_what_they_modulate() {
        assert_eq!(rows(0), [0, 1, 2, 3], "a stack");
        assert_eq!(rows(1), [0, 1, 2, 2]);
        assert_eq!(rows(5), [0, 0, 0, 1]);
        assert_eq!(rows(7), [0, 0, 0, 0]);
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0));
        for (alg, mods) in MODULATORS.iter().enumerate() {
            let at = layout(alg, r);
            for (target, mask) in mods.iter().enumerate() {
                for m in (0..4).filter(|m| mask & (1 << m) != 0) {
                    assert!(at[m].y < at[target].y, "algorithm {alg}: {m} above {target}");
                }
            }
            // No two boxes in the same place.
            for a in 0..4 {
                for b in a + 1..4 {
                    assert!(at[a].distance(at[b]) > 20.0, "algorithm {alg}");
                }
            }
        }
    }
}
