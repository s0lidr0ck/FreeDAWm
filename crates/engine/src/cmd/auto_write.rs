//! Live automation writing (Write, Touch, Latch, Touch/Latch and Trim modes) for every automatable
//! parameter: volume, pan, mute, sends, trim and module parameters.
//!
//! A control is *touched* while it is held (the Mix fader under the pointer, a control surface's
//! touch-sensitive fader: [`Engine::automation_hold`], `automation.touch`) and for
//! [`TOUCH_HOLD_SECONDS`] after each change it makes. Every command that changes an automatable
//! value reports such a change ([`Engine::automation_changed`]), so a knob in a module drawer, an
//! edit in a plugin's own window, a MIDI controller or an agent's `mix.pan` all write, whatever
//! sent them. While the transport plays, [`Engine::automation_pass`] (called by the host every
//! frame) writes the touched values into their lanes as the track's mode says:
//!
//! - **Write**: volume, pan, mute and every lane the track already has, touched or not.
//! - **Touch**: what is touched; released, the lane plays its old automation again. A switch
//!   (mute, a send's mute) has no hold, so a press writes until the transport stops.
//! - **Latch**: what is touched, and it keeps writing its last value until the transport stops.
//! - **Touch/Latch**: volume as Touch, everything else as Latch.
//! - **Trim**: the trim lane, from [`Engine::automation_trim`] (the fader in Trim mode), as Touch.
//!   Other controls read, as in Pro Tools.
//!
//! Each written point replaces the automation since the previous one; stopping writes a last
//! point. A pass, and everything dragged during it, is one undo step. Nothing here runs on the
//! audio thread: it edits the document, which the player picks up as for any edit.

use super::mix::{current_value, grouped_ids, is_link_role_param};
use super::{CommandSpec, always, bad, has_selection, str_param, tracks_required};
use crate::{Engine, Result};
use serde_json::{Value, json};
use soundcraft_model::{AutoParam, AutomationMode as M, Session, TrackId, TrackKind};
use soundcraft_time::Samples;

/// How long a control that never says it was let go (a mouse wheel, a MIDI knob, a plugin's own
/// window) stays touched after its last change.
pub const TOUCH_HOLD_SECONDS: f64 = 0.5;

/// Points are written at most this often while playing.
const STEP_SECONDS: f64 = 0.02;

/// How long a Touch control let go takes to glide back to the automation it wrote over (Pro
/// Tools' AutoMatch time).
pub const RETURN_SECONDS: f64 = 0.25;

/// The merge key of a pass (see [`Engine::hold_merge`]).
const PASS: &str = "automation_pass";

/// The `s.edit` flag that keeps every point a pass wrote (Options › More › Thin Automation After
/// Pass off). Unset, the lanes a pass wrote are thinned when it ends, as Pro Tools does.
pub const KEEP_PASS_POINTS: &str = "automation.keep_pass_points";

/// The parameter types the Automation window enables for writing, as `automation.enable`
/// names them. A type switched off is not written (its lanes still play), as Pro Tools' write
/// enables do.
pub const WRITE_TYPES: [&str; 7] = ["volume", "pan", "mute", "send_level", "send_pan", "send_mute", "plugin"];

/// The write-enable type of `p` (the trim lane goes with volume).
fn write_type(p: &AutoParam) -> &'static str {
    match p {
        AutoParam::Volume | AutoParam::Trim => "volume",
        AutoParam::Pan(_) | AutoParam::Width | AutoParam::Balance => "pan",
        AutoParam::Mute => "mute",
        AutoParam::SendLevel(_) => "send_level",
        AutoParam::SendPan(_) => "send_pan",
        AutoParam::SendMute(_) => "send_mute",
        AutoParam::Plugin { .. } => "plugin",
    }
}

/// The `s.edit` flag that switches writing of type `ty` off.
pub fn write_disabled_flag(ty: &str) -> String {
    format!("automation.enable.{ty}.off")
}

