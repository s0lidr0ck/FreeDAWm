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

/// Frames of earlier input the encoder's first convolutions and the deep-filter decoder's
/// reach back to (the graphs are fed them: `tools/ml-export/dfn/make_stream.py`).
const ENC_LEAD: usize = 2;
const C0_LEAD: usize = 4;
const C0_CH: usize = 64;
const HIDDEN: usize = 256;

/// What the graphs carry from one run to the next: the five GRU states and the frames the
/// convolutions over time reach back to. A fresh one is the start of a sequence.
struct Carry {
    /// enc `h_0`; erb_dec `h_0`, `h_1`; df_dec `h_0`, `h_1`.
    h: [Tensor; 5],
    /// The last [`ENC_LEAD`] frames of the ERB features, and of the complex ones (`[re | im]`,
    /// each `[frame][NB_DF]`).
    erb: Vec<f32>,
    spec: Vec<f32>,
    /// The last [`C0_LEAD`] frames of the encoder's `c0`, `[channel][frame][NB_DF]`.
    c0: Vec<f32>,
}

impl Carry {
    fn new() -> Carry {
        let h = || Tensor::new(vec![1, 1, HIDDEN as i64], vec![0.0; HIDDEN]);
        Carry {
            h: [h(), h(), h(), h(), h()],
            erb: vec![0.0; ENC_LEAD * NB_ERB],
            spec: vec![0.0; 2 * ENC_LEAD * NB_DF],
            c0: vec![0.0; C0_CH * C0_LEAD * NB_DF],
        }
    }
}

fn take(out: &mut [Tensor], i: usize) -> Result<Tensor> {
    out.get_mut(i).map(|t| std::mem::replace(t, Tensor::new(Vec::new(), Vec::new()))).ok_or_else(|| df_err("missing output"))
}

