//! Where the main mix and the hardware outputs land on the device's output channels.
//!
//! The mix renders the main mix (its own format) and one buffer per hardware output
//! ([`soundcraft_mix::MixEngine::hardware_out`]). An [`OutMap`], built from the session and the
//! open device's channel count, sums them into device-layout planar channels:
//! - main channel `c` goes to device channel `main_first + c`;
//! - a main mix wider than the device that starts at channel 0 is folded: to stereo (ITU-R
//!   BS.775) on a device with two or more channels, to the average of the two sides on a mono
//!   device. Starting elsewhere, the channels that don't fit are dropped;
//! - hardware output `i` channel `c` is summed into `first + c` when the device has it, and
//!   dropped otherwise (never wrapped);
//! - each device channel is clamped to ±1, and NaN or infinities become silence.
//!
//! Building and applying a map never allocates: it is plain data.

use soundcraft_mix::{MAX_CHANNELS, itu_stereo_fold, main_channels};
use soundcraft_model::Session;

/// Hardware outputs (besides the main) a map places; later ones are silent.
pub(crate) const MAX_HW_OUTPUTS: usize = 64;
/// Device output channels the player opens and fills at most.
pub(crate) const MAX_DEVICE_CHANNELS: usize = 64;

/// How the main mix reaches a device too narrow for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fold {
    /// It fits, or doesn't start at channel 0: channel by channel, the rest dropped.
    None,
    /// To stereo on device channels 0 and 1.
    Stereo,
    /// To the average of the stereo sides on device channel 0.
    Mono,
}

/// Device placement of the main mix and the hardware outputs (see the module docs).
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutMap {
    device_channels: usize,
    main_first: usize,
    main_ch: usize,
    fold: Fold,
    /// ITU stereo fold-down gains per main channel.
    gains: [(f32, f32); MAX_CHANNELS],
    /// Hardware output `j` is `Session::outputs[j + 1]`: (first device channel, channels).
    hw: [(usize, usize); MAX_HW_OUTPUTS],
    hw_len: usize,
}

impl OutMap {
    /// The map of `s` on a device with `device_channels` outputs (at least 1, at most
    /// [`MAX_DEVICE_CHANNELS`]). Does not allocate.
    pub(crate) fn new(s: &Session, device_channels: usize) -> OutMap {
        let device_channels = device_channels.clamp(1, MAX_DEVICE_CHANNELS);
        let main_first = s.outputs.first().map_or(0, |o| usize::from(o.first_channel));
        let main_ch = main_channels(s);
        let fold = match main_first.saturating_add(main_ch) <= device_channels {
            true => Fold::None,
            false if main_first != 0 => Fold::None,
            false if device_channels >= 2 => Fold::Stereo,
            false => Fold::Mono,
        };
        let mut hw = [(0, 0); MAX_HW_OUTPUTS];
        let mut hw_len = 0;
        for (slot, o) in hw.iter_mut().zip(s.outputs.iter().skip(1)) {
            *slot = (usize::from(o.first_channel), o.format.channels().clamp(1, MAX_CHANNELS));
            hw_len += 1;
        }
        OutMap { device_channels, main_first, main_ch, fold, gains: fold_gains(s), hw, hw_len }
    }

    /// The device channels this map fills.
    pub(crate) fn device_channels(&self) -> usize {
        self.device_channels
    }

    /// Writes `n` frames of the device channels into `out` (planar, device layout; channels past
    /// the map's device are left alone) from the planar `main` mix and the hardware outputs `hw`
    /// (indexed like `Session::outputs`; index 0 unused). Does not allocate.
    pub(crate) fn map(&self, main: &[Vec<f32>], hw: &[Vec<Vec<f32>>], out: &mut [Vec<f32>], n: usize) {
        let dev = self.device_channels.min(out.len());
        let Some(out) = out.get_mut(..dev) else { return };
        for c in out.iter_mut() {
            c.iter_mut().take(n).for_each(|x| *x = 0.0);
        }
        let main_ch = self.main_ch.min(main.len());
        match self.fold {
            Fold::None => {
                for (c, src) in main.iter().enumerate().take(main_ch) {
                    if let Some(dst) = self.main_first.checked_add(c).and_then(|d| out.get_mut(d)) {
                        dst.iter_mut().zip(src).take(n).for_each(|(o, x)| *o = *x);
                    }
                }
            }
            Fold::Stereo | Fold::Mono => {
                let at = |c: usize, i: usize| main.get(c).and_then(|v| v.get(i)).copied().unwrap_or(0.0);
                for i in 0..n {
                    // Fold surround to stereo first; a mono device averages the two sides.
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    if main_ch == 2 {
                        (l, r) = (at(0, i), at(1, i));
                    } else {
                        for (c, (gl, gr)) in self.gains.iter().enumerate().take(main_ch) {
                            let v = at(c, i);
                            l += v * gl;
                            r += v * gr;
                        }
                    }
                    let sides: [f32; 2] = if self.fold == Fold::Mono { [0.5 * l + 0.5 * r, 0.0] } else { [l, r] };
                    for (o, v) in out.iter_mut().zip(sides) {
                        if let Some(x) = o.get_mut(i) {
                            *x = v;
                        }
                    }
                }
            }
        }
        for (j, &(first, ch)) in self.hw.iter().enumerate().take(self.hw_len) {
            let Some(src) = hw.get(j + 1) else { continue };
            for (c, s) in src.iter().enumerate().take(ch) {
                if let Some(dst) = first.checked_add(c).and_then(|d| out.get_mut(d)) {
                    dst.iter_mut().zip(s).take(n).for_each(|(o, x)| *o += *x);
                }
            }
        }
        for c in out.iter_mut() {
            for x in c.iter_mut().take(n) {
                *x = if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
            }
        }
    }
}

