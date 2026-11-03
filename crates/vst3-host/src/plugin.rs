//! `Vst3Plugin`: a loaded VST3 instance behind the safe `soundcraft_dsp::Plugin` trait.

use crate::ffi::{
    AFTERTOUCH, BusBuffers, EVENT_CAP, EditorLink, HostContext, Instance, MIDI_CTRLS, NoteEvent, PARAM_QUEUE_CAP, PITCH_BEND, PROGRAM_CHANGE,
};
use soundcraft_dsp::{Plugin, PluginEditor, PluginInfo, Transport, Unit};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use vst3::ComWrapper;
use vst3::Steinberg::Vst as sv;

/// Largest block we set a plugin up for.
const MAX_BLOCK: usize = 1 << 16;
/// Most channels a host strip may ask for.
const MAX_CHANNELS: usize = 64;
/// An `unsynced` slot with no value waiting (a NaN pattern, which `set_param` never stores).
const SYNCED: u64 = u64::MAX;

/// How a SoundCraft parameter value maps to the VST3 normalized value.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mapping {
    /// Toggle/choice index `0..=steps` ↔ `index / steps`.
    Stepped(f64),
    /// Plain value through the controller's `plainParamToNormalized`.
    Plain,
    /// Linear over the info range (the controller gave no usable plain range).
    Linear,
}

/// A hosted VST3 plugin. Parameter ids are VST3 `ParamID`s as decimal strings ("0", "17", …);
/// values are plain (display) values with their names and ranges in `info().params`.
pub struct Vst3Plugin {
    inst: Instance,
    info: &'static PluginInfo,
    /// VST3 id of each `info.params` entry (same order).
    ids: Vec<u32>,
    maps: Vec<Mapping>,
    values: Vec<f32>,
    /// Normalized values waiting for the next block (coalesced per id; bounded).
    pending: Vec<(u32, f64)>,
    /// Host-set normalized values the controller has not seen yet (same order as `ids`; the bits
    /// of an `f64`, or [`SYNCED`]). `set_param` may run on the audio thread, so it never calls the
    /// controller; the state reader, which already does, hands them over first.
    unsynced: Arc<[AtomicU64]>,
    /// Pending notes, kept sorted by time. Never grows past `EVENT_CAP`.
    notes: Vec<NoteEvent>,
    /// The parameter each MIDI controller drives per channel (see `Instance::midi_mapping`),
    /// built at `prepare`.
    midi_map: Vec<Option<u32>>,
    /// Per MIDI channel: what a program number is divided by to give the program parameter's
    /// normalized value (its step count, or 127 when it has none).
    program_steps: [f64; 16],
    /// Program changes dropped because their channel has no program parameter.
    dropped_program_changes: u64,
    /// Controller points for the next block: (time, param id, normalized value). Bounded.
    controls: Vec<(u32, u32, f64)>,
    inputs: BusBuffers,
    outputs: BusBuffers,
    has_input: bool,
    has_output: bool,
    event_input: bool,
    /// Held notes: a bitmask of MIDI channels per key.
    held: [u16; 128],
    channels: usize,
    max_block: usize,
    latency: usize,
    tail: usize,
    position: i64,
    transport: Option<Transport>,
    /// Set up for an offline render (`kOffline` process mode) at the next `prepare`.
    offline: bool,
}

