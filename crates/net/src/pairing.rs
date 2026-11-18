//! Pairing: the first time a browser or PC connects, it asks to pair; this PC shows a 4-digit
//! code on its screen; whoever types that code on the device gets a token, remembered on both
//! sides. A guest on the Wi-Fi can ask, but without the code (which only the booth's screen
//! shows) gets nothing: a request takes 3 wrong codes, then it is gone; requests expire after 2
//! minutes; at most 4 wait at once and at most 20 are made in 10 minutes.
//!
//! A browser (the phone's page) sends the code itself. A PC never does: the code is the password
//! of a password-authenticated key exchange ([`exchange`], SPAKE2), and each side then proves it
//! derived the same key over the two PCs' link keys as it sees them, so a machine sitting
//! between the two (holding another key towards each) fails the check unless it guessed the
//! code, one guess per try. Each exchange a PC starts spends one of the request's tries.

use zeroize::Zeroizing;

/// The PC pairing this FreeDAWm speaks: the `pairing` field of `pair.request` and of its answer.
/// 2: the code proves both PCs' keys ([`exchange`]). A PC that sends none sent the code itself
/// (FreeDAWm before this) and is told to update.
pub const PAIRING_VERSION: u64 = 2;

/// One request waiting for its code.
#[derive(Clone, PartialEq)]
pub struct PendingPair {
    pub id: String,
    /// What the device calls itself ("Phone", "Piano PC").
    pub name: String,
    /// `browser` or `pc`.
    pub kind: String,
    /// The 4 digits shown on this PC's screen.
    pub code: String,
    /// Seconds (the caller's clock) it was made at.
    pub made: f64,
    pub wrong: u8,
}

/// The code is left out (so it never reaches a log by way of `{:?}`).
impl std::fmt::Debug for PendingPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingPair")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("made", &self.made)
            .field("wrong", &self.wrong)
            .finish_non_exhaustive()
    }
}

pub const EXPIRES_S: f64 = 120.0;
pub const MAX_WRONG: u8 = 3;
pub const MAX_PENDING: usize = 4;
/// Requests allowed per [`WINDOW_S`].
pub const MAX_RECENT: usize = 20;
pub const WINDOW_S: f64 = 600.0;

#[derive(Debug, Default)]
pub struct Pairing {
    pending: Vec<PendingPair>,
    /// When recent requests were made (for the rate limit).
    recent: Vec<f64>,
}

/// What a confirmation found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Paired: the device's name and kind (the caller makes and stores the token).
    Paired {
        name: String,
        kind: String,
    },
    Wrong {
        left: u8,
    },
    /// No such request (expired, used up or never made).
    Unknown,
}

impl Pairing {
    /// The requests waiting, oldest first (what the screen shows): not those whose last try an
    /// exchange under way is spending.
    pub fn pending(&self, now: f64) -> Vec<PendingPair> {
        self.pending.iter().filter(|p| now - p.made < EXPIRES_S && p.wrong < MAX_WRONG).cloned().collect()
    }

    /// A new request from `name`; `random` gives fresh random numbers (the code and the id).
    /// `None` when too many were made lately.
    pub fn request(&mut self, name: &str, kind: &str, now: f64, mut random: impl FnMut() -> u64) -> Option<PendingPair> {
        self.expire(now);
        self.recent.retain(|t| now - t < WINDOW_S);
        if self.recent.len() >= MAX_RECENT {
            return None;
        }
        self.recent.push(now);
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        let name = clean_name(name);
        let kind = if kind == "pc" { "pc" } else { "browser" };
        let p =
            PendingPair { id: format!("{:016x}", random()), name, kind: kind.into(), code: format!("{:04}", random() % 10_000), made: now, wrong: 0 };
        self.pending.push(p.clone());
        Some(p)
    }

