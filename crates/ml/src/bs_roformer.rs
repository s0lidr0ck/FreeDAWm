//! BS-RoFormer SW (6 stems: bass, drums, other, vocals, guitar, piano), 44.1 kHz stereo, STFT on
//! the host (2048 / 512), run through [`crate::roformer`]: 9.29 s chunks, 2 overlaps. Port of
//! `tools/ml-export/bs_roformer_sw/reference_infer.py`. Weights: licence unknown, personal use.

use crate::Result;
use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::roformer::{self, Settings};
use crate::runtime::Device;

pub const ID: &str = "bs_roformer_sw";

/// The stems in the graph's output order, unless the card says otherwise.
pub const DEFAULT_STEMS: [&str; 6] = ["bass", "drums", "other", "vocals", "guitar", "piano"];

/// The graph's defaults (the card overrides chunk, overlap and STFT).
pub const DEFAULTS: Settings = Settings { chunk: 409_600, overlaps: 2, n_fft: 2048, hop: 512, stems: 6, batched: false };

/// Separate `mix` (`[2][frames]` at 44.1 kHz): `(stem name, [2][frames])` in the card's order, and
/// the device note.
pub fn separate(model: &InstalledModel, mix: &[Vec<f32>], device: Device, ctx: &Ctx) -> Result<(Vec<(String, Vec<Vec<f32>>)>, String)> {
    let names: Vec<String> =
        if model.card.stems.is_empty() { DEFAULT_STEMS.iter().map(|s| s.to_string()).collect() } else { model.card.stems.clone() };
    let s = Settings::from_card(model, ID, Settings { stems: names.len(), ..DEFAULTS })?;
    let (out, note) = roformer::run(model, ID, s, mix, device, ctx, "Separating six stems")?;
    Ok((names.into_iter().zip(out).collect(), note))
}