impl Vst3Plugin {
    pub(crate) fn new(inst: Instance, info: &'static PluginInfo) -> Vst3Plugin {
        let ids: Vec<u32> = info.params.iter().map(|p| p.id.parse::<u32>().unwrap_or(u32::MAX)).collect();
        let maps: Vec<Mapping> = info
            .params
            .iter()
            .zip(&ids)
            .map(|(p, &id)| match p.unit {
                Unit::Toggle | Unit::Choice => Mapping::Stepped(f64::from(p.max.max(1.0))),
                _ => {
                    let lo = inst.to_plain(id, 0.0);
                    let hi = inst.to_plain(id, 1.0);
                    match (lo, hi) {
                        (Some(lo), Some(hi))
                            if (lo as f32 - p.min).abs() <= 1e-6 * p.min.abs().max(1.0)
                                && (hi as f32 - p.max).abs() <= 1e-6 * p.max.abs().max(1.0) =>
                        {
                            Mapping::Plain
                        }
                        _ => Mapping::Linear,
                    }
                }
            })
            .collect();
        let mut p = Vst3Plugin {
            info,
            values: Vec::with_capacity(ids.len()),
            pending: Vec::with_capacity(ids.len().min(PARAM_QUEUE_CAP)),
            unsynced: ids.iter().map(|_| AtomicU64::new(SYNCED)).collect(),
            notes: Vec::with_capacity(EVENT_CAP),
            midi_map: Vec::new(),
            program_steps: [127.0; 16],
            dropped_program_changes: 0,
            controls: Vec::with_capacity(EVENT_CAP),
            inputs: BusBuffers::new(&[], 0),
            outputs: BusBuffers::new(&[], 0),
            has_input: false,
            has_output: false,
            event_input: false,
            held: [0; 128],
            channels: 2,
            max_block: 0,
            latency: 0,
            tail: 0,
            position: 0,
            transport: None,
            offline: false,
            ids,
            maps,
            inst,
        };
        let values: Vec<f32> = (0..p.ids.len())
            .map(|i| {
                let (Some(pi), Some(&id)) = (p.info.params.get(i), p.ids.get(i)) else { return 0.0 };
                p.inst.normalized_value(id).and_then(|n| p.plain_from_normalized(i, n)).map_or(pi.default, |v| pi.clamp(v))
            })
            .collect();
        p.values = values;
        p
    }

    fn plain_from_normalized(&self, i: usize, n: f64) -> Option<f32> {
        let pi = self.info.params.get(i)?;
        let id = *self.ids.get(i)?;
        let v = match self.maps.get(i)? {
            Mapping::Stepped(steps) => (n * steps).round(),
            Mapping::Plain => self.inst.to_plain(id, n)?,
            Mapping::Linear => f64::from(pi.min) + n * f64::from(pi.max - pi.min),
        };
        v.is_finite().then_some(v as f32)
    }

    fn normalized_from_plain(&self, i: usize, v: f32) -> Option<f64> {
        let pi = self.info.params.get(i)?;
        let id = *self.ids.get(i)?;
        let span = f64::from(pi.max - pi.min);
        let linear = if span > 0.0 { (f64::from(v) - f64::from(pi.min)) / span } else { 0.0 };
        let n = match self.maps.get(i)? {
            Mapping::Stepped(steps) if *steps > 0.0 => f64::from(v) / steps,
            Mapping::Stepped(_) => 0.0,
            Mapping::Plain => self.inst.to_normalized(id, f64::from(v)).unwrap_or(linear),
            Mapping::Linear => linear,
        };
        n.is_finite().then(|| n.clamp(0.0, 1.0))
    }

    /// The editor's size in pixels without opening it (creates and releases a view). For
    /// diagnostics and tests; real plugins expect this on the main thread.
    pub fn editor_size(&self) -> Result<(u32, u32), String> {
        self.inst.editor().ok_or_else(|| "this plugin has no edit controller".to_string())?.probe()
    }

    /// A picture of the open editor as `(width, height, RGBA)` (Windows; `None` elsewhere, when
    /// closed, and off the main thread). For tests and tools; the app goes through
    /// `PluginEditor::capture`.
    pub fn capture_editor(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.inst.editor()?.capture()
    }

    /// Re-reads every parameter value from the controller (after a state load) and drops
    /// changes still queued from before it.
    fn refresh_values(&mut self) {
        for i in 0..self.ids.len() {
            let (Some(pi), Some(&id)) = (self.info.params.get(i), self.ids.get(i)) else { continue };
            if let Some(v) = self.inst.normalized_value(id).and_then(|n| self.plain_from_normalized(i, n))
                && let Some(slot) = self.values.get_mut(i)
            {
                *slot = pi.clamp(v);
            }
        }
        self.pending.clear();
        for u in self.unsynced.iter() {
            u.store(SYNCED, Ordering::Relaxed);
        }
    }

