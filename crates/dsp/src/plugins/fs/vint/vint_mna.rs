//! VintMna -- a circuit solved as a circuit.
//! Line-by-line translation of `Engine 11 - Vint/Source/VintMna.h`.
//!
//! Modified nodal analysis of a network of resistors, capacitors, inductors, ideal op-amps and
//! voltage-controlled voltage sources driven by one voltage source, discretised by the
//! TRAPEZOIDAL rule (what SPICE does in a transient analysis). Each reactive part becomes a
//! conductance plus a current source carrying its history:
//!
//! ```text
//!     capacitor   i = Gc v - J,  Gc = 2C/T,  J = Gc v' + i'      (primes: the previous sample)
//!     inductor    i = Gl v + J,  Gl = T/2L,  J = i' + Gl v'
//! ```
//!
//! Every sample is one linear solve. The running path (`run`) is reduced at `set` time to what a
//! sample needs: each reactive part's voltage and the output as linear functions of the input and
//! the histories. `open_circuit` / `commit` keep the full node solution, for circuits that
//! something nonlinear loads at a node. `ac` solves the same netlist in the frequency domain.
//!
//! Rust differences, none of which changes a computed value:
//! - The C++ keeps its solve matrices in `static thread_local` arrays; here they live in a
//!   [`Scratch`] the caller owns (the engine allocates one at construction), so nothing is
//!   allocated or put on the audio thread's stack. Every solve zeroes what it uses first, as the
//!   C++ does, so the scratch's previous contents never matter.
//! - The C++ allocates a solver's inverse (`invStore`) on the first `set()`, which can be on the
//!   audio thread; here it is allocated with the solver.
//! - `transfer`/`commit` read the inverse at whatever indices they are given. The C++ reads past
//!   the solved `M x M` block into never-written heap memory when the engine asks for a node the
//!   netlist does not have (the Passive's input-transformer node, 12, in the plugin build); that is
//!   indeterminate there. Here those entries read 0, the value a never-solved entry holds.
//! - Node and part indices that a malformed netlist could push out of range are bounds-checked
//!   (ignored) rather than undefined; the engine's netlists never get near the limits.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use rustfft::num_complex::Complex;

pub const K_MAX_NODES: usize = 48;
pub const K_MAX_PARTS: usize = 96;
pub const K_MAX_ACTIVE: usize = 12;
pub const K_MAX_REACTIVE: usize = 32;
pub const K_MAX_UNKNOWNS: usize = K_MAX_NODES + K_MAX_ACTIVE;
const KU: usize = K_MAX_UNKNOWNS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Kind {
    #[default]
    R,
    C,
    L,
    OpAmp,
    Vcvs,
}

/// R / C / L between a and b. OpAmp and Vcvs: a = non-inverting input, b = inverting input,
/// c = output; a Vcvs's value is its gain.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Part {
    pub kind: Kind,
    pub a: i32,
    pub b: i32,
    pub value: f64,
    pub c: i32,
}

/// A netlist: node 0 is ground, node 1 is the source (driven), `out` is read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Netlist {
    pub parts: [Part; K_MAX_PARTS],
    pub count: i32,
    pub nodes: i32,
    pub out: i32,
}

impl Default for Netlist {
    fn default() -> Self {
        Netlist { parts: [Part::default(); K_MAX_PARTS], count: 0, nodes: 2, out: 1 }
    }
}

impl Netlist {
    fn push(&mut self, p: Part) {
        if let Some(slot) = usize::try_from(self.count).ok().and_then(|c| self.parts.get_mut(c)) {
            *slot = p;
            self.count += 1;
        }
    }

    pub fn add(&mut self, k: Kind, a: i32, b: i32, v: f64) {
        self.push(Part { kind: k, a, b, value: v, c: 0 });
        self.nodes = self.nodes.max(a.max(b) + 1);
    }

    /// An ideal op-amp: out drives whatever makes v(plus) == v(minus).
    pub fn add_op_amp(&mut self, plus: i32, minus: i32, out_node: i32) {
        self.push(Part { kind: Kind::OpAmp, a: plus, b: minus, value: 0.0, c: out_node });
        self.nodes = self.nodes.max(plus.max(minus).max(out_node) + 1);
    }

