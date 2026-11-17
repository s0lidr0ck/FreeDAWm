//! Mackie Control (MCU) and its extender (XT), as the protocol is publicly documented (the
//! Mackie Control Universal manual's MIDI implementation):
//!
//! - **Faders**: pitch bend on channel `n` (strip 1-8, 9 the master), 14-bit, both ways: the
//!   device sends where the hand moved it, the host where the motor should go.
//! - **Fader touch**: note 0x68-0x6F (strips), 0x70 (master); velocity 0x7F touched, 0 let go.
//! - **Buttons**: note on, 0x7F pressed, 0 released; the host lights the same note (0x7F on,
//!   0 off). Strips: REC 0x00-0x07, SOLO 0x08-0x0F, MUTE 0x10-0x17, SELECT 0x18-0x1F, V-Pot push
//!   0x20-0x27. Bank 0x2E/0x2F, channel 0x30/0x31. Automation READ 0x4A, WRITE 0x4B, TRIM 0x4C,
//!   TOUCH 0x4D, LATCH 0x4E. SAVE 0x50, UNDO 0x51. CYCLE 0x56, CLICK 0x59. REWIND 0x5B, FAST
//!   FWD 0x5C, STOP 0x5D, PLAY 0x5E, RECORD 0x5F.
//! - **V-Pots**: CC 0x10-0x17, bit 6 the direction (set: counter-clockwise), bits 0-5 the
//!   detents. Their LED rings: CC 0x30-0x37, value `mode << 4 | position` (position 1-11, 0 off;
//!   mode 0 a single dot, used for pan).
//! - **Jog wheel**: CC 0x3C, as a V-Pot.
//! - **Scribble strips**: SysEx `F0 00 00 66 <dev> 12 <offset> <text> F7`, `dev` 0x14 for the
//!   MCU, 0x15 for the XT; offsets 0-55 the top row, 56-111 the bottom one, seven characters a
//!   strip.
//! - **Meters**: channel pressure `D0 <strip << 4 | level>`, level 0-12; the device lets a meter
//!   fall by itself, so the host sends it again while there is signal.
//! - **Handshake**: a device that asks (`… 01 <serial ×7> <challenge ×4> F7`) is answered with
//!   `… 02 <serial> <response ×4> F7`, the response worked out from the challenge as the protocol
//!   describes; devices in Mackie mode that never ask work without it.

use super::{Button, STRIPS, SurfaceEvent, SurfaceProtocol, SurfaceState, fader_14, fader_pos, meter_step, strip_text};
use soundcraft_model::AutomationMode;

/// The SysEx header of a device (`dev` 0x14 MCU, 0x15 XT).
fn header(dev: u8) -> [u8; 5] {
    [0xF0, 0x00, 0x00, 0x66, dev]
}

/// Characters a strip has on each row of the LCD.
const LCD_CELL: usize = 7;

/// The LED notes the host lights for the transport and modes, with what turns each on.
const MODE_LEDS: [(u8, fn(&SurfaceState) -> bool); 10] = [
    (0x5E, |s| s.playing),
    (0x5D, |s| !s.playing),
    (0x5F, |s| s.recording),
    (0x56, |s| s.looping),
    (0x59, |s| s.click),
    (0x4A, |s| s.auto == Some(AutomationMode::Read)),
    (0x4B, |s| s.auto == Some(AutomationMode::Write)),
    (0x4C, |s| s.auto == Some(AutomationMode::Trim)),
    (0x4D, |s| matches!(s.auto, Some(AutomationMode::Touch | AutomationMode::TouchLatch))),
    (0x4E, |s| matches!(s.auto, Some(AutomationMode::Latch | AutomationMode::TouchLatch))),
];

/// What was last sent to the device.
struct Sent {
    leds: [Option<bool>; 128],
    faders: [Option<u16>; STRIPS + 1],
    rings: [Option<u8>; STRIPS],
    lcd: Option<Vec<u8>>,
    meters: [u8; STRIPS],
}

impl Default for Sent {
    fn default() -> Self {
        Sent { leds: [None; 128], faders: [None; STRIPS + 1], rings: [None; STRIPS], lcd: None, meters: [0; STRIPS] }
    }
}

/// A Mackie Control unit (see the module docs).
pub struct Mackie {
    dev: u8,
    sent: Box<Sent>,
}

impl Mackie {
    /// The main unit (MCU): transport, master fader, eight strips.
    pub fn main() -> Mackie {
        Mackie { dev: 0x14, sent: Box::default() }
    }

    /// An extender (XT): eight more strips.
    pub fn extender() -> Mackie {
        Mackie { dev: 0x15, sent: Box::default() }
    }

    fn led(&mut self, note: u8, on: bool, out: &mut Vec<Vec<u8>>) {
        let Some(slot) = self.sent.leds.get_mut(usize::from(note & 0x7F)) else { return };
        if *slot != Some(on) {
            *slot = Some(on);
            out.push(vec![0x90, note & 0x7F, if on { 0x7F } else { 0 }]);
        }
    }

