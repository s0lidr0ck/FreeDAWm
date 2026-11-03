//! Vocal de-noise: DeepFilterNet 3, 48 kHz.
//!
//! The model (`assets/DeepFilterNet3_onnx.tar.gz`, from the DeepFilterNet repository, MIT /
//! Apache-2.0) is built into the app. Its DSP is libDF's own (`deep_filter`: the Vorbis-window
//! STFT, ERB bands, the exponential feature normalisation, synthesis); its three graphs (encoder,
//! ERB-gain decoder, deep-filter decoder) run on ONNX Runtime on the CPU over the whole clip at once
//! (as DeepFilterNet's Python `enhance` does), and each frame is then treated as libDF's streaming
//! `DfTract::process` treats it: the local-SNR stages (no processing on clean frames, a zero mask on
//! noise-only frames, ERB gains, deep filtering of the low 96 bins over 5 frames with 2 of
//! lookahead) and the attenuation limit, which mixes the noisy spectrum back in so noise drops by at
//! most that many dB. libDF's tract path is not used: tract fails to optimise these graphs
//! ("duplicate name /convt3/Conv.bias", tract 0.21.4–0.21.6).
//!
//! 100 dB is unlimited; 12–24 dB is gentle and suits singing, which the speech-trained model can
//! otherwise thin out.

use crate::job::Ctx;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::{MlError, Result};
use df::{Complex32, DFState};

/// DeepFilterNet's sample rate.
pub const SAMPLE_RATE: u32 = 48_000;

/// The default attenuation limit for vocals, dB.
pub const DEFAULT_ATTEN_DB: f32 = 24.0;

const MODEL_TAR: &[u8] = include_bytes!("../assets/DeepFilterNet3_onnx.tar.gz");

// From the model's config.ini ([df] and [deepfilternet]).
const FFT: usize = 960;
const HOP: usize = 480;
const NB_ERB: usize = 32;
const NB_DF: usize = 96;
const MIN_NB_ERB_FREQS: usize = 2;
const DF_ORDER: usize = 5;
const DF_LOOKAHEAD: usize = 2;
const CONV_LOOKAHEAD: usize = 2;
/// libDF's `calc_norm_alpha(48000, 480, norm_tau = 1)`.
const ALPHA: f32 = 0.99;
// libDF's RuntimeParams defaults.
const MIN_DB_THRESH: f32 = -10.0;
const MAX_DB_ERB_THRESH: f32 = 30.0;
const MAX_DB_DF_THRESH: f32 = 20.0;
/// Frames per encoder run, and frames of earlier context each run starts with (its GRU warm-up).
const SEGMENT: usize = 6000;
const CONTEXT: usize = 500;

fn df_err(e: impl std::fmt::Display) -> MlError {
    MlError::Runtime(format!("DeepFilterNet: {e}"))
}

/// The three graphs from the built-in model archive: (enc, erb_dec, df_dec).
fn graphs() -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    use std::io::Read;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(MODEL_TAR));
    let (mut enc, mut erb, mut dfd) = (Vec::new(), Vec::new(), Vec::new());
    for entry in archive.entries().map_err(df_err)? {
        let mut entry = entry.map_err(df_err)?;
        let path = entry.path().map_err(df_err)?.to_string_lossy().into_owned();
        let target = if path.ends_with("erb_dec.onnx") {
            &mut erb
        } else if path.ends_with("df_dec.onnx") {
            &mut dfd
        } else if path.ends_with("enc.onnx") {
            &mut enc
        } else {
            continue;
        };
        entry.read_to_end(target).map_err(df_err)?;
    }
    if enc.is_empty() || erb.is_empty() || dfd.is_empty() {
        return Err(MlError::Model("deepfilternet3".into(), "the built-in model archive is incomplete".into()));
    }
    Ok((enc, erb, dfd))
}

/// libDF's `apply_stages`: (apply ERB gains, apply a zero mask, apply deep filtering).
fn stages(lsnr: f32) -> (bool, bool, bool) {
    if lsnr < MIN_DB_THRESH {
        (false, true, false)
    } else if lsnr > MAX_DB_ERB_THRESH {
        (false, false, false)
    } else if lsnr > MAX_DB_DF_THRESH {
        (true, false, false)
    } else {
        (true, false, true)
    }
}

/// The model's per-frame outputs for one channel: local SNR, ERB gains, DF coefficients
/// (`[nb_df][order]` complex), indexed by raw (un-shifted) frame.
struct Raw {
    lsnr: Vec<f32>,
    gains: Vec<f32>,
    coefs: Vec<f32>,
}

