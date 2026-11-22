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
//!
//! A booth PIN (Setup › Network, off by default) stands in for the random code: a request made
//! while one is set waits for that PIN instead of a code on the screen, so nobody at the booth
//! has to read anything out. It is the same password of the same exchange for PCs (never sent)
//! and what the phone's page types. Because it does not change from one request to the next,
//! wrong PINs are also counted across requests ([`PinGuard`]): 5 in a row pause PIN pairing (5
//! minutes, doubling each time up to an hour) and 20 since the last success turn it off until
//! the booth's owner turns it back on. While it is paused or off a request gets a random code
//! shown on the booth's screen, as without a PIN.

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
    /// The 4 digits shown on this PC's screen (empty for a request waiting for the booth PIN).
    pub code: String,
    /// Seconds (the caller's clock) it was made at.
    pub made: f64,
    pub wrong: u8,
    /// Waits for the booth PIN instead of a code shown on the screen.
    pub pin: bool,
    /// May pair: a PIN request when the booth asks before each device joins, once Allow is
    /// pressed; every other request at once.
    pub allowed: bool,
    /// Where it came from, for the booth's notices: the name it gave, and its address when that
    /// is not this PC's own (a bridge).
    pub from: String,
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
            .field("pin", &self.pin)
            .field("allowed", &self.allowed)
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

pub const EXPIRES_S: f64 = 120.0;
pub const MAX_WRONG: u8 = 3;
pub const MAX_PENDING: usize = 4;
/// Requests allowed per [`WINDOW_S`].
pub const MAX_RECENT: usize = 20;
pub const WINDOW_S: f64 = 600.0;

/// The shortest booth PIN taken (characters).
pub const PIN_MIN_LEN: usize = 6;
/// The longest.
pub const PIN_MAX_LEN: usize = 64;
/// Wrong PINs in a row that pause PIN pairing.
pub const PIN_STREAK: u32 = 5;
/// The first pause (seconds); each next one is twice as long, up to [`PIN_PAUSE_MAX_S`].
pub const PIN_PAUSE_S: f64 = 300.0;
pub const PIN_PAUSE_MAX_S: f64 = 3600.0;
/// Wrong PINs since the last PIN pairing that turn PIN pairing off.
pub const PIN_OFF_AFTER: u32 = 20;

/// Whether `pin` can be the booth PIN (spaces around it are dropped): the PIN to keep, or why
/// not in plain words.
pub fn check_pin(pin: &str) -> Result<String, String> {
    let pin = pin.trim();
    let n = pin.chars().count();
    if n < PIN_MIN_LEN {
        return Err(format!("The booth PIN needs at least {PIN_MIN_LEN} characters (digits, letters or anything else)."));
    }
    if n > PIN_MAX_LEN {
        return Err(format!("The booth PIN can be at most {PIN_MAX_LEN} characters."));
    }
    if pin.chars().any(char::is_control) {
        return Err("The booth PIN cannot hold control characters.".into());
    }
    Ok(pin.to_string())
}

/// A random 6-digit PIN ("Suggest one"); `random` gives fresh random numbers.
pub fn suggest_pin(mut random: impl FnMut() -> u64) -> String {
    format!("{:06}", random() % 1_000_000)
}

/// Whether `typed` is `pin` (both trimmed), without the time taken saying how much matched:
/// both are hashed and the hashes compared in full.
pub fn same_pin(typed: &str, pin: &str) -> bool {
    use sha2::{Digest, Sha256};
    let (a, b) = (Sha256::digest(typed.trim().as_bytes()), Sha256::digest(pin.trim().as_bytes()));
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The booth PIN as [`Pairing`] holds it: wiped when dropped, never printed.
#[derive(Default)]
struct Secret(Option<Zeroizing<String>>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Secret(set)" } else { "Secret(none)" })
    }
}

/// Wrong booth PINs counted across requests (a request's own 3 tries are not enough for a
/// secret that stays the same).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PinGuard {
    /// Wrong in a row (since the last pause or success).
    pub streak: u32,
    /// Wrong since the last PIN pairing (or the owner's reset).
    pub wrong: u32,
    /// Pauses so far (each next one is twice as long).
    pub pauses: u32,
    /// Paused until (seconds, the caller's clock).
    pub paused_until: f64,
    /// Where the last wrong PIN came from.
    pub last_from: String,
}

