//! HUI, as its MIDI protocol is publicly described:
//!
//! - **Ping**: the host sends `90 00 00` about once a second; the device answers `90 00 7F` and
//!   goes offline when the pings stop.
//! - **Switches and LEDs** are addressed as zone and port. The device sends `B0 0F <zone>` then
//!   `B0 2F <0x40 | port>` (pressed) or `B0 2F <port>` (released); the host lights an LED with
//!   `B0 0C <zone>` then `B0 2C <0x40 | port>` (on) or `B0 2C <port>` (off). Zones 0x00-0x07 are
//!   the strips: port 0 fader touch, 1 select, 2 mute, 3 solo, 4 auto, 5 V-sel, 6 insert, 7
//!   rec/ready. Zone 0x0A: channel left, bank left, channel right, bank right (ports 0-3). Zone
//!   0x0E, the transport: rewind 1, fast forward 2, stop 3, play 4, record 5. Zone 0x0F: return
//!   to zero 0, end 1, loop 3.
//! - **Faders**: both ways, `B0 0<n> <hi>` then `B0 2<n> <lo>`, 14 bits (`hi << 7 | lo`).
//! - **V-Pots**: `B0 4<n> <v>`, bit 6 set for clockwise, the low bits the detents; their rings
//!   `B0 1<n> <v>`, position 1-11 (a single dot), 0 off.
//! - **Jog wheel**: `B0 0D <v>`, as a V-Pot.
//! - **Scribble strips** (four characters): `F0 00 00 66 05 00 10 <strip> <c ×4> F7`.
//! - **Meters**: `A0 <strip> <side << 4 | level>`, side 0 left and 1 right, level 0-12.

use super::{Button, STRIPS, SurfaceEvent, SurfaceProtocol, SurfaceState, fader_14, fader_pos, meter_step, strip_text};

/// How often the host pings.
const PING_MS: u64 = 1000;

/// What was last sent to the device.
#[derive(Default)]
struct Sent {
    /// By zone, the LEDs of ports 0-7.
    leds: [[Option<bool>; 8]; 0x20],
    faders: [Option<u16>; STRIPS],
    rings: [Option<u8>; STRIPS],
    names: [Option<Vec<u8>>; STRIPS],
    meters: [u8; STRIPS],
}

/// A HUI (see the module docs).
pub struct Hui {
    /// The zone the device selected last (its next port message is in it).
    zone: u8,
    /// Each strip's fader high byte, until its low byte comes.
    hi: [u8; STRIPS],
    last_ping: Option<u64>,
    sent: Box<Sent>,
}

impl Default for Hui {
    fn default() -> Self {
        Hui::new()
    }
}

impl Hui {
    pub fn new() -> Hui {
        Hui { zone: 0, hi: [0; STRIPS], last_ping: None, sent: Box::default() }
    }

    fn led(&mut self, zone: u8, port: u8, on: bool, out: &mut Vec<Vec<u8>>) {
        let Some(slot) = self.sent.leds.get_mut(usize::from(zone)).and_then(|z| z.get_mut(usize::from(port))) else { return };
        if *slot != Some(on) {
            *slot = Some(on);
            out.push(vec![0xB0, 0x0C, zone]);
            out.push(vec![0xB0, 0x2C, if on { 0x40 } else { 0 } | (port & 0x07)]);
        }
    }

    fn switch(&self, port: u8, down: bool) -> Option<SurfaceEvent> {
        let z = self.zone;
        let button = match (z, port) {
            (0x00..=0x07, 0) => return Some(SurfaceEvent::Touch { strip: z, on: down }),
            (0x00..=0x07, 1) => Button::Select(z),
            (0x00..=0x07, 2) => Button::Mute(z),
            (0x00..=0x07, 3) => Button::Solo(z),
            (0x00..=0x07, 7) => Button::Rec(z),
            (0x0A, 0) => Button::ChannelLeft,
            (0x0A, 1) => Button::BankLeft,
            (0x0A, 2) => Button::ChannelRight,
            (0x0A, 3) => Button::BankRight,
            (0x0E, 1) => Button::Rewind,
            (0x0E, 2) => Button::FastForward,
            (0x0E, 3) => Button::Stop,
            (0x0E, 4) => Button::Play,
            (0x0E, 5) => Button::Record,
            (0x0F, 0) => Button::ReturnToZero,
            (0x0F, 1) => Button::End,
            (0x0F, 3) => Button::Loop,
            _ => return None,
        };
        Some(SurfaceEvent::Button { button, down })
    }
}

