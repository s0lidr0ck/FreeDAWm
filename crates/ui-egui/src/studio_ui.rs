//! File › Connect to a Studio… and the Remote Mix window: mixing another PC's studio from here
//! (a live stream fed from the studio, its engineer at home).
//!
//! - **The dialog** lists the studios found on this network, the studios mixed before (one
//!   click reconnects with the kept pairing, no code), and a box for the code the studio shows,
//!   its booth PIN, or a `freedawm-invite:` code. It reuses the network's pairing and encrypted
//!   links and only dials out: this PC opens nothing. Its pairing is kept in the network
//!   settings (`studios`), apart from the booth this PC may follow as a remote recorder.
//! - **The Remote Mix window** is a window of its own (never the local Mix window: another title,
//!   a purple banner "Mixing <studio> remotely"), drawn from the studio's mixer feed through
//!   [`soundcraft_net::studio::MixMirror`]. It reuses the Mix window's widgets (fader, pan knob,
//!   keys, meters) but not its strip code, which reads and writes the local session in well over
//!   a hundred places: a sibling that draws only what the feed carries is the honest route.
//! - **Hearing**: the studio's master comes through the same player input the remote recorder's
//!   "Hear the booth's mix" uses (mixed into this PC's output, on its output device), with its own
//!   volume and mute here; it plays while the studio is stopped.
//!
//! Nothing here touches this PC's own session.

use crate::SoundApp;
use crate::net_ui::NetState;
use crate::theme::{Tokens, bold, regular, rgb};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_net::config::KnownStudio;
use soundcraft_net::studio::{Control, MixerView, RemoteStrip, Studio, StudioTarget};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The Remote Mix banner's colour: never the local Mix window's.
pub const REMOTE_COLOUR: Color32 = Color32::from_rgb(112, 64, 160);

/// What the dialog says under the box.
pub const DIALS_ONLY_NOTE: &str =
    "This PC only dials out: it opens nothing to the network. Your own session is not touched: the studio's mixer opens in its own window.";

/// The banner while the studio has locked remote mixing.
pub const LOCKED_NOTE: &str = "The studio has locked remote mixing: you can watch and listen.";

/// A third-party plug-in's tooltip in the remote window.
pub const HOSTED_NOTE: &str = "A third-party plug-in: its window and settings live on the studio PC. From here: name and bypass only.";

/// A pairing waiting for the code or PIN typed in the box.
#[derive(Debug, Clone)]
struct PendingPair {
    studio: KnownStudio,
    /// Where the request went (a bridge to the encrypted link, or the plain address).
    addr: String,
    request: String,
    prompt: String,
}

/// File › Connect to a Studio… and the link it made (kept in [`NetState`]).
pub struct StudioState {
    /// The link to the studio being mixed, and that studio's record.
    pub link: Option<Studio>,
    pub current: Option<KnownStudio>,
    /// The Remote Mix window is open.
    pub window: bool,
    /// The dialog's box, and what the dialog says.
    pub entry: String,
    pub status: String,
    job: Arc<Mutex<Option<Result<Value, String>>>>,
    pending: Option<PendingPair>,
    /// Hearing the studio's master: on, its volume (linear) and mute.
    pub hear: bool,
    pub volume: f32,
    pub muted: bool,
    /// Notes shown in the window for a few seconds (changes not sent, refusals).
    pub notes: Vec<(String, Instant)>,
    /// This window's meters (peak hold, clip) per remote strip.
    meters: HashMap<u64, crate::MeterDisplay>,
    meters_at: Option<Instant>,
    /// For renders and UI tests: shown as connected or reconnecting with this mixer, no link.
    pub demo: Option<DemoLink>,
    /// The control being dragged here now (one gesture on the studio until it is let go).
    pub drag: Option<Control>,
    /// Undo, Redo (or a snapshot recall) waiting for "yes, on the studio"; and whether this
    /// connection already said yes once (the question is asked the first time only).
    pub confirm: Option<Confirm>,
    pub undo_ok: bool,
    /// The insert editor open on one of the studio's built-in effects.
    pub editor: Option<crate::studio_editors::RemoteEditor>,
}

/// Something that changes the studio's session as a whole, asked about first.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    Undo(String),
    Redo(String),
}

/// A stand-in link for renders and UI tests.
#[derive(Debug, Clone, Default)]
pub struct DemoLink {
    pub connected: bool,
    /// The studio has locked remote mixing.
    pub locked: bool,
    pub view: MixerView,
    pub levels: HashMap<u64, soundcraft_net::studio::MeterLevels>,
    pub rtt_ms: f64,
    pub buffer_ms: f64,
}

