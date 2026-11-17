//! Modulators on the audio thread ([`soundcraft_model::Modulator`]): each block every modulator
//! of a track gets a value at the block's start and end, and each target parameter is moved
//! from the value it has (stored, or its lane's) by `depth` times that, as a share of its range
//! in the plug-in's own taper (so a cutoff sweeps evenly by ear). A plug-in that takes timed
//! changes ([`Plugin::set_param_at`]) gets the value every [`TIMED_GRID`] samples between the
//! two; others once a block. Fixed-size state, no allocation.

use super::{TIMED_GRID, plugin_lane};
use soundcraft_dsp::{Plugin, Transport};
use soundcraft_model::{Insert, MAX_MODULATORS, ModulatorKind, ModuleId, Track};
use soundcraft_time::Samples;

/// One modulator's running state.
#[derive(Debug, Clone, Copy, Default)]
struct State {
    /// LFO phase, in cycles.
    phase: f64,
    /// The envelope follower's level (linear).
    env: f32,
    /// The values at the start and the end of the current block.
    from: f32,
    to: f32,
}

/// A strip's modulators' state and this block's values, matched to the track's list by
/// position.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModRun {
    states: [State; MAX_MODULATORS],
}

/// The envelope follower's level as its value: 0 at -60 dBFS and below, 1 at 0 dBFS.
fn env_value(level: f32) -> f32 {
    if level <= 1e-3 { 0.0 } else { ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0) }
}

impl ModRun {
    /// Works out every modulator's values for the block of `frames` at `rate`, from `input`
    /// (the track's signal entering the chain) and the transport. Call once per block, before
    /// the chain runs.
    pub(crate) fn advance(&mut self, t: &Track, input: &[Vec<f32>], frames: usize, rate: f64, transport: &Transport) {
        let rate = if rate.is_finite() && rate > 0.0 { rate } else { 48_000.0 };
        let secs = frames as f64 / rate;
        for (m, st) in t.modulators.iter().zip(self.states.iter_mut()) {
            match m.kind {
                ModulatorKind::Lfo => {
                    let hz = f64::from(if m.rate_hz.is_finite() { m.rate_hz.clamp(0.01, 40.0) } else { 1.0 });
                    let shape = m.shape;
                    match m.sync_quarters().filter(|q| *q > 0.0) {
                        // locked: the phase is the song position, while playing
                        Some(q) if transport.playing && transport.quarters.is_finite() => {
                            st.phase = transport.quarters / q;
                            let per_sec = transport.tempo.max(1.0) / 60.0 / q;
                            let end = st.phase + per_sec * secs;
                            st.from = soundcraft_model::lfo_value(shape, st.phase, st.phase.floor() as i64 as u64);
                            st.to = soundcraft_model::lfo_value(shape, end, end.floor() as i64 as u64);
                        }
                        sync => {
                            let per_sec = match sync {
                                Some(q) => transport.tempo.max(1.0) / 60.0 / q,
                                None => hz,
                            };
                            let end = st.phase + per_sec * secs;
                            st.from = soundcraft_model::lfo_value(shape, st.phase, st.phase.floor() as i64 as u64);
                            st.to = soundcraft_model::lfo_value(shape, end, end.floor() as i64 as u64);
                            st.phase = end.rem_euclid(1_048_576.0);
                        }
                    }
                }
                ModulatorKind::Envelope => {
                    let coef = |ms: f32| {
                        let ms = if ms.is_finite() { ms.clamp(0.1, 2000.0) } else { 10.0 };
                        (-1.0 / (f64::from(ms) * 0.001 * rate)).exp() as f32
                    };
                    let (a, r) = (coef(m.attack_ms), coef(m.release_ms));
                    st.from = env_value(st.env);
                    for i in 0..frames {
                        let x = input.iter().filter_map(|c| c.get(i)).map(|v| v.abs()).filter(|v| v.is_finite()).fold(0.0f32, f32::max);
                        let k = if x > st.env { a } else { r };
                        st.env = x + k * (st.env - x);
                    }
                    if !st.env.is_finite() {
                        st.env = 0.0;
                    }
                    st.to = env_value(st.env);
                }
            }
            if m.bypass {
                st.from = 0.0;
                st.to = 0.0;
            }
        }
    }

    /// Moves every parameter of chain module `ins` (instance `plugin`) that a modulator targets,
    /// after the block-start values and lanes are applied: its value at `pos` (the lane's, else
    /// the stored one) moved by the modulators, at the block's start, and every [`TIMED_GRID`]
    /// samples after when the plug-in takes timed changes. Each parameter moved is marked unsent
    /// in `applied` (the insert's parameter order), so the plain value goes again next block and
    /// a parameter no longer modulated returns to it.
    pub(crate) fn apply(&self, t: &Track, ins: &Insert, plugin: &mut dyn Plugin, applied: &mut [f32], pos: Samples, frames: usize) {
        let module = ins.id;
        for m in &t.modulators {
            for target in m.targets.iter().filter(|x| x.module == module) {
                if moved_already(t, m.id, module, &target.param) {
                    continue;
                }
                let Some(info) = plugin.info().param(&target.param) else { continue };
                let base = ins.params.get(info.id).copied().unwrap_or(info.default);
                let base = plugin_lane(t, module, info.id).map_or(base, |l| l.value_at(pos, base));
                let at = |w: f32| -> f32 {
                    let mut n = info.to_normalized(info.clamp(base));
                    for (mm, st) in t.modulators.iter().zip(self.states.iter()) {
                        for tt in mm.targets.iter().filter(|x| x.module == module && x.param == info.id) {
                            let v = st.from + (st.to - st.from) * w;
                            let d = if tt.depth.is_finite() { tt.depth.clamp(-1.0, 1.0) } else { 0.0 };
                            n += d * v;
                        }
                    }
                    info.clamp(info.from_normalized(n.clamp(0.0, 1.0)))
                };
                plugin.set_param(info.id, at(0.0));
                let span = frames.max(1) as f32;
                let mut off = TIMED_GRID;
                while off < frames {
                    if !plugin.set_param_at(off, info.id, at(off as f32 / span)) {
                        break;
                    }
                    off += TIMED_GRID;
                }
                if let Some(a) = ins.params.keys().position(|k| k == info.id).and_then(|i| applied.get_mut(i)) {
                    *a = f32::NAN;
                }
            }
        }
    }
}

/// Whether an earlier modulator (or an earlier target of this one) already moved `param` of
/// `module` this block: [`ModRun::apply`] sums every modulator on a parameter in one go.
fn moved_already(t: &Track, id: u32, module: ModuleId, param: &str) -> bool {
    // the first modulator naming it does the work
    t.modulators.iter().find(|m| m.targets.iter().any(|x| x.module == module && x.param == param)).is_some_and(|m| m.id != id)
}