/// Runs the three graphs over `erb` (`[frame][NB_ERB]`) and `cplx` (`[frame][NB_DF]`) in
/// segments with warm-up context.
fn infer(models: &mut [OnnxModel; 3], erb: &[f32], cplx: &[Complex32], frames: usize, ctx: &Ctx) -> Result<Raw> {
    let [enc, erb_dec, df_dec] = models;
    let mut raw = Raw { lsnr: vec![0.0; frames], gains: vec![0.0; frames * NB_ERB], coefs: vec![0.0; frames * NB_DF * DF_ORDER * 2] };
    let n_seg = frames.div_ceil(SEGMENT).max(1);
    let (lsnr_i, emb_i, c0_i) = (enc.output_index("lsnr")?, enc.output_index("emb")?, enc.output_index("c0")?);
    let e_i = [enc.output_index("e0")?, enc.output_index("e1")?, enc.output_index("e2")?, enc.output_index("e3")?];
    for seg in 0..n_seg {
        ctx.check()?;
        let a = seg * SEGMENT;
        let b = (a + SEGMENT).min(frames);
        let from = a.saturating_sub(CONTEXT);
        let s = b - from;
        let feat_erb: Vec<f32> = erb.get(from * NB_ERB..b * NB_ERB).unwrap_or_default().to_vec();
        // feat_spec is [1, 2, S, nb_df]: real parts, then imaginary parts.
        let mut feat_spec = vec![0.0f32; 2 * s * NB_DF];
        for t in 0..s {
            for f in 0..NB_DF {
                let v = cplx.get((from + t) * NB_DF + f).copied().unwrap_or_default();
                if let Some(x) = feat_spec.get_mut(t * NB_DF + f) {
                    *x = v.re;
                }
                if let Some(x) = feat_spec.get_mut((s + t) * NB_DF + f) {
                    *x = v.im;
                }
            }
        }
        let si = s as i64;
        let out = enc.run_named(&[
            ("feat_erb", &Tensor::new(vec![1, 1, si, NB_ERB as i64], feat_erb)),
            ("feat_spec", &Tensor::new(vec![1, 2, si, NB_DF as i64], feat_spec)),
        ])?;
        let get = |i: usize| out.get(i).cloned().ok_or_else(|| df_err("missing encoder output"));
        let (lsnr, emb, c0) = (get(lsnr_i)?, get(emb_i)?, get(c0_i)?);
        let [e0, e1, e2, e3] = [get(e_i[0])?, get(e_i[1])?, get(e_i[2])?, get(e_i[3])?];
        let m = erb_dec.run_named(&[("emb", &emb), ("e3", &e3), ("e2", &e2), ("e1", &e1), ("e0", &e0)])?;
        let m = m.into_iter().next().ok_or_else(|| df_err("no ERB gains"))?;
        let coefs = df_dec.run_named(&[("emb", &emb), ("c0", &c0)])?;
        let coefs = coefs.into_iter().next().ok_or_else(|| df_err("no DF coefficients"))?;
        if lsnr.data.len() != s || m.data.len() != s * NB_ERB || coefs.data.len() != s * NB_DF * DF_ORDER * 2 {
            return Err(df_err(format!("unexpected output shapes {:?} {:?} {:?}", lsnr.shape, m.shape, coefs.shape)));
        }
        let skip = a - from;
        for t in skip..s {
            let g = from + t;
            if let (Some(o), Some(v)) = (raw.lsnr.get_mut(g), lsnr.data.get(t)) {
                *o = *v;
            }
            let (src, dst) = (m.data.get(t * NB_ERB..(t + 1) * NB_ERB), raw.gains.get_mut(g * NB_ERB..(g + 1) * NB_ERB));
            if let (Some(src), Some(dst)) = (src, dst) {
                dst.copy_from_slice(src);
            }
            let w = NB_DF * DF_ORDER * 2;
            let (src, dst) = (coefs.data.get(t * w..(t + 1) * w), raw.coefs.get_mut(g * w..(g + 1) * w));
            if let (Some(src), Some(dst)) = (src, dst) {
                dst.copy_from_slice(src);
            }
        }
        ctx.report(0.1 + 0.8 * (seg + 1) as f32 / n_seg as f32);
    }
    Ok(raw)
}

/// De-noise `audio` (`[channel][frame]` at 48 kHz) with an attenuation limit of `atten_db` dB.
/// The result is sample-aligned with the input and as long. Runs on the CPU.
pub fn denoise_48k(audio: &[Vec<f32>], atten_db: f32, ctx: &Ctx) -> Result<Vec<Vec<f32>>> {
    denoise_48k_with(audio, atten_db, true, ctx)
}