impl Default for StudioState {
    fn default() -> Self {
        StudioState {
            link: None,
            current: None,
            window: false,
            entry: String::new(),
            status: String::new(),
            job: Arc::new(Mutex::new(None)),
            pending: None,
            hear: true,
            volume: 1.0,
            muted: false,
            notes: Vec::new(),
            meters: HashMap::new(),
            meters_at: None,
            demo: None,
            drag: None,
            confirm: None,
            undo_ok: false,
            editor: None,
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl StudioState {
    /// Mixing a studio now (connected or trying to be).
    pub fn active(&self) -> bool {
        self.link.is_some() || self.demo.is_some()
    }

    /// The studio's name as the banner says it.
    pub fn studio_name(&self) -> String {
        if let Some(d) = &self.demo
            && self.current.is_none()
        {
            return if d.view.session.is_empty() { "the studio".into() } else { "Studio PC".into() };
        }
        self.current.as_ref().map_or_else(|| "the studio".into(), |s| s.name.clone())
    }

    fn note(&mut self, text: String) {
        self.notes.retain(|(t, _)| *t != text);
        self.notes.push((text, Instant::now()));
        if self.notes.len() > 4 {
            self.notes.remove(0);
        }
    }

    /// Stops mixing (the Disconnect button): the link goes, the studio's mix stays as last set.
    pub fn disconnect(&mut self) {
        if let Some(l) = self.link.take() {
            l.stop();
        }
        self.current = None;
        self.demo = None;
        self.window = false;
        self.meters.clear();
        self.drag = None;
        self.confirm = None;
        self.undo_ok = false;
        self.editor = None;
    }
}

/// Moves control `c` of the studio's mixer to `value` (shown here at once) with the command the
/// studio's own Mix window runs. `held`: a drag still going (one gesture, one undo step on the
/// studio, until it is let go).
fn act(net: &mut NetState, c: Control, value: f32, held: bool, command: &str, params: Value) {
    if held && net.studio.drag.as_ref() != Some(&c) {
        end_drag(net);
        net.studio.drag = Some(c.clone());
    }
    if let Some(link) = &net.studio.link {
        link.mirror().set(c.clone(), value, held);
        let g = c.gesture();
        link.execute(command, params, held.then_some(g.as_str()));
    } else if let Some(d) = &mut net.studio.demo
        && let Some(s) = d.view.strips.iter_mut().find(|s| s.id == c.strip())
    {
        soundcraft_net::studio::set_control(s, &c, value);
    }
}

/// The hand came off the control being dragged: the studio closes its undo step, and the feed
/// shows what the studio has.
fn end_drag(net: &mut NetState) {
    if let Some(c) = net.studio.drag.take()
        && let Some(link) = &net.studio.link
    {
        link.end_gesture(&c.gesture());
        link.mirror().release(&c);
    }
}

/// Runs a whole-session command on the studio (Undo, Redo).
fn run_on_studio(net: &mut NetState, command: &str) {
    if let Some(link) = &net.studio.link {
        link.execute(command, json!({}), None);
    }
}

/// Connects to `studio` with its kept pairing: the link it is reached by (internet bridge,
/// encrypted local link to its pinned key, or the plain address), then the Remote Mix window.
pub(crate) fn connect_known(net: &mut NetState, ctx: &egui::Context, mut studio: KnownStudio) {
    net.studio.disconnect();
    let addr = if !studio.internet.is_empty() {
        net.bridge(&studio.internet)
    } else if !studio.key.is_empty() && !studio.lan.is_empty() {
        // Where it announces its encrypted link now, when it moved (the key must match).
        if let Some((a, _)) = net.found().into_iter().find(|f| f.name == studio.name).and_then(|f| f.lan).filter(|(_, k)| *k == studio.key) {
            studio.lan = a;
        }
        net.lan_bridge(&studio.lan, &studio.key)
    } else {
        if let Some(f) = net.found().into_iter().find(|f| f.name == studio.name) {
            studio.addr = f.addr;
        }
        Ok(studio.addr.clone())
    };
    let addr = match addr {
        Ok(a) => a,
        Err(e) => {
            net.studio.status = e;
            return;
        }
    };
    let plain = studio.internet.is_empty() && studio.key.is_empty();
    let link = Studio::start(StudioTarget {
        name: if plain { studio.name.clone() } else { String::new() },
        addr,
        token: studio.token.clone(),
        my_name: net.hub.pc_name(),
    });
    let c = ctx.clone();
    link.set_waker(Some(Arc::new(move || c.request_repaint())));
    if plain && let Some(found) = net.found_handle() {
        link.set_lookup(found);
    }
    studio.last_used = unix_now();
    remember(net, &studio);
    net.studio.status = format!("Connecting to {}…", studio.name);
    net.studio.undo_ok = false;
    net.studio.link = Some(link);
    net.studio.current = Some(studio);
    net.studio.window = true;
}

/// Keeps `studio` in the network settings (replacing an older record of the same studio).
fn remember(net: &NetState, studio: &KnownStudio) {
    let s = studio.clone();
    net.hub.update_config(|c| {
        c.studios.retain(|k| !(k.name == s.name && (k.key == s.key || k.internet == s.internet || k.addr == s.addr)));
        c.studios.insert(0, s);
        c.studios.truncate(20);
    });
}

/// The studios to offer under "Studios you have mixed before": the ones kept, and the booth this
/// PC follows (its pairing works for mixing too).
fn known_studios(net: &NetState) -> Vec<KnownStudio> {
    let cfg = net.hub.config();
    let mut out = cfg.studios.clone();
    let f = &cfg.follow;
    if !f.token.is_empty() && !out.iter().any(|k| k.token == f.token) {
        out.push(KnownStudio {
            name: f.booth_name.clone(),
            addr: f.booth_addr.clone(),
            token: f.token.clone(),
            key: f.booth_key.clone(),
            lan: f.booth_lan.clone(),
            internet: f.internet.clone(),
            last_used: 0,
        });
    }
    out
}

/// What the box holds: an invite, an address, or a code / PIN for the pairing under way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Invite,
    Address(String),
    Code(String),
    Empty,
}

/// Reads the box.
pub fn read_entry(text: &str) -> Entry {
    let t = text.trim();
    if t.is_empty() {
        Entry::Empty
    } else if t.starts_with("freedawm-invite:") {
        Entry::Invite
    } else if t.contains(':') && t.rsplit(':').next().is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) && t.contains('.') {
        Entry::Address(t.to_string())
    } else {
        Entry::Code(t.to_string())
    }
}

/// Asks `studio` (found on the network or typed as an address) to pair: it shows a code on its
/// screen or waits for its booth PIN; the box then takes it.
fn ask_to_pair(net: &mut NetState, ctx: &egui::Context, studio: KnownStudio, lan: Option<(String, String)>) {
    let mut studio = studio;
    let addr = match &lan {
        Some((a, k)) => match net.lan_bridge(a, k) {
            Ok(b) => {
                studio.lan = a.clone();
                studio.key = k.clone();
                b
            }
            Err(e) => {
                log::info!("studio: pairing over the plain link: {e}");
                studio.addr.clone()
            }
        },
        None => studio.addr.clone(),
    };
    net.studio.status = format!("Asking {}…", if studio.name.is_empty() { &addr } else { &studio.name });
    net.studio.pending = Some(PendingPair { studio, addr: addr.clone(), request: String::new(), prompt: String::new() });
    let job = Arc::clone(&net.studio.job);
    *job.lock().unwrap_or_else(PoisonError::into_inner) = None;
    let me = net.hub.pc_name();
    let c = ctx.clone();
    std::thread::spawn(move || {
        let r = soundcraft_net::follower::pair_request_info(&addr, &me).map(|r| json!({"request": r.request, "pin": r.pin, "ask": r.ask}));
        *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
        c.request_repaint();
    });
}

/// The box's Connect: an invite joins over the internet; an address asks that studio to pair;
/// a code or PIN answers the pairing under way.
fn submit(net: &mut NetState, ctx: &egui::Context) {
    let text = net.studio.entry.trim().to_string();
    match read_entry(&text) {
        Entry::Empty => {}
        Entry::Invite => match soundcraft_net::internet::Invite::parse(&text) {
            Err(e) => net.studio.status = e,
            Ok(inv) => {
                let addr_json = serde_json::to_string(&inv.addr).unwrap_or_default();
                // Joined this studio before: its token goes along (to that studio only).
                let again = known_studios(net).into_iter().find(|k| same_key(&k.internet, &addr_json)).map(|k| k.token);
                match net.bridge(&addr_json) {
                    Err(e) => net.studio.status = e,
                    Ok(bridge) => {
                        net.studio.status = format!("Joining {}…", inv.name);
                        net.studio.entry.clear();
                        let studio = KnownStudio { name: inv.name.clone(), internet: addr_json, ..KnownStudio::default() };
                        net.studio.pending = Some(PendingPair { studio, addr: bridge.clone(), request: String::new(), prompt: String::new() });
                        let job = Arc::clone(&net.studio.job);
                        *job.lock().unwrap_or_else(PoisonError::into_inner) = None;
                        let me = net.hub.pc_name();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let r = soundcraft_net::follower::pair_invite_as(&bridge, &inv.secret, &me, again.as_deref())
                                .map(|(token, pc)| json!({"token": token, "pc": pc}));
                            *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
                            c.request_repaint();
                        });
                    }
                }
            }
        },
        Entry::Address(a) => {
            net.studio.entry.clear();
            ask_to_pair(net, ctx, KnownStudio { addr: a, ..KnownStudio::default() }, None);
        }
        Entry::Code(code) => {
            let Some(p) = net.studio.pending.clone().filter(|p| !p.request.is_empty()) else {
                net.studio.status = "Pick a studio found on this network first (or paste an invite, or type its address).".into();
                return;
            };
            net.studio.entry.clear();
            let keys = if p.studio.key.is_empty() {
                Some(soundcraft_net::pairing::exchange::LinkKeys::PLAIN)
            } else {
                net.lan_client_key().and_then(|me| soundcraft_net::pairing::exchange::LinkKeys::from_hex(&p.studio.key, &me))
            };
            let Some(keys) = keys else {
                net.studio.status = "The studio's key is damaged: pick it again.".into();
                return;
            };
            net.studio.status = "Pairing…".into();
            let job = Arc::clone(&net.studio.job);
            let c = ctx.clone();
            std::thread::spawn(move || {
                let r = soundcraft_net::follower::pair_confirm(&p.addr, &p.request, code, keys).map(|(token, pc)| json!({"token": token, "pc": pc}));
                *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
                c.request_repaint();
            });
        }
    }
}

