//! `Engine 6 - Delay/Source/DelayConfig.h`: the UI's tables (what each style is called and says,
//! how values print, which controls apply right now), plus the tap display the strip's graph
//! draws. The knob -> unit maps are NOT repeated here: they are the engine's own
//! ([`super::delay_engine_dsp`]), as in the C++.

use super::delay_engine_dsp as d;

/// The style table.
pub const STYLES: [&str; 6] = ["Digital", "Rack", "BBD", "Tape", "Drum", "Oil Can"];

/// One line under the style row, saying what you just picked.
pub fn style_blurb(style: i32) -> &'static str {
    const TEXT: [&str; 6] = [
        "Clean repeats. Time changes are seamless.",
        "80s rack delay: a preamp you can push, slightly grainy repeats.",
        "Analog pedal: dark, compressed repeats. Time bends pitch quickly.",
        "Tape echo: warm, wobbly, three heads. Time swoops like a motor.",
        "Drum echo: brighter and steadier than tape, four heads.",
        "Oil can: dark, smeared, warbly. Best short.",
    ];
    usize::try_from(style).ok().and_then(|i| TEXT.get(i)).copied().unwrap_or("")
}

/// Character does something different per style, so the knob is renamed.
pub fn character_label(style: i32) -> &'static str {
    const LABELS: [&str; 6] = ["Crush", "Preamp", "Grit", "Age", "Age", "Smear"];
    usize::try_from(style).ok().and_then(|i| LABELS.get(i)).copied().unwrap_or("Character")
}

/// The note values (`kDivisions`), short -> long.
pub const DIVISIONS: [&str; 16] =
    ["1/16T", "1/16", "1/8T", "1/16.", "1/8", "1/4T", "1/8.", "1/4", "1/2T", "1/4.", "1/2", "1/1T", "1/2.", "1/1", "1/1.", "2/1"];

/// R Note's list (`kDivisionsR`): "Ratio" (use R Offset) in front.
pub const DIVISIONS_R: [&str; 17] =
    ["Ratio", "1/16T", "1/16", "1/8T", "1/16.", "1/8", "1/4T", "1/8.", "1/4", "1/2T", "1/4.", "1/2", "1/1T", "1/2.", "1/1", "1/1.", "2/1"];

/// Mod Rate becomes a note value when Mod Sync is on: 8 bars .. 1/16.
pub const SYNC_RATES: [&str; 8] = ["8 bars", "4 bars", "2 bars", "1 bar", "1/2", "1/4", "1/8", "1/16"];

/// `syncRateIndex`.
pub fn sync_rate_index(mod_rate_knob: f32) -> usize {
    let i = (mod_rate_knob * 8.0).floor();
    if i.is_nan() { 0 } else { i.clamp(0.0, 7.0) as usize }
}

/// JUCE `roundToInt`.
fn round_to_int(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}

fn signed_db(db: f32) -> String {
    format!("{}{db:.1} dB", if db >= 0.0 { "+" } else { "" })
}

/// `formatValue` for Time.
pub fn time_text(v: f32) -> String {
    let ms = d::knob_to_ms(v);
    if ms >= 1000.0 { format!("{:.2} s", ms / 1000.0) } else { format!("{} ms", round_to_int(ms)) }
}

/// `formatValue` for Division at a tempo: "1/8.  ·  375 ms".
pub fn division_text(index: f32, bpm: f32) -> String {
    let i = round_to_int(index).clamp(0, 15);
    let ms = round_to_int(60_000.0 / bpm.max(1.0) * d::division_beats(i));
    format!("{}  \u{b7}  {ms} ms", DIVISIONS[i as usize])
}

pub fn feedback_text(v: f32) -> String {
    format!("{} %", round_to_int(v * 115.0))
}

pub fn percent_text(v: f32) -> String {
    format!("{} %", round_to_int(v * 100.0))
}

pub fn trim_text(v: f32) -> String {
    signed_db(d::knob_to_trim_db(v))
}

