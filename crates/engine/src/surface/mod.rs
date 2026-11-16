//! Control surfaces: hardware mixers that move the mix and show it back (motor faders, LEDs,
//! scribble strips, meters).
//!
//! The layer has three parts, so a protocol is only bytes and a new one goes behind the same
//! trait:
//!
//! - [`SurfaceProtocol`]: one protocol ([`mackie::Mackie`] for Mackie Control and its
//!   extenders, [`hui::Hui`] for HUI). It turns the device's MIDI into [`SurfaceEvent`]s
//!   (`parse`) and a [`SurfaceState`] into the MIDI that shows it (`render`, sending only what
//!   changed since it last sent), and keeps its link alive (`tick`: HUI's ping).
//! - [`SurfaceGroup`]: one surface and its extenders, sharing a bank. It applies the events to
//!   the engine through the ordinary commands (`mix.volume`, `mix.pan`, `mix.mute`, `mix.solo`,
//!   `mix.record_arm`, `edit.select`, the transport and automation-mode commands), so what a
//!   surface does is undone as the same on-screen action is: a fader move is one undo step from
//!   touch to release (`Engine::execute_merged`, as the Mix window's fader). A fader's touch holds
//!   the volume touched for automation (`Engine::automation_hold`), as holding the on-screen fader
//!   does, so Touch and Latch passes write while it is held and let go on release. It builds each
//!   unit's [`SurfaceState`] from the session (what a fader shows follows its automation while it
//!   reads and is not held: `Engine::automation_shown`).
//! - The host (the app's Setup › Control Surfaces window) opens the ports, feeds the device's
//!   bytes to [`SurfaceGroup::input`], and every frame sends what [`SurfaceGroup::output`] gives
//!   (with the meters it reads from the player). A surface's ports are its own: they are kept out
//!   of MIDI input (takes, Thru) and of MIDI output.
//!
//! Avid EUCON is not here: it is a proprietary network protocol available only through Avid's
//! licensed SDK. A EUCON bridge would be another [`SurfaceProtocol`] (its events and state are the
//! same roles), written against that SDK by someone who holds its licence.

pub mod hui;
pub mod mackie;

use crate::Engine;
use serde_json::{Value, json};
use soundcraft_model::{AutoParam, AutomationMode, Session, TrackId, TrackKind};

/// Strips one unit has (Mackie Control, its XT extender and HUI all have eight).
pub const STRIPS: usize = 8;

/// The protocols there are, by id (`mackie`, `hui`).
pub const PROTOCOLS: [(&str, &str); 3] = [("mackie", "Mackie Control"), ("mackie_xt", "Mackie Control Extender (XT)"), ("hui", "HUI")];

/// A protocol by id (`None` for an unknown one).
pub fn protocol(id: &str) -> Option<Box<dyn SurfaceProtocol>> {
    match id {
        "mackie" => Some(Box::new(mackie::Mackie::main())),
        "mackie_xt" => Some(Box::new(mackie::Mackie::extender())),
        "hui" => Some(Box::new(hui::Hui::new())),
        _ => None,
    }
}

/// A button on a surface, by role (each protocol maps its own notes or zone/port pairs here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Rec(u8),
    Solo(u8),
    Mute(u8),
    Select(u8),
    /// The V-Pot pushed in (pan back to centre).
    VPotPress(u8),
    Play,
    Stop,
    Record,
    Rewind,
    FastForward,
    ReturnToZero,
    End,
    Loop,
    Click,
    BankLeft,
    BankRight,
    ChannelLeft,
    ChannelRight,
    Undo,
    Redo,
    Save,
    /// Automation mode for the selected tracks.
    Auto(AutomationMode),
}