/// Two internet addresses (JSON, from invites) name the same studio: the same key.
fn same_key(a: &str, b: &str) -> bool {
    let id = |s: &str| serde_json::from_str::<Value>(s).ok().map(|v| v["id"].clone()).filter(|v| !v.is_null());
    id(a).is_some_and(|a| Some(a) == id(b))
}

/// What a pairing job brought back: the request's id (the box then takes the code or PIN), or
/// the token (the studio is kept and connected).
fn take_job(net: &mut NetState, ctx: &egui::Context) {
    let got = net.studio.job.lock().unwrap_or_else(PoisonError::into_inner).take();
    let Some(r) = got else { return };
    match r {
        Ok(v) if v.get("request").is_some() => {
            let prompt = if v["ask"] == true {
                "Type the studio's PIN in the box (someone at the studio must also press Allow)."
            } else if v["pin"] == true {
                "Type the studio's PIN in the box."
            } else {
                "Type the code the studio shows in the box."
            };
            if let Some(p) = &mut net.studio.pending {
                p.request = v["request"].as_str().unwrap_or("").to_string();
                p.prompt = prompt.to_string();
            }
            net.studio.status = prompt.into();
        }
        Ok(v) => {
            let Some(mut p) = net.studio.pending.take() else { return };
            p.studio.token = v["token"].as_str().unwrap_or("").to_string();
            if let Some(pc) = v["pc"].as_str().filter(|s| !s.is_empty()) {
                p.studio.name = pc.to_string();
            }
            if p.studio.name.is_empty() {
                p.studio.name = p.studio.addr.clone();
            }
            net.studio.status = format!("Paired with {}.", p.studio.name);
            connect_known(net, ctx, p.studio);
        }
        Err(e) => {
            net.studio.status = e;
            if net.studio.pending.as_ref().is_some_and(|p| p.request.is_empty()) {
                net.studio.pending = None;
            }
        }
    }
}

/// Once a frame (from `net_ui::step`): the studio's master into this PC's output, at the
/// window's volume, and the notes the link left.
pub(crate) fn step(app: &SoundApp, net: &mut NetState) {
    let Some(link) = net.studio.link.clone() else { return };
    let rate = app.engine.session().sample_rate.hz();
    let input = app.player.as_ref().map(soundcraft_playback::Player::stream_in);
    if let Some(i) = &input {
        i.set_gain(if net.studio.muted { 0.0 } else { net.studio.volume.clamp(0.0, 2.0) });
    }
    link.set_stream(net.studio.hear && !net.studio.muted, input, rate);
    let unsent = link.take_unsent();
    if unsent > 0 {
        net.studio.note(format!("{unsent} change{} not sent: the link to the studio was down.", if unsent == 1 { " was" } else { "s were" }));
    }
    for r in link.take_refusals() {
        let text = match (&r.held_by, r.locked) {
            (Some(who), _) => format!("{who} is moving that control: it is theirs until they let go."),
            (None, true) => "The studio has locked remote mixing: you can watch and listen.".to_string(),
            (None, false) => format!("The studio refused {}: {}", r.command, r.error),
        };
        net.studio.note(text);
    }
    net.studio.notes.retain(|(_, at)| at.elapsed() < Duration::from_secs(8));
}

/// The studio's link is the one feeding the player's stream input now (the follower then
/// leaves it alone).
pub(crate) fn owns_stream(net: &NetState) -> bool {
    net.studio.link.as_ref().is_some_and(|l| !l.stopped())
}

/// The dialog and the Remote Mix window (from `net_ui::show`).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if app.net.is_none() {
        return;
    }
    if app.ui.show_connect_studio {
        dialog(app, ctx);
    }
    let shows = app.net.as_ref().is_some_and(|n| n.studio.window && n.studio.active());
    if shows {
        remote_window(app, ctx);
    }
}