    /// Inserts keeping the queue time-sorted; drops the event when the queue is full (never
    /// allocates on the audio thread).
    fn push_note(&mut self, e: NoteEvent) {
        if self.notes.len() >= self.notes.capacity() {
            return;
        }
        let pos = self.notes.iter().rposition(|q| q.time() <= e.time()).map_or(0, |i| i + 1);
        self.notes.insert(pos, e);
    }

    /// How many program changes were dropped because their channel maps no program parameter.
    pub fn dropped_program_changes(&self) -> u64 {
        self.dropped_program_changes
    }

    /// Reads the controller's MIDI mapping for every channel, and the step count of each
    /// channel's program parameter. Main thread (`prepare`).
    fn build_midi_map(&mut self) {
        self.midi_map = self.inst.midi_mapping();
        self.program_steps = [127.0; 16];
        let programs: Vec<(usize, u32)> =
            (0..16).filter_map(|ch| self.midi_map.get(ch * MIDI_CTRLS + PROGRAM_CHANGE).copied().flatten().map(|id| (ch, id))).collect();
        if programs.is_empty() {
            return;
        }
        let params = self.inst.params();
        for (ch, id) in programs {
            let steps = params.iter().find(|p| p.id == id).map_or(0, |p| p.step_count);
            if let Some(slot) = self.program_steps.get_mut(ch) {
                *slot = if steps > 0 { f64::from(steps) } else { 127.0 };
            }
        }
    }

    /// A note-on on MIDI channel `ch` (0–15).
    fn key_on(&mut self, offset: usize, ch: u8, note: u8, velocity: u8) {
        if note > 127 || !self.event_input {
            return;
        }
        let ch = ch & 0x0F;
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        self.push_note(NoteEvent::On { time, channel: ch, key: note, velocity: f32::from(velocity.min(127)) / 127.0 });
        if let Some(h) = self.held.get_mut(note as usize) {
            *h |= 1 << ch;
        }
    }

    /// A note-off on MIDI channel `ch` (0–15).
    fn key_off(&mut self, offset: usize, ch: u8, note: u8) {
        if note > 127 || !self.event_input {
            return;
        }
        let ch = ch & 0x0F;
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        self.push_note(NoteEvent::Off { time, channel: ch, key: note });
        if let Some(h) = self.held.get_mut(note as usize) {
            *h &= !(1 << ch);
        }
    }
}

