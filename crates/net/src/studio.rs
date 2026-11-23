//! Mixing a studio from another PC (File › Connect to a Studio…): this side only dials.
//!
//! [`Studio`] keeps one link to the studio's hub as a listening PC (`hello {role: "listener"}`),
//! retrying every couple of seconds while it is away, as a remote recorder's link does. Over it
//! it subscribes to the mixer feed (`mixer.subscribe`: a snapshot, then deltas and meters) and,
//! when asked, to the studio's master (`stream.subscribe {live: true}`, heard while the studio is
//! stopped too). Control changes go back as the same commands the studio's own Mix window runs
//! (`engine.execute`, a drag as one `gesture` closed by `engine.end_gesture`).
//!
//! [`MixMirror`] is the pure part: it applies the feed ([`feed::apply_delta`]), notices a gap in
//! `seq` (the link then subscribes again for a fresh snapshot), decodes the compact meters, and
//! keeps the optimistic values of controls being moved here until the feed agrees (or the feed
//! wins once the hand lets go).
//!
//! Nothing here can make the studio wait: the studio's hub serves each subscriber on its own
//! bounded thread. A change queued while the link is down is never sent later: it is dropped and
//! counted ([`Studio::take_unsent`]).

use serde_json::{Value, json};
use soundcraft_engine::surface::feed;
use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::discovery::Found;
use crate::hub::read_line;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- The mirror (pure) ----

/// One send of a remote strip.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteSend {
    pub slot: usize,
    /// Where it goes, in words (a bus or track name as the studio routes it).
    pub target: String,
    pub level_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub pre_fader: bool,
}

/// One insert of a remote strip.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteInsert {
    pub slot: usize,
    pub module: u32,
    pub plugin: String,
    pub name: String,
    pub bypass: bool,
    /// A third-party plug-in: name and bypass only (its window lives on the studio PC).
    pub hosted: bool,
    /// A built-in module's parameters in plain units (`mix.module_param`), in the feed's order.
    pub params: Vec<(String, f32)>,
}

/// One strip of the studio's mixer, as the feed shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteStrip {
    pub id: u64,
    pub name: String,
    /// `audio`, `midi`, `instrument`, `aux`, `master`, `vca`, `folder`.
    pub kind: String,
    pub color: [u8; 3],
    pub format: String,
    pub volume_db: f32,
    pub pan: Vec<f32>,
    pub mute: bool,
    pub solo: bool,
    pub record_arm: bool,
    /// `Auto`, `On` or `Off`.
    pub input_monitor: String,
    pub automation_mode: String,
    pub vca: Option<u64>,
    pub groups: Vec<u64>,
    pub inactive: bool,
    pub hidden: bool,
    pub sends: Vec<RemoteSend>,
    pub inserts: Vec<RemoteInsert>,
}

/// A group of the studio's session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteGroup {
    pub id: u64,
    pub name: String,
    pub letter: String,
    pub members: Vec<u64>,
    pub active: bool,
    pub color: [u8; 3],
}

/// A mix snapshot stored in the studio's session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteSnapshot {
    pub index: usize,
    pub name: String,
}

/// The studio's mixer as this PC shows it (the feed with this PC's moves on top).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MixerView {
    pub session: String,
    pub strips: Vec<RemoteStrip>,
    pub groups: Vec<RemoteGroup>,
    /// Bus names by id.
    pub busses: Vec<(u64, String)>,
    pub solo_mode: String,
    /// `stopped`, `playing` or `recording`.
    pub transport: String,
    pub pre_fader_metering: bool,
    /// The studio's next Undo and Redo, by their labels (absent from an older studio).
    pub undo: Option<String>,
    pub redo: Option<String>,
    pub snapshots: Vec<RemoteSnapshot>,
}

impl MixerView {
    pub fn strip(&self, id: u64) -> Option<&RemoteStrip> {
        self.strips.iter().find(|s| s.id == id)
    }

    /// Any strip soloed (the others are muted by it).
    pub fn any_solo(&self) -> bool {
        self.strips.iter().any(|s| s.solo)
    }
}

/// A strip's meters as the studio's Mix window shows them (linear; gain reduction in dB).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterLevels {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub gr_db: f32,
}

/// A level byte of the meters message: dB = byte / 2 - 100, 0 silence.
pub fn level_from_byte(b: u8) -> f32 {
    if b == 0 { 0.0 } else { 10f32.powf((f32::from(b) / 2.0 - 100.0) / 20.0) }
}

/// Decodes a `mixer.meters` message's `{ids, lv}`: five bytes a strip, in `ids`' order.
pub fn decode_meters(params: &Value) -> Option<Vec<(u64, MeterLevels)>> {
    let ids: Vec<u64> = params.get("ids")?.as_array()?.iter().filter_map(Value::as_u64).collect();
    let bytes = crate::audio::b64_decode(params.get("lv")?.as_str()?)?;
    let mut out = Vec::with_capacity(ids.len());
    for (k, id) in ids.iter().enumerate() {
        let b = bytes.get(k * 5..k * 5 + 5)?;
        let g = |i: usize| b.get(i).copied().unwrap_or(0);
        out.push((
            *id,
            MeterLevels {
                peak: [level_from_byte(g(0)), level_from_byte(g(1))],
                rms: [level_from_byte(g(2)), level_from_byte(g(3))],
                gr_db: -f32::from(g(4)) / 4.0,
            },
        ));
    }
    Some(out)
}

/// A control this PC can move on a remote strip (the key of an optimistic value).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Control {
    Volume(u64),
    Pan(u64, usize),
    Mute(u64),
    Solo(u64),
    /// A send's level, by slot.
    Send(u64, usize),
    /// An insert's bypass, by module id.
    Bypass(u64, u32),
    /// A built-in module's parameter.
    Param(u64, u32, String),
}