fn dialog(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(mut net) = app.net.take() else { return };
    take_job(&mut net, ctx);
    let mut open = app.ui.show_connect_studio;
    let t = Tokens::current();
    let me = net.hub.pc_name();
    egui::Window::new("Connect to a Studio").open(&mut open).default_size(vec2(460.0, 420.0)).collapsible(false).show(ctx, |ui| {
        ui.label(egui::RichText::new("Mix a studio's live stream from here: see its mixer, hear its master, change the mix.").color(t.text_dim));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong("Studios on this network");
            if ui.small_button("Search").on_hover_text("Look for FreeDAWm PCs on this network").clicked() {
                net.browse();
            }
        });
        let found: Vec<_> = net.found().into_iter().filter(|f| f.name != me).collect();
        let known = known_studios(&net);
        if found.is_empty() {
            ui.label(
                egui::RichText::new("None found yet (the studio needs Setup › Network › Allow control from this network).").small().color(t.text_dim),
            );
        }
        let mut pick = None;
        for f in &found {
            ui.horizontal(|ui| {
                ui.label(&f.name);
                ui.label(egui::RichText::new(if f.lan.is_some() { "encrypted local link" } else { "local network" }).small().color(t.text_dim));
                if ui.button("Connect").clicked() {
                    pick = Some(f.clone());
                }
            });
        }
        if let Some(f) = pick {
            match known.iter().find(|k| k.name == f.name && k.internet.is_empty()) {
                Some(k) => connect_known(&mut net, ctx, k.clone()),
                None => {
                    ask_to_pair(&mut net, ctx, KnownStudio { name: f.name.clone(), addr: f.addr.clone(), ..KnownStudio::default() }, f.lan.clone())
                }
            }
        }
        ui.add_space(6.0);
        ui.strong("Studios you have mixed before");
        if known.is_empty() {
            ui.label(egui::RichText::new("None yet.").small().color(t.text_dim));
        }
        let mut again = None;
        let mut forget = None;
        for k in &known {
            ui.horizontal(|ui| {
                ui.label(&k.name);
                ui.label(egui::RichText::new(k.link()).small().color(t.text_dim));
                if ui.button("Reconnect").on_hover_text("With the pairing kept from before: no code, no invite").clicked() {
                    again = Some(k.clone());
                }
                if ui.small_button("Forget").on_hover_text("Forget this studio here (its own list keeps this PC until it forgets it too)").clicked() {
                    forget = Some(k.clone());
                }
            });
        }
        if let Some(k) = again {
            connect_known(&mut net, ctx, k);
        }
        if let Some(k) = forget {
            net.hub.update_config(|c| c.studios.retain(|s| s != &k));
        }
        ui.add_space(6.0);
        ui.strong("Paste the PIN, code or invite");
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut net.studio.entry)
                    .id(egui::Id::new("studio-entry"))
                    .hint_text("PIN, code, freedawm-invite:… or 192.168.1.20:48100")
                    .desired_width(300.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.add_enabled(!net.studio.entry.trim().is_empty(), egui::Button::new("Connect")).clicked() || enter {
                submit(&mut net, ctx);
            }
        });
        if let Some(p) = &net.studio.pending
            && !p.prompt.is_empty()
        {
            ui.label(egui::RichText::new(format!("Pairing with {}", if p.studio.name.is_empty() { &p.addr } else { &p.studio.name })).small());
        }
        if !net.studio.status.is_empty() {
            ui.label(egui::RichText::new(&net.studio.status).color(t.text));
        }
        if let Some(c) = &net.studio.current {
            ui.horizontal(|ui| {
                ui.label(format!("Mixing {} now.", c.name));
                if ui.button("Open the Remote Mix window").clicked() {
                    net.studio.window = true;
                }
            });
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new(DIALS_ONLY_NOTE).small().color(t.text_dim));
        ui.label(egui::RichText::new(crate::net_ui::PAIRING_NOTE).small().color(t.text_dim));
    });
    app.ui.show_connect_studio = open;
    app.net = Some(net);
}

/// What the window shows of the link: connected, its mixer, and the banner's quality line.
struct Shown {
    connected: bool,
    /// The studio locked remote mixing: watch and listen only.
    locked: bool,
    state: String,
    view: Option<MixerView>,
    quality: String,
    weak: bool,
}

fn shown(net: &mut NetState) -> Shown {
    if let Some(d) = &net.studio.demo {
        return Shown {
            connected: d.connected,
            locked: d.locked,
            state: if d.connected { "connected".into() } else { "reconnecting".into() },
            view: Some(d.view.clone()),
            quality: format!("Round trip {:.0} ms · stream buffer {:.0} ms · 0 dropouts", d.rtt_ms, d.buffer_ms),
            weak: false,
        };
    }
    let Some(link) = &net.studio.link else {
        return Shown { connected: false, locked: false, state: "not connected".into(), view: None, quality: String::new(), weak: false };
    };
    let h = link.health();
    let view = link.mirror().view();
    let mut parts = Vec::new();
    parts.push(h.rtt_ms.map_or_else(|| "Round trip …".to_string(), |r| format!("Round trip {r:.0} ms")));
    if let Some(b) = h.buffer_ms {
        parts.push(format!("stream buffer {b:.0} ms"));
    } else if net.studio.hear && !net.studio.muted {
        parts.push("stream starting…".into());
    }
    parts.push(format!("{} dropout{}", h.dropouts.min(999_999), if h.dropouts == 1 { "" } else { "s" }));
    if h.lost > 0 {
        parts.push(format!("{} lost", h.lost));
    }
    let weak = h.rtt_ms.is_some_and(|r| r > 400.0) || (h.packets > 50 && h.lost * 20 > h.packets);
    Shown { connected: h.connected, locked: link.locked(), state: h.state, view, quality: parts.join(" · "), weak }
}

/// The text of the greyed state while the link is down.
pub fn down_text(state: &str) -> String {
    match state {
        "not paired" => "The studio does not know this PC any more: pair again (File › Connect to a Studio…).".into(),
        "sent away" => "The studio disconnected this PC. It can let you back in from Setup › Network.".into(),
        "not reachable" => "Reconnecting… (the studio is not reachable)".into(),
        s if s.starts_with("refused") => format!("Reconnecting… ({s})"),
        _ => "Reconnecting…".into(),
    }
}

fn remote_window(app: &mut SoundApp, ctx: &egui::Context) {
    let title = {
        let Some(net) = app.net.as_ref() else { return };
        format!("Remote Mix — {} (not your session)", net.studio.studio_name())
    };
    let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([1100.0, 720.0]).with_min_inner_size([480.0, 360.0]);
    let id = egui::ViewportId::from_hash_of("freedawm-remote-mix");
    ctx.show_viewport_immediate(id, builder, |ui, _class| {
        ui.set_style(ui.ctx().global_style());
        let Some(mut net) = app.net.take() else { return };
        contents(app, &mut net, ui);
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            net.studio.window = false;
        }
        app.net = Some(net);
    });
}