/// What a booth PIN try changed, for the booth's notices.
#[derive(Debug, Clone, PartialEq)]
pub enum PinEvent {
    /// A device paired with the booth PIN.
    Paired { name: String, kind: String },
    /// [`PIN_STREAK`] wrong in a row: paused for `secs`.
    Paused { from: String, secs: f64 },
    /// [`PIN_OFF_AFTER`] wrong since the last success: off until the owner turns it back on.
    TurnedOff { from: String },
}

impl PinGuard {
    /// Seconds of pause left, while paused.
    pub fn paused(&self, now: f64) -> Option<f64> {
        (now < self.paused_until).then_some(self.paused_until - now)
    }

    /// Turned off by wrong PINs.
    pub fn is_off(&self) -> bool {
        self.wrong >= PIN_OFF_AFTER
    }

    /// PIN pairing may be tried now.
    pub fn open(&self, now: f64) -> bool {
        !self.is_off() && self.paused(now).is_none()
    }

    /// How long the next pause lasts.
    pub fn next_pause(&self) -> f64 {
        let doubled = 2f64.powi(i32::try_from(self.pauses.min(16)).unwrap_or(16));
        (PIN_PAUSE_S * doubled).min(PIN_PAUSE_MAX_S)
    }

    /// A wrong PIN from `from`.
    pub fn wrong(&mut self, now: f64, from: &str) -> Option<PinEvent> {
        self.streak = self.streak.saturating_add(1);
        self.wrong = self.wrong.saturating_add(1);
        self.last_from = from.to_string();
        if self.is_off() {
            return Some(PinEvent::TurnedOff { from: from.to_string() });
        }
        if self.streak >= PIN_STREAK {
            let secs = self.next_pause();
            self.paused_until = now + secs;
            self.pauses = self.pauses.saturating_add(1);
            self.streak = 0;
            return Some(PinEvent::Paused { from: from.to_string(), secs });
        }
        None
    }

    /// A correct PIN pairing, or the owner's reset: everything starts again.
    pub fn reset(&mut self) {
        *self = PinGuard::default();
    }
}

#[derive(Debug, Default)]
pub struct Pairing {
    pending: Vec<PendingPair>,
    /// When recent requests were made (for the rate limit).
    recent: Vec<f64>,
    /// The booth PIN, when one is set and PIN pairing is on.
    pin: Secret,
    /// Ask before each device joins with the PIN.
    ask: bool,
    guard: PinGuard,
    events: Vec<PinEvent>,
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
    pub fn request(&mut self, name: &str, kind: &str, now: f64, random: impl FnMut() -> u64) -> Option<PendingPair> {
        self.request_from(name, kind, "", now, random)
    }

    /// [`Pairing::request`], noting where it came from (`from`: an address, or empty). It waits
    /// for the booth PIN when one is set and PIN pairing is open (not paused or turned off);
    /// otherwise for a random code shown on the screen.
    pub fn request_from(&mut self, name: &str, kind: &str, from: &str, now: f64, mut random: impl FnMut() -> u64) -> Option<PendingPair> {
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
        let pin = self.pin_open(now);
        let id = format!("{:016x}", random());
        let code = if pin { String::new() } else { format!("{:04}", random() % 10_000) };
        let from = clean_text(&if from.is_empty() { name.clone() } else { format!("{name} ({from})") }, 80);
        let allowed = !(pin && self.ask);
        let p = PendingPair { id, name, kind: kind.into(), code, made: now, wrong: 0, pin, allowed, from };
        self.pending.push(p.clone());
        Some(p)
    }

    /// Sets the booth PIN (`None`: no PIN, or PIN pairing turned off) and whether to ask before
    /// each device joins with it. Requests already made keep what they wait for.
    pub fn set_pin(&mut self, pin: Option<&str>, ask: bool) {
        let same = match (&self.pin.0, pin) {
            (Some(a), Some(b)) => a.as_str() == b,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.pin = Secret(pin.map(|p| Zeroizing::new(p.to_string())));
        }
        self.ask = ask;
    }

    /// PIN pairing is set and open now.
    pub fn pin_open(&self, now: f64) -> bool {
        self.pin.0.is_some() && self.guard.open(now)
    }