    /// A voltage-controlled voltage source: v(out) = gain (v(plus) - v(minus)).
    pub fn add_vcvs(&mut self, plus: i32, minus: i32, out_node: i32, gain: f64) {
        self.push(Part { kind: Kind::Vcvs, a: plus, b: minus, value: gain, c: out_node });
        self.nodes = self.nodes.max(plus.max(minus).max(out_node) + 1);
    }

    /// The parts in use.
    pub fn used(&self) -> &[Part] {
        let n = usize::try_from(self.count).unwrap_or(0).min(K_MAX_PARTS);
        self.parts.get(..n).unwrap_or(&[])
    }

    pub fn active_count(&self) -> i32 {
        self.used().iter().filter(|p| p.kind >= Kind::OpAmp).count() as i32
    }
}

/// A node's unknown index (`node - 2`), when the node is an unknown.
#[inline]
fn unk(node: i32) -> Option<usize> {
    if node >= 2 { usize::try_from(node - 2).ok().filter(|&u| u < KU) } else { None }
}

/// The arithmetic the stamp needs, for `f64` and `Complex<f64>`.
pub trait Scalar: Copy + std::ops::AddAssign + std::ops::Neg<Output = Self> {
    fn of(v: f64) -> Self;
}
impl Scalar for f64 {
    fn of(v: f64) -> Self {
        v
    }
}
impl Scalar for Complex<f64> {
    fn of(v: f64) -> Self {
        Complex::new(v, 0.0)
    }
}

/// Stamps the netlist into A (unknowns: nodes 2.., then each active part's output current) with
/// admittances `y(part index, part)`, and the source's column into src. Returns M.
pub fn stamp<T: Scalar>(n: &Netlist, a_mat: &mut [[T; KU]; KU], src: &mut [T; KU], mut y: impl FnMut(usize, &Part) -> T) -> usize {
    let u = usize::try_from(n.nodes - 2).unwrap_or(0);
    let mut m = u;
    let at = |a_mat: &mut [[T; KU]; KU], src: &mut [T; KU], row: Option<usize>, node: i32, v: T| {
        let Some(row) = row else { return };
        if node == 1 {
            if let Some(s) = src.get_mut(row) {
                *s += v;
            }
        } else if let Some(c) = unk(node)
            && let Some(cell) = a_mat.get_mut(row).and_then(|r| r.get_mut(c))
        {
            *cell += v;
        }
    };
    for (p, e) in n.used().iter().enumerate() {
        if e.kind <= Kind::L {
            let g = y(p, e);
            let (ra, rb) = (unk(e.a), unk(e.b));
            at(a_mat, src, ra, e.a, g);
            at(a_mat, src, ra, e.b, -g);
            at(a_mat, src, rb, e.b, g);
            at(a_mat, src, rb, e.a, -g);
        } else {
            let k = m; // this part's output current
            m += 1;
            if let Some(c) = unk(e.c)
                && let Some(cell) = a_mat.get_mut(c).and_then(|r| r.get_mut(k))
            {
                *cell += T::of(-1.0); // injected into its output node
            }
            if e.kind == Kind::OpAmp {
                at(a_mat, src, Some(k), e.a, T::of(1.0));
                at(a_mat, src, Some(k), e.b, T::of(-1.0));
            } else {
                at(a_mat, src, Some(k), e.c, T::of(1.0));
                at(a_mat, src, Some(k), e.a, T::of(-e.value));
                at(a_mat, src, Some(k), e.b, T::of(e.value));
            }
        }
    }
    m.min(KU)
}

/// The analog response at f (Hz): exact, by complex Gaussian elimination. UI/setup only (its
/// matrix is allocated here; the C++ keeps it in a thread-local).
pub fn ac(n: &Netlist, f: f64) -> Complex<f64> {
    type Cd = Complex<f64>;
    if n.out == 1 {
        return Cd::new(1.0, 0.0);
    }
    let zero = Cd::new(0.0, 0.0);
    let mut a = vec![[zero; KU]; KU];
    let Ok(a_mat): Result<&mut [[Cd; KU]; KU], _> = a.as_mut_slice().try_into() else { return zero };
    let mut src = [zero; KU];
    let s = Cd::new(0.0, 2.0 * 3.14159265358979323846 * f);
    let m = stamp::<Cd>(n, a_mat, &mut src, |_, e| match e.kind {
        Kind::R => Cd::new(1.0 / e.value, 0.0),
        Kind::C => s * e.value,
        _ => Cd::new(1.0, 0.0) / (s * e.value),
    });
    // A x = -src (the source's known 1 V moved to the right-hand side)
    let mut b = [zero; KU];
    for r in 0..m {
        b[r] = -src[r];
    }
    for c in 0..m {
        let mut piv = c;
        for r in c + 1..m {
            if a_mat[r][c].norm() > a_mat[piv][c].norm() {
                piv = r;
            }
        }
        if piv != c {
            a_mat.swap(c, piv);
            b.swap(c, piv);
        }
        for r in c + 1..m {
            if a_mat[r][c] == zero {
                continue;
            }
            let fct = a_mat[r][c] / a_mat[c][c];
            for k in c..m {
                let v = a_mat[c][k];
                a_mat[r][k] -= fct * v;
            }
            let bc = b[c];
            b[r] -= fct * bc;
        }
    }
    let mut x = [zero; KU];
    for r in (0..m).rev() {
        let mut acc = b[r];
        for k in r + 1..m {
            acc -= a_mat[r][k] * x[k];
        }
        x[r] = acc / a_mat[r][r];
    }
    unk(n.out).and_then(|o| x.get(o).copied()).unwrap_or(zero)
}

