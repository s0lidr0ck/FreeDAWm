//! HTDemucs v4 (4 stems: drums, bass, other, vocals), a self-contained ONNX graph (its STFT is
//! inside), 44.1 kHz stereo. Port of `ml-models/htdemucs/reference_infer.py`.

use crate::chunk::{MAX_SHIFT_SECONDS, Quality, SHIFT_SEED, demucs_shifts};
use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::{MlError, Result};

pub const ID: &str = "htdemucs";

/// The stems in the model's output order, unless the card says otherwise.
pub const DEFAULT_STEMS: [&str; 4] = ["drums", "bass", "other", "vocals"];

/// Separate `mix` (`[2][frames]` at the model rate) at `quality` (High: shift averaging, see
/// [`crate::chunk::demucs_shifts`]). Returns `(stem name, [2][frames])` in the card's stem order,
/// plus the device it ran on.
pub fn separate(
    model: &InstalledModel,
    mix: &[Vec<f32>],
    quality: Quality,
    device: Device,
    ctx: &Ctx,
) -> Result<(Vec<(String, Vec<Vec<f32>>)>, String)> {
    let card = &model.card;
    let rate = card.sample_rate.max(1) as f64;
    // chunk.samples (343980 = 7.8 s), or `segment` in seconds.
    let segment = card
        .value("samples")
        .or_else(|| card.value("segment_samples"))
        .and_then(serde_json::Value::as_u64)
        .map_or_else(|| (card.num("segment", 7.8) * rate).round() as usize, |v| usize::try_from(v).unwrap_or(0));
    if segment == 0 || segment > 30 * 48_000 {
        return Err(MlError::Model(ID.into(), format!("bad segment length {segment}")));
    }
    let overlap = quality.overlap(card.num("overlap", 0.25) as f32);
    let shifts = if quality == Quality::High { crate::chunk::HIGH_SHIFTS } else { 0 };
    let max_shift = (MAX_SHIFT_SECONDS * rate) as usize;
    let power = card.num("transition_power", 1.0) as f32;
    let stems: Vec<String> = if card.stems.is_empty() { DEFAULT_STEMS.iter().map(|s| s.to_string()).collect() } else { card.stems.clone() };
    let mix = crate::to_channels(mix, 2);
    let len = crate::frames(&mix);
    if len == 0 {
        return Ok((stems.into_iter().map(|s| (s, vec![Vec::new(), Vec::new()])).collect(), String::new()));
    }
    // Normalise by the mono reference: mu = mean, sd = std (ddof 1) + 1e-8; undone at the end.
    let mono: Vec<f64> = (0..len).map(|i| f64::from(mix.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / 2.0)).collect();
    let mean = mono.iter().sum::<f64>() / len as f64;
    let var = mono.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (len.saturating_sub(1).max(1)) as f64;
    let (mean, std) = (mean as f32, var.sqrt() as f32 + 1e-8);
    let input: Vec<Vec<f32>> = mix.iter().map(|c| c.iter().map(|v| (v - mean) / std).collect()).collect();
    ctx.say("Loading HTDemucs");
    let mut onnx = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    ctx.say(&format!(
        "Separating on the {}{}",
        onnx.device_label(),
        if quality == Quality::High { format!(" ({shifts} shifted passes)") } else { String::new() }
    ));
    let n_stems = stems.len();
    let out = demucs_shifts(&input, shifts, max_shift, SHIFT_SEED, segment, overlap, power, ctx, |chunk| {
        let seg = crate::frames(chunk);
        let data: Vec<f32> = chunk.iter().flat_map(|c| c.iter().copied()).collect();
        let res = onnx.run(vec![Tensor::new(vec![1, 2, seg as i64], data)])?;
        let Some(t) = res.into_iter().next() else { return Err(MlError::Model(ID.into(), "no output".into())) };
        // [1, stems, 2, seg]
        if t.data.len() != n_stems * 2 * seg {
            return Err(MlError::Model(ID.into(), format!("output shape {:?}, expected [1, {n_stems}, 2, {seg}]", t.shape)));
        }
        Ok((0..n_stems)
            .map(|s| (0..2).map(|c| t.data.get((s * 2 + c) * seg..(s * 2 + c + 1) * seg).unwrap_or_default().to_vec()).collect())
            .collect())
    })?;
    let note = onnx.fallback_note.clone().unwrap_or_else(|| onnx.device_label().to_string());
    let res = stems
        .into_iter()
        .zip(out)
        .map(|(name, s)| (name, s.into_iter().map(|c| c.into_iter().map(|v| v * std + mean).collect()).collect()))
        .collect();
    Ok((res, note))
}
