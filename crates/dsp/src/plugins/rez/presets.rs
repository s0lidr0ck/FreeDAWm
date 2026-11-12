//! Rez's factory presets: FreeDAWm's own, written for Rez (starting points, by the job at hand).
//!
//! Each preset lists only what it changes from Rez's defaults; the JSON a preset menu recalls
//! (`{"name", "group", "desc", "values"}`, as the FireStation engines' presets) carries every
//! parameter, so recalling one never leaves a setting of the previous preset behind.

use super::REZ_INFO;
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct RezPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> RezPreset {
    RezPreset { name, group, desc, set }
}

const HARD: f32 = 1.0;
const MID_SIDE: f32 = 1.0;

/// The presets, in menu order.
pub static REZ_PRESETS: [RezPreset; 111] = [
    // ---------------------------------------------------------------- Start Here
    preset("Start Here", "Even Hand", "A light touch across the whole range. Raise Depth until you hear it, then back off.", &[("depth", 55.0)]),
    preset(
        "Start Here",
        "Barely There",
        "For when you are not sure it needs anything: shallow, and half of the original mixed back in.",
        &[("depth", 42.0), ("mix", 60.0)],
    ),
    preset(
        "Start Here",
        "Find It With Delta",
        "Delta is on: you hear only what Rez removes. Shape the bands until that is all problem, then switch Delta off.",
        &[("depth", 65.0), ("delta", 1.0)],
    ),
    preset(
        "Start Here",
        "Firm Grip",
        "Hard mode for a source that really rings. It reaches into the low mids too; Mix keeps some of the original.",
        &[("mode", HARD), ("depth", 55.0), ("mix", 75.0)],
    ),
    preset("Start Here", "Highs Only", "Everything under 3 kHz is out of range and untouched.", &[("depth", 60.0), ("low", 3000.0)]),
    preset(
        "Start Here",
        "Mids Only",
        "The 300 Hz to 4 kHz band where most hardness lives, and nothing above or below it.",
        &[("depth", 62.0), ("low", 300.0), ("high", 4000.0)],
    ),
    // ---------------------------------------------------------------- Vocals
    preset(
        "Vocals",
        "Everyday Lead",
        "A lead vocal that is mostly fine: a little off the presence range, most of it left alone.",
        &[("b3_freq", 4000.0), ("b3_gain", 3.0), ("depth", 52.0), ("low", 200.0), ("high", 14000.0), ("release", 25.0), ("mix", 85.0)],
    ),
    preset(
        "Vocals",
        "Soften The Edge",
        "The 2.5 to 5 kHz bite of a pushed vocal, eased only on the notes that have it.",
        &[("b3_freq", 3400.0), ("b3_gain", 5.0), ("b3_q", 0.9), ("depth", 62.0), ("low", 1200.0), ("high", 9000.0), ("release", 30.0)],
    ),
    preset(
        "Vocals",
        "Ess Catcher",
        "Sibilance only: fast, narrow to the top, the body of the voice left alone.",
        &[
            ("b4_freq", 7200.0),
            ("b4_gain", 7.0),
            ("b4_q", 1.2),
            ("depth", 66.0),
            ("sharpness", 55.0),
            ("low", 2000.0),
            ("b3_on", 0.0),
            ("attack", 3.0),
            ("release", 45.0),
        ],
    ),
    preset(
        "Vocals",
        "Bright Mic Tamer",
        "A condenser with a hyped top: the 8 to 12 kHz lift brought back to earth.",
        &[("b4_freq", 10000.0), ("b4_gain", 5.0), ("depth", 62.0), ("low", 5000.0)],
    ),
    preset(
        "Vocals",
        "Out Of The Nose",
        "Honk and boxiness around 600 Hz to 1.2 kHz. Hard mode, because Rez goes easy down there otherwise.",
        &[
            ("mode", HARD),
            ("b2_freq", 850.0),
            ("b2_gain", 6.0),
            ("b2_q", 0.8),
            ("depth", 52.0),
            ("low", 300.0),
            ("high", 2500.0),
            ("mix", 80.0),
            ("release", 60.0),
        ],
    ),
    preset(
        "Vocals",
        "Close Mic Chest",
        "The low-mid bloom of a voice worked right on the microphone.",
        &[
            ("mode", HARD),
            ("b1_freq", 240.0),
            ("b1_gain", 9.0),
            ("b1_q", 0.7),
            ("depth", 48.0),
            ("low", 120.0),
            ("high", 1000.0),
            ("attack", 30.0),
            ("release", 120.0),
            ("mix", 70.0),
        ],
    ),
    preset(
        "Vocals",
        "Shout And Belt",
        "Shallow on purpose: quiet lines pass under the gate, the loud ones that turn hard are caught.",
        &[("b3_freq", 2800.0), ("b3_gain", 6.0), ("depth", 48.0), ("low", 800.0), ("high", 6000.0), ("attack", 10.0), ("release", 50.0)],
    ),
    preset(
        "Vocals",
        "Whisper Close",
        "Breath and air on an intimate take, smoothed without losing the closeness.",
        &[("depth", 58.0), ("sharpness", 35.0), ("low", 3000.0), ("release", 60.0), ("mix", 70.0)],
    ),
    preset(
        "Vocals",
        "Rap Presence",
        "Keeps the bite and takes only the spikes: narrow, picky, quick.",
        &[("depth", 55.0), ("sharpness", 60.0), ("selectivity", 65.0), ("low", 1500.0), ("high", 7000.0), ("attack", 5.0), ("release", 30.0)],
    ),
    preset(
        "Vocals",
        "Spoken Word",
        "A voice recorded for speech: the boxy middle and the sharp consonants, gently.",
        &[
            ("b2_freq", 700.0),
            ("b2_gain", 3.0),
            ("b4_freq", 7000.0),
            ("b4_gain", 4.0),
            ("depth", 58.0),
            ("low", 150.0),
            ("high", 12000.0),
            ("release", 60.0),
        ],
    ),
    preset(
        "Vocals",
        "Thin And Harsh Rescue",
        "A small, hard-sounding recording made easier to listen to. Not a cure, a help.",
        &[("mode", HARD), ("b3_freq", 2500.0), ("b3_gain", 4.0), ("depth", 48.0), ("low", 500.0), ("high", 5000.0), ("mix", 80.0)],
    ),
    preset(
        "Vocals",
        "Lead Bus",
        "Several lead takes summed: broad and slow, so it steadies the bus instead of chasing words.",
        &[("depth", 50.0), ("sharpness", 38.0), ("selectivity", 28.0), ("low", 300.0), ("attack", 30.0), ("release", 120.0), ("mix", 75.0)],
    ),
    preset(
        "Vocals",
        "Stack Polish",
        "Backing vocals as one: broad, gentle, slow enough not to chatter.",
        &[("depth", 58.0), ("sharpness", 32.0), ("selectivity", 20.0), ("low", 400.0), ("attack", 30.0), ("release", 140.0)],
    ),
    preset(
        "Vocals",
        "Choir In A Room",
        "Many voices and the room they are in: wide cuts that follow the swell.",
        &[("depth", 56.0), ("sharpness", 30.0), ("selectivity", 22.0), ("low", 250.0), ("attack", 40.0), ("release", 200.0)],
    ),
    // ---------------------------------------------------------------- Drums
    preset(
        "Drums",
        "Cymbal Wash",
        "Overheads that hiss and pile up: the top smoothed, the kit under it untouched.",
        &[("b4_freq", 9000.0), ("b4_gain", 4.0), ("depth", 64.0), ("sharpness", 58.0), ("low", 2000.0), ("release", 70.0)],
    ),
    preset(
        "Drums",
        "Overheads Even",
        "The whole overhead picture a little more even, toms and snare included.",
        &[("depth", 56.0), ("sharpness", 40.0), ("low", 400.0), ("release", 90.0)],
    ),
    preset(
        "Drums",
        "Hi-Hat Chill",
        "A hat that cuts too hard: its own band, fast in and fast out.",
        &[("b4_freq", 8500.0), ("b4_gain", 6.0), ("b4_q", 1.4), ("depth", 64.0), ("low", 4000.0), ("attack", 3.0), ("release", 30.0)],
    ),
    preset(
        "Drums",
        "Ride Ping",
        "The ping of a ride that pokes out of the wash.",
        &[("depth", 60.0), ("sharpness", 75.0), ("selectivity", 60.0), ("low", 2500.0), ("high", 9000.0)],
    ),
    preset(
        "Drums",
        "Snare Ring",
        "One singing overtone and nothing else: narrow, picky, firm.",
        &[
            ("mode", HARD),
            ("depth", 53.0),
            ("sharpness", 88.0),
            ("selectivity", 72.0),
            ("low", 250.0),
            ("high", 4000.0),
            ("release", 90.0),
            ("mix", 85.0),
        ],
    ),
    preset(
        "Drums",
        "Snare Crack Softer",
        "The top of the crack rounded off; the body and the wires stay.",
        &[("b3_freq", 4000.0), ("b3_gain", 5.0), ("depth", 58.0), ("low", 1500.0), ("high", 9000.0), ("attack", 2.0), ("release", 40.0)],
    ),
    preset(
        "Drums",
        "Hat Bleed In The Snare",
        "The top of a snare microphone calmed between hits, with Mix keeping the crack.",
        &[("depth", 68.0), ("low", 2000.0), ("sharpness", 50.0), ("attack", 15.0), ("release", 35.0), ("mix", 65.0)],
    ),
    preset(
        "Drums",
        "Kick Cardboard",
        "The boxy middle of a kick, so the click and the weight meet.",
        &[
            ("mode", HARD),
            ("b1_freq", 380.0),
            ("b1_gain", 8.0),
            ("b1_q", 1.0),
            ("depth", 52.0),
            ("low", 180.0),
            ("high", 1500.0),
            ("attack", 8.0),
            ("release", 80.0),
        ],
    ),
    preset(
        "Drums",
        "Kick Click Tamer",
        "A beater click that is all plastic: only the click, only as it lands.",
        &[("b3_freq", 3500.0), ("b3_gain", 5.0), ("depth", 60.0), ("low", 2000.0), ("high", 9000.0), ("attack", 1.0), ("release", 25.0)],
    ),
    preset(
        "Drums",
        "Tom Ring",
        "The note a tom keeps singing after the hit.",
        &[
            ("mode", HARD),
            ("b1_freq", 180.0),
            ("b1_gain", 10.0),
            ("b1_q", 1.2),
            ("depth", 50.0),
            ("sharpness", 85.0),
            ("selectivity", 70.0),
            ("low", 80.0),
            ("high", 1500.0),
            ("release", 150.0),
            ("mix", 85.0),
        ],
    ),
    preset(
        "Drums",
        "Room Glue",
        "A room microphone's hard reflections taken down a size: wide cuts, slow release.",
        &[("depth", 60.0), ("sharpness", 26.0), ("selectivity", 18.0), ("attack", 25.0), ("release", 220.0), ("mix", 80.0)],
    ),
    preset(
        "Drums",
        "Drum Bus Sheen",
        "The whole kit, a little smoother on top, with most of the punch mixed back in.",
        &[("depth", 50.0), ("sharpness", 40.0), ("selectivity", 30.0), ("low", 600.0), ("attack", 30.0), ("release", 100.0), ("mix", 70.0)],
    ),
    preset(
        "Drums",
        "Shakers And Tambourine",
        "Small bright percussion that takes over the top of a mix.",
        &[("b4_freq", 9000.0), ("b4_gain", 4.0), ("depth", 66.0), ("sharpness", 55.0), ("low", 5000.0), ("release", 40.0)],
    ),
    preset(
        "Drums",
        "Hand Drum Ring",
        "Congas and bongos with a note that hangs on.",
        &[("mode", HARD), ("depth", 48.0), ("sharpness", 80.0), ("selectivity", 65.0), ("low", 150.0), ("high", 3000.0), ("mix", 80.0)],
    ),
    preset(
        "Drums",
        "Crushed Loop Tamer",
        "A sampled loop with a hard, squashed midrange.",
        &[("depth", 62.0), ("sharpness", 48.0), ("selectivity", 25.0), ("low", 800.0), ("high", 8000.0)],
    ),
    // ---------------------------------------------------------------- Electric Guitar
    preset(
        "Electric Guitar",
        "Amp Fizz",
        "The grainy top of a distorted guitar, 4 to 8 kHz, without dulling the pick.",
        &[
            ("b4_freq", 5500.0),
            ("b4_gain", 5.0),
            ("b4_q", 0.9),
            ("depth", 64.0),
            ("sharpness", 52.0),
            ("low", 2000.0),
            ("high", 12000.0),
            ("release", 50.0),
        ],
    ),
    preset(
        "Electric Guitar",
        "Cab Honk",
        "The cardboard midrange of a small cabinet or a cheap impulse response.",
        &[("mode", HARD), ("b2_freq", 900.0), ("b2_gain", 5.0), ("depth", 48.0), ("low", 300.0), ("high", 2500.0), ("mix", 80.0)],
    ),
    preset(
        "Electric Guitar",
        "Lead Bite",
        "A solo that hurts on the high notes and is fine everywhere else.",
        &[("b3_freq", 3200.0), ("b3_gain", 5.0), ("depth", 58.0), ("low", 1500.0), ("high", 7000.0), ("attack", 8.0), ("release", 60.0)],
    ),
    preset(
        "Electric Guitar",
        "Clean Glass",
        "The ice-pick of a clean single-coil.",
        &[("b3_freq", 4500.0), ("b3_gain", 5.0), ("b3_q", 1.4), ("depth", 60.0), ("low", 2000.0), ("high", 9000.0)],
    ),
    preset(
        "Electric Guitar",
        "Palm Mute Thump",
        "Palm mutes that jump out of the low end of a heavy rhythm part.",
        &[
            ("mode", HARD),
            ("b1_freq", 130.0),
            ("b1_gain", 12.0),
            ("b1_q", 0.8),
            ("depth", 48.0),
            ("low", 60.0),
            ("high", 600.0),
            ("attack", 5.0),
            ("release", 120.0),
            ("mix", 75.0),
        ],
    ),
    preset(
        "Electric Guitar",
        "DI Before The Amp",
        "On the dry signal ahead of an amp model, so the amp sees an evener guitar.",
        &[("depth", 55.0), ("sharpness", 50.0), ("low", 500.0), ("high", 6000.0), ("attack", 5.0), ("release", 40.0)],
    ),
    preset(
        "Electric Guitar",
        "Wall Of Rhythm",
        "A guitar bus that has grown a hard midrange: both sides treated as one.",
        &[
            ("b3_freq", 2600.0),
            ("b3_gain", 3.0),
            ("depth", 58.0),
            ("sharpness", 38.0),
            ("selectivity", 28.0),
            ("low", 700.0),
            ("high", 9000.0),
            ("attack", 25.0),
            ("release", 110.0),
        ],
    ),
    // ---------------------------------------------------------------- Acoustic Guitar
    preset(
        "Acoustic Guitar",
        "Acoustic Boom",
        "The body note that jumps out of a strummed acoustic. Follows the chords instead of thinning them all.",
        &[
            ("mode", HARD),
            ("b1_freq", 190.0),
            ("b1_gain", 10.0),
            ("b1_q", 0.9),
            ("depth", 47.0),
            ("low", 90.0),
            ("high", 1000.0),
            ("attack", 20.0),
            ("release", 150.0),
            ("mix", 75.0),
        ],
    ),
    preset(
        "Acoustic Guitar",
        "Nylon Thumb",
        "The thumb on the low strings of a classical guitar.",
        &[("mode", HARD), ("b1_freq", 150.0), ("b1_gain", 10.0), ("depth", 46.0), ("low", 70.0), ("high", 800.0)],
    ),
    preset(
        "Acoustic Guitar",
        "Pick And Squeak",
        "String noise and a hard pick on a close-miked acoustic.",
        &[("b3_freq", 3000.0), ("b3_gain", 4.0), ("depth", 62.0), ("sharpness", 60.0), ("low", 1500.0), ("attack", 5.0), ("release", 40.0)],
    ),
    preset(
        "Acoustic Guitar",
        "Bright Strum",
        "Fresh strings and a hard strum, a little less glassy.",
        &[("b4_freq", 7000.0), ("b4_gain", 4.0), ("depth", 58.0), ("sharpness", 40.0), ("low", 2500.0), ("release", 60.0)],
    ),
    preset(
        "Acoustic Guitar",
        "Pickup Quack",
        "The hard, plastic midrange of an under-saddle pickup.",
        &[("b3_freq", 2200.0), ("b3_gain", 6.0), ("b3_q", 1.2), ("depth", 60.0), ("sharpness", 60.0), ("low", 800.0), ("high", 6000.0)],
    ),
    preset(
        "Acoustic Guitar",
        "Twelve String Shimmer",
        "The doubled top strings, kept but not piercing.",
        &[("depth", 56.0), ("sharpness", 45.0), ("low", 3000.0), ("release", 80.0), ("mix", 80.0)],
    ),
    preset(
        "Acoustic Guitar",
        "Mandolin And Banjo",
        "Small, loud, bright strings: the attack of each note.",
        &[("b3_freq", 3000.0), ("b3_gain", 5.0), ("depth", 60.0), ("low", 1200.0), ("high", 9000.0), ("attack", 3.0), ("release", 40.0)],
    ),
    // ---------------------------------------------------------------- Bass
    preset(
        "Bass",
        "Even Notes",
        "The one or two notes that bloom on every bass. Slow and Hard, so it rides the note, not the pluck.",
        &[
            ("mode", HARD),
            ("b1_freq", 110.0),
            ("b1_gain", 12.0),
            ("b1_q", 0.6),
            ("b2_freq", 260.0),
            ("b2_gain", 8.0),
            ("depth", 46.0),
            ("sharpness", 60.0),
            ("low", 40.0),
            ("high", 1000.0),
            ("attack", 40.0),
            ("release", 180.0),
            ("mix", 70.0),
        ],
    ),
    preset(
        "Bass",
        "Sub Even",
        "Under 250 Hz only: the lowest notes kept the same size.",
        &[
            ("mode", HARD),
            ("b1_freq", 60.0),
            ("b1_gain", 12.0),
            ("b1_q", 0.7),
            ("depth", 50.0),
            ("sharpness", 55.0),
            ("low", 25.0),
            ("high", 250.0),
            ("attack", 50.0),
            ("release", 250.0),
            ("mix", 65.0),
        ],
    ),
    preset(
        "Bass",
        "Upright Body",
        "A double bass with a note or two that takes over the room.",
        &[
            ("mode", HARD),
            ("b1_freq", 140.0),
            ("b1_gain", 10.0),
            ("depth", 46.0),
            ("low", 50.0),
            ("high", 700.0),
            ("attack", 40.0),
            ("release", 220.0),
            ("mix", 70.0),
        ],
    ),
    preset(
        "Bass",
        "Fret Clank",
        "Finger and fret noise up top, the low end out of range and untouched.",
        &[("b3_freq", 2400.0), ("b3_gain", 5.0), ("depth", 64.0), ("low", 900.0), ("high", 8000.0), ("attack", 4.0), ("release", 45.0)],
    ),
    preset(
        "Bass",
        "Pick Attack",
        "The click of a pick, eased so the note is what you hear.",
        &[("b3_freq", 2800.0), ("b3_gain", 5.0), ("depth", 60.0), ("low", 1200.0), ("high", 6000.0), ("attack", 2.0), ("release", 40.0)],
    ),
    preset(
        "Bass",
        "Slap Tamer",
        "Slapped and popped notes that leap out: only the peaks, only for an instant.",
        &[
            ("b3_freq", 3500.0),
            ("b3_gain", 4.0),
            ("depth", 58.0),
            ("selectivity", 50.0),
            ("low", 1500.0),
            ("high", 9000.0),
            ("attack", 1.0),
            ("release", 35.0),
        ],
    ),
    preset(
        "Bass",
        "Synth Bass Growl",
        "The midrange snarl of a driven synth bass, held where it does not mask the vocal.",
        &[("mode", HARD), ("b2_freq", 700.0), ("b2_gain", 5.0), ("depth", 50.0), ("low", 150.0), ("high", 3000.0), ("mix", 75.0)],
    ),
    // ---------------------------------------------------------------- Keys
    preset(
        "Keys",
        "Hammer Softener",
        "A bright piano's hard hammer strike, with the ring after it left in.",
        &[("b3_freq", 2800.0), ("b3_gain", 4.0), ("depth", 60.0), ("sharpness", 50.0), ("low", 900.0), ("attack", 3.0), ("release", 35.0)],
    ),
    preset(
        "Keys",
        "Grand Piano Even",
        "A whole piano a little more even from bottom to top.",
        &[("depth", 52.0), ("low", 200.0), ("high", 10000.0), ("attack", 15.0), ("release", 120.0), ("mix", 80.0)],
    ),
    preset(
        "Keys",
        "Upright Piano Box",
        "The wooden box around an upright, 300 to 800 Hz.",
        &[("mode", HARD), ("b2_freq", 500.0), ("b2_gain", 6.0), ("depth", 47.0), ("low", 200.0), ("high", 2000.0), ("mix", 75.0), ("release", 120.0)],
    ),
    preset(
        "Keys",
        "Electric Piano Bark",
        "The midrange bark of an electric piano played hard, eased so chords stay round.",
        &[
            ("mode", HARD),
            ("b2_freq", 1100.0),
            ("b2_gain", 5.0),
            ("b2_q", 0.8),
            ("depth", 45.0),
            ("low", 350.0),
            ("high", 3500.0),
            ("mix", 70.0),
            ("release", 90.0),
        ],
    ),
    preset(
        "Keys",
        "Clavinet Spikes",
        "A funky, pointed part made to sit without losing its snap.",
        &[("b3_freq", 2800.0), ("b3_gain", 5.0), ("depth", 60.0), ("low", 1500.0), ("high", 7000.0), ("attack", 2.0), ("release", 30.0)],
    ),
    preset(
        "Keys",
        "Organ Whistle",
        "One drawbar or one note that whistles over everything.",
        &[("depth", 62.0), ("sharpness", 85.0), ("selectivity", 70.0), ("low", 1500.0), ("high", 8000.0), ("release", 100.0)],
    ),
    preset(
        "Keys",
        "Accordion Reed",
        "Reeds that turn nasal when pushed.",
        &[("b3_freq", 2500.0), ("b3_gain", 5.0), ("depth", 58.0), ("low", 1000.0), ("high", 7000.0), ("release", 110.0)],
    ),
    preset(
        "Keys",
        "Vibes And Bells",
        "Struck metal: the few partials that ring loudest, the rest left to shimmer.",
        &[("depth", 56.0), ("sharpness", 80.0), ("selectivity", 65.0), ("low", 1500.0), ("release", 150.0), ("mix", 80.0)],
    ),
    // ---------------------------------------------------------------- Strings & Brass
    preset(
        "Strings & Brass",
        "Bow Edge",
        "Rosin and edge on close strings: slow to act, slow to let go.",
        &[
            ("b3_freq", 3600.0),
            ("b3_gain", 4.0),
            ("depth", 60.0),
            ("sharpness", 42.0),
            ("low", 1500.0),
            ("high", 10000.0),
            ("attack", 35.0),
            ("release", 160.0),
        ],
    ),
    preset(
        "Strings & Brass",
        "Solo Violin Close",
        "A violin a hand's width from the microphone.",
        &[
            ("b3_freq", 3200.0),
            ("b3_gain", 5.0),
            ("b3_q", 1.2),
            ("depth", 60.0),
            ("low", 2000.0),
            ("high", 9000.0),
            ("attack", 30.0),
            ("release", 140.0),
        ],
    ),
    preset(
        "Strings & Brass",
        "Cello Wolf Note",
        "The one note a cello howls on.",
        &[
            ("mode", HARD),
            ("b1_freq", 180.0),
            ("b1_gain", 10.0),
            ("b1_q", 1.5),
            ("depth", 48.0),
            ("sharpness", 85.0),
            ("selectivity", 70.0),
            ("low", 80.0),
            ("high", 600.0),
            ("attack", 40.0),
            ("release", 200.0),
            ("mix", 80.0),
        ],
    ),
    preset(
        "Strings & Brass",
        "Section Smooth",
        "A string section that goes hard when it plays loud: wide and unhurried.",
        &[("depth", 56.0), ("sharpness", 34.0), ("selectivity", 24.0), ("low", 600.0), ("attack", 40.0), ("release", 200.0)],
    ),
    preset(
        "Strings & Brass",
        "Brass Blare",
        "A horn section at full volume, 1.5 to 4 kHz, held back only on the loud phrases.",
        &[
            ("b3_freq", 2300.0),
            ("b3_gain", 5.0),
            ("b3_q", 0.8),
            ("depth", 54.0),
            ("sharpness", 40.0),
            ("selectivity", 30.0),
            ("low", 800.0),
            ("high", 7000.0),
            ("attack", 15.0),
            ("release", 100.0),
        ],
    ),
    preset(
        "Strings & Brass",
        "Trumpet Laser",
        "A lead trumpet's top notes, without muting the horn.",
        &[("b3_freq", 3500.0), ("b3_gain", 6.0), ("depth", 52.0), ("low", 2000.0), ("high", 8000.0), ("attack", 10.0), ("release", 80.0)],
    ),
    preset(
        "Strings & Brass",
        "Sax Honk",
        "The honk in the middle of a saxophone.",
        &[("mode", HARD), ("b2_freq", 1200.0), ("b2_gain", 5.0), ("depth", 47.0), ("low", 400.0), ("high", 3000.0), ("mix", 80.0), ("release", 90.0)],
    ),
    preset(
        "Strings & Brass",
        "Flute Breath",
        "Breath noise over a flute, eased rather than removed.",
        &[("depth", 60.0), ("sharpness", 40.0), ("low", 4000.0), ("release", 70.0), ("mix", 75.0)],
    ),
    preset(
        "Strings & Brass",
        "Reed And Keys",
        "Key clicks and reed buzz on close woodwinds.",
        &[("depth", 58.0), ("sharpness", 60.0), ("low", 1500.0), ("high", 9000.0), ("attack", 4.0), ("release", 50.0)],
    ),
    // ---------------------------------------------------------------- Synths
    preset(
        "Synths",
        "Filter Sweep Tamer",
        "A resonant filter that screams at the top of its sweep. Narrow and quick enough to follow it.",
        &[("depth", 68.0), ("sharpness", 72.0), ("selectivity", 60.0), ("attack", 5.0), ("release", 40.0)],
    ),
    preset(
        "Synths",
        "Lead Whistle",
        "A lead with one overtone that drills: very narrow, very picky.",
        &[("depth", 62.0), ("sharpness", 88.0), ("selectivity", 72.0), ("low", 1500.0), ("attack", 5.0), ("release", 60.0)],
    ),
    preset(
        "Synths",
        "Saw Stack Glare",
        "Stacked saws that fill the top of a mix with glare.",
        &[("b3_freq", 4000.0), ("b3_gain", 3.0), ("depth", 62.0), ("low", 2000.0), ("high", 12000.0), ("release", 60.0)],
    ),
    preset(
        "Synths",
        "Pluck Edge",
        "The front of every plucked note, so a busy part stops ticking.",
        &[("depth", 60.0), ("low", 2500.0), ("attack", 1.0), ("release", 25.0)],
    ),
    preset(
        "Synths",
        "Bell Tone Bite",
        "Glassy, bell-like patches whose partials sting.",
        &[("depth", 58.0), ("sharpness", 78.0), ("selectivity", 60.0), ("low", 1500.0), ("high", 10000.0), ("release", 90.0)],
    ),
    preset(
        "Synths",
        "Thick Bass Mids",
        "A wide, detuned bass whose middle swallows the track.",
        &[
            ("mode", HARD),
            ("depth", 48.0),
            ("sharpness", 50.0),
            ("selectivity", 20.0),
            ("low", 150.0),
            ("high", 2500.0),
            ("attack", 15.0),
            ("release", 100.0),
            ("mix", 70.0),
        ],
    ),
    preset(
        "Synths",
        "Pad Smoother",
        "A thick pad made easier to sit things on: broad, slow, never obvious.",
        &[("depth", 60.0), ("sharpness", 28.0), ("selectivity", 20.0), ("attack", 50.0), ("release", 260.0), ("mix", 80.0)],
    ),
    preset(
        "Synths",
        "Busy Arp, Softer",
        "An arpeggio that competes with the voice, taken down in the range the voice lives in.",
        &[("depth", 55.0), ("sharpness", 35.0), ("selectivity", 20.0), ("low", 1000.0), ("high", 6000.0), ("mix", 80.0)],
    ),
    // ---------------------------------------------------------------- Mix Bus
    preset(
        "Mix Bus",
        "Quiet Polish",
        "A finished mix, a couple of dB at most where it turns hard. Linked, slow, mostly out of the way.",
        &[("depth", 48.0), ("sharpness", 40.0), ("selectivity", 30.0), ("low", 500.0), ("attack", 40.0), ("release", 150.0), ("mix", 60.0)],
    ),
    preset(
        "Mix Bus",
        "Top End Calm",
        "Only above 4 kHz: a bright mix made less tiring.",
        &[("depth", 52.0), ("sharpness", 40.0), ("selectivity", 30.0), ("low", 4000.0), ("attack", 30.0), ("release", 120.0), ("mix", 60.0)],
    ),
    preset(
        "Mix Bus",
        "Midrange Watch",
        "Only 400 Hz to 3 kHz, half mixed in: for a mix that barks when everyone plays.",
        &[
            ("depth", 55.0),
            ("sharpness", 40.0),
            ("selectivity", 40.0),
            ("low", 400.0),
            ("high", 3000.0),
            ("attack", 40.0),
            ("release", 150.0),
            ("mix", 50.0),
        ],
    ),
    preset(
        "Mix Bus",
        "Loud Sections Only",
        "Shallow, so verses pass under the gate and only the loudest sections are touched.",
        &[("depth", 42.0), ("sharpness", 40.0), ("selectivity", 30.0), ("low", 800.0), ("attack", 20.0), ("release", 100.0)],
    ),
    preset(
        "Mix Bus",
        "Stem Polish",
        "A group of instruments printed together, steadied before it meets the rest.",
        &[("depth", 52.0), ("sharpness", 36.0), ("selectivity", 26.0), ("low", 300.0), ("attack", 30.0), ("release", 130.0), ("mix", 70.0)],
    ),
    preset(
        "Mix Bus",
        "Live Recording Tamer",
        "A stage recording with everything in every microphone.",
        &[
            ("b3_freq", 3000.0),
            ("b3_gain", 3.0),
            ("depth", 58.0),
            ("sharpness", 40.0),
            ("low", 500.0),
            ("high", 12000.0),
            ("attack", 25.0),
            ("release", 140.0),
        ],
    ),
    preset(
        "Mix Bus",
        "Hard Master Softener",
        "A finished, loud master that is hard on the ear, taken down a notch.",
        &[
            ("depth", 48.0),
            ("sharpness", 44.0),
            ("selectivity", 34.0),
            ("low", 2000.0),
            ("high", 12000.0),
            ("attack", 35.0),
            ("release", 160.0),
            ("mix", 55.0),
        ],
    ),
    preset(
        "Mix Bus",
        "Sides Only",
        "Mid/side with the balance all the way to the sides: wide cymbals and guitars smoothed, the centre untouched.",
        &[("stereo", MID_SIDE), ("link", 0.0), ("balance", 100.0), ("depth", 60.0), ("low", 1500.0), ("attack", 25.0), ("release", 110.0)],
    ),
    preset(
        "Mix Bus",
        "Centre Only",
        "Mid/side with the balance all the way to the middle: the vocal and snare eased, the width left as it is.",
        &[
            ("stereo", MID_SIDE),
            ("link", 0.0),
            ("balance", -100.0),
            ("depth", 58.0),
            ("low", 1000.0),
            ("high", 9000.0),
            ("attack", 20.0),
            ("release", 90.0),
        ],
    ),
    // ---------------------------------------------------------------- Reverbs & Returns
    preset(
        "Reverbs & Returns",
        "Plate Ring Tamer",
        "The metallic ring a plate adds to loud notes.",
        &[("depth", 62.0), ("sharpness", 55.0), ("selectivity", 45.0), ("low", 800.0), ("attack", 20.0), ("release", 200.0)],
    ),
    preset(
        "Reverbs & Returns",
        "Hall Build-Up",
        "A long reverb whose low mids pile up phrase after phrase.",
        &[
            ("depth", 60.0),
            ("sharpness", 30.0),
            ("selectivity", 18.0),
            ("low", 200.0),
            ("high", 6000.0),
            ("attack", 50.0),
            ("release", 300.0),
            ("mix", 80.0),
        ],
    ),
    preset(
        "Reverbs & Returns",
        "Feedback Whistle",
        "A delay or reverb feeding back into one pitch.",
        &[("depth", 64.0), ("sharpness", 85.0), ("selectivity", 70.0), ("attack", 10.0), ("release", 150.0)],
    ),
    preset(
        "Reverbs & Returns",
        "Spring Boing",
        "The boing of a spring tank, kept as a flavour and not a feature.",
        &[("mode", HARD), ("depth", 50.0), ("sharpness", 60.0), ("low", 300.0), ("high", 3000.0), ("mix", 75.0)],
    ),
    // ---------------------------------------------------------------- Dialogue & Post
    preset(
        "Dialogue & Post",
        "Clip Mic Chest",
        "A clip-on microphone on the chest: the thick low mids it always hears.",
        &[
            ("mode", HARD),
            ("b1_freq", 250.0),
            ("b1_gain", 9.0),
            ("depth", 46.0),
            ("low", 100.0),
            ("high", 800.0),
            ("attack", 30.0),
            ("release", 120.0),
            ("mix", 75.0),
        ],
    ),
    preset(
        "Dialogue & Post",
        "Dialogue Sibilance",
        "Sharp consonants in speech, without a lisp.",
        &[("b4_freq", 7500.0), ("b4_gain", 6.0), ("depth", 64.0), ("sharpness", 55.0), ("low", 4000.0), ("attack", 3.0), ("release", 45.0)],
    ),
    preset(
        "Dialogue & Post",
        "Room Ring",
        "A small room that rings at a few pitches whenever someone speaks.",
        &[("mode", HARD), ("depth", 58.0), ("sharpness", 85.0), ("selectivity", 70.0), ("attack", 40.0), ("release", 200.0), ("mix", 80.0)],
    ),
    preset(
        "Dialogue & Post",
        "Harsh Location Sound",
        "A recording made in a hurry in a hard place.",
        &[("b3_freq", 3000.0), ("b3_gain", 4.0), ("depth", 60.0), ("low", 1500.0), ("high", 8000.0), ("release", 50.0)],
    ),
    preset(
        "Dialogue & Post",
        "Bright Booth",
        "A narration booth and a bright microphone: the top taken down a little.",
        &[("depth", 56.0), ("sharpness", 42.0), ("low", 3000.0), ("release", 60.0), ("mix", 80.0)],
    ),
    // ---------------------------------------------------------------- With A Key
    preset(
        "With A Key",
        "Make Room For The Key",
        "Pick a sidechain key in the Mix window (a vocal, say): this track ducks only where the key is strong.",
        &[("depth", 70.0), ("sharpness", 26.0), ("selectivity", 12.0), ("low", 250.0), ("attack", 10.0), ("release", 120.0)],
    ),
    preset(
        "With A Key",
        "Guitars Under The Vocal",
        "On the guitars, keyed from the vocal: they step back in the vocal's range while it sings.",
        &[("depth", 66.0), ("sharpness", 28.0), ("selectivity", 12.0), ("low", 800.0), ("high", 6000.0), ("attack", 10.0), ("release", 120.0)],
    ),
    preset(
        "With A Key",
        "Pad Under The Lead",
        "On a pad, keyed from the lead line: the pad opens a slot and closes it slowly.",
        &[("depth", 68.0), ("sharpness", 24.0), ("selectivity", 10.0), ("low", 300.0), ("attack", 30.0), ("release", 200.0)],
    ),
    preset(
        "With A Key",
        "Reverb Clears For The Dry",
        "On a reverb return, keyed from the dry signal: the tail stays out of the way of the words.",
        &[("depth", 70.0), ("sharpness", 26.0), ("selectivity", 10.0), ("attack", 5.0), ("release", 150.0)],
    ),
    preset(
        "With A Key",
        "Kick Through The Bass",
        "On the bass, keyed from the kick: the notes give way where the kick lands. Hard, so it reaches the low end.",
        &[
            ("mode", HARD),
            ("b1_freq", 80.0),
            ("b1_gain", 12.0),
            ("b1_q", 0.6),
            ("depth", 55.0),
            ("sharpness", 30.0),
            ("selectivity", 10.0),
            ("low", 30.0),
            ("high", 1000.0),
            ("attack", 2.0),
            ("release", 90.0),
        ],
    ),
    preset(
        "With A Key",
        "Snare Through The Guitars",
        "On a guitar bus, keyed from the snare: a moment of space on every backbeat.",
        &[
            ("mode", HARD),
            ("depth", 55.0),
            ("sharpness", 30.0),
            ("selectivity", 10.0),
            ("low", 150.0),
            ("high", 5000.0),
            ("attack", 2.0),
            ("release", 80.0),
            ("mix", 80.0),
        ],
    ),
    // ---------------------------------------------------------------- Creative
    preset(
        "Creative",
        "Resonances Only",
        "Delta and Hard together: what plays is only the ringing part of the sound.",
        &[("mode", HARD), ("depth", 85.0), ("sharpness", 80.0), ("selectivity", 60.0), ("delta", 1.0)],
    ),
    preset(
        "Creative",
        "Ironed Flat",
        "Everything that sticks out, pressed down: a sound with its character taken away.",
        &[("mode", HARD), ("depth", 90.0), ("sharpness", 70.0), ("selectivity", 0.0), ("release", 15.0)],
    ),
    preset(
        "Creative",
        "Slow Bloom",
        "Very slow in and very slow out: the tone shifts as a note is held.",
        &[("depth", 75.0), ("sharpness", 30.0), ("selectivity", 10.0), ("attack", 600.0), ("release", 1200.0)],
    ),
    preset(
        "Creative",
        "The Part It Throws Away",
        "Delta with broad cuts: a soft, filtered shadow of the sound for layering.",
        &[("depth", 80.0), ("sharpness", 20.0), ("selectivity", 0.0), ("delta", 1.0)],
    ),
    preset(
        "Creative",
        "Hollow Middle",
        "The middle scooped by the sound itself, different on every note.",
        &[
            ("mode", HARD),
            ("b2_freq", 1000.0),
            ("b2_gain", 9.0),
            ("depth", 80.0),
            ("sharpness", 25.0),
            ("selectivity", 0.0),
            ("low", 300.0),
            ("high", 3000.0),
        ],
    ),
    preset(
        "Creative",
        "Air Ghost",
        "Delta on the top only: the fizz and breath by themselves.",
        &[("depth", 80.0), ("sharpness", 60.0), ("low", 6000.0), ("delta", 1.0)],
    ),
];

