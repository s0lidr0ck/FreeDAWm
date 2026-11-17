//! `setup.record_format`: what takes are recorded as. The bit depth is the session's (16, 24 or
//! 32-bit float, which bounces and remote takes use too); the file type is Broadcast WAV (a
//! `bext` chunk with FreeDAWm as originator and the take's time reference) or AIFF.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{BitDepthSetting, RecordFileType};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "setup.record_format",
        "Recording Format",
        [],
        None,
        "{bit_depth?: 16|24|32f, file_type?: wav|aiff} → {bit_depth, file_type}. Omitted, each stays; with neither, the current format is returned.",
        always,
        record_format
    )]
}

fn record_format(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "setup.record_format";
    let depth = match p.get("bit_depth") {
        None | Some(Value::Null) => None,
        Some(v) => Some(match v.to_string().trim_matches('"').to_ascii_lowercase().as_str() {
            "16" => BitDepthSetting::Int16,
            "24" => BitDepthSetting::Int24,
            "32" | "32f" | "float" => BitDepthSetting::Float32,
            other => return Err(bad(C, format!("unknown bit depth `{other}` (16, 24 or 32f)"))),
        }),
    };
    let kind = match p.get("file_type").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None => None,
        Some("wav" | "bwf") => Some(RecordFileType::Wav),
        Some("aiff" | "aif") => Some(RecordFileType::Aiff),
        Some(other) => return Err(bad(C, format!("unknown file type `{other}` (wav or aiff)"))),
    };
    if depth.is_some() || kind.is_some() {
        let s = e.session_mut();
        if let Some(d) = depth {
            s.bit_depth = d;
        }
        if let Some(k) = kind {
            s.record_file_type = k;
        }
    }
    let s = e.session();
    let bits = match s.bit_depth {
        BitDepthSetting::Int16 => "16",
        BitDepthSetting::Int24 => "24",
        BitDepthSetting::Float32 => "32f",
    };
    let kind = match s.record_file_type {
        RecordFileType::Wav => "wav",
        RecordFileType::Aiff => "aiff",
    };
    Ok(json!({"bit_depth": bits, "file_type": kind}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_format_is_set_read_and_checked() {
        let mut e = Engine::default();
        assert_eq!(e.execute("setup.record_format", &json!({})).unwrap(), json!({"bit_depth": "24", "file_type": "wav"}));
        assert_eq!(e.execute("setup.record_format", &json!({"bit_depth": 16})).unwrap(), json!({"bit_depth": "16", "file_type": "wav"}));
        assert_eq!(e.execute("setup.record_format", &json!({"file_type": "AIFF"})).unwrap(), json!({"bit_depth": "16", "file_type": "aiff"}));
        assert!(e.execute("setup.record_format", &json!({"bit_depth": 12})).is_err());
        assert!(e.execute("setup.record_format", &json!({"file_type": "mp3"})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().record_file_type, RecordFileType::Wav);
    }
}
