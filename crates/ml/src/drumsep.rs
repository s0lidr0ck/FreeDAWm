//! MDX23C DrumSep (6 stems: kick, snare, toms, hi-hat, ride, crash), its STFT inside the graph,
//! 44.1 kHz stereo. Port of `tools/ml-export/drumsep/reference_infer.py`: ZFTurbo MSS `demix`
//! (generic mode, 4 overlaps) over 130560-sample chunks.

use crate::chunk::msst_split;
use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::{MlError, Result};

pub const ID: &str = "drumsep_mdx23c";

pub const DEFAULT_STEMS: [&str; 6] = ["kick", "snare", "toms", "hh", "ride", "crash"];

/// A whole number from the card, or `d`.
pub(crate) fn card_usize(card: &crate::ModelCard, key: &str, d: usize) -> usize {
    card.value(key).and_then(serde_json::Value::as_u64).and_then(|v| usize::try_from(v).ok()).unwrap_or(d)
}

/// Split `drums` (`[2][frames]` at 44.1 kHz). Returns `(stem name, [2][frames])` and the device.
pub fn run(model: &InstalledModel, drums: &[Vec<f32>], device: Device, ctx: &Ctx) -> Result<(Vec<(String, Vec<Vec<f32>>)>, String)> {
    let card = &model.card;
    let chunk = card_usize(card, "samples", 130_560);
    let overlaps = card_usize(card, "num_overlap", 4);
    if chunk == 0 || chunk > 10 * 48_000 {
        return Err(MlError::Model(ID.into(), format!("bad chunk length {chunk}")));
    }
    let stems: Vec<String> = if card.stems.is_empty() { DEFAULT_STEMS.iter().map(|s| s.to_string()).collect() } else { card.stems.clone() };
    let x = crate::to_channels(drums, 2);
    ctx.say("Loading DrumSep");
    let mut onnx = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    ctx.say(&format!("Splitting drums on the {}", onnx.device_label()));
    let n_stems = stems.len();
    let out = msst_split(&x, chunk, overlaps, ctx, |part| {
        let data: Vec<f32> = part.iter().flat_map(|c| c.iter().copied()).collect();
        let res = onnx.run(vec![Tensor::new(vec![1, 2, chunk as i64], data)])?;
        let Some(t) = res.into_iter().next() else { return Err(MlError::Model(ID.into(), "no output".into())) };
        if t.data.len() != n_stems * 2 * chunk {
            return Err(MlError::Model(ID.into(), format!("output shape {:?}, expected [1, {n_stems}, 2, {chunk}]", t.shape)));
        }
        Ok((0..n_stems)
            .map(|s| (0..2).map(|c| t.data.get((s * 2 + c) * chunk..(s * 2 + c + 1) * chunk).unwrap_or_default().to_vec()).collect())
            .collect())
    })?;
    let note = onnx.fallback_note.clone().unwrap_or_else(|| onnx.device_label().to_string());
    Ok((stems.into_iter().zip(out).collect(), note))
}
