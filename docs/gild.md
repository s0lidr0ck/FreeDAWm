# Gild: effects on chosen words

**Gild** puts an effect on just the words you choose in a vocal: the echo that repeats only the
last word of a line, a reverb that blooms after one word, a reversed swell rising into a word, a
double, a lower octave, some grit, or harmony on a single word. You pick words in the lyric;
FreeDAWm does the rest, and it all follows your edits.

## Before you start: time the words

Gild works on the words of the lyric, timed on the vocal track. Type the lyrics in the Lyrics tab
and time them (see [Timing the words](lyrics.md#timing-the-words): Time Words to Vocal, or tap
them in), or use transcribed words. If the track has no timed words yet, the Gild panel says so
and offers Time Words to Vocal and Tap Words….

## Gilding a word, step by step

1. Open the Vocal Editor on the vocal (AI › Tune Vocal… or AI › Gild Words › Open…), and click
   **Gild** in its header. The Gild panel takes the place of the Tuning panel (click **Tuning** to
   go back).
2. **Choose words** in the word lane under the ruler: click a word; Shift-click to make a phrase
   (adjacent words become one gilding); Ctrl-click (Cmd on a Mac) to add separate words (each
   gets its own gilding, with the same settings). The panel's **Select** buttons pick the last
   word of every line, the first word of every line, or every time the selected word is sung.
3. **Turn an effect on** with its button. The words are gilded at once, starting from the settings
   you used last for that effect. Play: you hear it straight away.
4. **Shape it** on the effect's card. Every change is one Undo step (a whole slider drag too).
5. Gilded words get a gold bar along their foot, here and in the track's word strip in the Edit
   window; hover one to see what is on it. The **Gild strip** under the word lane shows each
   gilding as a bar with a short mark per effect: **Sh** Shift, **Gr** Grit, **Db** Double,
   **Hm** Harmony, **Dl** Delay, **Rv** Reverb, **Sw** Swell-in. Click a bar to select its words.
6. With one gilding selected, drag the **handles** at the ends of its bar to start earlier (catch
   the consonant) or end later (take the breath out); **Audition** plays the span and its tail.
   A gilding whose handles you moved keeps its own times; **Follow words** lets it follow its
   words again.

**Copy Gild** / **Paste Gild** puts one gilding's effects on other words. **Remove** takes Gild
off the selected words; **Remove All Gild on This Track…** (also in AI › Gild Words) asks first.
**Presets…** gives ready-made settings for one effect ("Quarter-note throw", "Long hall throw",
"Tight double"…) or a whole set ("Last-word throw", "Swell into the word", "Big ending",
"Spotlight word").

## The effects

Effects run in a fixed order: Shift and Grit change the word itself; then Double, Harmony, Delay
and Reverb are added, each hearing the word and what the effects before it added (so a Reverb
also rings the echoes); Swell-in rises into the word.

| Effect | What it does | Controls |
|---|---|---|
| **Shift** | Moves the word's pitch and throat size (formant), with the Vocal Editor's formant-keeping shifter. | Pitch (−12…+12 semitones), Throat (−6…+6), Layered or In place, Level (layered) |
| **Grit** | Saturates or distorts the word and blends it with itself. | Drive, Character (Warm, Bright, Hard), Blend, Level |
| **Double** | A second copy a little late and a little sharp. | Detune (2–30 cents), Late by (5–60 ms), Level |
| **Harmony** | Up to three voices that follow the word in the song's key (the Key ruler; a key change re-makes them). | Each voice: interval (3rd, 4th, 5th, 6th, octave, up or down), Level, Pan |
| **Delay** | A throw: only the word feeds an echo that repeats on after it. Follows the tempo map. | Time (Tempo or Free), Note, Free time, Repeats, Low cut, High cut, Bounce (left–right, stereo tracks), Level |
| **Reverb** | A throw: the word rings on in a space. | Space (Room, Hall, Plate, Cloud), Size, Decay, Pre-delay, Width, Level |
| **Swell-in** | A reversed reverb of the word that rises into it and stops as it starts. | Length, Ends at (before or just after the word's start), Space, Level |

Shift, Grit, Double, Harmony and Swell-in are **made ahead** in the background a moment after a
change ("Gilding Lead Vox…" in the status bar); until then the word plays without them. Bounce,
stems, Freeze and Commit make anything still missing first.

## Where the sound goes

Everything Gild adds comes back into the **same track**, after its inserts and before its fader,
sends and pan: the throws hear the vocal as your EQ, compressor and tuning made it, and the
fader, sends, mute and automation move the dry vocal and its effects together. Shift and Grit
change the word in the clip's audio, before the inserts. Adding Gild doesn't change the track's
latency.

**Dry on / off** (top of the panel; the header says "Dry off"): off, the track gives only what Gild
adds. Duplicate the vocal track, turn Dry off on the copy and remove Gild from the original: the
effects now sit on their own fader, with their own inserts and sends. **Level** trims everything
Gild adds on the track.

## How it follows your edits

A gilding lives on the clip, in the clip's own time, like the words: move, nudge, trim, split,
copy, comp or undo the clip and its gildings go along; they are saved with the session (older
versions of FreeDAWm open the session and ignore them). When you retime a word, the gilding
follows it (unless you moved its handles); if you time the line again, it finds its words by
their text. A track duplicate keeps its gildings; Make Harmony doesn't copy them unless asked
(`keep_gild`).

## Limits

- On a **mono** track the throws are mono like the track (Bounce and Width need a stereo track;
  a stereo duplicate with Dry off gives wide throws).
- Shift (in place) and Grit work on a clip played as recorded (not stretched, reversed, looped
  or transposed). On a clip played through an **ARA plug-in**, Shift, Grit, Double and Harmony
  are refused (they are made from the recorded audio, which the plug-in replaces); Delay, Reverb
  and Swell-in work (the throws gild what the plug-in plays; the swell is made from the
  recording).
- A swell before the start of the session can't be heard.
- Live throws cost CPU while they ring: gildings with the very same throw settings share one
  delay and one reverb, so a throw on the last word of every line costs about as much as one.
  Twenty throws each with their own settings, all ringing, cost about a quarter of one core.
- A frozen track plays its Gild as frozen; thaw it to change Gild.
- Backwards play, input monitoring and a stopped transport don't run Gild.

Every action is a command (`gild.*`, see `docs/control-protocol.md`), so scripts, Scout and remote
control can gild too.
