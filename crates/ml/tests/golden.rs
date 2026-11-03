//! Each model runner against its export lane's golden output (`<models dir>/<id>/golden/`, made by
//! the model's `reference_infer.py` on CPU ONNX Runtime): SNR >= 60 dB per stem on the CPU.
//!
//! The models are not in the repository: point `SOUNDCRAFT_MODELS_DIR` at a folder holding them
//! (e.g. the export lane's `ml-models`). A missing model prints `SKIPPED` and passes, so CI without
//! models stays green; `ML_GOLDEN_STRICT=1` makes a missing model a failure.

use soundcraft_ml::models::{self, InstalledModel};
use soundcraft_ml::{Ctx, Device};
use std::path::Path;

/// The GPU tests take turns: DirectML sessions run side by side in one process give wrong
/// output (SNR well under the floor) or crash, which made the suite fail whenever it ran threaded.
fn gpu_turn() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

/// `data` as `[rows][len]`.
fn planar(data: &[f32], rows: usize) -> Vec<Vec<f32>> {
    let len = data.len() / rows;
    (0..rows).map(|r| data[r * len..(r + 1) * len].to_vec()).collect()
}

fn snr(reference: &[f32], x: &[f32]) -> f64 {
    assert_eq!(reference.len(), x.len());
    let (mut s, mut e) = (0.0f64, 0.0f64);
    for (r, v) in reference.iter().zip(x) {
        s += f64::from(*r) * f64::from(*r);
        e += f64::from(r - v) * f64::from(r - v);
    }
    if e == 0.0 { f64::INFINITY } else { 10.0 * (s.max(1e-30) / e).log10() }
}

fn model(id: &str) -> Option<InstalledModel> {
    let dir = models::models_dir();
    match models::load(&dir, id) {
        Ok(m) if m.dir.join("golden").is_dir() => Some(m),
        other => {
            let why = other.map_or_else(|e| e.to_string(), |_| "no golden/ folder".into());
            assert!(std::env::var_os("ML_GOLDEN_STRICT").is_none(), "{id}: {why}");
            println!("SKIPPED {id}: {why} (set SOUNDCRAFT_MODELS_DIR to the folder holding the models)");
            None
        }
    }
}

/// Stems `[stem][channel][frame]` against the golden `[stem][channel][frame]` file.
fn check(id: &str, case: &str, got: &[(String, Vec<Vec<f32>>)], want: &[f32]) {
    let channels = got.first().map_or(0, |s| s.1.len());
    let want = planar(want, got.len() * channels);
    for (s, (name, audio)) in got.iter().enumerate() {
        for (c, ch) in audio.iter().enumerate() {
            let v = snr(&want[s * channels + c], ch);
            println!("{id} {case} {name} ch{c}: SNR {v:.1} dB");
            assert!(v >= 60.0, "{id} {case} {name} ch{c}: SNR {v:.1} dB < 60");
        }
    }
}

#[test]
fn htdemucs_matches_golden() {
    let Some(m) = model("htdemucs") else { return };
    for case in ["5s", "10s"] {
        let input = planar(&read_f32(&m.dir.join(format!("golden/input_{case}.f32"))), 2);
        let (stems, dev) = soundcraft_ml::htdemucs::separate(&m, &input, soundcraft_ml::Quality::Fast, Device::Cpu, &Ctx::default()).unwrap();
        assert_eq!(dev, "CPU");
        check("htdemucs", case, &stems, &read_f32(&m.dir.join(format!("golden/output_{case}.f32"))));
    }
}

#[test]
fn drumsep_matches_golden() {
    let Some(m) = model("drumsep_mdx23c") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let (stems, _) = soundcraft_ml::drumsep::run(&m, &input, Device::Cpu, &Ctx::default()).unwrap();
    assert_eq!(stems.len(), 6);
    check("drumsep_mdx23c", "5s", &stems, &read_f32(&m.dir.join("golden/output_5s.f32")));
}

