//! Setup › Control Surfaces: hardware mixers (Mackie Control and its extenders, HUI) on MIDI
//! ports, driving the mix and shown it back (`soundcraft_engine::surface`).
//!
//! The list is an app setting (`ui.json`: `surfaces`), per machine as the hardware is: each row a
//! protocol, an input port and an output port. A Mackie Control Extender row adds eight strips to
//! the Mackie Control above it. A surface's ports are its own: MIDI input closes them (what a
//! surface sends is never a note on a track, in a take or through Thru) and MIDI output does not
//! offer them to tracks.
//!
//! Every frame ([`tick`]) the surfaces' messages are applied to the engine and what changed is
//! sent back: motor faders, LEDs, scribble strips, meters (from the Mix meters).

use crate::SoundApp;
use crate::theme::Tokens;
use soundcraft_engine::surface::{self, SurfaceGroup};
use soundcraft_playback::midi_in::MidiConnection;
use soundcraft_playback::midi_out::MidiSend;
use std::sync::mpsc::{Receiver, Sender};

/// One configured surface unit (a row of the window).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SurfaceConfig {
    /// `mackie`, `mackie_xt` or `hui` ([`surface::PROTOCOLS`]).
    pub protocol: String,
    /// The MIDI input port it sends on.
    #[serde(default)]
    pub input: String,
    /// The MIDI output port it listens on.
    #[serde(default)]
    pub output: String,
}

/// How a surface's ports are opened: the system's, or a test's.
pub trait SurfaceIo {
    /// Opens input `name`: its messages arrive on the receiver; dropping the box closes it.
    fn open_in(&mut self, name: &str) -> Result<(Box<dyn MidiConnection>, Receiver<Vec<u8>>), String>;
    /// Opens output `name`.
    fn open_out(&mut self, name: &str) -> Result<Box<dyn MidiSend>, String>;
}

/// The system's ports (`midir`).
pub struct SystemIo;

impl SurfaceIo for SystemIo {
    fn open_in(&mut self, name: &str) -> Result<(Box<dyn MidiConnection>, Receiver<Vec<u8>>), String> {
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = std::sync::mpsc::channel();
        let c = soundcraft_playback::midi_in::open_raw_input(name, tx)?;
        Ok((c, rx))
    }

    fn open_out(&mut self, name: &str) -> Result<Box<dyn MidiSend>, String> {
        soundcraft_playback::midi_out::open_system_port(name)
    }
}

/// One running unit: its ports.
struct UnitIo {
    row: usize,
    _conn: Option<Box<dyn MidiConnection>>,
    rx: Option<Receiver<Vec<u8>>>,
    out: Option<Box<dyn MidiSend>>,
}

/// One running surface and its extenders.
struct Running {
    group: SurfaceGroup,
    units: Vec<UnitIo>,
}

/// The running surfaces (not saved), made again when the list changes.
#[derive(Default)]
pub struct Surfaces {
    built_for: Option<Vec<SurfaceConfig>>,
    running: Vec<Running>,
    /// Why each row's ports could not open, by row.
    pub errors: Vec<Option<String>>,
    /// The ports to open them with (`None`: the system's, once the host asks for them).
    pub io: Option<Box<dyn SurfaceIo>>,
    started: Option<std::time::Instant>,
}

/// Groups the rows: each Mackie Control Extender joins the Mackie Control before it; anything
/// else starts a group. A row without a known protocol is left out.
fn groups(rows: &[SurfaceConfig]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if surface::protocol(&r.protocol).is_none() {
            continue;
        }
        let joins = r.protocol == "mackie_xt"
            && out.last().and_then(|g| g.first()).and_then(|f| rows.get(*f)).is_some_and(|f| f.protocol.starts_with("mackie"));
        match out.last_mut() {
            Some(g) if joins => g.push(i),
            _ => out.push(vec![i]),
        }
    }
    out
}

impl Surfaces {
    fn build(&mut self, rows: &[SurfaceConfig]) {
        self.running.clear();
        self.errors = vec![None; rows.len()];
        let Some(io) = self.io.as_mut() else {
            self.built_for = Some(rows.to_vec());
            return;
        };
        for g in groups(rows) {
            let protocols = g.iter().filter_map(|i| rows.get(*i)).filter_map(|r| surface::protocol(&r.protocol)).collect();
            let mut units = Vec::new();
            for &row in &g {
                let Some(r) = rows.get(row) else { continue };
                let mut errs = Vec::new();
                let (conn, rx) = if r.input.is_empty() {
                    (None, None)
                } else {
                    match io.open_in(&r.input) {
                        Ok((c, rx)) => (Some(c), Some(rx)),
                        Err(e) => {
                            errs.push(format!("input: {e}"));
                            (None, None)
                        }
                    }
                };
                let out = if r.output.is_empty() {
                    None
                } else {
                    match io.open_out(&r.output) {
                        Ok(o) => Some(o),
                        Err(e) => {
                            errs.push(format!("output: {e}"));
                            None
                        }
                    }
                };
                if let Some(slot) = self.errors.get_mut(row) {
                    *slot = (!errs.is_empty()).then(|| errs.join("; "));
                }
                units.push(UnitIo { row, _conn: conn, rx, out });
            }
            self.running.push(Running { group: SurfaceGroup::new(protocols), units });
        }
        self.built_for = Some(rows.to_vec());
    }

