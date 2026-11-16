//! `ClapPlugin`: a loaded CLAP instance behind the safe `soundcraft_dsp::Plugin` trait.

use crate::ffi::{EVENT_CAP, Event, GuiLink, Instance, NotePort, PortBuffers};
use clap_sys::events::{
    CLAP_NOTE_EXPRESSION_BRIGHTNESS, CLAP_NOTE_EXPRESSION_EXPRESSION, CLAP_NOTE_EXPRESSION_PAN, CLAP_NOTE_EXPRESSION_PRESSURE,
    CLAP_NOTE_EXPRESSION_TUNING, CLAP_NOTE_EXPRESSION_VIBRATO, CLAP_NOTE_EXPRESSION_VOLUME,
};
use clap_sys::ext::note_ports::{CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI};
use soundcraft_dsp::{Plugin, PluginEditor, PluginInfo, Transport};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Largest block we activate a plugin for.
const MAX_BLOCK: usize = 1 << 16;
/// Most channels a host strip may ask for.
const MAX_CHANNELS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
enum NoteMode {
    None,
    Clap,
    Midi,
}

/// A hosted CLAP plugin. Parameter ids are the CLAP param ids as decimal strings ("0", "17", …);
/// their human names are in `info().params`.
pub struct ClapPlugin {
    inst: Instance,
    info: &'static PluginInfo,
    /// CLAP id of each `info.params` entry (same order).
    ids: Vec<u32>,
    values: Vec<f32>,
    /// Pending events, kept sorted by time (stable for equal times). Never grows past `EVENT_CAP`.
    queue: Vec<Event>,
    inputs: PortBuffers,
    outputs: PortBuffers,
    in_main: Option<usize>,
    out_main: Option<usize>,
    /// The sidechain input: the first input port after the main one with channels (see
    /// [`key_port`]); fed by `process_with_sidechain`, silent otherwise.
    in_key: Option<usize>,
    notes: NoteMode,
    /// The note port takes raw MIDI (controllers, pitch bend, program changes).
    midi_in: bool,
    /// Held notes: a bitmask of MIDI channels per key.
    held: [u16; 128],
    /// For a plug-in that takes only CLAP notes: each channel's last value of each note
    /// expression its controllers set (NaN: none yet), sent again with every new note.
    expr: [[f64; EXPRESSIONS]; 16],
    channels: usize,
    max_block: usize,
    latency: usize,
    steady: i64,
    transport: Option<Transport>,
    /// Set for an offline render (`clap.render`) at the next `prepare`.
    offline: bool,
}

impl ClapPlugin {
    /// A parameter's value as the plugin holds it now (`clap_plugin_params.get_value`, main
    /// thread), not the host's last-set copy; hidden and read-only parameters included. For
    /// diagnostics and tests.
    pub fn live_value(&self, id: &str) -> Option<f32> {
        self.inst.param_value(id.parse::<u32>().ok()?).map(|v| v as f32)
    }

    pub(crate) fn new(inst: Instance, info: &'static PluginInfo) -> ClapPlugin {
        let ids: Vec<u32> = info.params.iter().map(|p| p.id.parse::<u32>().unwrap_or(u32::MAX)).collect();
        let values = info.params.iter().zip(&ids).map(|(p, id)| inst.param_value(*id).map_or(p.default, |v| p.clamp(v as f32))).collect();
        ClapPlugin {
            inst,
            info,
            ids,
            values,
            queue: Vec::with_capacity(EVENT_CAP),
            inputs: PortBuffers::new(&[], 0),
            outputs: PortBuffers::new(&[], 0),
            in_main: None,
            out_main: None,
            in_key: None,
            notes: NoteMode::None,
            midi_in: false,
            held: [0; 128],
            expr: [[f64::NAN; EXPRESSIONS]; 16],
            channels: 2,
            max_block: 0,
            latency: 0,
            steady: 0,
            transport: None,
            offline: false,
        }
    }

    /// Re-reads every parameter value from the plugin (after a state load).
    fn refresh_values(&mut self) {
        for ((v, p), id) in self.values.iter_mut().zip(self.info.params).zip(&self.ids) {
            if let Some(x) = self.inst.param_value(*id) {
                *v = p.clamp(x as f32);
            }
        }
        // Pending changes from before the load would undo it.
        self.queue.retain(|e| !matches!(e, Event::Param { .. }));
    }

