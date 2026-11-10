//! The Llama 3 text tokenizer (byte-level BPE), read from a Hugging Face `tokenizer.json`, for
//! models whose prompt is text ([`crate::textmidi`]). It reproduces `tokenizers`' encoding of a
//! plain string for that file: the Llama 3 split regex (`Isolated`), GPT-2 byte-to-character
//! mapping, BPE with `ignore_merges` (a piece already in the vocabulary is one token), and the
//! `<|begin_of_text|>` the post-processor puts first.
//!
//! Added tokens written out in the text (`<|begin_of_text|>`, `<|midi_token_7|>`) are not
//! recognised: [`LlamaBpe::encode`] keeps them as ordinary text, so a prompt can't inject control
//! or MIDI tokens.

use crate::{MlError, Result};
use std::collections::HashMap;
use std::path::Path;

/// The Llama 3 pre-tokenizer pattern (`tokenizer.json` → `pre_tokenizer` → `Split`).
pub const LLAMA3_PATTERN: &str =
    r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+";

/// A byte-level BPE tokenizer.
pub struct LlamaBpe {
    vocab: HashMap<String, u32>,
    /// (left id, right id) → (rank, merged id).
    merges: HashMap<(u32, u32), (u32, u32)>,
    /// The token id of each byte's character.
    byte_ids: [u32; 256],
    byte_chars: [char; 256],
    split: fancy_regex::Regex,
    bos: Option<u32>,
}

impl std::fmt::Debug for LlamaBpe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlamaBpe").field("vocab", &self.vocab.len()).field("merges", &self.merges.len()).field("bos", &self.bos).finish()
    }
}

fn bad(msg: impl Into<String>) -> MlError {
    MlError::Model("tokenizer".into(), msg.into())
}

/// GPT-2's byte → printable character table.
fn byte_chars() -> [char; 256] {
    let mut out = ['\0'; 256];
    let mut extra = 0u32;
    for b in 0..=255u8 {
        let printable = matches!(b, b'!'..=b'~' | 0xA1..=0xAC | 0xAE..=0xFF);
        let code = if printable {
            u32::from(b)
        } else {
            extra += 1;
            255 + extra
        };
        if let Some(slot) = out.get_mut(usize::from(b)) {
            *slot = char::from_u32(code).unwrap_or('\0');
        }
    }
    out
}

