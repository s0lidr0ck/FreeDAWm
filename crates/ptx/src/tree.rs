// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/tree.rs`. MIT licence, Copyright (c) 2026 John Forward
// (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: the write side (`encode_block`, `build_directory_0002`) removed; the block
// budget lowered from 4 000 000 to `MAX_BLOCKS`; `Block::data` no longer slices; the tests
// rewritten without the writer (and moved to `tests/`).
//! `0x5A` block tree parser.

use crate::PtxError;
use crate::primitives::{u16_at, u32_at};
use crate::scramble::HEADER_LEN;

/// The deepest block nesting read (deeper is refused).
pub const MAX_DEPTH: usize = 64;
/// The most blocks read from one file (more is refused).
pub const MAX_BLOCKS: usize = 2_000_000;

/// Known content types (`tools/ptx_probe.py`) used to reject false `0x5A` matches inside opaque blobs.
pub const KNOWN_CTS: &[u16] = &[
    0x0000, 0x0002, 0x0003, 0x0F3C, 0x0F3D, 0x0F3E, 0x1000, 0x1001, 0x1003, 0x1004, 0x1006, 0x1014, 0x1015, 0x1017, 0x1018, 0x1021, 0x1022, 0x1028,
    0x1029, 0x102A, 0x102D, 0x1033, 0x1038, 0x103A, 0x103D, 0x103E, 0x103F, 0x1040, 0x1041, 0x1042, 0x1045, 0x1048, 0x1049, 0x104A, 0x104B, 0x104C,
    0x104D, 0x104F, 0x1050, 0x1052, 0x1054, 0x1055, 0x1056, 0x1057, 0x1058, 0x1059, 0x2000, 0x2004, 0x2005, 0x2006, 0x200A, 0x200B, 0x200C, 0x200D,
    0x2010, 0x2011, 0x2013, 0x2015, 0x2016, 0x2017, 0x2018, 0x2019, 0x201A, 0x201B, 0x201C, 0x201D, 0x201E, 0x201F, 0x2020, 0x2021, 0x2023, 0x2024,
    0x2025, 0x2026, 0x2027, 0x2028, 0x2029, 0x202A, 0x202B, 0x202C, 0x202D, 0x202E, 0x2030, 0x2031, 0x2032, 0x2033, 0x2036, 0x2037, 0x2038, 0x203A,
    0x203B, 0x203C, 0x203D, 0x203E, 0x2047, 0x2049, 0x204A, 0x204B, 0x204D, 0x2050, 0x2055, 0x2056, 0x2058, 0x205E, 0x205F, 0x2062, 0x2063, 0x2064,
    0x2066, 0x2067, 0x2068, 0x2069, 0x206A, 0x206B, 0x206C, 0x206D, 0x206E, 0x206F, 0x2070, 0x2071, 0x2072, 0x2073, 0x2074, 0x2077, 0x2078, 0x207A,
    0x207B, 0x207E, 0x2086, 0x208C, 0x208E, 0x208F, 0x2096, 0x2097, 0x2098, 0x2099, 0x209F, 0x20A0, 0x20A1, 0x20A2, 0x2103, 0x2104, 0x2106, 0x2107,
    0x210B, 0x210C, 0x2203, 0x2301, 0x2302, 0x2305, 0x230A, 0x230B, 0x2423, 0x2424, 0x2425, 0x2426, 0x2428, 0x242B, 0x242E, 0x242F, 0x2430, 0x2432,
    0x2433, 0x2434, 0x2435, 0x2437, 0x2501, 0x2502, 0x2504, 0x2505, 0x2506, 0x2507, 0x2508, 0x2509, 0x2510, 0x2511, 0x2516, 0x2519, 0x251A, 0x251B,
    0x251C, 0x2523, 0x2526, 0x252A, 0x252C, 0x2530, 0x2550, 0x2551, 0x2552, 0x2553, 0x2554, 0x2556, 0x2557, 0x255F, 0x2567, 0x2580, 0x2581, 0x2582,
    0x2583, 0x2585, 0x2587, 0x2588, 0x2589, 0x258A, 0x258B, 0x258C, 0x258E, 0x258F, 0x2595, 0x2597, 0x259B, 0x259C, 0x25C0, 0x2600, 0x2601, 0x2602,
    0x2603, 0x2604, 0x260A, 0x260B, 0x260C, 0x260D, 0x260E, 0x260F, 0x2611, 0x2613, 0x2614, 0x2615, 0x2616, 0x2617, 0x2619, 0x261B, 0x261C, 0x261D,
    0x261E, 0x2620, 0x2621, 0x2624, 0x2625, 0x2626, 0x2627, 0x2628, 0x2629, 0x262A, 0x262B, 0x262C, 0x262E, 0x262F, 0x2630, 0x2632, 0x2633, 0x2634,
    0x2636, 0x2637, 0x263B, 0x2701, 0x270A, 0x270D, 0x2710, 0x2715, 0x2716, 0x2718, 0x2719, 0x271A, 0x271B, 0x271C, 0x271F, 0x2721, 0x2A00, 0x4000,
    0x4300, 0x4301, 0x4400, 0x4403, 0x4420, 0x4421, 0x4422, 0x4501, 0x4530, 0x4701, 0x4702, 0x4807, 0x4808, 0x4809, 0x4820, 0x4823, 0x4824, 0x4826,
    0x4827, 0x4828, 0x4829, 0x4833, 0x4834, 0x483B, 0x483D, 0x4842, 0x4843, 0x4845, 0x4846, 0x4847, 0x4848, 0x4849, 0x484D, 0x484E, 0x484F, 0x4850,
    0x4852, 0x4856, 0x485A, 0x485D, 0xFFFF,
];

