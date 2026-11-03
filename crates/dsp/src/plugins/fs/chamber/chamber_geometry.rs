//! `ChamberGeometry.h`, translated line by line: the room, and what a sound does in it.
//!
//! Everything geometric lives here (room outline, wall normals, the `confine()` walk, objects,
//! speaker and microphone placement, the absorption budget, Eyring's RT60 and the image-source
//! tap list), and, as in the C++, BOTH the DSP and the editor (the strip's room plan) call it, so
//! the picture cannot disagree with the sound.
//!
//! No allocation anywhere in here: every structure is fixed-size, so the DSP can rebuild the room
//! on the audio thread exactly as the C++ does.
//!
//! C++ semantics kept deliberately: `float` maths in the C++ order of operations (`f32` here),
//! `std::pow` / `std::log` / `std::sqrt` / `std::cos` / `std::sin` on floats as `f32` calls,
//! `std::min` / `std::max` / `clampT` as plain comparisons (their NaN behaviour never arises: the
//! host clamps every parameter into 0..1 or a whole choice index first).

// The C++ names and argument lists are kept for line-by-line review.
#![allow(clippy::too_many_arguments, clippy::needless_range_loop, clippy::manual_clamp)]

use std::ops::{Add, Mul, Sub};

// ------------------------------------------------------------------ sizes
pub const MAX_WALLS: usize = 6;
/// 0..nW-1 walls, 6 floor, 7 ceiling.
pub const NUM_SURFACES: usize = 8;
pub const SURF_FLOOR: usize = 6;
pub const SURF_CEILING: usize = 7;
pub const NUM_MATERIALS: usize = 15;
pub const NUM_OBJECT_TYPES: usize = 9;
pub const MAX_OBJECTS: usize = 8;
pub const NUM_SHAPES: usize = 6;
pub const NUM_SPEAKERS: usize = 5;
pub const NUM_MIC_PATTERNS: usize = 4;

/// Per microphone: 0 is the direct sound, 1..8 are first-order reflections off surface a,
/// 9..72 are second-order (a*8+b), 73..80 are the objects. Two microphones, so 162 in all.
pub const TAPS_PER_MIC: usize = 81;
pub const NUM_TAPS: usize = TAPS_PER_MIC * 2;
pub const TAP_DIRECT: usize = 0;
pub const TAP_FIRST: usize = 1;
pub const TAP_SECOND: usize = 9;
pub const TAP_OBJECT: usize = 73;

pub const SPEED_OF_SOUND: f32 = 343.0;
/// Nothing is allowed closer than this to a wall, in metres.
pub const WALL_MARGIN: f32 = 0.12;

/// `inline constexpr float pi = 3.14159265358979323846f`.
pub const PI: f32 = std::f32::consts::PI;

// ------------------------------------------------------------------ the two vector types
/// A quantity measured in three bands: roughly 125-250 Hz, 500 Hz-1 kHz, 2-4 kHz.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Band3 {
    pub lo: f32,
    pub mid: f32,
    pub hi: f32,
}

impl Band3 {
    pub const fn new(lo: f32, mid: f32, hi: f32) -> Self {
        Band3 { lo, mid, hi }
    }

    /// `explicit Band3 (float all)`.
    pub const fn all(v: f32) -> Self {
        Band3 { lo: v, mid: v, hi: v }
    }

    /// `operator[]` (const): 0 lo, 1 mid, anything else hi.
    pub fn get(&self, i: usize) -> f32 {
        if i == 0 {
            self.lo
        } else if i == 1 {
            self.mid
        } else {
            self.hi
        }
    }

    /// `operator[]` (non-const).
    pub fn set(&mut self, i: usize, v: f32) {
        if i == 0 {
            self.lo = v;
        } else if i == 1 {
            self.mid = v;
        } else {
            self.hi = v;
        }
    }

    /// `operator* (float s)`.
    pub fn scale(self, s: f32) -> Band3 {
        Band3 { lo: self.lo * s, mid: self.mid * s, hi: self.hi * s }
    }

    /// `operator+ (float s)`.
    pub fn plus(self, s: f32) -> Band3 {
        Band3 { lo: self.lo + s, mid: self.mid + s, hi: self.hi + s }
    }

    /// `operator*= (const Band3&)`.
    pub fn mul_assign(&mut self, o: Band3) {
        self.lo *= o.lo;
        self.mid *= o.mid;
        self.hi *= o.hi;
    }

    /// `operator+= (const Band3&)`.
    pub fn add_assign(&mut self, o: Band3) {
        self.lo += o.lo;
        self.mid += o.mid;
        self.hi += o.hi;
    }

    pub fn sum_abs(&self) -> f32 {
        self.lo.abs() + self.mid.abs() + self.hi.abs()
    }
}

impl Mul for Band3 {
    type Output = Band3;
    fn mul(self, o: Band3) -> Band3 {
        Band3 { lo: self.lo * o.lo, mid: self.mid * o.mid, hi: self.hi * o.hi }
    }
}

impl Add for Band3 {
    type Output = Band3;
    fn add(self, o: Band3) -> Band3 {
        Band3 { lo: self.lo + o.lo, mid: self.mid + o.mid, hi: self.hi + o.hi }
    }
}

impl Sub for Band3 {
    type Output = Band3;
    fn sub(self, o: Band3) -> Band3 {
        Band3 { lo: self.lo - o.lo, mid: self.mid - o.mid, hi: self.hi - o.hi }
    }
}