    /// Inserts keeping the queue time-sorted; drops the event when the queue is full (never
    /// allocates on the audio thread).
    fn push(&mut self, e: Event) {
        if self.queue.len() >= self.queue.capacity() {
            return;
        }
        let pos = self.queue.iter().rposition(|q| q.time() <= e.time()).map_or(0, |i| i + 1);
        self.queue.insert(pos, e);
    }

    /// A note-on on MIDI channel `ch` (0–15): a CLAP note event on that channel, or the MIDI
    /// bytes for a MIDI-only note port.
    fn key_on(&mut self, offset: usize, ch: u8, note: u8, velocity: u8) {
        if note > 127 {
            return;
        }
        let ch = ch & 0x0F;
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        let vel = velocity.min(127);
        match self.notes {
            NoteMode::Clap => {
                self.push(Event::NoteOn { time, channel: ch, key: note, velocity: f64::from(vel) / 127.0 });
                // The channel's bend, pressure and controllers so far apply to the new note too.
                let state = self.expr.get(usize::from(ch)).copied().unwrap_or([f64::NAN; EXPRESSIONS]);
                for (expression, value) in (0i32..).zip(state) {
                    if value.is_finite() {
                        self.push(Event::Expression { time, expression, channel: ch, key: i16::from(note), value });
                    }
                }
            }
            NoteMode::Midi => self.push(Event::Midi { time, data: [0x90 | ch, note, vel] }),
            NoteMode::None => return,
        }
        if let Some(h) = self.held.get_mut(note as usize) {
            *h |= 1 << ch;
        }
    }

    /// A note-off on MIDI channel `ch` (0–15), with its release velocity for a MIDI note port.
    fn key_off(&mut self, offset: usize, ch: u8, note: u8, release: u8) {
        if note > 127 {
            return;
        }
        let ch = ch & 0x0F;
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        match self.notes {
            NoteMode::Clap => self.push(Event::NoteOff { time, channel: ch, key: note, velocity: f64::from(release.min(127)) / 127.0 }),
            NoteMode::Midi => self.push(Event::Midi { time, data: [0x80 | ch, note, release & 0x7F] }),
            NoteMode::None => return,
        }
        if let Some(h) = self.held.get_mut(note as usize) {
            *h &= !(1 << ch);
        }
    }

    /// One `process` call (split into `max_block` pieces): the main input from `io`, the key (when
    /// the plugin has a key port) from `key`, silent without one; the main output back to `io`.
    fn run(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        if !self.inst.is_active() || self.max_block == 0 {
            return;
        }
        let ch = self.channels.min(io.len());
        let frames = io.iter().take(ch).map(Vec::len).fold(frames, usize::min);
        if frames == 0 || ch == 0 {
            return;
        }
        let last = u32::try_from(frames - 1).unwrap_or(u32::MAX);
        for e in &mut self.queue {
            if e.time() > last {
                e.set_time(last);
            }
        }
        let mut start = 0usize;
        while start < frames {
            let n = (frames - start).min(self.max_block);
            let end = u32::try_from(start + n).unwrap_or(u32::MAX);
            // Feed the main input port (mono sources fill every port channel).
            if let Some(p) = self.in_main {
                for c in 0..self.inputs.channels(p) {
                    let src = io.get(c.min(ch - 1)).and_then(|s| s.get(start..start + n));
                    if let (Some(src), Some(dst)) = (src, self.inputs.channel_mut(p, c).and_then(|d| d.get_mut(..n))) {
                        dst.copy_from_slice(src);
                    }
                }
            }
            // Feed the key port: key channel `c mod n`, silence without a key.
            if let Some(p) = self.in_key {
                for c in 0..self.inputs.channels(p) {
                    let Some(dst) = self.inputs.channel_mut(p, c).and_then(|d| d.get_mut(..n)) else { continue };
                    let kn = key.map_or(0, <[Vec<f32>]>::len);
                    match key.and_then(|k| k.get(c % kn.max(1))).and_then(|s| s.get(start..start + n)) {
                        Some(src) => dst.copy_from_slice(src),
                        None => dst.fill(0.0),
                    }
                }
            }
            let k = self.queue.iter().position(|e| e.time() >= end).unwrap_or(self.queue.len());
            let offset = u32::try_from(start).unwrap_or(u32::MAX);
            let events = self.queue.get(..k).unwrap_or(&[]);
            let r = self.inst.process(n, self.steady, &mut self.inputs, &mut self.outputs, events, offset, self.transport.as_ref());
            self.queue.drain(..k);
            self.steady = self.steady.saturating_add(i64::try_from(n).unwrap_or(0));
            match (r, self.out_main) {
                (Ok(_), Some(p)) => {
                    let oc = self.outputs.channels(p);
                    for (c, dst) in io.iter_mut().take(ch).enumerate() {
                        let Some(dst) = dst.get_mut(start..start + n) else { continue };
                        if ch == 1 && oc >= 2 {
                            if let (Some(l), Some(r)) = (self.outputs.channel(p, 0), self.outputs.channel(p, 1)) {
                                for ((d, a), b) in dst.iter_mut().zip(l).zip(r) {
                                    *d = 0.5 * (a + b);
                                }
                            }
                        } else if let Some(src) = self.outputs.channel(p, c.min(oc.saturating_sub(1))).and_then(|s| s.get(..n)) {
                            dst.copy_from_slice(src);
                        }
                    }
                }
                (Err(e), _) => log::debug!("{}: {e}", self.info.id),
                _ => {}
            }
            start += n;
        }
    }
}