impl LlamaBpe {
    /// Read `tokenizer.json`.
    pub fn open(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| bad(format!("reading {}: {e}", path.display())))?;
        Self::from_json(&text)
    }

    /// Parse the contents of a `tokenizer.json`.
    pub fn from_json(text: &str) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| bad(format!("not JSON: {e}")))?;
        let model = v.get("model").ok_or_else(|| bad("no `model`"))?;
        if model.get("type").and_then(serde_json::Value::as_str) != Some("BPE") {
            return Err(bad("not a BPE tokenizer"));
        }
        let vocab: HashMap<String, u32> = model
            .get("vocab")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| bad("no `model.vocab`"))?
            .iter()
            .filter_map(|(k, id)| Some((k.clone(), u32::try_from(id.as_u64()?).ok()?)))
            .collect();
        let raw_merges = model.get("merges").and_then(serde_json::Value::as_array).ok_or_else(|| bad("no `model.merges`"))?;
        let mut merges = HashMap::with_capacity(raw_merges.len());
        for (rank, m) in raw_merges.iter().enumerate() {
            // "a b" (older files) or ["a", "b"].
            let (a, b) = match m {
                serde_json::Value::String(s) => match s.split_once(' ') {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => continue,
                },
                serde_json::Value::Array(p) => match (p.first().and_then(|x| x.as_str()), p.get(1).and_then(|x| x.as_str())) {
                    (Some(a), Some(b)) => (a.to_string(), b.to_string()),
                    _ => continue,
                },
                _ => continue,
            };
            let (Some(&ia), Some(&ib), Some(&im)) = (vocab.get(&a), vocab.get(&b), vocab.get(&format!("{a}{b}"))) else { continue };
            let rank = u32::try_from(rank).map_err(|_| bad("too many merges"))?;
            merges.entry((ia, ib)).or_insert((rank, im));
        }
        let chars = byte_chars();
        let mut byte_ids = [0u32; 256];
        for (slot, c) in byte_ids.iter_mut().zip(chars.iter()) {
            *slot = *vocab.get(&c.to_string()).ok_or_else(|| bad(format!("the byte character {c:?} is not in the vocabulary")))?;
        }
        let bos = v
            .get("added_tokens")
            .and_then(serde_json::Value::as_array)
            .and_then(|a| a.iter().find(|t| t.get("content").and_then(serde_json::Value::as_str) == Some("<|begin_of_text|>")))
            .and_then(|t| t.get("id")?.as_u64())
            .and_then(|id| u32::try_from(id).ok());
        let split = fancy_regex::Regex::new(LLAMA3_PATTERN).map_err(|e| bad(format!("split pattern: {e}")))?;
        Ok(LlamaBpe { vocab, merges, byte_ids, byte_chars: chars, split, bos })
    }

    /// The `<|begin_of_text|>` id, when the file has one.
    pub fn bos(&self) -> Option<u32> {
        self.bos
    }

    /// Token ids of `text`, with `<|begin_of_text|>` first when `with_bos` (as `tokenizer(text)`).
    pub fn encode(&self, text: &str, with_bos: bool) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        if with_bos {
            out.push(self.bos.ok_or_else(|| bad("no <|begin_of_text|> token"))?);
        }
        let mut last = 0;
        for m in self.split.find_iter(text) {
            let m = m.map_err(|e| bad(format!("splitting the text: {e}")))?;
            if m.start() > last {
                self.encode_piece(text.get(last..m.start()).unwrap_or(""), &mut out)?;
            }
            self.encode_piece(m.as_str(), &mut out)?;
            last = m.end();
        }
        if last < text.len() {
            self.encode_piece(text.get(last..).unwrap_or(""), &mut out)?;
        }
        Ok(out)
    }

    fn encode_piece(&self, piece: &str, out: &mut Vec<u32>) -> Result<()> {
        if piece.is_empty() {
            return Ok(());
        }
        let mapped: String = piece.bytes().map(|b| self.byte_chars.get(usize::from(b)).copied().unwrap_or('\0')).collect();
        // `ignore_merges`: a whole piece in the vocabulary is one token.
        if let Some(&id) = self.vocab.get(&mapped) {
            out.push(id);
            return Ok(());
        }
        let mut syms: Vec<u32> = piece.bytes().map(|b| self.byte_ids.get(usize::from(b)).copied().unwrap_or(0)).collect();
        loop {
            // The leftmost pair of the lowest rank.
            let mut best: Option<(u32, usize, u32)> = None;
            for (i, w) in syms.windows(2).enumerate() {
                if let (Some(&a), Some(&b)) = (w.first(), w.get(1))
                    && let Some(&(rank, merged)) = self.merges.get(&(a, b))
                    && best.is_none_or(|(r, _, _)| rank < r)
                {
                    best = Some((rank, i, merged));
                }
            }
            let Some((_, i, merged)) = best else { break };
            if let Some(slot) = syms.get_mut(i) {
                *slot = merged;
            }
            if i + 1 < syms.len() {
                syms.remove(i + 1);
            }
        }
        out.extend(syms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy tokenizer: bytes as single characters plus a few merges.
    fn toy() -> LlamaBpe {
        let mut vocab = serde_json::Map::new();
        let chars = byte_chars();
        for (i, c) in chars.iter().enumerate() {
            vocab.insert(c.to_string(), serde_json::json!(i));
        }
        for (i, t) in ["he", "ll", "hell", "hello", "Ġw", "Ġwo"].iter().enumerate() {
            vocab.insert((*t).to_string(), serde_json::json!(256 + i));
        }
        let json = serde_json::json!({
            "model": {"type": "BPE", "vocab": vocab, "merges": ["h e", "l l", "he ll", "Ġ w", "Ġw o"], "ignore_merges": true},
            "added_tokens": [{"id": 1000, "content": "<|begin_of_text|>"}],
        });
        LlamaBpe::from_json(&json.to_string()).unwrap()
    }

    #[test]
    fn byte_table_is_gpt2s() {
        let t = byte_chars();
        assert_eq!(t[usize::from(b'A')], 'A');
        assert_eq!(t[usize::from(b' ')], 'Ġ');
        assert_eq!(t[0], 'Ā');
        assert_eq!(t[usize::from(b'\n')], 'Ċ');
    }

    #[test]
    fn merges_apply_by_rank_and_whole_words_are_one_token() {
        let t = toy();
        // "hello" is in the vocabulary: one token (ignore_merges).
        assert_eq!(t.encode("hello", false).unwrap(), vec![259]);
        // "hellx": he + ll → hell, then x.
        assert_eq!(t.encode("hellx", false).unwrap(), vec![258, u32::from(b'x')]);
        // " wox" splits as " wox" (one piece): Ġw → Ġwo, then x.
        assert_eq!(t.encode(" wox", false).unwrap(), vec![261, u32::from(b'x')]);
        assert_eq!(t.encode("hello", true).unwrap(), vec![1000, 259]);
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let t = toy();
        for s in ["", "\u{0}\u{1}", "<|begin_of_text|>", "日本語 ✓ \r\n\t  x", &"a".repeat(5000)] {
            let ids = t.encode(s, false).unwrap();
            assert!(s.is_empty() || !ids.is_empty());
        }
        assert!(LlamaBpe::from_json("{}").is_err());
        assert!(LlamaBpe::from_json("not json").is_err());
    }
}