    /// The browser typed `code` for its request `id` (a browser's only: a PC's code is never
    /// sent, see [`Pairing::begin_exchange`]).
    pub fn confirm(&mut self, id: &str, code: &str, now: f64) -> Confirm {
        self.expire(now);
        let Some(i) = self.pending.iter().position(|p| p.id == id && p.kind == "browser") else { return Confirm::Unknown };
        let Some(p) = self.pending.get_mut(i) else { return Confirm::Unknown };
        if p.code == code.trim() {
            let p = self.pending.remove(i);
            return Confirm::Paired { name: p.name, kind: p.kind };
        }
        p.wrong = p.wrong.saturating_add(1);
        let left = MAX_WRONG.saturating_sub(p.wrong);
        if left == 0 {
            self.pending.remove(i);
        }
        Confirm::Wrong { left }
    }

    /// A PC starts the key exchange for its request `id`: one of the request's tries is spent
    /// now (whoever runs the exchange learns whether its code was right), and the code comes
    /// back to run it with. `None`: no such PC request, or its tries are spent.
    pub fn begin_exchange(&mut self, id: &str, now: f64) -> Option<Zeroizing<String>> {
        self.expire(now);
        let p = self.pending.iter_mut().find(|p| p.id == id && p.kind == "pc")?;
        if p.wrong >= MAX_WRONG {
            return None;
        }
        p.wrong = p.wrong.saturating_add(1);
        Some(Zeroizing::new(p.code.clone()))
    }

    /// How the exchange begun for `id` ended: `ok` (both sides' confirmations matched) pairs;
    /// otherwise its try stays spent, and after the last one the request is gone.
    pub fn finish_exchange(&mut self, id: &str, ok: bool, now: f64) -> Confirm {
        self.expire(now);
        let Some(i) = self.pending.iter().position(|p| p.id == id && p.kind == "pc") else { return Confirm::Unknown };
        if ok {
            let p = self.pending.remove(i);
            return Confirm::Paired { name: p.name, kind: p.kind };
        }
        let left = self.pending.get(i).map_or(0, |p| MAX_WRONG.saturating_sub(p.wrong));
        if left == 0 {
            self.pending.remove(i);
        }
        Confirm::Wrong { left }
    }

    /// Turns a request down (Deny on this PC's screen).
    pub fn deny(&mut self, id: &str) {
        self.pending.retain(|p| p.id != id);
    }

    fn expire(&mut self, now: f64) {
        self.pending.retain(|p| now - p.made < EXPIRES_S);
    }
}

/// Text from the other PC for a status line: no control characters, at most `max` characters.
pub fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// A device's name as it asked to pair: no control characters, at most 40, "Device" if empty.
pub fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    if name.trim().is_empty() { "Device".to_string() } else { name.trim().to_string() }
}

/// A PC's first pairing: the code on the booth's screen proves both PCs' link keys.
///
/// SPAKE2 over Ed25519 (the `spake2` crate): the remote PC is side A (identity `freedawm-pc`),
/// the booth side B (`freedawm-booth`), the password the code. Each sends its 33-byte message and
/// derives the shared key `k`; each then sends
/// `HMAC-SHA256(HKDF-SHA256(k, info "freedawm pairing confirm"), role || booth key || PC key)`,
/// the role being its own identity and the keys the two link keys as it sees them in its TLS
/// session, each field after a byte giving its length (0 for a key on the plain link, which has
/// none). A machine in the middle holds a different key towards each side, so the two sides'
/// confirmations cannot match unless it guessed the code. The confirmations are compared in
/// constant time (`hmac`'s `verify_slice`); the code itself is never sent. No cryptography is
/// written here: only the crates' calls and the order of the fields.
///
/// What is wiped when done: this side's copies of the code, `k` and the confirmation key. The
/// `spake2` crate keeps its own copy of the password and its secret scalar inside its state
/// without wiping them when that state is dropped.
pub mod exchange {
    use hkdf::Hkdf;
    use hkdf::hmac::{Hmac, Mac};
    use sha2::Sha256;
    use spake2::{Ed25519Group, Identity, Password, Spake2};
    use zeroize::Zeroizing;

