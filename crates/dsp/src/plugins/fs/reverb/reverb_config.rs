//! ReverbConfig: the UI's tables, translated from `Engine 7 - Reverb/Source/ReverbConfig.h` (what each
//! machine is called and says about itself, how every value prints, which controls apply right now),
//! plus [`machine_decay`] and [`decay_shape`], the decay the engine is set to, for the strip's graph.
//!
//! The knob -> unit maps are not repeated here: they are the engine's own ([`super::reverb_engine_dsp`]),
//! so a readout cannot drift away from what the DSP does.

use super::reverb_engine_dsp as d;

// ---------------------------------------------------------------- machine table
/// `getMachines`.
pub const MACHINES: [&str; 6] = ["Room", "Hall", "Plate", "Spring", "Cloud", "Shimmer"];
/// The machine that has its own engine (no colour, no band-split decay, no diffusion, no attack).
pub const SPRING: i32 = 3;
/// The machine with the pitch shifter in its loop (the only one that uses Interval).
pub const SHIMMER: i32 = 5;

/// `machineBlurb`.
pub fn machine_blurb(machine: i32) -> &'static str {
    const TEXT: [&str; 6] = [
        "A real-sized room with early reflections. Short decays sit behind the amp.",
        "Concert hall: slower build, long smooth tail, reflections up front.",
        "Steel plate: dense from the first millisecond, bright. Zing is the metallic whip at the start.",
        "Three-spring tank with a driven input. Dwell is how hard you hit the springs: more drip, more splash.",
        "Huge, slow, modulated wash. Bloom makes the sound take longer to arrive.",
        "Hall with a pitch shifter inside the tail. Every trip round climbs again. -Oct is the dark version.",
    ];
    usize::try_from(machine).ok().and_then(|m| TEXT.get(m)).copied().unwrap_or("")
}

/// `characterLabel`: Character does something different per machine, so the knob is renamed.
pub fn character_label(machine: i32) -> &'static str {
    const LABELS: [&str; 6] = ["Early", "Early", "Zing", "Dwell", "Bloom", "Shimmer"];
    usize::try_from(machine).ok().and_then(|m| LABELS.get(m)).copied().unwrap_or("Character")
}

/// `colorBlurb`: what the era does, when it is not "Now".
pub fn color_blurb(color: i32) -> &'static str {
    const TEXT: [&str; 3] = [
        "1970s: dark and soft. 8 kHz converters, a little grain, modulation that wanders.",
        "1980s: brighter but still rolled off, with the old two-point modulation.",
        "",
    ];
    usize::try_from(color).ok().and_then(|c| TEXT.get(c)).copied().unwrap_or("")
}

/// `headerBlurb`: the era if it says anything (not on the Spring), otherwise the machine.
pub fn header_blurb(machine: i32, color: i32) -> &'static str {
    if machine != SPRING {
        let era = color_blurb(color);
        if !era.is_empty() {
            return era;
        }
    }
    machine_blurb(machine)
}

// ---------------------------------------------------------------- value readouts
/// `juce::roundToInt`: rounds half up (`floor (x + 0.5)`), as JUCE's magic-number rounding does.
fn round_to_int(x: f32) -> i32 {
    (x + 0.5).floor() as i32
}

/// `formatSeconds`.
pub fn format_seconds(s: f32) -> String {
    if s >= 10.0 {
        return format!("{} s", round_to_int(s));
    }
    if s >= 1.0 {
        return format!("{s:.1} s");
    }
    format!("{} ms", round_to_int(s * 1000.0))
}

/// `formatKilohertz`.
pub fn format_kilohertz(f: f64) -> String {
    if f >= 10000.0 { format!("{:.1} k", f / 1000.0) } else { format!("{:.2} k", f / 1000.0) }
}