/// The solve matrices the C++ keeps in `static thread_local`s.
pub struct Scratch {
    a: [[f64; KU]; KU],
    w: [[f64; 2 * KU]; KU],
    xj: [[f64; KU]; K_MAX_REACTIVE],
}

impl Scratch {
    pub fn new_boxed() -> Box<Scratch> {
        // Built on the heap a row at a time, so no 100 kB value ever sits on the stack.
        let a: Box<[[f64; KU]; KU]> = vec![[0.0; KU]; KU].into_boxed_slice().try_into().unwrap_or_else(|_| Box::new([[0.0; KU]; KU]));
        let w: Box<[[f64; 2 * KU]; KU]> = vec![[0.0; 2 * KU]; KU].into_boxed_slice().try_into().unwrap_or_else(|_| Box::new([[0.0; 2 * KU]; KU]));
        let xj: Box<[[f64; KU]; K_MAX_REACTIVE]> =
            vec![[0.0; KU]; K_MAX_REACTIVE].into_boxed_slice().try_into().unwrap_or_else(|_| Box::new([[0.0; KU]; K_MAX_REACTIVE]));
        Box::new(Scratch { a: *a, w: *w, xj: *xj })
    }

    /// Zeroes the scratch in place (its contents never matter to a solve; a reset clears it
    /// anyway, so a reset engine is bit-for-bit a new one).
    pub fn clear(&mut self) {
        self.a.iter_mut().for_each(|r| r.fill(0.0));
        self.w.iter_mut().for_each(|r| r.fill(0.0));
        self.xj.iter_mut().for_each(|r| r.fill(0.0));
    }
}

/// The same network, running: trapezoidal companions, a precomputed inverse.
pub struct Solver {
    pub net: Netlist,
    /// inverse of the reduced system matrix, row-major `KU x KU` (set() only)
    inv: Box<[f64]>,
    src_col: [f64; KU],
    g: [f64; K_MAX_PARTS],
    v_prev: [f64; K_MAX_PARTS],
    i_prev: [f64; K_MAX_PARTS],
    u: usize,
    m: usize,
    t: f64,
    vin_now: f64,
    x_now: [f64; KU],
    j_now: [f64; K_MAX_PARTS],
    // the running path's reduction: reactive parts' voltages and the output, as
    //   v_q = volV[q] vin + sum_r volJ[q][r] J_r,   y = outV vin + sum_r outJ[r] J_r
    nr: usize,
    reactive: [usize; K_MAX_REACTIVE],
    vol_v: [f64; K_MAX_REACTIVE],
    vol_j: [[f64; K_MAX_REACTIVE]; K_MAX_REACTIVE],
    out_v: f64,
    out_j: [f64; K_MAX_REACTIVE],
}

impl Default for Solver {
    fn default() -> Self {
        Solver::with_inv(vec![0.0; KU * KU].into_boxed_slice())
    }
}

