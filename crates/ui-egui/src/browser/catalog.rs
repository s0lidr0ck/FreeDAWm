//! The browser's catalog: one list of every insertable plugin (built-in, FireStation, VST3, CLAP,
//! AU) with a browser category, plus the files and plugins the scans could not use.
//!
//! Pure: [`build_catalog`] takes the scans' results, [`filter`] answers the panel's search and
//! chips. Categories come from the plugin itself (built-in registrations, VST3 sub-categories,
//! CLAP features); the user may re-categorise a plugin (the `overrides` map handed to [`filter`]).
//!
//! FireStation engines whose nature is split between two categories are placed as follows:
//! Filter (envelope / LFO filters: wahs, Mu-Tron) under Modulation; Transient, Shale (drum gate),
//! Swale (volume swells) and Twine (clipper + limiter) under Dynamics; LevelUp (gain matching and
//! loudness metering) under Utility; Vint (vintage EQ / colour boxes) under EQ.

use soundcraft_au_host::AuDescriptor;
use soundcraft_clap_host::ClapDescriptor;
use soundcraft_dsp::{Category, PluginInfo};
use soundcraft_vst3_host::Vst3Descriptor;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

/// Vendor shown for the built-in plugins.
pub const BUILTIN_VENDOR: &str = "FreeDAWm";
/// Vendor shown for the FireStation engines.
pub const FIRESTATION_VENDOR: &str = "FireStation";

/// Where a plugin comes from (the card's badge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Format {
    BuiltIn,
    FireStation,
    Vst3,
    Clap,
    Au,
}

impl Format {
    /// The badge text.
    pub fn label(self) -> &'static str {
        match self {
            Format::BuiltIn => "Built-in",
            Format::FireStation => "FS",
            Format::Vst3 => "VST3",
            Format::Clap => "CLAP",
            Format::Au => "AU",
        }
    }
}

/// The browser's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    Instrument,
    Effect,
}

/// The fixed list of browser categories every plugin maps onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum BrowserCategory {
    Eq,
    Dynamics,
    Reverb,
    Delay,
    Modulation,
    Distortion,
    Pitch,
    Utility,
    Analyzer,
    Synth,
    Keys,
    Sampler,
    Drums,
    Other,
}

impl BrowserCategory {
    /// Every category, in chip order.
    pub const ALL: [BrowserCategory; 14] = [
        BrowserCategory::Eq,
        BrowserCategory::Dynamics,
        BrowserCategory::Reverb,
        BrowserCategory::Delay,
        BrowserCategory::Modulation,
        BrowserCategory::Distortion,
        BrowserCategory::Pitch,
        BrowserCategory::Utility,
        BrowserCategory::Analyzer,
        BrowserCategory::Synth,
        BrowserCategory::Keys,
        BrowserCategory::Sampler,
        BrowserCategory::Drums,
        BrowserCategory::Other,
    ];

    /// The chip text.
    pub fn label(self) -> &'static str {
        match self {
            BrowserCategory::Eq => "EQ",
            BrowserCategory::Dynamics => "Dynamics",
            BrowserCategory::Reverb => "Reverb",
            BrowserCategory::Delay => "Delay",
            BrowserCategory::Modulation => "Modulation",
            BrowserCategory::Distortion => "Distortion",
            BrowserCategory::Pitch => "Pitch",
            BrowserCategory::Utility => "Utility",
            BrowserCategory::Analyzer => "Analyzer",
            BrowserCategory::Synth => "Synth",
            BrowserCategory::Keys => "Keys",
            BrowserCategory::Sampler => "Sampler",
            BrowserCategory::Drums => "Drums",
            BrowserCategory::Other => "Other",
        }
    }
}

/// One plugin card.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrowserItem {
    /// The id inserts use (`eq_7band`, `fs_comp`, `vst3:…`, `clap:…`, `au:…`).
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub format: Format,
    pub kind: Kind,
    /// The plugin's own category (before any user override).
    pub category: BrowserCategory,
}

/// A file or plugin a scan could not use, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScanFailure {
    pub path: String,
    pub reason: String,
}

impl From<(PathBuf, String)> for ScanFailure {
    /// From a host's `scan_failures()` entry.
    fn from((path, reason): (PathBuf, String)) -> Self {
        ScanFailure { path: path.display().to_string(), reason }
    }
}

/// Everything the browser lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub items: Vec<BrowserItem>,
    pub failures: Vec<ScanFailure>,
}

/// The panel's search and chips.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// The tab.
    pub kind: Kind,
    /// Matched against name, vendor and category label, case-insensitively.
    pub text: String,
    /// Only favourites.
    pub favourites: bool,
    /// Only recent plugins, newest first.
    pub recent: bool,
    pub category: Option<BrowserCategory>,
    pub vendor: Option<String>,
}