/// How far thinning after a pass may move a lane: a twentieth of a dB, a thousandth of the pan.
fn pass_tolerance(p: &AutoParam) -> f32 {
    match p {
        AutoParam::Volume | AutoParam::Trim | AutoParam::SendLevel(_) => 0.05,
        AutoParam::Pan(_) | AutoParam::SendPan(_) => 0.001,
        // A plug-in's range is its own: only points exactly on the line go.
        _ => 0.0,
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        crate::cmd!(noundo
            "automation.touch",
            "Touch Automation Control",
            [],
            None,
            "{tracks?, param, touch?: bool} — true holds the control touched (a touch-sensitive fader), false lets it go; absent touches it once (it stays touched for half a second, as after any change)",
            has_selection,
            touch
        ),
        crate::cmd!(noundo
            "automation.trim",
            "Trim Automation",
            [],
            None,
            "{tracks?, db: -144..24, touch?: bool} — the trim the fader sets in Trim mode, written to the trim lane while playing; touch false lets the fader go",
            has_selection,
            trim
        ),
        crate::cmd!(
            "automation.enable",
            "Enable Automation Writing",
            [],
            None,
            "{type: volume|pan|mute|send_level|send_pan|send_mute|plugin, value?: bool} — whether passes write that type (on unless turned off); toggles when omitted",
            always,
            |e, p| {
                let ty = str_param(p, "type").map(str::to_ascii_lowercase).unwrap_or_default();
                if !WRITE_TYPES.contains(&ty.as_str()) {
                    return Err(bad("automation.enable", format!("`type` is one of {}", WRITE_TYPES.join(", "))));
                }
                let flag = write_disabled_flag(&ty);
                let on = p.get("value").and_then(Value::as_bool).unwrap_or_else(|| e.session().edit.flag(&flag));
                e.session_mut().edit.set_flag(&flag, !on);
                Ok(json!({"type": ty, "value": on}))
            }
        ),
        crate::cmd!(
            "automation.thin_after_pass",
            "Thin Automation After Pass",
            ["Options", "More"],
            None,
            "{value?: bool} — thin the lanes a writing pass wrote when it ends (on unless turned off); toggles when omitted",
            always,
            |e, p| {
                let keep = !p.get("value").and_then(Value::as_bool).unwrap_or_else(|| e.session().edit.flag(KEEP_PASS_POINTS));
                e.session_mut().edit.set_flag(KEEP_PASS_POINTS, keep);
                Ok(json!({"value": !keep}))
            }
        ),
        crate::cmd!(query "automation.touched", "Touched Automation Controls", [], None, "{} → [{track, param}]", always, |e, _| {
            let pos = e.heard_position();
            let rate = e.session().sample_rate;
            let w = &e.auto_write;
            let mut v: Vec<(TrackId, AutoParam)> = Vec::new();
            for (t, p) in w.held.iter().chain(&w.ui_held).cloned().chain(w.pulses.iter().filter(|x| w.pulse_live(x, pos, rate.samples(TOUCH_HOLD_SECONDS))).map(|x| (x.0, x.1.clone()))) {
                if !v.contains(&(t, p.clone())) {
                    v.push((t, p));
                }
            }
            Ok(Value::Array(v.into_iter().map(|(t, p)| json!({"track": t.0, "param": p.id()})).collect()))
        }),
    ]
}

/// The writer's state: kept by the engine, out of the document.
#[derive(Debug, Clone, Default)]
pub struct AutoWriter {
    /// Controls changed lately: track, param and the transport position of the last change.
    pulses: Vec<(TrackId, AutoParam, Samples)>,
    /// Held by a command (`automation.touch` with `touch: true`) until let go.
    held: Vec<(TrackId, AutoParam)>,
    /// Held by the host's pointer this frame ([`Engine::automation_ui_held`]).
    ui_held: Vec<(TrackId, AutoParam)>,
    /// The lanes this pass has written (Latch goes on writing them).
    written: Vec<(TrackId, AutoParam)>,
    /// The trim each track's fader sets in Trim mode, while it is set.
    trim: Vec<(TrackId, f32)>,
    /// Where the last points were written (`i64::MIN`: nothing yet, or the transport jumped).
    last_at: Samples,
    /// Whether the last pass ran with the transport playing.
    playing: bool,
    /// How far behind the transport the sound is heard (the mix's delay compensation and the
    /// output's latency), in samples: moves are written where they were heard.
    heard_delay: Samples,
    /// The lanes the last step wrote (a Touch control missing from the next step was let go).
    last_writes: Vec<(TrackId, AutoParam)>,
    /// Each lane this pass wrote, as it was before the pass touched it (what a let-go control
    /// glides back to).
    before: Vec<(TrackId, soundcraft_model::AutomationLane)>,
}

impl AutoWriter {
    pub fn new() -> Self {
        AutoWriter { last_at: i64::MIN, ..AutoWriter::default() }
    }

    fn pulse_live(&self, p: &(TrackId, AutoParam, Samples), pos: Samples, hold: Samples) -> bool {
        // A pulse "ahead" of the transport came before a loop wrap or a jump back: it is old.
        pos >= p.2 && pos.saturating_sub(p.2) <= hold
    }

    fn touched(&self, t: TrackId, p: &AutoParam, pos: Samples, hold: Samples) -> bool {
        let is = |x: &(TrackId, AutoParam)| x.0 == t && &x.1 == p;
        self.held.iter().any(is) || self.ui_held.iter().any(is) || self.pulses.iter().any(|x| x.0 == t && &x.1 == p && self.pulse_live(x, pos, hold))
    }

    fn was_written(&self, t: TrackId, p: &AutoParam) -> bool {
        self.written.iter().any(|x| x.0 == t && &x.1 == p)
    }