/// How many CLAP note expressions there are (`CLAP_NOTE_EXPRESSION_VOLUME` … `PRESSURE`).
const EXPRESSIONS: usize = 7;

/// The CLAP note expression a MIDI channel message stands for, for a plug-in that takes only
/// CLAP notes: (expression, key: -1 for every key on the channel, value). Pitch bend is tuning
/// (± 2 semitones), channel and poly pressure are pressure, CC1 vibrato, CC7 volume (amplitude,
/// the General MIDI curve), CC10 pan, CC11 expression, CC74 brightness. Anything else has none.
pub(crate) fn expression_of(msg: [u8; 3]) -> Option<(i32, i16, f64)> {
    let [status, d1, d2] = msg;
    let unit = |v: u8| f64::from(v.min(127)) / 127.0;
    match status & 0xF0 {
        0xE0 => {
            let bend = (i32::from(d1 & 0x7F) | (i32::from(d2 & 0x7F) << 7)) - 8192;
            Some((CLAP_NOTE_EXPRESSION_TUNING, -1, f64::from(bend) / 8192.0 * 2.0))
        }
        0xD0 => Some((CLAP_NOTE_EXPRESSION_PRESSURE, -1, unit(d1))),
        0xA0 => Some((CLAP_NOTE_EXPRESSION_PRESSURE, i16::from(d1 & 0x7F), unit(d2))),
        0xB0 => match d1 {
            1 => Some((CLAP_NOTE_EXPRESSION_VIBRATO, -1, unit(d2))),
            7 => Some((CLAP_NOTE_EXPRESSION_VOLUME, -1, unit(d2) * unit(d2))),
            10 => Some((CLAP_NOTE_EXPRESSION_PAN, -1, unit(d2))),
            11 => Some((CLAP_NOTE_EXPRESSION_EXPRESSION, -1, unit(d2))),
            74 => Some((CLAP_NOTE_EXPRESSION_BRIGHTNESS, -1, unit(d2))),
            _ => None,
        },
        _ => None,
    }
}

fn pick_main(ports: &[crate::ffi::PortInfo]) -> Option<usize> {
    ports.iter().position(|p| p.main && p.channels > 0).or_else(|| ports.iter().position(|p| p.channels > 0))
}

/// The input port a sidechain key goes to: the first port with channels that is not the main
/// one (CLAP plugins declare their sidechain as a second, non-main input port).
pub(crate) fn key_port(ports: &[crate::ffi::PortInfo]) -> Option<usize> {
    let main = pick_main(ports)?;
    ports.iter().enumerate().position(|(i, p)| i != main && p.channels > 0)
}

fn note_mode(p: Option<NotePort>) -> NoteMode {
    match p {
        Some(p) if p.supported & CLAP_NOTE_DIALECT_CLAP != 0 => NoteMode::Clap,
        Some(p) if p.supported & CLAP_NOTE_DIALECT_MIDI != 0 => NoteMode::Midi,
        _ => NoteMode::None,
    }
}