pub fn level_text(v: f32) -> String {
    let g = d::knob_to_level_gain(v);
    signed_db(if g > 0.0 { 20.0 * g.log10() } else { -60.0 })
}

pub fn low_cut_text(v: f32) -> String {
    format!("{} Hz", round_to_int(d::knob_to_low_cut_hz(v) as f32))
}

pub fn high_cut_text(v: f32) -> String {
    let f = d::knob_to_high_cut_hz(v);
    if f >= 10_000.0 { format!("{:.1} k", f / 1000.0) } else { format!("{:.2} k", f / 1000.0) }
}

/// Mod Rate in Hz (as printed with Mod Sync off).
pub fn mod_rate_text(v: f32) -> String {
    format!("{:.2} Hz", d::knob_to_mod_rate_hz(v))
}

/// Mod Rate as a note value (Mod Sync on).
pub fn mod_rate_sync_text(v: f32) -> String {
    SYNC_RATES[sync_rate_index(v)].to_string()
}

pub fn noise_text(v: f32) -> String {
    if v <= 0.01 { "Off".to_string() } else { format!("{} %", round_to_int(v * 100.0)) }
}

pub fn filt_mod_text(v: f32) -> String {
    if v <= 0.01 { "Off".to_string() } else { format!("\u{b1} {:.1} oct", 2.0 * v) }
}

pub fn offset_text(v: f32) -> String {
    format!("x {:.2}", 0.5 + v)
}

pub fn swing_text(v: f32) -> String {
    format!("{} %", round_to_int((v - 0.5) * 200.0))
}

pub fn width_text(v: f32) -> String {
    format!("{} %", round_to_int(v * 200.0))
}

pub fn tempo_text(v: f32) -> String {
    format!("{:.1}", d::knob_to_bpm(v))
}

pub fn threshold_text(v: f32) -> String {
    if v <= 0.01 { "Off".to_string() } else { format!("{} dB", round_to_int(d::knob_to_threshold_db(v))) }
}

pub fn focus_text(v: f32) -> String {
    if v <= 0.01 { "Whole note".to_string() } else { format!("{} %", round_to_int(v * 100.0)) }
}

/// The default: tenths.
pub fn tenths_text(v: f32) -> String {
    format!("{}", round_to_int(v * 10.0))
}

/// What applies right now (`whatApplies`): controls that do not apply to the current style /
/// routing are hidden, not greyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applicable {
    /// tape and drum only, and not while Pattern or Reverse rewrite the taps
    pub heads: bool,
    /// Ping-Pong only
    pub swing: bool,
    /// Dual only, and only while R Note is on "Ratio"
    pub offset: bool,
    /// Dual + Sync only
    pub division_r: bool,
    /// Sync swaps the Time knob for a note-value knob
    pub time: bool,
    pub division: bool,
}

pub fn what_applies(style: i32, routing: i32, synced: bool, pattern: i32, reverse: bool, division_r: i32) -> Applicable {
    let r_note_on = routing == 3 && synced && division_r > 0;
    Applicable {
        heads: (style == 3 || style == 4) && pattern == 0 && !reverse,
        swing: routing == 2,
        offset: routing == 3 && !r_note_on,
        division_r: routing == 3 && synced,
        time: !synced,
        division: synced,
    }
}

// ------------------------------------------------------------------ the strip's tap display

/// Which output side a repeat comes out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Both sides (Mono routing).
    Both,
    Left,
    Right,
}

/// One repeat of an impulse: when it comes back (ms), how loud relative to the input going in
/// (the loop's linear gains: heads, feedback, pattern taps; the style's colour, filters,
/// saturation and limiter are left out), and on which side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Echo {
    pub ms: f32,
    pub gain: f32,
    pub side: Side,
    /// A pattern tap (output only, never fed back).
    pub pattern: bool,
}

