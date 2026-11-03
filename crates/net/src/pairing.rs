//! Pairing: the first time a browser or PC connects, it asks to pair; this PC shows a 4-digit
//! code on its screen; whoever types that code on the device gets a token, remembered on both
//! sides. A guest on the Wi-Fi can ask, but without the code (which only the booth's screen
//! shows) gets nothing: a request takes 3 wrong codes, then it is gone; requests expire after 2
//! minutes; at most 4 wait at once and at most 20 are made in 10 minutes.

/// One request waiting for its code.
#[derive(Debug, Clone, PartialEq)]
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
    /// The requests waiting, oldest first (what the screen shows).
    pub fn pending(&self, now: f64) -> Vec<PendingPair> {
        self.pending.iter().filter(|p| now - p.made < EXPIRES_S).cloned().collect()
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

    /// The device typed `code` for request `id`.
    pub fn confirm(&mut self, id: &str, code: &str, now: f64) -> Confirm {
        self.expire(now);
        let Some(i) = self.pending.iter().position(|p| p.id == id) else { return Confirm::Unknown };
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

    /// Turns a request down (Deny on this PC's screen).
    pub fn deny(&mut self, id: &str) {
        self.pending.retain(|p| p.id != id);
    }

    fn expire(&mut self, now: f64) {
        self.pending.retain(|p| now - p.made < EXPIRES_S);
    }
}

/// A device's name as it asked to pair: no control characters, at most 40, "Device" if empty.
pub fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    if name.trim().is_empty() { "Device".to_string() } else { name.trim().to_string() }
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
}
