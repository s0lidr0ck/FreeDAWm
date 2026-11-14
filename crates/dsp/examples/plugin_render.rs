//! Renders a WAV file through one built-in plugin, offline, as a DAW export would: the plugin's
//! latency is taken out, so the output lines up with the input sample for sample.
//!
//! `cargo run --release -p soundcraft-dsp --example plugin_render -- <plugin id> <in.wav> <out.wav> [param=value ...]`
//!
//! Reads 16, 24 and 32-bit PCM and 32-bit float; writes 32-bit float at the input's rate and
//! channel count. Used to tune Rez (`tools/rez-tuning`).

use std::process::ExitCode;

struct Wav {
    sample_rate: u32,
    channels: Vec<Vec<f32>>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn read_wav(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF/WAVE file".into());
    }
    let (mut format, mut channels, mut sample_rate, mut bits) = (0u16, 0usize, 0u32, 0usize);
    let mut data: Option<&[u8]> = None;
    let mut at = 12usize;
    while let (Some(id), Some(size)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let body = at + 8;
        let end = body.saturating_add(size as usize).min(bytes.len());
        if id == b"fmt " {
            format = u16_at(bytes, body).ok_or("short fmt chunk")?;
            channels = usize::from(u16_at(bytes, body + 2).ok_or("short fmt chunk")?);
            sample_rate = u32_at(bytes, body + 4).ok_or("short fmt chunk")?;
            bits = usize::from(u16_at(bytes, body + 14).ok_or("short fmt chunk")?);
            if format == 0xFFFE {
                // WAVE_FORMAT_EXTENSIBLE: the real format is the sub-format's first two bytes.
                format = u16_at(bytes, body + 24).ok_or("short extensible fmt chunk")?;
            }
        } else if id == b"data" {
            data = bytes.get(body..end);
        }
        at = end.saturating_add(size as usize & 1);
    }
    let data = data.ok_or("no data chunk")?;
    let width = bits / 8;
    if channels == 0 || sample_rate == 0 || !matches!((format, bits), (1, 16 | 24 | 32) | (3, 32)) {
        return Err(format!("unsupported WAV: format {format}, {bits} bits, {channels} channels"));
    }
    let mut out = vec![Vec::with_capacity(data.len() / (width * channels)); channels];
    for frame in data.chunks_exact(width * channels) {
        for (ch, s) in out.iter_mut().zip(frame.chunks_exact(width)) {
            let v = match (format, s) {
                (3, &[a, b, c, d]) => f32::from_le_bytes([a, b, c, d]),
                (_, &[a, b]) => f32::from(i16::from_le_bytes([a, b])) / 32_768.0,
                (_, &[a, b, c]) => (i32::from_le_bytes([0, a, b, c]) >> 8) as f32 / 8_388_608.0,
                (_, &[a, b, c, d]) => i32::from_le_bytes([a, b, c, d]) as f32 / 2_147_483_648.0,
                _ => 0.0,
            };
            ch.push(v);
        }
    }
    Ok(Wav { sample_rate, channels: out })
}

fn write_wav(wav: &Wav) -> Result<Vec<u8>, String> {
    let channels = wav.channels.len();
    let frames = wav.channels.iter().map(Vec::len).min().unwrap_or(0);
    let data_len = u32::try_from(frames * channels * 4).map_err(|_| "too long for a WAV file")?;
    let channels_u16 = u16::try_from(channels).map_err(|_| "too many channels")?;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&channels_u16.to_le_bytes());
    out.extend_from_slice(&wav.sample_rate.to_le_bytes());
    out.extend_from_slice(&(wav.sample_rate * u32::from(channels_u16) * 4).to_le_bytes());
    out.extend_from_slice(&(channels_u16 * 4).to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        for ch in &wav.channels {
            out.extend_from_slice(&ch.get(i).copied().unwrap_or(0.0).to_le_bytes());
        }
    }
    Ok(out)
}

fn run(args: &[String]) -> Result<(), String> {
    let [id, input, output, params @ ..] = args else {
        return Err("usage: plugin_render <plugin id> <in.wav> <out.wav> [param=value ...]".into());
    };
    let mut wav = read_wav(&std::fs::read(input).map_err(|e| format!("{input}: {e}"))?)?;
    let mut plugin = soundcraft_dsp::create(id).ok_or_else(|| format!("no plugin with id {id}"))?;
    for kv in params {
        let (key, value) = kv.split_once('=').ok_or_else(|| format!("{kv}: expected param=value"))?;
        let value: f32 = value.parse().map_err(|_| format!("{kv}: not a number"))?;
        if !plugin.set_param(key, value) {
            let known: Vec<&str> = plugin.info().params.iter().map(|p| p.id).collect();
            return Err(format!("{id} has no parameter {key} (it has: {})", known.join(", ")));
        }
    }
    const BLOCK: usize = 512;
    let channels = wav.channels.len();
    let frames = wav.channels.iter().map(Vec::len).min().unwrap_or(0);
    plugin.prepare(wav.sample_rate as f32, BLOCK, channels);
    let latency = plugin.latency();
    let mut rendered = vec![Vec::with_capacity(frames + latency); channels];
    let mut io = vec![vec![0.0f32; BLOCK]; channels];
    let mut deepest = 0.0f32;
    let mut pos = 0usize;
    // The input, then silence for as long as the latency holds audio back.
    while pos < frames + latency {
        let n = BLOCK.min(frames + latency - pos);
        for (buf, ch) in io.iter_mut().zip(&wav.channels) {
            for (i, x) in buf.iter_mut().enumerate() {
                *x = ch.get(pos + i).copied().unwrap_or(0.0);
            }
        }
        plugin.process(&mut io, n);
        deepest = deepest.max(plugin.gain_reduction_db());
        for (out, buf) in rendered.iter_mut().zip(&io) {
            out.extend_from_slice(buf.get(..n).unwrap_or(&[]));
        }
        pos += n;
    }
    for (ch, out) in wav.channels.iter_mut().zip(rendered) {
        *ch = out.get(latency..).unwrap_or(&[]).to_vec();
    }
    std::fs::write(output, write_wav(&wav)?).map_err(|e| format!("{output}: {e}"))?;
    println!(
        "{id}: {frames} frames, {channels} channels at {} Hz, latency {latency} samples, deepest gain reduction {deepest:.1} dB",
        wav.sample_rate
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