    /// The wrong-PIN counters.
    pub fn pin_guard(&self) -> &PinGuard {
        &self.guard
    }

    /// The owner's reset, or the count carried over from the saved settings.
    pub fn set_pin_guard(&mut self, guard: PinGuard) {
        self.guard = guard;
    }

    /// What booth PIN tries changed since last asked (oldest first).
    pub fn take_pin_events(&mut self) -> Vec<PinEvent> {
        std::mem::take(&mut self.events)
    }

    /// Lets a PIN request pair (Allow on this PC's screen, when it asks before each device
    /// joins).
    pub fn allow(&mut self, id: &str) {
        for p in self.pending.iter_mut().filter(|p| p.id == id) {
            p.allowed = true;
        }
    }

    /// The request `id` waits for Allow on this PC's screen (its PIN is not tried until then).
    pub fn waiting_for_allow(&self, id: &str) -> bool {
        self.pending.iter().any(|p| p.id == id && !p.allowed)
    }

    /// The request `id` waits for the booth PIN.
    pub fn is_pin_request(&self, id: &str) -> bool {
        self.pending.iter().any(|p| p.id == id && p.pin)
    }

    /// A wrong PIN from `from`: counted across requests too; a pause or the PIN turning itself
    /// off ends every PIN request waiting.
    fn pin_wrong(&mut self, from: &str, now: f64) {
        if let Some(e) = self.guard.wrong(now, from) {
            self.pending.retain(|p| !p.pin);
            self.events.push(e);
        }
    }

    /// A PIN request paired.
    fn pin_paired(&mut self, name: &str, kind: &str) {
        self.guard.reset();
        self.events.push(PinEvent::Paired { name: name.to_string(), kind: kind.to_string() });
    }

    /// What the request at `i` must be given: its code, or the booth PIN while PIN pairing is
    /// open. `None`: a PIN request while PIN pairing is not open, or before Allow.
    fn secret_for(&self, i: usize, now: f64) -> Option<Zeroizing<String>> {
        let p = self.pending.get(i)?;
        if !p.pin {
            return Some(Zeroizing::new(p.code.clone()));
        }
        if !p.allowed || !self.pin_open(now) {
            return None;
        }
        self.pin.0.as_ref().map(|s| Zeroizing::new(s.as_str().to_string()))
    }

    /// The browser typed `code` for its request `id` (a browser's only: a PC's code is never
    /// sent, see [`Pairing::begin_exchange`]).
    pub fn confirm(&mut self, id: &str, code: &str, now: f64) -> Confirm {
        self.expire(now);
        let Some(i) = self.pending.iter().position(|p| p.id == id && p.kind == "browser") else { return Confirm::Unknown };
        // A PIN request before Allow, or while PIN pairing is paused or off: nothing is tried.
        let Some(secret) = self.secret_for(i, now) else { return Confirm::Unknown };
        let pin = self.pending.get(i).is_some_and(|p| p.pin);
        let right = if pin { same_pin(code, &secret) } else { secret.as_str() == code.trim() };
        drop(secret);
        if right {
            let p = self.pending.remove(i);
            if pin {
                self.pin_paired(&p.name, &p.kind);
            }
            return Confirm::Paired { name: p.name, kind: p.kind };
        }
        self.spend(i, id, pin, now)
    }

    /// A wrong try on the request at `i` (`id`): its own count, and the PIN's across requests.
    fn spend(&mut self, i: usize, id: &str, pin: bool, now: f64) -> Confirm {
        let Some(p) = self.pending.get_mut(i) else { return Confirm::Unknown };
        p.wrong = p.wrong.saturating_add(1);
        let left = MAX_WRONG.saturating_sub(p.wrong);
        let from = p.from.clone();
        if left == 0 {
            self.pending.remove(i);
        }
        if pin {
            self.pin_wrong(&from, now);
        }
        // A pause ends the PIN requests waiting: none left to try.
        Confirm::Wrong { left: if self.pending.iter().any(|p| p.id == id) { left } else { 0 } }
    }