impl Plugin for Vst3Plugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        let mb = max_block.clamp(1, MAX_BLOCK);
        self.channels = channels.clamp(1, MAX_CHANNELS);
        self.max_block = 0;
        match self.inst.configure(sr, mb, self.channels == 1, self.offline) {
            Ok(layout) => {
                self.inputs = BusBuffers::new(&layout.inputs, mb);
                self.outputs = BusBuffers::new(&layout.outputs, mb);
                self.has_input = layout.inputs.first().is_some_and(|&c| c > 0);
                self.has_output = layout.outputs.first().is_some_and(|&c| c > 0);
                self.event_input = layout.event_input;
                self.max_block = mb;
                self.latency = self.inst.latency() as usize;
                self.tail = self.inst.tail() as usize;
                self.build_midi_map();
            }
            Err(e) => {
                log::warn!("{}: {e}", self.info.id);
                self.latency = 0;
            }
        }
        // Re-send every value so a re-activated plugin matches the session.
        self.pending.clear();
        for i in 0..self.ids.len().min(PARAM_QUEUE_CAP) {
            if let (Some(&id), Some(n)) = (self.ids.get(i), self.values.get(i).and_then(|&v| self.normalized_from_plain(i, v))) {
                self.pending.push((id, n));
            }
        }
    }

    fn reset(&mut self) {
        self.notes.clear();
        self.controls.clear();
        self.held = [0; 128];
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.info.params.iter().position(|p| p.id == id) else { return false };
        let (Some(pi), Some(&vid)) = (self.info.params.get(i), self.ids.get(i)) else { return false };
        let v = pi.clamp(value);
        if let Some(slot) = self.values.get_mut(i) {
            *slot = v;
        }
        let Some(n) = self.normalized_from_plain(i, v) else { return true };
        if let Some(u) = self.unsynced.get(i) {
            u.store(n.to_bits(), Ordering::Relaxed);
        }
        if let Some(e) = self.pending.iter_mut().find(|(pid, _)| *pid == vid) {
            e.1 = n;
        } else if self.pending.len() < self.pending.capacity() {
            self.pending.push((vid, n));
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        let i = self.info.params.iter().position(|p| p.id == id)?;
        self.values.get(i).copied()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if !self.inst.is_active() || self.max_block == 0 {
            return;
        }
        let ch = self.channels.min(io.len());
        let frames = io.iter().take(ch).map(Vec::len).fold(frames, usize::min);
        if frames == 0 || ch == 0 {
            return;
        }
        if self.inst.host().latency_changed.swap(false, Ordering::Relaxed) {
            self.latency = self.inst.latency() as usize;
        }
        let last = u32::try_from(frames - 1).unwrap_or(u32::MAX);
        for e in &mut self.notes {
            if e.time() > last {
                e.set_time(last);
            }
        }
        for c in &mut self.controls {
            c.0 = c.0.min(last);
        }
        let mut start = 0usize;
        while start < frames {
            let n = (frames - start).min(self.max_block);
            let end = u32::try_from(start + n).unwrap_or(u32::MAX);
            // Feed the main input bus (mono sources fill every bus channel).
            if self.has_input {
                for c in 0..self.inputs.channels(0) {
                    let src = io.get(c.min(ch - 1)).and_then(|s| s.get(start..start + n));
                    if let (Some(src), Some(dst)) = (src, self.inputs.channel_mut(0, c).and_then(|d| d.get_mut(..n))) {
                        dst.copy_from_slice(src);
                    }
                }
            }
            let k = self.notes.iter().position(|e| e.time() >= end).unwrap_or(self.notes.len());
            let offset = u32::try_from(start).unwrap_or(u32::MAX);
            let notes = self.notes.get(..k).unwrap_or(&[]);
            let r = self.inst.process(
                n,
                &mut self.inputs,
                &mut self.outputs,
                &self.pending,
                &self.controls,
                notes,
                offset,
                self.position,
                self.transport.as_ref(),
            );
            self.pending.clear();
            self.notes.drain(..k);
            self.position = self.position.saturating_add(i64::try_from(n).unwrap_or(0));
            match r {
                Ok(()) if self.has_output => {
                    let oc = self.outputs.channels(0);
                    for (c, dst) in io.iter_mut().take(ch).enumerate() {
                        let Some(dst) = dst.get_mut(start..start + n) else { continue };
                        if ch == 1 && oc >= 2 {
                            if let (Some(l), Some(r)) = (self.outputs.channel(0, 0), self.outputs.channel(0, 1)) {
                                for ((d, a), b) in dst.iter_mut().zip(l).zip(r) {
                                    *d = 0.5 * (a + b);
                                }
                            }
                        } else if let Some(src) = self.outputs.channel(0, c.min(oc.saturating_sub(1))).and_then(|s| s.get(..n)) {
                            dst.copy_from_slice(src);
                        }
                    }
                }
                Err(e) => log::debug!("{}: {e}", self.info.id),
                _ => {}
            }
            start += n;
        }
        self.controls.clear();
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = Some(*transport);
    }

    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.key_on(offset, 0, note, velocity);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.key_off(offset, 0, note);
    }

    /// Controllers, pressure, pitch bend and program change on the message's own channel: each
    /// reaches the parameter the plugin maps it to on that channel (`IMidiMapping`), or is
    /// dropped (a dropped program change is counted).
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        let ch = msg[0] & 0x0F;
        let [_, d1, d2] = msg.map(|b| b & 0x7F);
        let (ctrl, value) = match msg[0] & 0xF0 {
            0xB0 => (usize::from(d1), f64::from(d2) / 127.0),
            0xD0 => (AFTERTOUCH, f64::from(d1) / 127.0),
            0xE0 => (PITCH_BEND, f64::from(u16::from(d2) << 7 | u16::from(d1)) / 16383.0),
            0xC0 => {
                let steps = self.program_steps.get(usize::from(ch)).copied().unwrap_or(127.0);
                (PROGRAM_CHANGE, (f64::from(d1) / steps).clamp(0.0, 1.0))
            }
            0xA0 if self.event_input => {
                self.push_note(NoteEvent::Pressure { time, channel: ch, key: d1, pressure: f32::from(d2) / 127.0 });
                return;
            }
            _ => return,
        };
        match self.midi_map.get(usize::from(ch) * MIDI_CTRLS + ctrl) {
            Some(&Some(id)) => {
                if self.controls.len() < self.controls.capacity() {
                    self.controls.push((time, id, value));
                }
            }
            _ if ctrl == PROGRAM_CHANGE => self.dropped_program_changes = self.dropped_program_changes.saturating_add(1),
            _ => {}
        }
    }

    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        let [status, d1, d2] = msg;
        let ch = status & 0x0F;
        match status & 0xF0 {
            0x90 if d2 > 0 => self.key_on(offset, ch, d1, d2),
            0x80 | 0x90 => self.key_off(offset, ch, d1),
            _ => self.midi(offset, msg),
        }
    }

    fn all_notes_off(&mut self) {
        for k in 0..128u8 {
            let mask = self.held.get(k as usize).copied().unwrap_or(0);
            for ch in 0..16u8 {
                if mask & (1 << ch) != 0 {
                    self.key_off(0, ch, k);
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
        self.inst.editor().ok_or_else(|| "this plugin has no editor".to_string())?.open(self.info.name)
    }

    fn close_editor(&mut self) {
        if let Some(e) = self.inst.editor() {
            e.close();
        }
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        let link = self.inst.editor()?;
        let mut units: HashMap<u32, String> = self.inst.params().into_iter().map(|p| (p.id, p.units)).collect();
        Some(Box::new(Vst3Editor {
            link,
            host: self.inst.host_ref(),
            info: self.info,
            index: self.info.params.iter().enumerate().map(|(i, p)| (p.id, i)).collect(),
            units: self.ids.iter().map(|id| units.remove(id).unwrap_or_default()).collect(),
            ids: self.ids.clone(),
            maps: self.maps.clone(),
        }))
    }

    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        Some(Box::new(Vst3State { link: self.inst.state_link(), ids: self.ids.clone(), unsynced: Arc::clone(&self.unsynced) }))
    }
}

/// A VST3 instance's state handle (see [`soundcraft_dsp::PluginStateReader`]).
struct Vst3State {
    link: Arc<EditorLink>,
    ids: Vec<u32>,
    unsynced: Arc<[AtomicU64]>,
}

impl soundcraft_dsp::PluginStateReader for Vst3State {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        // A parameter only the controller stores (no processor state) would otherwise be saved
        // with the controller's stale value.
        for (&id, u) in self.ids.iter().zip(self.unsynced.iter()) {
            let bits = u.swap(SYNCED, Ordering::Relaxed);
            if bits != SYNCED {
                self.link.sync_param(id, f64::from_bits(bits));
            }
        }
        self.link.save_state()
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        // Host-set values from before the load are stale now.
        for u in self.unsynced.iter() {
            u.store(SYNCED, Ordering::Relaxed);
        }
        self.link.load_state(data)
    }
}