#[test]
fn srs_matches_golden() {
    let Some(m) = model("srs_small") else { return };
    // The high-pass alone.
    let input = read_f32(&m.dir.join("golden/input_5s.f32"));
    let hp = soundcraft_ml::srs::highpass60(&input);
    let v = snr(&read_f32(&m.dir.join("golden/highpass_5s.f32")), &hp);
    println!("srs_small high-pass: SNR {v:.1} dB");
    assert!(v >= 80.0, "high-pass SNR {v:.1}");
    for case in ["5s", "12s"] {
        let input = read_f32(&m.dir.join(format!("golden/input_{case}.f32")));
        let (out, _) = soundcraft_ml::srs::run(&m, &[input], Device::Cpu, &Ctx::default()).unwrap();
        check("srs_small", case, &out, &read_f32(&m.dir.join(format!("golden/output_{case}.f32"))));
    }
}

#[test]
fn karaoke_matches_golden() {
    let Some(m) = model("karaoke_melroformer") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let (stems, _) = soundcraft_ml::karaoke::run(&m, &input, Device::Cpu, &Ctx::default()).unwrap();
    check("karaoke_melroformer", "5s", &stems, &read_f32(&m.dir.join("golden/output_5s.f32")));
}

#[test]
fn vocals_melroformer_matches_golden() {
    let Some(m) = model("vocals_melroformer") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let ((v, i), _) = soundcraft_ml::vocals::run(&m, &input, Device::Cpu, &Ctx::default()).unwrap();
    let stems = vec![("vocals".to_string(), v), ("instrumental".to_string(), i)];
    check("vocals_melroformer", "5s", &stems, &read_f32(&m.dir.join("golden/output_5s.f32")));
}

#[test]
fn vocals_melroformer_on_directml_matches_golden() {
    let _gpu = gpu_turn();
    let Some(m) = model("vocals_melroformer") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let ((v, i), dev) = soundcraft_ml::vocals::run(&m, &input, Device::Gpu, &Ctx::default()).unwrap();
    if dev != "GPU (DirectML)" {
        println!("SKIPPED vocals on DirectML: {dev}");
        return;
    }
    let stems = vec![("vocals".to_string(), v), ("instrumental".to_string(), i)];
    check("vocals_melroformer (DirectML)", "5s", &stems, &read_f32(&m.dir.join("golden/output_5s.f32")));
}

#[test]
fn bs_roformer_sw_matches_golden() {
    let Some(m) = model("bs_roformer_sw") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let (stems, _) = soundcraft_ml::bs_roformer::separate(&m, &input, Device::Cpu, &Ctx::default()).unwrap();
    assert_eq!(stems.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), soundcraft_ml::bs_roformer::DEFAULT_STEMS);
    check("bs_roformer_sw", "5s", &stems, &read_f32(&m.dir.join("golden/output_5s.f32")));
}

#[test]
fn bs_roformer_sw_on_directml_matches_golden() {
    let _gpu = gpu_turn();
    let Some(m) = model("bs_roformer_sw") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let (stems, dev) = soundcraft_ml::bs_roformer::separate(&m, &input, Device::Gpu, &Ctx::default()).unwrap();
    if dev != "GPU (DirectML)" {
        println!("SKIPPED bs_roformer_sw on DirectML: {dev}");
        return;
    }
    check("bs_roformer_sw (DirectML)", "5s", &stems, &read_f32(&m.dir.join("golden/output_5s.f32")));
}

#[test]
fn htdemucs_on_directml_matches_golden() {
    let _gpu = gpu_turn();
    let Some(m) = model("htdemucs") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_10s.f32")), 2);
    let (stems, dev) = soundcraft_ml::htdemucs::separate(&m, &input, soundcraft_ml::Quality::Fast, Device::Gpu, &Ctx::default()).unwrap();
    if dev != "GPU (DirectML)" {
        println!("SKIPPED DirectML: {dev}");
        return;
    }
    check("htdemucs (DirectML)", "10s", &stems, &read_f32(&m.dir.join("golden/output_10s.f32")));
}

