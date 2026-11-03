//! The host-STFT RoFormer runner shared by the Mel-Band RoFormers ([`crate::karaoke`],
//! [`crate::vocals`]) and BS-RoFormer SW ([`crate::bs_roformer`]): ZFTurbo MSS `demix` (generic
//! mode) over fixed chunks ([`msst_split`]); each chunk is STFT'd on the host (periodic Hann,
//! centred, `torch.stft` semantics), masked by the graph and iSTFT'd. Port of
//! `tools/ml-export/karaoke/reference_infer.py` (and `bs_roformer_sw/reference_infer.py`, the same
//! steps with another hop and chunk).
//!
//! Graph I/O: input `spec` `[2, bins, frames, 2]` (channel, bin, frame, re/im; or `[1, 2, …]` on
//! the first karaoke export), output `[stems, 2, bins, frames, 2]` (a one-stem graph may drop the
//! stem axis: the same data).

use crate::chunk::msst_split;
use crate::drumsep::card_usize;
use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::stft::Stft;
use crate::{MlError, Result};
use rustfft::num_complex::Complex32;

/// How a host-STFT graph is run: from its card, with the model's defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Samples per chunk (`chunk.samples`).
    pub chunk: usize,
    /// MSS `num_overlap` (`chunk.num_overlap`).
    pub overlaps: usize,
    pub n_fft: usize,
    pub hop: usize,
    /// Stems the graph outputs.
    pub stems: usize,
    /// The input has a batch axis (`[1, 2, F, T, 2]`).
    pub batched: bool,
}

impl Settings {
    /// The card's settings (`chunk.samples`, `chunk.num_overlap`, `stft.n_fft`, `stft.hop`, the
    /// input's rank), else `defaults`; refused when out of range.
    pub fn from_card(model: &InstalledModel, id: &str, defaults: Settings) -> Result<Settings> {
        let card = &model.card;
        let batched = card
            .extra
            .get("inputs")
            .and_then(|i| i.get(0))
            .and_then(|i| i.get("shape"))
            .and_then(serde_json::Value::as_array)
            .map_or(defaults.batched, |s| s.len() == 5);
        let s = Settings {
            chunk: card_usize(card, "samples", defaults.chunk),
            overlaps: card_usize(card, "num_overlap", defaults.overlaps),
            n_fft: card_usize(card, "n_fft", defaults.n_fft),
            hop: card_usize(card, "hop", defaults.hop),
            stems: defaults.stems,
            batched,
        };
        if s.chunk == 0 || s.chunk > 30 * 48_000 || s.n_fft < 2 || s.n_fft > 16_384 || s.hop == 0 || s.stems == 0 || s.stems > 16 || s.overlaps == 0 {
            return Err(MlError::Model(id.into(), "bad chunk or STFT settings on the card".into()));
        }
        Ok(s)
    }
}

/// The graph's input for one chunk (`[channel][frame]`, two channels): `[2, bins, frames, 2]`.
pub fn spectrogram(stft: &Stft, part: &[Vec<f32>], frames: usize) -> Vec<f32> {
    let bins = stft.bins();
    let mut spec = vec![0.0f32; 2 * bins * frames * 2];
    for (c, ch) in part.iter().enumerate().take(2) {
        for (f, row) in stft.forward(ch).iter().enumerate() {
            for (t, v) in row.iter().enumerate().take(frames) {
                let k = (((c * bins + f) * frames) + t) * 2;
                if let Some(s) = spec.get_mut(k..k + 2) {
                    s.copy_from_slice(&[v.re, v.im]);
                }
            }
        }
    }
    spec
}

