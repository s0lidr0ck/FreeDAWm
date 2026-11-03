//! Port of `Meter.h`.
//!
//! Meter: time-signature tables (4/4, 6/8, 3/4). Pure, no JUCE. A step is always a 16th note.
//! 4/4 helpers return exactly the values the engine used before meters existed.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Meter {
    #[default]
    M44 = 0,
    M68 = 1,
    M34 = 2,
}

impl Meter {
    /// `(Meter) v` for a stored meter index (0 4/4, 1 6/8, 2 3/4); anything else is 4/4.
    pub fn from_index(v: i32) -> Meter {
        match v {
            1 => Meter::M68,
            2 => Meter::M34,
            _ => Meter::M44,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterInfo {
    pub steps_per_bar: i32,
    pub beats_per_bar: i32,
    pub steps_per_beat: i32,
    pub bar_ppq: f32,
    pub name: &'static str,
    pub num: i32,
    pub den: i32,
}

pub const fn info(m: Meter) -> MeterInfo {
    match m {
        Meter::M68 => MeterInfo { steps_per_bar: 12, beats_per_bar: 2, steps_per_beat: 6, bar_ppq: 3.0, name: "6/8", num: 6, den: 8 },
        Meter::M34 => MeterInfo { steps_per_bar: 12, beats_per_bar: 3, steps_per_beat: 4, bar_ppq: 3.0, name: "3/4", num: 3, den: 4 },
        Meter::M44 => MeterInfo { steps_per_bar: 16, beats_per_bar: 4, steps_per_beat: 4, bar_ppq: 4.0, name: "4/4", num: 4, den: 4 },
    }
}
pub const fn is12(m: Meter) -> bool {
    !matches!(m, Meter::M44)
}
pub const fn is_beat(m: Meter, sib: i32) -> bool {
    sib % info(m).steps_per_beat == 0
}
pub const fn beat_of(m: Meter, sib: i32) -> i32 {
    sib / info(m).steps_per_beat
}
pub const fn is_last_beat(m: Meter, sib: i32) -> bool {
    beat_of(m, sib) == info(m).beats_per_bar - 1
}
pub const fn half_step(m: Meter) -> i32 {
    if matches!(m, Meter::M44) { 8 } else { 6 }
}
pub const fn fill_zone_start(m: Meter) -> i32 {
    match m {
        Meter::M44 => 12,
        Meter::M68 => 9,
        Meter::M34 => 8,
    }
}
pub const fn is_backbeat(m: Meter, sib: i32) -> bool {
    match m {
        Meter::M44 => sib == 4 || sib == 12,
        Meter::M68 => sib == 6,
        Meter::M34 => false,
    }
}
pub const fn is_light_beat(m: Meter, sib: i32) -> bool {
    matches!(m, Meter::M34) && (sib == 4 || sib == 8)
}
/// The bar's strong steps: 4/4 beats 1 and 3, 6/8 both dotted beats, 3/4 the downbeat.
pub const fn strong_step(m: Meter, sib: i32) -> bool {
    if matches!(m, Meter::M34) { sib == 0 } else { sib % half_step(m) == 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GridRole {
    Drums,
    Bass,
    Comp,
    Ensemble,
    Melodic,
}

/// 6/8 target-in-half (0..5) -> source 16th within the 8-step half, per spec 3.2.
pub const fn accent_src(m: Meter, sib12: i32) -> i32 {
    if !matches!(m, Meter::M68) {
        return sib12;
    }
    const H: [i32; 6] = [0, 1, 2, 4, 6, 7];
    // C++ indexes h[sib12 % 6]; the step is never negative there (wrapped here instead of UB).
    (sib12 / 6) * 8 + H[(sib12 % 6).rem_euclid(6) as usize]
}

pub mod meter_detail {
    use super::GridRole;

    /// Per-half lookup (target 0..5 -> source offset within the 8-step half, or -1) for each 6/8 role.
    pub const fn lookup(role: GridRole, t: i32) -> i32 {
        match role {
            GridRole::Drums => {
                if t == 0 {
                    0
                } else if t == 1 {
                    1
                } else if t == 2 {
                    2
                } else if t == 3 {
                    4
                } else if t == 4 {
                    6
                } else {
                    7
                }
            }
            GridRole::Bass => {
                if t == 0 {
                    0
                } else if t == 4 {
                    4
                } else {
                    -1
                }
            }
            GridRole::Comp => {
                if t == 0 {
                    0
                } else if t == 2 {
                    2
                } else if t == 4 {
                    6
                } else {
                    -1
                }
            }
            GridRole::Ensemble => {
                if t == 0 {
                    0
                } else {
                    -1
                }
            }
            GridRole::Melodic => {
                if t == 0 {
                    0
                } else if t == 1 {
                    2
                } else if t == 2 {
                    3
                } else if t == 3 {
                    4
                } else if t == 4 {
                    6
                } else {
                    7
                }
            }
        }
    }
}

/// Source 16-step index whose hit plays at sib12 (per-index part only), or -1. Bar-context rules live in gridStep.
pub const fn map16(m: Meter, sib12: i32, role: GridRole) -> i32 {
    if !matches!(m, Meter::M68) {
        return sib12;
    }
    let o = meter_detail::lookup(role, sib12 % 6);
    if o < 0 { -1 } else { (sib12 / 6) * 8 + o }
}

/// Re-accent factors (spec 3.1 / 3.2) applied to the 12-step accent row.
pub fn accent12_scale(m: Meter, sib12: i32) -> f32 {
    if m == Meter::M34 {
        return if sib12 == 0 {
            1.15
        } else if sib12 == 4 || sib12 == 8 {
            0.9
        } else {
            1.0
        };
    }
    if m == Meter::M68 {
        return if sib12 == 0 {
            1.15
        } else if sib12 == 6 {
            1.05
        } else if sib12 == 3 || sib12 == 9 {
            0.92
        } else {
            1.0
        };
    }
    1.0
}

/// Host time signature -> meter. Anything but 4/4, 3/4, 6/8 (12/8 included) falls back to 4/4 and sets unsupported
/// (the second value).
pub fn meter_from_host(num: i32, den: i32) -> (Meter, bool) {
    if num == 4 && den == 4 {
        return (Meter::M44, false);
    }
    if num == 3 && den == 4 {
        return (Meter::M34, false);
    }
    if num == 6 && den == 8 {
        return (Meter::M68, false);
    }
    (Meter::M44, true)
}
pub fn meter_name(m: Meter) -> &'static str {
    info(m).name
}
pub fn meter_from_name(n: Option<&str>) -> Meter {
    if let Some(n) = n {
        if n == "6/8" {
            return Meter::M68;
        }
        if n == "3/4" {
            return Meter::M34;
        }
    }
    Meter::M44
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_cpp() {
        assert_eq!(info(Meter::M68).steps_per_beat, 6);
        assert_eq!(accent_src(Meter::M68, 9), 12);
        assert_eq!(map16(Meter::M68, 10, GridRole::Bass), 12);
        assert_eq!(map16(Meter::M68, 7, GridRole::Bass), -1);
        assert!(strong_step(Meter::M68, 6) && !strong_step(Meter::M34, 4));
        assert_eq!(meter_from_name(Some("3/4")), Meter::M34);
        assert_eq!(meter_from_name(None), Meter::M44);
    }
}
