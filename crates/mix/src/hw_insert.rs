//! Hardware Insert: an insert that sends the track out of a hardware output and takes it back
//! from a hardware input, so outboard gear sits in the chain like a plug-in.
//!
//! The module's parameters name the output path (`out`, an index into `Session::outputs`, 1 for
//! the first path after the main), the input path (`in`, an index into `Session::inputs`) and the
//! round trip (`latency`, samples at the session rate: what the ping measured, 0 for "not
//! measured"). Live, the mixer special-cases the module in the chain ([`crate::MixEngine`]): the
//! module's input goes into the strip's send for that output, summed into the output's buffer
//! after the strips render, and the module's output is the device input the mix has for this
//! block. The return is therefore the send of earlier callbacks by exactly the device round trip,
//! which the module reports as its latency so delay compensation lines it up with everything
//! else.
//!
//! Offline (a bounce, a freeze) there is no gear to play through: the module passes its input
//! through, delayed by the same latency so the mix keeps its timing. A real-time bounce prints it
//! as heard.

use soundcraft_dsp::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};

/// The Hardware Insert's plugin id.
pub const ID: &str = "hw_insert";

/// The longest round trip the module compensates, in samples (a second at 192 kHz).
pub const MAX_LATENCY: usize = 192_000;

static PARAMS: [ParamInfo; 3] = [
    ParamInfo {
        id: "out",
        name: "Output",
        min: 1.0,
        max: 64.0,
        default: 1.0,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
        display: Some(index_text),
        group: "",
    },
    ParamInfo {
        id: "in",
        name: "Input",
        min: 0.0,
        max: 63.0,
        default: 0.0,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
        display: Some(index_text),
        group: "",
    },
    ParamInfo {
        id: "latency",
        name: "Round Trip",
        min: 0.0,
        max: MAX_LATENCY as f32,
        default: 0.0,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
        display: Some(latency_text),
        group: "",
    },
];

fn index_text(v: f32) -> String {
    format!("{}", v.round() as i64)
}

fn latency_text(v: f32) -> String {
    if v < 0.5 { "Not measured".into() } else { format!("{} samples", v.round() as i64) }
}

pub static INFO: PluginInfo = PluginInfo {
    id: ID,
    name: "Hardware Insert",
    short_name: "HW Ins",
    category: Category::Other,
    params: &PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The plugin info of `id` when it is the Hardware Insert.
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    (id == ID).then_some(&INFO)
}

/// A new Hardware Insert when `id` names it.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    (id == ID).then(|| Box::new(HwInsert::default()) as Box<dyn Plugin>)
}

/// The module: its paths and round trip, and the delay its offline pass-through runs through.
#[derive(Default)]
pub struct HwInsert {
    out: usize,
    input: usize,
    latency: usize,
    /// One ring per channel, `MAX_LATENCY` long once prepared (the offline pass-through).
    delay: Vec<Vec<f32>>,
    at: usize,
}

impl HwInsert {
    /// The output path (index into `Session::outputs`).
    pub fn output(&self) -> usize {
        self.out
    }

    /// The input path (index into `Session::inputs`).
    pub fn input(&self) -> usize {
        self.input
    }
}

impl Plugin for HwInsert {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, channels: usize) {
        let len = MAX_LATENCY + 1;
        self.delay = vec![vec![0.0; len]; channels.max(1)];
        self.at = 0;
    }

    fn reset(&mut self) {
        for d in &mut self.delay {
            d.iter_mut().for_each(|x| *x = 0.0);
        }
        self.at = 0;
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = PARAMS.iter().find(|p| p.id == id) else { return false };
        let v = p.clamp(value).round().max(0.0) as usize;
        match id {
            "out" => self.out = v,
            "in" => self.input = v,
            _ => self.latency = v.min(MAX_LATENCY),
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "out" => Some(self.out as f32),
            "in" => Some(self.input as f32),
            "latency" => Some(self.latency as f32),
            _ => None,
        }
    }

    /// Offline: the input, `latency` later (see the module docs). Never allocates.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let len = MAX_LATENCY + 1;
        let at = self.at;
        for (c, d) in io.iter_mut().zip(self.delay.iter_mut()) {
            for (i, x) in c.iter_mut().take(frames).enumerate() {
                let w = (at + i) % len;
                let r = (w + len - self.latency.min(MAX_LATENCY)) % len;
                if let Some(slot) = d.get_mut(w) {
                    *slot = *x;
                }
                *x = d.get(r).copied().unwrap_or(0.0);
            }
        }
        self.at = (at + frames) % len;
    }

    fn latency(&self) -> usize {
        self.latency
    }
}

/// The live hardware insert's work on a strip's buffer (`io`, `frames` long): `io` goes into
/// `send` (the strip's send for the module's output, which the mix sums into that output's
/// buffer), and the device input channels from `first` (the input path's first channel) come back
/// into `io`, one per channel (a narrower input repeats its last channel). Never allocates.
pub fn swap_live(io: &mut [Vec<f32>], send: &mut [Vec<f32>], input: &[Vec<f32>], first: usize, width: usize, frames: usize) {
    for (c, o) in io.iter_mut().enumerate() {
        if let Some(s) = send.get_mut(c) {
            for (d, x) in s.iter_mut().zip(o.iter()).take(frames) {
                *d = *x;
            }
        }
        let src = input.get(first + c.min(width.saturating_sub(1)));
        for (i, x) in o.iter_mut().take(frames).enumerate() {
            let v = src.and_then(|b| b.get(i)).copied().unwrap_or(0.0);
            *x = if v.is_finite() { v } else { 0.0 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_it_passes_its_input_through_its_latency_later() {
        let mut p = HwInsert::default();
        p.prepare(48_000.0, 64, 1);
        p.set_param("latency", 10.0);
        assert_eq!(p.latency(), 10);
        let mut io = vec![(0..64).map(|i| i as f32 + 1.0).collect::<Vec<f32>>()];
        p.process(&mut io, 64);
        assert!(io[0][..10].iter().all(|x| *x == 0.0));
        assert_eq!(io[0][10], 1.0);
        assert_eq!(io[0][63], 54.0);
        let mut io = vec![vec![0.0; 64]];
        p.process(&mut io, 64);
        assert_eq!(&io[0][..10], &[55.0, 56.0, 57.0, 58.0, 59.0, 60.0, 61.0, 62.0, 63.0, 64.0]);
    }

    #[test]
    fn live_it_sends_its_input_and_returns_the_device_input() {
        let mut io = vec![vec![1.0; 8], vec![2.0; 8]];
        let mut send = vec![vec![0.0; 8], vec![0.0; 8]];
        let input = vec![vec![7.0; 8], vec![8.0; 8], vec![9.0; 8]];
        swap_live(&mut io, &mut send, &input, 1, 2, 8);
        assert_eq!(send, vec![vec![1.0; 8], vec![2.0; 8]]);
        assert_eq!(io, vec![vec![8.0; 8], vec![9.0; 8]]);
        // A mono return feeds both channels.
        swap_live(&mut io, &mut send, &input, 0, 1, 8);
        assert_eq!(io, vec![vec![7.0; 8], vec![7.0; 8]]);
    }
}
