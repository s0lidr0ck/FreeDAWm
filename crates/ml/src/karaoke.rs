//! Mel-Band RoFormer karaoke (lead vs backing vocals), 44.1 kHz stereo, STFT on the host. Port of
//! `tools/ml-export/karaoke/reference_infer.py`: ZFTurbo MSS `demix` (generic mode, 4 overlaps)
//! over 8 s chunks; each chunk is STFT'd (2048 / 441, periodic Hann, centred), masked by the
//! graph and iSTFT'd. The graph gives the lead vocal; backing = input - lead.

use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::roformer::{self, Settings};
use crate::runtime::Device;
use crate::{MlError, Result};

pub const ID: &str = "karaoke_melroformer";

/// The karaoke graph's defaults (the card overrides them).
pub const DEFAULTS: Settings = Settings { chunk: 352_800, overlaps: 4, n_fft: 2048, hop: 441, stems: 1, batched: false };

/// Split `vocals` (`[2][frames]` at 44.1 kHz) into `("lead_vocal", …)` and `("backing", …)`.
pub fn run(model: &InstalledModel, vocals: &[Vec<f32>], device: Device, ctx: &Ctx) -> Result<(Vec<(String, Vec<Vec<f32>>)>, String)> {
    let s = Settings::from_card(model, ID, DEFAULTS)?;
    let x = crate::to_channels(vocals, 2);
    let (out, note) = roformer::run(model, ID, s, &x, device, ctx, "Splitting lead and backing vocals")?;
    let lead = out.into_iter().next().unwrap_or_else(|| vec![Vec::new(), Vec::new()]);
    let backing = roformer::minus(&x, &lead);
    Ok((vec![("lead_vocal".into(), lead), ("backing".into(), backing)], note))
}

/// The lead and backing vocals among the model's stems: the stem named like "lead"/"vocals" is
/// the lead, the other one the backing.
pub fn lead_and_backing(parts: &[(String, Vec<Vec<f32>>)]) -> Result<(&Vec<Vec<f32>>, &Vec<Vec<f32>>)> {
    let is_lead = |n: &str| {
        let n = n.to_ascii_lowercase();
        n.contains("lead") || n == "karaoke" || n == "vocals"
    };
    let lead = parts.iter().position(|(n, _)| is_lead(n)).unwrap_or(0);
    let backing = (0..parts.len()).find(|i| *i != lead);
    match (parts.get(lead), backing.and_then(|b| parts.get(b))) {
        (Some(l), Some(b)) => Ok((&l.1, &b.1)),
        _ => Err(MlError::Model(ID.into(), "expected two stems (lead and backing vocals)".into())),
    }
}

/// [`lead_and_backing`], taking the two stems out of `parts`.
pub fn take_lead_and_backing(parts: &mut Vec<(String, Vec<Vec<f32>>)>) -> Result<(Vec<Vec<f32>>, Vec<Vec<f32>>)> {
    let is_lead = |n: &str| {
        let n = n.to_ascii_lowercase();
        n.contains("lead") || n == "karaoke" || n == "vocals"
    };
    if parts.len() < 2 {
        return Err(MlError::Model(ID.into(), "expected two stems (lead and backing vocals)".into()));
    }
    let li = parts.iter().position(|(n, _)| is_lead(n)).unwrap_or(0);
    let lead = parts.remove(li).1;
    let backing = parts.remove(0).1;
    Ok((lead, backing))
}
