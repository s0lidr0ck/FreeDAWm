//! ONNX Runtime sessions on DirectML (the GPU) or the CPU.
//!
//! [`Device::Auto`] and [`Device::Gpu`] try DirectML first (Windows; it runs on any DirectX 12
//! GPU with no CUDA install) and fall back to the CPU when it can't start; a run that fails on
//! DirectML (out of video memory, an unsupported node) is retried on the CPU once, and the
//! session stays on the CPU from then on. Sessions are built and run on worker threads only.

use crate::{MlError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where models run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    /// The GPU when DirectML works, else the CPU.
    #[default]
    Auto,
    /// The GPU (DirectML); still falls back to the CPU, with a note, when it can't run.
    Gpu,
    Cpu,
}

impl Device {
    pub fn parse(s: &str) -> Option<Device> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Device::Auto),
            "gpu" | "directml" | "dml" => Some(Device::Gpu),
            "cpu" => Some(Device::Cpu),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Device::Auto => "auto",
            Device::Gpu => "gpu",
            Device::Cpu => "cpu",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Device::Auto => "Auto (GPU when available)",
            Device::Gpu => "GPU (DirectML)",
            Device::Cpu => "CPU",
        }
    }

    /// Whether to try DirectML at all.
    fn wants_gpu(self) -> bool {
        cfg!(windows) && self != Device::Cpu
    }
}

/// A tensor going in or coming out: shape and row-major `f32` data.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<i64>,
    pub data: Vec<f32>,
}

impl Tensor {
    pub fn new(shape: Vec<i64>, data: Vec<f32>) -> Self {
        Tensor { shape, data }
    }

    /// Elements the shape holds (0 for a negative dimension).
    pub fn elements(shape: &[i64]) -> usize {
        shape.iter().map(|d| usize::try_from(*d).unwrap_or(0)).product()
    }
}

/// A typed tensor: shape and row-major data.
#[derive(Debug, Clone, PartialEq)]
pub enum AnyTensor {
    F32(Vec<i64>, Vec<f32>),
    I64(Vec<i64>, Vec<i64>),
    Bool(Vec<i64>, Vec<bool>),
}

impl AnyTensor {
    pub fn shape(&self) -> &[i64] {
        match self {
            AnyTensor::F32(s, _) | AnyTensor::I64(s, _) | AnyTensor::Bool(s, _) => s,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            AnyTensor::F32(_, d) => d.len(),
            AnyTensor::I64(_, d) => d.len(),
            AnyTensor::Bool(_, d) => d.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An ONNX model loaded in a session.
pub struct OnnxModel {
    id: String,
    src: Source,
    session: ort::session::Session,
    on_gpu: bool,
    dml_config: Vec<(String, String)>,
    cancel: Option<crate::Cancel>,
    /// Why DirectML isn't used, when it was wanted.
    pub fallback_note: Option<String>,
}

impl std::fmt::Debug for OnnxModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxModel").field("id", &self.id).field("src", &self.src.describe()).field("on_gpu", &self.on_gpu).finish()
    }
}

fn rt(id: &str, what: &str, e: impl std::fmt::Display) -> MlError {
    MlError::Runtime(format!("`{id}`: {what}: {e}"))
}

/// Where a model's graph comes from.
#[derive(Clone)]
enum Source {
    File(PathBuf),
    Bytes(std::sync::Arc<[u8]>),
}

impl Source {
    fn describe(&self) -> String {
        match self {
            Source::File(p) => p.display().to_string(),
            Source::Bytes(b) => format!("{} bytes in memory", b.len()),
        }
    }
}

/// Build a session for `src` on the CPU (`gpu == false`) or DirectML.
fn build(id: &str, src: &Source, gpu: bool, dml_config: &[(String, String)]) -> Result<ort::session::Session> {
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).clamp(1, 16);
    let mut b = ort::session::Session::builder().map_err(|e| rt(id, "session builder", e))?;
    b = b.with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3).map_err(|e| rt(id, "optimization level", e))?;
    // Memory: the arenas keep their high-water mark for the session's life (the karaoke RoFormer
    // on the CPU reached 14.5 GB with them); initializers go straight to the device allocator.
    // FREEDAWM_ML_ARENA=1 (or SOUNDCRAFT_ML_ARENA=1) turns the arenas back on (for comparison).
    let arena = crate::env_var_os("ML_ARENA").is_some_and(|v| v == "1");
    if !arena {
        b = b.with_config_entry("session.use_device_allocator_for_initializers", "1").map_err(|e| rt(id, "initializer allocator", e))?;
        b = b.with_memory_pattern(false).map_err(|e| rt(id, "memory pattern", e))?;
    }
    if gpu {
        // DirectML wants sequential execution and no memory pattern (ORT's DML notes).
        b = b.with_parallel_execution(false).map_err(|e| rt(id, "sequential execution", e))?;
        b = b.with_memory_pattern(false).map_err(|e| rt(id, "memory pattern", e))?;
        // Per-model DirectML settings from its card (`dml_session_config`, e.g. HTDemucs needs
        // `ep.dml.disable_graph_fusion = 1` or its output is garbage).
        for (k, v) in dml_config {
            b = b.with_config_entry(k, v).map_err(|e| rt(id, &format!("session config {k}"), e))?;
        }
        b = with_directml(b).map_err(|e| rt(id, "DirectML", e))?;
    } else {
        b = b.with_intra_threads(threads).map_err(|e| rt(id, "threads", e))?;
        b = b.with_execution_providers([ort::ep::CPU::default().with_arena_allocator(arena).build()]).map_err(|e| rt(id, "CPU allocator", e))?;
    }
    match src {
        Source::File(p) => b.commit_from_file(p),
        Source::Bytes(bytes) => b.commit_from_memory(bytes),
    }
    .map_err(|e| rt(id, &format!("loading {}", src.describe()), e))
}