/// What the taps depend on, from the module's parameters and the engine's readouts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TapSetup {
    pub style: i32,
    pub routing: i32,
    pub heads: i32,
    pub pattern: i32,
    pub reverse: bool,
    pub freeze: bool,
    /// The loop gain (the Feedback knob x 1.15, the engine's `fbSm` target).
    pub feedback: f32,
    /// The echo times the lines run at, ms (`DelayEngineDsp::echo_ms`: the read target plus the
    /// loop's own delay).
    pub ms_l: f32,
    pub ms_r: f32,
    /// The loop's own delay, ms (`loopComp`: diffusion and the shifter, passed once a round).
    pub loop_ms: f32,
}

/// The loop's own delay a module would run with, ms (`loop_geometry` at `sample_rate`, with Oil
/// Can's diffusion from its Character).
pub fn loop_ms(sample_rate: f32, style: i32, character: f32, diffusion: f32, pitch: i32) -> f32 {
    let fs = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
    let g = d::loop_geometry(fs, diffusion + d::style_diff(style, character), d::pitch_ratio_for(pitch), pitch);
    g.loop_comp / fs * 1000.0
}

/// The delay times a module would run at with these parameters and tempo, ms: `updateControl`'s
/// delay-time block (before loop compensation and the buffer clamp, which the engine's readout
/// includes when there is one).
#[allow(clippy::too_many_arguments)]
pub fn delay_ms(time: f32, synced: bool, division: i32, routing: i32, swing: f32, offset: f32, division_r: i32, bpm: f32) -> (f32, f32) {
    let bpm = d::clamp_t(bpm, 20.0, 999.0);
    let mut ms = d::knob_to_ms(time);
    if synced {
        ms = 60_000.0 / bpm * d::division_beats(division);
    }
    let (mut l, mut r) = (ms, ms);
    if routing == 2 {
        let s = (swing - 0.5) * 0.6;
        l = ms * (1.0 + s);
        r = ms * (1.0 - s);
    } else if routing == 3 {
        r = ms * (0.5 + offset);
        if synced && division_r > 0 {
            r = 60_000.0 / bpm * d::division_beats(division_r - 1);
        }
    }
    (l, r)
}