    /// The row's group and unit, for a test or the window.
    pub fn bank_of(&self, row: usize) -> Option<usize> {
        self.running.iter().find(|r| r.units.iter().any(|u| u.row == row)).map(|r| r.group.bank())
    }
}

/// The ports the surfaces use (kept from MIDI input and output).
pub fn reserved_ports(app: &SoundApp) -> (Vec<String>, Vec<String>) {
    let ins = app.ui.surfaces.iter().map(|s| s.input.clone()).filter(|n| !n.is_empty()).collect();
    let outs = app.ui.surfaces.iter().map(|s| s.output.clone()).filter(|n| !n.is_empty()).collect();
    (ins, outs)
}

/// Every frame (from `logic`): the surfaces follow the list, what they sent moves the mix, and
/// what changed is shown on them.
pub fn tick(app: &mut SoundApp) {
    if app.surfaces.io.is_none() && app.use_system_midi {
        app.surfaces.io = Some(Box::new(SystemIo));
    }
    let (ins, outs) = reserved_ports(app);
    if let Some(m) = &mut app.midi_inputs {
        m.set_reserved(ins);
    }
    if let Some(m) = &mut app.midi_outputs {
        m.set_reserved(outs);
    }
    if app.surfaces.built_for.as_ref() != Some(&app.ui.surfaces) {
        let rows = app.ui.surfaces.clone();
        app.surfaces.build(&rows);
    }
    if app.surfaces.running.is_empty() {
        return;
    }
    let started = *app.surfaces.started.get_or_insert_with(std::time::Instant::now);
    let now_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut running = std::mem::take(&mut app.surfaces.running);
    for r in &mut running {
        for (k, u) in r.units.iter_mut().enumerate() {
            let Some(rx) = &u.rx else { continue };
            // At most a few hundred messages a frame: a stuck device cannot stall the UI.
            for msg in rx.try_iter().take(512) {
                for reply in r.group.input(&mut app.engine, k, &msg) {
                    if let Some(o) = u.out.as_mut() {
                        o.send(&reply);
                    }
                }
            }
        }
        let meters = &app.meters;
        let meter = |t: soundcraft_model::TrackId| meters.get(&t).map_or(0.0, |m| m.level[0].max(m.level[1]));
        for (k, msg) in r.group.output(&app.engine, now_ms, &meter) {
            if let Some(o) = r.units.get_mut(k).and_then(|u| u.out.as_mut()) {
                o.send(&msg);
            }
        }
    }
    app.surfaces.running = running;
}

/// Setup › Control Surfaces…: add, set up and remove surfaces.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_surfaces;
    if !open {
        return;
    }
    let t = Tokens::current();
    let in_ports: Vec<String> = app.midi_inputs.as_ref().map(|m| m.ports().into_iter().map(|p| p.name).collect()).unwrap_or_default();
    let out_ports: Vec<String> = app.midi_outputs.as_ref().map(|m| m.ports().into_iter().map(|p| p.name).collect()).unwrap_or_default();
    egui::Window::new("Control Surfaces").open(&mut open).default_size(egui::vec2(560.0, 260.0)).resizable(true).show(ctx, |ui| {
        if app.ui.surfaces.is_empty() {
            ui.label("No control surfaces.");
            ui.label(
                egui::RichText::new("Add one, pick its protocol and the MIDI ports it is on (set the device to Mackie Control or HUI mode).")
                    .small()
                    .color(t.text_dim),
            );
        }
        let mut remove = None;
        let mut rows = app.ui.surfaces.clone();
        egui::Grid::new("surfaces").num_columns(5).spacing(egui::vec2(10.0, 6.0)).striped(true).show(ui, |ui| {
            ui.strong("Protocol");
            ui.strong("Input");
            ui.strong("Output");
            ui.strong("");
            ui.strong("");
            ui.end_row();
            for (i, r) in rows.iter_mut().enumerate() {
                let label = surface::PROTOCOLS.iter().find(|(id, _)| *id == r.protocol).map_or(r.protocol.as_str(), |(_, l)| l);
                egui::ComboBox::from_id_salt(("surface-protocol", i)).selected_text(label).show_ui(ui, |ui| {
                    for (id, l) in surface::PROTOCOLS {
                        ui.selectable_value(&mut r.protocol, id.to_string(), l);
                    }
                });
                port_combo(ui, ("surface-in", i), &mut r.input, &in_ports);
                port_combo(ui, ("surface-out", i), &mut r.output, &out_ports);
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
                let state = match app.surfaces.errors.get(i).cloned().flatten() {
                    Some(e) => format!("Could not open {e}"),
                    None if r.input.is_empty() || r.output.is_empty() => "Pick both ports".into(),
                    None => "Connected".into(),
                };
                ui.label(egui::RichText::new(state).small().color(t.text_dim));
                ui.end_row();
            }
        });
        if let Some(i) = remove
            && i < rows.len()
        {
            rows.remove(i);
        }
        if ui.button("Add Surface").clicked() {
            rows.push(SurfaceConfig { protocol: "mackie".into(), ..Default::default() });
        }
        ui.label(
            egui::RichText::new(
                "A Mackie Control Extender adds eight strips to the Mackie Control above it. A surface's ports are its own: its messages are never recorded or played as notes.",
            )
            .small()
            .color(t.text_dim),
        );
        app.ui.surfaces = rows;
    });
    app.ui.show_surfaces = open;
}