fn is_known_child(bt: u16, ct: u16) -> bool {
    if bt > 64 {
        return false;
    }
    if KNOWN_CTS.contains(&ct) {
        return true;
    }
    matches!(
        ct >> 8,
        0x00 | 0x0F | 0x10 | 0x20 | 0x21 | 0x22 | 0x23 | 0x24 | 0x25 | 0x26 | 0x27 | 0x2A | 0x40 | 0x43 | 0x44 | 0x45 | 0x47 | 0x48 | 0xFF
    )
}

/// One `0x5A` block in a `.ptx` tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Block type (`u16` at `+1`).
    pub block_type: u16,
    /// Content type (`u16` at `+7`).
    pub content_type: u16,
    /// Byte offset of `0x5A` in the descrambled file.
    pub pos: usize,
    /// Payload size field (`u32` at `+3`: includes the 2-byte `content_type` plus data).
    pub size: usize,
    /// Nested child blocks.
    pub kids: Vec<Block>,
}

impl Block {
    /// First data byte after the 9-byte header.
    #[must_use]
    pub const fn data_start(&self) -> usize {
        self.pos.saturating_add(9)
    }

    /// One byte past the end of this block (`pos + 7 + size`).
    #[must_use]
    pub const fn end(&self) -> usize {
        self.pos.saturating_add(7).saturating_add(self.size)
    }

    /// Data slice (`d[pos + 9 .. end]`), clamped safely.
    #[must_use]
    pub fn data<'a>(&self, d: &'a [u8]) -> &'a [u8] {
        let s = self.data_start().min(d.len());
        let e = self.end().min(d.len());
        d.get(s..e).unwrap_or(&[])
    }

    /// Total number of blocks in a forest.
    #[must_use]
    pub fn count(nodes: &[Block]) -> usize {
        nodes.iter().map(|n| 1 + Self::count(&n.kids)).sum()
    }

    /// First block matching `content_type` in depth-first file order.
    #[must_use]
    pub fn find(nodes: &[Block], ct: u16) -> Option<&Block> {
        nodes.iter().find_map(|n| if n.content_type == ct { Some(n) } else { Self::find(&n.kids, ct) })
    }

    /// Depth-first walk collecting all blocks in `nodes`.
    pub fn walk<'a>(nodes: &'a [Block], out: &mut Vec<&'a Block>) {
        for n in nodes {
            out.push(n);
            Self::walk(&n.kids, out);
        }
    }

    /// Return all blocks in `nodes` in depth-first file order.
    #[must_use]
    pub fn walk_all(nodes: &[Block]) -> Vec<&Block> {
        let mut out = Vec::new();
        Self::walk(nodes, &mut out);
        out
    }
}

const fn is_flat_ct(ct: u16) -> bool {
    matches!(ct, 0x0002 | 0x1028 | 0x204D | 0x260A | 0x262F | 0x2637)
}

const fn raw_prefix_len(ct: u16) -> usize {
    match ct {
        0x1054 => 4,
        _ => 0,
    }
}

fn parse_range(d: &[u8], mut pos: usize, end: usize, depth: usize, strict: bool, budget: &mut usize) -> Result<Vec<Block>, PtxError> {
    if depth > MAX_DEPTH {
        return Err(PtxError::Limit("block nesting deeper than 64"));
    }
    let mut out = Vec::new();
    while pos < end {
        if d.get(pos) != Some(&0x5A) || pos + 9 > end {
            pos += 1;
            continue;
        }
        let (Some(bt), Some(sz_u32), Some(ct)) = (u16_at(d, pos + 1), u32_at(d, pos + 3), u16_at(d, pos + 7)) else {
            pos += 1;
            continue;
        };
        let Ok(size) = usize::try_from(sz_u32) else {
            pos += 1;
            continue;
        };
        let block_end = pos.saturating_add(7).saturating_add(size);
        if size < 2 || block_end > end || (strict && !is_known_child(bt, ct)) {
            pos += 1;
            continue;
        }
        *budget = budget.checked_sub(1).ok_or(PtxError::Limit("more than 2 000 000 blocks"))?;
        let child_start = pos.saturating_add(9).saturating_add(raw_prefix_len(ct));
        let kids =
            if is_flat_ct(ct) || child_start > block_end { Vec::new() } else { parse_range(d, child_start, block_end, depth + 1, true, budget)? };
        out.push(Block { block_type: bt, content_type: ct, pos, size, kids });
        pos = block_end;
    }
    Ok(out)
}

/// Parse all top-level blocks after the 20-byte header.
///
/// # Errors
/// Returns [`PtxError::Limit`] or [`PtxError::Layout`] if no valid blocks exist.
pub fn parse_blocks(data: &[u8]) -> Result<Vec<Block>, PtxError> {
    if data.len() < HEADER_LEN + 9 {
        return Err(PtxError::TooShort(data.len()));
    }
    let mut budget = MAX_BLOCKS;
    let tree = parse_range(data, HEADER_LEN, data.len(), 0, false, &mut budget)?;
    if tree.is_empty() {
        return Err(PtxError::Layout { what: "no block after the header", offset: HEADER_LEN });
    }
    Ok(tree)
}