/// A VST3 plugin's editor (`IPlugView` in a host window), driven from the main thread.
struct Vst3Editor {
    link: Arc<EditorLink>,
    host: ComWrapper<HostContext>,
    info: &'static PluginInfo,
    /// Position of each SoundCraft param id in `info.params` (looked up once per row and frame).
    index: HashMap<&'static str, usize>,
    /// The plugin's own unit label of each param (same order as `ids`).
    units: Vec<String>,
    ids: Vec<u32>,
    maps: Vec<Mapping>,
}

impl Vst3Editor {
    fn plain(&self, i: usize, n: f64) -> Option<f32> {
        let pi = self.info.params.get(i)?;
        let id = *self.ids.get(i)?;
        let v = match self.maps.get(i)? {
            Mapping::Stepped(steps) => (n * steps).round(),
            Mapping::Plain => self.link.to_plain(id, n)?,
            Mapping::Linear => f64::from(pi.min) + n * f64::from(pi.max - pi.min),
        };
        v.is_finite().then(|| pi.clamp(v as f32))
    }

    fn normalized(&self, i: usize, v: f32) -> Option<f64> {
        let pi = self.info.params.get(i)?;
        let id = *self.ids.get(i)?;
        let span = f64::from(pi.max - pi.min);
        let linear = if span > 0.0 { (f64::from(v) - f64::from(pi.min)) / span } else { 0.0 };
        let n = match self.maps.get(i)? {
            Mapping::Stepped(steps) if *steps > 0.0 => f64::from(v) / steps,
            Mapping::Stepped(_) => 0.0,
            Mapping::Plain => self.link.to_normalized(id, f64::from(v)).unwrap_or(linear),
            Mapping::Linear => linear,
        };
        n.is_finite().then(|| n.clamp(0.0, 1.0))
    }
}