impl Plugin for ClapPlugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        let mb = max_block.clamp(1, MAX_BLOCK);
        self.channels = channels.clamp(1, MAX_CHANNELS);
        self.inst.deactivate();
        self.inst.set_render_mode(self.offline);
        self.inst.service_callback();
        let ports = self.inst.audio_ports(true).and_then(|ins| Ok((ins, self.inst.audio_ports(false)?)));
        let (ins, outs) = match ports {
            Ok(p) => p,
            Err(e) => {
                // Unsupported layout: stay inactive (`process` passes audio through untouched).
                log::warn!("{}: {e}", self.info.id);
                self.inputs = PortBuffers::new(&[], 0);
                self.outputs = PortBuffers::new(&[], 0);
                self.in_main = None;
                self.out_main = None;
                self.in_key = None;
                self.max_block = 0;
                self.latency = 0;
                return;
            }
        };
        self.in_main = pick_main(&ins);
        self.out_main = pick_main(&outs);
        self.in_key = key_port(&ins);
        self.inputs = PortBuffers::new(&ins.iter().map(|p| p.channels).collect::<Vec<_>>(), mb);
        self.outputs = PortBuffers::new(&outs.iter().map(|p| p.channels).collect::<Vec<_>>(), mb);
        let port = self.inst.note_port();
        self.notes = note_mode(port);
        self.midi_in = port.is_some_and(|p| p.supported & CLAP_NOTE_DIALECT_MIDI != 0);
        self.max_block = mb;
        self.inst.state().restart.store(false, Ordering::Relaxed);
        match self.inst.activate(sr, 1, u32::try_from(mb).unwrap_or(u32::MAX)) {
            Ok(()) => {
                self.latency = self.inst.latency() as usize;
                self.inst.state().latency_changed.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                log::warn!("{}: {e}", self.info.id);
                self.latency = 0;
            }
        }
    }

    fn reset(&mut self) {
        self.inst.reset();
        self.queue.retain(|e| matches!(e, Event::Param { .. }));
        self.held = [0; 128];
        self.expr = [[f64::NAN; EXPRESSIONS]; 16];
    }

    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.info.params.iter().position(|p| p.id == id) else { return false };
        let (Some(pi), Some(&cid)) = (self.info.params.get(i), self.ids.get(i)) else { return false };
        let v = pi.clamp(value);
        if let Some(slot) = self.values.get_mut(i) {
            *slot = v;
        }
        // Coalesce with a pending change of the same parameter.
        if let Some(Event::Param { value, .. }) = self.queue.iter_mut().find(|e| matches!(e, Event::Param { id, .. } if *id == cid)) {
            *value = f64::from(v);
        } else {
            self.push(Event::Param { time: 0, id: cid, value: f64::from(v) });
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        let i = self.info.params.iter().position(|p| p.id == id)?;
        self.values.get(i).copied()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(io, frames, None);
    }

    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        self.run(io, frames, key);
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = Some(*transport);
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.key_on(offset, 0, note, velocity);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.key_off(offset, 0, note, 0);
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        if self.midi_in {
            self.push(Event::Midi { time, data: msg });
        } else if self.notes == NoteMode::Clap
            && let Some((expression, key, value)) = expression_of(msg)
        {
            // A plug-in without MIDI input hears controllers, bend and pressure as note
            // expressions on the sounding notes (and on the notes that follow).
            let channel = msg[0] & 0x0F;
            if key < 0
                && let Some(slot) = self.expr.get_mut(usize::from(channel)).and_then(|c| c.get_mut(usize::try_from(expression).unwrap_or(usize::MAX)))
            {
                *slot = value;
            }
            self.push(Event::Expression { time, expression, channel, key, value });
        }
    }

    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        let [status, d1, d2] = msg;
        let ch = status & 0x0F;
        match status & 0xF0 {
            0x90 if d2 > 0 => self.key_on(offset, ch, d1, d2),
            0x80 => self.key_off(offset, ch, d1, d2),
            0x90 => self.key_off(offset, ch, d1, 0),
            // Controllers, pitch bend, pressure and program change (`0xCn`) go as MIDI.
            _ => self.midi(offset, msg),
        }
    }

    fn all_notes_off(&mut self) {
        for k in 0..128u8 {
            let mask = self.held.get(k as usize).copied().unwrap_or(0);
            for ch in 0..16u8 {
                if mask & (1 << ch) != 0 {
                    self.key_off(0, ch, k, 0);
                }
            }
        }
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        let ok = self.inst.load_state(data);
        if ok {
            self.refresh_values();
        }
        ok
    }

    fn open_editor(&mut self) -> Result<(), String> {
        let link = self.inst.gui().ok_or_else(|| "this plugin has no editor".to_string())?;
        link.open(self.info.name)
    }

    fn close_editor(&mut self) {
        if let Some(link) = self.inst.gui() {
            link.close();
        }
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        let link = self.inst.gui()?;
        Some(Box::new(ClapEditor { link, info: self.info, ids: self.ids.clone() }))
    }

    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        Some(Box::new(ClapState(self.inst.state_link()?)))
    }
}

