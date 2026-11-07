//! Ogg pages (RFC 3533): written around Opus packets on export, read back on import.

use std::io::Write;

use crate::{AudioError, Result};

/// Ogg's CRC-32 (polynomial 0x04C11DB7, no reflection, no final XOR).
const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            k += 1;
        }
        t[i] = r;
        i += 1;
    }
    t
};

fn crc(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |c, &b| (c << 8) ^ CRC_TABLE[usize::from((c >> 24) as u8 ^ b)])
}

const FLAG_CONTINUED: u8 = 1;
const FLAG_BOS: u8 = 2;
const FLAG_EOS: u8 = 4;
/// A page's body is at most 255 lacing values of up to 255 bytes; start a new one past this.
const PAGE_TARGET: usize = 4096;

/// Writes one logical stream's packets as Ogg pages. The last page is held until
/// [`PageWriter::finish`] so it can carry the end-of-stream flag.
pub(crate) struct PageWriter {
    serial: u32,
    seq: u32,
    /// The page being filled: its lacing values, body and granule (of the last packet it ends).
    lacing: Vec<u8>,
    body: Vec<u8>,
    granule: i64,
    first: bool,
}

impl PageWriter {
    pub(crate) fn new(serial: u32) -> PageWriter {
        PageWriter { serial, seq: 0, lacing: Vec::new(), body: Vec::new(), granule: 0, first: true }
    }

    /// Writes `packet` on a page of its own (the Opus header packets each need one), flushing
    /// whatever was pending first.
    pub(crate) fn header_packet(&mut self, out: &mut impl Write, packet: &[u8]) -> Result<()> {
        self.flush(out, false)?;
        self.push(packet, 0);
        self.flush(out, false)
    }

    /// Queues `packet`, which ends at `granule`, writing the pending page first if it is full.
    pub(crate) fn packet(&mut self, out: &mut impl Write, packet: &[u8], granule: i64) -> Result<()> {
        let segments = packet.len() / 255 + 1;
        if segments > 255 {
            return Err(AudioError::Encode(format!("a {}-byte packet does not fit an Ogg page", packet.len())));
        }
        if self.lacing.len() + segments > 255 || self.body.len() + packet.len() > PAGE_TARGET {
            self.flush(out, false)?;
        }
        self.push(packet, granule);
        Ok(())
    }

    fn push(&mut self, packet: &[u8], granule: i64) {
        let full = packet.len() / 255;
        self.lacing.extend(std::iter::repeat_n(255u8, full));
        self.lacing.push((packet.len() % 255) as u8);
        self.body.extend_from_slice(packet);
        self.granule = granule;
    }