/// `std::max` for floats: `(a < b) ? b : a`.
#[inline]
pub fn fmax(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// `std::min` for floats: `(b < a) ? b : a`.
#[inline]
pub fn fmin(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

/// `clampT`: `v < lo ? lo : (v > hi ? hi : v)`.
#[inline]
pub fn clamp_f(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `clampT` on ints.
#[inline]
pub fn clamp_i(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

pub fn band_sqrt(b: Band3) -> Band3 {
    Band3 { lo: fmax(b.lo, 0.0).sqrt(), mid: fmax(b.mid, 0.0).sqrt(), hi: fmax(b.hi, 0.0).sqrt() }
}

pub fn band_min(a: Band3, b: Band3) -> Band3 {
    Band3 { lo: fmin(a.lo, b.lo), mid: fmin(a.mid, b.mid), hi: fmin(a.hi, b.hi) }
}

/// A position or a direction in the room. x across, y along, z up.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }

    /// `operator* (float s)`.
    pub fn scale(self, s: f32) -> Vec3 {
        Vec3 { x: self.x * s, y: self.y * s, z: self.z * s }
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3 { x: self.x + o.x, y: self.y + o.y, z: self.z + o.z }
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3 { x: self.x - o.x, y: self.y - o.y, z: self.z - o.z }
    }
}

pub fn dot3(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

pub fn len3(a: Vec3) -> f32 {
    fmax(dot3(a, a), 1.0e-12).sqrt()
}

pub fn norm3(a: Vec3) -> Vec3 {
    a.scale(1.0 / len3(a))
}

// ------------------------------------------------------------------ materials
#[derive(Debug, Clone, Copy)]
pub struct Material {
    pub name: &'static str,
    /// 125-250 / 500-1k / 2-4k.
    pub absorption: Band3,
    /// How rough the surface is.
    pub scatter: f32,
}

const fn mat(name: &'static str, lo: f32, mid: f32, hi: f32, scatter: f32) -> Material {
    Material { name, absorption: Band3::new(lo, mid, hi), scatter }
}

/// `materials()`: the ordinary published absorption coefficients.
pub static MATERIALS: [Material; NUM_MATERIALS] = [
    mat("Painted Concrete", 0.010, 0.015, 0.020, 0.04),
    mat("Hard Plaster", 0.015, 0.025, 0.035, 0.05),
    mat("Glazed Tile", 0.010, 0.012, 0.018, 0.03),
    mat("Bare Brick", 0.030, 0.040, 0.060, 0.20),
    mat("Rough Stone", 0.020, 0.045, 0.070, 0.45),
    mat("Steel Plate", 0.015, 0.020, 0.030, 0.03),
    mat("Glass", 0.220, 0.050, 0.030, 0.02),
    mat("Hardwood", 0.120, 0.080, 0.070, 0.08),
    mat("Wood Panel", 0.300, 0.120, 0.080, 0.12),
    mat("Drywall", 0.250, 0.080, 0.060, 0.05),
    mat("Packed Earth", 0.150, 0.300, 0.450, 0.35),
    mat("Carpet", 0.050, 0.300, 0.600, 0.15),
    mat("Heavy Drape", 0.150, 0.550, 0.720, 0.30),
    mat("Acoustic Foam", 0.150, 0.700, 0.950, 0.30),
    mat("Painted Cinder Block", 0.080, 0.060, 0.090, 0.15),
];

// ------------------------------------------------------------------ objects
pub const RES_NONE: i32 = 0;
pub const RES_POT: i32 = 1;
pub const RES_BARREL: i32 = 2;

#[derive(Debug, Clone, Copy)]
pub struct ObjectSpec {
    pub name: &'static str,
    /// Metres, across Size.
    pub radius_min: f32,
    pub radius_max: f32,
    pub height_min: f32,
    pub height_max: f32,
    pub absorption: Band3,
    /// How much it throws energy off the mirror paths.
    pub scatter: f32,
    /// How hard a shadow it casts.
    pub occlusion: f32,
    /// none | pot | barrel
    pub resonator: i32,
    /// f0 = this / radius
    pub res_f0_times_radius: f32,
    /// Equivalent absorption area = this * radius^2.
    pub res_area_per_radius_sq: f32,
}

#[allow(clippy::too_many_arguments)]
const fn obj(
    name: &'static str,
    radius_min: f32,
    radius_max: f32,
    height_min: f32,
    height_max: f32,
    a: [f32; 3],
    scatter: f32,
    occlusion: f32,
    resonator: i32,
    res_f0_times_radius: f32,
    res_area_per_radius_sq: f32,
) -> ObjectSpec {
    ObjectSpec {
        name,
        radius_min,
        radius_max,
        height_min,
        height_max,
        absorption: Band3::new(a[0], a[1], a[2]),
        scatter,
        occlusion,
        resonator,
        res_f0_times_radius,
        res_area_per_radius_sq,
    }
}

/// `objectSpecs()`.
pub static OBJECT_SPECS: [ObjectSpec; NUM_OBJECT_TYPES] = [
    obj("None", 0.10, 0.10, 0.10, 0.10, [0.00, 0.00, 0.00], 0.00, 0.0, RES_NONE, 0.0, 0.0),
    obj("Concrete Pillar", 0.15, 0.60, 3.00, 3.00, [0.01, 0.02, 0.02], 1.00, 1.0, RES_NONE, 0.0, 0.0),
    obj("Ceramic Pot", 0.08, 0.35, 0.20, 0.80, [0.02, 0.03, 0.03], 0.35, 0.6, RES_POT, 38.0, 6.4),
    obj("Wood Diffuser", 0.30, 1.00, 1.20, 2.20, [0.12, 0.10, 0.08], 1.60, 0.8, RES_NONE, 0.0, 0.0),
    obj("Heavy Drape", 0.30, 1.00, 1.80, 2.40, [0.25, 0.70, 0.85], 0.30, 0.9, RES_NONE, 0.0, 0.0),
    obj("Bass Trap", 0.25, 0.60, 2.00, 2.40, [0.85, 0.50, 0.30], 0.20, 0.8, RES_NONE, 0.0, 0.0),
    obj("Steel Barrel", 0.20, 0.45, 0.90, 1.30, [0.03, 0.03, 0.04], 0.80, 1.0, RES_BARREL, 42.0, 2.5),
    obj("Sofa", 0.50, 1.10, 0.70, 0.90, [0.35, 0.60, 0.65], 0.40, 0.7, RES_NONE, 0.0, 0.0),
    obj("Glass Gobo", 0.40, 1.00, 1.60, 2.00, [0.20, 0.05, 0.03], 0.90, 0.9, RES_NONE, 0.0, 0.0),
];

/// Diffusers, drapes, sofas and gobos are panels: they have a facing direction.
pub fn object_is_flat(ty: i32) -> bool {
    ty == 3 || ty == 4 || ty == 7 || ty == 8
}

/// `speakerDirectivity()`: how much each speaker beams, per band.
pub static SPEAKER_DIRECTIVITY: [Band3; NUM_SPEAKERS] = [
    Band3::new(0.10, 0.45, 0.80), // Studio Monitor
    Band3::new(0.15, 0.60, 0.92), // Horn Duplex
    Band3::new(0.10, 0.50, 0.85), // Dual Concentric
    Band3::new(0.05, 0.35, 0.75), // Small Cone
    Band3::new(0.20, 0.70, 0.95), // PA Column
];

// ------------------------------------------------------------------ the room outline
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    pub num_walls: usize,
    /// Vertices, counter-clockwise.
    pub vx: [f32; MAX_WALLS],
    pub vy: [f32; MAX_WALLS],
    /// Inward normals.
    pub nx: [f32; MAX_WALLS],
    pub ny: [f32; MAX_WALLS],
    /// Edge unit vector and length.
    pub ex: [f32; MAX_WALLS],
    pub ey: [f32; MAX_WALLS],
    pub elen: [f32; MAX_WALLS],

    pub width_m: f32,
    pub length_m: f32,
    pub height_m: f32,
    pub centre_x: f32,
    pub centre_y: f32,

    /// Sloped ceiling: z = ceilH0 - ceilM * y.
    pub ceil_h0: f32,
    pub ceil_m: f32,
    pub ceil_norm: f32,

    pub floor_area: f32,
    pub perimeter: f32,
}

impl Default for Room {
    fn default() -> Self {
        Room {
            num_walls: 4,
            vx: [0.0; MAX_WALLS],
            vy: [0.0; MAX_WALLS],
            nx: [0.0; MAX_WALLS],
            ny: [0.0; MAX_WALLS],
            ex: [0.0; MAX_WALLS],
            ey: [0.0; MAX_WALLS],
            elen: [0.0; MAX_WALLS],
            width_m: 4.0,
            length_m: 7.0,
            height_m: 3.2,
            centre_x: 2.0,
            centre_y: 3.5,
            ceil_h0: 3.2,
            ceil_m: 0.0,
            ceil_norm: 1.0,
            floor_area: 28.0,
            perimeter: 22.0,
        }
    }
}

impl Room {
    pub fn ceiling_at(&self, y: f32) -> f32 {
        self.ceil_h0 - self.ceil_m * y
    }

    /// Signed distance from a point to a surface, positive inside the room.
    pub fn surface_distance(&self, s: usize, p: Vec3) -> f32 {
        if s == SURF_FLOOR {
            return p.z;
        }
        if s == SURF_CEILING {
            return (self.ceil_h0 - self.ceil_m * p.y - p.z) / self.ceil_norm;
        }
        (p.x - self.vx[s]) * self.nx[s] + (p.y - self.vy[s]) * self.ny[s]
    }

    /// The image of a point in a surface.
    pub fn mirror(&self, s: usize, p: Vec3) -> Vec3 {
        let d = self.surface_distance(s, p);
        if s == SURF_FLOOR {
            return Vec3::new(p.x, p.y, p.z - 2.0 * d);
        }
        if s == SURF_CEILING {
            return Vec3::new(p.x, p.y + 2.0 * d * self.ceil_m / self.ceil_norm, p.z + 2.0 * d / self.ceil_norm);
        }
        Vec3::new(p.x - 2.0 * d * self.nx[s], p.y - 2.0 * d * self.ny[s], p.z)
    }

    pub fn surface_active(&self, s: usize) -> bool {
        s < self.num_walls || s >= SURF_FLOOR
    }

    pub fn min_wall_distance(&self, x: f32, y: f32) -> f32 {
        let mut m = 1.0e9f32;
        for i in 0..self.num_walls.min(MAX_WALLS) {
            m = fmin(m, (x - self.vx[i]) * self.nx[i] + (y - self.vy[i]) * self.ny[i]);
        }
        m
    }

    /// Is a point that already lies on a surface's plane inside that surface's outline?
    pub fn within(&self, s: usize, p: Vec3) -> bool {
        const EPS: f32 = 0.002;
        if s >= SURF_FLOOR {
            return self.min_wall_distance(p.x, p.y) >= -EPS;
        }
        if p.z < -EPS || p.z > self.ceiling_at(p.y) + EPS {
            return false;
        }
        let t = (p.x - self.vx[s]) * self.ex[s] + (p.y - self.vy[s]) * self.ey[s];
        t >= -EPS && t <= self.elen[s] + EPS
    }

    /// Pull a 0..1 position into the room: map it onto the bounding box, then walk it toward the
    /// centroid until it is `wallMargin` inside every wall.
    pub fn confine(&self, u: f32, v: f32) -> (f32, f32) {
        let mut x = clamp_f(u, 0.0, 1.0) * self.width_m;
        let mut y = clamp_f(v, 0.0, 1.0) * self.length_m;
        for _ in 0..40 {
            if self.min_wall_distance(x, y) >= WALL_MARGIN {
                break;
            }
            x = self.centre_x + (x - self.centre_x) * 0.93;
            y = self.centre_y + (y - self.centre_y) * 0.93;
        }
        (x, y)
    }
}

/// The floor plan. Shapes: Rectangle, Trapezoid, Pentagon, Hexagon, Skewed, Irregular.
pub fn build_room(shape: i32, w_knob: f32, l_knob: f32, h_knob: f32, taper: f32, ceil_slope: f32) -> Room {
    let mut r = Room {
        width_m: 1.5 * 20.0f32.powf(clamp_f(w_knob, 0.0, 1.0)),
        length_m: 1.5 * 20.0f32.powf(clamp_f(l_knob, 0.0, 1.0)),
        height_m: 1.5 * 10.0f32.powf(clamp_f(h_knob, 0.0, 1.0)),
        ..Room::default()
    };

    let w = r.width_m;
    let l = r.length_m;
    let t = clamp_f(taper, 0.0, 1.0);

    // Up to +-25 % of the height end to end, with the mean height unchanged.
    let k_s = 0.5 * clamp_f(ceil_slope, 0.0, 1.0);
    r.ceil_h0 = r.height_m * (1.0 + 0.5 * k_s);
    r.ceil_m = r.height_m * k_s / l;
    r.ceil_norm = (1.0 + r.ceil_m * r.ceil_m).sqrt();

    let set = |r: &mut Room, i: usize, x: f32, y: f32| {
        r.vx[i] = x;
        r.vy[i] = y;
    };

    if shape == 1 {
        // trapezoid: the far wall is narrower
        let k = 0.45 * t * w;
        r.num_walls = 4;
        set(&mut r, 0, 0.0, 0.0);
        set(&mut r, 1, w, 0.0);
        set(&mut r, 2, w - k, l);
        set(&mut r, 3, k, l);
    } else if shape == 2 {
        // pentagon: pointed far end
        let yy = l * (1.0 - 0.4 * t);
        r.num_walls = 5;
        set(&mut r, 0, 0.0, 0.0);
        set(&mut r, 1, w, 0.0);
        set(&mut r, 2, w, yy);
        set(&mut r, 3, 0.5 * w, l);
        set(&mut r, 4, 0.0, yy);
    } else if shape == 3 {
        // hexagon: both ends angled, near enough a tank
        let c = 0.3 * t * w;
        r.num_walls = 6;
        set(&mut r, 0, c, 0.0);
        set(&mut r, 1, w - c, 0.0);
        set(&mut r, 2, w, 0.5 * l);
        set(&mut r, 3, w - c, l);
        set(&mut r, 4, c, l);
        set(&mut r, 5, 0.0, 0.5 * l);
    } else if shape == 4 {
        // skewed: a parallelogram, so one pair is still parallel
        let k = 0.4 * t * w;
        r.num_walls = 4;
        set(&mut r, 0, 0.0, 0.0);
        set(&mut r, 1, w - k, 0.0);
        set(&mut r, 2, w, l);
        set(&mut r, 3, k, l);
    } else if shape == 5 {
        // irregular: no two walls parallel, so nothing flutters
        r.num_walls = 4;
        set(&mut r, 0, 0.0, 0.0);
        set(&mut r, 1, w, 0.12 * t * l);
        set(&mut r, 2, w * (1.0 - 0.22 * t), l);
        set(&mut r, 3, 0.10 * t * w, l * (1.0 - 0.18 * t));
    } else {
        // rectangle
        r.num_walls = 4;
        set(&mut r, 0, 0.0, 0.0);
        set(&mut r, 1, w, 0.0);
        set(&mut r, 2, w, l);
        set(&mut r, 3, 0.0, l);
    }

    // Edges, inward normals, area by the shoelace formula, perimeter, centroid.
    let mut a2 = 0.0f32;
    let mut per = 0.0f32;
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;

    let n = r.num_walls;
    for i in 0..n {
        let j = if i + 1 >= n { 0 } else { i + 1 };
        let dx = r.vx[j] - r.vx[i];
        let dy = r.vy[j] - r.vy[i];
        let ln = fmax((dx * dx + dy * dy).sqrt(), 1.0e-4);

        r.ex[i] = dx / ln;
        r.ey[i] = dy / ln;
        r.elen[i] = ln;
        r.nx[i] = -dy / ln;
        r.ny[i] = dx / ln;

        a2 += r.vx[i] * r.vy[j] - r.vx[j] * r.vy[i];
        per += ln;
        cx += r.vx[i];
        cy += r.vy[i];
    }

    r.floor_area = fmax(0.5 * a2, 0.5);
    r.perimeter = per;
    r.centre_x = cx / n as f32;
    r.centre_y = cy / n as f32;

    r
}

// ------------------------------------------------------------------ objects in the room
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectState {
    pub on: bool,
    pub flat: bool,
    pub ty: i32,
    /// x, y, and z carrying the object's TOP height.
    pub pos: Vec3,
    pub radius: f32,
    /// Unit vector along a flat object's face.
    pub ux: f32,
    pub uy: f32,
}

impl Default for ObjectState {
    fn default() -> Self {
        ObjectState { on: false, flat: false, ty: 0, pos: Vec3::default(), radius: 0.1, ux: 1.0, uy: 0.0 }
    }
}

pub type Objects = [ObjectState; MAX_OBJECTS];

pub fn build_objects(
    room: &Room,
    ty: &[i32; MAX_OBJECTS],
    x: &[f32; MAX_OBJECTS],
    y: &[f32; MAX_OBJECTS],
    size: &[f32; MAX_OBJECTS],
    rotate: &[f32; MAX_OBJECTS],
) -> Objects {
    let mut out = [ObjectState::default(); MAX_OBJECTS];

    for k in 0..MAX_OBJECTS {
        let t = clamp_i(ty[k], 0, NUM_OBJECT_TYPES as i32 - 1);
        let o = &mut out[k];
        o.ty = t;
        o.on = t > 0;
        if !o.on {
            continue;
        }

        let sp = &OBJECT_SPECS[t as usize];
        let s = clamp_f(size[k], 0.0, 1.0);
        let a = sp.radius_min + s * (sp.radius_max - sp.radius_min);
        let mut h = sp.height_min + s * (sp.height_max - sp.height_min);

        let (px, py) = room.confine(x[k], y[k]);

        if t == 1 {
            h = room.ceiling_at(py); // pillars run floor to ceiling
        }
        if t == 2 {
            h = 2.2 * a; // a pot is about as tall as it is wide
        }
        h = fmin(h, room.ceiling_at(py));

        o.pos = Vec3::new(px, py, h);
        o.radius = a;
        o.flat = object_is_flat(t);
        o.ux = (PI * rotate[k]).cos();
        o.uy = (PI * rotate[k]).sin();
    }

    out
}

/// How much of a path from P to Q the objects block, per band.
pub fn occlusion(objects: &Objects, p: Vec3, q: Vec3) -> Band3 {
    let mut g = Band3::new(1.0, 1.0, 1.0);

    let dx = q.x - p.x;
    let dy = q.y - p.y;
    let l2 = fmax(dx * dx + dy * dy, 1.0e-6);

    for o in objects {
        if !o.on {
            continue;
        }

        let a = o.radius;
        let occl = OBJECT_SPECS[o.ty as usize].occlusion;

        if o.flat {
            // Which side of the panel each end is on, and where along it the path crosses.
            let pn = -(p.x - o.pos.x) * o.uy + (p.y - o.pos.y) * o.ux;
            let qn = -(q.x - o.pos.x) * o.uy + (q.y - o.pos.y) * o.ux;

            if pn * qn < 0.0 {
                let tc = pn / (pn - qn);
                let pu = (p.x - o.pos.x) * o.ux + (p.y - o.pos.y) * o.uy;
                let qu = (q.x - o.pos.x) * o.ux + (q.y - o.pos.y) * o.uy;
                let along = (pu + tc * (qu - pu)).abs();
                let czf = p.z + tc * (q.z - p.z);

                if along < a && czf < o.pos.z && tc > 0.02 && tc < 0.98 {
                    let grazing = clamp_f((dx * o.uy - dy * o.ux).abs() / (l2.sqrt() * 0.26), 0.0, 1.0);
                    let shf = clamp_f((a - along) / (0.35 * a), 0.0, 1.0) * occl * grazing;
                    g.mul_assign(Band3::new(1.0 - 0.25 * shf * fmin(1.0, a), 1.0 - 0.60 * shf, 1.0 - 0.90 * shf));
                }
            }
        }

        // Round objects, and the body of a panel: nearest approach of the path to the axis.
        let t = clamp_f(((o.pos.x - p.x) * dx + (o.pos.y - p.y) * dy) / l2, 0.0, 1.0);
        let cx = p.x + t * dx - o.pos.x;
        let cy = p.y + t * dy - o.pos.y;
        let cz = p.z + t * (q.z - p.z);
        let dd = (cx * cx + cy * cy).sqrt();

        if !o.flat && dd < a && cz < o.pos.z && t > 0.02 && t < 0.98 {
            let sh = ((a - dd) / a) * occl;
            g.mul_assign(Band3::new(1.0 - 0.25 * sh * fmin(1.0, a), 1.0 - 0.60 * sh, 1.0 - 0.90 * sh));
        }
    }

    g
}

// ------------------------------------------------------------------ placement
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Placement {
    pub speaker: Vec3,
    pub mic_a: Vec3,
    pub mic_b: Vec3,
    pub speaker_aim: Vec3,
    pub mic_a_aim: Vec3,
    pub mic_b_aim: Vec3,
}

pub fn build_placement(
    room: &Room,
    spk_x: f32,
    spk_y: f32,
    spk_z: f32,
    spk_aim: f32,
    mic_ax: f32,
    mic_ay: f32,
    mic_bx: f32,
    mic_by: f32,
    mic_z: f32,
    mic_a_aim: f32,
    mic_b_aim: f32,
) -> Placement {
    let place = |u: f32, v: f32, height_frac: f32| {
        let (x, y) = room.confine(u, v);
        // Heights are a fraction of the ceiling WHERE THE THING STANDS.
        Vec3::new(x, y, room.ceiling_at(y) * height_frac)
    };

    let speaker = place(spk_x, spk_y, 0.1 + 0.8 * clamp_f(spk_z, 0.0, 1.0));

    let mz_f = 0.1 + 0.8 * clamp_f(mic_z, 0.0, 1.0);
    let mic_a = place(mic_ax, mic_ay, mz_f);
    let mic_b = place(mic_bx, mic_by, mz_f);

    let aim = |knob: f32| Vec3::new((2.0 * PI * knob).cos(), (2.0 * PI * knob).sin(), 0.0);

    Placement { speaker, mic_a, mic_b, speaker_aim: aim(spk_aim), mic_a_aim: aim(mic_a_aim), mic_b_aim: aim(mic_b_aim) }
}

// ------------------------------------------------------------------ the acoustics budget
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acoustics {
    /// Per band, seconds.
    pub rt60: Band3,
    /// Absorption area, m^2.
    pub total_absorption: Band3,
    pub volume: f32,
    pub surface_area: f32,
    /// 0 mirror-like, 1 fully diffuse.
    pub scatter: f32,
    pub object_scatter: f32,
    pub surface_reflection: [Band3; NUM_SURFACES],
    /// Mean free path and the mean late-field delay the FDN lines are scaled around.
    pub mean_free_path: f32,
    pub mean_line_seconds: f32,
    /// The sixteen room proportions the FDN line lengths come from, in metres.
    pub line_proportions: [f32; 16],
}

impl Default for Acoustics {
    fn default() -> Self {
        Acoustics {
            rt60: Band3::default(),
            total_absorption: Band3::default(),
            volume: 90.0,
            surface_area: 126.0,
            scatter: 0.1,
            object_scatter: 0.0,
            surface_reflection: [Band3::default(); NUM_SURFACES],
            mean_free_path: 1.0,
            mean_line_seconds: 0.06,
            line_proportions: [0.0; 16],
        }
    }
}

pub fn build_acoustics(
    room: &Room,
    objects: &Objects,
    wall_mat: i32,
    floor_mat: i32,
    ceil_mat: i32,
    shape: i32,
    taper: f32,
    ceil_slope: f32,
    decay_knob: f32,
) -> Acoustics {
    let mut ac = Acoustics::default();

    let h = room.height_m;
    let wall_area = room.perimeter * h;
    ac.surface_area = room.floor_area * (1.0 + room.ceil_norm) + wall_area;

    // ---- what the objects take out of the room, and out of the mirror paths
    let mut obj_vol = 0.0f32;
    let mut obj_a = Band3::new(0.0, 0.0, 0.0);
    let mut keep = 1.0f32;
    let cross = room.floor_area.sqrt() * h;

    for o in objects {
        if !o.on {
            continue;
        }

        let sp = &OBJECT_SPECS[o.ty as usize];
        let a = o.radius;
        let oh = o.pos.z;

        if o.flat && o.ty != 7 {
            // a panel: two faces, hardly any volume. Not a sofa.
            obj_vol += 0.2 * a * oh;
            obj_a.add_assign(sp.absorption.scale(4.0 * a * oh));
        } else {
            obj_vol += PI * a * a * oh;
            obj_a.add_assign(sp.absorption.scale(2.0 * PI * a * oh + PI * a * a));
        }

        keep *= 1.0 - clamp_f(sp.scatter * (2.0 * a * oh) / cross, 0.0, 0.7);
    }

    ac.object_scatter = 1.0 - keep;
    ac.volume = fmax(room.floor_area * h - obj_vol, 1.0);

    let mw = clamp_i(wall_mat, 0, NUM_MATERIALS as i32 - 1) as usize;
    let mf = clamp_i(floor_mat, 0, NUM_MATERIALS as i32 - 1) as usize;
    let mc = clamp_i(ceil_mat, 0, NUM_MATERIALS as i32 - 1) as usize;

    // The door, the cables, the speaker and the microphones themselves.
    let residual = 0.3 + 0.004 * ac.surface_area;

    ac.total_absorption = (MATERIALS[mw].absorption.scale(wall_area)
        + (MATERIALS[mf].absorption + MATERIALS[mc].absorption.scale(room.ceil_norm)).scale(room.floor_area)
        + obj_a)
        .plus(residual);

    // ---- how diffuse the tail is: materials, objects, non-parallel walls, sloped ceiling
    let mat_scatter =
        (MATERIALS[mw].scatter * wall_area + (MATERIALS[mf].scatter + MATERIALS[mc].scatter * room.ceil_norm) * room.floor_area) / ac.surface_area;

    let mut shape_sc = 0.0f32;
    if shape == 2 || shape == 3 {
        shape_sc = 0.08 + 0.17 * taper;
    } else if shape != 0 {
        shape_sc = 0.30 * taper;
    }

    let slope_sc = 0.15 * clamp_f(ceil_slope, 0.0, 1.0);

    ac.scatter = clamp_f(1.0 - (1.0 - mat_scatter) * (1.0 - ac.object_scatter) * (1.0 - shape_sc) * (1.0 - slope_sc), 0.0, 0.95);

    // ---- Eyring's RT60 per band, with air absorption.
    let decay_mul = 4.0f32.powf(2.0 * clamp_f(decay_knob, 0.0, 1.0) - 1.0);
    let air_m = Band3::new(0.0002, 0.001, 0.009);

    for b in 0..3 {
        let al = clamp_f(ac.total_absorption.get(b) / ac.surface_area, 0.002, 0.95);
        let r = 0.161 * ac.volume / (-ac.surface_area * (1.0 - al).ln() + 4.0 * air_m.get(b) * ac.volume);
        ac.rt60.set(b, clamp_f(r * decay_mul, 0.06, 25.0));
    }

    // ---- per-surface amplitude reflection.
    let wsc = Band3::new(0.3, 0.7, 1.0);
    let one = Band3::new(1.0, 1.0, 1.0);

    for s in 0..NUM_SURFACES {
        let mut m = mw;
        if s == SURF_FLOOR {
            m = mf;
        }
        if s == SURF_CEILING {
            m = mc;
        }
        ac.surface_reflection[s] = band_sqrt((one - MATERIALS[m].absorption) * (one - wsc.scale(MATERIALS[m].scatter)));
    }

    // ---- the late field's timing, from the room's own proportions
    ac.mean_free_path = 4.0 * ac.volume / ac.surface_area;
    ac.mean_line_seconds = clamp_f(2.2 * ac.mean_free_path / SPEED_OF_SOUND, 0.035, 0.120);

    let w = room.floor_area / room.length_m; // an equivalent width
    let l = room.length_m;
    let p = &mut ac.line_proportions;

    p[0] = 2.0 * w;
    p[1] = 2.0 * l;
    p[2] = 2.0 * h;
    p[3] = 2.0 * (w * w + l * l).sqrt();
    p[4] = 2.0 * (w * w + h * h).sqrt();
    p[5] = 2.0 * (l * l + h * h).sqrt();
    p[6] = 2.0 * (w * w + l * l + h * h).sqrt();
    p[7] = w + l;
    p[8] = l + h;
    p[9] = w + h;
    p[10] = w + l + h;
    p[11] = (4.0 * w * w + l * l).sqrt();
    p[12] = (w * w + 4.0 * l * l).sqrt();
    p[13] = (4.0 * h * h + l * l).sqrt();
    p[14] = (4.0 * h * h + w * w).sqrt();
    p[15] = (4.0 * w * w + 4.0 * l * l + h * h).sqrt();

    ac
}

// ------------------------------------------------------------------ taps
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tap {
    pub gain: Band3,
    /// Metres.
    pub distance: f32,
    pub live: bool,
    /// Where the last bounce happened, for the editor to draw. Not used by the DSP.
    pub reflection_point: Vec3,
    pub has_point: bool,
}

pub type Taps = [Tap; NUM_TAPS];

/// The polar response of one speaker in one band.
pub fn speaker_lobe(place: &Placement, speaker_type: i32, emit: Vec3) -> Band3 {
    let ce = dot3(norm3(emit), place.speaker_aim);
    let sh = clamp_f(0.5 + 0.5 * ce, 0.0, 1.0); // 1 on axis, 0 behind
    let k = SPEAKER_DIRECTIVITY[clamp_i(speaker_type, 0, NUM_SPEAKERS as i32 - 1) as usize];

    let mut out = Band3::default();
    for b in 0..3 {
        let beam = clamp_f(k.get(b), 0.0, 0.99);
        let power = 1.0 + 2.75 * beam; // how tight the lobe is
        let floor_g = (1.0 - beam).powf(1.6); // how much leaks out the back
        out.set(b, floor_g + (1.0 - floor_g) * sh.powf(power));
    }
    out
}

/// The polar response of one microphone in one band.
pub fn mic_polar(mic_pattern: i32, arrive: Vec3, mic_aim: Vec3) -> Band3 {
    // The omnidirectional share of each pattern: 1 omni, 0.5 cardioid, 0.25 hyper, 0 fig-8.
    let mut a = 1.0f32;
    if mic_pattern == 1 {
        a = 0.5;
    } else if mic_pattern == 2 {
        a = 0.25;
    } else if mic_pattern == 3 {
        a = 0.0;
    }

    // How far each band is dragged back toward omni.
    let omni_pull = Band3::new(0.55, 0.15, 0.0);
    let cos_a = dot3(norm3(arrive), mic_aim);

    let mut out = Band3::default();
    for b in 0..3 {
        let a_b = 0.04 + 0.96 * (a + (1.0 - a) * omni_pull.get(b));
        out.set(b, a_b + (1.0 - a_b) * cos_a);
    }
    out
}

/// Everything that depends on where the sound left the speaker and how it arrived.
pub fn path_gain(
    place: &Placement,
    speaker_type: i32,
    mic_pattern: i32,
    emit: Vec3,
    arrive: Vec3,
    mic_aim: Vec3,
    air_dist: f32,
    level_dist: f32,
) -> Band3 {
    // Air absorption, about 0.0005 / 0.004 / 0.035 dB per metre.
    let air = Band3::new(10.0f32.powf(-0.000025 * air_dist), 10.0f32.powf(-0.0002 * air_dist), 10.0f32.powf(-0.00175 * air_dist));

    (speaker_lobe(place, speaker_type, emit) * mic_polar(mic_pattern, arrive, mic_aim) * air).scale(1.0 / fmax(level_dist, 0.5))
}

fn set_tap(taps: &mut Taps, idx: usize, g: Band3, dist: f32, point: Vec3, has_point: bool) {
    if let Some(t) = taps.get_mut(idx) {
        t.gain = g;
        t.distance = dist;
        t.live = true;
        t.reflection_point = point;
        t.has_point = has_point;
    }
}

fn clear_tap(taps: &mut Taps, idx: usize) {
    if let Some(t) = taps.get_mut(idx) {
        t.gain = Band3::new(0.0, 0.0, 0.0);
        t.live = false;
        t.has_point = false;
    }
}

/// The image-source solve for one microphone: direct sound, every first-order reflection,
/// every VALID second-order reflection, and one scattered path per object.
pub fn build_taps(
    taps: &mut Taps,
    mic: usize,
    room: &Room,
    objects: &Objects,
    ac: &Acoustics,
    place: &Placement,
    speaker_type: i32,
    mic_pattern: i32,
) {
    let base = mic * TAPS_PER_MIC;
    let s = place.speaker;
    let m = if mic == 0 { place.mic_a } else { place.mic_b };
    let mic_aim = if mic == 0 { place.mic_a_aim } else { place.mic_b_aim };

    // Objects knock energy out of the mirror-like paths; a second bounce pays it twice.
    let spec1 = 1.0 - 0.35 * ac.object_scatter;

    // ---- direct
    {
        let dist = len3(m - s);
        let g = path_gain(place, speaker_type, mic_pattern, m - s, s - m, mic_aim, dist, dist) * occlusion(objects, s, m);
        set_tap(taps, base + TAP_DIRECT, g, dist, s, false);
    }

    for a in 0..NUM_SURFACES {
        let slot1 = base + TAP_FIRST + a;
        let mut ok1 = false;

        if !room.surface_active(a) {
            for b in 0..NUM_SURFACES {
                clear_tap(taps, base + TAP_SECOND + a * 8 + b);
            }
            clear_tap(taps, slot1);
            continue;
        }

        let s1 = room.mirror(a, s);
        let d_m = room.surface_distance(a, m);
        let d_s = room.surface_distance(a, s1);

        if d_m > 1.0e-4 && d_s < -1.0e-4 {
            let u = d_m / (d_m - d_s);
            let p = m + (s1 - m).scale(u);

            if room.within(a, p) {
                let dist = len3(s1 - m);
                let g = (path_gain(place, speaker_type, mic_pattern, p - s, s1 - m, mic_aim, dist, dist)
                    * ac.surface_reflection[a]
                    * occlusion(objects, s, p)
                    * occlusion(objects, p, m))
                .scale(spec1);
                set_tap(taps, slot1, g, dist, p, true);
                ok1 = true;
            }
        }

        // ---- second order: off a, then off b
        for b in 0..NUM_SURFACES {
            let slot2 = base + TAP_SECOND + a * 8 + b;
            let mut ok2 = false;

            if b != a && room.surface_active(b) && room.surface_distance(b, s1) > 1.0e-4 {
                let s2 = room.mirror(b, s1);
                let d_m2 = room.surface_distance(b, m);
                let d_s2 = room.surface_distance(b, s2);

                if d_m2 > 1.0e-4 && d_s2 < -1.0e-4 {
                    let u2 = d_m2 / (d_m2 - d_s2);
                    let p2 = m + (s2 - m).scale(u2);

                    if room.within(b, p2) {
                        let d2a = room.surface_distance(a, p2);
                        let d1a = room.surface_distance(a, s1);

                        if d2a > 1.0e-4 && d1a < -1.0e-4 {
                            let u1 = d2a / (d2a - d1a);
                            let p1 = p2 + (s1 - p2).scale(u1);

                            if room.within(a, p1) {
                                let dist = len3(s2 - m);
                                let g = (path_gain(place, speaker_type, mic_pattern, p1 - s, s2 - m, mic_aim, dist, dist)
                                    * ac.surface_reflection[a]
                                    * ac.surface_reflection[b])
                                    .scale(spec1 * spec1);
                                set_tap(taps, slot2, g, dist, p2, true);
                                ok2 = true;
                            }
                        }
                    }
                }
            }

            if !ok2 {
                clear_tap(taps, slot2);
            }
        }

        if !ok1 {
            clear_tap(taps, slot1);
        }
    }

    // ---- objects: speaker -> object -> microphone
    for (k, o) in objects.iter().enumerate() {
        let slot = base + TAP_OBJECT + k;

        if !o.on {
            clear_tap(taps, slot);
            continue;
        }

        let sp = &OBJECT_SPECS[o.ty as usize];
        let a = o.radius;
        let oz = clamp_f(0.5 * (s.z + m.z), 0.05, o.pos.z);
        let ob = Vec3::new(o.pos.x, o.pos.y, oz);

        let d1 = len3(ob - s);
        let d2 = len3(m - ob);

        // Small things only scatter short wavelengths. (ka)^2, capped at 1.
        let ka = Band3::new(3.66 * a, 18.3 * a, 91.6 * a);
        let one = Band3::new(1.0, 1.0, 1.0);
        let ray = band_min(ka * ka, one);
        let refl = band_sqrt(one - sp.absorption);

        let mut facing = 1.0f32;
        if o.flat {
            let ns = (-(s.x - ob.x) * o.uy + (s.y - ob.y) * o.ux) / fmax(d1, 0.05);
            let nm = (-(m.x - ob.x) * o.uy + (m.y - ob.y) * o.ux) / fmax(d2, 0.05);
            facing = 0.3; // edge-on, or the speaker and mic are opposite sides
            if ns * nm > 0.0 {
                facing = 0.3 + 1.4 * (ns.abs() * nm.abs()).sqrt();
            }
        }

        // scatter is capped at 2, not 1 (the C++'s own fix for the Wood Diffuser).
        let g = (path_gain(place, speaker_type, mic_pattern, ob - s, ob - m, mic_aim, d1 + d2, 1.0) * ray * refl)
            .scale(facing * 0.5 * a * fmin(sp.scatter, 2.0) / (fmax(d1, 0.3) * fmax(d2, 0.3)));

        set_tap(taps, slot, g, d1 + d2, ob, true);
    }
}

// ------------------------------------------------------------------ the whole chamber
/// Every geometry-affecting parameter, in one place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    pub shape: i32,
    pub room_w: f32,
    pub room_l: f32,
    pub room_h: f32,
    pub taper: f32,
    pub ceil_slope: f32,
    pub wall_mat: i32,
    pub floor_mat: i32,
    pub ceil_mat: i32,

    pub spk_x: f32,
    pub spk_y: f32,
    pub spk_z: f32,
    pub spk_aim: f32,
    pub spk_type: i32,

    pub mic_ax: f32,
    pub mic_ay: f32,
    pub mic_bx: f32,
    pub mic_by: f32,
    pub mic_z: f32,
    pub mic_a_aim: f32,
    pub mic_b_aim: f32,
    pub mic_pattern: i32,

    pub obj_type: [i32; MAX_OBJECTS],
    pub obj_x: [f32; MAX_OBJECTS],
    pub obj_y: [f32; MAX_OBJECTS],
    pub obj_size: [f32; MAX_OBJECTS],
    pub obj_rotate: [f32; MAX_OBJECTS],

    pub decay: f32,
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            shape: 5,
            room_w: 0.3059,
            room_l: 0.4683,
            room_h: 0.3153,
            taper: 0.7,
            ceil_slope: 0.4,
            wall_mat: 0,
            floor_mat: 0,
            ceil_mat: 0,
            spk_x: 0.25,
            spk_y: 0.15,
            spk_z: 0.3,
            spk_aim: 0.625,
            spk_type: 1,
            mic_ax: 0.3,
            mic_ay: 0.8,
            mic_bx: 0.7,
            mic_by: 0.8,
            mic_z: 0.45,
            mic_a_aim: 0.375,
            mic_b_aim: 0.125,
            mic_pattern: 0,
            obj_type: [0; MAX_OBJECTS],
            obj_x: [0.5; MAX_OBJECTS],
            obj_y: [0.5; MAX_OBJECTS],
            obj_size: [0.5; MAX_OBJECTS],
            obj_rotate: [0.0; MAX_OBJECTS],
            decay: 0.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chamber {
    pub room: Room,
    pub objects: Objects,
    pub placement: Placement,
    pub acoustics: Acoustics,
    pub taps: Taps,
}

impl Default for Chamber {
    fn default() -> Self {
        Chamber {
            room: Room::default(),
            objects: [ObjectState::default(); MAX_OBJECTS],
            placement: Placement::default(),
            acoustics: Acoustics::default(),
            taps: [Tap::default(); NUM_TAPS],
        }
    }
}

impl Chamber {
    /// How many taps actually survived the validity tests, per microphone.
    pub fn live_taps(&self, mic: usize) -> i32 {
        let mut n = 0;
        for i in 0..TAPS_PER_MIC {
            if self.taps.get(mic * TAPS_PER_MIC + i).is_some_and(|t| t.live) {
                n += 1;
            }
        }
        n
    }
}

/// `build (const Knobs&)`: the whole room from the knobs, written into `c` in place (the C++
/// returns it by value; the DSP rebuilds on the audio thread, so no new value is made here).
pub fn build_into(c: &mut Chamber, k: &Knobs) {
    c.room = build_room(k.shape, k.room_w, k.room_l, k.room_h, k.taper, k.ceil_slope);
    c.objects = build_objects(&c.room, &k.obj_type, &k.obj_x, &k.obj_y, &k.obj_size, &k.obj_rotate);
    c.acoustics = build_acoustics(&c.room, &c.objects, k.wall_mat, k.floor_mat, k.ceil_mat, k.shape, k.taper, k.ceil_slope, k.decay);
    c.placement =
        build_placement(&c.room, k.spk_x, k.spk_y, k.spk_z, k.spk_aim, k.mic_ax, k.mic_ay, k.mic_bx, k.mic_by, k.mic_z, k.mic_a_aim, k.mic_b_aim);

    // The C++ `Chamber c;` starts with default taps; every slot is then set or cleared.
    c.taps = [Tap::default(); NUM_TAPS];
    build_taps(&mut c.taps, 0, &c.room, &c.objects, &c.acoustics, &c.placement, k.spk_type, k.mic_pattern);
    build_taps(&mut c.taps, 1, &c.room, &c.objects, &c.acoustics, &c.placement, k.spk_type, k.mic_pattern);
}

/// `build (const Knobs&)`.
pub fn build(k: &Knobs) -> Chamber {
    let mut c = Chamber::default();
    build_into(&mut c, k);
    c
}