    /// Every param this track's controls touched lately (held or pulsed).
    fn touched_params(&self, t: TrackId, pos: Samples, hold: Samples) -> Vec<AutoParam> {
        let mut v: Vec<AutoParam> = Vec::new();
        let live = self.pulses.iter().filter(|x| x.0 == t && self.pulse_live(x, pos, hold)).map(|x| &x.1);
        for p in self.held.iter().chain(&self.ui_held).filter(|x| x.0 == t).map(|x| &x.1).chain(live) {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
        v
    }
}

/// Whether another pass is due: the transport moved at least `step` since `last`, either way
/// (`last == i64::MIN`: nothing written yet). Saturating, so a restart or a loop wrap never
/// overflows.
pub fn step_due(pos: Samples, last: Samples, step: Samples) -> bool {
    last == i64::MIN || pos.saturating_sub(last) >= step || last.saturating_sub(pos) >= step
}

/// A mute-like control: a press, with nothing to hold.
fn is_switch(p: &AutoParam) -> bool {
    matches!(p, AutoParam::Mute | AutoParam::SendMute(_))
}

/// `held` plus, for a fader or a mute, the same control of the tracks that share it through an
/// active mix group: a group member moves with the held one, so it is held as long (without this
/// a member stopped writing half a second after the dragged fader stood still).
fn with_group(s: &Session, held: Vec<(TrackId, AutoParam)>) -> Vec<(TrackId, AutoParam)> {
    let mut out = held.clone();
    for (t, p) in held {
        let attr = match p {
            AutoParam::Volume => "volume",
            AutoParam::Mute => "mute",
            _ => continue,
        };
        for m in grouped_ids(s, t, attr) {
            if !out.iter().any(|x| x.0 == m && x.1 == p) {
                out.push((m, p.clone()));
            }
        }
    }
    out
}

/// Whether `p` still names something on the track (a module lane whose module was removed, a send
/// lane whose send is gone, a panner the track does not have) and may be written.
fn writable(s: &Session, t: TrackId, p: &AutoParam) -> bool {
    let Some(tr) = s.track(t) else { return false };
    match p {
        AutoParam::Volume | AutoParam::Trim => true,
        AutoParam::Mute => tr.kind != TrackKind::Master,
        AutoParam::Pan(i) => usize::from(*i) < tr.mixer.pan.len(),
        // Width and balance shape a stereo track's main output.
        AutoParam::Width | AutoParam::Balance => tr.mixer.pan.len() == 2,
        AutoParam::SendLevel(i) | AutoParam::SendPan(i) | AutoParam::SendMute(i) => tr.mixer.sends.get(usize::from(*i)).is_some_and(Option::is_some),
        AutoParam::Plugin { module, param } => tr.module_or_instrument(*module).is_some_and(|ins| !is_link_role_param(&ins.plugin, param)),
    }
}

impl Engine {
    /// An automatable control of `track` changed: it counts as touched for
    /// [`TOUCH_HOLD_SECONDS`]. Every command that sets such a value calls this.
    pub fn automation_changed(&mut self, track: TrackId, param: AutoParam) {
        let pos = self.heard_position();
        let w = &mut self.auto_write;
        match w.pulses.iter_mut().find(|x| x.0 == track && x.1 == param) {
            Some(x) => x.2 = pos,
            None => w.pulses.push((track, param, pos)),
        }
    }

    /// Hold (or let go of) a control, as a touch-sensitive fader does. Holding a fader or a mute
    /// in a mix group holds the group's (they move with it).
    pub fn automation_hold(&mut self, track: TrackId, param: AutoParam, on: bool) {
        for (track, param) in with_group(self.session(), vec![(track, param)]) {
            let w = &mut self.auto_write;
            w.held.retain(|x| !(x.0 == track && x.1 == param));
            if on {
                w.held.push((track, param));
            } else {
                // A let-go control stops writing now, not half a second later.
                w.pulses.retain(|x| !(x.0 == track && x.1 == param));
            }
        }
    }

    /// The controls the host's pointer holds this frame (a fader or a knob being dragged, even
    /// standing still), with their mix groups' (see [`Engine::automation_hold`]). Replaces the
    /// last frame's list; one let go stops writing at once.
    pub fn automation_ui_held(&mut self, held: Vec<(TrackId, AutoParam)>) {
        let held = with_group(self.session(), held);
        let w = &mut self.auto_write;
        for x in &w.ui_held {
            if !held.contains(x) {
                w.pulses.retain(|p| !(p.0 == x.0 && p.1 == x.1));
            }
        }
        w.ui_held = held;
    }

    /// The trim the fader of a track in Trim mode sets (`None` lets the fader go: the trim lane
    /// plays again).
    pub fn automation_trim(&mut self, track: TrackId, db: Option<f32>) {
        self.auto_write.trim.retain(|x| x.0 != track);
        match db.filter(|d| d.is_finite()) {
            Some(d) => {
                self.auto_write.trim.push((track, d.clamp(-144.0, 24.0)));
                self.automation_changed(track, AutoParam::Trim);
            }
            None => self.automation_hold(track, AutoParam::Trim, false),
        }
    }

    /// What a Trim-mode fader shows: the trim it holds, else the trim lane at the transport.
    pub fn trim_shown(&self, track: TrackId) -> f32 {
        if let Some((_, d)) = self.auto_write.trim.iter().find(|x| x.0 == track) {
            return *d;
        }
        self.session()
            .track(track)
            .and_then(|t| t.lane(&AutoParam::Trim))
            .filter(|l| !l.points.is_empty())
            .map_or(0.0, |l| l.value_at(self.transport.position, 0.0))
    }

    /// How far behind the transport the sound is heard now, in samples (the host's mix delay
    /// plus output latency): a pass writes a move where it was heard, not where the transport
    /// had rendered to, and a control shows the automation being heard.
    pub fn automation_heard_delay(&mut self, samples: Samples) {
        self.auto_write.heard_delay = samples.clamp(0, i64::from(u32::MAX));
    }

    /// The timeline position being heard now (see [`Engine::automation_heard_delay`]).
    pub fn heard_position(&self) -> Samples {
        self.transport.position.saturating_sub(self.auto_write.heard_delay).max(0)
    }

    /// The transport jumped (a locate while playing, a new start): the next points do not replace
    /// the automation between the old position and the new one.
    pub fn automation_jump(&mut self) {
        self.auto_write.last_at = i64::MIN;
    }