/// What a surface did, as its protocol reads it from the device's MIDI.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceEvent {
    /// A fader moved to `pos` (0..1 travel along the fader taper). Strip [`STRIPS`] is the master.
    Fader { strip: u8, pos: f32 },
    /// A touch-sensitive fader was touched or let go.
    Touch { strip: u8, on: bool },
    /// A V-Pot turned `delta` detents (positive clockwise).
    VPot { strip: u8, delta: i8 },
    /// A button went down (`true`) or up.
    Button { button: Button, down: bool },
    /// The jog wheel turned `delta` detents.
    Jog(i8),
    /// Bytes to send straight back (a handshake's answer).
    Reply(Vec<u8>),
}

/// What one strip shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StripState {
    /// No track on this strip (past the last one): dark, fader down.
    pub empty: bool,
    pub name: String,
    /// Fader travel 0..1 ([`soundcraft_model::fader_db_to_pos`]).
    pub fader: f32,
    /// Pan -1..1.
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub rec: bool,
    pub select: bool,
    /// Held by the hand on the surface: its motor stays still.
    pub touched: bool,
    /// Peak level 0..1 (linear) since the last frame.
    pub meter: f32,
}

/// What one unit shows: its strips, the master fader and the transport.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfaceState {
    pub strips: [StripState; STRIPS],
    /// The master fader's travel (main units).
    pub master: f32,
    pub master_touched: bool,
    pub playing: bool,
    pub recording: bool,
    pub looping: bool,
    pub click: bool,
    /// The automation mode of the first selected track.
    pub auto: Option<AutomationMode>,
}

/// One protocol's bytes (see the module docs). Implementations keep what they last sent, so
/// `render` sends only changes; `reset` forgets it, so the next `render` sends everything (a
/// device that came back).
pub trait SurfaceProtocol: Send {
    /// The protocol's id ([`PROTOCOLS`]).
    fn id(&self) -> &'static str;
    /// Whether this unit carries the transport and master fader (not an extender).
    fn is_main(&self) -> bool {
        true
    }
    /// The device's message `msg` (one complete MIDI message, SysEx included) as events.
    fn parse(&mut self, msg: &[u8], out: &mut Vec<SurfaceEvent>);
    /// The messages that bring the device to `state`.
    fn render(&mut self, state: &SurfaceState, out: &mut Vec<Vec<u8>>);
    /// Keeps the link alive: called every frame with the time in milliseconds.
    fn tick(&mut self, _now_ms: u64, _out: &mut Vec<Vec<u8>>) {}
    /// Forget what was sent: the next `render` sends everything.
    fn reset(&mut self);
}

/// One unit of a group: its protocol and its place in the bank.
struct Unit {
    protocol: Box<dyn SurfaceProtocol>,
    /// Its first strip's offset from the group's bank (0 for the first unit, 8 for the next).
    offset: usize,
    touched: [bool; STRIPS],
}

/// A surface and its extenders, sharing a bank (see the module docs).
pub struct SurfaceGroup {
    units: Vec<Unit>,
    /// The first track (in mixer order) on the first unit's first strip.
    bank: usize,
    master_touched: bool,
    events: Vec<SurfaceEvent>,
}

/// The tracks a surface can show, in mixer order: everything with a fader but the master
/// (which has its own) and video tracks.
pub fn surface_tracks(s: &Session) -> Vec<TrackId> {
    s.tracks.iter().filter(|t| !matches!(t.kind, TrackKind::Master | TrackKind::Video)).map(|t| t.id).collect()
}

fn master_track(s: &Session) -> Option<TrackId> {
    s.tracks.iter().find(|t| t.kind == TrackKind::Master).map(|t| t.id)
}

impl SurfaceGroup {
    /// A group of units, the first being the main one (an extender's strips follow it).
    pub fn new(protocols: Vec<Box<dyn SurfaceProtocol>>) -> SurfaceGroup {
        let units = protocols.into_iter().enumerate().map(|(i, protocol)| Unit { protocol, offset: i * STRIPS, touched: [false; STRIPS] }).collect();
        SurfaceGroup { units, bank: 0, master_touched: false, events: Vec::new() }
    }

