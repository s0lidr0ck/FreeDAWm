//! Mel-Band RoFormer vocals (Kimberley Jensen, MIT), 44.1 kHz stereo, STFT on the host: the same
//! graph as [`crate::karaoke`] with other weights, run through [`crate::roformer`] (2 overlaps).
//! Port of `tools/ml-export/melband_vocals/reference_infer.py`. The graph gives the vocals;
//! instrumental = input - vocals.
//!
//! The vocal source of Make Karaoke, of Separate Stems' `best` mode and of Convert to MIDI's
//! vocal isolation, when installed (HTDemucs' vocals otherwise).

use crate::Result;
use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::roformer::{self, Settings};
use crate::runtime::Device;

pub const ID: &str = "vocals_melroformer";

/// The graph's defaults (the card overrides them).
pub const DEFAULTS: Settings = Settings { chunk: 352_800, overlaps: 2, n_fft: 2048, hop: 441, stems: 1, batched: false };

/// Separate `mix` (`[2][frames]` at 44.1 kHz) into `(vocals, instrumental)` (`[2][frames]` each)
/// and the device note.
pub fn run(model: &InstalledModel, mix: &[Vec<f32>], device: Device, ctx: &Ctx) -> Result<((Vec<Vec<f32>>, Vec<Vec<f32>>), String)> {
    let s = Settings::from_card(model, ID, DEFAULTS)?;
    let x = crate::to_channels(mix, 2);
    let (out, note) = roformer::run(model, ID, s, &x, device, ctx, "Separating the vocals")?;
    let vocals = out.into_iter().next().unwrap_or_else(|| vec![Vec::new(), Vec::new()]);
    let instrumental = roformer::minus(&x, &vocals);
    Ok(((vocals, instrumental), note))
}