/// A JSON string for `text` (quotes and backslashes escaped; the presets' own text has no
/// control characters).
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One preset as the JSON a preset menu recalls: every parameter, the preset's own values over
/// the defaults.
fn json_of(p: &RezPreset) -> String {
    let values: Vec<String> = REZ_INFO
        .params
        .iter()
        .map(|info| {
            let v = p.set.iter().find(|(id, _)| *id == info.id).map_or(info.default, |(_, v)| info.clamp(*v));
            format!("{}:{v}", quoted(info.id))
        })
        .collect();
    format!("{{\"name\":{},\"group\":{},\"desc\":{},\"values\":{{{}}}}}", quoted(p.name), quoted(p.group), quoted(p.desc), values.join(","))
}

/// The factory presets as `(name, json)` pairs, in menu order (built once).
pub fn factory_presets() -> &'static [(&'static str, &'static str)] {
    static BUILT: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    BUILT.get_or_init(|| REZ_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plugin;
    use crate::plugins::fs::FsPreset;
    use crate::plugins::rez::Rez;

    #[test]
    fn every_preset_is_well_formed() {
        let mut names = std::collections::HashSet::new();
        for p in &REZ_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{} is listed twice", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty(), "{}", p.name);
            let mut ids = std::collections::HashSet::new();
            for (id, v) in p.set {
                let info = REZ_INFO.param(id).unwrap_or_else(|| panic!("{}: no parameter {id}", p.name));
                assert!(ids.insert(*id), "{}: {id} set twice", p.name);
                assert_eq!(info.clamp(*v), *v, "{}: {id} = {v} is out of range", p.name);
            }
            // The range stays the right way round.
            let get = |id: &str| p.set.iter().find(|(k, _)| *k == id).map_or(REZ_INFO.param(id).unwrap().default, |(_, v)| *v);
            assert!(get("low") < get("high"), "{}: Low is not below High", p.name);
        }
    }

    #[test]
    fn the_json_carries_every_parameter_and_parses_back() {
        let list = factory_presets();
        assert_eq!(list.len(), REZ_PRESETS.len());
        for ((name, json), p) in list.iter().zip(&REZ_PRESETS) {
            let parsed = FsPreset::parse(json, &REZ_INFO).unwrap_or_else(|| panic!("{name} does not parse"));
            assert_eq!((parsed.name.as_str(), parsed.group.as_str()), (p.name, p.group));
            assert_eq!(parsed.values.len(), REZ_INFO.params.len(), "{name}");
            for (id, v) in p.set {
                assert_eq!(parsed.values.get(*id), Some(v), "{name}: {id}");
            }
            let json: serde_json::Value = serde_json::from_str(json).unwrap();
            assert_eq!(json["desc"], p.desc);
        }
        // Built once: the same strings every time.
        assert!(std::ptr::eq(list, factory_presets()));
    }

    #[test]
    fn every_preset_processes_sound_and_only_the_delta_one_inverts_it() {
        for p in &REZ_PRESETS {
            let mut r = Rez::new();
            r.prepare(48_000.0, 512, 2);
            for (id, v) in p.set {
                assert!(r.set_param(id, *v));
            }
            let mut seed = 0x9E37_79B9u32;
            let mut io: Vec<Vec<f32>> = (0..2)
                .map(|_| {
                    (0..48_000)
                        .map(|i| {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                            noise * 0.2 + 0.2 * (std::f32::consts::TAU * 3000.0 * i as f32 / 48_000.0).sin()
                        })
                        .collect()
                })
                .collect();
            let input_peak = io[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            for start in (0..48_000).step_by(512) {
                let mut block: Vec<Vec<f32>> = io.iter().map(|c| c[start..(start + 512).min(48_000)].to_vec()).collect();
                let n = block[0].len();
                r.process(&mut block, n);
                for (c, b) in io.iter_mut().zip(&block) {
                    c[start..start + n].copy_from_slice(b);
                }
            }
            let out_peak = io[0][24_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(io.iter().flatten().all(|v| v.is_finite()), "{}", p.name);
            assert!(out_peak < input_peak * 1.5, "{} made it louder: {out_peak} from {input_peak}", p.name);
            assert!(out_peak > 0.0, "{} is silent", p.name);
        }
    }
}