/// Runs the three graphs over the next `s` frames of a sequence (`erb`: `[frame][NB_ERB]`,
/// `cplx`: `[frame][NB_DF]`), carrying `carry` on.
fn step(models: &mut [OnnxModel; 3], carry: &mut Carry, erb: &[f32], cplx: &[Complex32], s: usize) -> Result<Raw> {
    let [enc, erb_dec, df_dec] = models;
    let n = s + ENC_LEAD;
    let mut feat_erb = Vec::with_capacity(n * NB_ERB);
    feat_erb.extend_from_slice(&carry.erb);
    feat_erb.extend_from_slice(erb.get(..s * NB_ERB).ok_or_else(|| df_err("ERB features"))?);
    // feat_spec is [1, 2, n, nb_df]: real parts, then imaginary parts.
    let mut feat_spec = vec![0.0f32; 2 * n * NB_DF];
    let lead = ENC_LEAD * NB_DF;
    for part in 0..2 {
        let (Some(dst), Some(src)) = (feat_spec.get_mut(part * n * NB_DF..(part + 1) * n * NB_DF), carry.spec.get(part * lead..(part + 1) * lead))
        else {
            return Err(df_err("buffer sizes"));
        };
        if let Some(d) = dst.get_mut(..lead) {
            d.copy_from_slice(src);
        }
        for (d, v) in dst.iter_mut().skip(lead).zip(cplx) {
            *d = if part == 0 { v.re } else { v.im };
        }
    }
    // What the next run reaches back to.
    carry.erb = feat_erb.get(s * NB_ERB..).unwrap_or_default().to_vec();
    for part in 0..2 {
        let from = part * n * NB_DF + s * NB_DF;
        if let (Some(src), Some(dst)) = (feat_spec.get(from..from + lead), carry.spec.get_mut(part * lead..(part + 1) * lead)) {
            dst.copy_from_slice(src);
        }
    }
    let (ni, si) = (n as i64, s as i64);
    let idx = |m: &OnnxModel, name: &str| m.output_index(name);
    let mut out = enc.run_named(&[
        ("feat_erb", &Tensor::new(vec![1, 1, ni, NB_ERB as i64], feat_erb)),
        ("feat_spec", &Tensor::new(vec![1, 2, ni, NB_DF as i64], feat_spec)),
        ("h_0", &carry.h[0]),
    ])?;
    let (lsnr, emb, c0) = (take(&mut out, idx(enc, "lsnr")?)?, take(&mut out, idx(enc, "emb")?)?, take(&mut out, idx(enc, "c0")?)?);
    let [e0, e1, e2, e3] =
        [take(&mut out, idx(enc, "e0")?)?, take(&mut out, idx(enc, "e1")?)?, take(&mut out, idx(enc, "e2")?)?, take(&mut out, idx(enc, "e3")?)?];
    carry.h[0] = take(&mut out, idx(enc, "h_0_out")?)?;
    let mut m =
        erb_dec.run_named(&[("emb", &emb), ("e3", &e3), ("e2", &e2), ("e1", &e1), ("e0", &e0), ("h_0", &carry.h[1]), ("h_1", &carry.h[2])])?;
    let gains = take(&mut m, idx(erb_dec, "m")?)?;
    carry.h[1] = take(&mut m, idx(erb_dec, "h_0_out")?)?;
    carry.h[2] = take(&mut m, idx(erb_dec, "h_1_out")?)?;
    // c0 is [1, C0_CH, s, nb_df]: each channel gets its earlier frames in front.
    if c0.data.len() != C0_CH * s * NB_DF {
        return Err(df_err(format!("unexpected c0 shape {:?}", c0.shape)));
    }
    let (old, new) = (C0_LEAD * NB_DF, s * NB_DF);
    let mut c0_in = Vec::with_capacity(C0_CH * (old + new));
    for ch in 0..C0_CH {
        c0_in.extend_from_slice(carry.c0.get(ch * old..(ch + 1) * old).unwrap_or_default());
        c0_in.extend_from_slice(c0.data.get(ch * new..(ch + 1) * new).unwrap_or_default());
    }
    for ch in 0..C0_CH {
        let from = ch * (old + new) + new;
        if let (Some(src), Some(dst)) = (c0_in.get(from..from + old), carry.c0.get_mut(ch * old..(ch + 1) * old)) {
            dst.copy_from_slice(src);
        }
    }
    let c0_in = Tensor::new(vec![1, C0_CH as i64, si + C0_LEAD as i64, NB_DF as i64], c0_in);
    let mut d = df_dec.run_named(&[("emb", &emb), ("c0", &c0_in), ("h_0", &carry.h[3]), ("h_1", &carry.h[4])])?;
    let coefs = take(&mut d, idx(df_dec, "coefs")?)?;
    carry.h[3] = take(&mut d, idx(df_dec, "h_0_out")?)?;
    carry.h[4] = take(&mut d, idx(df_dec, "h_1_out")?)?;
    if lsnr.data.len() != s || gains.data.len() != s * NB_ERB || coefs.data.len() != s * NB_DF * DF_ORDER * 2 {
        return Err(df_err(format!("unexpected output shapes {:?} {:?} {:?}", lsnr.shape, gains.shape, coefs.shape)));
    }
    Ok(Raw { lsnr: lsnr.data, gains: gains.data, coefs: coefs.data })
}