/// What a lower-case word from a VST3 sub-category, CLAP feature or plugin name says about an
/// instrument.
fn instrument_word(w: &str) -> Option<BrowserCategory> {
    match w {
        "drum" | "drums" | "drum-machine" | "drum machine" | "percussion" => Some(BrowserCategory::Drums),
        "sampler" | "rompler" => Some(BrowserCategory::Sampler),
        "piano" | "keys" | "organ" | "e-piano" | "electric-piano" | "electric piano" => Some(BrowserCategory::Keys),
        "synth" | "synthesizer" | "synthesiser" => Some(BrowserCategory::Synth),
        _ => None,
    }
}

/// What a lower-case word from a VST3 sub-category or CLAP feature says about an effect.
fn effect_word(w: &str) -> Option<BrowserCategory> {
    use BrowserCategory as C;
    Some(match w {
        "eq" | "equalizer" | "equaliser" => C::Eq,
        "dynamics" | "compressor" | "expander" | "gate" | "limiter" | "transient-shaper" | "de-esser" => C::Dynamics,
        "reverb" => C::Reverb,
        "delay" | "echo" => C::Delay,
        "modulation" | "chorus" | "flanger" | "phaser" | "tremolo" | "vibrato" | "rotary" | "ring-modulator" | "frequency-shifter" | "filter" => {
            C::Modulation
        }
        "distortion" | "saturation" | "saturator" | "bitcrusher" | "lo-fi" => C::Distortion,
        "pitch shift" | "pitch-shifter" | "pitch-correction" | "pitch" => C::Pitch,
        "analyzer" | "analyser" | "meter" | "metering" => C::Analyzer,
        "tools" | "utility" | "spatial" | "surround" | "up-downmix" | "mixing" | "mastering" | "restoration" => C::Utility,
        _ => return None,
    })
}

/// The first word (in the plugin's own order) that names a category, else the fallback.
fn classify(words: &[String], is_instrument: bool) -> BrowserCategory {
    let word = if is_instrument { instrument_word } else { effect_word };
    words.iter().find_map(|w| word(w.trim().to_lowercase().as_str())).unwrap_or(if is_instrument {
        BrowserCategory::Synth
    } else {
        BrowserCategory::Other
    })
}

/// The browser category of a VST3 plugin from its sub-categories (`["Fx", "Reverb"]`).
/// Instruments without a known word are synths; effects without one are Other.
pub fn category_for_vst3(sub_categories: &[String], is_instrument: bool) -> BrowserCategory {
    classify(sub_categories, is_instrument)
}

/// The browser category of a CLAP plugin from its features (`["audio-effect", "reverb"]`).
pub fn category_for_clap(features: &[String], is_instrument: bool) -> BrowserCategory {
    classify(features, is_instrument)
}

/// The browser category of an Audio Unit: instruments by the words of their name (else Synth),
/// effects by the host's category.
fn category_for_au(d: &AuDescriptor) -> BrowserCategory {
    if d.is_instrument {
        let words: Vec<String> = d.name.split_whitespace().map(str::to_string).collect();
        return classify(&words, true);
    }
    from_dsp_category(d.category)
}

/// A host [`Category`] as a browser category (effects).
fn from_dsp_category(c: Category) -> BrowserCategory {
    match c {
        Category::Eq => BrowserCategory::Eq,
        Category::Dynamics => BrowserCategory::Dynamics,
        Category::PitchShift => BrowserCategory::Pitch,
        Category::Reverb => BrowserCategory::Reverb,
        Category::Delay => BrowserCategory::Delay,
        Category::Modulation => BrowserCategory::Modulation,
        Category::Harmonic => BrowserCategory::Distortion,
        Category::Dither => BrowserCategory::Utility,
        Category::Instrument => BrowserCategory::Synth,
        Category::FireStation | Category::Other => BrowserCategory::Other,
    }
}

/// The browser category of a built-in or FireStation plugin.
pub fn category_for_builtin(info: &PluginInfo) -> BrowserCategory {
    use BrowserCategory as C;
    match info.id {
        "sampler" => return C::Sampler,
        "drum_synth" | "juke" => return C::Drums,
        "fs_comp" | "fs_transient" | "fs_shale" | "fs_swale" | "fs_twine" => return C::Dynamics,
        "fs_cleanq" | "fs_vint" | "fs_tarn" => return C::Eq,
        "fs_reverb" | "fs_chamber" => return C::Reverb,
        "fs_delay" => return C::Delay,
        "fs_mod" | "fs_filter" => return C::Modulation,
        "fs_dist" | "fs_sat" | "fs_tape" | "fs_amp" => return C::Distortion,
        "fs_pitch" => return C::Pitch,
        "fs_levelup" => return C::Utility,
        _ => {}
    }
    match info.category {
        Category::Instrument => C::Synth,
        // Gain, trim, invert, time shift, DC offset removal, signal generator.
        Category::Other => C::Utility,
        c => from_dsp_category(c),
    }
}

