//! The track colour palette: the 16 original colours (palette indexes 0–15, unchanged since
//! the first release) and a grid of 12 hues by 4 shades (indexes 16–63, a shade per row and a
//! hue per column), plus how a new track picks a colour no other track has.
//!
//! The grid was laid out in OKLab (OKLCH: one lightness and chroma per shade row, hues 30°
//! apart starting at 25°, chroma reduced where a colour would leave sRGB), so the steps look
//! even. The rows are Light, Main, Deep and Dark; Main is what new tracks take first. Every
//! colour reads on the dark graph backgrounds and under a clip name (dark text on Light, Main
//! and Deep, light text on Dark); the UI tests check the contrast.
//!
//! The originals stay a row of their own rather than being folded into the grid: they are not
//! evenly spaced (three blues sit close together), and keeping them whole means a palette index
//! from an old script, test or control client still names exactly the colour it did.
//!
//! Tracks store their actual colour, never an index, so nothing here changes an existing session.

use crate::TRACK_COLORS;

/// Columns of the grid.
pub const HUES: usize = 12;
/// Rows of the grid.
pub const SHADES: usize = 4;
/// The row new tracks take first ([`SHADE_NAMES`]).
pub const MAIN_SHADE: usize = 1;
/// The first grid palette index (after the 16 originals).
pub const GRID_START: usize = TRACK_COLORS.len();
/// Every palette index: the originals then the grid.
pub const PALETTE_LEN: usize = GRID_START + HUES * SHADES;

/// The grid's column names, left to right (OKLCH hue 25°, 55° … 355°).
pub const HUE_NAMES: [&str; HUES] = ["Red", "Orange", "Gold", "Lime", "Green", "Teal", "Cyan", "Sky", "Blue", "Violet", "Magenta", "Pink"];
/// The grid's row names, top to bottom.
pub const SHADE_NAMES: [&str; SHADES] = ["Light", "Main", "Deep", "Dark"];

/// The grid, a row per shade: OKLCH (L, C) of (0.86, 0.075), (0.76, 0.135), (0.66, 0.15),
/// (0.53, 0.13), rounded to sRGB.
pub const GRID: [[[u8; 3]; HUES]; SHADES] = [
    [
        [255, 191, 184],
        [248, 197, 162],
        [232, 206, 153],
        [207, 215, 159],
        [179, 223, 179],
        [156, 226, 206],
        [149, 224, 232],
        [161, 218, 252],
        [188, 209, 254],
        [212, 200, 254],
        [235, 193, 236],
        [250, 189, 211],
    ],
    [
        [251, 141, 133],
        [242, 152, 87],
        [216, 169, 59],
        [175, 187, 77],
        [119, 200, 122],
        [43, 204, 172],
        [8, 200, 216],
        [72, 190, 250],
        [139, 175, 254],
        [183, 159, 253],
        [220, 146, 224],
        [243, 139, 181],
    ],
    [
        [223, 104, 98],
        [214, 117, 35],
        [184, 138, 6],
        [144, 156, 7],
        [78, 169, 84],
        [3, 170, 142],
        [5, 165, 179],
        [0, 158, 219],
        [100, 142, 237],
        [154, 125, 227],
        [191, 111, 196],
        [215, 103, 151],
    ],
    [
        [170, 72, 68],
        [163, 83, 3],
        [136, 102, 2],
        [106, 115, 0],
        [49, 127, 56],
        [9, 126, 104],
        [3, 122, 132],
        [2, 117, 163],
        [70, 104, 182],
        [114, 90, 174],
        [144, 78, 149],
        [163, 71, 112],
    ],
];

/// The column a new track's colour search starts at (Blue: the first track is blue, as before).
const FIRST_HUE: usize = 8;

/// A main-row colour closer than this (OKLab) to a colour already in use counts as taken.
const TAKEN: f32 = 0.03;

/// How much being far from the last track's colour counts, beside being far from every track's.
const RECENT_WEIGHT: f32 = 0.3;

/// The palette index of grid `shade` (row) and `hue` (column).
pub const fn grid_index(shade: usize, hue: usize) -> usize {
    GRID_START + shade * HUES + hue
}