#[cfg(windows)]
fn with_directml(b: ort::session::builder::SessionBuilder) -> std::result::Result<ort::session::builder::SessionBuilder, String> {
    b.with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()]).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn with_directml(_: ort::session::builder::SessionBuilder) -> std::result::Result<ort::session::builder::SessionBuilder, String> {
    Err("DirectML is Windows only".into())
}

impl OnnxModel {
    /// Load `path` (model `id`, for messages) on `device`, falling back to the CPU.
    pub fn open(id: &str, path: &Path, device: Device) -> Result<Self> {
        Self::open_source(id, Source::File(path.into()), device)
    }

    /// Load a model graph held in memory.
    pub fn open_bytes(id: &str, bytes: &[u8], device: Device) -> Result<Self> {
        Self::open_source(id, Source::Bytes(bytes.into()), device)
    }

    /// Load an installed model on `device`, as its card says: `dml_supported: false` keeps it on
    /// the CPU, `dml_session_config` entries are set on a DirectML session. `cancel` stops a run
    /// in the middle (ORT's terminate flag), not only between chunks.
    pub fn open_model(model: &crate::models::InstalledModel, device: Device, cancel: &crate::Cancel) -> Result<Self> {
        Self::open_model_file(model, &model.card.file, device, cancel)
    }