impl Solver {
    /// A solver at its defaults around an existing inverse block (zeroed by the caller).
    fn with_inv(inv: Box<[f64]>) -> Self {
        Solver {
            net: Netlist::default(),
            inv,
            src_col: [0.0; KU],
            g: [0.0; K_MAX_PARTS],
            v_prev: [0.0; K_MAX_PARTS],
            i_prev: [0.0; K_MAX_PARTS],
            u: 0,
            m: 0,
            t: 1.0 / 192000.0,
            vin_now: 0.0,
            x_now: [0.0; KU],
            j_now: [0.0; K_MAX_PARTS],
            nr: 0,
            reactive: [0; K_MAX_REACTIVE],
            vol_v: [0.0; K_MAX_REACTIVE],
            vol_j: [[0.0; K_MAX_REACTIVE]; K_MAX_REACTIVE],
            out_v: 0.0,
            out_j: [0.0; K_MAX_REACTIVE],
        }
    }

    /// This solver as `Solver::default()` would build it, reusing its inverse block (zeroed):
    /// no allocation, for a reset on the audio thread. `self` keeps an empty block, which
    /// owns no memory.
    pub fn take_reset(&mut self) -> Solver {
        let mut inv = std::mem::take(&mut self.inv);
        inv.fill(0.0);
        Solver::with_inv(inv)
    }
    #[inline]
    fn iv(&self, r: usize, c: usize) -> f64 {
        if r < KU && c < KU { self.inv.get(r * KU + c).copied().unwrap_or(0.0) } else { 0.0 }
    }

    /// The inverse's heap block (for the allocation guard).
    #[cfg(test)]
    pub fn heap_block(&self) -> usize {
        self.inv.as_ptr() as usize
    }

    fn conductances(&mut self, n: &Netlist) {
        for (p, e) in n.used().iter().enumerate() {
            self.g[p] = match e.kind {
                Kind::R => 1.0 / e.value,
                Kind::C => 2.0 * e.value / self.t,
                Kind::L => self.t / (2.0 * e.value),
                _ => 0.0,
            };
        }
    }

    /// Clears the stamp area for a netlist with `m0` unknowns, then stamps it; returns M.
    fn stamp_into(&mut self, n: &Netlist, sc: &mut Scratch) -> usize {
        let m0 = (self.u + usize::try_from(n.active_count()).unwrap_or(0)).min(KU);
        for r in 0..m0 {
            self.src_col[r] = 0.0;
            sc.a[r][..m0].fill(0.0);
        }
        let g = self.g;
        stamp::<f64>(n, &mut sc.a, &mut self.src_col, |p, _| g.get(p).copied().unwrap_or(0.0))
    }

    /// Rebuild for new part values (same topology): keeps every part's history, so a pot turning
    /// mid-note does not click.
    pub fn set(&mut self, n: &Netlist, fs: f64, sc: &mut Scratch) {
        let same_shape = n.count == self.net.count && n.nodes == self.net.nodes;
        self.net = *n;
        self.t = 1.0 / fs;
        self.u = usize::try_from(n.nodes - 2).unwrap_or(0);
        if !same_shape {
            self.reset();
        }
        self.conductances(n);
        let m = self.stamp_into(n, sc);
        self.m = m;

        // invert by Gauss-Jordan with partial pivoting
        let w = &mut sc.w;
        for r in 0..m {
            w[r][..2 * m].fill(0.0);
            w[r][..m].copy_from_slice(&sc.a[r][..m]);
            w[r][m + r] = 1.0;
        }
        for c in 0..m {
            let mut piv = c;
            for r in c + 1..m {
                if w[r][c].abs() > w[piv][c].abs() {
                    piv = r;
                }
            }
            w.swap(c, piv);
            let d = w[c][c];
            for k in 0..2 * m {
                w[c][k] /= d;
            }
            for r in 0..m {
                if r == c || w[r][c] == 0.0 {
                    continue;
                }
                let fct = w[r][c];
                for k in 0..2 * m {
                    let v = w[c][k];
                    w[r][k] -= fct * v;
                }
            }
        }
        for r in 0..m {
            for c in 0..m {
                if let Some(cell) = self.inv.get_mut(r * KU + c) {
                    *cell = w[r][m + c];
                }
            }
        }
        self.reduce(sc);
    }

    pub fn reset(&mut self) {
        self.v_prev = [0.0; K_MAX_PARTS];
        self.i_prev = [0.0; K_MAX_PARTS];
    }