    /// The remote PC's identity in the exchange (side A).
    pub const ID_PC: &[u8] = b"freedawm-pc";
    /// The booth's (side B).
    pub const ID_BOOTH: &[u8] = b"freedawm-booth";
    /// HKDF's info for the confirmation key.
    pub const CONFIRM_INFO: &[u8] = b"freedawm pairing confirm";
    /// One side's SPAKE2 message: a side byte and a point.
    pub const MSG_LEN: usize = 33;
    /// One side's confirmation (HMAC-SHA256).
    pub const TAG_LEN: usize = 32;

    /// Which side of the exchange this is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Side {
        /// The PC showing the code.
        Booth,
        /// The PC the code is typed on.
        Pc,
    }

    impl Side {
        fn id(self) -> &'static [u8] {
            match self {
                Side::Booth => ID_BOOTH,
                Side::Pc => ID_PC,
            }
        }

        fn other(self) -> Side {
            match self {
                Side::Booth => Side::Pc,
                Side::Pc => Side::Booth,
            }
        }
    }

    /// The two PCs' link keys as this side sees them in its TLS session (public keys, raw).
    /// Both `None` on the plain link.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct LinkKeys {
        pub booth: Option<[u8; 32]>,
        pub pc: Option<[u8; 32]>,
    }

    impl LinkKeys {
        /// The plain link: no keys.
        pub const PLAIN: LinkKeys = LinkKeys { booth: None, pc: None };

        /// Both keys from hex; `None` unless both are keys.
        pub fn from_hex(booth: &str, pc: &str) -> Option<LinkKeys> {
            Some(LinkKeys { booth: Some(key_from_hex(booth)?), pc: Some(key_from_hex(pc)?) })
        }
    }

    fn key_from_hex(s: &str) -> Option<[u8; 32]> {
        let s = s.trim();
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(out)
    }

    /// A message from the other side that is not one (wrong length, wrong side, not a point).
    /// Says nothing more, so nothing secret can reach an error or a log through it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Damaged;

    impl std::fmt::Display for Damaged {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the pairing message is damaged")
        }
    }

    /// One side's exchange, after its message went out.
    pub struct Exchange {
        side: Side,
        state: Spake2<Ed25519Group>,
        keys: LinkKeys,
    }

    impl Exchange {
        /// Starts this side's exchange with `code` (spaces around it ignored): the exchange and the
        /// message to send.
        pub fn start(side: Side, code: &str, keys: LinkKeys) -> (Exchange, Vec<u8>) {
            let password = Password::new(code.trim().as_bytes());
            let (a, b) = (Identity::new(ID_PC), Identity::new(ID_BOOTH));
            let (state, msg) = match side {
                Side::Pc => Spake2::<Ed25519Group>::start_a(&password, &a, &b),
                Side::Booth => Spake2::<Ed25519Group>::start_b(&password, &a, &b),
            };
            (Exchange { side, state, keys }, msg)
        }

        /// The other side's message: the confirmation key, or [`Damaged`].
        pub fn finish(self, theirs: &[u8]) -> Result<Confirmed, Damaged> {
            if theirs.len() != MSG_LEN {
                return Err(Damaged);
            }
            let k = Zeroizing::new(self.state.finish(theirs).map_err(|_| Damaged)?);
            let mut key = Zeroizing::new([0u8; 32]);
            Hkdf::<Sha256>::new(None, &k).expand(CONFIRM_INFO, key.as_mut()).map_err(|_| Damaged)?;
            Ok(Confirmed { side: self.side, key, keys: self.keys })
        }
    }

    /// Both messages in: this side's confirmation to send, and the check of the other's.
    pub struct Confirmed {
        side: Side,
        key: Zeroizing<[u8; 32]>,
        keys: LinkKeys,
    }

    impl Confirmed {
        fn mac(&self, side: Side) -> Option<Hmac<Sha256>> {
            let mut m = <Hmac<Sha256> as Mac>::new_from_slice(self.key.as_ref()).ok()?;
            let id = side.id();
            m.update(&[u8::try_from(id.len()).ok()?]);
            m.update(id);
            for k in [self.keys.booth, self.keys.pc] {
                match k {
                    Some(k) => {
                        m.update(&[32]);
                        m.update(&k);
                    }
                    None => m.update(&[0]),
                }
            }
            Some(m)
        }

        /// This side's confirmation.
        pub fn tag(&self) -> Vec<u8> {
            self.mac(self.side).map(|m| m.finalize().into_bytes().to_vec()).unwrap_or_default()
        }

        /// Whether `tag` is the other side's confirmation over the same key and the same two link
        /// keys (compared in constant time).
        pub fn check(&self, tag: &[u8]) -> bool {
            tag.len() == TAG_LEN && self.mac(self.side.other()).is_some_and(|m| m.verify_slice(tag).is_ok())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn keys(b: u8, p: u8) -> LinkKeys {
            LinkKeys { booth: Some([b; 32]), pc: Some([p; 32]) }
        }

        /// Runs both sides with their codes and keys: (booth accepts the PC, PC accepts the booth).
        fn run(booth_code: &str, booth_keys: LinkKeys, pc_code: &str, pc_keys: LinkKeys) -> (bool, bool) {
            let (pc, msg_a) = Exchange::start(Side::Pc, pc_code, pc_keys);
            let (booth, msg_b) = Exchange::start(Side::Booth, booth_code, booth_keys);
            let booth = booth.finish(&msg_a).unwrap();
            let pc = pc.finish(&msg_b).unwrap();
            (booth.check(&pc.tag()), pc.check(&booth.tag()))
        }

        #[test]
        fn the_right_code_confirms_both_ways() {
            assert_eq!(run("4821", keys(1, 2), " 4821 ", keys(1, 2)), (true, true));
            assert_eq!(run("0007", LinkKeys::PLAIN, "0007", LinkKeys::PLAIN), (true, true), "the plain link: no keys");
        }

        #[test]
        fn a_wrong_code_confirms_neither_way() {
            assert_eq!(run("4821", keys(1, 2), "4820", keys(1, 2)), (false, false));
        }

        #[test]
        fn a_machine_in_the_middle_fails_the_confirmation_even_with_the_code() {
            // It relays both messages unchanged (so both derive the same k), but each side sees
            // its key in place of the other's.
            assert_eq!(run("4821", keys(1, 9), "4821", keys(8, 2)), (false, false));
            // One key differing is enough, and a plain-link transcript is not an encrypted one.
            assert_eq!(run("4821", keys(1, 2), "4821", keys(1, 3)), (false, false));
            assert_eq!(run("4821", keys(1, 2), "4821", LinkKeys::PLAIN), (false, false));
        }

        #[test]
        fn a_confirmation_is_not_mistaken_for_the_other_sides() {
            let (pc, msg_a) = Exchange::start(Side::Pc, "1234", keys(1, 2));
            let (booth, msg_b) = Exchange::start(Side::Booth, "1234", keys(1, 2));
            let (booth, pc) = (booth.finish(&msg_a).unwrap(), pc.finish(&msg_b).unwrap());
            assert!(!booth.check(&booth.tag()), "the booth's own confirmation reflected back");
            assert!(!pc.check(&pc.tag()));
            assert!(!booth.check(&[]) && !booth.check(&[0u8; TAG_LEN]) && !booth.check(&[0u8; 64]));
        }

        #[test]
        fn a_recorded_exchange_replayed_fails() {
            let (pc, msg_a) = Exchange::start(Side::Pc, "1234", keys(1, 2));
            let (booth, msg_b) = Exchange::start(Side::Booth, "1234", keys(1, 2));
            let (_, pc) = (booth.finish(&msg_a).unwrap(), pc.finish(&msg_b).unwrap());
            let recorded = pc.tag();
            // The same PC message and confirmation sent to a new exchange with the same code.
            let (again, _) = Exchange::start(Side::Booth, "1234", keys(1, 2));
            let again = again.finish(&msg_a).unwrap();
            assert!(!again.check(&recorded));
        }

        #[test]
        fn damaged_messages_are_refused_without_a_panic() {
            let (_, msg_a) = Exchange::start(Side::Pc, "1234", LinkKeys::PLAIN);
            assert_eq!(msg_a.len(), MSG_LEN);
            for bad in [vec![], vec![0x41], msg_a[..32].to_vec(), [msg_a.as_slice(), &[0]].concat(), vec![0xff; 1000]] {
                let (booth, _) = Exchange::start(Side::Booth, "1234", LinkKeys::PLAIN);
                assert!(booth.finish(&bad).is_err(), "{} bytes", bad.len());
            }
            // The booth's own message reflected back (the wrong side).
            let (booth, msg_b) = Exchange::start(Side::Booth, "1234", LinkKeys::PLAIN);
            assert!(booth.finish(&msg_b).is_err());
            // Any 33 bytes: refused or carried on, never a panic.
            let mut x = 0x9e37_79b9_u32;
            for _ in 0..200 {
                let bytes: Vec<u8> = (0..MSG_LEN)
                    .map(|i| {
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        if i == 0 { 0x41 } else { x as u8 }
                    })
                    .collect();
                let (booth, _) = Exchange::start(Side::Booth, "1234", LinkKeys::PLAIN);
                if let Ok(c) = booth.finish(&bytes) {
                    assert_eq!(c.tag().len(), TAG_LEN);
                }
            }
        }

        #[test]
        fn link_keys_from_hex() {
            let h = "ab".repeat(32);
            assert_eq!(LinkKeys::from_hex(&h, &h), Some(LinkKeys { booth: Some([0xab; 32]), pc: Some([0xab; 32]) }));
            assert_eq!(LinkKeys::from_hex(&h, "zz"), None);
            assert_eq!(LinkKeys::from_hex("é".repeat(32).as_str(), &h), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter() -> impl FnMut() -> u64 {
        let mut n = 4820u64;
        move || {
            n += 1;
            n
        }
    }

    #[test]
    fn the_right_code_pairs_once() {
        let mut p = Pairing::default();
        let r = p.request("Phone", "browser", 0.0, counter()).unwrap();
        assert_eq!(r.code.len(), 4);
        assert_eq!(p.pending(1.0).len(), 1);
        assert_eq!(p.confirm(&r.id, &r.code, 5.0), Confirm::Paired { name: "Phone".into(), kind: "browser".into() });
        assert_eq!(p.confirm(&r.id, &r.code, 6.0), Confirm::Unknown, "a code works once");
        assert!(p.pending(6.0).is_empty());
    }

    #[test]
    fn three_wrong_codes_end_the_request() {
        let mut p = Pairing::default();
        let r = p.request("Guest", "browser", 0.0, counter()).unwrap();
        let wrong = if r.code == "0000" { "1111" } else { "0000" };
        assert_eq!(p.confirm(&r.id, wrong, 1.0), Confirm::Wrong { left: 2 });
        assert_eq!(p.confirm(&r.id, wrong, 1.0), Confirm::Wrong { left: 1 });
        assert_eq!(p.confirm(&r.id, wrong, 1.0), Confirm::Wrong { left: 0 });
        assert_eq!(p.confirm(&r.id, &r.code, 1.0), Confirm::Unknown, "even the right code is too late");
    }

    #[test]
    fn requests_expire_and_are_rate_limited() {
        let mut p = Pairing::default();
        let r = p.request("Phone", "browser", 0.0, counter()).unwrap();
        assert_eq!(p.confirm(&r.id, &r.code, EXPIRES_S + 1.0), Confirm::Unknown);
        let mut rng = counter();
        let made = (0..50).filter(|_| p.request("x", "pc", 200.0, &mut rng).is_some()).count();
        assert_eq!(made, MAX_RECENT - 1, "one was made before");
        assert!(p.pending(200.0).len() <= MAX_PENDING);
        assert!(p.request("x", "pc", 200.0 + WINDOW_S + 1.0, &mut rng).is_some(), "the window passes");
    }

    #[test]
    fn names_are_cleaned() {
        let mut p = Pairing::default();
        let r = p.request("  \u{7}Ph\none  ", "weird", 0.0, counter()).unwrap();
        assert_eq!(r.name, "Phone");
        assert_eq!(r.kind, "browser");
    }

    #[test]
    fn each_exchange_spends_a_try_and_the_right_one_pairs() {
        let mut p = Pairing::default();
        let r = p.request("Piano PC", "pc", 0.0, counter()).unwrap();
        assert_eq!(p.begin_exchange(&r.id, 1.0).as_deref(), Some(&r.code));
        assert_eq!(p.finish_exchange(&r.id, false, 1.0), Confirm::Wrong { left: 2 });
        // An exchange started and abandoned (the connection dropped) spent its try too.
        assert!(p.begin_exchange(&r.id, 2.0).is_some());
        assert_eq!(p.pending(2.0).len(), 1, "still on screen with a try left");
        assert!(p.begin_exchange(&r.id, 3.0).is_some(), "the last try");
        assert!(p.pending(3.0).is_empty(), "no longer offered while its last try runs");
        assert!(p.begin_exchange(&r.id, 3.0).is_none(), "no fourth");
        assert_eq!(p.finish_exchange(&r.id, true, 4.0), Confirm::Paired { name: "Piano PC".into(), kind: "pc".into() });
        assert_eq!(p.finish_exchange(&r.id, true, 4.0), Confirm::Unknown, "once");
    }

    #[test]
    fn three_failed_exchanges_end_the_request() {
        let mut p = Pairing::default();
        let r = p.request("Piano PC", "pc", 0.0, counter()).unwrap();
        for left in [2, 1, 0] {
            assert!(p.begin_exchange(&r.id, 1.0).is_some());
            assert_eq!(p.finish_exchange(&r.id, false, 1.0), Confirm::Wrong { left });
        }
        assert!(p.begin_exchange(&r.id, 1.0).is_none());
        assert_eq!(p.finish_exchange(&r.id, true, 1.0), Confirm::Unknown, "gone");
    }

    #[test]
    fn a_pc_request_never_takes_a_typed_code_nor_a_browser_one_an_exchange() {
        let mut p = Pairing::default();
        let mut rng = counter();
        let pc = p.request("Piano PC", "pc", 0.0, &mut rng).unwrap();
        assert_eq!(p.confirm(&pc.id, &pc.code, 1.0), Confirm::Unknown, "a PC's code is never sent");
        let phone = p.request("Phone", "browser", 0.0, &mut rng).unwrap();
        assert_ne!(phone.id, pc.id);
        assert!(p.begin_exchange(&phone.id, 1.0).is_none());
        assert_eq!(p.finish_exchange(&phone.id, true, 1.0), Confirm::Unknown);
        assert_eq!(p.pending(1.0).len(), 2, "neither spent a try");
        assert!(p.pending(1.0).iter().all(|r| r.wrong == 0));
    }

    #[test]
    fn a_request_printed_for_a_log_leaves_its_code_out() {
        let mut p = Pairing::default();
        let r = p.request("Piano PC", "pc", 0.0, || 4821).unwrap();
        assert_eq!(r.code, "4821");
        assert!(!format!("{r:?}").contains("4821"), "{r:?}");
    }
}