impl Control {
    pub fn strip(&self) -> u64 {
        match self {
            Control::Volume(s) | Control::Pan(s, _) | Control::Mute(s) | Control::Solo(s) | Control::Send(s, _) => *s,
            Control::Bypass(s, _) | Control::Param(s, _, _) => *s,
        }
    }

    /// The gesture key a drag of this control sends (`engine.execute {gesture}`): the same key
    /// for the same control on every PC, so the studio knows two hands are on one control.
    pub fn gesture(&self) -> String {
        match self {
            Control::Volume(s) => format!("fader:{s}"),
            Control::Pan(s, i) => format!("pan:{s}:{i}"),
            Control::Mute(s) => format!("mute:{s}"),
            Control::Solo(s) => format!("solo:{s}"),
            Control::Send(s, k) => format!("send:{s}:{k}"),
            Control::Bypass(s, m) => format!("bypass:{s}:{m}"),
            Control::Param(s, m, p) => format!("param:{s}:{m}:{p}"),
        }
    }
}

/// How long a control let go keeps its optimistic value while the feed has not shown it yet;
/// after that the feed wins (someone else moved it, or the change was refused).
pub const RECONCILE: Duration = Duration::from_millis(700);

#[derive(Debug, Clone)]
struct Optimistic {
    value: f32,
    held: bool,
    released: Option<Instant>,
}

/// What a feed message did to the mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    Changed,
    Unchanged,
    /// A delta did not follow the last one: the mirror waits for a fresh snapshot (subscribe
    /// again).
    Gap,
}

/// The studio's mixer kept up to date from its feed (see the module docs).
#[derive(Debug, Default)]
pub struct MixMirror {
    state: Option<Value>,
    seq: Option<u64>,
    view: Option<MixerView>,
    meters: HashMap<u64, MeterLevels>,
    meter_seq: Option<u64>,
    optimistic: HashMap<Control, Optimistic>,
    /// Snapshots taken (one per connection, more after a gap).
    pub snapshots: u32,
    /// Gaps seen in `seq`.
    pub gaps: u32,
    /// Changes applied (for redraw checks).
    pub revision: u64,
}

impl MixMirror {
    /// A feed message: `mixer.snapshot`, `mixer.delta` or `mixer.meters` and its params.
    pub fn on_event(&mut self, event: &str, params: &Value) -> Fed {
        match event {
            "mixer.snapshot" => {
                let Some(state) = params.get("state").filter(|s| s.is_object()) else { return Fed::Unchanged };
                self.state = Some(state.clone());
                self.seq = params.get("seq").and_then(Value::as_u64);
                self.snapshots += 1;
                self.rebuild();
                Fed::Changed
            }
            "mixer.delta" => {
                let seq = params.get("seq").and_then(Value::as_u64);
                match (self.seq, seq, self.state.as_mut()) {
                    (Some(last), Some(s), Some(state)) if s == last.wrapping_add(1) => {
                        feed::apply_delta(state, params);
                        self.seq = Some(s);
                        self.rebuild();
                        Fed::Changed
                    }
                    // Old news (already in the snapshot).
                    (Some(last), Some(s), Some(_)) if s <= last => Fed::Unchanged,
                    _ => {
                        self.seq = None;
                        self.gaps += 1;
                        Fed::Gap
                    }
                }
            }
            "mixer.meters" => {
                let seq = params.get("seq").and_then(Value::as_u64);
                if seq.is_some() && seq == self.meter_seq {
                    return Fed::Unchanged;
                }
                self.meter_seq = seq;
                match decode_meters(params) {
                    Some(m) => {
                        self.meters = m.into_iter().collect();
                        Fed::Changed
                    }
                    None => Fed::Unchanged,
                }
            }
            _ => Fed::Unchanged,
        }
    }

    /// A new connection: the next snapshot starts over (what is shown stays until it comes).
    pub fn reconnected(&mut self) {
        self.seq = None;
        self.meter_seq = None;
        self.optimistic.clear();
    }

    /// Waiting for a snapshot (none yet, or after a gap).
    pub fn needs_snapshot(&self) -> bool {
        self.seq.is_none()
    }

    pub fn has_state(&self) -> bool {
        self.view.is_some()
    }

    /// The raw feed state (as the studio's [`feed::mixer_state`] makes it).
    pub fn state(&self) -> Option<&Value> {
        self.state.as_ref()
    }

    fn rebuild(&mut self) {
        self.view = self.state.as_ref().map(view_of);
        self.revision += 1;
    }

    /// The meters of strip `id` (the latest message).
    pub fn meters(&self, id: u64) -> Option<MeterLevels> {
        self.meters.get(&id).copied()
    }

    /// The mixer as the feed has it, without this PC's moves.
    pub fn feed_view(&self) -> Option<&MixerView> {
        self.view.as_ref()
    }

    /// The mixer to draw: the feed with the controls being moved here on top. Settles the
    /// optimistic values first (see [`MixMirror::settle`]).
    pub fn view(&mut self) -> Option<MixerView> {
        self.settle(Instant::now());
        let mut v = self.view.clone()?;
        for (c, o) in &self.optimistic {
            if let Some(s) = v.strips.iter_mut().find(|s| s.id == c.strip()) {
                set_control(s, c, o.value);
            }
        }
        Some(v)
    }