    /// Forgets every touch, hold and trim, and the pass (the session was replaced).
    pub fn automation_reset(&mut self) {
        self.auto_write = AutoWriter::new();
    }

    /// Whether the running pass has written anything: its undo step is still open.
    pub fn automation_pass_writing(&self) -> bool {
        self.auto_write.playing && !self.auto_write.written.is_empty()
    }

    /// The value a control shows now: what its lane plays at the transport while the track reads
    /// automation and the control is not touched (so a fader moves with its automation, as in
    /// the four reference DAWs); `None` when the control shows its own value (no lane, Off,
    /// Write, or held).
    pub fn automation_shown(&self, track: TrackId, param: &AutoParam) -> Option<f32> {
        let tr = self.session().track(track)?;
        let mode = tr.mixer.automation_mode;
        if !mode.reads() || mode == M::Write || self.automation_touched(track, param) {
            return None;
        }
        let lane = tr.lane(param).filter(|l| !l.points.is_empty())?;
        Some(lane.value_at(self.heard_position(), current_value(tr, param)))
    }

    /// Whether a control of the track is touched now (for a host that lights it).
    pub fn automation_touched(&self, track: TrackId, param: &AutoParam) -> bool {
        let hold = self.session().sample_rate.samples(TOUCH_HOLD_SECONDS);
        self.auto_write.touched(track, param, self.heard_position(), hold)
    }

    /// One writing pass at the transport's position: call it every frame. Writes nothing unless
    /// `playing`, but the frame playback stops on writes each written lane's last point. Returns
    /// how many points it wrote.
    pub fn automation_pass(&mut self, playing: bool) -> usize {
        let writes_any = |m: M| matches!(m, M::Write | M::Touch | M::Latch | M::TouchLatch | M::Trim);
        let may_write = playing && self.session().tracks.iter().any(|t| writes_any(t.mixer.automation_mode));
        self.hold_merge(may_write.then_some(PASS));
        let pos = self.heard_position();
        let rate = self.session().sample_rate;
        let hold = rate.samples(TOUCH_HOLD_SECONDS);
        let started = playing && !self.auto_write.playing;
        let stopped = !playing && self.auto_write.playing;
        self.auto_write.playing = playing;
        if started {
            // Touches made while stopped belong to no pass.
            self.auto_write.pulses.clear();
            self.auto_write.written.clear();
            self.auto_write.last_writes.clear();
            self.auto_write.before.clear();
            self.auto_write.last_at = i64::MIN;
        }
        if !playing && !stopped {
            self.auto_write.last_at = i64::MIN;
            return 0;
        }
        if playing && !step_due(pos, self.auto_write.last_at, rate.samples(STEP_SECONDS)) {
            return 0;
        }
        let from = std::mem::replace(&mut self.auto_write.last_at, if playing { pos } else { i64::MIN });
        let writes = self.due_writes(pos, hold);
        // Keep each lane as it was before the pass first writes it.
        for (t, param, _) in &writes {
            if !self.auto_write.before.iter().any(|(bt, l)| bt == t && l.param == *param) {
                let lane = self.session().track(*t).and_then(|tr| tr.lane(param)).cloned();
                self.auto_write.before.push((*t, lane.unwrap_or_else(|| soundcraft_model::AutomationLane::new(param.clone()))));
            }
        }
        // A Touch control let go since the last step glides back to what it wrote over.
        if playing && from != i64::MIN {
            let let_go: Vec<(TrackId, AutoParam)> =
                self.auto_write.last_writes.iter().filter(|(t, p)| !writes.iter().any(|(wt, wp, _)| wt == t && wp == p)).cloned().collect();
            for (t, p) in let_go {
                self.glide_back(t, &p, from, rate.samples(RETURN_SECONDS));
            }
        }
        self.auto_write.last_writes = writes.iter().map(|(t, p, _)| (*t, p.clone())).collect();
        let mut n = 0;
        for (t, param, v) in writes {
            let mut point = json!({"track": t.0, "param": param.id(), "at": pos, "value": v});
            if from != i64::MIN {
                point["from"] = from.into();
            }
            if self.execute_merged("automation.set_point", &point, PASS).is_ok() {
                n += 1;
                if !self.auto_write.was_written(t, &param) {
                    self.auto_write.written.push((t, param));
                }
            }
        }
        // Forget pulses that ran out, so the list stays short.
        self.auto_write.pulses.retain(|x| pos >= x.2 && pos.saturating_sub(x.2) <= hold);
        if stopped && !self.session().edit.flag(KEEP_PASS_POINTS) {
            // A pass leaves a point every step: what lies on a line goes (one undo step with it).
            let written = self.auto_write.written.clone();
            for (t, param) in written {
                let p = json!({"tracks": [t.0], "param": param.id(), "tolerance": pass_tolerance(&param)});
                let _ = self.execute_merged("automation.thin", &p, PASS);
            }
        }
        if stopped {
            self.auto_write.written.clear();
            self.auto_write.last_writes.clear();
            self.auto_write.before.clear();
            self.auto_write.pulses.clear();
            self.auto_write.trim.clear();
            self.end_merge();
        }
        n
    }