fn kind_of(is_instrument: bool) -> Kind {
    if is_instrument { Kind::Instrument } else { Kind::Effect }
}

/// One list of every plugin: the listed built-ins (FireStation engines as their own format), then CLAP,
/// VST3 and AU, each id once (the first wins). `failures` are kept as given.
pub fn build_catalog(
    builtins: &[&PluginInfo],
    clap: &[ClapDescriptor],
    vst3: &[Vst3Descriptor],
    au: &[AuDescriptor],
    failures: Vec<ScanFailure>,
) -> Catalog {
    let mut items: Vec<BrowserItem> = Vec::with_capacity(builtins.len() + clap.len() + vst3.len() + au.len());
    // The hidden stock effects are not offered (`PluginInfo::listed`).
    for info in builtins.iter().filter(|i| i.listed()) {
        let fs = info.category == Category::FireStation;
        items.push(BrowserItem {
            id: info.id.to_string(),
            name: info.name.to_string(),
            vendor: if fs { FIRESTATION_VENDOR } else { BUILTIN_VENDOR }.to_string(),
            version: String::new(),
            format: if fs { Format::FireStation } else { Format::BuiltIn },
            kind: kind_of(info.is_instrument),
            category: category_for_builtin(info),
        });
    }
    for d in clap {
        items.push(BrowserItem {
            id: d.id.clone(),
            name: d.name.clone(),
            vendor: d.vendor.clone(),
            version: d.version.clone(),
            format: Format::Clap,
            kind: kind_of(d.is_instrument),
            category: category_for_clap(&d.features, d.is_instrument),
        });
    }
    for d in vst3 {
        items.push(BrowserItem {
            id: d.id.clone(),
            name: d.name.clone(),
            vendor: d.vendor.clone(),
            version: d.version.clone(),
            format: Format::Vst3,
            kind: kind_of(d.is_instrument),
            category: category_for_vst3(&d.sub_categories, d.is_instrument),
        });
    }
    for d in au {
        items.push(BrowserItem {
            id: d.id.clone(),
            name: d.name.clone(),
            vendor: d.vendor.clone(),
            version: d.version.clone(),
            format: Format::Au,
            kind: kind_of(d.is_instrument),
            category: category_for_au(d),
        });
    }
    let mut seen = HashSet::new();
    items.retain(|i| seen.insert(i.id.clone()));
    Catalog { items, failures }
}

/// The indices into `items` the panel shows for `q`. A category override (by plugin id) replaces
/// the plugin's own category for the category chip and the text search. With the Recent chip
/// the order is `recent`'s (newest first); otherwise favourites come first, then by name.
pub fn filter(
    items: &[BrowserItem],
    q: &Query,
    overrides: &BTreeMap<String, BrowserCategory>,
    favourites: &[String],
    recent: &[String],
) -> Vec<usize> {
    let text = q.text.trim().to_lowercase();
    let is_fav = |id: &str| favourites.iter().any(|f| f == id);
    let recency = |id: &str| recent.iter().position(|r| r == id);
    let mut out: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| {
            let category = overrides.get(&it.id).copied().unwrap_or(it.category);
            it.kind == q.kind
                && q.category.is_none_or(|c| c == category)
                && q.vendor.as_deref().is_none_or(|v| v.eq_ignore_ascii_case(&it.vendor))
                && (!q.favourites || is_fav(&it.id))
                && (!q.recent || recency(&it.id).is_some())
                && (text.is_empty()
                    || it.name.to_lowercase().contains(&text)
                    || it.vendor.to_lowercase().contains(&text)
                    || category.label().to_lowercase().contains(&text))
        })
        .map(|(i, _)| i)
        .collect();
    let by_name = |a: &BrowserItem, b: &BrowserItem| -> Ordering { a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)) };
    out.sort_by(|&a, &b| match (items.get(a), items.get(b)) {
        (Some(x), Some(y)) if q.recent => recency(&x.id).cmp(&recency(&y.id)).then_with(|| by_name(x, y)),
        (Some(x), Some(y)) => is_fav(&y.id).cmp(&is_fav(&x.id)).then_with(|| by_name(x, y)),
        _ => a.cmp(&b),
    });
    out
}