    /// As [`OnnxModel::open_model`], for one of the model's graphs (`file`, relative to its
    /// folder; a card may list several). A card with `recommended_ep: "cpu"` runs on the CPU
    /// under [`Device::Auto`] (the GPU is still tried when asked for).
    pub fn open_model_file(model: &crate::models::InstalledModel, file: &str, device: Device, cancel: &crate::Cancel) -> Result<Self> {
        let card = &model.card;
        let path = Path::new(file);
        if file.is_empty() || path.is_absolute() || path.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(MlError::Model(card.id.clone(), format!("`{file}` is not a file in the model folder")));
        }
        let cpu_first = card.extra.get("recommended_ep").and_then(serde_json::Value::as_str).is_some_and(|e| e.eq_ignore_ascii_case("cpu"));
        let device = if cpu_first && device == Device::Auto { Device::Cpu } else { device };
        let dml_ok = card.extra.get("dml_supported").and_then(serde_json::Value::as_bool).unwrap_or(true);
        let config: Vec<(String, String)> = card
            .extra
            .get("dml_session_config")
            .and_then(serde_json::Value::as_object)
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect())
            .unwrap_or_default();
        let id = if card.id.is_empty() { "model" } else { card.id.as_str() };
        let wanted = if dml_ok { device } else { Device::Cpu };
        let mut m = Self::open_with(id, Source::File(model.dir.join(file)), wanted, config)?;
        if !dml_ok && device.wants_gpu() {
            m.fallback_note = Some("CPU (this model does not run correctly on DirectML)".into());
        }
        m.cancel = Some(cancel.clone());
        Ok(m)
    }

    fn open_source(id: &str, src: Source, device: Device) -> Result<Self> {
        Self::open_with(id, src, device, Vec::new())
    }

    fn open_with(id: &str, src: Source, device: Device, dml_config: Vec<(String, String)>) -> Result<Self> {
        let mut note = None;
        if device.wants_gpu() {
            match build(id, &src, true, &dml_config) {
                Ok(session) => return Ok(OnnxModel { id: id.into(), src, session, on_gpu: true, dml_config, cancel: None, fallback_note: None }),
                Err(e) => {
                    log::warn!("{e}; running on the CPU");
                    note = Some(format!("DirectML unavailable ({e}); ran on the CPU"));
                }
            }
        }
        let session = build(id, &src, false, &[])?;
        Ok(OnnxModel { id: id.into(), src, session, on_gpu: false, dml_config, cancel: None, fallback_note: note })
    }

    pub fn on_gpu(&self) -> bool {
        self.on_gpu
    }

    /// "GPU (DirectML)" or "CPU".
    pub fn device_label(&self) -> &'static str {
        if self.on_gpu { "GPU (DirectML)" } else { "CPU" }
    }

    pub fn input_names(&self) -> Vec<String> {
        self.session.inputs().iter().map(|i| i.name().to_string()).collect()
    }

    pub fn output_names(&self) -> Vec<String> {
        self.session.outputs().iter().map(|o| o.name().to_string()).collect()
    }

    /// Run with the inputs in the model's input order; returns every output in the model's output
    /// order. On DirectML a failed run is retried once on the CPU (and the model stays there).
    pub fn run(&mut self, inputs: Vec<Tensor>) -> Result<Vec<Tensor>> {
        // Keep a copy for the CPU retry only when a DirectML failure could need one.
        let retry = if self.on_gpu { Some(inputs.clone()) } else { None };
        match self.run_once(inputs) {
            Ok(v) => Ok(v),
            Err(MlError::Cancelled) => Err(MlError::Cancelled),
            Err(e) if self.on_gpu => {
                log::warn!("{e}; retrying on the CPU");
                self.session = build(&self.id, &self.src, false, &self.dml_config)?;
                self.on_gpu = false;
                self.fallback_note = Some(format!("DirectML failed ({e}); finished on the CPU"));
                self.run_once(retry.unwrap_or_default())
            }
            Err(e) => Err(e),
        }
    }

    /// Run with inputs given by name (any order); returns every output in the model's output order.
    pub fn run_named(&mut self, inputs: &[(&str, &Tensor)]) -> Result<Vec<Tensor>> {
        let names = self.input_names();
        let mut ordered = Vec::with_capacity(names.len());
        for n in &names {
            let Some((_, t)) = inputs.iter().find(|(k, _)| k == n) else {
                return Err(MlError::Model(self.id.clone(), format!("no value for input `{n}`")));
            };
            ordered.push((*t).clone());
        }
        self.run(ordered)
    }

    /// The index of output `name` in the model's output order.
    pub fn output_index(&self, name: &str) -> Result<usize> {
        self.output_names().iter().position(|n| n == name).ok_or_else(|| MlError::Model(self.id.clone(), format!("no output `{name}`")))
    }

    /// Run with typed inputs given by name (`f32`, `i64` or `bool` tensors); returns every output
    /// by name, typed. On the device the session is on (no CPU retry).
    pub fn run_typed(&mut self, inputs: Vec<(&str, AnyTensor)>) -> Result<Vec<(String, AnyTensor)>> {
        let id = self.id.clone();
        let mut values: Vec<(String, ort::session::SessionInputValue<'static>)> = Vec::with_capacity(inputs.len());
        for (name, t) in inputs {
            let (shape, n) = (t.shape().to_vec(), t.len());
            if Tensor::elements(&shape) != n {
                return Err(MlError::Model(id, format!("input `{name}`: shape {shape:?} does not hold {n} values")));
            }
            let v: ort::session::SessionInputValue<'static> = match t {
                AnyTensor::F32(s, d) => ort::value::Tensor::from_array((s, d)).map_err(|e| rt(&id, "input tensor", e))?.into(),
                AnyTensor::I64(s, d) => ort::value::Tensor::from_array((s, d)).map_err(|e| rt(&id, "input tensor", e))?.into(),
                AnyTensor::Bool(s, d) => ort::value::Tensor::from_array((s, d)).map_err(|e| rt(&id, "input tensor", e))?.into(),
            };
            values.push((name.to_string(), v));
        }
        if self.cancel.as_ref().is_some_and(crate::Cancel::is_cancelled) {
            return Err(MlError::Cancelled);
        }
        let out_names = self.output_names();
        let outputs = self.session.run(values).map_err(|e| rt(&id, "run", e))?;
        let mut res = Vec::with_capacity(out_names.len());
        for name in out_names {
            let Some(v) = outputs.get(name.as_str()) else { return Err(MlError::Model(id, format!("no output `{name}`"))) };
            let t = if let Ok((s, d)) = v.try_extract_tensor::<f32>() {
                AnyTensor::F32(s.iter().copied().collect(), d.to_vec())
            } else if let Ok((s, d)) = v.try_extract_tensor::<i64>() {
                AnyTensor::I64(s.iter().copied().collect(), d.to_vec())
            } else if let Ok((s, d)) = v.try_extract_tensor::<bool>() {
                AnyTensor::Bool(s.iter().copied().collect(), d.to_vec())
            } else {
                return Err(MlError::Model(id, format!("output `{name}` is not f32, i64 or bool")));
            };
            res.push((name, t));
        }
        Ok(res)
    }

    /// Run with ready-made ONNX Runtime values by name and keep every output as a value, for
    /// graphs whose outputs go straight back in (a decoder's key/value cache) without copying.
    /// On the device the session is on (no CPU retry).
    pub fn run_values(
        &mut self,
        inputs: Vec<(String, ort::session::SessionInputValue<'static>)>,
    ) -> Result<std::collections::HashMap<String, ort::value::DynValue>> {
        if self.cancel.as_ref().is_some_and(crate::Cancel::is_cancelled) {
            return Err(MlError::Cancelled);
        }
        let id = self.id.clone();
        let names = self.output_names();
        let mut outputs = self.session.run(inputs).map_err(|e| rt(&id, "run", e))?;
        let mut res = std::collections::HashMap::with_capacity(names.len());
        for name in names {
            if let Some(v) = outputs.remove(name.as_str()) {
                res.insert(name, v);
            }
        }
        Ok(res)
    }

    fn run_once(&mut self, inputs: Vec<Tensor>) -> Result<Vec<Tensor>> {
        let id = self.id.clone();
        let names = self.input_names();
        if names.len() != inputs.len() {
            return Err(MlError::Model(id, format!("expects {} inputs, given {}", names.len(), inputs.len())));
        }
        let mut values: Vec<(String, ort::session::SessionInputValue<'static>)> = Vec::with_capacity(inputs.len());
        for (name, t) in names.into_iter().zip(inputs) {
            if Tensor::elements(&t.shape) != t.data.len() {
                return Err(MlError::Model(id, format!("input `{name}`: shape {:?} does not hold {} values", t.shape, t.data.len())));
            }
            let v = ort::value::Tensor::from_array((t.shape, t.data)).map_err(|e| rt(&id, "input tensor", e))?;
            values.push((name, v.into()));
        }
        let out_names = self.output_names();
        if self.cancel.as_ref().is_some_and(crate::Cancel::is_cancelled) {
            return Err(MlError::Cancelled);
        }
        let opts = std::sync::Arc::new(ort::session::RunOptions::new().map_err(|e| rt(&id, "run options", e))?);
        // A watcher sets ORT's terminate flag when the job is cancelled mid-run.
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(cancel) = self.cancel.clone() {
            let (o, d) = (std::sync::Arc::clone(&opts), std::sync::Arc::clone(&done));
            let _ = std::thread::Builder::new().name("ml-cancel".into()).spawn(move || {
                while !d.load(std::sync::atomic::Ordering::Relaxed) {
                    if cancel.is_cancelled() {
                        let _ = o.terminate();
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
        }
        let run = self.session.run_with_options(values, &opts);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        if self.cancel.as_ref().is_some_and(crate::Cancel::is_cancelled) {
            return Err(MlError::Cancelled);
        }
        let outputs = run.map_err(|e| rt(&id, "run", e))?;
        let mut res = Vec::with_capacity(out_names.len());
        for name in &out_names {
            let Some(v) = outputs.get(name.as_str()) else { return Err(MlError::Model(id, format!("no output `{name}`"))) };
            let (shape, data) = v.try_extract_tensor::<f32>().map_err(|e| rt(&id, &format!("output `{name}`"), e))?;
            res.push(Tensor { shape: shape.iter().copied().collect(), data: data.to_vec() });
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_round_trip() {
        for d in [Device::Auto, Device::Gpu, Device::Cpu] {
            assert_eq!(Device::parse(d.id()), Some(d));
        }
        assert_eq!(Device::parse("DirectML"), Some(Device::Gpu));
        assert_eq!(Device::parse("tpu"), None);
        assert_eq!(serde_json::to_string(&Device::Gpu).unwrap(), "\"gpu\"");
    }

    #[test]
    fn a_missing_model_file_is_an_error_not_a_panic() {
        let e = OnnxModel::open("nothing", Path::new("C:/definitely/not/here.onnx"), Device::Auto).unwrap_err();
        assert!(matches!(e, MlError::Runtime(_)), "{e:?}");
    }
}