fn contents(app: &mut SoundApp, net: &mut NetState, ui: &mut Ui) {
    let t = Tokens::current();
    let s = shown(net);
    let studio = net.studio.studio_name();
    let session = s.view.as_ref().map(|v| v.session.clone()).unwrap_or_default();
    // The banner: whose mixer this is, how the link is doing, the ears, Disconnect.
    egui::Panel::top("remote-mix-banner").frame(egui::Frame::NONE.fill(REMOTE_COLOUR).inner_margin(egui::Margin::symmetric(10, 6))).show(ui, |ui| {
        ui.horizontal(|ui| {
            let head = if session.is_empty() { format!("Mixing {studio} remotely") } else { format!("Mixing {studio} remotely · {session}") };
            ui.label(egui::RichText::new(head).font(bold(15.0)).color(Color32::WHITE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Disconnect").on_hover_text("Stop mixing this studio: its mix stays as it is now").clicked() {
                    net.studio.disconnect();
                }
                let (undo, redo) = s.view.as_ref().map_or((None, None), |v| (v.undo.clone(), v.redo.clone()));
                let redo_text = redo.as_ref().map_or_else(|| "Redo".to_string(), |l| format!("Redo {l}"));
                if ui
                    .add_enabled(s.connected && redo.is_some(), egui::Button::new(redo_text))
                    .on_hover_text("Redo on the studio PC (its own history)")
                    .clicked()
                {
                    ask_or_run(net, Confirm::Redo(redo.clone().unwrap_or_default()));
                }
                let undo_text = undo.as_ref().map_or_else(|| "Undo".to_string(), |l| format!("Undo {l}"));
                if ui
                    .add_enabled(s.connected && undo.is_some(), egui::Button::new(undo_text))
                    .on_hover_text("Undo on the studio PC: it takes back the studio's last change, whoever made it")
                    .clicked()
                {
                    ask_or_run(net, Confirm::Undo(undo.clone().unwrap_or_default()));
                }
                ui.checkbox(&mut net.studio.muted, egui::RichText::new("Mute").color(Color32::WHITE));
                ui.add(egui::Slider::new(&mut net.studio.volume, 0.0..=2.0).show_value(false))
                    .on_hover_text("How loud the studio's master plays here (it changes nothing at the studio)");
                ui.label(egui::RichText::new("Listen").color(Color32::WHITE));
            });
        });
        ui.horizontal(|ui| {
            let q = if s.connected { s.quality.clone() } else { down_text(&s.state) };
            let col = if !s.connected || s.weak { Color32::from_rgb(255, 214, 120) } else { Color32::from_gray(230) };
            ui.label(egui::RichText::new(q).font(regular(11.0)).color(col));
            if s.connected && s.locked {
                ui.label(egui::RichText::new(LOCKED_NOTE).font(bold(11.0)).color(Color32::from_rgb(255, 214, 120)));
            }
            if s.connected && s.weak {
                ui.label(egui::RichText::new("The connection is weak: changes and the sound may lag.").font(regular(11.0)).color(col));
            }
        });
        let notes: Vec<String> = net.studio.notes.iter().map(|(n, _)| n.clone()).collect();
        for n in notes {
            ui.label(egui::RichText::new(n).font(regular(11.0)).color(Color32::from_rgb(255, 214, 120)));
        }
    });
    let Some(view) = s.view else {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(if s.connected { "Waiting for the studio's mixer…".to_string() } else { down_text(&s.state) })
                    .size(16.0)
                    .color(t.text_dim),
            );
        });
        return;
    };
    let area = ui.available_rect_before_wrap();
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(area));
    if !s.connected || s.locked {
        c.disable();
    }
    strips(net, &mut c, &view);
    // The hand came off: the drag is one undo step on the studio, closed now.
    if net.studio.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
        end_drag(net);
    }
    if !s.connected {
        net.studio.drag = None;
        net.studio.confirm = None;
    }
    // Undo / Redo keys in this window act on the studio (asked the first time), never on this
    // PC's own session (only in a window of its own: embedded, the keys are the main window's).
    if s.connected && ui.ctx().viewport_id() != egui::ViewportId::ROOT {
        let (undo, redo) = ui.input_mut(|i| {
            (i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z), i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z))
        });
        if undo && let Some(l) = view.undo.clone() {
            ask_or_run(net, Confirm::Undo(l));
        } else if redo && let Some(l) = view.redo.clone() {
            ask_or_run(net, Confirm::Redo(l));
        }
    }
    confirm_window(net, ui.ctx());
    crate::studio_editors::window(app, net, &ui.ctx().clone(), &view, s.connected);
    if !s.connected {
        // Greyed, the last state kept visible, saying what it does.
        ui.painter().rect_filled(area, 0.0, t.window_bg.gamma_multiply(0.55));
        let plate = Rect::from_center_size(area.center(), vec2(420.0f32.min(area.width() - 20.0), 56.0));
        ui.painter().rect(plate, 6.0, t.panel_bg, Stroke::new(1.0, REMOTE_COLOUR), egui::StrokeKind::Inside);
        ui.painter().text(plate.center(), Align2::CENTER_CENTER, down_text(&s.state), bold(14.0), t.text);
    }
    ui.ctx().request_repaint_after(Duration::from_millis(50));
}

/// Undo or Redo on the studio: run at once once this connection said yes, else asked first.
fn ask_or_run(net: &mut NetState, c: Confirm) {
    if net.studio.undo_ok {
        run_confirmed(net, &c);
    } else {
        net.studio.confirm = Some(c);
    }
}

fn run_confirmed(net: &mut NetState, c: &Confirm) {
    match c {
        Confirm::Undo(_) => run_on_studio(net, "edit.undo"),
        Confirm::Redo(_) => run_on_studio(net, "edit.redo"),
    }
}

/// The question before the first Undo or Redo on someone else's machine.
fn confirm_window(net: &mut NetState, ctx: &egui::Context) {
    let Some(c) = net.studio.confirm.clone() else { return };
    let studio = net.studio.studio_name();
    let (title, what, button) = match &c {
        Confirm::Undo(l) => ("Undo on the studio?", format!("This takes back “{l}” in {studio}'s session, whoever made that change."), "Undo there"),
        Confirm::Redo(l) => ("Redo on the studio?", format!("This does “{l}” again in {studio}'s session."), "Redo there"),
    };
    let mut answer = None;
    egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
        ui.label(what);
        ui.label(egui::RichText::new("Your own session is not touched. You will not be asked again while connected.").small());
        ui.horizontal(|ui| {
            if ui.button(button).clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            net.studio.undo_ok = true;
            net.studio.confirm = None;
            run_confirmed(net, &c);
        }
        Some(false) => net.studio.confirm = None,
        None => {}
    }
}

/// One frame of the strips' meters: the studio's levels (they carry its ballistics), with this
/// window's peak hold and clip lights.
fn meter_frame(net: &mut NetState, view: &MixerView) {
    let now = Instant::now();
    let dt = net.studio.meters_at.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
    net.studio.meters_at = Some(now);
    let levels: Vec<(u64, soundcraft_net::studio::MeterLevels)> = match (&net.studio.demo, &net.studio.link) {
        (Some(d), _) => d.levels.iter().map(|(k, v)| (*k, *v)).collect(),
        (None, Some(l)) => {
            let m = l.mirror();
            view.strips.iter().filter_map(|s| m.meters(s.id).map(|x| (s.id, x))).collect()
        }
        _ => Vec::new(),
    };
    for (id, l) in levels {
        let md = net.studio.meters.entry(id).or_default();
        md.level = l.peak;
        md.rms = l.rms;
        md.gr = -l.gr_db;
        for c in 0..2 {
            if l.peak[c] >= md.hold[c] {
                md.hold[c] = l.peak[c];
                md.hold_age = 0.0;
            }
        }
        md.hold_age += dt;
        if md.hold_age > 2.0 {
            md.hold = l.peak;
        }
        md.peak = md.peak.max(l.peak[0].max(l.peak[1]));
        md.clip |= l.peak[0] >= 1.0 || l.peak[1] >= 1.0;
    }
}

const STRIP_W: f32 = 96.0;

fn strips(net: &mut NetState, ui: &mut Ui, view: &MixerView) {
    meter_frame(net, view);
    let t = Tokens::current();
    // Groups and VCA masters, in one line above the strips.
    if !view.groups.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Groups:").small().color(t.text_dim));
            for g in &view.groups {
                let text = format!("{} {}{}", g.letter, g.name, if g.active { "" } else { " (off)" });
                ui.label(egui::RichText::new(text).small().color(rgb(g.color)));
            }
        });
    }
    egui::ScrollArea::horizontal().id_salt("remote-mix-strips").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for s in view.strips.iter().filter(|s| !s.hidden) {
                strip(net, ui, view, s);
            }
        });
    });
}