    /// How many units it has.
    pub fn units(&self) -> usize {
        self.units.len()
    }

    /// The first track's index on the first strip.
    pub fn bank(&self) -> usize {
        self.bank
    }

    fn strips(&self) -> usize {
        self.units.len() * STRIPS
    }

    /// The track on `strip` of unit `unit` now.
    pub fn track_on(&self, s: &Session, unit: usize, strip: usize) -> Option<TrackId> {
        let u = self.units.get(unit)?;
        surface_tracks(s).get(self.bank + u.offset + strip).copied()
    }

    /// Unit `unit`'s device sent `msg`: applied to `e`. Returns what to send straight back to it.
    pub fn input(&mut self, e: &mut Engine, unit: usize, msg: &[u8]) -> Vec<Vec<u8>> {
        let mut events = std::mem::take(&mut self.events);
        events.clear();
        if let Some(u) = self.units.get_mut(unit) {
            u.protocol.parse(msg, &mut events);
        }
        let mut replies = Vec::new();
        for ev in events.drain(..) {
            if let SurfaceEvent::Reply(b) = ev {
                replies.push(b);
            } else {
                self.apply(e, unit, ev);
            }
        }
        self.events = events;
        replies
    }

    fn apply(&mut self, e: &mut Engine, unit: usize, ev: SurfaceEvent) {
        let track = |g: &SurfaceGroup, e: &Engine, strip: u8| {
            if usize::from(strip) == STRIPS { master_track(e.session()) } else { g.track_on(e.session(), unit, usize::from(strip)) }
        };
        let run = |e: &mut Engine, id: &str, p: Value| {
            if let Err(err) = e.execute(id, &p) {
                log::debug!("control surface: {id}: {err}");
            }
        };
        match ev {
            SurfaceEvent::Fader { strip, pos } => {
                let Some(t) = track(self, e, strip) else { return };
                let db = soundcraft_model::fader_pos_to_db(pos.clamp(0.0, 1.0)).clamp(-144.0, 12.0);
                if e.session().track(t).is_some_and(|tr| tr.mixer.automation_mode == AutomationMode::Trim) {
                    e.automation_trim(t, Some(db));
                } else if let Err(err) = e.execute_merged("mix.volume", &json!({"track": t.0, "db": db}), &format!("surface-fader:{}", t.0)) {
                    log::debug!("control surface: fader: {err}");
                }
            }
            SurfaceEvent::Touch { strip, on } => {
                if usize::from(strip) == STRIPS {
                    self.master_touched = on;
                } else if let Some(x) = self.units.get_mut(unit).and_then(|u| u.touched.get_mut(usize::from(strip))) {
                    *x = on;
                }
                let Some(t) = track(self, e, strip) else { return };
                if on {
                    e.automation_hold(t, AutoParam::Volume, true);
                } else {
                    e.automation_hold(t, AutoParam::Volume, false);
                    if e.session().track(t).is_some_and(|tr| tr.mixer.automation_mode == AutomationMode::Trim) {
                        e.automation_trim(t, None);
                    }
                    // The gesture is one undo step, from touch to release.
                    e.end_merge();
                }
            }
            SurfaceEvent::VPot { strip, delta } => {
                let Some(t) = track(self, e, strip) else { return };
                let pan = e.session().track(t).and_then(|tr| tr.mixer.pan.first().copied()).unwrap_or(0.0);
                let shown = e.automation_shown(t, &AutoParam::Pan(0)).unwrap_or(pan);
                let to = (shown + f32::from(delta) * 0.02).clamp(-1.0, 1.0);
                if let Err(err) = e.execute_merged("mix.pan", &json!({"track": t.0, "pan": to}), &format!("surface-pan:{}", t.0)) {
                    log::debug!("control surface: pan: {err}");
                }
            }
            SurfaceEvent::Jog(delta) => {
                let s = e.session();
                // A tenth of a second a detent (a beat would depend on the tempo map).
                let step = s.sample_rate.as_f64() / 10.0;
                let at = e.transport.position.max(s.edit.playhead);
                let to = (at as f64 + f64::from(delta) * step).max(0.0) as i64;
                run(e, "transport.locate", json!({"at": to}));
            }
            SurfaceEvent::Button { button, down: true } => self.press(e, unit, button),
            SurfaceEvent::Button { .. } | SurfaceEvent::Reply(_) => {}
        }
    }