impl PluginEditor for Vst3Editor {
    fn open(&mut self) -> Result<(), String> {
        // Edits reported before the editor opened (e.g. during setup) are stale.
        let _ = self.host.take_edits();
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

    /// Services the editor and its messages, then reports the edits made in it and, after a
    /// `restartComponent(kParamValuesChanged)` (a preset loaded in the plugin's own GUI, a
    /// processor message that changed the controller's values), every parameter's current
    /// value, so the session follows what the plugin now plays.
    fn idle(&mut self) -> Vec<(String, f32)> {
        self.link.idle();
        let mut out: Vec<(String, f32)> = Vec::new();
        let mut put = |id: &str, v: f32| match out.iter_mut().find(|(k, _)| k == id) {
            Some(e) => e.1 = v,
            None => out.push((id.to_string(), v)),
        };
        for (vid, n) in self.host.take_edits() {
            let Some(i) = self.ids.iter().position(|&x| x == vid) else { continue };
            if let (Some(p), Some(v)) = (self.info.params.get(i), self.plain(i, n)) {
                put(p.id, v);
            }
        }
        let flags = self.host.restart_flags.swap(0, Ordering::Relaxed);
        if flags & sv::RestartFlags_::kParamValuesChanged as u32 != 0 {
            for (i, &vid) in self.ids.iter().enumerate() {
                if let (Some(p), Some(v)) = (self.info.params.get(i), self.link.normalized_value(vid).and_then(|n| self.plain(i, n))) {
                    put(p.id, v);
                }
            }
        }
        if flags & sv::RestartFlags_::kIoChanged as u32 != 0 {
            log::info!("{}: bus configuration changed (kIoChanged); it applies when the plugin is next prepared", self.info.name);
        }
        if flags & sv::RestartFlags_::kReloadComponent as u32 != 0 {
            log::warn!("{}: asked to be reloaded (kReloadComponent); not supported, the instance keeps running", self.info.name);
        }
        out
    }

    fn set_param(&mut self, id: &str, value: f32) {
        let Some(i) = self.info.params.iter().position(|p| p.id == id) else { return };
        if let (Some(&vid), Some(n)) = (self.ids.get(i), self.normalized(i, value)) {
            self.link.set_normalized(vid, n);
        }
    }

    fn value_text(&mut self, id: &str, value: f32) -> Option<String> {
        let &i = self.index.get(id)?;
        let pi = self.info.params.get(i)?;
        // A plugin whose plain mapping is the identity (JUCE, the SDK's default `Parameter`) leaves
        // a 0..1 range with the units of its real one: only its own text says what the value is.
        if !matches!(self.maps.get(i)?, Mapping::Plain) || pi.min != 0.0 || pi.max != 1.0 {
            return None;
        }
        let text = self.link.value_text(*self.ids.get(i)?, self.normalized(i, value)?)?;
        // VST3 keeps the units apart from the text (JUCE's label, the SDK's `RangeParameter`),
        // but some plugins put them in both.
        let units = self.units.get(i).map_or("", |u| u.trim());
        if units.is_empty() || text.trim_end().to_lowercase().ends_with(&units.to_lowercase()) { Some(text) } else { Some(format!("{text} {units}")) }
    }
}
