//! The window's words: plain ones, for someone who has never used a DAW. Names of takes and times
//! as people say them, the devices described as what they are ("Microphone + Piano keyboard"),
//! and the studio's problems said simply. Nothing here touches a device or the network.

use crate::files::TakeFiles;
use crate::settings::{Stamp, days_between, local_stamp, parse_stem, weekday};

pub const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `m:ss` or `h:mm:ss`.
pub fn clock(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// `2:03 pm`, `12:00 am`.
pub fn time_of_day(s: Stamp) -> String {
    let (h, m) = (s.3, s.4);
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{m:02} {}", if h < 12 { "am" } else { "pm" })
}

/// A take's name as the window shows it: a name the Recorder made (`Idea 2026-11-23 14-03-10`)
/// as `Idea · Monday 2:03 pm` (the file keeps its own, safe name); any other as it is.
pub fn display_name(stem: &str) -> String {
    match parse_stem(stem) {
        Some((prefix, at, more)) => format!("{prefix} · {} {}{more}", DAYS.get(weekday(at)).copied().unwrap_or(""), time_of_day(at)),
        None => stem.to_string(),
    }
}

/// When a take was made, in local time: from its name when the Recorder named it, else from its
/// file's time.
pub fn take_time(t: &TakeFiles) -> Option<(Stamp, bool)> {
    if let Some((_, at, _)) = parse_stem(&t.stem) {
        return Some((at, true));
    }
    let secs = t.modified?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some((local_stamp(i64::try_from(secs).ok()?), false))
}

/// `Today`, `Yesterday`, a weekday within the week, else `Nov 16` (`Nov 16, 2025` another year).
pub fn day_word(now: Stamp, then: Stamp) -> String {
    match days_between(then, now) {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => DAYS.get(weekday(then)).copied().unwrap_or("").to_string(),
        _ => {
            let month = MONTHS.get(usize::try_from(then.1).unwrap_or(1).saturating_sub(1)).copied().unwrap_or("");
            if then.0 == now.0 { format!("{month} {}", then.2) } else { format!("{month} {}, {}", then.2, then.0) }
        }
    }
}

/// What a take holds: `voice + piano notes`, `voice`, `piano notes`.
pub fn kinds(wav: bool, mid: bool) -> &'static str {
    match (wav, mid) {
        (true, true) => "voice + piano notes",
        (true, false) => "voice",
        (false, true) => "piano notes",
        (false, false) => "nothing",
    }
}

/// The line under a take's name in the list: `Today 2:03 pm · 0:42 · voice + piano notes`
/// (the time left out when the name already says it).
pub fn when_line(t: &TakeFiles, now: Stamp) -> String {
    let mut parts = Vec::new();
    if let Some((at, named)) = take_time(t) {
        let day = day_word(now, at);
        parts.push(if named { day } else { format!("{day} {}", time_of_day(at)) });
    }
    let layers = t.layer_count();
    if layers > 1 {
        parts.push(format!("{layers} layers"));
    }
    if let Some(s) = t.length() {
        parts.push(clock(s));
    }
    if layers <= 1 {
        parts.push(kinds(t.wav.is_some(), t.mid.is_some()).to_string());
    }
    parts.join(" · ")
}