    fn press(&mut self, e: &mut Engine, unit: usize, button: Button) {
        let run = |e: &mut Engine, id: &str, p: Value| {
            if let Err(err) = e.execute(id, &p) {
                log::debug!("control surface: {id}: {err}");
            }
        };
        let strip_track = |g: &SurfaceGroup, e: &Engine, k: u8| g.track_on(e.session(), unit, usize::from(k));
        let flip = |e: &mut Engine, id: &str, k: Option<TrackId>, get: fn(&soundcraft_model::Mixer) -> bool| {
            let Some(t) = k else { return };
            let now = e.session().track(t).is_some_and(|tr| get(&tr.mixer));
            run(e, id, json!({"tracks": [t.0], "value": !now}));
        };
        let total = surface_tracks(e.session()).len();
        let last_bank = total.saturating_sub(self.strips().min(total.max(1)));
        match button {
            Button::Mute(k) => flip(e, "mix.mute", strip_track(self, e, k), |m| m.mute),
            Button::Solo(k) => flip(e, "mix.solo", strip_track(self, e, k), |m| m.solo),
            Button::Rec(k) => flip(e, "mix.record_arm", strip_track(self, e, k), |m| m.record_arm),
            Button::Select(k) => {
                if let Some(t) = strip_track(self, e, k) {
                    run(e, "edit.select", json!({"tracks": [t.0]}));
                }
            }
            Button::VPotPress(k) => {
                if let Some(t) = strip_track(self, e, k) {
                    e.end_merge();
                    run(e, "mix.pan", json!({"track": t.0, "pan": 0.0}));
                }
            }
            Button::Play => run(e, "transport.play", json!({})),
            Button::Stop => run(e, "transport.stop", json!({})),
            Button::Record => run(e, "transport.record", json!({})),
            Button::Rewind => run(e, "transport.rewind", json!({})),
            Button::FastForward => run(e, "transport.fast_forward", json!({})),
            Button::ReturnToZero => run(e, "transport.rtz", json!({})),
            Button::End => run(e, "transport.go_to_end", json!({})),
            Button::Loop => run(e, "options.loop_playback", json!({})),
            Button::Click => run(e, "options.click", json!({})),
            Button::Undo => run(e, "edit.undo", json!({})),
            Button::Redo => run(e, "edit.redo", json!({})),
            Button::Save => run(e, "file.save", json!({})),
            Button::BankLeft => self.bank = self.bank.saturating_sub(self.strips()),
            Button::BankRight => self.bank = (self.bank + self.strips()).min(last_bank),
            Button::ChannelLeft => self.bank = self.bank.saturating_sub(1),
            Button::ChannelRight => self.bank = (self.bank + 1).min(last_bank),
            Button::Auto(mode) => {
                let sel = e.session().edit.selected_tracks.clone();
                if !sel.is_empty() {
                    let ids: Vec<u64> = sel.iter().map(|t| t.0).collect();
                    run(e, "mix.automation_mode", json!({"tracks": ids, "mode": mode.label()}));
                }
            }
        }
    }