/// Runs the three graphs over `erb` (`[frame][NB_ERB]`) and `cplx` (`[frame][NB_DF]`) in
/// segments, each started afresh on [`CONTEXT`] earlier frames (its GRUs' warm-up).
fn infer(models: &mut [OnnxModel; 3], erb: &[f32], cplx: &[Complex32], frames: usize, ctx: &Ctx) -> Result<Raw> {
    let mut raw = Raw { lsnr: vec![0.0; frames], gains: vec![0.0; frames * NB_ERB], coefs: vec![0.0; frames * NB_DF * DF_ORDER * 2] };
    let n_seg = frames.div_ceil(SEGMENT).max(1);
    let w = NB_DF * DF_ORDER * 2;
    for seg in 0..n_seg {
        ctx.check()?;
        let a = seg * SEGMENT;
        let b = (a + SEGMENT).min(frames);
        let from = a.saturating_sub(CONTEXT);
        let s = b - from;
        let part = step(
            models,
            &mut Carry::new(),
            erb.get(from * NB_ERB..b * NB_ERB).unwrap_or_default(),
            cplx.get(from * NB_DF..b * NB_DF).unwrap_or_default(),
            s,
        )?;
        let skip = a - from;
        let n = s - skip;
        if let (Some(dst), Some(src)) = (raw.lsnr.get_mut(a..b), part.lsnr.get(skip..)) {
            dst.copy_from_slice(src);
        }
        if let (Some(dst), Some(src)) = (raw.gains.get_mut(a * NB_ERB..b * NB_ERB), part.gains.get(skip * NB_ERB..(skip + n) * NB_ERB)) {
            dst.copy_from_slice(src);
        }
        if let (Some(dst), Some(src)) = (raw.coefs.get_mut(a * w..b * w), part.coefs.get(skip * w..(skip + n) * w)) {
            dst.copy_from_slice(src);
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

/// The three graphs, loaded: for [`Stream`].
pub struct Models([OnnxModel; 3]);

impl Models {
    /// Loads the built-in model on the CPU.
    pub fn open() -> Result<Models> {
        let (enc, erb, dfd) = graphs()?;
        Ok(Models([
            OnnxModel::open_bytes("deepfilternet3 enc", &enc, Device::Cpu)?,
            OnnxModel::open_bytes("deepfilternet3 erb_dec", &erb, Device::Cpu)?,
            OnnxModel::open_bytes("deepfilternet3 df_dec", &dfd, Device::Cpu)?,
        ]))
    }
}

/// The model's hop: what [`Stream::push`] takes and each frame of [`Stream::pull`] gives.
pub const STREAM_HOP: usize = HOP;
/// How many hops must be pushed beyond the last one wanted back before [`Stream::pull`] can
/// give it: the synthesis delay (one hop) and the model's look-ahead (two).
pub const STREAM_LOOK: usize = 1 + CONV_LOOKAHEAD;

struct StreamChannel {
    st: DFState,
    carry: Carry,
    /// Spectra of frames `base..`, and the features of the frames the graphs have not had yet.
    spec: Vec<Complex32>,
    erb: Vec<f32>,
    cplx: Vec<Complex32>,
}

/// DeepFilterNet 3 a few frames at a time, for live use ([`crate::live`]): the same analysis,
/// stages, attenuation limit and synthesis as [`denoise_48k`]. The graphs carry their
/// state from one [`Stream::pull`] to the next, so the result is that of one pass over the
/// whole sound, at the cost of the new frames only.
pub struct Stream {
    ch: Vec<StreamChannel>,
    /// The first frame the channels' spectra still hold, frames pushed and frames pulled.
    base: usize,
    pushed: usize,
    pulled: usize,
    /// How much of the noisy spectrum is mixed back (`None`: no limit).
    lim: Option<f32>,
}

impl Stream {
    /// A stream of `channels` channels that takes noise down by at most `atten_db` dB (100: no
    /// limit).
    pub fn new(channels: usize, atten_db: f32) -> Stream {
        let atten = if atten_db.is_finite() { atten_db.abs().min(100.0) } else { DEFAULT_ATTEN_DB };
        let lim = (atten < 100.0).then(|| 10f32.powf(-atten / 20.0));
        let ch = (0..channels.max(1))
            .map(|_| {
                let mut st = DFState::new(SAMPLE_RATE as usize, FFT, HOP, NB_ERB, MIN_NB_ERB_FREQS);
                st.init_norm_states(NB_DF);
                StreamChannel { st, carry: Carry::new(), spec: Vec::new(), erb: Vec::new(), cplx: Vec::new() }
            })
            .collect();
        Stream { ch, base: 0, pushed: 0, pulled: 0, lim }
    }

    /// Takes one hop ([`STREAM_HOP`] samples) of every channel.
    pub fn push(&mut self, hop: &[&[f32]]) {
        let n_freqs = FFT / 2 + 1;
        let mut frame = [0.0f32; HOP];
        for (c, ch) in self.ch.iter_mut().enumerate() {
            let src = hop.get(c).or(hop.first()).copied().unwrap_or(&[]);
            for (k, x) in frame.iter_mut().enumerate() {
                *x = src.get(k).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            }
            let (s0, e0, c0) = (ch.spec.len(), ch.erb.len(), ch.cplx.len());
            ch.spec.resize(s0 + n_freqs, Complex32::default());
            ch.erb.resize(e0 + NB_ERB, 0.0);
            ch.cplx.resize(c0 + NB_DF, Complex32::default());
            let (Some(sp), Some(e), Some(cx)) = (ch.spec.get_mut(s0..), ch.erb.get_mut(e0..), ch.cplx.get_mut(c0..)) else { continue };
            ch.st.analysis(&frame, sp);
            ch.st.feat_erb(sp, ALPHA, e);
            ch.st.feat_cplx(sp.get(..NB_DF).unwrap_or_default(), ALPHA, cx);
        }
        self.pushed += 1;
    }

    /// How many frames [`Stream::pull`] can give now.
    pub fn ready(&self) -> usize {
        (self.pushed + 1).saturating_sub(self.pulled + STREAM_LOOK)
    }

    /// Every frame it can give: `[channel][frames * STREAM_HOP]`. The first hop ever pulled is
    /// the synthesis's lead-in; after it, hop `k` pulled is hop `k - 1` pushed, de-noised.
    /// `same` says every channel carries the same sound and always has on this stream: the
    /// graphs then run once.
    pub fn pull(&mut self, models: &mut Models, same: bool) -> Result<Vec<Vec<f32>>> {
        let frames = self.ready();
        let n_ch = self.ch.len();
        if frames == 0 {
            return Ok(vec![Vec::new(); n_ch]);
        }
        let n_freqs = FFT / 2 + 1;
        // The graphs take every frame pushed since the last pull: the first pull's first
        // CONV_LOOKAHEAD outputs belong to no frame (the model looks that far ahead).
        let first = self.pulled == 0;
        let mut raws: Vec<Raw> = Vec::with_capacity(n_ch);
        for ch in self.ch.iter_mut().take(if same { 1 } else { n_ch }) {
            let s = ch.erb.len() / NB_ERB;
            raws.push(step(&mut models.0, &mut ch.carry, &ch.erb, &ch.cplx, s)?);
            ch.erb.clear();
            ch.cplx.clear();
        }
        let skip = if first { CONV_LOOKAHEAD } else { 0 };
        let multi = raws.len() > 1;
        let mut out = vec![vec![0.0f32; frames * HOP]; n_ch];
        let mut buf = vec![Complex32::default(); n_freqs];
        let mut gains = [0.0f32; NB_ERB];
        let base = self.base;
        for (c, (ch, y)) in self.ch.iter_mut().zip(&mut out).enumerate() {
            let Some(raw) = raws.get(c).or(raws.first()) else { continue };
            let spec = &ch.spec;
            let at = |t: isize, f: usize| -> Complex32 {
                usize::try_from(t).ok().and_then(|t| t.checked_sub(base)).and_then(|t| spec.get(t * n_freqs + f)).copied().unwrap_or_default()
            };
            for k in 0..frames {
                let t = self.pulled + k;
                let r = skip + k;
                let (apply_gains, zeros, apply_df) = stages(raw.lsnr.get(r).copied().unwrap_or(-15.0));
                for (f, b) in buf.iter_mut().enumerate() {
                    *b = at(t as isize, f);
                }
                if apply_gains {
                    // One mask for all channels (their mean), as the whole-clip pass.
                    for (i, g) in gains.iter_mut().enumerate() {
                        *g = if multi {
                            raws.iter().map(|x| x.gains.get(r * NB_ERB + i).copied().unwrap_or(0.0)).sum::<f32>() / raws.len() as f32
                        } else {
                            raw.gains.get(r * NB_ERB + i).copied().unwrap_or(0.0)
                        };
                    }
                    ch.st.apply_mask(&mut buf, &gains);
                } else if zeros {
                    ch.st.apply_mask(&mut buf, &[0.0; NB_ERB]);
                }
                if apply_df {
                    let w = NB_DF * DF_ORDER * 2;
                    let co = raw.coefs.get(r * w..(r + 1) * w).unwrap_or_default();
                    for f in 0..NB_DF {
                        let mut acc = Complex32::default();
                        for n in 0..DF_ORDER {
                            let i = (f * DF_ORDER + n) * 2;
                            let coef = Complex32::new(co.get(i).copied().unwrap_or(0.0), co.get(i + 1).copied().unwrap_or(0.0));
                            acc += at(t as isize - (DF_ORDER - 1 - DF_LOOKAHEAD) as isize + n as isize, f) * coef;
                        }
                        if let Some(b) = buf.get_mut(f) {
                            *b = acc;
                        }
                    }
                }
                if let Some(lim) = self.lim {
                    for (f, b) in buf.iter_mut().enumerate() {
                        *b = *b * (1.0 - lim) + at(t as isize, f) * lim;
                    }
                }
                if let Some(dst) = y.get_mut(k * HOP..(k + 1) * HOP) {
                    ch.st.synthesis(&mut buf, dst);
                    for v in dst.iter_mut() {
                        if !v.is_finite() {
                            *v = 0.0;
                        }
                    }
                }
            }
        }
        self.pulled += frames;
        // Keep the spectra the next pull's deep filter reaches back to.
        let keep = self.pulled.saturating_sub(DF_ORDER).max(self.base);
        let drop = keep - self.base;
        if drop > 0 {
            for ch in &mut self.ch {
                ch.spec.drain(..(drop * n_freqs).min(ch.spec.len()));
            }
            self.base = keep;
        }
        Ok(out)
    }
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

    #[test]
    fn the_stream_is_the_whole_clip_pass_a_few_frames_at_a_time() {
        let (_, noisy) = noisy_tone();
        let whole = denoise_48k(std::slice::from_ref(&noisy), 100.0, &Ctx::default()).unwrap().remove(0);
        let mut models = Models::open().unwrap();
        for block in [1usize, 4, 37] {
            let mut st = Stream::new(1, 100.0);
            let mut out = Vec::new();
            for hop in noisy.chunks(HOP) {
                st.push(&[hop]);
                if st.ready() >= block {
                    out.extend(st.pull(&mut models, true).unwrap().remove(0));
                }
            }
            // The first hop pulled is the lead-in.
            let out = out.split_off(HOP);
            assert!(out.len() > 75_000);
            let (mut sig, mut diff) = (0.0f64, 0.0f64);
            for (w, o) in whole.iter().zip(&out) {
                sig += f64::from(*w).powi(2);
                diff += f64::from(w - o).powi(2);
            }
            let db = 10.0 * (sig / diff.max(1e-30)).log10();
            assert!(db > 60.0, "block {block}: the stream is {db:.1} dB from the whole-clip pass");
        }
    }

    #[test]
    fn two_different_sides_share_one_mask() {
        let (tone, noisy) = noisy_tone();
        let quiet: Vec<f32> = tone.iter().map(|v| v * 0.5).collect();
        let whole = denoise_48k(&[noisy.clone(), quiet.clone()], 18.0, &Ctx::default()).unwrap();
        let mut models = Models::open().unwrap();
        let mut st = Stream::new(2, 18.0);
        let mut out = vec![Vec::new(), Vec::new()];
        for (l, r) in noisy.chunks(HOP).zip(quiet.chunks(HOP)) {
            st.push(&[l, r]);
            for (o, y) in out.iter_mut().zip(st.pull(&mut models, false).unwrap()) {
                o.extend(y);
            }
        }
        for (w, o) in whole.iter().zip(&out) {
            let worst = w.iter().zip(&o[HOP..]).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            assert!(worst < 1e-3, "worst difference {worst}");
        }
    }
}