    /// A PC starts the key exchange for its request `id`: one of the request's tries is spent
    /// now (whoever runs the exchange learns whether its code was right), and the code comes
    /// back to run it with. `None`: no such PC request, or its tries are spent.
    pub fn begin_exchange(&mut self, id: &str, now: f64) -> Option<Zeroizing<String>> {
        self.expire(now);
        let i = self.pending.iter().position(|p| p.id == id && p.kind == "pc")?;
        if self.pending.get(i).is_none_or(|p| p.wrong >= MAX_WRONG) {
            return None;
        }
        // A PIN request before Allow, or while PIN pairing is paused or off: nothing is tried.
        let secret = self.secret_for(i, now)?;
        let p = self.pending.get_mut(i)?;
        p.wrong = p.wrong.saturating_add(1);
        if p.pin {
            // The PIN's count across requests is spent now too (the PC learns from the booth's
            // answer whether its guess was right, and may never finish); a success resets it.
            // The exchange under way stays, so a right PIN still pairs.
            let from = p.from.clone();
            if let Some(e) = self.guard.wrong(now, &from) {
                self.pending.retain(|p| !p.pin || p.id == id);
                self.events.push(e);
            }
        }
        Some(secret)
    }

    /// How the exchange begun for `id` ended: `ok` (both sides' confirmations matched) pairs;
    /// otherwise its try stays spent, and after the last one the request is gone.
    pub fn finish_exchange(&mut self, id: &str, ok: bool, now: f64) -> Confirm {
        self.expire(now);
        let Some(i) = self.pending.iter().position(|p| p.id == id && p.kind == "pc") else { return Confirm::Unknown };
        if ok {
            let p = self.pending.remove(i);
            if p.pin {
                self.pin_paired(&p.name, &p.kind);
            }
            return Confirm::Paired { name: p.name, kind: p.kind };
        }
        let left = self.pending.get(i).map_or(0, |p| MAX_WRONG.saturating_sub(p.wrong));
        // A PIN request whose PIN pairing was paused meanwhile: no more tries.
        let paused = self.pending.get(i).is_some_and(|p| p.pin) && !self.pin_open(now);
        let left = if paused { 0 } else { left };
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

    fn pin_pairing() -> Pairing {
        let mut p = Pairing::default();
        p.set_pin(Some("482113"), false);
        p
    }

    #[test]
    fn with_a_booth_pin_a_request_waits_for_it_and_shows_no_code() {
        let mut p = pin_pairing();
        let r = p.request_from("Phone", "browser", "192.168.1.9", 0.0, counter()).unwrap();
        assert!(r.pin && r.allowed && r.code.is_empty());
        assert_eq!(r.from, "Phone (192.168.1.9)");
        assert_eq!(p.confirm(&r.id, " 482113 ", 1.0), Confirm::Paired { name: "Phone".into(), kind: "browser".into() });
        assert_eq!(p.take_pin_events(), vec![PinEvent::Paired { name: "Phone".into(), kind: "browser".into() }]);
        // Without a PIN: a code, as before.
        p.set_pin(None, false);
        let r = p.request("Phone", "browser", 2.0, counter()).unwrap();
        assert!(!r.pin && r.code.len() == 4);
    }

    #[test]
    fn wrong_pins_count_across_requests_pause_double_and_turn_off() {
        let mut p = pin_pairing();
        let mut rng = counter();
        let mut now = 0.0;
        let mut wrong = |p: &mut Pairing, now: f64| {
            let r = p.request("Guest", "browser", now, &mut rng).unwrap();
            assert!(r.pin, "PIN pairing open at {now}");
            assert!(matches!(p.confirm(&r.id, "000000", now), Confirm::Wrong { .. }));
        };
        for _ in 0..4 {
            wrong(&mut p, now);
        }
        assert!(p.pin_open(now) && p.take_pin_events().is_empty());
        wrong(&mut p, now);
        assert_eq!(p.take_pin_events(), vec![PinEvent::Paused { from: "Guest".into(), secs: 300.0 }]);
        assert!(!p.pin_open(now + 299.0));
        // While paused a request gets a code shown on the screen, as without a PIN.
        let r = p.request("Friend", "browser", now + 10.0, counter()).unwrap();
        assert!(!r.pin && r.code.len() == 4);
        p.deny(&r.id);
        now += 301.0;
        assert!(p.pin_open(now));
        for _ in 0..5 {
            wrong(&mut p, now);
        }
        assert_eq!(p.take_pin_events(), vec![PinEvent::Paused { from: "Guest".into(), secs: 600.0 }], "doubled");
        now += 601.0;
        for _ in 0..5 {
            wrong(&mut p, now);
        }
        assert!(matches!(p.take_pin_events().as_slice(), [PinEvent::Paused { secs, .. }] if *secs == 1200.0));
        now += 1201.0;
        for _ in 0..5 {
            wrong(&mut p, now);
        }
        assert_eq!(p.take_pin_events(), vec![PinEvent::TurnedOff { from: "Guest".into() }], "20 since the last success");
        assert!(!p.pin_open(now + 1e6), "off until the owner turns it back on");
        p.set_pin_guard(PinGuard::default());
        assert!(p.pin_open(now));
    }

    #[test]
    fn the_pause_caps_at_an_hour_and_a_right_pin_resets_the_count() {
        let g = PinGuard { pauses: 9, ..PinGuard::default() };
        assert_eq!(g.next_pause(), PIN_PAUSE_MAX_S);
        let mut p = pin_pairing();
        let mut rng = counter();
        for _ in 0..4 {
            let r = p.request("Guest", "browser", 0.0, &mut rng).unwrap();
            let _ = p.confirm(&r.id, "nope-nope", 0.0);
        }
        assert_eq!(p.pin_guard().streak, 4);
        let r = p.request("Phone", "browser", 0.0, &mut rng).unwrap();
        assert!(matches!(p.confirm(&r.id, "482113", 0.0), Confirm::Paired { .. }));
        assert_eq!(*p.pin_guard(), PinGuard::default());
    }

    #[test]
    fn a_pc_pin_exchange_counts_when_it_begins_and_a_right_one_still_pairs() {
        let mut p = pin_pairing();
        let mut rng = counter();
        for _ in 0..4 {
            let r = p.request("Guest PC", "pc", 0.0, &mut rng).unwrap();
            assert_eq!(p.begin_exchange(&r.id, 0.0).as_deref().map(String::as_str), Some("482113"));
            // Abandoned without finishing: still counted.
        }
        let r = p.request("Piano PC", "pc", 0.0, &mut rng).unwrap();
        assert!(p.begin_exchange(&r.id, 0.0).is_some());
        assert!(matches!(p.take_pin_events().as_slice(), [PinEvent::Paused { .. }]), "the fifth begun");
        // Its own exchange stays: the right PIN pairs and ends the pause.
        assert!(matches!(p.finish_exchange(&r.id, true, 1.0), Confirm::Paired { .. }));
        assert!(p.pin_open(1.0));
    }

    #[test]
    fn asking_first_tries_nothing_before_allow() {
        let mut p = Pairing::default();
        p.set_pin(Some("482113"), true);
        let r = p.request("Phone", "browser", 0.0, counter()).unwrap();
        assert!(r.pin && !r.allowed && p.waiting_for_allow(&r.id));
        assert_eq!(p.confirm(&r.id, "482113", 1.0), Confirm::Unknown, "not tried before Allow");
        assert_eq!(p.pending(1.0)[0].wrong, 0);
        p.allow(&r.id);
        assert!(matches!(p.confirm(&r.id, "482113", 2.0), Confirm::Paired { .. }));
    }

    #[test]
    fn pins_shorter_than_six_are_refused_and_suggestions_are_six_digits() {
        assert!(check_pin("12345").unwrap_err().contains("at least 6"));
        assert!(check_pin("  12345 ").is_err());
        assert_eq!(check_pin(" sunday ").unwrap(), "sunday");
        assert!(check_pin("a\u{7}bcdef").is_err());
        assert!(check_pin(&"x".repeat(65)).is_err());
        assert_eq!(suggest_pin(|| 42), "000042");
        assert_eq!(suggest_pin(|| 123_456_789), "456789");
        assert!(same_pin(" 482113", "482113") && !same_pin("482112", "482113") && !same_pin("", "482113"));
        assert!(!format!("{:?}", pin_pairing()).contains("482113"));
    }

    #[test]
    fn a_request_printed_for_a_log_leaves_its_code_out() {
        let mut p = Pairing::default();
        let r = p.request("Piano PC", "pc", 0.0, || 4821).unwrap();
        assert_eq!(r.code, "4821");
        assert!(!format!("{r:?}").contains("4821"), "{r:?}");
    }
}