    /// What unit `unit` should show now; `meter` gives a track's peak (linear) since the last
    /// frame.
    pub fn state(&self, e: &Engine, unit: usize, meter: &dyn Fn(TrackId) -> f32) -> SurfaceState {
        let s = e.session();
        let mut st = SurfaceState::default();
        let Some(u) = self.units.get(unit) else { return st };
        let tracks = surface_tracks(s);
        let fader_of = |t: TrackId| {
            let tr = s.track(t);
            let db = e.automation_shown(t, &AutoParam::Volume).or_else(|| tr.map(|tr| tr.mixer.volume_db)).unwrap_or(-144.0);
            soundcraft_model::fader_db_to_pos(db).clamp(0.0, 1.0)
        };
        for (k, strip) in st.strips.iter_mut().enumerate() {
            let Some(tr) = tracks.get(self.bank + u.offset + k).and_then(|t| s.track(*t)) else {
                strip.empty = true;
                continue;
            };
            strip.name = tr.name.clone();
            strip.fader = fader_of(tr.id);
            let pan = tr.mixer.pan.first().copied().unwrap_or(0.0);
            strip.pan = e.automation_shown(tr.id, &AutoParam::Pan(0)).unwrap_or(pan).clamp(-1.0, 1.0);
            strip.mute = tr.mixer.mute;
            strip.solo = tr.mixer.solo;
            strip.rec = tr.mixer.record_arm;
            strip.select = s.edit.selected_tracks.contains(&tr.id);
            strip.touched = u.touched.get(k).copied().unwrap_or(false);
            strip.meter = meter(tr.id).clamp(0.0, 1.0);
        }
        st.master = master_track(s).map_or(0.0, fader_of);
        st.master_touched = self.master_touched;
        st.playing = e.transport.playing;
        st.recording = e.transport.recording;
        st.looping = s.edit.loop_playback;
        st.click = s.edit.click;
        st.auto = s.edit.selected_tracks.first().and_then(|t| s.track(*t)).map(|t| t.mixer.automation_mode);
        st
    }

    /// Every frame: what each unit should be sent (`(unit, message)`), its keep-alive included.
    pub fn output(&mut self, e: &Engine, now_ms: u64, meter: &dyn Fn(TrackId) -> f32) -> Vec<(usize, Vec<u8>)> {
        let mut out = Vec::new();
        let mut msgs = Vec::new();
        for i in 0..self.units.len() {
            let st = self.state(e, i, meter);
            msgs.clear();
            if let Some(u) = self.units.get_mut(i) {
                u.protocol.tick(now_ms, &mut msgs);
                u.protocol.render(&st, &mut msgs);
            }
            out.extend(msgs.drain(..).map(|m| (i, m)));
        }
        out
    }

    /// Unit `unit` reconnected: everything is sent again.
    pub fn reset(&mut self, unit: usize) {
        if let Some(u) = self.units.get_mut(unit) {
            u.protocol.reset();
            u.touched = [false; STRIPS];
        }
    }
}

/// The first `n` characters of `name` for a scribble strip, padded with spaces (non-ASCII
/// shown as `?`).
pub fn strip_text(name: &str, n: usize) -> Vec<u8> {
    let mut out: Vec<u8> = name.chars().filter(|c| !c.is_control()).take(n).map(|c| if c.is_ascii() { c as u8 } else { b'?' }).collect();
    out.resize(n, b' ');
    out
}

/// A 14-bit fader value from travel 0..1.
pub fn fader_14(pos: f32) -> u16 {
    (pos.clamp(0.0, 1.0) * 16383.0).round() as u16
}

/// Travel 0..1 from a 14-bit fader value.
pub fn fader_pos(v: u16) -> f32 {
    f32::from(v.min(16383)) / 16383.0
}

/// A meter level 0..12 from a linear peak (Mackie's and HUI's meters both have 12 steps, about
/// 5 dB each from -60 dB).
pub fn meter_step(peak: f32) -> u8 {
    if peak.is_nan() || peak <= 0.0 {
        return 0;
    }
    let db = 20.0 * peak.log10();
    ((db + 60.0) / 5.0).clamp(0.0, 12.0).round() as u8
}
