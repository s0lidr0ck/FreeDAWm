//! `VintConfig.h`, translated: each circuit's name, description, output stage and front panel —
//! the EQs' hardware faces (every control where it sits on the panel, dressed as the box's knob,
//! with its printed scale), the flavour circuits' columns — plus `formatKnob` and `stepSuffix`.

use soundcraft_dsp::plugins::fs::vint::vint_engine_dsp as d;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Knob,
    Step,
    Toggle,
    Seg,
}

/// A flavour column's control (`Control`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Control {
    pub param: usize,
    pub label: &'static str,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub(crate) struct Column {
    pub title: &'static str,
    pub controls: Vec<Control>,
}

/// `FaceStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FaceStyle {
    Round,
    Fluted,
    Chicken,
    Knurled,
    Cream,
    Wing,
    ApiGain,
    ApiFreq,
    SslCap,
    /// Not in the C++: the BBE 882's knob (the Maximizer's panel).
    Silver,
}

/// `ToggleStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToggleStyle {
    Pill,
    Bat,
    Plate,
    Dome,
    Ssl,
    SslRed,
    SslTall,
    /// In the C++ enum; no panel uses it.
    #[allow(dead_code)]
    SslTallRed,
}

/// A printed mark (`Mark`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mark {
    pub pos: f32,
    pub text: &'static str,
}

/// A control on a face (`Placed`): (x, y) the knob's (a toggle's) centre on the 1000 x 300 panel.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    pub param: usize,
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub name: &'static str,
    pub cap: u32,
    pub marks: Vec<Mark>,
    pub minor_ticks: i32,
    pub style: FaceStyle,
    pub name_below: bool,
    pub toggle: ToggleStyle,
    pub inverted: bool,
    pub travel_start: f32,
    pub travel_sweep: f32,
    pub dots: bool,
    pub legend_ink: u32,
}

#[allow(clippy::too_many_arguments)]
fn pl(param: usize, kind: Kind, x: f32, y: f32, w: f32, name: &'static str, cap: u32, marks: Vec<Mark>, minor: i32, style: FaceStyle) -> Placed {
    Placed {
        param,
        kind,
        x,
        y,
        w,
        name,
        cap,
        marks,
        minor_ticks: minor,
        style,
        name_below: false,
        toggle: ToggleStyle::Pill,
        inverted: false,
        travel_start: 0.0,
        travel_sweep: 0.0,
        dots: false,
        legend_ink: 0,
    }
}

fn toggle(param: usize, x: f32, y: f32, w: f32, name: &'static str, style: ToggleStyle, inverted: bool) -> Placed {
    Placed { toggle: style, inverted, ..pl(param, Kind::Toggle, x, y, w, name, 0, vec![], 0, FaceStyle::Round) }
}