fn strip(net: &mut NetState, ui: &mut Ui, view: &MixerView, s: &RemoteStrip) {
    let t = Tokens::current();
    let h = ui.available_height().max(480.0);
    let (r, _) = ui.allocate_exact_size(vec2(STRIP_W, h), Sense::hover());
    crate::widgets::vgradient(ui.painter(), r, crate::widgets::shade(t.strip_bg, 0.06), t.strip_bg);
    ui.painter().vline(r.min.x + 0.5, r.y_range(), Stroke::new(1.0, t.edge_light));
    // The strip's colour along its top, as the studio's Mix window shows it.
    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(STRIP_W, 4.0)), 0.0, rgb(s.color));
    let x0 = r.min.x + 4.0;
    let w = STRIP_W - 8.0;
    let mut y = r.min.y + 8.0;
    // Inserts: name and bypass.
    let sec_h = 14.0 + 16.0 * s.inserts.len().max(1) as f32;
    let sec = Rect::from_min_size(pos2(x0, y), vec2(w, sec_h));
    crate::widgets::inset(ui.painter(), sec, t.strip_section);
    ui.painter().text(pos2(sec.min.x + 3.0, sec.min.y + 1.0), Align2::LEFT_TOP, "INSERTS", regular(8.0), t.text_dim);
    for (k, ins) in s.inserts.iter().enumerate() {
        let ir = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 13.0 + 16.0 * k as f32), vec2(w - 4.0, 15.0));
        match insert_row(ui, s, ins, ir) {
            Some(InsertClick::Bypass(on)) => act(
                net,
                Control::Bypass(s.id, ins.module),
                if on { 1.0 } else { 0.0 },
                false,
                "mix.insert_bypass",
                json!({"track": s.id, "slot": ins.slot, "value": on}),
            ),
            Some(InsertClick::Open) => crate::studio_editors::open(net, s, ins),
            None => {}
        }
    }
    y = sec.max.y + 4.0;
    // Sends: where to and how loud.
    let sec_h = 14.0 + 16.0 * s.sends.len().max(1) as f32;
    let sec = Rect::from_min_size(pos2(x0, y), vec2(w, sec_h));
    crate::widgets::inset(ui.painter(), sec, t.strip_section);
    ui.painter().text(pos2(sec.min.x + 3.0, sec.min.y + 1.0), Align2::LEFT_TOP, "SENDS", regular(8.0), t.text_dim);
    for (k, x) in s.sends.iter().enumerate() {
        let sr = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 13.0 + 16.0 * k as f32), vec2(w - 4.0, 15.0));
        let resp = ui.interact(sr, ui.id().with(("remote-send", s.id, x.slot)), Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, ui.is_enabled(), format!("Send {} to {}", s.name, x.target)));
        let fill = if resp.hovered() || resp.dragged() { t.slot_bg.gamma_multiply(1.3) } else { t.slot_bg };
        ui.painter().rect_filled(sr, 2.0, fill);
        // The level as a bar along the row (0 dB at about four fifths).
        let pos = soundcraft_model::fader_db_to_pos(x.level_db).clamp(0.0, 1.0);
        ui.painter().rect_filled(Rect::from_min_size(sr.min, vec2(sr.width() * pos, sr.height())), 2.0, t.accent.gamma_multiply(0.35));
        let text = format!("{} {}", x.target, crate::widgets::db_text(x.level_db));
        let col = if x.mute { t.text_dim } else { t.text };
        ui.painter().with_clip_rect(sr).text(sr.left_center() + vec2(2.0, 0.0), Align2::LEFT_CENTER, text, regular(9.5), col);
        let resp = resp.on_hover_text("Drag left or right: the send's level on the studio (double-click: 0 dB)");
        let c = Control::Send(s.id, x.slot);
        if resp.double_clicked() {
            act(net, c, 0.0, false, "mix.send_level", json!({"track": s.id, "slot": x.slot, "db": 0.0}));
        } else if resp.dragged() && resp.drag_delta().x != 0.0 {
            let fine = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
            let p = (pos + resp.drag_delta().x / sr.width() * if fine { 0.1 } else { 1.0 }).clamp(0.0, 1.0);
            let db = soundcraft_model::fader_pos_to_db(p).clamp(-144.0, 12.0);
            act(net, c, db, true, "mix.send_level", json!({"track": s.id, "slot": x.slot, "db": db}));
        }
    }
    y = sec.max.y + 4.0;
    // Groups and VCA.
    let mut tags: Vec<String> = s.groups.iter().filter_map(|g| view.groups.iter().find(|x| x.id == *g)).map(|g| g.letter.clone()).collect();
    if let Some(v) = s.vca.and_then(|v| view.strip(v)) {
        tags.push(format!("VCA {}", v.name));
    }
    let tag = if tags.is_empty() { "no group".to_string() } else { tags.join(" ") };
    ui.painter().text(pos2(r.center().x, y + 6.0), Align2::CENTER_CENTER, tag, regular(9.0), t.text_dim);
    y += 16.0;
    // Pan.
    if !s.pan.is_empty() && s.kind != "vca" {
        let n = s.pan.len().min(2);
        let ks = if n == 2 { (w / 2.0 - 6.0).min(30.0) } else { 32.0 };
        for i in 0..n {
            let cx = if n == 2 { x0 + w * (0.25 + 0.5 * i as f32) } else { x0 + w * 0.5 };
            let kr = Rect::from_center_size(pos2(cx, y + 16.0), vec2(ks, ks));
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(kr));
            let mut v = s.pan.get(i).copied().unwrap_or(0.0);
            let before = v;
            let resp = crate::widgets::pan_knob(&mut c, ks, &mut v, &format!("Pan {} (drag; double-click centres)", s.name));
            if (v - before).abs() > f32::EPSILON {
                act(net, Control::Pan(s.id, i), v, resp.dragged(), "mix.pan", json!({"tracks": [s.id], "pan": v, "index": i}));
            }
            ui.painter().text(pos2(cx, y + 38.0), Align2::CENTER_CENTER, crate::widgets::pan_text(v), regular(8.5), t.text_dim);
        }
        y += 48.0;
    }
    // Input monitoring (read-only here), then Solo / Mute.
    let bw = (w - 6.0) / 2.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    if matches!(s.kind.as_str(), "audio" | "instrument" | "midi") {
        let state = match s.input_monitor.as_str() {
            "On" => soundcraft_model::InputMonitor::On,
            "Off" => soundcraft_model::InputMonitor::Off,
            _ => soundcraft_model::InputMonitor::Auto,
        };
        row.add_enabled_ui(false, |ui| crate::widgets::input_monitor_key(ui, vec2(bw, 18.0), state, crate::mix_window::input_monitor_tip(state)))
            .inner
            .on_disabled_hover_text("Input monitoring: changed on the studio PC");
    }
    y += 22.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    if s.kind != "master" {
        let look = if s.solo {
            crate::widgets::SoloLook::Soloed
        } else if view.any_solo() {
            crate::widgets::SoloLook::MutedBySolo
        } else {
            crate::widgets::SoloLook::Off
        };
        if crate::widgets::solo_key(&mut row, vec2(bw, 18.0), &look, &format!("Solo {}", s.name)).clicked() {
            let on = !s.solo;
            act(net, Control::Solo(s.id), if on { 1.0 } else { 0.0 }, false, "mix.solo", json!({"tracks": [s.id], "value": on}));
        }
        if crate::widgets::mute_key(&mut row, vec2(bw, 18.0), s.mute, &look, &format!("Mute {}", s.name)).clicked() {
            let on = !s.mute;
            act(net, Control::Mute(s.id), if on { 1.0 } else { 0.0 }, false, "mix.mute", json!({"tracks": [s.id], "value": on}));
        }
    }
    y += 26.0;
    // Fader and meters.
    let bottom_h = 44.0;
    let fh = (r.max.y - bottom_h - y).max(120.0);
    let fr = Rect::from_min_size(pos2(x0, y), vec2(w * 0.6, fh));
    if let Some(db) = crate::widgets::fader(ui, fr, s.volume_db, egui::Id::new(("remote-fader", s.id))) {
        let held = ui.input(|i| i.pointer.any_down());
        act(net, Control::Volume(s.id), db, held, "mix.volume", json!({"tracks": [s.id], "db": db}));
    }
    if s.automation_mode != "off" && !s.automation_mode.is_empty() {
        // The studio's automation moves this fader while it reads: say so.
        ui.painter().text(
            pos2(fr.center().x, fr.min.y - 2.0),
            Align2::CENTER_BOTTOM,
            format!("auto {}", s.automation_mode),
            regular(8.0),
            t.auto_read,
        );
    }
    let md = net.studio.meters.get(&s.id).copied().unwrap_or_default();
    let slot = crate::widgets::fader_slot(fr);
    let mr = Rect::from_min_max(pos2(fr.max.x + 4.0, slot.min.y - 5.0), pos2(x0 + w - 2.0, slot.max.y + 1.0));
    let mw = mr.width() / 2.0 - 1.0;
    crate::widgets::meter_of(ui, Rect::from_min_size(mr.min, vec2(mw, mr.height())), &md, 0);
    crate::widgets::meter_of(ui, Rect::from_min_size(pos2(mr.min.x + mw + 2.0, mr.min.y), vec2(mw, mr.height())), &md, 1);
    if md.gr > 0.1 {
        ui.painter().text(pos2(mr.center().x, mr.min.y - 3.0), Align2::CENTER_BOTTOM, format!("-{:.0}", md.gr), regular(8.0), t.meter_yellow);
    }
    // Volume and name.
    let vr = Rect::from_min_size(pos2(x0 + 4.0, r.max.y - bottom_h + 2.0), vec2(w - 8.0, 15.0));
    crate::widgets::inset(ui.painter(), vr, t.counter_bg);
    ui.painter().text(vr.center(), Align2::CENTER_CENTER, crate::widgets::db_text(s.volume_db), regular(10.0), t.counter_text);
    let nr = Rect::from_min_size(pos2(x0, r.max.y - 24.0), vec2(w, 18.0));
    ui.painter().rect(nr, 2.0, rgb(s.color).gamma_multiply(0.85), Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
    ui.painter().with_clip_rect(nr).text(nr.center(), Align2::CENTER_CENTER, &s.name, bold(11.5), readable_on(s.color));
    if s.inactive {
        ui.painter().rect_filled(Rect::from_min_max(r.min, pos2(r.max.x, nr.min.y - 1.0)), 0.0, t.strip_bg.gamma_multiply(0.7));
        ui.painter().text(pos2(r.center().x, fr.min.y + 24.0), Align2::CENTER_CENTER, "INACTIVE", bold(9.0), t.text_dim);
    }
    ui.painter().line_segment([pos2(r.max.x, r.min.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
}

/// Black or white text, whichever reads on `c`.
fn readable_on(c: [u8; 3]) -> Color32 {
    let l = 0.299 * f32::from(c[0]) + 0.587 * f32::from(c[1]) + 0.114 * f32::from(c[2]);
    if l > 140.0 { Color32::from_gray(20) } else { Color32::WHITE }
}

/// One insert: its name (third-party ones say their window is on the studio) and a bypass key.
/// Returns the bypass to set when the key is clicked.
/// What a click on an insert row asked for.
enum InsertClick {
    Bypass(bool),
    Open,
}

fn insert_row(ui: &mut Ui, s: &RemoteStrip, ins: &soundcraft_net::studio::RemoteInsert, r: Rect) -> Option<InsertClick> {
    let t = Tokens::current();
    let key = Rect::from_min_max(pos2(r.max.x - 15.0, r.min.y), r.max);
    let body = Rect::from_min_max(r.min, pos2(key.min.x - 1.0, r.max.y));
    let opens = crate::studio_editors::kind_of(ins);
    let resp = ui.interact(body, ui.id().with(("remote-insert", s.id, ins.module)), if opens.is_some() { Sense::click() } else { Sense::hover() });
    let label = format!("{} on {}", ins.name, s.name);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label));
    let fill = if ins.bypass {
        t.slot_bg.gamma_multiply(0.6)
    } else if resp.hovered() && opens.is_some() {
        t.slot_bg.gamma_multiply(1.3)
    } else {
        t.slot_bg
    };
    ui.painter().rect_filled(body, 2.0, fill);
    let col = if ins.bypass { t.text_dim } else { t.text };
    let name = if ins.hosted { format!("{} ⧉", ins.name) } else { ins.name.clone() };
    ui.painter().with_clip_rect(body.shrink(1.0)).text(body.left_center() + vec2(3.0, 0.0), Align2::LEFT_CENTER, name, regular(9.5), col);
    let resp = match opens {
        _ if ins.hosted => resp.on_hover_text(HOSTED_NOTE),
        Some(crate::studio_editors::EditorKind::Full) => resp.on_hover_text("Click: its editor, working on the studio's insert"),
        Some(crate::studio_editors::EditorKind::Generic) => resp.on_hover_text("Click: its parameters, working on the studio's insert"),
        None => resp,
    };
    let open = resp.clicked();
    // The bypass key: lit while the insert plays.
    let kresp = ui.interact(key, ui.id().with(("remote-bypass", s.id, ins.module)), Sense::click());
    let label = format!("Bypass {} on {}", ins.name, s.name);
    kresp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), ins.bypass, &label));
    let on = !ins.bypass;
    ui.painter().rect_filled(key, 2.0, if kresp.hovered() { t.button_hi } else { t.button });
    ui.painter().circle_stroke(key.center(), 4.0, Stroke::new(1.5, if on { t.meter_green } else { t.text_dim }));
    ui.painter().vline(key.center().x, key.center().y - 5.5..=key.center().y - 1.0, Stroke::new(1.5, if on { t.meter_green } else { t.text_dim }));
    let kresp =
        kresp.on_hover_text(if ins.bypass { "Bypassed on the studio: click to play it" } else { "Playing on the studio: click to bypass it" });
    if kresp.clicked() {
        Some(InsertClick::Bypass(!ins.bypass))
    } else if open {
        Some(InsertClick::Open)
    } else {
        None
    }
}