    /// Drops the optimistic values the feed has caught up with, and those let go longer ago
    /// than [`RECONCILE`] (the feed wins).
    pub fn settle(&mut self, now: Instant) {
        let view = self.view.as_ref();
        self.optimistic.retain(|c, o| {
            if o.held {
                return true;
            }
            let fed = view.and_then(|v| v.strip(c.strip())).and_then(|s| control_value(s, c));
            let agrees = fed.is_some_and(|f| (f - o.value).abs() < 0.011);
            !agrees && o.released.is_some_and(|t| now.saturating_duration_since(t) < RECONCILE)
        });
    }

    /// This PC moved `c` to `value` (shown at once). `held`: a drag still in progress.
    pub fn set(&mut self, c: Control, value: f32, held: bool) {
        self.optimistic.insert(c, Optimistic { value, held, released: (!held).then(Instant::now) });
    }

    /// The drag of `c` ended: the feed wins once it shows the value or [`RECONCILE`] passes.
    pub fn release(&mut self, c: &Control) {
        if let Some(o) = self.optimistic.get_mut(c) {
            o.held = false;
            o.released = Some(Instant::now());
        }
    }

    /// Forgets every optimistic value (a refused change, a lost link): the feed shows.
    pub fn drop_optimistic(&mut self) {
        self.optimistic.clear();
    }

    /// Whether `c` shows an optimistic value now.
    pub fn is_optimistic(&self, c: &Control) -> bool {
        self.optimistic.contains_key(c)
    }
}

/// The value of control `c` on strip `s` (bools as 0 or 1).
pub fn control_value(s: &RemoteStrip, c: &Control) -> Option<f32> {
    let b = |v: bool| if v { 1.0 } else { 0.0 };
    match c {
        Control::Volume(_) => Some(s.volume_db),
        Control::Pan(_, i) => s.pan.get(*i).copied(),
        Control::Mute(_) => Some(b(s.mute)),
        Control::Solo(_) => Some(b(s.solo)),
        Control::Send(_, k) => s.sends.iter().find(|x| x.slot == *k).map(|x| x.level_db),
        Control::Bypass(_, m) => s.inserts.iter().find(|x| x.module == *m).map(|x| b(x.bypass)),
        Control::Param(_, m, p) => s.inserts.iter().find(|x| x.module == *m).and_then(|x| x.params.iter().find(|(k, _)| k == p)).map(|(_, v)| *v),
    }
}

fn set_control(s: &mut RemoteStrip, c: &Control, v: f32) {
    match c {
        Control::Volume(_) => s.volume_db = v,
        Control::Pan(_, i) => {
            if let Some(p) = s.pan.get_mut(*i) {
                *p = v;
            }
        }
        Control::Mute(_) => s.mute = v > 0.5,
        Control::Solo(_) => s.solo = v > 0.5,
        Control::Send(_, k) => {
            if let Some(x) = s.sends.iter_mut().find(|x| x.slot == *k) {
                x.level_db = v;
            }
        }
        Control::Bypass(_, m) => {
            if let Some(x) = s.inserts.iter_mut().find(|x| x.module == *m) {
                x.bypass = v > 0.5;
            }
        }
        Control::Param(_, m, p) => {
            if let Some(x) = s.inserts.iter_mut().find(|x| x.module == *m)
                && let Some(pv) = x.params.iter_mut().find(|(k, _)| k == p)
            {
                pv.1 = v;
            }
        }
    }
}

fn f(v: &Value, k: &str) -> f32 {
    v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).map_or(0.0, |x| x as f32)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").chars().take(200).collect()
}

fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn ids(v: &Value, k: &str) -> Vec<u64> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default()
}

/// `#rrggbb` as bytes (grey when it is not one).
pub fn colour(text: &str) -> [u8; 3] {
    let h = text.trim_start_matches('#');
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
    match (h.len(), byte(0), byte(2), byte(4)) {
        (6, Some(r), Some(g), Some(b)) => [r, g, b],
        _ => [128, 128, 128],
    }
}

/// A route as the feed carries it (as `session.inspect` does), in words.
fn route_text(v: Option<&Value>, busses: &[(u64, String)]) -> String {
    match v {
        None | Some(Value::Null) => "none".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => {
            if let Some(id) = o.get("Bus").and_then(Value::as_u64) {
                return busses.iter().find(|(b, _)| *b == id).map_or_else(|| format!("Bus {id}"), |(_, n)| n.clone());
            }
            o.iter().next().map_or_else(|| "none".into(), |(k, v)| if v.is_null() { k.clone() } else { format!("{k} {v}") })
        }
        Some(other) => other.to_string(),
    }
}