impl Placed {
    fn below(mut self) -> Self {
        self.name_below = true;
        self
    }
    fn travel(mut self, start: f32, sweep: f32) -> Self {
        self.travel_start = start;
        self.travel_sweep = sweep;
        self
    }
    fn dots(mut self) -> Self {
        self.dots = true;
        self
    }
    fn ink(mut self, c: u32) -> Self {
        self.legend_ink = c;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PanelText {
    pub x: f32,
    pub y: f32,
    pub text: &'static str,
    pub size: f32,
    pub colour: u32,
    pub vertical: bool,
}

const fn tx(x: f32, y: f32, text: &'static str, size: f32) -> PanelText {
    PanelText { x, y, text, size, colour: 0, vertical: false }
}
const fn txc(x: f32, y: f32, text: &'static str, size: f32, colour: u32, vertical: bool) -> PanelText {
    PanelText { x, y, text, size, colour, vertical }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    HighShelf,
    Peak,
    LowShelf,
    HighPass,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PanelGlyph {
    pub x: f32,
    pub y: f32,
    pub kind: Glyph,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PanelLine {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Face {
    pub plate: u32,
    pub ink: u32,
    pub badge: Option<&'static str>,
    pub controls: Vec<Placed>,
    pub texts: Vec<PanelText>,
    pub meter_x: f32,
    pub meter_y: f32,
    pub meter_w: f32,
    pub meter_h: f32,
    pub lamp_x: f32,
    pub lamp_y: f32,
    pub texture: bool,
    pub led_lamp: bool,
    pub glyphs: Vec<PanelGlyph>,
    pub lines: Vec<PanelLine>,
}

impl Face {
    pub fn empty(&self) -> bool {
        self.controls.is_empty()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CircuitDef {
    pub name: &'static str,
    pub blurb: &'static str,
    /// The hardware credit line (`crate::hardware_credit`); empty for our own designs.
    pub credit: &'static str,
    pub stage: &'static str,
    pub columns: Vec<Column>,
    pub face: Face,
}

// ---- printed scales
fn legend(texts: &[&'static str]) -> Vec<Mark> {
    let n = texts.len();
    texts.iter().enumerate().map(|(i, t)| Mark { pos: if n > 1 { i as f32 / (n - 1) as f32 } else { 0.5 }, text: t }).collect()
}
fn linear_scale(lo: f32, hi: f32, at: &[(f32, &'static str)]) -> Vec<Mark> {
    at.iter().map(|(v, t)| Mark { pos: (v - lo) / (hi - lo), text: t }).collect()
}
fn zero_to_ten() -> Vec<Mark> {
    legend(&["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"])
}
fn gain_twelve() -> Vec<Mark> {
    legend(&["-12", "-9", "-6", "-3", "0", "3", "6", "9", "12"])
}
fn ssl_gain(lo: f32, hi: f32) -> Vec<Mark> {
    linear_scale(
        lo,
        hi,
        &[
            (-15.0, "-15"),
            (-12.0, "12"),
            (-9.0, "9"),
            (-6.0, "6"),
            (-3.0, "3"),
            (0.0, "0"),
            (3.0, "3"),
            (6.0, "6"),
            (9.0, "9"),
            (12.0, "12"),
            (15.0, "+15"),
        ],
    )
}
fn ssl_q() -> Vec<Mark> {
    linear_scale(0.5, 2.5, &[(0.5, ".5"), (1.0, "1"), (1.5, "1.5"), (2.0, "2"), (2.5, "2.5")])
}

// knob caps and panels
const CAP_PULTEC: u32 = 0xFF1B1B1D;
const CAP_NEVE_RED: u32 = 0xFFC7302A;
const CAP_NEVE_GREY: u32 = 0xFF9EA4A9;
const CAP_NEVE_HPF: u32 = 0xFF2159AE;
const CAP_API_BLUE: u32 = 0xFF1F63C6;
pub(crate) const INK_API_BLUE: u32 = 0xFF34ADE8;
const PLATE_NEVE_BLUE: u32 = 0xFF1D4656;
const PLATE_API_BLACK: u32 = 0xFF101112;
const PLATE_SSL_NAVY: u32 = 0xFF1E2433;
const CAP_SSL_RED: u32 = 0xFFC4373F;
const CAP_SSL_GREEN: u32 = 0xFF2C7B53;
const CAP_SSL_BLUE: u32 = 0xFF2A87B8;
const CAP_SSL_BLACK: u32 = 0xFF2B2C30;
const CAP_SSL_CREAM: u32 = 0xFFE9E6DC;
const PLATE_PULTEC: u32 = 0xFF557480;
const INK_LIGHT: u32 = 0xFFF1F3F4;

/// `circuits()`, by circuit index.
pub(crate) fn circuits() -> &'static [CircuitDef; 8] {
    static DEFS: OnceLock<[CircuitDef; 8]> = OnceLock::new();
    DEFS.get_or_init(build)
}

/// `circuit (i)`: clamped to 0..7.
pub(crate) fn circuit(i: i32) -> &'static CircuitDef {
    &circuits()[i.clamp(0, 7) as usize]
}

/// `circuitOrder`: the Passive's two units side by side.
pub(crate) const CIRCUIT_ORDER: [i32; 8] =
    [d::C_PASSIVE, d::C_PASSIVE_MID, d::C_CONSOLE, d::C_PROPORTIONAL, d::C_BRITISH, d::C_ENHANCER, d::C_AIR, d::C_SAMPLER];

fn build() -> [CircuitDef; 8] {
    use FaceStyle as F;
    use Kind as K;
    use ToggleStyle as T;
    let passive = CircuitDef {
        name: "Passive",
        blurb: "The EQP-1A: boost and cut the lows together for weight without mud; a broad or sharp top boost, and a top cut.",
        credit: "Component-level model of the Pultec EQP-1A's passive network. No affiliation; Pultec is a trademark of its owner.",
        stage: if d::VINT_MODELLED_PASSIVE { "Valve makeup amp" } else { "Ideal: the circuit alone" },
        columns: vec![],
        face: Face {
            plate: PLATE_PULTEC,
            ink: INK_LIGHT,
            badge: None,
            controls: vec![
                toggle(d::P_BYPASS, 62.0, 222.0, 60.0, "In", T::Bat, true),
                pl(d::P_PAS_LOW_BOOST, K::Knob, 232.0, 100.0, 125.0, "Boost", CAP_PULTEC, zero_to_ten(), 0, F::Fluted),
                pl(d::P_PAS_LOW_ATTEN, K::Knob, 362.0, 100.0, 125.0, "Atten", CAP_PULTEC, zero_to_ten(), 0, F::Fluted),
                pl(d::P_PAS_HIGH_BOOST, K::Knob, 507.0, 100.0, 125.0, "Boost", CAP_PULTEC, zero_to_ten(), 0, F::Fluted),
                pl(d::P_PAS_HIGH_ATTEN, K::Knob, 637.0, 100.0, 125.0, "Atten", CAP_PULTEC, zero_to_ten(), 0, F::Fluted),
                pl(d::P_PAS_HIGH_ATTEN_FREQ, K::Step, 752.0, 104.0, 100.0, "Atten Sel", CAP_PULTEC, legend(&["5", "10", "20"]), 0, F::Chicken),
                pl(d::P_PAS_LOW_FREQ, K::Step, 300.0, 228.0, 105.0, "Low Frequency", CAP_PULTEC, legend(&["20", "30", "60", "100"]), 0, F::Chicken)
                    .below(),
                pl(
                    d::P_PAS_BANDWIDTH,
                    K::Knob,
                    440.0,
                    228.0,
                    125.0,
                    "Bandwidth",
                    CAP_PULTEC,
                    linear_scale(
                        0.0,
                        1.0,
                        &[
                            (0.0, "0"),
                            (0.1, "1"),
                            (0.2, "2"),
                            (0.3, "3"),
                            (0.4, "4"),
                            (0.5, "5"),
                            (0.6, "6"),
                            (0.7, "7"),
                            (0.8, "8"),
                            (0.9, "9"),
                            (1.0, "10"),
                        ],
                    ),
                    0,
                    F::Fluted,
                )
                .below(),
                pl(
                    d::P_PAS_HIGH_FREQ,
                    K::Step,
                    585.0,
                    228.0,
                    105.0,
                    "High Frequency",
                    CAP_PULTEC,
                    legend(&["3", "4", "5", "8", "10", "12", "16"]),
                    0,
                    F::Chicken,
                )
                .below(),
                pl(d::P_OUTPUT, K::Knob, 885.0, 228.0, 125.0, "Gain", CAP_PULTEC, gain_twelve(), 0, F::Fluted).below(),
            ],
            texts: vec![
                tx(88.0, 108.0, "PROGRAM EQUALIZER", 10.5),
                tx(88.0, 128.0, "EQP-1A", 17.0),
                tx(300.0, 170.0, "CPS", 11.0),
                tx(585.0, 170.0, "KCS", 11.0),
                tx(364.0, 262.0, "SHARP", 9.5),
                tx(518.0, 262.0, "BROAD", 9.5),
            ],
            meter_x: 885.0,
            meter_y: 90.0,
            meter_w: 170.0,
            meter_h: 100.0,
            lamp_x: 122.0,
            lamp_y: 232.0,
            texture: true,
            ..Face::default()
        },
    };

    let console = CircuitDef {
        name: "Console",
        blurb: "The Neve 1073 EQ, solved from Neve's drawings: the tapped-inductor high-pass, the Baxandall LF and 12 kHz shelves, the inductor presence band.",
        credit: "Component-level model of the Neve 1073 EQ. No affiliation; Neve is a trademark of its owner.",
        stage: "Class A and two transformers",
        columns: vec![],
        face: Face {
            plate: PLATE_NEVE_BLUE,
            ink: INK_LIGHT,
            badge: None,
            controls: vec![
                pl(
                    d::P_CON_PRE,
                    K::Step,
                    146.0,
                    138.0,
                    150.0,
                    "",
                    CAP_NEVE_RED,
                    legend(&["-20", "", "10", "", "0", "", "+10", "OFF", "-20", "", "30", "", "40", "", "50", "OFF", "", "60", "", "70", "", "-80"]),
                    0,
                    F::Wing,
                )
                .travel(30.0, 330.0)
                .dots(),
                pl(
                    d::P_CON_HIGH_GAIN,
                    K::Knob,
                    347.0,
                    143.0,
                    140.0,
                    "",
                    CAP_NEVE_GREY,
                    linear_scale(-16.0, 16.0, &[(-16.0, "-"), (0.0, "0"), (16.0, "+")]),
                    16,
                    F::Knurled,
                )
                .travel(40.0, 280.0)
                .dots(),
                pl(d::P_CON_MID_FREQ, K::Step, 517.0, 72.0, 140.0, "", 0, legend(&[".36", ".7", "1.6", "3.2", "4.8", "7.2"]), 0, F::Cream)
                    .travel(120.0, 150.0)
                    .dots(),
                pl(
                    d::P_CON_MID_GAIN,
                    K::Knob,
                    517.0,
                    222.0,
                    140.0,
                    "",
                    CAP_NEVE_GREY,
                    linear_scale(-18.0, 18.0, &[(-18.0, "-"), (0.0, "0"), (18.0, "+")]),
                    18,
                    F::Knurled,
                )
                .travel(40.0, 280.0)
                .dots(),
                pl(d::P_CON_LOW_FREQ, K::Step, 686.0, 72.0, 140.0, "", 0, legend(&["OFF", "35", "60", "110", "220"]), 0, F::Cream)
                    .travel(90.0, 120.0)
                    .dots(),
                pl(
                    d::P_CON_LOW_GAIN,
                    K::Knob,
                    686.0,
                    222.0,
                    140.0,
                    "",
                    CAP_NEVE_GREY,
                    linear_scale(-16.0, 16.0, &[(-16.0, "-"), (0.0, "0"), (16.0, "+")]),
                    16,
                    F::Knurled,
                )
                .travel(40.0, 280.0)
                .dots(),
                pl(d::P_CON_HPF, K::Step, 839.0, 146.0, 140.0, "", CAP_NEVE_HPF, legend(&["OFF", "50", "80", "160", "300"]), 0, F::Wing)
                    .travel(90.0, 120.0)
                    .dots(),
                toggle(d::P_BYPASS, 958.0, 138.0, 78.0, "EQL", T::Plate, true),
            ],
            texts: vec![
                tx(146.0, 26.0, "dB", 12.0),
                tx(60.0, 222.0, "MIC.", 11.0),
                tx(222.0, 58.0, "LINE", 11.0),
                tx(581.0, 110.0, "kHz", 12.0),
                tx(750.0, 110.0, "Hz", 12.0),
                tx(897.0, 198.0, "Hz", 12.0),
            ],
            lamp_x: -1.0,
            lamp_y: -1.0,
            texture: true,
            glyphs: vec![
                PanelGlyph { x: 347.0, y: 16.0, kind: Glyph::HighShelf },
                PanelGlyph { x: 517.0, y: 16.0, kind: Glyph::Peak },
                PanelGlyph { x: 686.0, y: 16.0, kind: Glyph::LowShelf },
                PanelGlyph { x: 839.0, y: 16.0, kind: Glyph::HighPass },
            ],
            ..Face::default()
        },
    };

    let api_gain = || legend(&["12", "9", "6", "4", "2", "", "2", "4", "6", "9", "12"]);
    let api_freq = |param, x, marks: &[&'static str]| {
        pl(param, K::Step, x, 72.0, 128.0, "", CAP_API_BLUE, legend(marks), 0, F::ApiFreq).travel(270.0, -120.0).ink(INK_API_BLUE)
    };
    let proportional = CircuitDef {
        name: "Proportional",
        blurb: "The API 550A, solved from API's drawing of the 550A-1: three Sallen-Key bands in the 2 / 4 / 6 / 9 / 12 dB steps, boost and cut mirror images.",
        credit: "Component-level model of the API 550A. No affiliation; API is a trademark of its owner.",
        stage: "Discrete op-amp and transformer",
        columns: vec![],
        face: Face {
            plate: PLATE_API_BLACK,
            ink: INK_LIGHT,
            badge: None,
            controls: vec![
                api_freq(d::P_PRO_HIGH_FREQ, 185.0, &["5", "7", "10", "12.5", "15"]),
                pl(d::P_PRO_HIGH_GAIN, K::Step, 185.0, 222.0, 128.0, "", 0, api_gain(), 0, F::ApiGain),
                api_freq(d::P_PRO_MID_FREQ, 421.0, &[".4", ".8", "1.5", "3", "5"]),
                pl(d::P_PRO_MID_GAIN, K::Step, 421.0, 222.0, 128.0, "", 0, api_gain(), 0, F::ApiGain),
                api_freq(d::P_PRO_LOW_FREQ, 651.0, &["50", "100", "200", "300", "400"]),
                pl(d::P_PRO_LOW_GAIN, K::Step, 651.0, 222.0, 128.0, "", 0, api_gain(), 0, F::ApiGain),
                toggle(d::P_PRO_FILTER, 821.0, 229.0, 50.0, "", T::Bat, false),
                toggle(d::P_PRO_LOW_SHELF, 903.0, 224.0, 56.0, "", T::Dome, false),
                toggle(d::P_PRO_HIGH_SHELF, 903.0, 149.0, 56.0, "", T::Dome, false),
                toggle(d::P_BYPASS, 903.0, 73.0, 56.0, "", T::Dome, true),
            ],
            texts: vec![
                tx(107.0, 122.0, "HF", 13.0),
                txc(265.0, 122.0, "kHz", 13.0, INK_API_BLUE, false),
                tx(133.0, 166.0, "-", 17.0),
                tx(237.0, 166.0, "+", 17.0),
                tx(343.0, 122.0, "MF", 13.0),
                txc(501.0, 122.0, "kHz", 13.0, INK_API_BLUE, false),
                tx(369.0, 166.0, "-", 17.0),
                tx(473.0, 166.0, "+", 17.0),
                tx(573.0, 122.0, "LF", 13.0),
                txc(731.0, 122.0, "Hz", 13.0, INK_API_BLUE, false),
                tx(599.0, 166.0, "-", 17.0),
                tx(703.0, 166.0, "+", 17.0),
                txc(821.0, 160.0, "FLTR", 13.0, 0, true),
                tx(968.0, 224.0, "L/F", 13.0),
                tx(968.0, 149.0, "H/F", 13.0),
                tx(968.0, 73.0, "IN", 13.0),
                txc(43.0, 150.0, "550A", 17.0, INK_API_BLUE, true),
            ],
            lamp_x: 821.0,
            lamp_y: 70.0,
            texture: false,
            led_lamp: true,
            ..Face::default()
        },
    };

    let ssl = |param, x, y, cap, marks| pl(param, K::Knob, x, y, 120.0, "", cap, marks, 0, F::SslCap).dots();
    let british = CircuitDef {
        name: "British",
        blurb: "The SSL 4000 E channel EQ, solved from SSL's drawings of both cards: Brown is the 82E02, Black the 82E242 -- different values throughout, and an 18 dB/oct high-pass on the black.",
        credit: "Component-level model of the SSL 4000 E channel EQ. No affiliation; SSL and Solid State Logic are trademarks of their owner.",
        stage: "Op-amp line stage",
        columns: vec![],
        face: Face {
            plate: PLATE_SSL_NAVY,
            ink: INK_LIGHT,
            badge: None,
            controls: vec![
                ssl(d::P_BRI_LPF, 82.0, 76.0, CAP_SSL_CREAM, legend(&["OUT", "12", "8", "5", "4", "3.5", "3"])),
                ssl(d::P_BRI_HPF, 82.0, 218.0, CAP_SSL_CREAM, legend(&["OUT", "20", "70", "120", "200", "300", "350"])),
                toggle(d::P_BRI_BLACK, 584.0, 96.0, 76.0, "BLACK", T::Ssl, false),
                ssl(d::P_BRI_HIGH_GAIN, 219.0, 217.0, CAP_SSL_RED, ssl_gain(-16.5, 16.5)),
                toggle(d::P_BRI_HIGH_BELL, 200.0, 58.0, 32.0, "BELL", T::SslTall, false),
                ssl(d::P_BRI_HIGH_FREQ, 292.0, 70.0, CAP_SSL_RED, legend(&["1.5", "2", "5", "8", "10", "14", "16"])),
                ssl(d::P_BRI_HM_GAIN, 361.0, 217.0, CAP_SSL_GREEN, ssl_gain(-15.0, 15.0)),
                ssl(d::P_BRI_HM_FREQ, 433.0, 70.0, CAP_SSL_GREEN, legend(&[".6", ".8", "1.5", "3", "4.5", "6", "7"])),
                ssl(d::P_BRI_HM_Q, 504.0, 217.0, CAP_SSL_GREEN, ssl_q()).travel(135.0, -270.0),
                toggle(d::P_BYPASS, 584.0, 50.0, 76.0, "BYP", T::SslRed, false),
                ssl(d::P_BRI_LM_GAIN, 646.0, 217.0, CAP_SSL_BLUE, ssl_gain(-15.0, 15.0)),
                ssl(d::P_BRI_LM_FREQ, 718.0, 70.0, CAP_SSL_BLUE, legend(&[".2", ".3", ".8", "1", "1.5", "2", "2.5"])),
                ssl(d::P_BRI_LM_Q, 790.0, 217.0, CAP_SSL_BLUE, ssl_q()).travel(135.0, -270.0),
                ssl(d::P_BRI_LOW_FREQ, 862.0, 70.0, CAP_SSL_BLACK, legend(&["30", "50", "100", "200", "300", "450"])),
                ssl(d::P_BRI_LOW_GAIN, 933.0, 217.0, CAP_SSL_BLACK, ssl_gain(-16.5, 16.5)),
                toggle(d::P_BRI_LOW_BELL, 953.0, 58.0, 32.0, "BELL", T::SslTall, false),
            ],
            texts: vec![
                txc(21.0, 207.0, "FILTERS", 13.0, 0, true),
                tx(257.0, 143.0, "HF", 17.0),
                tx(437.0, 152.0, "HMF", 17.0),
                tx(718.0, 153.0, "LMF", 17.0),
                tx(894.0, 147.0, "LF", 17.0),
                tx(82.0, 120.0, "kHz", 11.0),
                tx(82.0, 262.0, "Hz", 11.0),
                tx(219.0, 261.0, "dB", 11.0),
                tx(292.0, 114.0, "kHz", 11.0),
                tx(361.0, 261.0, "dB", 11.0),
                tx(433.0, 114.0, "kHz", 11.0),
                tx(646.0, 261.0, "dB", 11.0),
                tx(718.0, 114.0, "kHz", 11.0),
                tx(862.0, 114.0, "Hz", 11.0),
                tx(933.0, 261.0, "dB", 11.0),
            ],
            lamp_x: -1.0,
            lamp_y: -1.0,
            texture: true,
            lines: [
                (150.0, 297.0, 150.0, 0.0),
                (292.0, 297.0, 292.0, 162.0),
                (292.0, 162.0, 361.0, 120.0),
                (361.0, 120.0, 361.0, 0.0),
                (578.0, 297.0, 578.0, 144.0),
                (530.0, 144.0, 628.0, 144.0),
                (530.0, 144.0, 530.0, 0.0),
                (628.0, 144.0, 628.0, 0.0),
                (859.0, 297.0, 859.0, 162.0),
                (859.0, 162.0, 791.0, 120.0),
                (791.0, 120.0, 791.0, 0.0),
                (21.0, 287.0, 21.0, 247.0),
                (21.0, 166.0, 21.0, 1.0),
            ]
            .into_iter()
            .map(|(x0, y0, x1, y1)| PanelLine { x0, y0, x1, y1 })
            .collect(),
            ..Face::default()
        },
    };

    let c = |param, label, kind| Control { param, label, kind };
    let enhancer = CircuitDef {
        name: "Enhancer",
        blurb: "Adds what a recording lost: new harmonics on the top, phase-aligned bands, a fuller low end.",
        credit: "",
        stage: "Op-amp line stage",
        columns: vec![
            Column { title: "Type", controls: vec![c(d::P_ENH_TYPE, "", K::Seg)] },
            Column {
                title: "Process",
                controls: vec![c(d::P_ENH_AMOUNT, "Amount", K::Knob), c(d::P_ENH_TUNE, "Tune", K::Knob), c(d::P_ENH_LOW, "Low", K::Knob)],
            },
        ],
        face: Face { lamp_x: -1.0, lamp_y: -1.0, ..Face::default() },
    };
    let air = CircuitDef {
        name: "Air",
        blurb: "The Dolby A encode trick: quiet high detail lifted up to 15 dB, loud parts left alone. Air without harshness.",
        credit: "Modelled after the Dolby A noise reduction encoder. No affiliation; Dolby is a trademark of its owner.",
        stage: "Op-amp line stage",
        columns: vec![Column {
            title: "Air",
            controls: vec![c(d::P_AIR_AMOUNT, "Amount", K::Knob), c(d::P_AIR_BANDS, "Bands", K::Seg), c(d::P_AIR_REF, "Dolby Level", K::Knob)],
        }],
        face: Face { lamp_x: -1.0, lamp_y: -1.0, ..Face::default() },
    };
    let sampler = CircuitDef {
        name: "Sampler",
        blurb: "12-bit sampler converters. The 26k machine has no input filter, so the top folds back down; that is the sound.",
        credit: "",
        stage: "the converter itself",
        columns: vec![
            Column { title: "Machine", controls: vec![c(d::P_SMP_MACHINE, "", K::Seg)] },
            Column {
                title: "Converter",
                controls: vec![
                    c(d::P_SMP_RATE, "Rate", K::Knob),
                    c(d::P_SMP_BITS, "Bits", K::Knob),
                    c(d::P_SMP_DRIVE, "Drive", K::Knob),
                    c(d::P_SMP_FILTER, "Filters", K::Toggle),
                ],
            },
        ],
        face: Face { lamp_x: -1.0, lamp_y: -1.0, ..Face::default() },
    };

    let passive_mid = CircuitDef {
        name: "Passive Mid",
        blurb: "The MEQ-5, its own unit: a peak at the low mids, a dip anywhere from 200 Hz to 7 kHz, a peak at the presence range. Broad, passive, inductor curves.",
        credit: "Component-level model of the Pultec MEQ-5's passive network. No affiliation; Pultec is a trademark of its owner.",
        stage: "Ideal: the circuit alone",
        columns: vec![],
        face: Face {
            plate: PLATE_PULTEC,
            ink: INK_LIGHT,
            badge: None,
            controls: vec![
                toggle(d::P_BYPASS, 58.0, 110.0, 60.0, "In", T::Bat, true),
                pl(d::P_PAS_MID_LOW_FREQ, K::Step, 168.0, 168.0, 118.0, "", CAP_PULTEC, legend(&["200", "300", "500", "700", "1000"]), 0, F::Chicken)
                    .travel(-80.0, 160.0),
                pl(d::P_PAS_MID_LOW_PEAK, K::Knob, 290.0, 150.0, 128.0, "Peak", CAP_PULTEC, zero_to_ten(), 0, F::Fluted),
                pl(
                    d::P_PAS_MID_DIP_FREQ,
                    K::Step,
                    416.0,
                    168.0,
                    118.0,
                    "",
                    CAP_PULTEC,
                    legend(&["200", "300", "500", "700", "1", "1.5", "2", "3", "4", "5", "7"]),
                    0,
                    F::Chicken,
                ),
                pl(d::P_PAS_MID_DIP, K::Knob, 538.0, 150.0, 128.0, "Dip", CAP_PULTEC, zero_to_ten(), 0, F::Fluted),
                pl(d::P_PAS_MID_HIGH_FREQ, K::Step, 664.0, 168.0, 110.0, "", CAP_PULTEC, legend(&["1.5", "2", "3", "4", "5"]), 0, F::Chicken),
                pl(d::P_PAS_MID_HIGH_PEAK, K::Knob, 786.0, 150.0, 128.0, "Peak", CAP_PULTEC, zero_to_ten(), 0, F::Fluted),
                pl(d::P_OUTPUT, K::Knob, 915.0, 222.0, 128.0, "Gain", CAP_PULTEC, gain_twelve(), 0, F::Fluted).below(),
            ],
            texts: vec![tx(480.0, 286.0, "MIDRANGE EQUALIZER  MEQ-5", 11.0)],
            meter_x: 915.0,
            meter_y: 88.0,
            meter_w: 140.0,
            meter_h: 88.0,
            lamp_x: 58.0,
            lamp_y: 225.0,
            texture: true,
            ..Face::default()
        },
    };

    [passive, console, proportional, british, enhancer, air, sampler, passive_mid]
}

// ============================================================================ readouts

fn signed_db(db: f32) -> String {
    let db = if db.abs() < 0.05 { 0.0 } else { db };
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}

fn hz(f: f32) -> String {
    if f >= 1000.0 {
        if f >= 10000.0 { format!("{:.1} kHz", f / 1000.0) } else { format!("{:.2} kHz", f / 1000.0) }
    } else {
        format!("{} Hz", f.round() as i32)
    }
}

/// `formatKnob`: a continuous control's readout in its real unit.
pub(crate) fn format_knob(param: usize, v: f32) -> String {
    match param {
        d::P_PAS_LOW_BOOST
        | d::P_PAS_LOW_ATTEN
        | d::P_PAS_HIGH_BOOST
        | d::P_PAS_HIGH_ATTEN
        | d::P_PAS_MID_LOW_PEAK
        | d::P_PAS_MID_DIP
        | d::P_PAS_MID_HIGH_PEAK
        | d::P_PAS_BANDWIDTH => format!("{:.1}", d::knob_to_pultec(v)),
        d::P_CON_LOW_GAIN | d::P_CON_HIGH_GAIN => signed_db(d::knob_to_console_db(v)),
        d::P_CON_MID_GAIN => signed_db(d::knob_to_console_mid_db(v)),
        d::P_BRI_LOW_GAIN | d::P_BRI_HIGH_GAIN => signed_db(d::knob_to_bri_shelf_db(v)),
        d::P_BRI_LM_GAIN | d::P_BRI_HM_GAIN => signed_db(d::knob_to_bri_bell_db(v)),
        d::P_BRI_LM_Q | d::P_BRI_HM_Q => format!("{:.2}", d::knob_to_bri_q(v)),
        d::P_BRI_LOW_FREQ => hz(d::dial(v, &d::BRI_LOW_DIAL)),
        d::P_BRI_LM_FREQ => hz(d::dial(v, &d::BRI_LM_DIAL)),
        d::P_BRI_HM_FREQ => hz(d::dial(v, &d::BRI_HM_DIAL)),
        d::P_BRI_HIGH_FREQ => hz(d::dial(v, &d::BRI_HIGH_DIAL)),
        d::P_BRI_HPF => {
            if v < 0.001 {
                "Off".into()
            } else {
                hz(d::dial(v, &d::BRI_HPF_DIAL))
            }
        }
        d::P_BRI_LPF => {
            if v < 0.001 {
                "Off".into()
            } else {
                hz(d::dial(v, &d::BRI_LPF_DIAL))
            }
        }
        d::P_ENH_TUNE => hz(d::sweep(v, 700.0, 7000.0)),
        d::P_AIR_REF => format!("{:.0} dBFS", d::knob_to_air_ref_db(v)),
        d::P_SMP_RATE => hz(d::knob_to_smp_rate_hz(v)),
        d::P_SMP_BITS => format!("{} bit", d::knob_to_smp_bits(v).round() as i32),
        d::P_SMP_DRIVE => signed_db(d::knob_to_smp_drive_db(v)),
        d::P_DRIVE => signed_db(d::knob_to_drive_db(v)),
        d::P_OUTPUT => signed_db(d::knob_to_out_db(v)),
        _ => format!("{} %", (v * 100.0).round() as i32),
    }
}

/// `stepSuffix`: a stepped control's unit.
fn step_suffix(param: usize) -> &'static str {
    match param {
        d::P_PRO_LOW_GAIN | d::P_PRO_MID_GAIN | d::P_PRO_HIGH_GAIN => " dB",
        d::P_PAS_LOW_FREQ
        | d::P_PAS_MID_LOW_FREQ
        | d::P_PAS_MID_DIP_FREQ
        | d::P_PAS_MID_HIGH_FREQ
        | d::P_CON_HPF
        | d::P_CON_LOW_FREQ
        | d::P_CON_MID_FREQ
        | d::P_PRO_LOW_FREQ
        | d::P_PRO_MID_FREQ
        | d::P_PRO_HIGH_FREQ
        | d::P_PAS_HIGH_FREQ
        | d::P_PAS_HIGH_ATTEN_FREQ => " Hz",
        _ => "",
    }
}

/// `stepText`: a stepped control's reading, the choice's label with its unit ("10k" -> "10 kHz").
pub(crate) fn step_text(param: usize, label: &str) -> String {
    let suffix = step_suffix(param);
    if label == "Off" || suffix.is_empty() {
        return label.to_string();
    }
    if suffix == " Hz"
        && let Some(k) = label.strip_suffix('k')
    {
        return format!("{k} kHz");
    }
    format!("{label}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_panel_is_built() {
        let all = circuits();
        assert_eq!(all.iter().filter(|c| !c.face.empty()).count(), 5, "the five EQs have faces");
        assert_eq!(all[d::C_SAMPLER as usize].columns[1].controls.len(), 4);
        assert_eq!(circuit(99).name, "Passive Mid");
        assert_eq!(step_text(d::P_PAS_HIGH_FREQ, "10k"), "10 kHz");
        assert_eq!(step_text(d::P_CON_HPF, "Off"), "Off");
        assert_eq!(step_text(d::P_PRO_LOW_GAIN, "+2"), "+2 dB");
        assert_eq!(format_knob(d::P_PAS_LOW_BOOST, 0.5), "5.0");
        assert_eq!(format_knob(d::P_SMP_RATE, 1.0), "48.0 kHz");
        // Each face's printed scale runs 0..1 along the travel.
        for c in all {
            for p in &c.face.controls {
                assert!(p.marks.iter().all(|m| (-0.001..=1.001).contains(&m.pos)), "{} {}", c.name, p.param);
            }
        }
    }
}