/// Opens the editor of insert `module` on the studio's strip `strip` (as a click on its name
/// does; for tests and renders).
pub fn open_editor(app: &mut SoundApp, strip: u64, module: u32) -> Result<(), String> {
    let net = app.net.as_mut().ok_or("no network state")?;
    let view = match (&net.studio.demo, &net.studio.link) {
        (Some(d), _) => Some(d.view.clone()),
        (None, Some(l)) => l.mirror().feed_view().cloned(),
        _ => None,
    }
    .ok_or("not mixing a studio")?;
    let s = view.strip(strip).ok_or("no such strip")?;
    let ins = s.inserts.iter().find(|i| i.module == module).ok_or("no such insert")?;
    crate::studio_editors::open(net, s, ins);
    if net.studio.editor.is_none() {
        return Err("that insert does not open here".into());
    }
    Ok(())
}

/// Connects this PC to `studio` with its kept pairing and opens the Remote Mix window (as the
/// dialog's Reconnect does).
pub fn connect_to(app: &mut SoundApp, ctx: &egui::Context, studio: KnownStudio) {
    if let Some(net) = app.net.as_mut() {
        connect_known(net, ctx, studio);
    }
}

/// A small live-stream session as a studio would have it (for renders and tests): five
/// channels, an EQ and a compressor on the vocal, a reverb bus with its return, two snapshots.
pub fn demo_studio_engine() -> soundcraft_engine::Engine {
    let mut e = soundcraft_engine::Engine::default();
    e.session_mut().name = "Sunday Service Live".into();
    for (n, c) in [
        ("Pastor Mic", [214, 92, 74]),
        ("Vocal", [222, 170, 60]),
        ("Guitar", [96, 168, 92]),
        ("Keys", [86, 140, 214]),
        ("Crowd L/R", [150, 110, 200]),
    ] {
        let _ = e.execute("track.new", &json!({"name": n, "kind": "audio"}));
        if let Some(id) = e.session().tracks.iter().find(|t| t.name == n).map(|t| t.id.0) {
            let _ = e.execute("track.color", &json!({"tracks": [id], "color": [c[0], c[1], c[2]]}));
        }
    }
    let _ = e.execute("track.new", &json!({"name": "Master", "kind": "master"}));
    let id = |e: &soundcraft_engine::Engine, n: &str| e.session().tracks.iter().find(|t| t.name == n).map_or(0, |t| t.id.0);
    let v = id(&e, "Vocal");
    let p = id(&e, "Pastor Mic");
    let _ = e.execute("mix.insert", &json!({"track": v, "plugin": "eq_7band"}));
    let _ = e.execute("mix.insert", &json!({"track": v, "plugin": "fs_comp"}));
    let _ = e.execute("mix.insert", &json!({"track": p, "plugin": "fs_comp"}));
    let _ = e.execute("mix.send", &json!({"track": v, "slot": 0, "bus": "Verb", "level_db": -14.0, "return": true}));
    let _ = e.execute("mix.send", &json!({"track": id(&e, "Guitar"), "slot": 0, "bus": "Verb", "level_db": -20.0}));
    let _ = e.execute("mix.volume", &json!({"tracks": [v], "db": -3.5}));
    let _ = e.execute("mix.volume", &json!({"tracks": [id(&e, "Crowd L/R")], "db": -12.0}));
    let _ = e.execute("mix.pan", &json!({"tracks": [id(&e, "Guitar")], "pan": -0.4}));
    let _ = e.execute("mix.pan", &json!({"tracks": [id(&e, "Keys")], "pan": 0.35}));
    let _ = e.execute("mix.mute", &json!({"tracks": [id(&e, "Crowd L/R")], "value": true}));
    let _ = e.execute("mix.input_monitor", &json!({"tracks": [p, v], "value": "on"}));
    let _ = e.execute("mix.snapshot_store", &json!({"name": "Worship"}));
    let _ = e.execute("mix.snapshot_store", &json!({"name": "Sermon"}));
    e
}

