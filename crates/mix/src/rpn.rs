//! Registered and non-registered parameters (RPN, NRPN) when controllers are chased.
//!
//! An RPN or NRPN is not one controller but a little protocol: CC 101/100 (RPN) or 99/98 (NRPN)
//! select a parameter, then Data Entry (CC 6, and CC 38 for its fine part) sets it. Chasing each
//! controller's last value on its own, in controller order, sent Data Entry before the select and
//! only the last value of all: a pitch-bend range (RPN 0) set at the start of a clip reached
//! whatever parameter was selected last, or none. [`ParamChase`] follows the protocol while the
//! controllers before the play position are read and sends, for each parameter that was set, its
//! select and its value, then the selection that was in effect, so later Data Entry in the clip
//! lands where it did. Fixed storage: no allocation (more than [`MAX_PARAMS`] parameters keep the
//! latest ones).
//!
//! Bank select (CC 0 and 32) needs nothing here: chased controllers go out before the program
//! change, so the bank is chosen before the program.

/// Parameters remembered per channel while chasing.
pub(crate) const MAX_PARAMS: usize = 16;

/// The controllers [`ParamChase`] handles instead of the plain chase: Data Entry (6, 38),
/// increment and decrement (96, 97) and the NRPN and RPN selects (98-101).
pub(crate) fn is_param_cc(n: u8) -> bool {
    matches!(n, 6 | 38 | 96..=101)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Param {
    nrpn: bool,
    msb: u8,
    lsb: u8,
    data: u8,
    fine: Option<u8>,
}

/// The parameters set before the play position (see the module docs).
#[derive(Debug, Clone, Default)]
pub(crate) struct ParamChase {
    /// The selects as last received: RPN MSB, RPN LSB, NRPN MSB, NRPN LSB.
    sel: [Option<u8>; 4],
    /// The last select touched was an NRPN one.
    nrpn: bool,
    params: [Param; MAX_PARAMS],
    n: usize,
    /// Where the next parameter goes once the table is full (the oldest).
    next: usize,
    /// The parameter Data Entry goes to now, as an index into `params`.
    current: Option<usize>,
}

impl ParamChase {
    /// A controller read before the play position, in time order.
    pub(crate) fn cc(&mut self, n: u8, value: u8) {
        let v = value & 0x7F;
        match n {
            98..=101 => {
                let k = usize::from(101 - n);
                if let Some(s) = self.sel.get_mut(k) {
                    *s = Some(v);
                }
                self.nrpn = n <= 99;
                self.current = None;
            }
            6 => {
                let (msb, lsb) = if self.nrpn { (self.sel[2], self.sel[3]) } else { (self.sel[0], self.sel[1]) };
                let (Some(msb), Some(lsb)) = (msb, lsb) else { return };
                // RPN 127/127 is the "null" parameter: nothing is set.
                if !self.nrpn && msb == 127 && lsb == 127 {
                    return;
                }
                let nrpn = self.nrpn;
                let at = match self.params.iter().take(self.n).position(|p| p.nrpn == nrpn && p.msb == msb && p.lsb == lsb) {
                    Some(i) => i,
                    None if self.n < MAX_PARAMS => {
                        self.n += 1;
                        self.n - 1
                    }
                    None => {
                        let i = self.next;
                        self.next = (self.next + 1) % MAX_PARAMS;
                        i
                    }
                };
                if let Some(p) = self.params.get_mut(at) {
                    *p = Param { nrpn, msb, lsb, data: v, fine: None };
                }
                self.current = Some(at);
            }
            38 => {
                if let Some(p) = self.current.and_then(|i| self.params.get_mut(i)) {
                    p.fine = Some(v);
                }
            }
            _ => {}
        }
    }

    /// The messages that put an instrument in the state read (channel 1; the caller sets the
    /// channel): each parameter's select and value, then the selection in effect.
    pub(crate) fn send(&self, mut out: impl FnMut([u8; 3])) {
        for p in self.params.iter().take(self.n) {
            let (hi, lo) = if p.nrpn { (99, 98) } else { (101, 100) };
            out([0xB0, hi, p.msb]);
            out([0xB0, lo, p.lsb]);
            out([0xB0, 6, p.data]);
            if let Some(f) = p.fine {
                out([0xB0, 38, f]);
            }
        }
        let (a, b, hi, lo) = if self.nrpn { (self.sel[2], self.sel[3], 99, 98) } else { (self.sel[0], self.sel[1], 101, 100) };
        if self.n > 0 || a.is_some() || b.is_some() {
            if let Some(a) = a {
                out([0xB0, hi, a]);
            }
            if let Some(b) = b {
                out([0xB0, lo, b]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(p: &ParamChase) -> Vec<[u8; 3]> {
        let mut v = Vec::new();
        p.send(|m| v.push(m));
        v
    }

    #[test]
    fn each_parameter_is_selected_before_its_value_and_the_selection_is_restored() {
        let mut p = ParamChase::default();
        // Pitch-bend range 12 semitones (RPN 0), fine tuning (RPN 1) MSB 70 LSB 3, then an NRPN
        // selected with no data yet.
        for (n, v) in [(101, 0), (100, 0), (6, 12), (38, 0), (101, 0), (100, 1), (6, 70), (38, 3), (99, 5), (98, 9)] {
            p.cc(n, v);
        }
        assert_eq!(
            sent(&p),
            vec![
                [0xB0, 101, 0],
                [0xB0, 100, 0],
                [0xB0, 6, 12],
                [0xB0, 38, 0],
                [0xB0, 101, 0],
                [0xB0, 100, 1],
                [0xB0, 6, 70],
                [0xB0, 38, 3],
                [0xB0, 99, 5],
                [0xB0, 98, 9],
            ]
        );
        // A later value of a parameter replaces its earlier one; the null RPN sets nothing.
        let mut p = ParamChase::default();
        for (n, v) in [(101, 0), (100, 0), (6, 2), (6, 24), (101, 127), (100, 127), (6, 99)] {
            p.cc(n, v);
        }
        assert_eq!(sent(&p), vec![[0xB0, 101, 0], [0xB0, 100, 0], [0xB0, 6, 24], [0xB0, 101, 127], [0xB0, 100, 127]]);
        // Data Entry with no select is lost, as on an instrument.
        let mut p = ParamChase::default();
        p.cc(6, 40);
        assert!(sent(&p).is_empty());
        // Many parameters: the latest kept.
        let mut p = ParamChase::default();
        for k in 0..(MAX_PARAMS as u8 + 4) {
            for (n, v) in [(99, 1), (98, k), (6, k)] {
                p.cc(n, v);
            }
        }
        let values: Vec<u8> = sent(&p).iter().filter(|m| m[1] == 6).map(|m| m[2]).collect();
        assert_eq!(values.len(), MAX_PARAMS);
        assert!(values.contains(&(MAX_PARAMS as u8 + 3)) && !values.contains(&0));
    }
}