/// A gain in dB with its sign, one decimal; unity prints +0.0 (the 0.7071 default is 2 v^2 =
/// 0.99998), not -0.0.
fn db_text(g: f32) -> String {
    let mut db = if g > 0.0 { 20.0 * g.log10() } else { -60.0 };
    if db.abs() < 0.05 {
        db = 0.0;
    }
    format!("{}{db:.1} dB", if db >= 0.0 { "+" } else { "" })
}

// `formatValue`, one function per parameter (the parameter table's `display`).
pub fn decay_text(v: f32) -> String {
    format_seconds(d::knob_to_decay_sec(v))
}
pub fn pre_delay_text(v: f32) -> String {
    format!("{} ms", round_to_int(d::knob_to_pre_ms(v)))
}
pub fn mix_text(v: f32) -> String {
    if v > 0.995 { "Wet only".to_string() } else { format!("{} %", round_to_int(v * 100.0)) }
}
/// Size, Diffusion and Mod Depth.
pub fn percent_text(v: f32) -> String {
    format!("{} %", round_to_int(v * 100.0))
}
pub fn level_text(v: f32) -> String {
    db_text(d::knob_to_level_gain(v))
}
pub fn low_cut_text(v: f32) -> String {
    format!("{} Hz", round_to_int(d::knob_to_low_cut_hz(v) as f32))
}
pub fn high_cut_text(v: f32) -> String {
    format_kilohertz(d::knob_to_high_cut_hz(v))
}
pub fn damping_text(v: f32) -> String {
    if v > 0.995 { "Off".to_string() } else { format_kilohertz(d::knob_to_high_cut_hz(v)) }
}
pub fn bass_text(v: f32) -> String {
    format!("x {:.2}", d::knob_to_low_decay_x(v))
}
pub fn mod_rate_text(v: f32) -> String {
    format!("{:.2} Hz", d::knob_to_mod_rate_hz(v))
}
pub fn width_text(v: f32) -> String {
    format!("{} %", round_to_int(v * 200.0))
}
pub fn duck_text(v: f32) -> String {
    if v <= 0.01 { "Off".to_string() } else { format!("{} %", round_to_int(v * 100.0)) }
}
pub fn swell_text(v: f32) -> String {
    if v <= 0.01 { "Off".to_string() } else { format_seconds(d::knob_to_swell_sec(v)) }
}
pub fn attack_text(v: f32) -> String {
    if v <= 0.01 { "Off".to_string() } else { format!("{} ms", round_to_int(d::knob_to_attack_ms(v))) }
}
pub fn tempo_text(v: f32) -> String {
    format!("{:.1}", d::knob_to_bpm(v))
}
/// `default`: tenths (Character).
pub fn tenths_text(v: f32) -> String {
    format!("{}", round_to_int(v * 10.0))
}

/// The division readout: "1/16  ·  125 ms" at the tempo in force.
pub fn division_text(index: i32, bpm: f32) -> String {
    let dv = index.clamp(0, 8);
    let ms = round_to_int(60000.0 / bpm.max(1.0) * d::division_beats(dv));
    format!("{}  \u{b7}  {ms} ms", DIVISIONS.get(dv as usize).copied().unwrap_or(""))
}

/// Pre-delay note values (`kDivisions`). Must stay in step with `division_beats`.
pub const DIVISIONS: [&str; 9] = ["1/64", "1/32", "1/16T", "1/16", "1/8T", "1/16.", "1/8", "1/8.", "1/4"];

// ---------------------------------------------------------------- what applies right now
/// `Applicable`: controls that do not apply to the current machine are hidden, not greyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applicable {
    /// Shimmer only.
    pub interval: bool,
    /// The spring tank has no band-split decay...
    pub low_decay: bool,
    /// ...and no diffusion control.
    pub diffusion: bool,
    /// A spring is a spring in any decade.
    pub color: bool,
    pub attack: bool,
    /// Sync swaps the Pre-Delay knob for a note-value knob.
    pub pre_delay: bool,
    pub division: bool,
}