/// A CLAP instance's state handle (see [`soundcraft_dsp::PluginStateReader`]).
struct ClapState(Arc<GuiLink>);

impl soundcraft_dsp::PluginStateReader for ClapState {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.0.save_state()
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        self.0.load_state(data)
    }

    fn load_preset(&mut self, kind: u32, location: Option<&str>, load_key: Option<&str>) -> bool {
        self.0.load_preset(kind, location, load_key)
    }
}

/// A CLAP plugin's floating editor, driven from the main thread (see [`GuiLink`]).
struct ClapEditor {
    link: Arc<GuiLink>,
    info: &'static PluginInfo,
    ids: Vec<u32>,
}

impl PluginEditor for ClapEditor {
    fn open(&mut self) -> Result<(), String> {
        // Edits reported before the editor opened are stale.
        let _ = self.link.idle();
        self.link.open(self.info.name)
    }

    fn close(&mut self) {
        self.link.close();
    }

    fn is_open(&self) -> bool {
        self.link.is_open()
    }

    fn capture(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        self.link.capture()
    }

    fn idle(&mut self) -> Vec<(String, f32)> {
        let raw = self.link.idle();
        let _ = self.link.host().dirty.swap(false, Ordering::Relaxed);
        let mut out: Vec<(String, f32)> = Vec::new();
        for (cid, v) in raw {
            let Some(i) = self.ids.iter().position(|&x| x == cid) else { continue };
            let Some(p) = self.info.params.get(i) else { continue };
            let v = p.clamp(v as f32);
            // Keep only the latest value per parameter.
            match out.iter_mut().find(|(id, _)| id == p.id) {
                Some(e) => e.1 = v,
                None => out.push((p.id.to_string(), v)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parity section 7, bug 24: controllers, bend and pressure become CLAP note expressions for
    /// a plug-in that takes only CLAP notes.
    #[test]
    fn channel_messages_become_note_expressions() {
        assert_eq!(expression_of([0xE3, 0, 0x40]), Some((CLAP_NOTE_EXPRESSION_TUNING, -1, 0.0)));
        assert_eq!(expression_of([0xE0, 0, 0]), Some((CLAP_NOTE_EXPRESSION_TUNING, -1, -2.0)));
        assert_eq!(expression_of([0xD1, 127, 0]), Some((CLAP_NOTE_EXPRESSION_PRESSURE, -1, 1.0)));
        assert_eq!(expression_of([0xA0, 60, 127]), Some((CLAP_NOTE_EXPRESSION_PRESSURE, 60, 1.0)));
        assert_eq!(expression_of([0xB0, 1, 127]), Some((CLAP_NOTE_EXPRESSION_VIBRATO, -1, 1.0)));
        assert_eq!(expression_of([0xB0, 7, 127]), Some((CLAP_NOTE_EXPRESSION_VOLUME, -1, 1.0)));
        assert_eq!(expression_of([0xB0, 11, 0]), Some((CLAP_NOTE_EXPRESSION_EXPRESSION, -1, 0.0)));
        assert_eq!(expression_of([0xB0, 74, 127]), Some((CLAP_NOTE_EXPRESSION_BRIGHTNESS, -1, 1.0)));
        assert_eq!(expression_of([0xB0, 10, 127]), Some((CLAP_NOTE_EXPRESSION_PAN, -1, 1.0)));
        assert_eq!(expression_of([0xB0, 64, 127]), None);
        assert_eq!(expression_of([0xC0, 3, 0]), None);
        assert!(EXPRESSIONS as i32 > CLAP_NOTE_EXPRESSION_PRESSURE);
    }
}