    /// set() for the running path alone (run(), not open_circuit / commit / transfer): the system
    /// is LU-factored and solved only for the columns a sample needs.
    pub fn set_running(&mut self, n: &Netlist, fs: f64, sc: &mut Scratch) {
        let same_shape = n.count == self.net.count && n.nodes == self.net.nodes;
        self.net = *n;
        self.t = 1.0 / fs;
        self.u = usize::try_from(n.nodes - 2).unwrap_or(0);
        if !same_shape {
            self.reset();
        }
        self.conductances(n);
        let m = self.stamp_into(n, sc);
        self.m = m;

        // LU with partial pivoting, in place; perm[r] = the original row now at r
        let a = &mut sc.a;
        let mut perm = [0usize; KU];
        for (r, p) in perm.iter_mut().enumerate().take(m) {
            *p = r;
        }
        for c in 0..m {
            let mut piv = c;
            for r in c + 1..m {
                if a[r][c].abs() > a[piv][c].abs() {
                    piv = r;
                }
            }
            if piv != c {
                a.swap(c, piv);
                perm.swap(c, piv);
            }
            let d = 1.0 / a[c][c];
            for r in c + 1..m {
                if a[r][c] == 0.0 {
                    continue;
                }
                a[r][c] *= d;
                let fct = a[r][c];
                for k in c + 1..m {
                    let v = a[c][k];
                    a[r][k] -= fct * v;
                }
            }
        }
        let solve = |a: &[[f64; KU]; KU], b: &[f64; KU], x: &mut [f64; KU]| {
            for r in 0..m {
                let mut acc = b[perm[r]];
                for k in 0..r {
                    acc -= a[r][k] * x[k];
                }
                x[r] = acc;
            }
            for r in (0..m).rev() {
                let mut acc = x[r];
                for k in r + 1..m {
                    acc -= a[r][k] * x[k];
                }
                x[r] = acc / a[r][r];
            }
        };
        self.collect_reactive();
        let mut b = [0.0f64; KU];
        let mut xv = [0.0f64; KU];
        for r in 0..m {
            b[r] = -self.src_col[r];
        }
        solve(a, &b, &mut xv);
        for q in 0..self.nr {
            let e = self.net.parts[self.reactive[q]];
            b[..m].fill(0.0);
            if let Some(i) = unk(e.a).filter(|&i| i < m) {
                b[i] += 1.0;
            }
            if let Some(i) = unk(e.b).filter(|&i| i < m) {
                b[i] -= 1.0;
            }
            solve(a, &b, &mut sc.xj[q]);
        }
        self.reduce_from(&xv, &sc.xj);
    }

    /// Takes another solver's running path (set by set() or set_running() for the same circuit)
    /// and keeps this one's history: two channels of one circuit share one solve.
    pub fn adopt(&mut self, o: &Solver) {
        let same_shape = o.net.count == self.net.count && o.net.nodes == self.net.nodes;
        self.net = o.net;
        self.t = o.t;
        self.u = o.u;
        self.m = o.m;
        if !same_shape {
            self.reset();
        }
        let count = usize::try_from(self.net.count).unwrap_or(0).min(K_MAX_PARTS);
        self.g[..count].copy_from_slice(&o.g[..count]);
        self.nr = o.nr;
        self.out_v = o.out_v;
        for q in 0..self.nr {
            self.reactive[q] = o.reactive[q];
            self.vol_v[q] = o.vol_v[q];
            self.out_j[q] = o.out_j[q];
            for r in 0..self.nr {
                self.vol_j[q][r] = o.vol_j[q][r];
            }
        }
    }

    /// One sample, the running path: the source at vin, the output node's voltage out.
    #[inline]
    pub fn run(&mut self, vin: f64) -> f64 {
        let nr = self.nr;
        let mut j = [0.0f64; K_MAX_REACTIVE];
        for q in 0..nr {
            let p = self.reactive[q];
            j[q] = if self.net.parts[p].kind == Kind::C {
                self.g[p] * self.v_prev[p] + self.i_prev[p]
            } else {
                -(self.i_prev[p] + self.g[p] * self.v_prev[p])
            };
        }
        let mut y = self.out_v * vin;
        for q in 0..nr {
            y += self.out_j[q] * j[q];
        }
        for q in 0..nr {
            let mut v = self.vol_v[q] * vin;
            let row = &self.vol_j[q];
            for r in 0..nr {
                v += row[r] * j[r];
            }
            let p = self.reactive[q];
            self.i_prev[p] = self.g[p] * v - j[q];
            self.v_prev[p] = v;
        }
        y
    }