/// The graph's output (`[stems, 2, bins, frames, 2]`) back to audio: `[stem][channel][len]`.
pub fn waveforms(stft: &Stft, data: &[f32], stems: usize, frames: usize, len: usize) -> Vec<Vec<Vec<f32>>> {
    let bins = stft.bins();
    (0..stems)
        .map(|s| {
            (0..2)
                .map(|c| {
                    let spec: Vec<Vec<Complex32>> = (0..bins)
                        .map(|f| {
                            (0..frames)
                                .map(|t| {
                                    let k = ((((s * 2 + c) * bins + f) * frames) + t) * 2;
                                    Complex32::new(data.get(k).copied().unwrap_or(0.0), data.get(k + 1).copied().unwrap_or(0.0))
                                })
                                .collect()
                        })
                        .collect();
                    stft.inverse(&spec, len)
                })
                .collect()
        })
        .collect()
}

/// Run the host-STFT graph of `model` over `audio` (`[2][frames]` at the model rate): the graph's
/// stems `[stem][2][frames]` and the device note. `what` is the progress text ("Separating the
/// vocals").
pub fn run(
    model: &InstalledModel,
    id: &str,
    s: Settings,
    audio: &[Vec<f32>],
    device: Device,
    ctx: &Ctx,
    what: &str,
) -> Result<(Vec<Vec<Vec<f32>>>, String)> {
    let x = crate::to_channels(audio, 2);
    let stft = Stft::new(s.n_fft, s.hop);
    let (bins, frames) = (stft.bins(), stft.frames(s.chunk));
    ctx.say(&format!("Loading {}", model.card.name.split(" (").next().unwrap_or(id)));
    let mut onnx = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    ctx.say(&format!("{what} on the {}", onnx.device_label()));
    let out = msst_split(&x, s.chunk, s.overlaps, ctx, |part| {
        let spec = spectrogram(&stft, part, frames);
        let shape = if s.batched { vec![1, 2, bins as i64, frames as i64, 2] } else { vec![2, bins as i64, frames as i64, 2] };
        let res = onnx.run(vec![Tensor::new(shape, spec)])?;
        let Some(y) = res.into_iter().next() else { return Err(MlError::Model(id.into(), "no output".into())) };
        if y.data.len() != s.stems * 2 * bins * frames * 2 {
            return Err(MlError::Model(id.into(), format!("output shape {:?}, expected [{}, 2, {bins}, {frames}, 2]", y.shape, s.stems)));
        }
        Ok(waveforms(&stft, &y.data, s.stems, frames, s.chunk))
    })?;
    let note = onnx.fallback_note.clone().unwrap_or_else(|| onnx.device_label().to_string());
    Ok((out, note))
}

/// `a - b`, channel by channel (the complement of a stem: instrumental = mix - vocals).
pub fn minus(a: &[Vec<f32>], b: &[Vec<f32>]) -> Vec<Vec<f32>> {
    a.iter()
        .enumerate()
        .map(|(c, ac)| ac.iter().enumerate().map(|(i, x)| x - b.get(c).and_then(|bc| bc.get(i)).copied().unwrap_or(0.0)).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrogram_and_back_is_transparent() {
        // An identity "graph" (output = input spectrogram) gives the chunk back, per stem.
        let stft = Stft::new(64, 16);
        let len = 640;
        let part: Vec<Vec<f32>> = (0..2).map(|c| (0..len).map(|i| ((i as f32) * 0.05 + c as f32).sin() * 0.3).collect()).collect();
        let frames = stft.frames(len);
        let spec = spectrogram(&stft, &part, frames);
        let two = [spec.clone(), spec].concat();
        let y = waveforms(&stft, &two, 2, frames, len);
        assert_eq!(y.len(), 2);
        for stem in &y {
            for (a, b) in stem.iter().zip(&part) {
                let err = a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
                assert!(err < 1e-4, "{err}");
            }
        }
    }

    #[test]
    fn minus_is_the_complement() {
        let mix = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let v = vec![vec![0.5, 0.5], vec![1.0]];
        assert_eq!(minus(&mix, &v), vec![vec![0.5, 1.5], vec![2.0, 4.0]]);
    }
}