/// Shortened to `max` characters, with `…`.
fn short(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// An audio input as a person names it: `Microphone` for anything like one (and the system's
/// default input), else the device's own name without its driver details in brackets.
pub fn describe_input(name: &str) -> String {
    let lower = name.to_lowercase();
    let generic = ["default", "pulse", "pipewire", "sysdefault", "default input", "primary sound capture driver"];
    if lower.contains("mic") || generic.contains(&lower.trim()) {
        return "Microphone".into();
    }
    let base = name.split(" (").next().unwrap_or(name);
    let base = if base.trim().is_empty() { name } else { base };
    short(base, 26)
}

/// The MIDI inputs as a person names them: one keyboard as `Piano keyboard` (or its name when
/// it does not sound like a keyboard), several as `2 keyboards`; `None` without any. The system's
/// own pass-through port (Linux's "Midi Through") is not a keyboard and is left out.
pub fn describe_keys(ports: &[String]) -> Option<String> {
    let real: Vec<&String> = ports.iter().filter(|p| !p.to_lowercase().contains("through")).collect();
    match real.as_slice() {
        [] => None,
        [one] => {
            let lower = one.to_lowercase();
            let keyboardish = ["key", "piano", "midi", "usb", "keystation", "launchkey", "oxygen", "mpk", "digital"];
            Some(if keyboardish.iter().any(|k| lower.contains(k)) {
                "Piano keyboard".into()
            } else {
                short(one.split(" (").next().unwrap_or(one), 22)
            })
        }
        many => Some(format!("{} keyboards", many.len())),
    }
}

/// `Microphone + Piano keyboard`, either alone, or `None` with neither.
pub fn recording_from(input: Option<String>, keys: Option<String>) -> Option<String> {
    match (input, keys) {
        (Some(a), Some(k)) => Some(format!("{a} + {k}")),
        (Some(a), None) => Some(a),
        (None, Some(k)) => Some(k),
        (None, None) => None,
    }
}

/// The caption under a take's shape: what it holds (`notes`: how many piano notes, when known).
pub fn saved_caption(wav: bool, mid: bool, notes: Option<usize>) -> String {
    let count = match notes {
        Some(1) => " (1 note)".to_string(),
        Some(n) => format!(" ({n} notes)"),
        None => String::new(),
    };
    match (wav, mid) {
        (true, true) => format!("Your voice, plus the piano notes you played{count}"),
        (true, false) => "Your voice".into(),
        (false, true) => format!("The piano notes you played{count}"),
        (false, false) => "Nothing came in: no sound and no piano notes".into(),
    }
}

/// Where takes are kept, as a person reads it: `Documents › FreeDAWm Ideas` (the folder and the
/// one it is in).
pub fn folder_words(dir: &std::path::Path) -> String {
    let names: Vec<String> = dir.components().rev().take(2).filter_map(|c| c.as_os_str().to_str().map(str::to_string)).collect();
    let names: Vec<String> = names.into_iter().rev().filter(|n| !n.is_empty() && n != "/" && n != "\\").collect();
    if names.is_empty() { dir.display().to_string() } else { names.join(" › ") }
}

/// The DAW's word for the other PC, in the Recorder's: "studio".
pub fn studio_words(s: &str) -> String {
    s.replace("booths", "studios").replace("Booths", "Studios").replace("booth", "studio").replace("Booth", "Studio")
}

/// A problem joining a studio, in plain words.
pub fn studio_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if says(&["wrong code", "wrong pin", "does not match", "didn't match"]) {
        "That didn't match. Check the code or PIN and type it again.".into()
    } else if says(&["too many"]) {
        "Too many wrong tries. Wait a minute, then try again.".into()
    } else if says(&["update freedawm", "newer exchange"]) {
        "The studio's FreeDAWm is too old to join. Update FreeDAWm on the studio's computer.".into()
    } else if says(&["not a freedawm invite"]) {
        "That isn't a PIN or an invite. An invite starts with freedawm-invite:".into()
    } else if says(&["invite is damaged", "address is damaged"]) {
        "Part of the invite is missing. Copy all of it and paste it again.".into()
    } else if says(&["invite has expired", "already used"]) {
        "That invite has run out. Ask the studio for a new one.".into()
    } else if says(&["request has expired"]) {
        "That took too long. Press Join and try again.".into()
    } else if says(&["cannot reach", "refused", "timed out", "did not answer", "connection", "unreachable", "no route"]) {
        "Can't reach the studio. Check that FreeDAWm is open there and on the same network.".into()
    } else if raw.trim().is_empty() {
        "Something went wrong. Try again.".into()
    } else {
        let s = studio_words(raw.trim());
        let mut c = s.chars();
        let first: String = c.next().map(|f| f.to_uppercase().collect()).unwrap_or_default();
        let mut out = format!("{first}{}", c.as_str());
        if !out.ends_with('.') {
            out.push('.');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_times_read_as_people_say_them() {
        // 2026-11-23 was a Monday.
        assert_eq!(display_name("Idea 2026-11-23 14-03-10"), "Idea · Monday 2:03 pm");
        assert_eq!(display_name("Idea 2026-11-22 21-41-00 (2)"), "Idea · Sunday 9:41 pm (2)");
        assert_eq!(display_name("Sunday Service 2026-11-22 00-05-00"), "Sunday Service · Sunday 12:05 am");
        assert_eq!(display_name("Chorus idea"), "Chorus idea");
        let now = (2026, 11, 23, 15, 0, 0);
        assert_eq!(day_word(now, (2026, 11, 23, 9, 0, 0)), "Today");
        assert_eq!(day_word(now, (2026, 11, 22, 23, 0, 0)), "Yesterday");
        assert_eq!(day_word(now, (2026, 11, 19, 9, 0, 0)), "Thursday");
        assert_eq!(day_word(now, (2026, 11, 2, 9, 0, 0)), "Nov 2");
        assert_eq!(day_word(now, (2025, 12, 31, 9, 0, 0)), "Dec 31, 2025");
        let mut t = TakeFiles::single("Idea 2026-11-22 21-41-00", Some("x.wav".into()), None, Some(75.0));
        assert_eq!(when_line(&t, now), "Yesterday · 1:15 · voice");
        t.layers.push(crate::files::LayerFiles { n: 2, wav: Some("x (layer 2).wav".into()), mid: None, keys: None, seconds: Some(75.0) });
        t.layers.push(crate::files::LayerFiles { n: 3, wav: None, mid: Some("x (layer 3).mid".into()), keys: None, seconds: None });
        assert_eq!(when_line(&t, now), "Yesterday · 3 layers · 1:15", "an idea with layers is one row");
    }

    #[test]
    fn devices_are_described_in_plain_words() {
        assert_eq!(describe_input("Microphone (Realtek(R) Audio)"), "Microphone");
        assert_eq!(describe_input("Headset Mic (Jabra Link 380)"), "Microphone");
        assert_eq!(describe_input("default"), "Microphone");
        assert_eq!(describe_input("Focusrite USB (Scarlett 2i2 USB)"), "Focusrite USB");
        assert_eq!(describe_keys(&["Keystation 49 MK3".into()]).as_deref(), Some("Piano keyboard"));
        assert_eq!(describe_keys(&["Midi Through Port-0".into()]), None);
        assert_eq!(describe_keys(&["Roland FP-30".into()]).as_deref(), Some("Roland FP-30"));
        assert_eq!(describe_keys(&["A".into(), "B".into()]).as_deref(), Some("2 keyboards"));
        assert_eq!(recording_from(Some("Microphone".into()), Some("Piano keyboard".into())).as_deref(), Some("Microphone + Piano keyboard"));
    }

    #[test]
    fn studio_problems_are_plain() {
        assert!(studio_error(soundcraft_net::follower::WRONG_OR_BETWEEN).starts_with("That didn't match"));
        assert!(studio_error("cannot reach the booth at 127.0.0.1:1: connection refused").starts_with("Can't reach the studio"));
        assert!(studio_error("that is not a FreeDAWm invite (it starts with freedawm-invite:)").contains("isn't a PIN or an invite"));
        assert_eq!(studio_words("Take sent to the booth"), "Take sent to the studio");
    }
}