/// The colour at palette index `i`: 0–15 the originals, 16–63 the grid.
pub fn palette_color(i: usize) -> Option<[u8; 3]> {
    match i.checked_sub(GRID_START) {
        None => TRACK_COLORS.get(i).copied(),
        Some(g) => GRID.get(g / HUES).and_then(|row| row.get(g % HUES)).copied(),
    }
}

/// The colour a palette index from a script names: as [`palette_color`], larger indexes wrapping
/// round (old clients sent any number and got it modulo the palette).
pub fn palette_color_wrapping(i: u64) -> [u8; 3] {
    let n = PALETTE_LEN as u64;
    palette_color(usize::try_from(i % n).unwrap_or(0)).unwrap_or(TRACK_COLORS[0])
}

/// The first palette index holding exactly `rgb`, if it is a palette colour.
pub fn palette_index_of(rgb: [u8; 3]) -> Option<usize> {
    (0..PALETTE_LEN).find(|&i| palette_color(i) == Some(rgb))
}

/// A palette colour's name: "Original 3", "Blue, main".
pub fn palette_name(i: usize) -> Option<String> {
    match i.checked_sub(GRID_START) {
        None => (i < GRID_START).then(|| format!("Original {}", i + 1)),
        Some(g) if g < HUES * SHADES => {
            let hue = HUE_NAMES.get(g % HUES)?;
            let shade = SHADE_NAMES.get(g / HUES)?;
            Some(format!("{hue}, {}", shade.to_lowercase()))
        }
        Some(_) => None,
    }
}