    /// The first half of a sample that something nonlinear loads: the output's voltage with
    /// nothing drawn from it. History is not touched; commit() finishes the sample.
    pub fn open_circuit(&mut self, vin: f64) -> f64 {
        self.vin_now = vin;
        let m = self.m;
        let mut rhs = [0.0f64; KU];
        for u in 0..m {
            rhs[u] = -self.src_col[u] * vin;
        }
        let count = usize::try_from(self.net.count).unwrap_or(0).min(K_MAX_PARTS);
        for p in 0..count {
            let e = self.net.parts[p];
            if e.kind != Kind::C && e.kind != Kind::L {
                continue;
            }
            // the history current source, injected as the companion model says
            let j = if e.kind == Kind::C { self.g[p] * self.v_prev[p] + self.i_prev[p] } else { -(self.i_prev[p] + self.g[p] * self.v_prev[p]) };
            self.j_now[p] = j;
            if let Some(i) = unk(e.a) {
                rhs[i] += j;
            }
            if let Some(i) = unk(e.b) {
                rhs[i] -= j;
            }
        }
        for r in 0..m {
            let mut acc = 0.0;
            for (c, x) in rhs.iter().enumerate().take(m) {
                acc += self.iv(r, c) * x;
            }
            self.x_now[r] = acc;
        }
        match unk(self.net.out) {
            Some(o) => self.x_now[o],
            None => self.vin_now,
        }
    }

    /// Node n's voltage this sample with nothing drawn anywhere (after open_circuit).
    pub fn node_open(&self, n: i32) -> f64 {
        match unk(n) {
            Some(i) => self.x_now[i],
            None => {
                if n == 1 {
                    self.vin_now
                } else {
                    0.0
                }
            }
        }
    }

    /// How far node a's voltage falls per ampere drawn from node b (a == b: the driving-point
    /// resistance).
    pub fn transfer(&self, a: i32, b: i32) -> f64 {
        match (unk(a), unk(b)) {
            (Some(i), Some(j)) => self.iv(i, j),
            _ => 0.0,
        }
    }

    /// How far the output's voltage falls per ampere drawn from it, this sample.
    pub fn output_resistance(&self) -> f64 {
        match unk(self.net.out) {
            Some(o) => self.iv(o, o),
            None => 0.0,
        }
    }

    /// Finishes the sample with currents drawn from two nodes (either may be 0 for none);
    /// returns the output's voltage.
    pub fn commit2(&mut self, node_a: i32, i_a: f64, node_b: i32, i_b: f64) -> f64 {
        if let Some(c) = unk(node_a)
            && i_a != 0.0
        {
            for r in 0..self.m {
                self.x_now[r] -= self.iv(r, c) * i_a;
            }
        }
        self.commit_at(node_b, i_b)
    }

    /// Finishes the sample with i_out drawn from the output node; returns its voltage.
    pub fn commit(&mut self, i_out: f64) -> f64 {
        self.commit_at(self.net.out, i_out)
    }

    /// Finishes the sample with i drawn from node n; returns the output's voltage.
    pub fn commit_at(&mut self, n: i32, i: f64) -> f64 {
        if let Some(c) = unk(n)
            && i != 0.0
        {
            for r in 0..self.m {
                self.x_now[r] -= self.iv(r, c) * i;
            }
        }
        let volt = |s: &Solver, node: i32| -> f64 {
            if node == 0 {
                0.0
            } else if node == 1 {
                s.vin_now
            } else {
                unk(node).map(|i| s.x_now[i]).unwrap_or(0.0)
            }
        };
        let count = usize::try_from(self.net.count).unwrap_or(0).min(K_MAX_PARTS);
        for p in 0..count {
            let e = self.net.parts[p];
            if e.kind != Kind::C && e.kind != Kind::L {
                continue;
            }
            let v = volt(self, e.a) - volt(self, e.b);
            self.i_prev[p] = self.g[p] * v - self.j_now[p];
            self.v_prev[p] = v;
        }
        volt(self, self.net.out)
    }

    fn collect_reactive(&mut self) {
        self.nr = 0;
        let count = usize::try_from(self.net.count).unwrap_or(0).min(K_MAX_PARTS);
        for p in 0..count {
            if self.nr >= K_MAX_REACTIVE {
                break;
            }
            let k = self.net.parts[p].kind;
            if k == Kind::C || k == Kind::L {
                self.reactive[self.nr] = p;
                self.nr += 1;
            }
        }
    }