/// The repeats of a single impulse up to `window_ms`, from the engine's own tables: its head
/// table ([`d::head_table`]), its pattern table ([`d::pattern_table`]), its routing (which line
/// feeds which) and its feedback law (`fb = fbSm x headFbNorm`, or `fbSm` while a pattern or
/// Reverse takes the heads out). Freeze re-records the line untouched (loop gain 1). Repeats
/// below -60 dB are dropped and each gain is capped at 1 (the loop's soft limiter tops out at
/// 0 dBFS). Reverse plays each repeat backwards but at the same times, so it reads the same here.
pub fn echo_taps(t: &TapSetup, window_ms: f32, max: usize) -> Vec<Echo> {
    let mut out = Vec::new();
    if window_ms.is_nan() || window_ms <= 0.0 || max == 0 {
        return out;
    }
    let pat_or_rev = t.pattern > 0 || t.reverse;
    let heads = if pat_or_rev { d::head_table(false, t.style, t.heads) } else { d::head_table(d::style_is_multi_head(t.style), t.style, t.heads) };
    let (pat_n, pat_frac, pat_gain) = if t.reverse { (0, [0.0; 4], [0.0; 4]) } else { d::pattern_table(t.pattern) };
    let fb_raw = if t.freeze {
        1.0
    } else if t.feedback.is_finite() {
        t.feedback.max(0.0)
    } else {
        0.0
    };
    let fb = if pat_or_rev || t.freeze { fb_raw } else { fb_raw * heads.fb_norm };
    let floor = 0.001f32;
    let ms_l = if t.ms_l.is_finite() { t.ms_l.max(0.1) } else { 0.1 };
    let ms_r = if t.ms_r.is_finite() { t.ms_r.max(0.1) } else { 0.1 };
    // The engine reads each head at target x (h + 1), target = echo time - loopComp, and a pass
    // adds loopComp once (the diffusion and shifter sit after the heads); pattern taps read the line
    // at target x fraction and skip both.
    let lp = if t.loop_ms.is_finite() { t.loop_ms.clamp(0.0, ms_l.min(ms_r) - 0.05) } else { 0.0 };

    // A line's content is the impulse (where it enters) plus fb x what that line's taps read,
    // tracked as (time, amplitude) arrivals; each arrival in a line reappears at every head.
    // `pending` holds (time ms, amplitude, line) of content written into a line.
    let mut pending: Vec<(f32, f32, usize)> = Vec::new();
    match t.routing {
        // Mono and Ping-Pong record the mono input into the left line only.
        0 | 2 => pending.push((0.0, 1.0, 0)),
        _ => {
            pending.push((0.0, 1.0, 0));
            pending.push((0.0, 1.0, 1));
        }
    }
    let mut guard = 0;
    while let Some(i) = (0..pending.len()).min_by(|a, b| pending[*a].0.total_cmp(&pending[*b].0)) {
        guard += 1;
        if guard > 4096 || out.len() >= max {
            break;
        }
        let (t0, a, line) = pending.swap_remove(i);
        let period = if line == 0 { ms_l } else { ms_r } - lp;
        let side = match (t.routing, line) {
            (0, _) => Side::Both,
            (_, 0) => Side::Left,
            _ => Side::Right,
        };
        // Pattern taps: output only, from the content, at fractions of the delay.
        for k in 0..pat_n.max(0) as usize {
            let at = t0 + period * pat_frac[k];
            let g = (a * pat_gain[k]).min(1.0);
            if pat_gain[k] > 0.0 && at <= window_ms && g >= floor {
                add_echo(&mut out, Echo { ms: at, gain: g, side, pattern: true });
            }
        }
        // The heads: the main repeat(s); their sum is what feeds back.
        let mut fed = Vec::with_capacity(4);
        for (h, g) in heads.gain.iter().enumerate() {
            if *g <= 0.0 {
                continue;
            }
            let at = t0 + period * (h + 1) as f32 + lp;
            let amp = a * g;
            if at <= window_ms && amp.min(1.0) >= floor {
                add_echo(&mut out, Echo { ms: at, gain: amp, side, pattern: false });
            }
            fed.push((at, amp));
        }
        // Mono / Stereo / Dual: a line feeds itself; Ping-Pong: each feeds the other.
        let next = if t.routing == 2 { 1 - line } else { line };
        for (at, amp) in fed {
            let back = (amp * fb).min(1.0);
            if at < window_ms && back >= floor {
                // Repeats that come round at the same moment add (the loop is linear here).
                match pending.iter_mut().find(|p| p.2 == next && (p.0 - at).abs() < 0.01) {
                    Some(p) => p.1 = (p.1 + back).min(1.0),
                    None => pending.push((at, back, next)),
                }
            }
        }
    }
    out.sort_by(|a, b| a.ms.total_cmp(&b.ms));
    out.truncate(max);
    for e in &mut out {
        e.gain = e.gain.min(1.0);
    }
    out
}

/// Adds a repeat, summing it into one already there at the same moment on the same side.
fn add_echo(out: &mut Vec<Echo>, e: Echo) {
    match out.iter_mut().find(|o| o.side == e.side && o.pattern == e.pattern && (o.ms - e.ms).abs() < 0.01) {
        Some(o) => o.gain += e.gain,
        None => out.push(e),
    }
}

/// The feedback decay envelope through `taps`: at each repeat's time, the loudest repeat from
/// there on (a falling upper hull), so the curve is drawn from the same repeats, not a model.
pub fn decay_envelope(taps: &[Echo]) -> Vec<(f32, f32)> {
    let mut env: Vec<(f32, f32)> = Vec::with_capacity(taps.len());
    let mut run = 0.0f32;
    for e in taps.iter().rev() {
        run = run.max(e.gain);
        env.push((e.ms, run));
    }
    env.reverse();
    env
}