#[test]
fn karaoke_cancels_mid_run() {
    let _gpu = gpu_turn();
    // Cancel from the progress callback at the first chunk report (no wall-clock race with other
    // tests sharing the GPU): the job must stop with Cancelled and never report completion.
    let Some(m) = model("karaoke_melroformer") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let cancel = soundcraft_ml::Cancel::new();
    let state = std::sync::Arc::new(std::sync::Mutex::new((None::<std::time::Instant>, 0.0f32, None::<std::time::Duration>)));
    let (c2, s2) = (cancel.clone(), std::sync::Arc::clone(&state));
    let t0 = std::time::Instant::now();
    let ctx = Ctx::new(cancel, move |f, _| {
        let mut st = s2.lock().unwrap();
        st.1 = st.1.max(f);
        if f > 0.0 && st.0.is_none() {
            // The first chunk took this long; cancel now.
            st.2 = Some(t0.elapsed());
            st.0 = Some(std::time::Instant::now());
            c2.cancel();
        }
    });
    let r = soundcraft_ml::karaoke::run(&m, &input, Device::Gpu, &ctx);
    let st = *state.lock().unwrap();
    assert!(matches!(r, Err(soundcraft_ml::MlError::Cancelled)), "{r:?}");
    assert!(st.1 < 1.0, "progress reached {}", st.1);
    let (Some(at), Some(chunk)) = (st.0, st.2) else { panic!("no progress was reported before the job ended") };
    let stop = at.elapsed();
    println!("karaoke: first chunk {:.2} s, cancel to return {:.2} s", chunk.as_secs_f64(), stop.as_secs_f64());
    assert!(stop <= chunk * 2 + std::time::Duration::from_millis(500), "cancel took {stop:?} (one chunk {chunk:?})");
}

#[test]
fn karaoke_on_directml_matches_golden() {
    let _gpu = gpu_turn();
    let Some(m) = model("karaoke_melroformer") else { return };
    let input = planar(&read_f32(&m.dir.join("golden/input_5s.f32")), 2);
    let (stems, dev) = soundcraft_ml::karaoke::run(&m, &input, Device::Gpu, &Ctx::default()).unwrap();
    if dev != "GPU (DirectML)" {
        println!("SKIPPED karaoke on DirectML: {dev}");
        return;
    }
    check("karaoke_melroformer (DirectML)", "5s", &stems, &read_f32(&m.dir.join("golden/output_5s.f32")));
}

#[test]
fn srs_keeps_a_stereo_image() {
    let Some(m) = model("srs_small") else { return };
    let x = read_f32(&m.dir.join("golden/input_5s.f32"));
    // Left: the vocal; right: the vocal at a quarter level, half a second later.
    let d = 24_000;
    let right: Vec<f32> = (0..x.len()).map(|i| if i >= d { x[i - d] * 0.25 } else { 0.0 }).collect();
    let (out, _) = soundcraft_ml::srs::run(&m, &[x.clone(), right], Device::Cpu, &Ctx::default()).unwrap();
    let st = &out[0].1;
    assert_eq!(st.len(), 2, "stereo in, stereo out");
    let rms = |c: &[f32]| (c.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / c.len() as f64).sqrt();
    let ratio = rms(&st[1]) / rms(&st[0]);
    println!("srs stereo: right/left level {ratio:.3}");
    assert!(ratio < 0.9, "the channels stay different (dual mono would be 1.0): {ratio}");
    // The left channel alone is what the mono model gives it (one shared peak gain).
    let (mono, _) = soundcraft_ml::srs::run(&m, &[x], Device::Cpu, &Ctx::default()).unwrap();
    assert!(snr(&mono[0].1[0], &st[0]) > 60.0);
}