impl SurfaceProtocol for Hui {
    fn id(&self) -> &'static str {
        "hui"
    }

    fn parse(&mut self, msg: &[u8], out: &mut Vec<SurfaceEvent>) {
        let Some(&status) = msg.first() else { return };
        if status & 0xF0 != 0xB0 {
            // The ping's answer (`90 00 7F`) and anything else: nothing to do.
            return;
        }
        let d1 = msg.get(1).copied().unwrap_or(0) & 0x7F;
        let d2 = msg.get(2).copied().unwrap_or(0) & 0x7F;
        match d1 {
            0x00..=0x07 => {
                if let Some(h) = self.hi.get_mut(usize::from(d1)) {
                    *h = d2;
                }
            }
            0x20..=0x27 => {
                let k = d1 & 0x07;
                let hi = self.hi.get(usize::from(k)).copied().unwrap_or(0);
                out.push(SurfaceEvent::Fader { strip: k, pos: fader_pos((u16::from(hi) << 7) | u16::from(d2)) });
            }
            0x0F => self.zone = d2,
            0x2F => {
                if let Some(ev) = self.switch(d2 & 0x07, d2 & 0x40 != 0) {
                    out.push(ev);
                }
            }
            0x40..=0x47 => {
                let n = (d2 & 0x3F) as i8;
                out.push(SurfaceEvent::VPot { strip: d1 & 0x07, delta: if d2 & 0x40 != 0 { n } else { -n } });
            }
            0x0D => {
                let n = (d2 & 0x3F) as i8;
                out.push(SurfaceEvent::Jog(if d2 & 0x40 != 0 { n } else { -n }));
            }
            _ => {}
        }
    }

    fn render(&mut self, st: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        for (k, s) in st.strips.iter().enumerate() {
            let z = k as u8;
            self.led(z, 1, s.select, out);
            self.led(z, 2, s.mute, out);
            self.led(z, 3, s.solo, out);
            self.led(z, 7, s.rec, out);
            if !s.touched {
                let v = fader_14(if s.empty { 0.0 } else { s.fader });
                if let Some(slot) = self.sent.faders.get_mut(k)
                    && *slot != Some(v)
                {
                    *slot = Some(v);
                    out.push(vec![0xB0, z, (v >> 7) as u8]);
                    out.push(vec![0xB0, 0x20 + z, (v & 0x7F) as u8]);
                }
            }
            let ring = if s.empty { 0 } else { (6.0 + s.pan.clamp(-1.0, 1.0) * 5.0).round() as u8 };
            if let Some(slot) = self.sent.rings.get_mut(k)
                && *slot != Some(ring)
            {
                *slot = Some(ring);
                out.push(vec![0xB0, 0x10 + z, ring]);
            }
            let name = strip_text(if s.empty { "" } else { &s.name }, 4);
            if let Some(slot) = self.sent.names.get_mut(k)
                && slot.as_ref() != Some(&name)
            {
                let mut m = vec![0xF0, 0x00, 0x00, 0x66, 0x05, 0x00, 0x10, z];
                m.extend_from_slice(&name);
                m.push(0xF7);
                out.push(m);
                *slot = Some(name);
            }
            let m = if s.empty { 0 } else { meter_step(s.meter) };
            if let Some(last) = self.sent.meters.get_mut(k)
                && (m > 0 || *last > 0)
            {
                *last = m;
                out.push(vec![0xA0, z, m]);
                out.push(vec![0xA0, z, 0x10 | m]);
            }
        }
        self.led(0x0E, 3, !st.playing, out);
        self.led(0x0E, 4, st.playing, out);
        self.led(0x0E, 5, st.recording, out);
        self.led(0x0F, 3, st.looping, out);
    }

    fn tick(&mut self, now_ms: u64, out: &mut Vec<Vec<u8>>) {
        if self.last_ping.is_none_or(|t| now_ms.saturating_sub(t) >= PING_MS) {
            self.last_ping = Some(now_ms);
            out.push(vec![0x90, 0x00, 0x00]);
        }
    }

    fn reset(&mut self) {
        *self.sent = Sent::default();
        self.last_ping = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_and_port_messages_become_buttons() {
        let mut h = Hui::new();
        let mut ev = Vec::new();
        for m in [
            [0xB0, 0x0F, 0x03],
            [0xB0, 0x2F, 0x42],
            [0xB0, 0x2F, 0x02],
            [0xB0, 0x0F, 0x0E],
            [0xB0, 0x2F, 0x44],
            [0xB0, 0x0F, 0x01],
            [0xB0, 0x2F, 0x40],
        ] {
            h.parse(&m, &mut ev);
        }
        // Fader 5 to the top, a V-Pot two clicks counter-clockwise, the ping's answer.
        h.parse(&[0xB0, 0x04, 0x7F], &mut ev);
        h.parse(&[0xB0, 0x24, 0x7F], &mut ev);
        h.parse(&[0xB0, 0x41, 0x02], &mut ev);
        h.parse(&[0x90, 0x00, 0x7F], &mut ev);
        assert_eq!(
            ev,
            vec![
                SurfaceEvent::Button { button: Button::Mute(3), down: true },
                SurfaceEvent::Button { button: Button::Mute(3), down: false },
                SurfaceEvent::Button { button: Button::Play, down: true },
                SurfaceEvent::Touch { strip: 1, on: true },
                SurfaceEvent::Fader { strip: 4, pos: 1.0 },
                SurfaceEvent::VPot { strip: 1, delta: -2 },
            ]
        );
    }

    #[test]
    fn the_host_pings_once_a_second() {
        let mut h = Hui::new();
        let mut out = Vec::new();
        h.tick(0, &mut out);
        h.tick(500, &mut out);
        h.tick(1000, &mut out);
        assert_eq!(out, vec![vec![0x90, 0, 0], vec![0x90, 0, 0]]);
    }
}