/// For renders (`ui_shot` `{"remote_mix_demo": state}`) and UI tests: the Remote Mix window as
/// it is mixing [`demo_studio_engine`] (`connected`), greyed while the link is down
/// (`reconnecting`), or the dialog with a studio mixed before (`dialog`). No network is used.
pub fn demo(app: &mut SoundApp, state: &str) -> Result<(), String> {
    let e = demo_studio_engine();
    let mut m = soundcraft_net::studio::MixMirror::default();
    m.on_event("mixer.snapshot", &json!({"seq": 1, "state": soundcraft_engine::surface::feed::mixer_state(&e)}));
    let view = m.view().ok_or("no mixer")?;
    let levels = view
        .strips
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let l = if s.mute { 0.0 } else { 0.18 + 0.11 * (k % 4) as f32 };
            (s.id, soundcraft_net::studio::MeterLevels { peak: [l, l * 0.92], rms: [l * 0.45, l * 0.42], gr_db: if k == 1 { -3.0 } else { 0.0 } })
        })
        .collect();
    let net = app.net.as_mut().ok_or("no network state (run a frame first)")?;
    net.studio.disconnect();
    match state {
        "connected" | "reconnecting" | "locked" => {
            net.studio.current = Some(KnownStudio { name: "Studio PC".into(), ..KnownStudio::default() });
            net.studio.demo =
                Some(DemoLink { connected: state != "reconnecting", locked: state == "locked", view, levels, rtt_ms: 38.0, buffer_ms: 150.0 });
            net.studio.window = true;
            if state == "reconnecting" {
                net.studio.note("2 changes were not sent: the link to the studio was down.".into());
            }
        }
        "dialog" => {
            net.hub.update_config(|c| {
                c.studios = vec![KnownStudio { name: "Studio PC".into(), internet: "{}".into(), token: "demo".into(), ..KnownStudio::default() }];
            });
            app.ui.show_connect_studio = true;
        }
        other => return Err(format!("unknown remote mix demo `{other}`")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_reads_invites_addresses_and_codes() {
        assert_eq!(read_entry("  "), Entry::Empty);
        assert_eq!(read_entry("freedawm-invite:abc"), Entry::Invite);
        assert_eq!(read_entry("192.168.1.20:48100"), Entry::Address("192.168.1.20:48100".into()));
        assert_eq!(read_entry("4821"), Entry::Code("4821".into()));
        assert_eq!(read_entry("sunday-stream"), Entry::Code("sunday-stream".into()));
    }

    #[test]
    fn the_down_text_says_what_happens() {
        assert_eq!(down_text("reconnecting"), "Reconnecting…");
        assert!(down_text("not paired").contains("pair again"));
        assert!(down_text("sent away").contains("disconnected"));
    }
}