fn view_of(st: &Value) -> MixerView {
    let busses: Vec<(u64, String)> = st
        .get("busses")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|b| Some((b.get("id")?.as_u64()?, s(b, "name")))).collect())
        .unwrap_or_default();
    let strips = st
        .get("strips")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let id = x.get("id")?.as_u64()?;
                    let sends = x
                        .get("sends")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|y| {
                                    Some(RemoteSend {
                                        slot: usize::try_from(y.get("slot")?.as_u64()?).ok()?,
                                        target: route_text(y.get("target"), &busses),
                                        level_db: f(y, "level_db"),
                                        pan: f(y, "pan"),
                                        mute: b(y, "mute"),
                                        pre_fader: b(y, "pre_fader"),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let inserts = x
                        .get("inserts")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|y| {
                                    let hosted = b(y, "hosted");
                                    let params = if hosted {
                                        Vec::new()
                                    } else {
                                        y.get("params")
                                            .and_then(Value::as_object)
                                            .map(|p| p.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32))).collect())
                                            .unwrap_or_default()
                                    };
                                    Some(RemoteInsert {
                                        slot: usize::try_from(y.get("slot")?.as_u64()?).ok()?,
                                        module: u32::try_from(y.get("module")?.as_u64()?).ok()?,
                                        plugin: s(y, "plugin"),
                                        name: s(y, "name"),
                                        bypass: b(y, "bypass"),
                                        hosted,
                                        params,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(RemoteStrip {
                        id,
                        name: s(x, "name"),
                        kind: s(x, "kind"),
                        color: colour(&s(x, "color")),
                        format: s(x, "format"),
                        volume_db: x.get("volume_db").and_then(Value::as_f64).map_or(-144.0, |v| v as f32),
                        pan: x
                            .get("pan")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().map(|p| p.as_f64().unwrap_or(0.0) as f32).collect())
                            .unwrap_or_default(),
                        mute: b(x, "mute"),
                        solo: b(x, "solo"),
                        record_arm: b(x, "record_arm"),
                        input_monitor: s(x, "input_monitor"),
                        automation_mode: s(x, "automation_mode"),
                        vca: x.get("vca").and_then(Value::as_u64),
                        groups: ids(x, "groups"),
                        inactive: b(x, "inactive"),
                        hidden: b(x, "hidden"),
                        sends,
                        inserts,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let groups = st
        .get("groups")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|g| {
                    Some(RemoteGroup {
                        id: g.get("id")?.as_u64()?,
                        name: s(g, "name"),
                        letter: g.get("letter").map(|l| l.as_str().map_or_else(|| l.to_string(), str::to_string)).unwrap_or_default(),
                        members: ids(g, "members"),
                        active: b(g, "active"),
                        color: colour(&s(g, "color")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let label = |k: &str| st.get(k).and_then(Value::as_str).map(|x| x.chars().take(120).collect::<String>());
    MixerView {
        session: s(st, "session"),
        strips,
        groups,
        busses,
        solo_mode: st.get("solo_mode").map(|m| m.as_str().map_or_else(|| m.to_string(), str::to_string)).unwrap_or_default(),
        transport: st.get("transport").map(|t| s(t, "state")).unwrap_or_default(),
        pre_fader_metering: b(st, "pre_fader_metering"),
        undo: label("undo"),
        redo: label("redo"),
        snapshots: st
            .get("snapshots")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| Some(RemoteSnapshot { index: usize::try_from(x.get("index")?.as_u64()?).ok()?, name: s(x, "name") }))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// ---- The link ----

/// Which studio, and as whom.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StudioTarget {
    /// The studio's name as it announces itself (looked up by mDNS on the plain link).
    pub name: String,
    /// `host:port`: the studio's plain address, or a local bridge to its encrypted local link or
    /// its internet listener.
    pub addr: String,
    pub token: String,
    /// This PC's name.
    pub my_name: String,
}

/// How the link is doing, for the Remote Mix window's banner.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LinkHealth {
    pub connected: bool,
    /// `connecting`, `connected`, `reconnecting`, `not paired`, `sent away`, `refused: …`.
    pub state: String,
    /// The studio PC's own name.
    pub studio_pc: String,
    /// The last round trip of a ping (ms).
    pub rtt_ms: Option<f64>,
    /// Connections made so far (1 the first; more after drops).
    pub connects: u32,
    /// The stream: its buffer (ms), packets lost and late, and times it ran dry.
    pub buffer_ms: Option<f64>,
    pub lost: u64,
    pub late: u64,
    pub packets: u64,
    pub dropouts: u64,
}

/// What the studio answered to a change it did not take.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub command: String,
    pub error: String,
    /// The control is held by another PC (its name).
    pub held_by: Option<String>,
    /// Remote mixing is locked on the studio.
    pub locked: bool,
}

struct Inner {
    target: Mutex<StudioTarget>,
    health: Mutex<LinkHealth>,
    mirror: Mutex<MixMirror>,
    /// Lines waiting to go (commands), and those dropped because the link went.
    outbox: Mutex<VecDeque<(String, Value)>>,
    unsent: AtomicUsize,
    refusals: Mutex<Vec<Refusal>>,
    /// Commands sent and not answered yet: their id → (command, when).
    pending: Mutex<HashMap<String, (String, Instant)>>,
    /// Round trips of commands answered (ms), the newest last (bounded).
    answered: Mutex<VecDeque<f64>>,
    next_id: AtomicU64,
    resubscribe: AtomicBool,
    want_stream: AtomicBool,
    stream_in: Mutex<Option<(Arc<soundcraft_playback::stream::StreamIn>, u32)>>,
    decoder: Mutex<Option<crate::stream::MixDecoder>>,
    stream_on: AtomicBool,
    stop: AtomicBool,
    found: Mutex<Option<Arc<Mutex<Vec<Found>>>>>,
    waker: Mutex<Option<crate::hub::Waker>>,
    connects: AtomicU32,
    /// Bytes read from the studio (bandwidth).
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

/// A running link to a studio (stopped with [`Studio::stop`]).
#[derive(Clone)]
pub struct Studio {
    inner: Arc<Inner>,
}

/// A ping goes this often; without its answer for [`LOST_AFTER`] the link counts as lost.
const PING_EVERY: Duration = Duration::from_millis(1000);
const LOST_AFTER: Duration = Duration::from_secs(4);
/// The most changes queued (a burst beyond it drops the oldest: the newest value wins anyway).
const MAX_OUTBOX: usize = 256;

impl Studio {
    /// Starts the link to `target` (on its own thread).
    pub fn start(target: StudioTarget) -> Studio {
        let s = Studio {
            inner: Arc::new(Inner {
                target: Mutex::new(target),
                health: Mutex::new(LinkHealth { state: "connecting".into(), ..LinkHealth::default() }),
                mirror: Mutex::new(MixMirror::default()),
                outbox: Mutex::new(VecDeque::new()),
                unsent: AtomicUsize::new(0),
                refusals: Mutex::new(Vec::new()),
                pending: Mutex::new(HashMap::new()),
                answered: Mutex::new(VecDeque::new()),
                next_id: AtomicU64::new(1),
                resubscribe: AtomicBool::new(false),
                want_stream: AtomicBool::new(false),
                stream_in: Mutex::new(None),
                decoder: Mutex::new(None),
                stream_on: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                found: Mutex::new(None),
                waker: Mutex::new(None),
                connects: AtomicU32::new(0),
                bytes_in: AtomicU64::new(0),
                bytes_out: AtomicU64::new(0),
            }),
        };
        let me = s.clone();
        let _ = std::thread::Builder::new().name("freedawm-studio".into()).spawn(move || me.run());
        s
    }

    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        if let Some((q, _)) = &*lock(&self.inner.stream_in) {
            q.set_on(false);
        }
    }

    pub fn stopped(&self) -> bool {
        self.inner.stop.load(Ordering::SeqCst)
    }

    pub fn target(&self) -> StudioTarget {
        lock(&self.inner.target).clone()
    }

    /// Wakes the UI thread when the mixer changes.
    pub fn set_waker(&self, w: Option<crate::hub::Waker>) {
        *lock(&self.inner.waker) = w;
    }

    fn wake(&self) {
        if let Some(w) = lock(&self.inner.waker).as_ref() {
            w();
        }
    }

    /// Looks the studio up by name in what mDNS browsing finds (the plain link).
    pub fn set_lookup(&self, found: Arc<Mutex<Vec<Found>>>) {
        *lock(&self.inner.found) = Some(found);
    }

    pub fn connected(&self) -> bool {
        lock(&self.inner.health).connected
    }

    /// The link as the banner shows it.
    pub fn health(&self) -> LinkHealth {
        let mut h = lock(&self.inner.health).clone();
        h.connects = self.inner.connects.load(Ordering::SeqCst);
        let s = lock(&self.inner.stream_in).clone();
        if let Some((q, _)) = s {
            let d = lock(&self.inner.decoder);
            if let Some(d) = d.as_ref() {
                h.buffer_ms = Some(d.target_ms);
                h.lost = d.lost;
                h.late = d.late;
                h.packets = d.packets;
            }
            h.dropouts = q.starved();
        }
        h
    }

    /// The mirror, to read or move optimistic values.
    pub fn mirror(&self) -> MutexGuard<'_, MixMirror> {
        lock(&self.inner.mirror)
    }

    /// Bytes read from and written to the studio so far.
    pub fn bytes(&self) -> (u64, u64) {
        (self.inner.bytes_in.load(Ordering::Relaxed), self.inner.bytes_out.load(Ordering::Relaxed))
    }

    /// Runs `command` on the studio, as its own UI would (`engine.execute`); with `gesture`, one
    /// undo step with every other call of the same key until [`Studio::end_gesture`]. While the
    /// link is down nothing is queued: the change is counted as not sent. Returns whether it was
    /// queued.
    pub fn execute(&self, command: &str, params: Value, gesture: Option<&str>) -> bool {
        let mut p = json!({"command": command, "params": params});
        if let Some(g) = gesture {
            p["gesture"] = json!(g);
        }
        self.queue("engine.execute", p)
    }

    /// The drag of `gesture` ended (the studio closes its undo step).
    pub fn end_gesture(&self, gesture: &str) -> bool {
        self.queue("engine.end_gesture", json!({"gesture": gesture}))
    }

    fn queue(&self, method: &str, params: Value) -> bool {
        if !self.connected() {
            self.inner.unsent.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        let mut q = lock(&self.inner.outbox);
        if q.len() >= MAX_OUTBOX {
            q.pop_front();
            self.inner.unsent.fetch_add(1, Ordering::SeqCst);
        }
        q.push_back((method.to_string(), params));
        true
    }

    /// Changes not sent since the last call (the link went with them queued, or they were made
    /// while it was down). They are never sent later.
    pub fn take_unsent(&self) -> usize {
        self.inner.unsent.swap(0, Ordering::SeqCst)
    }

    /// Changes the studio refused since the last call.
    pub fn take_refusals(&self) -> Vec<Refusal> {
        std::mem::take(&mut *lock(&self.inner.refusals))
    }

    /// The round trips of the commands answered lately (ms), oldest first.
    pub fn command_rtts(&self) -> Vec<f64> {
        lock(&self.inner.answered).iter().copied().collect()
    }

    /// Hear the studio's master through the player's `stream_in` at session rate `rate` (once a
    /// frame from the UI thread; `None` without a player).
    pub fn set_stream(&self, want: bool, stream_in: Option<Arc<soundcraft_playback::stream::StreamIn>>, rate: u32) {
        self.inner.want_stream.store(want, Ordering::SeqCst);
        let mut cur = lock(&self.inner.stream_in);
        let same = match (&*cur, &stream_in) {
            (Some((a, r)), Some(b)) => Arc::ptr_eq(a, b) && *r == rate,
            (None, None) => true,
            _ => false,
        };
        if !same {
            if let Some((old, _)) = &*cur {
                old.set_on(false);
            }
            *cur = stream_in.map(|s| (s, rate));
        }
        let now = cur.clone();
        drop(cur);
        if !same {
            *lock(&self.inner.decoder) = None;
        }
        if let Some((s, _)) = &now {
            s.set_on(want && self.connected() && !self.stopped());
        }
    }

    fn set_health(&self, f: impl FnOnce(&mut LinkHealth)) {
        f(&mut lock(&self.inner.health));
        self.wake();
    }

    fn sleep(&self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end && !self.stopped() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn resolve(&self) -> Option<SocketAddr> {
        let mut t = lock(&self.inner.target);
        if let Some(found) = lock(&self.inner.found).as_ref() {
            let f = found.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(hit) = f.iter().find(|x| !t.name.is_empty() && x.name.eq_ignore_ascii_case(&t.name)) {
                t.addr = hit.addr.clone();
            }
        }
        t.addr.to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
    }

    fn run(&self) {
        while !self.stopped() {
            let mut wait = Duration::from_secs(2);
            match self.resolve() {
                None => self.set_health(|h| {
                    h.connected = false;
                    h.state = "looking for the studio".into();
                }),
                Some(addr) => {
                    if let Err(e) = self.session(addr) {
                        log::debug!("studio: {e}");
                        let state = lock(&self.inner.health).state.clone();
                        if state == "not paired" || state == "sent away" {
                            wait = Duration::from_secs(10);
                        } else if state == "connecting" || state.starts_with("looking") {
                            self.set_health(|h| h.state = "not reachable".into());
                        }
                    }
                }
            }
            self.link_lost();
            self.sleep(wait);
        }
        self.link_lost();
    }

    /// The link went: what was queued is dropped and counted, the stream goes silent, the mirror
    /// keeps the last picture until a fresh snapshot.
    fn link_lost(&self) {
        let dropped = {
            let mut q = lock(&self.inner.outbox);
            let n = q.iter().filter(|(m, _)| m == "engine.execute").count();
            q.clear();
            n
        };
        if dropped > 0 {
            self.inner.unsent.fetch_add(dropped, Ordering::SeqCst);
        }
        lock(&self.inner.pending).clear();
        if let Some((q, _)) = &*lock(&self.inner.stream_in) {
            q.set_on(false);
        }
        self.inner.stream_on.store(false, Ordering::SeqCst);
        self.inner.mirror.lock().unwrap_or_else(PoisonError::into_inner).drop_optimistic();
        let was = lock(&self.inner.health).connected;
        self.set_health(|h| {
            h.connected = false;
            if was || h.state == "connected" {
                h.state = "reconnecting".into();
            }
            h.rtt_ms = None;
        });
    }

    /// One connection, until it drops.
    fn session(&self, addr: SocketAddr) -> Result<(), String> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
        let _ = stream.set_nodelay(true);
        // Over the internet the first line also makes the connection: give it time.
        stream.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        let out = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
        let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let t = self.target();
        self.send(&out, &json!({"id": "hello", "method": "hello", "params": {"token": t.token, "name": t.my_name, "role": "listener"}}))?;
        let mut buf = Vec::new();
        // The answer to `hello` (an event the studio pushes first is skipped).
        let mut hello = Value::Null;
        for _ in 0..16 {
            if !read_line(&mut reader, &mut buf, 1 << 20)? {
                return Err("closed".into());
            }
            let v: Value = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
            if v["id"] == "hello" {
                hello = v;
                break;
            }
        }
        if hello["ok"] != true {
            let err = hello["error"].as_str().unwrap_or("refused").to_string();
            let state = if hello["away"] == true {
                "sent away".to_string()
            } else if hello["pair"] == true {
                "not paired".to_string()
            } else {
                format!("refused: {}", crate::pairing::clean_text(&err, 200))
            };
            self.set_health(|h| h.state = state);
            return Err(err);
        }
        stream.set_read_timeout(Some(Duration::from_secs(15))).map_err(|e| e.to_string())?;
        let pc = hello["result"]["pc"].as_str().unwrap_or("").to_string();
        lock(&self.inner.mirror).reconnected();
        self.inner.connects.fetch_add(1, Ordering::SeqCst);
        lock(&self.inner.outbox).clear();
        self.set_health(|h| {
            h.connected = true;
            h.state = "connected".into();
            h.studio_pc = pc;
        });
        self.send(&out, &json!({"id": "m", "method": "mixer.subscribe", "params": {"on": true, "meters": true}}))?;
        let done = Arc::new(AtomicBool::new(false));
        let last_heard = Arc::new(Mutex::new(Instant::now()));
        let reader_thread = {
            let me = self.clone();
            let (done, heard) = (Arc::clone(&done), Arc::clone(&last_heard));
            std::thread::Builder::new()
                .name("freedawm-studio-read".into())
                .spawn(move || {
                    me.read_loop(reader, &heard);
                    done.store(true, Ordering::SeqCst);
                })
                .map_err(|e| e.to_string())?
        };
        let result = self.write_loop(&out, &done, &last_heard);
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let _ = reader_thread.join();
        result
    }

    fn send(&self, out: &Arc<Mutex<TcpStream>>, v: &Value) -> Result<(), String> {
        let line = format!("{v}\n");
        let mut s = lock(out);
        s.write_all(line.as_bytes()).and_then(|_| s.flush()).map_err(|e| e.to_string())?;
        self.inner.bytes_out.fetch_add(line.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn write_loop(&self, out: &Arc<Mutex<TcpStream>>, done: &AtomicBool, heard: &Mutex<Instant>) -> Result<(), String> {
        let mut last_ping = Instant::now() - PING_EVERY;
        self.inner.stream_on.store(false, Ordering::SeqCst);
        while !self.stopped() && !done.load(Ordering::SeqCst) {
            if lock(heard).elapsed() > LOST_AFTER {
                return Err("the studio stopped answering".into());
            }
            // A gap in the feed: subscribe again for a fresh snapshot.
            if self.inner.resubscribe.swap(false, Ordering::SeqCst) {
                self.send(out, &json!({"id": "m", "method": "mixer.subscribe", "params": {"on": true, "meters": true}}))?;
            }
            let want = self.inner.want_stream.load(Ordering::SeqCst) && lock(&self.inner.stream_in).is_some();
            if want != self.inner.stream_on.load(Ordering::SeqCst) {
                self.inner.stream_on.store(want, Ordering::SeqCst);
                if let Some((q, _)) = &*lock(&self.inner.stream_in) {
                    q.set_on(want);
                }
                self.send(out, &json!({"id": "s", "method": "stream.subscribe", "params": {"on": want, "live": true}}))?;
            }
            if want {
                self.pump_stream();
            }
            if last_ping.elapsed() >= PING_EVERY {
                last_ping = Instant::now();
                let id = format!("p:{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
                self.track(&id, "net.ping");
                self.send(out, &json!({"id": id, "method": "net.ping", "params": {}}))?;
            }
            loop {
                let next = lock(&self.inner.outbox).pop_front();
                let Some((method, params)) = next else { break };
                let command = params.get("command").and_then(Value::as_str).unwrap_or(&method).to_string();
                let id = format!("c:{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
                self.track(&id, &command);
                self.send(out, &json!({"id": id, "method": method, "params": params}))?;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    fn track(&self, id: &str, what: &str) {
        let mut p = lock(&self.inner.pending);
        // Unanswered ones are forgotten.
        if p.len() > 512 {
            p.clear();
        }
        p.insert(id.to_string(), (what.to_string(), Instant::now()));
    }

    fn read_loop(&self, mut reader: BufReader<TcpStream>, heard: &Mutex<Instant>) {
        let mut buf = Vec::new();
        while !self.stopped() {
            match read_line(&mut reader, &mut buf, 64 << 20) {
                Ok(true) => {}
                _ => return,
            }
            self.inner.bytes_in.fetch_add(buf.len() as u64 + 1, Ordering::Relaxed);
            *lock(heard) = Instant::now();
            let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
            if let Some(event) = v.get("event").and_then(Value::as_str) {
                match event {
                    "stream" => self.on_stream(&v),
                    "mixer.snapshot" | "mixer.delta" | "mixer.meters" => {
                        let fed = lock(&self.inner.mirror).on_event(event, &v["params"]);
                        match fed {
                            Fed::Gap => self.inner.resubscribe.store(true, Ordering::SeqCst),
                            Fed::Changed => self.wake(),
                            Fed::Unchanged => {}
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let Some(id) = v.get("id").and_then(Value::as_str) else { continue };
            let Some((what, sent)) = lock(&self.inner.pending).remove(id) else { continue };
            let ms = sent.elapsed().as_secs_f64() * 1000.0;
            if what == "net.ping" {
                self.set_health(|h| h.rtt_ms = Some(ms));
                continue;
            }
            {
                let mut a = lock(&self.inner.answered);
                if a.len() >= 200 {
                    a.pop_front();
                }
                a.push_back(ms);
            }
            if v["ok"] != true {
                let error = v["error"].as_str().unwrap_or("refused").to_string();
                lock(&self.inner.refusals).push(Refusal {
                    command: what,
                    held_by: v["held_by"].as_str().map(str::to_string),
                    locked: v["locked"] == true,
                    error: crate::pairing::clean_text(&error, 300),
                });
                // What this PC showed was not taken: the feed shows what is.
                lock(&self.inner.mirror).drop_optimistic();
                self.wake();
            }
        }
    }

    fn pump_stream(&self) {
        let s = lock(&self.inner.stream_in).clone();
        let Some((queue, _)) = s else { return };
        if let Some(dec) = lock(&self.inner.decoder).as_mut() {
            dec.pump(&queue);
        }
    }

    fn on_stream(&self, v: &Value) {
        let Ok(p) = serde_json::from_value::<crate::stream::StreamPacket>(v["params"].clone()) else { return };
        let s = lock(&self.inner.stream_in).clone();
        let Some((queue, rate)) = s else { return };
        {
            let mut d = lock(&self.inner.decoder);
            let dec = d.get_or_insert_with(|| crate::stream::MixDecoder::new(rate));
            dec.on_packet(p, &queue);
        }
        self.pump_stream();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(n: usize) -> soundcraft_engine::Engine {
        let mut e = soundcraft_engine::Engine::default();
        for k in 0..n {
            let _ = e.execute("track.new", &json!({"name": format!("Ch {k}"), "kind": "audio"}));
        }
        e
    }

    fn id_of(e: &soundcraft_engine::Engine, name: &str) -> u64 {
        e.session().tracks.iter().find(|t| t.name == name).map_or(0, |t| t.id.0)
    }

    #[test]
    fn the_mirror_follows_snapshot_and_deltas_and_asks_again_after_a_gap() {
        let mut e = engine(3);
        let mut m = MixMirror::default();
        assert!(m.needs_snapshot());
        assert_eq!(m.on_event("mixer.snapshot", &json!({"seq": 5, "state": feed::mixer_state(&e)})), Fed::Changed);
        let a = id_of(&e, "Ch 1");
        let before = feed::mixer_state(&e);
        e.execute("mix.volume", &json!({"tracks": [a], "db": -7.5})).unwrap();
        let d = feed::mixer_delta(&before, &feed::mixer_state(&e)).unwrap();
        let mut d6 = d.clone();
        d6["seq"] = json!(6);
        assert_eq!(m.on_event("mixer.delta", &d6), Fed::Changed);
        assert_eq!(m.view().unwrap().strip(a).unwrap().volume_db, -7.5);
        // An old one again: nothing.
        assert_eq!(m.on_event("mixer.delta", &d6), Fed::Unchanged);
        // A gap: the mirror waits for a snapshot and keeps what it shows.
        let mut d9 = d;
        d9["seq"] = json!(9);
        assert_eq!(m.on_event("mixer.delta", &d9), Fed::Gap);
        assert!(m.needs_snapshot());
        assert!(m.has_state(), "the last picture stays");
        assert_eq!(m.gaps, 1);
        assert_eq!(m.on_event("mixer.snapshot", &json!({"seq": 12, "state": feed::mixer_state(&e)})), Fed::Changed);
        assert!(!m.needs_snapshot());
        assert_eq!(m.snapshots, 2);
    }

    #[test]
    fn strips_carry_what_the_remote_window_draws() {
        let mut e = engine(1);
        e.execute("track.new", &json!({"name": "Master", "kind": "master"})).unwrap();
        let a = id_of(&e, "Ch 0");
        e.execute("mix.insert", &json!({"track": a, "plugin": "eq_7band"})).unwrap();
        e.execute("mix.pan", &json!({"tracks": [a], "pan": -0.5})).unwrap();
        e.execute("mix.solo", &json!({"tracks": [a], "value": true})).unwrap();
        let mut m = MixMirror::default();
        m.on_event("mixer.snapshot", &json!({"seq": 1, "state": feed::mixer_state(&e)}));
        let v = m.view().unwrap();
        let s = v.strip(a).unwrap();
        assert_eq!(s.name, "Ch 0");
        assert_eq!(s.kind, "audio");
        assert!(s.solo && v.any_solo());
        assert!((s.pan[0] + 0.5).abs() < 0.01);
        let eq = s.inserts.iter().find(|i| i.plugin == "eq_7band").unwrap();
        assert!(!eq.hosted && !eq.params.is_empty(), "a built-in carries its parameters");
        assert!(v.strips.iter().any(|s| s.kind == "master"));
        // A third-party plug-in: name and bypass only, even if the studio sent more.
        let mut st = feed::mixer_state(&e);
        st["strips"][1]["inserts"] = json!([{"slot": 0, "module": 9, "plugin": "clap:com.acme.verb", "name": "Acme Verb", "bypass": true, "hosted": true, "params": {"x": 1}}]);
        m.on_event("mixer.snapshot", &json!({"seq": 1, "state": st}));
        let v = m.view().unwrap();
        let ins = &v.strips[1].inserts[0];
        assert_eq!((ins.name.as_str(), ins.bypass, ins.hosted, ins.params.len()), ("Acme Verb", true, true, 0));
    }

    #[test]
    fn a_moved_control_shows_at_once_and_the_feed_wins_after_release() {
        let mut e = engine(1);
        let a = id_of(&e, "Ch 0");
        let mut m = MixMirror::default();
        m.on_event("mixer.snapshot", &json!({"seq": 1, "state": feed::mixer_state(&e)}));
        let c = Control::Volume(a);
        m.set(c.clone(), -3.0, true);
        assert_eq!(m.view().unwrap().strip(a).unwrap().volume_db, -3.0, "optimistic");
        // Someone else moves it meanwhile: still held here, still shown as held.
        let before = feed::mixer_state(&e);
        e.execute("mix.volume", &json!({"tracks": [a], "db": -20.0})).unwrap();
        let mut d = feed::mixer_delta(&before, &feed::mixer_state(&e)).unwrap();
        d["seq"] = json!(2);
        m.on_event("mixer.delta", &d);
        assert_eq!(m.view().unwrap().strip(a).unwrap().volume_db, -3.0);
        // Let go: the feed wins once the reconcile time passes.
        m.release(&c);
        assert!(m.is_optimistic(&c));
        m.settle(Instant::now() + RECONCILE + Duration::from_millis(10));
        assert_eq!(m.view().unwrap().strip(a).unwrap().volume_db, -20.0);
        // Let go and the feed agrees: dropped at once.
        m.set(c.clone(), -20.0, false);
        m.settle(Instant::now());
        assert!(!m.is_optimistic(&c));
    }

    #[test]
    fn meters_decode_from_the_studios_bytes() {
        let levels = [(3u64, feed::StripLevels { peak: [1.0, 0.5], rms: [0.0, 0.25], gr_db: -3.0 }), (7, feed::StripLevels::default())];
        let msg = feed::meters_json(2.0, &levels);
        let got = decode_meters(&msg).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, 3);
        assert!((got[0].1.peak[0] - 1.0).abs() < 0.01 && (got[0].1.peak[1] - 0.5).abs() < 0.01);
        assert_eq!(got[0].1.rms[0], 0.0);
        assert!((got[0].1.rms[1] - 0.25).abs() < 0.01);
        assert_eq!(got[0].1.gr_db, -3.0);
        assert_eq!(got[1].1, MeterLevels::default());
        // Hostile input: short bytes or no ids.
        assert!(decode_meters(&json!({"ids": [1, 2], "lv": "AAAA"})).is_none());
        assert!(decode_meters(&json!({"lv": "AAAA"})).is_none());
        let mut m = MixMirror::default();
        assert_eq!(m.on_event("mixer.meters", &json!({"seq": 1, "ids": [3, 7], "lv": msg["lv"]})), Fed::Changed);
        assert!(m.meters(3).is_some_and(|l| l.peak[0] > 0.9));
    }

    #[test]
    fn colours_and_gesture_keys() {
        assert_eq!(colour("#ff8000"), [255, 128, 0]);
        assert_eq!(colour("nonsense"), [128, 128, 128]);
        assert_eq!(Control::Volume(4).gesture(), "fader:4");
        assert_eq!(Control::Param(4, 2, "gain".into()).gesture(), "param:4:2:gain");
    }
}