fn port_combo(ui: &mut egui::Ui, id: (&str, usize), value: &mut String, ports: &[String]) {
    let shown = if value.is_empty() { "(none)".to_string() } else { value.clone() };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(170.0).show_ui(ui, |ui| {
        ui.selectable_value(value, String::new(), "(none)");
        for p in ports {
            ui.selectable_value(value, p.clone(), p);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_playback::midi_out::FakeOutBackend;
    use std::sync::{Arc, Mutex};

    /// Ports a test plays into, and a fake output backend that keeps what was sent.
    struct TestIo {
        inputs: Arc<Mutex<Vec<(String, Sender<Vec<u8>>)>>>,
        outs: FakeOutBackend,
    }

    struct NoConn;
    impl MidiConnection for NoConn {}

    impl SurfaceIo for TestIo {
        fn open_in(&mut self, name: &str) -> Result<(Box<dyn MidiConnection>, Receiver<Vec<u8>>), String> {
            let (tx, rx) = std::sync::mpsc::channel();
            self.inputs.lock().unwrap().push((name.to_string(), tx));
            Ok((Box::new(NoConn), rx))
        }

        fn open_out(&mut self, name: &str) -> Result<Box<dyn MidiSend>, String> {
            Ok(self.outs.open_send(name))
        }
    }

    #[test]
    fn a_surface_added_in_the_window_moves_the_mix_and_lights_up() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let kick = app.engine.execute("track.new", &json!({"kind": "audio", "name": "Kick"})).unwrap()["tracks"][0].as_u64().unwrap();
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let outs = FakeOutBackend::default();
        app.surfaces.io = Some(Box::new(TestIo { inputs: Arc::clone(&inputs), outs: outs.clone() }));
        let keys = crate::midi_devices::tests::with_fake(&mut app, &["MCU In", "USB Keys"]);
        app.ui.surfaces = vec![SurfaceConfig { protocol: "mackie".into(), input: "MCU In".into(), output: "MCU Out".into() }];
        tick(&mut app);
        assert!(!keys.is_open("MCU In"), "the surface's port is kept from MIDI input");
        assert!(keys.is_open("USB Keys"));
        let sent = outs.sent("MCU Out");
        assert!(sent.iter().any(|m| m.starts_with(&[0xF0, 0, 0, 0x66, 0x14, 0x12, 0x00, b'K', b'i', b'c', b'k'])), "Kick on the LCD");
        // The device mutes strip 1.
        let tx = inputs.lock().unwrap().first().unwrap().1.clone();
        tx.send(vec![0x90, 0x10, 0x7F]).unwrap();
        tick(&mut app);
        assert!(app.engine.session().track(soundcraft_model::TrackId(kick)).unwrap().mixer.mute);
        assert_eq!(outs.sent("MCU Out").last(), Some(&vec![0x90, 0x10, 0x7F]), "its LED lit");
        assert_eq!(serde_json::to_value(&app.ui).unwrap()["surfaces"][0]["protocol"], "mackie");
        // Removed: the port is MIDI input's again.
        app.ui.surfaces.clear();
        tick(&mut app);
        assert!(keys.is_open("MCU In"));
    }

    #[test]
    fn extenders_join_the_mackie_above_them() {
        let row = |p: &str| SurfaceConfig { protocol: p.into(), ..Default::default() };
        assert_eq!(groups(&[row("mackie"), row("mackie_xt"), row("hui"), row("mackie_xt"), row("eucon")]), vec![vec![0, 1], vec![2], vec![3]]);
    }
}