    /// Writes the pending page, marked as the last when `eos`.
    fn flush(&mut self, out: &mut impl Write, eos: bool) -> Result<()> {
        if self.lacing.is_empty() && !eos {
            return Ok(());
        }
        let flags = if self.first { FLAG_BOS } else { 0 } | if eos { FLAG_EOS } else { 0 };
        let mut page = Vec::with_capacity(27 + self.lacing.len() + self.body.len());
        page.extend_from_slice(b"OggS\0");
        page.push(flags);
        page.extend_from_slice(&self.granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.seq.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(self.lacing.len() as u8);
        page.extend_from_slice(&self.lacing);
        page.extend_from_slice(&self.body);
        let sum = crc(&page).to_le_bytes();
        if let Some(slot) = page.get_mut(22..26) {
            slot.copy_from_slice(&sum);
        }
        out.write_all(&page)?;
        self.seq = self.seq.wrapping_add(1);
        self.first = false;
        self.lacing.clear();
        self.body.clear();
        Ok(())
    }

    /// Writes the last page, flagged end-of-stream, with granule `granule`.
    pub(crate) fn finish(mut self, out: &mut impl Write, granule: i64) -> Result<()> {
        self.granule = granule;
        self.flush(out, true)
    }
}

/// The packets of the first logical stream in an Ogg file, with the granule position of the
/// last page that ends a packet.
pub(crate) struct Packets {
    pub(crate) packets: Vec<Vec<u8>>,
    pub(crate) last_granule: i64,
}

fn malformed(why: &str) -> AudioError {
    AudioError::Malformed(format!("Ogg: {why}"))
}

/// Reads the packets of the first logical stream of `bytes` (pages of other streams are
/// skipped), checking every page's CRC. Stops after `max_packets` packets past which the caller
/// has no use for more, and after `max_bytes` of packet data in all.
pub(crate) fn read_packets(bytes: &[u8], max_packets: usize, max_bytes: usize) -> Result<Packets> {
    let mut at = 0usize;
    let mut serial = None;
    let mut packets = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut last_granule = 0i64;
    while at < bytes.len() {
        let head = bytes.get(at..at + 27).ok_or_else(|| malformed("truncated page header"))?;
        if head.get(..4) != Some(b"OggS") || head.get(4) != Some(&0) {
            return Err(malformed("not an Ogg page"));
        }
        let flags = head.get(5).copied().unwrap_or(0);
        let granule = i64::from_le_bytes(head.get(6..14).and_then(|g| g.try_into().ok()).unwrap_or_default());
        let page_serial = u32::from_le_bytes(head.get(14..18).and_then(|g| g.try_into().ok()).unwrap_or_default());
        let stored_crc = u32::from_le_bytes(head.get(22..26).and_then(|g| g.try_into().ok()).unwrap_or_default());
        let n_seg = usize::from(head.get(26).copied().unwrap_or(0));
        let lacing = bytes.get(at + 27..at + 27 + n_seg).ok_or_else(|| malformed("truncated lacing"))?;
        let body_len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
        let page_end = at + 27 + n_seg + body_len;
        let page = bytes.get(at..page_end).ok_or_else(|| malformed("truncated page"))?;
        let mut check = page.to_vec();
        if let Some(slot) = check.get_mut(22..26) {
            slot.copy_from_slice(&[0; 4]);
        }
        if crc(&check) != stored_crc {
            return Err(malformed("page checksum mismatch"));
        }
        at = page_end;
        if *serial.get_or_insert(page_serial) != page_serial {
            continue;
        }
        if flags & FLAG_CONTINUED == 0 && !partial.is_empty() {
            return Err(malformed("a packet ends without its continuation"));
        }
        let mut pos = 27 + n_seg;
        let mut ended_one = false;
        for &l in lacing {
            let seg = page.get(pos..pos + usize::from(l)).ok_or_else(|| malformed("truncated segment"))?;
            pos += usize::from(l);
            total = total.checked_add(seg.len()).filter(|&t| t <= max_bytes).ok_or_else(|| AudioError::TooLarge("Ogg stream is too large".into()))?;
            partial.extend_from_slice(seg);
            if l < 255 {
                packets.push(std::mem::take(&mut partial));
                ended_one = true;
                if packets.len() >= max_packets {
                    return Ok(Packets { packets, last_granule: granule });
                }
            }
        }
        if ended_one && granule >= 0 {
            last_granule = granule;
        }
        if flags & FLAG_EOS != 0 {
            break;
        }
    }
    Ok(Packets { packets, last_granule })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_round_trip_with_their_granules() {
        let mut out = Vec::new();
        let mut w = PageWriter::new(7);
        assert!(w.header_packet(&mut out, b"head").is_ok());
        let packets: Vec<Vec<u8>> = (0..300usize).map(|i| vec![(i % 251) as u8; (i * 37) % 900 + 1]).collect();
        for (i, p) in packets.iter().enumerate() {
            assert!(w.packet(&mut out, p, (i as i64 + 1) * 960).is_ok());
        }
        assert!(w.finish(&mut out, 299 * 960 + 5).is_ok());
        let read = read_packets(&out, usize::MAX, usize::MAX);
        let Ok(read) = read else { panic!("read failed") };
        assert_eq!(read.packets.first().map(Vec::as_slice), Some(&b"head"[..]));
        assert_eq!(&read.packets[1..], &packets[..]);
        assert_eq!(read.last_granule, 299 * 960 + 5);
    }

    #[test]
    fn a_packet_of_exactly_255_bytes_is_terminated() {
        let mut out = Vec::new();
        let mut w = PageWriter::new(1);
        assert!(w.packet(&mut out, &[9u8; 255], 1).is_ok());
        assert!(w.packet(&mut out, &[], 2).is_ok());
        assert!(w.finish(&mut out, 2).is_ok());
        let Ok(read) = read_packets(&out, usize::MAX, usize::MAX) else { panic!("read failed") };
        assert_eq!(read.packets, vec![vec![9u8; 255], Vec::new()]);
    }

    #[test]
    fn corrupt_pages_are_refused() {
        let mut out = Vec::new();
        let mut w = PageWriter::new(1);
        assert!(w.packet(&mut out, b"abc", 1).is_ok());
        assert!(w.finish(&mut out, 1).is_ok());
        let mut bad = out.clone();
        if let Some(b) = bad.last_mut() {
            *b ^= 1;
        }
        assert!(read_packets(&bad, usize::MAX, usize::MAX).is_err());
        assert!(read_packets(&out[..out.len() - 1], usize::MAX, usize::MAX).is_err());
        assert!(read_packets(&out, usize::MAX, 2).is_err());
    }
}