/// `rgb` as `#rrggbb`.
pub fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// `#rrggbb` (or `rrggbb`) as a colour.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let byte = |i: usize| t.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok());
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// `rgb` in OKLab (L, a, b): L is 0 (black) to 1 (white).
pub fn oklab(rgb: [u8; 3]) -> [f32; 3] {
    let lin = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    let [r, g, b] = rgb.map(lin);
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// How different two colours look: their distance in OKLab (0 the same; black to white is 1).
pub fn distance(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (a, b) = (oklab(a), oklab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The grid colours in the order a tie goes to: the Main row from Blue round the wheel, then
/// Light, Deep and Dark the same way.
fn candidates() -> impl Iterator<Item = usize> {
    let rows = std::iter::once(MAIN_SHADE).chain((0..SHADES).filter(|r| *r != MAIN_SHADE));
    rows.flat_map(|r| (0..HUES).map(move |k| grid_index(r, (FIRST_HUE + k) % HUES)))
}

/// The palette index (in the grid) of the colour that looks most unlike every colour in `used`,
/// `used` in track order (its last is the newest track's).
///
/// A Main-row colour no track has comes first: of those, the one furthest (OKLab) from the
/// nearest colour in use, with a little weight on being far from the last track's colour too,
/// so consecutive tracks jump round the wheel rather than step along it. Once every Main colour
/// is taken, the same choice over the rest of the grid, and once every colour is taken, over
/// all of them. Ties go to the earlier of [`candidates`], so
/// the answer is the same every time. With nothing used: Blue, Main.
pub fn most_distinct_index(used: &[[u8; 3]]) -> usize {
    let nearest = |c: [u8; 3]| used.iter().map(|u| distance(c, *u)).fold(f32::INFINITY, f32::min);
    let score = |c: [u8; 3]| {
        let n = nearest(c);
        let n = if n.is_finite() { n } else { 10.0 };
        n + RECENT_WEIGHT * used.last().map_or(0.0, |last| distance(c, *last))
    };
    // Main and untaken; else untaken anywhere; else (a full grid) anything.
    let pick = |main_only: bool, untaken: bool| {
        let mut best: Option<(f32, usize)> = None;
        for i in candidates() {
            let Some(c) = palette_color(i) else { continue };
            let main = (i - GRID_START) / HUES == MAIN_SHADE;
            if (main_only && !main) || (untaken && nearest(c) < TAKEN) {
                continue;
            }
            let s = score(c);
            if best.is_none_or(|(b, _)| s > b + 1e-6) {
                best = Some((s, i));
            }
        }
        best.map(|(_, i)| i)
    };
    pick(true, true).or_else(|| pick(false, true)).or_else(|| pick(false, false)).unwrap_or(grid_index(MAIN_SHADE, FIRST_HUE))
}

/// The colour of [`most_distinct_index`].
pub fn most_distinct(used: &[[u8; 3]]) -> [u8; 3] {
    palette_color(most_distinct_index(used)).unwrap_or(TRACK_COLORS[0])
}

/// Colours for `n` tracks in order, each the most distinct from `others` and those before it.
pub fn spread(others: &[[u8; 3]], n: usize) -> Vec<[u8; 3]> {
    let mut used = others.to_vec();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let c = most_distinct(&used);
        used.push(c);
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_sixteen_keep_their_indexes_and_the_grid_follows() {
        for (i, c) in TRACK_COLORS.iter().enumerate() {
            assert_eq!(palette_color(i), Some(*c), "index {i}");
            assert_eq!(palette_name(i), Some(format!("Original {}", i + 1)));
        }
        assert_eq!(PALETTE_LEN, 64);
        assert_eq!(palette_color(16), Some(GRID[0][0]));
        assert_eq!(palette_color(grid_index(MAIN_SHADE, 8)), Some([139, 175, 254]));
        assert_eq!(palette_color(63), Some(GRID[3][11]));
        assert_eq!(palette_color(64), None);
        assert_eq!(palette_name(grid_index(1, 8)).as_deref(), Some("Blue, main"));
        assert_eq!(palette_name(grid_index(3, 0)).as_deref(), Some("Red, dark"));
        assert_eq!(palette_name(64), None);
        // An old script's index above the palette wraps round, as it always did.
        assert_eq!(palette_color_wrapping(3), TRACK_COLORS[3]);
        assert_eq!(palette_color_wrapping(64 + 5), TRACK_COLORS[5]);
        assert_eq!(palette_color_wrapping(u64::MAX), palette_color(usize::try_from(u64::MAX % 64).unwrap()).unwrap());
        // Every colour is found back by its value (the window marks the selected track's).
        for i in 0..PALETTE_LEN {
            let c = palette_color(i).unwrap();
            assert_eq!(palette_color(palette_index_of(c).unwrap()), Some(c));
        }
        assert_eq!(palette_index_of([1, 2, 3]), None);
    }

    #[test]
    fn every_grid_colour_is_its_own() {
        // No two grid colours (or a grid colour and an original) are near-duplicates, and each
        // row is lighter than the next, as the window shows them.
        for i in 0..PALETTE_LEN {
            for j in (i + 1)..PALETTE_LEN {
                let (a, b) = (palette_color(i).unwrap(), palette_color(j).unwrap());
                assert!(distance(a, b) > 0.025, "{i} {a:?} and {j} {b:?} look alike: {}", distance(a, b));
            }
        }
        for r in 1..SHADES {
            for h in 0..HUES {
                assert!(
                    oklab(GRID[r - 1][h])[0] > oklab(GRID[r][h])[0] + 0.05,
                    "{} {} lighter than {}",
                    HUE_NAMES[h],
                    SHADE_NAMES[r - 1],
                    SHADE_NAMES[r]
                );
            }
        }
        // Hues go round the wheel: each column's Main colour is at its own hue angle.
        for h in 0..HUES {
            let [_, a, b] = oklab(GRID[MAIN_SHADE][h]);
            let angle = b.atan2(a).to_degrees().rem_euclid(360.0);
            let want = 25.0 + 30.0 * h as f32;
            let off = (angle - want + 540.0).rem_euclid(360.0) - 180.0;
            assert!(off.abs() < 6.0, "{}: hue {angle} not {want}", HUE_NAMES[h]);
        }
    }

    #[test]
    fn oklab_and_distance_match_known_values() {
        let near = |a: f32, b: f32| (a - b).abs() < 2e-3;
        let w = oklab([255, 255, 255]);
        assert!(near(w[0], 1.0) && near(w[1], 0.0) && near(w[2], 0.0), "{w:?}");
        let k = oklab([0, 0, 0]);
        assert!(near(k[0], 0.0) && near(k[1], 0.0), "{k:?}");
        // sRGB red, green and blue (reference values of the OKLab paper's conversion).
        let r = oklab([255, 0, 0]);
        assert!(near(r[0], 0.6279) && near(r[1], 0.2249) && near(r[2], 0.1258), "{r:?}");
        let g = oklab([0, 255, 0]);
        assert!(near(g[0], 0.8664) && near(g[1], -0.2339) && near(g[2], 0.1795), "{g:?}");
        let b = oklab([0, 0, 255]);
        assert!(near(b[0], 0.4520) && near(b[1], -0.0325) && near(b[2], -0.3115), "{b:?}");
        assert!(near(distance([0, 0, 0], [255, 255, 255]), 1.0));
        assert_eq!(distance([10, 20, 30], [10, 20, 30]), 0.0);
        assert!(near(distance([255, 0, 0], [0, 0, 255]), distance([0, 0, 255], [255, 0, 0])));
        // Perceptual, not RGB: two greys 30 apart in the shadows differ more than two 30 apart
        // in the highlights.
        assert!(distance([20, 20, 20], [50, 50, 50]) > distance([200, 200, 200], [230, 230, 230]));
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex([139, 175, 254]), "#8baffe");
        assert_eq!(parse_hex("#8baffe"), Some([139, 175, 254]));
        assert_eq!(parse_hex(" 8BAFFE "), Some([139, 175, 254]));
        for bad in ["", "#12345", "#1234567", "#gg0000", "#ééé", "12 456"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn new_colours_take_every_main_hue_first_jumping_round_the_wheel() {
        let mut used = Vec::new();
        let mut order = Vec::new();
        for _ in 0..HUES {
            let i = most_distinct_index(&used);
            order.push(i);
            used.push(palette_color(i).unwrap());
        }
        // The twelve Main colours, each once, starting at Blue.
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..HUES).map(|h| grid_index(MAIN_SHADE, h)).collect::<Vec<_>>());
        assert_eq!(order[0], grid_index(MAIN_SHADE, 8), "the first track is blue");
        // Neighbours never sit next to each other on the wheel.
        for w in order.windows(2) {
            let (a, b) = ((w[0] - GRID_START) % HUES, (w[1] - GRID_START) % HUES);
            let step = (a + HUES - b) % HUES;
            assert!(step > 1 && step < HUES - 1, "{} then {}", palette_name(w[0]).unwrap(), palette_name(w[1]).unwrap());
        }
        // The pinned order (deterministic): Blue, Gold, Pink, Teal, then the rest.
        assert_eq!(&order[..4], &[grid_index(1, 8), grid_index(1, 2), grid_index(1, 11), grid_index(1, 5)]);
        // Then other shades, still each the furthest from what is used; nothing repeats until
        // the grid runs out.
        for _ in HUES..(HUES * SHADES) {
            let i = most_distinct_index(&used);
            let c = palette_color(i).unwrap();
            assert!(!used.contains(&c), "{} repeated", palette_name(i).unwrap());
            used.push(c);
        }
        // A full grid still answers.
        assert!(palette_color(most_distinct_index(&used)).is_some());
        assert_eq!(most_distinct(&used), most_distinct(&used));
    }

    #[test]
    fn a_new_colour_is_the_furthest_from_those_in_use() {
        // Brute force: no Main colour that is not taken sits further from every used colour.
        let used = [TRACK_COLORS[0], TRACK_COLORS[5], [30, 200, 60]];
        let pick = most_distinct(&used);
        let nearest = |c: [u8; 3]| used.iter().map(|u| distance(c, *u)).fold(f32::INFINITY, f32::min);
        let best = (0..HUES).map(|h| nearest(GRID[MAIN_SHADE][h])).fold(0.0f32, f32::max);
        assert!(nearest(pick) > best * 0.75, "{pick:?}: {} vs {best}", nearest(pick));
        // Old sessions' colours (originals, customs) count as used like any other: a session
        // whose tracks are all near-blue gets a warm colour next.
        let blues = [TRACK_COLORS[0], TRACK_COLORS[1], TRACK_COLORS[11]];
        let [_, a, _] = oklab(most_distinct(&blues));
        assert!(a > 0.0, "warm (red-ish), not blue: {:?}", most_distinct(&blues));
    }

    #[test]
    fn spread_gives_each_track_its_own_colour_avoiding_the_others() {
        let others = [GRID[MAIN_SHADE][8]];
        let s = spread(&others, 6);
        assert_eq!(s.len(), 6);
        for (i, c) in s.iter().enumerate() {
            assert!(!others.contains(c));
            assert!(!s[..i].contains(c), "{c:?} twice");
        }
        assert_eq!(s, spread(&others, 6), "deterministic");
        assert!(spread(&[], 0).is_empty());
        assert_eq!(spread(&[], 1), vec![GRID[MAIN_SHADE][8]]);
    }
}