    /// Touch let go at `at` (the last point written): the lane ramps from there back to the
    /// automation it had before the pass over `ret` samples, instead of jumping to wherever its
    /// next old point is. Part of the pass's undo step. Switches (mute) jump.
    fn glide_back(&mut self, t: TrackId, p: &AutoParam, at: Samples, ret: Samples) {
        let Some(tr) = self.session().track(t) else { return };
        if is_switch(p) || !matches!(tr.mixer.automation_mode, M::Touch | M::TouchLatch) {
            return;
        }
        let Some(old) = self.auto_write.before.iter().find(|(bt, l)| *bt == t && l.param == *p).map(|(_, l)| l.clone()) else { return };
        if old.points.is_empty() {
            return;
        }
        let end = at.saturating_add(ret.max(1));
        let target = old.value_at(end, current_value(tr, p));
        let inside: Vec<Samples> = tr.lane(p).map(|l| l.points_in(at.saturating_add(1), end)).unwrap_or_default();
        if !inside.is_empty() {
            let _ = self.execute_merged("automation.delete", &json!({"track": t.0, "param": p.id(), "points": inside}), PASS);
        }
        let _ = self.execute_merged("automation.set_point", &json!({"track": t.0, "param": p.id(), "at": end, "value": target}), PASS);
    }