    fn fader(&mut self, strip: usize, pos: f32, out: &mut Vec<Vec<u8>>) {
        let v = fader_14(pos);
        let Some(slot) = self.sent.faders.get_mut(strip) else { return };
        if *slot != Some(v) {
            *slot = Some(v);
            out.push(vec![0xE0 | (strip as u8 & 0x0F), (v & 0x7F) as u8, (v >> 7) as u8]);
        }
    }
}

/// The answer to a device's challenge (`l` its four bytes), as the protocol describes it.
pub fn challenge_response(l: [u8; 4]) -> [u8; 4] {
    let [l1, l2, l3, l4] = l.map(i32::from);
    [0x7F & (l1 + (l2 ^ 0xA) - l4), 0x7F & ((l3 >> 4) ^ (l1 + l4)), 0x7F & ((l4 - (l3 << 2)) ^ (l1 | l2)), 0x7F & (l2 - l3 + (0xF0 ^ (l4 << 4)))]
        .map(|x| (x & 0x7F) as u8)
}

/// A V-Pot's or the jog wheel's detents (bit 6: counter-clockwise).
fn detents(v: u8) -> i8 {
    let n = (v & 0x3F) as i8;
    if v & 0x40 != 0 { -n } else { n }
}

impl SurfaceProtocol for Mackie {
    fn id(&self) -> &'static str {
        if self.dev == 0x15 { "mackie_xt" } else { "mackie" }
    }

    fn is_main(&self) -> bool {
        self.dev == 0x14
    }

    fn parse(&mut self, msg: &[u8], out: &mut Vec<SurfaceEvent>) {
        let Some(&status) = msg.first() else { return };
        let d1 = msg.get(1).copied().unwrap_or(0) & 0x7F;
        let d2 = msg.get(2).copied().unwrap_or(0) & 0x7F;
        match status & 0xF0 {
            0xE0 => {
                let strip = status & 0x0F;
                if usize::from(strip) <= STRIPS && (usize::from(strip) < STRIPS || self.is_main()) {
                    out.push(SurfaceEvent::Fader { strip, pos: fader_pos(u16::from(d1) | (u16::from(d2) << 7)) });
                }
            }
            0x90 | 0x80 => {
                let down = status & 0xF0 == 0x90 && d2 > 0;
                let k = d1 & 0x07;
                let button = match d1 {
                    0x00..=0x07 => Button::Rec(k),
                    0x08..=0x0F => Button::Solo(k),
                    0x10..=0x17 => Button::Mute(k),
                    0x18..=0x1F => Button::Select(k),
                    0x20..=0x27 => Button::VPotPress(k),
                    0x68..=0x6F => return out.push(SurfaceEvent::Touch { strip: k, on: down }),
                    0x70 if self.is_main() => return out.push(SurfaceEvent::Touch { strip: STRIPS as u8, on: down }),
                    _ if !self.is_main() => return,
                    0x2E => Button::BankLeft,
                    0x2F => Button::BankRight,
                    0x30 => Button::ChannelLeft,
                    0x31 => Button::ChannelRight,
                    0x4A => Button::Auto(AutomationMode::Read),
                    0x4B => Button::Auto(AutomationMode::Write),
                    0x4C => Button::Auto(AutomationMode::Trim),
                    0x4D => Button::Auto(AutomationMode::Touch),
                    0x4E => Button::Auto(AutomationMode::Latch),
                    0x50 => Button::Save,
                    0x51 => Button::Undo,
                    0x56 => Button::Loop,
                    0x59 => Button::Click,
                    0x5B => Button::Rewind,
                    0x5C => Button::FastForward,
                    0x5D => Button::Stop,
                    0x5E => Button::Play,
                    0x5F => Button::Record,
                    _ => return,
                };
                out.push(SurfaceEvent::Button { button, down });
            }
            0xB0 => match d1 {
                0x10..=0x17 => out.push(SurfaceEvent::VPot { strip: d1 & 0x07, delta: detents(d2) }),
                0x3C if self.is_main() => out.push(SurfaceEvent::Jog(detents(d2))),
                _ => {}
            },
            0xF0 => {
                // F0 00 00 66 <dev> 01 <serial ×7> <challenge ×4> F7: host connection query.
                let h = header(self.dev);
                if msg.len() >= 18 && msg.get(..5) == Some(&h[..]) && msg.get(5) == Some(&0x01) {
                    let serial = msg.get(6..13).unwrap_or(&[]);
                    let ch = msg.get(13..17).unwrap_or(&[]);
                    let c = [ch.first(), ch.get(1), ch.get(2), ch.get(3)].map(|b| b.copied().unwrap_or(0));
                    let mut reply = h.to_vec();
                    reply.push(0x02);
                    reply.extend_from_slice(serial);
                    reply.extend_from_slice(&challenge_response(c));
                    reply.push(0xF7);
                    out.push(SurfaceEvent::Reply(reply));
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, st: &SurfaceState, out: &mut Vec<Vec<u8>>) {
        for (k, s) in st.strips.iter().enumerate() {
            let k8 = k as u8;
            self.led(k8, s.rec, out);
            self.led(0x08 + k8, s.solo, out);
            self.led(0x10 + k8, s.mute, out);
            self.led(0x18 + k8, s.select, out);
            // A held fader's motor stays still (the hand would fight it).
            if !s.touched {
                self.fader(k, if s.empty { 0.0 } else { s.fader }, out);
            }
            // Pan as a single dot: 1 hard left, 6 centre, 11 hard right.
            let ring = if s.empty { 0 } else { (6.0 + s.pan.clamp(-1.0, 1.0) * 5.0).round() as u8 };
            if let Some(slot) = self.sent.rings.get_mut(k)
                && *slot != Some(ring)
            {
                *slot = Some(ring);
                out.push(vec![0xB0, 0x30 + k8, ring]);
            }
            // Meters fall on the device by themselves: sent again while there is signal.
            let m = if s.empty { 0 } else { meter_step(s.meter) };
            if let Some(last) = self.sent.meters.get_mut(k)
                && (m > 0 || *last > 0)
            {
                *last = m;
                out.push(vec![0xD0, (k8 << 4) | m]);
            }
        }
        if self.is_main() {
            if !st.master_touched {
                self.fader(STRIPS, st.master, out);
            }
            for (note, on) in MODE_LEDS {
                self.led(note, on(st), out);
            }
        }
        // The LCD: names on the top row, the pan on the bottom one.
        let mut lcd = Vec::with_capacity(2 * STRIPS * LCD_CELL);
        for s in &st.strips {
            let mut cell = strip_text(if s.empty { "" } else { &s.name }, LCD_CELL - 1);
            cell.push(b' ');
            lcd.extend(cell);
        }
        for s in &st.strips {
            let text = if s.empty {
                String::new()
            } else if s.pan.abs() < 0.005 {
                " <C> ".to_string()
            } else {
                format!("{}{:>3}", if s.pan < 0.0 { "L" } else { "R" }, (s.pan.abs() * 100.0).round() as u32)
            };
            let mut cell = strip_text(&text, LCD_CELL - 1);
            cell.push(b' ');
            lcd.extend(cell);
        }
        if self.sent.lcd.as_ref() != Some(&lcd) {
            let mut m = header(self.dev).to_vec();
            m.extend_from_slice(&[0x12, 0x00]);
            m.extend_from_slice(&lcd);
            m.push(0xF7);
            out.push(m);
            self.sent.lcd = Some(lcd);
        }
    }

    fn reset(&mut self) {
        *self.sent = Sent::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_bytes_become_events() {
        let mut m = Mackie::main();
        let mut ev = Vec::new();
        m.parse(&[0xE2, 0x7F, 0x7F], &mut ev);
        m.parse(&[0x90, 0x6A, 0x7F], &mut ev);
        m.parse(&[0x90, 0x12, 0x7F], &mut ev);
        m.parse(&[0x90, 0x12, 0x00], &mut ev);
        m.parse(&[0xB0, 0x13, 0x43], &mut ev);
        m.parse(&[0xB0, 0x3C, 0x02], &mut ev);
        m.parse(&[0x90, 0x5E, 0x7F], &mut ev);
        assert_eq!(
            ev,
            vec![
                SurfaceEvent::Fader { strip: 2, pos: 1.0 },
                SurfaceEvent::Touch { strip: 2, on: true },
                SurfaceEvent::Button { button: Button::Mute(2), down: true },
                SurfaceEvent::Button { button: Button::Mute(2), down: false },
                SurfaceEvent::VPot { strip: 3, delta: -3 },
                SurfaceEvent::Jog(2),
                SurfaceEvent::Button { button: Button::Play, down: true },
            ]
        );
        // An extender has no transport, master fader or jog wheel.
        let mut xt = Mackie::extender();
        let mut ev = Vec::new();
        xt.parse(&[0x90, 0x5E, 0x7F], &mut ev);
        xt.parse(&[0xE8, 0, 0x40], &mut ev);
        xt.parse(&[0x90, 0x01, 0x7F], &mut ev);
        assert_eq!(ev, vec![SurfaceEvent::Button { button: Button::Rec(1), down: true }]);
    }

    #[test]
    fn a_device_asking_for_the_host_gets_its_answer() {
        let mut m = Mackie::main();
        let mut ev = Vec::new();
        let mut q = vec![0xF0, 0, 0, 0x66, 0x14, 0x01, 1, 2, 3, 4, 5, 6, 7, 0x10, 0x20, 0x30, 0x40, 0xF7];
        m.parse(&q, &mut ev);
        let r = challenge_response([0x10, 0x20, 0x30, 0x40]);
        let mut want = vec![0xF0, 0, 0, 0x66, 0x14, 0x02, 1, 2, 3, 4, 5, 6, 7];
        want.extend_from_slice(&r);
        want.push(0xF7);
        assert_eq!(ev, vec![SurfaceEvent::Reply(want)]);
        assert!(r.iter().all(|b| *b < 0x80));
        q[4] = 0x15;
        ev.clear();
        m.parse(&q, &mut ev);
        assert!(ev.is_empty(), "another device's query is not ours");
    }
}