/// ITU stereo fold-down gains for each main channel of a session.
pub(crate) fn fold_gains(s: &Session) -> [(f32, f32); MAX_CHANNELS] {
    let mut g = [(0.0, 0.0); MAX_CHANNELS];
    let f = s.main_format();
    let sp = f.speakers();
    if sp.len() == f.channels() {
        for (o, x) in g.iter_mut().zip(sp.iter()) {
            *o = itu_stereo_fold(*x);
        }
    } else {
        // Ambisonics: W only, at -3 dB per side.
        g[0] = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
    }
    g
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use soundcraft_model::{ChannelFormat, OutputPath};

    /// Main channels at 0.1, 0.2, ... and hardware output `i` channel `c` at `0.01 * (10 i + c + 1)`.
    fn sources(s: &Session, n: usize) -> (Vec<Vec<f32>>, Vec<Vec<Vec<f32>>>) {
        let main = (0..main_channels(s)).map(|c| vec![0.1 * (c + 1) as f32; n]).collect();
        let hw = s
            .outputs
            .iter()
            .enumerate()
            .map(|(i, o)| match i {
                0 => Vec::new(),
                _ => (0..o.format.channels()).map(|c| vec![0.01 * (10 * i + c + 1) as f32; n]).collect(),
            })
            .collect();
        (main, hw)
    }

    fn mapped(s: &Session, device: usize) -> Vec<f32> {
        let (main, hw) = sources(s, 4);
        let mut out = vec![vec![9.0; 4]; device];
        OutMap::new(s, device).map(&main, &hw, &mut out, 4);
        out.iter().map(|c| c[3]).collect()
    }

    #[test]
    fn main_lands_on_its_first_channel_and_outputs_on_theirs() {
        let mut s = Session::default();
        s.outputs[0].first_channel = 2;
        s.outputs.push(OutputPath { name: "Mon 5-6".into(), first_channel: 4, format: ChannelFormat::Stereo });
        let got = mapped(&s, 8);
        let want = [0.0, 0.0, 0.1, 0.2, 0.11, 0.12, 0.0, 0.0];
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-6, "{got:?}");
        }
    }

    #[test]
    fn outputs_sharing_channels_sum_and_clip() {
        let mut s = Session::default();
        s.outputs.push(OutputPath { name: "A".into(), first_channel: 1, format: ChannelFormat::Mono });
        let (main, mut hw) = sources(&s, 4);
        hw[1][0] = vec![0.95; 4];
        let mut out = vec![vec![0.0; 4]; 2];
        OutMap::new(&s, 2).map(&main, &hw, &mut out, 4);
        assert!((out[0][0] - 0.1).abs() < 1e-6);
        assert_eq!(out[1][0], 1.0, "0.2 + 0.95 clips");
    }

    #[test]
    fn channels_beyond_the_device_are_dropped_not_wrapped() {
        let mut s = Session::default();
        s.outputs.push(OutputPath { name: "Mon 5-6".into(), first_channel: 4, format: ChannelFormat::Stereo });
        s.outputs.push(OutputPath { name: "Edge".into(), first_channel: 1, format: ChannelFormat::Stereo });
        let got = mapped(&s, 2);
        assert!((got[0] - 0.1).abs() < 1e-6 && (got[1] - (0.2 + 0.21)).abs() < 1e-6, "{got:?}");
        // A main on 3-4 of a stereo device: dropped, not folded or wrapped.
        let mut s = Session::default();
        s.outputs[0].first_channel = 2;
        assert_eq!(mapped(&s, 2), [0.0, 0.0]);
    }

    #[test]
    fn non_finite_channels_are_silence() {
        let s = Session::default();
        let main = vec![vec![f32::NAN; 4], vec![f32::INFINITY; 4]];
        let mut out = vec![vec![0.5; 4]; 2];
        OutMap::new(&s, 2).map(&main, &[], &mut out, 4);
        assert_eq!(out, vec![vec![0.0; 4]; 2]);
    }

    #[test]
    fn outmap_owns_no_heap_memory_and_map_keeps_the_buffers() {
        // What this proves: an OutMap owns no heap memory (nothing to drop, so building one can
        // hand nothing to free to the audio thread), and mapping writes into the buffers it is
        // given without reallocating them. (It does not count allocations.)
        assert!(!std::mem::needs_drop::<OutMap>());
        let mut s = Session::default();
        for i in 0..80u16 {
            s.outputs.push(OutputPath { name: format!("Out {i}"), first_channel: i, format: ChannelFormat::Stereo });
        }
        let (main, hw) = sources(&s, 256);
        let mut out = vec![vec![0.0f32; 256]; MAX_DEVICE_CHANNELS];
        let before: Vec<(usize, *const f32)> = out.iter().map(|c| (c.capacity(), c.as_ptr())).collect();
        for d in [1, 2, 8, 64, 200] {
            let map = OutMap::new(&s, d);
            assert_eq!(map.hw_len, MAX_HW_OUTPUTS, "outputs past {MAX_HW_OUTPUTS} are silent");
            map.map(&main, &hw, &mut out, 256);
        }
        let after: Vec<(usize, *const f32)> = out.iter().map(|c| (c.capacity(), c.as_ptr())).collect();
        assert_eq!(before, after);
    }
}