/// `whatApplies`.
pub fn what_applies(machine: i32, synced: bool) -> Applicable {
    let spring = machine == SPRING;
    Applicable {
        interval: machine == SHIMMER,
        low_decay: !spring,
        diffusion: !spring,
        color: !spring,
        attack: !spring,
        pre_delay: !synced,
        division: synced,
    }
}

// ---------------------------------------------------------------- the decay, for the graph (Rust addition)
/// What `applyMachine` sets that shapes a machine's decay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MachineDecay {
    /// `mDecayScale`: the requested T60 is scaled by this.
    pub decay_scale: f32,
    /// `mBassMult`: multiplied into the Low Decay knob.
    pub bass_mult: f32,
    /// `mHfRatio`: how much shorter than the mids the highs ring.
    pub hf_ratio: f32,
    /// `mLoXover`: the in-loop low band's corner, Hz.
    pub lo_xover: f32,
    /// `mDampMax`: the ceiling on the in-loop high band's corner, Hz.
    pub damp_max: f32,
    /// The Spring: one band, no split.
    pub spring: bool,
}

/// The values `applyMachine` gives machine `m` (any other index is the Shimmer, as the C++ `else`).
/// A test checks them against the engine itself.
pub fn machine_decay(m: i32) -> MachineDecay {
    let base = MachineDecay { decay_scale: 1.0, bass_mult: 1.0, hf_ratio: 0.35, lo_xover: 300.0, damp_max: 20000.0, spring: false };
    match m {
        0 => MachineDecay { hf_ratio: 0.30, lo_xover: 250.0, ..base },
        1 => base,
        2 => MachineDecay { hf_ratio: 0.33, lo_xover: 350.0, bass_mult: 0.50, damp_max: 2500.0, decay_scale: 1.25, ..base },
        // The spring keeps the previous machine's split values; it does not use them.
        3 => MachineDecay { spring: true, ..base },
        4 => MachineDecay { hf_ratio: 0.4, ..base },
        _ => MachineDecay { hf_ratio: 0.45, ..base },
    }
}

/// The decay the engine is set to: per band, the time to fall 60 dB, and where the bands split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecayShape {
    /// The pre-delay before the tail starts, seconds.
    pub pre_s: f32,
    /// T60 of the low band (below `lo_hz`), the mids and the high band (above `hi_hz`), seconds.
    /// The Spring has one band: all three are the same.
    pub t60_low: f32,
    pub t60_mid: f32,
    pub t60_high: f32,
    pub lo_hz: f32,
    pub hi_hz: f32,
    /// Freeze: the loop gains go to unity, the tail holds.
    pub frozen: bool,
}

/// [`DecayShape`] from the engine's own maps (`updateControl`): T60 = Decay x `mDecayScale`; the
/// low band rings `knobToLowDecayX (bass) x mBassMult` times as long, the high band `mHfRatio`
/// times; the high band starts at Damping, capped by the colour (1970s 6 kHz, 1980s 10 kHz; the
/// Spring has no colour) and the machine. `pre_s` is the pre-delay in force (the caller passes the
/// engine's readout when it has one: a synced pre-delay depends on the host tempo).
pub fn decay_shape(machine: i32, decay: f32, bass: f32, damping: f32, color: i32, frozen: bool, pre_s: f32) -> DecayShape {
    let md = machine_decay(machine);
    let t60 = d::knob_to_decay_sec(decay) * md.decay_scale;
    let color_damp_max = match (md.spring, color) {
        (false, 0) => 6000.0,
        (false, 1) => 10000.0,
        _ => 20000.0,
    };
    let hi_hz = d::min_t(d::min_t(d::knob_to_high_cut_hz(damping) as f32, color_damp_max), md.damp_max);
    let (low, high) = if md.spring { (t60, t60) } else { (t60 * d::knob_to_low_decay_x(bass) * md.bass_mult, t60 * md.hf_ratio) };
    DecayShape { pre_s, t60_low: low, t60_mid: t60, t60_high: high, lo_hz: md.lo_xover, hi_hz, frozen }
}