    /// The (track, lane, value) points due now, as each track's mode says.
    fn due_writes(&self, pos: Samples, hold: Samples) -> Vec<(TrackId, AutoParam, f32)> {
        let s = self.session();
        let w = &self.auto_write;
        let mut out = Vec::new();
        for tr in &s.tracks {
            let mode = tr.mixer.automation_mode;
            let touched = w.touched_params(tr.id, pos, hold);
            let mut params: Vec<AutoParam> = match mode {
                M::Write => {
                    let mut v = vec![AutoParam::Volume, AutoParam::Mute];
                    v.extend((0..tr.mixer.pan.len().min(2)).map(|i| AutoParam::Pan(i as u8)));
                    v.extend(tr.automation.iter().map(|l| l.param.clone()).filter(|p| *p != AutoParam::Trim));
                    v.extend(touched.into_iter().filter(|p| *p != AutoParam::Trim));
                    v
                }
                M::Touch | M::Latch | M::TouchLatch => {
                    let latches = |p: &AutoParam| match mode {
                        M::Latch => true,
                        M::TouchLatch => *p != AutoParam::Volume,
                        _ => is_switch(p),
                    };
                    let mut v: Vec<AutoParam> = touched.into_iter().filter(|p| *p != AutoParam::Trim).collect();
                    v.extend(w.written.iter().filter(|x| x.0 == tr.id && latches(&x.1)).map(|x| x.1.clone()));
                    v
                }
                M::Trim => touched.into_iter().filter(|p| *p == AutoParam::Trim).collect(),
                M::Off | M::Read => Vec::new(),
            };
            params.sort();
            params.dedup();
            for p in params {
                if !writable(s, tr.id, &p) || s.edit.flag(&write_disabled_flag(write_type(&p))) {
                    continue;
                }
                let v = if p == AutoParam::Trim {
                    match w.trim.iter().find(|x| x.0 == tr.id) {
                        Some((_, d)) => *d,
                        None => continue,
                    }
                } else {
                    current_value(tr, &p)
                };
                out.push((tr.id, p, v));
            }
        }
        out
    }
}

fn touch(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "automation.touch";
    let tracks = tracks_required(e, C, p)?;
    let name = str_param(p, "param").ok_or_else(|| bad(C, "`param` required"))?.to_string();
    let lanes = super::mix::lane_params(e, &tracks, &json!({"param": name}), C)?;
    let on = p.get("touch").and_then(Value::as_bool);
    for (t, param) in lanes {
        match on {
            Some(on) => e.automation_hold(t, param, on),
            None => e.automation_changed(t, param),
        }
    }
    Ok(json!({}))
}

fn trim(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "automation.trim";
    let tracks = tracks_required(e, C, p)?;
    let release = p.get("touch").and_then(Value::as_bool) == Some(false);
    let db = p.get("db").and_then(Value::as_f64).filter(|v| v.is_finite()).map(|v| v as f32);
    if db.is_none() && !release {
        return Err(bad(C, "`db` (number) required"));
    }
    for t in tracks {
        e.automation_trim(t, if release { None } else { db });
    }
    Ok(json!({"db": db}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{AutoParam, ModuleId, TrackId};
    use soundcraft_time::Samples;

    /// The demo session with Kick in `mode`, a CleanQ on Kick and a send A, playing from 0. Every
    /// written point is kept (no thinning after a pass), so the tests can count them.
    fn kick(mode: &str) -> (Engine, TrackId, ModuleId, Samples) {
        let mut e = crate::demo::demo_engine();
        e.execute("automation.thin_after_pass", &json!({"value": false})).unwrap();
        e.execute("mix.module_add", &json!({"track": "Kick", "plugin": "fs_cleanq"})).unwrap();
        e.execute("mix.send", &json!({"track": "Kick", "slot": 0, "bus": "Verb", "level_db": -10.0})).unwrap();
        e.execute("mix.automation_mode", &json!({"tracks": ["Kick"], "mode": mode})).unwrap();
        let t = e.session().track_by_name("Kick").unwrap();
        let (t, m) = (t.id, t.mixer.inserts.iter().find(|i| i.plugin == "fs_cleanq").unwrap().id);
        let q = i64::from(e.session().sample_rate.hz()) / 10;
        play(&mut e, 0);
        (e, t, m, q)
    }

    /// A frame at `at` with the transport playing.
    fn play(e: &mut Engine, at: Samples) -> usize {
        e.transport.playing = true;
        e.transport.position = at;
        e.automation_pass(true)
    }

    fn stop(e: &mut Engine, at: Samples) -> usize {
        e.transport.playing = false;
        e.transport.position = at;
        e.automation_pass(false)
    }

    fn points(e: &Engine, t: TrackId, p: &AutoParam) -> Vec<(Samples, f32)> {
        e.session().track(t).unwrap().lane(p).map(|l| l.points.iter().map(|x| (x.at, x.value)).collect()).unwrap_or_default()
    }

    fn outgain(m: ModuleId) -> AutoParam {
        AutoParam::Plugin { module: m, param: "outGain".into() }
    }

    #[test]
    fn a_pass_is_due_after_a_step_either_way_without_overflow() {
        use super::step_due;
        // Nothing written yet (`i64::MIN`): always due, and no overflow.
        assert!(step_due(0, i64::MIN, 960));
        assert!(step_due(48_000, i64::MIN, 960));
        assert!(!step_due(1_000, 900, 960));
        assert!(step_due(2_000, 900, 960));
        // A loop wrap.
        assert!(step_due(0, 1_000_000, 960));
    }

    #[test]
    fn write_mode_writes_volume_pan_mute_and_the_tracks_lanes() {
        let (mut e, t, m, q) = kick("write");
        // A lane the track already has is written over too.
        e.execute("automation.set_point", &json!({"track": t.0, "param": outgain(m).id(), "at": 0, "value": 0.8})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "param": outgain(m).id(), "at": 10 * q, "value": 0.8})).unwrap();
        e.execute("mix.module_param", &json!({"track": t.0, "module": m.0, "param": "outGain", "value": 0.25})).unwrap();
        play(&mut e, q);
        e.execute("mix.volume", &json!({"track": t.0, "db": -6.0})).unwrap();
        play(&mut e, 2 * q);
        stop(&mut e, 3 * q);
        assert_eq!(points(&e, t, &AutoParam::Volume).last(), Some(&(3 * q, -6.0)));
        assert!(points(&e, t, &AutoParam::Mute).iter().all(|p| p.1 == 0.0));
        assert!(!points(&e, t, &AutoParam::Mute).is_empty());
        assert!(!points(&e, t, &AutoParam::Pan(0)).is_empty());
        let og = points(&e, t, &outgain(m));
        assert!(og.contains(&(2 * q, 0.25)), "{og:?}");
        // Past the stop the old ride is kept.
        assert_eq!(og.last(), Some(&(10 * q, 0.8)));
        // A send without a lane is not written in Write mode unless touched.
        assert!(points(&e, t, &AutoParam::SendLevel(0)).is_empty());
    }

    #[test]
    fn touch_writes_a_pan_only_while_touched_then_plays_the_old_lane() {
        let (mut e, t, _, q) = kick("touch");
        e.execute("automation.set_point", &json!({"track": t.0, "param": "pan", "at": 0, "value": -1.0})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "param": "pan", "at": 50 * q, "value": -1.0})).unwrap();
        assert_eq!(play(&mut e, q), 0, "nothing touched, nothing written");
        e.execute("mix.pan", &json!({"track": t.0, "pan": 0.5})).unwrap();
        assert_eq!(play(&mut e, 2 * q), 1);
        assert_eq!(play(&mut e, 3 * q), 1, "still touched within the hold");
        // No change for longer than the hold: released.
        assert_eq!(play(&mut e, 20 * q), 0);
        assert_eq!(play(&mut e, 21 * q), 0);
        stop(&mut e, 22 * q);
        let pan = points(&e, t, &AutoParam::Pan(0));
        // Let go, it glides back to the old automation over the return time.
        let back = 3 * q + e.session().sample_rate.samples(super::RETURN_SECONDS);
        assert_eq!(pan, [(0, -1.0), (2 * q, 0.5), (3 * q, 0.5), (back, -1.0), (50 * q, -1.0)]);
    }

    #[test]
    fn a_held_control_writes_standing_still_and_stops_when_let_go() {
        let (mut e, t, _, q) = kick("touch");
        e.execute("automation.touch", &json!({"track": t.0, "param": "volume", "touch": true})).unwrap();
        for i in 1..=10 {
            assert_eq!(play(&mut e, i * q), 1);
        }
        e.execute("automation.touch", &json!({"track": t.0, "param": "volume", "touch": false})).unwrap();
        assert_eq!(play(&mut e, 11 * q), 0);
        assert_eq!(points(&e, t, &AutoParam::Volume).len(), 10);
        // The host's pointer holds work the same way, frame by frame.
        e.automation_ui_held(vec![(t, AutoParam::Volume)]);
        assert_eq!(play(&mut e, 30 * q), 1);
        e.automation_ui_held(Vec::new());
        assert_eq!(play(&mut e, 31 * q), 0);
    }

    /// Tier 2: a pass's lanes are thinned when it ends (one undo step with the pass), unless
    /// Thin Automation After Pass is off.
    #[test]
    fn a_pass_is_thinned_when_it_ends() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").unwrap().id;
        e.execute("mix.automation_mode", &json!({"tracks": [t.0], "mode": "touch"})).unwrap();
        let q = i64::from(e.session().sample_rate.hz()) / 10;
        play(&mut e, 0);
        e.automation_ui_held(vec![(t, AutoParam::Volume)]);
        e.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
        for i in 1..=20 {
            play(&mut e, i * q);
        }
        e.automation_ui_held(Vec::new());
        stop(&mut e, 21 * q);
        let pts = points(&e, t, &AutoParam::Volume);
        assert!(pts.len() <= 3, "a held still fader leaves its ends: {pts:?}");
        assert!(pts.iter().all(|p| p.1 == -6.0));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(points(&e, t, &AutoParam::Volume).is_empty(), "the pass and its thinning are one step");
        // Off: every point stays.
        e.execute("automation.thin_after_pass", &json!({"value": false})).unwrap();
        play(&mut e, 0);
        e.automation_ui_held(vec![(t, AutoParam::Volume)]);
        for i in 1..=20 {
            play(&mut e, i * q);
        }
        e.automation_ui_held(Vec::new());
        stop(&mut e, 21 * q);
        assert!(points(&e, t, &AutoParam::Volume).len() >= 20);
    }

    /// Tier 2: a parameter type switched off in the Automation window is not written, though its
    /// lanes still play.
    #[test]
    fn a_disabled_type_is_not_written() {
        let (mut e, t, _, q) = kick("write");
        e.execute("automation.enable", &json!({"type": "pan", "value": false})).unwrap();
        let pan_before = points(&e, t, &AutoParam::Pan(0));
        e.execute("mix.volume", &json!({"track": t.0, "db": -6.0})).unwrap();
        play(&mut e, q);
        play(&mut e, 2 * q);
        stop(&mut e, 3 * q);
        assert!(!points(&e, t, &AutoParam::Volume).is_empty(), "volume is written");
        assert_eq!(points(&e, t, &AutoParam::Pan(0)), pan_before, "pan is not");
        assert!(e.execute("automation.enable", &json!({"type": "colour"})).is_err());
        assert_eq!(e.execute("automation.enable", &json!({"type": "pan"})).unwrap()["value"], true, "toggles back on");
    }

    /// Tier 2: a move is written where it was heard, the host's mix and output delay behind the
    /// transport.
    #[test]
    fn moves_are_written_where_they_were_heard() {
        let (mut e, t, _, q) = kick("touch");
        e.automation_heard_delay(1000);
        e.execute("mix.volume", &json!({"track": t.0, "db": -6.0})).unwrap();
        play(&mut e, 2 * q);
        stop(&mut e, 3 * q);
        let pts = points(&e, t, &AutoParam::Volume);
        assert!(pts.contains(&(2 * q - 1000, -6.0)) && !pts.iter().any(|p| p.0 == 2 * q), "{pts:?}");
        assert!(pts.iter().any(|p| p.0 == 3 * q - 1000), "{pts:?}");
    }

    /// Tier 2: a control shows what its automation plays, except while it is touched or written.
    #[test]
    fn a_control_shows_what_its_automation_plays() {
        let (mut e, t, _, q) = kick("read");
        assert_eq!(e.automation_shown(t, &AutoParam::Volume), None, "no lane: the fader's own value");
        e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 0, "value": 0.0})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 10 * q, "value": -10.0})).unwrap();
        e.transport.position = 5 * q;
        assert_eq!(e.automation_shown(t, &AutoParam::Volume), Some(-5.0));
        e.automation_ui_held(vec![(t, AutoParam::Volume)]);
        assert_eq!(e.automation_shown(t, &AutoParam::Volume), None, "held: the fader's own value");
        e.automation_ui_held(Vec::new());
        e.execute("mix.automation_mode", &json!({"tracks": [t.0], "mode": "off"})).unwrap();
        assert_eq!(e.automation_shown(t, &AutoParam::Volume), None);
    }

    /// Mix bug 6: the members of a group stopped writing half a second after the dragged fader
    /// stood still, while it was still held.
    #[test]
    fn a_held_grouped_fader_holds_its_group() {
        let mut e = crate::demo::demo_engine();
        e.execute("track.group", &json!({"tracks": [3, 6]})).unwrap();
        e.execute("mix.automation_mode", &json!({"tracks": [3, 6], "mode": "touch"})).unwrap();
        let (a, b) = (TrackId(3), TrackId(6));
        let q = i64::from(e.session().sample_rate.hz()) / 10;
        play(&mut e, 0);
        e.automation_ui_held(vec![(a, AutoParam::Volume)]);
        e.execute("mix.volume", &json!({"tracks": [a.0], "db": -3.0})).unwrap();
        for i in 1..=20 {
            assert_eq!(play(&mut e, i * q), 2, "both faders write at {i}");
        }
        e.automation_ui_held(Vec::new());
        assert_eq!(play(&mut e, 21 * q), 0, "let go, the group stops at once");
        stop(&mut e, 22 * q);
        assert_eq!(points(&e, b, &AutoParam::Volume).len(), points(&e, a, &AutoParam::Volume).len());
    }

    #[test]
    fn latch_keeps_writing_a_send_level_until_stop() {
        let (mut e, t, _, q) = kick("latch");
        e.execute("mix.send_level", &json!({"track": t.0, "slot": 0, "db": -3.0})).unwrap();
        play(&mut e, q);
        for i in 2..30 {
            assert_eq!(play(&mut e, i * q), 1, "latched at {i}");
        }
        assert_eq!(stop(&mut e, 30 * q), 1, "stop writes the last point");
        let lvl = points(&e, t, &AutoParam::SendLevel(0));
        assert_eq!(lvl.first(), Some(&(q, -3.0)));
        assert_eq!(lvl.last(), Some(&(30 * q, -3.0)));
        assert!(points(&e, t, &AutoParam::Volume).is_empty(), "volume was not touched");
        // The next pass starts with nothing latched.
        assert_eq!(play(&mut e, 40 * q), 0);
    }

    #[test]
    fn touch_mode_latches_a_mute_press_until_stop() {
        let (mut e, t, _, q) = kick("touch");
        play(&mut e, q);
        e.execute("mix.mute", &json!({"track": t.0})).unwrap();
        play(&mut e, 2 * q);
        play(&mut e, 40 * q);
        stop(&mut e, 41 * q);
        assert_eq!(points(&e, t, &AutoParam::Mute), [(2 * q, 1.0), (40 * q, 1.0), (41 * q, 1.0)]);
    }

    #[test]
    fn touch_latch_drops_volume_but_latches_a_module_parameter() {
        let (mut e, t, m, q) = kick("touch/latch");
        e.execute("mix.volume", &json!({"track": t.0, "db": -4.0})).unwrap();
        e.execute("mix.module_param", &json!({"track": t.0, "module": m.0, "param": "outGain", "value": 0.5})).unwrap();
        play(&mut e, q);
        play(&mut e, 30 * q);
        stop(&mut e, 31 * q);
        assert_eq!(points(&e, t, &AutoParam::Volume), [(q, -4.0)]);
        assert_eq!(points(&e, t, &outgain(m)), [(q, 0.5), (30 * q, 0.5), (31 * q, 0.5)]);
    }

    #[test]
    fn trim_mode_writes_the_trim_lane_and_reads_the_rest() {
        let (mut e, t, _, q) = kick("trim");
        e.execute("mix.volume", &json!({"track": t.0, "db": -4.0})).unwrap();
        e.execute("mix.pan", &json!({"track": t.0, "pan": 0.3})).unwrap();
        e.execute("automation.trim", &json!({"track": t.0, "db": -2.5})).unwrap();
        assert_eq!(e.trim_shown(t), -2.5);
        play(&mut e, q);
        play(&mut e, 2 * q);
        e.execute("automation.trim", &json!({"track": t.0, "touch": false})).unwrap();
        play(&mut e, 3 * q);
        stop(&mut e, 4 * q);
        assert_eq!(points(&e, t, &AutoParam::Trim), [(q, -2.5), (2 * q, -2.5)]);
        assert!(points(&e, t, &AutoParam::Volume).is_empty());
        assert!(points(&e, t, &AutoParam::Pan(0)).is_empty());
        assert_eq!(e.trim_shown(t), -2.5, "the lane plays the trim back");
    }

    #[test]
    fn read_and_off_write_nothing_and_stopped_touches_belong_to_no_pass() {
        for mode in ["read", "off"] {
            let (mut e, t, _, q) = kick(mode);
            e.execute("mix.volume", &json!({"track": t.0, "db": -4.0})).unwrap();
            assert_eq!(play(&mut e, q), 0);
            assert!(e.session().track(t).unwrap().automation.iter().all(|l| l.points.is_empty()));
        }
        let (mut e, t, _, q) = kick("touch");
        stop(&mut e, 0);
        e.execute("mix.volume", &json!({"track": t.0, "db": -4.0})).unwrap();
        assert_eq!(stop(&mut e, 0), 0);
        assert_eq!(play(&mut e, q), 0, "a touch made while stopped is not written by the next pass");
    }

    #[test]
    fn a_jump_does_not_replace_the_automation_it_skips() {
        let (mut e, t, _, q) = kick("write");
        e.execute("automation.set_point", &json!({"track": t.0, "at": 5 * q, "value": -30.0})).unwrap();
        play(&mut e, q);
        e.automation_jump();
        play(&mut e, 10 * q);
        stop(&mut e, 11 * q);
        assert!(points(&e, t, &AutoParam::Volume).contains(&(5 * q, -30.0)));
    }

    #[test]
    fn a_pass_and_what_drives_it_are_one_undo_step() {
        let (mut e, t, m, q) = kick("latch");
        let steps = e.undo_history().len();
        for i in 1..40 {
            play(&mut e, i * q);
            e.execute_merged("mix.pan", &json!({"track": t.0, "pan": i as f64 / 50.0}), "pan").unwrap();
            e.execute_merged("mix.module_param", &json!({"track": t.0, "module": m.0, "param": "outGain", "value": 1.0}), "knob").unwrap();
        }
        stop(&mut e, 40 * q);
        assert_eq!(e.undo_history().len(), steps + 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track(t).unwrap().automation.iter().all(|l| l.points.is_empty()));
    }

    #[test]
    fn touched_reports_held_and_recently_changed_controls() {
        let (mut e, t, _, _) = kick("touch");
        e.execute("mix.send_level", &json!({"track": t.0, "slot": 0, "pan": 0.25})).unwrap();
        let v = e.execute("automation.touched", &json!({})).unwrap();
        assert_eq!(v, json!([{"track": t.0, "param": "send_a_pan"}]));
        assert!(e.automation_touched(t, &AutoParam::SendPan(0)));
        assert!(e.execute("automation.touch", &json!({"track": t.0, "param": "nonsense"})).is_err());
    }
}