/// [`denoise_48k`]; with `snr_stages` false every frame gets the ERB gains and deep filtering
/// (DeepFilterNet's Python `enhance`), not libDF's local-SNR stages.
pub fn denoise_48k_with(audio: &[Vec<f32>], atten_db: f32, snr_stages: bool, ctx: &Ctx) -> Result<Vec<Vec<f32>>> {
    let channels = audio.len();
    let len = crate::frames(audio);
    if channels == 0 || len == 0 {
        return Ok(audio.to_vec());
    }
    let atten = if atten_db.is_finite() { atten_db.abs().min(100.0) } else { DEFAULT_ATTEN_DB };
    if atten < 0.01 {
        return Ok(crate::fit(audio.to_vec(), len));
    }
    // libDF: no limit at 100 dB, else mix back 10^(-dB/20) of the noisy spectrum.
    let lim = if atten >= 100.0 { None } else { Some(10f32.powf(-atten / 20.0)) };
    ctx.check()?;
    let (enc, erb, dfd) = graphs()?;
    let mut models = [
        OnnxModel::open_bytes("deepfilternet3 enc", &enc, Device::Cpu)?,
        OnnxModel::open_bytes("deepfilternet3 erb_dec", &erb, Device::Cpu)?,
        OnnxModel::open_bytes("deepfilternet3 df_dec", &dfd, Device::Cpu)?,
    ];
    // Synthesis delays the output by FFT - HOP; the model outputs for frame t come CONV_LOOKAHEAD
    // frames later. Pad the end so every input sample has its output.
    let delay = FFT - HOP;
    let frames = (len + delay).div_ceil(HOP) + CONV_LOOKAHEAD + DF_LOOKAHEAD + 1;
    let n_freqs = FFT / 2 + 1;
    let mut gains_mean = vec![0.0f32; frames * NB_ERB];
    let mut per_channel = Vec::with_capacity(channels);
    for (c, ch) in audio.iter().enumerate() {
        let cctx = ctx.stage(c as f32 / channels as f32, (c as f32 + 0.9) / channels as f32);
        cctx.say("De-noising (DeepFilterNet 3)");
        let mut st = DFState::new(SAMPLE_RATE as usize, FFT, HOP, NB_ERB, MIN_NB_ERB_FREQS);
        st.init_norm_states(NB_DF);
        let mut spec = vec![Complex32::default(); frames * n_freqs];
        let mut erb = vec![0.0f32; frames * NB_ERB];
        let mut cplx = vec![Complex32::default(); frames * NB_DF];
        let mut frame = vec![0.0f32; HOP];
        for t in 0..frames {
            for (k, x) in frame.iter_mut().enumerate() {
                *x = ch.get(t * HOP + k).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            }
            let (Some(sp), Some(e), Some(cx)) =
                (spec.get_mut(t * n_freqs..(t + 1) * n_freqs), erb.get_mut(t * NB_ERB..(t + 1) * NB_ERB), cplx.get_mut(t * NB_DF..(t + 1) * NB_DF))
            else {
                return Err(df_err("buffer sizes"));
            };
            st.analysis(&frame, sp);
            st.feat_erb(sp, ALPHA, e);
            st.feat_cplx(sp.get(..NB_DF).unwrap_or_default(), ALPHA, cx);
        }
        cctx.report(0.1);
        let raw = infer(&mut models, &erb, &cplx, frames, &cctx)?;
        for (m, g) in gains_mean.iter_mut().zip(&raw.gains) {
            *m += g / channels as f32;
        }
        per_channel.push((st, spec, raw));
    }
    let mut out = Vec::with_capacity(channels);
    let multi = channels > 1;
    for (st, spec, raw) in &mut per_channel {
        ctx.check()?;
        let mut y = vec![0.0f32; frames * HOP];
        let mut buf = vec![Complex32::default(); n_freqs];
        let mut hop_out = vec![0.0f32; HOP];
        let at = |t: isize, f: usize| -> Complex32 {
            usize::try_from(t).ok().filter(|t| *t < frames).and_then(|t| spec.get(t * n_freqs + f)).copied().unwrap_or_default()
        };
        for t in 0..frames {
            // The model's outputs for frame t (it looks CONV_LOOKAHEAD frames ahead).
            let r = t + CONV_LOOKAHEAD;
            let lsnr = raw.lsnr.get(r).copied().unwrap_or(-15.0);
            let (apply_gains, zeros, apply_df) = match (r < frames, snr_stages) {
                (false, _) => (false, false, false),
                (true, true) => stages(lsnr),
                (true, false) => (true, false, true),
            };
            for (f, b) in buf.iter_mut().enumerate() {
                *b = at(t as isize, f);
            }
            if apply_gains {
                // ReduceMask::MEAN: one mask for all channels.
                let g = if multi { gains_mean.get(r * NB_ERB..(r + 1) * NB_ERB) } else { raw.gains.get(r * NB_ERB..(r + 1) * NB_ERB) };
                st.apply_mask(&mut buf, g.unwrap_or(&[0.0; NB_ERB]));
            } else if zeros {
                st.apply_mask(&mut buf, &[0.0; NB_ERB]);
            }
            if apply_df {
                let w = NB_DF * DF_ORDER * 2;
                let c = raw.coefs.get(r * w..(r + 1) * w).unwrap_or_default();
                for f in 0..NB_DF {
                    let mut acc = Complex32::default();
                    for n in 0..DF_ORDER {
                        let k = (f * DF_ORDER + n) * 2;
                        let coef = Complex32::new(c.get(k).copied().unwrap_or(0.0), c.get(k + 1).copied().unwrap_or(0.0));
                        acc += at(t as isize - (DF_ORDER - 1 - DF_LOOKAHEAD) as isize + n as isize, f) * coef;
                    }
                    if let Some(b) = buf.get_mut(f) {
                        *b = acc;
                    }
                }
            }
            if let Some(lim) = lim {
                for (f, b) in buf.iter_mut().enumerate() {
                    *b = *b * (1.0 - lim) + at(t as isize, f) * lim;
                }
            }
            st.synthesis(&mut buf, &mut hop_out);
            if let Some(dst) = y.get_mut(t * HOP..(t + 1) * HOP) {
                dst.copy_from_slice(&hop_out);
            }
        }
        out.push((0..len).map(|i| y.get(i + delay).copied().filter(|v| v.is_finite()).unwrap_or(0.0)).collect());
    }
    ctx.report(1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sung-like 220 Hz tone with harmonics plus white-ish noise, 2 s at 48 kHz.
    fn noisy_tone() -> (Vec<f32>, Vec<f32>) {
        let mut seed = 1u32;
        let tone: Vec<f32> = (0..96_000)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                let f = 220.0 * (1.0 + 0.01 * (2.0 * std::f32::consts::PI * 5.0 * t).sin());
                (1..6).map(|h| 0.15 / h as f32 * (2.0 * std::f32::consts::PI * f * h as f32 * t).sin()).sum::<f32>()
            })
            .collect();
        let noisy = tone
            .iter()
            .map(|t| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                t + ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.08
            })
            .collect();
        (tone, noisy)
    }

    #[test]
    fn removes_noise_keeps_length_and_alignment() {
        let (tone, noisy) = noisy_tone();
        let out = denoise_48k(std::slice::from_ref(&noisy), 100.0, &Ctx::default()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), noisy.len());
        // Aligned: correlation with the clean tone peaks at lag 0.
        let corr = |lag: i64| -> f32 { (20_000..80_000).map(|i| tone[i] * out[0][(i as i64 + lag) as usize]).sum::<f32>() };
        let best = (-60..=60).max_by(|a, b| corr(*a).total_cmp(&corr(*b))).unwrap();
        assert_eq!(best, 0, "output lags by {best}");
        // Noise-only stretches (the tone stops from 1.0 s to 1.5 s) get much quieter.
        let gap = |x: &[f32]| (50_000..70_000).map(|i| x[i] * x[i]).sum::<f32>();
        let mut gapped = noisy.clone();
        for (i, v) in gapped.iter_mut().enumerate().take(72_000).skip(48_000) {
            *v -= tone[i];
        }
        let full = denoise_48k(&[gapped.clone()], 100.0, &Ctx::default()).unwrap();
        assert!(gap(&full[0]) < gap(&gapped) * 0.1, "{} vs {}", gap(&full[0]), gap(&gapped));
        // A gentle limit removes less: at most 6 dB.
        let gentle = denoise_48k(&[gapped.clone()], 6.0, &Ctx::default()).unwrap();
        assert!(gap(&gentle[0]) > gap(&gapped) * 0.2, "{} vs {}", gap(&gentle[0]), gap(&gapped));
        // Stereo runs and keeps both channels.
        let st = denoise_48k(&[noisy.clone(), noisy.clone()], 24.0, &Ctx::default()).unwrap();
        assert_eq!(st.len(), 2);
        assert_eq!(st[1].len(), noisy.len());
    }

    #[test]
    fn zero_attenuation_is_a_bypass_and_empty_is_fine() {
        let (_, noisy) = noisy_tone();
        let out = denoise_48k(std::slice::from_ref(&noisy), 0.0, &Ctx::default()).unwrap();
        assert_eq!(out[0], noisy);
        assert_eq!(denoise_48k(&[], 24.0, &Ctx::default()).unwrap(), Vec::<Vec<f32>>::new());
        let c = Ctx::default();
        c.cancel_flag().cancel();
        assert_eq!(denoise_48k(&[noisy], 24.0, &c), Err(MlError::Cancelled));
    }
}