    fn reduce(&mut self, sc: &mut Scratch) {
        self.collect_reactive();
        let m = self.m;
        // the unknowns for vin = 1 (no history), and for each history source alone
        let mut xv = [0.0f64; KU];
        for r in 0..m {
            let mut acc = 0.0;
            for c in 0..m {
                acc -= self.iv(r, c) * self.src_col[c];
            }
            xv[r] = acc;
        }
        for q in 0..self.nr {
            let e = self.net.parts[self.reactive[q]];
            for r in 0..m {
                sc.xj[q][r] = unk(e.a).map(|i| self.iv(r, i)).unwrap_or(0.0) - unk(e.b).map(|i| self.iv(r, i)).unwrap_or(0.0);
            }
        }
        self.reduce_from(&xv, &sc.xj);
    }

    /// The running path from the unknowns for vin = 1 (xv) and for each history source (xj).
    fn reduce_from(&mut self, xv: &[f64; KU], xj: &[[f64; KU]; K_MAX_REACTIVE]) {
        let node_v = |x: &[f64; KU], node: i32, vin: f64| -> f64 {
            if node == 0 {
                0.0
            } else if node == 1 {
                vin
            } else {
                unk(node).map(|i| x[i]).unwrap_or(0.0)
            }
        };
        for q in 0..self.nr {
            let e = self.net.parts[self.reactive[q]];
            self.vol_v[q] = node_v(xv, e.a, 1.0) - node_v(xv, e.b, 1.0);
            for r in 0..self.nr {
                self.vol_j[q][r] = node_v(&xj[r], e.a, 0.0) - node_v(&xj[r], e.b, 0.0);
            }
        }
        self.out_v = node_v(xv, self.net.out, 1.0);
        for r in 0..self.nr {
            self.out_j[r] = node_v(&xj[r], self.net.out, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An RC low-pass: 1k into 159.15n (1 kHz).
    fn rc() -> Netlist {
        let mut n = Netlist::default();
        n.add(Kind::R, 1, 2, 1000.0);
        n.add(Kind::C, 2, 0, 1.0 / (2.0 * std::f64::consts::PI * 1000.0 * 1000.0));
        n.out = 2;
        n
    }

    #[test]
    fn ac_matches_the_rc_formula() {
        let h = ac(&rc(), 1000.0);
        assert!((h.norm() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert!((ac(&rc(), 0.0).norm() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn set_and_set_running_and_open_circuit_agree() {
        let mut sc = Scratch::new_boxed();
        let (mut a, mut b, mut c) = (Solver::default(), Solver::default(), Solver::default());
        a.set(&rc(), 48_000.0, &mut sc);
        b.set_running(&rc(), 48_000.0, &mut sc);
        c.set(&rc(), 48_000.0, &mut sc);
        for n in 0..2000 {
            let x = (n as f64 * 0.05).sin();
            let ya = a.run(x);
            let yb = b.run(x);
            let _ = c.open_circuit(x);
            let yc = c.commit(0.0);
            assert!((ya - yb).abs() < 1e-12 && (ya - yc).abs() < 1e-12, "{n}: {ya} {yb} {yc}");
        }
        // a DC step settles at unity
        let mut y = 0.0;
        for _ in 0..48_000 {
            y = a.run(1.0);
        }
        assert!((y - 1.0).abs() < 1e-9);
        assert!(a.output_resistance() > 0.0 && a.transfer(2, 2) == a.output_resistance());
        assert_eq!(a.transfer(12, 2), 0.0, "a node the netlist lacks reads 0");
    }

    #[test]
    fn adopt_shares_the_solve_and_keeps_history() {
        let mut sc = Scratch::new_boxed();
        let mut a = Solver::default();
        a.set_running(&rc(), 48_000.0, &mut sc);
        let mut b = Solver::default();
        b.adopt(&a);
        for n in 0..500 {
            let x = (n as f64 * 0.1).cos();
            assert_eq!(a.run(x), b.run(x));
        }
    }

    #[test]
    fn op_amp_inverting_gain() {
        let mut n = Netlist::default();
        n.add(Kind::R, 1, 2, 1000.0);
        n.add(Kind::R, 2, 3, 2000.0);
        n.add_op_amp(0, 2, 3);
        n.out = 3;
        assert!((ac(&n, 100.0) - Complex::new(-2.0, 0.0)).norm() < 1e-12);
        let mut sc = Scratch::new_boxed();
        let mut s = Solver::default();
        s.set_running(&n, 48_000.0, &mut sc);
        assert!((s.run(0.5) + 1.0).abs() < 1e-12);
    }
}
