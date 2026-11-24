# The lyric sheet

**Window › Transport & Session › Lyrics** is a plain text editor for the song's words. FreeDAWm numbers the lyric lines,
counts their syllables, letters their rhymes and keeps each line in step with its marker on the
Lyrics ruler and with the Teleprompter. This page covers how the sheet is written: section
headings, chords, and how chords move between the sheet and the Chords ruler.

The sheet follows the ChordPro convention many songwriters already use: `{...}` for sections,
`[Am]` for chords.

## Section headings: `{Verse 1}`

A line that is only a name in curly brackets starts a section:

```
{Verse 1}
When the night was long
And my heart could not be still

{Chorus}
Jesus, the only one
```

The heading names the section for the Teleprompter, the fit check (Verse 2's lines are compared
with Verse 1's, a repeated Chorus with the first) and Copy for AI. Blank lines separate; every
other line is a lyric line, numbered from 1.

**Sheets from before 0.10.0** wrote headings in square brackets (`[Verse 1]`). They open as they
were: as a session loads, a line that is only a square-bracketed name that is not a chord becomes
the same name in curly brackets (in the lines, their alternates and the drafts). Nothing else
changes, the session is not marked as changed and there is no undo step for it; the next save
writes the curly form. A line that is only chords, such as `[Am]` or `[G/B]  [C]`, stays a line of
chords.

## Chords: `[Am]`

Write a chord in square brackets right before the syllable it falls on:

```
[Am]Hello dark[G]ness my old [C]friend
```

- **What counts as a chord** is what the Band and the Chords ruler read: a root `A` to `G` (a
  capital letter) with `#` or `b` (or ♯ ♭), a quality such as `m`, `7`, `maj7`, `m7b5`, `dim`,
  `sus4`, `add9`, `6`, `9`, and an optional slash bass (`G/B`).
- **In the editor** the text stays exactly what you type. Each chord's name is drawn above the
  syllable after it, in the chord colour, and its bracket text is dimmed (it lights up while the
  cursor is inside it). While the sheet has chords every row is a little taller to make room for
  them, so the line numbers and the margin columns stay lined up.
- **Something in brackets that is not a chord**, such as `[sic]`, is left as words. It is shown in a
  soft orange, and hovering it says why.
- **A line of only chords** (`[F]  [C]`) is an instrumental bar: it is not numbered or counted.
- **Chords are not words.** Syllable counts, stress, fit, rhyme letters and colours, the Words
  list and the Hook counter, rhyme suggestions, Find Words, Copy for AI, the Lyrics ruler's
  markers and the Teleprompter all read each line without its chords.
- **Teleprompter:** right-click it and turn on **Show chords** to see each line's chords above its
  words. It is off by default, so the singer sees just the words.

## The sheet and the Chords ruler

The Chords ruler stays what the Band plays. The sheet never changes it on its own, and it never
changes the sheet. Two actions move chords between them, for one line at a time. Right-click a
line in the Lyrics tab:

- **Send Chords to the Chords Ruler** lays the line's chords on the Chords ruler across the
  line's time on the Lyrics ruler. Each chord goes to the time of its syllable: the word times
  Transcribe Lyrics found, when the line has them, else spread evenly by the chord's place in the
  text. Each chord then snaps to the Chords ruler's grid (the nearest half bar) and lasts until
  the next chord or the end of the line. Two chords that land on the same half bar keep the
  first. One undo step.
- **Take Chords from the Chords Ruler** writes the ruler's chords under the line's time into the
  line, each at the start of the word nearest its time, replacing the line's own chords. One undo
  step.

Both need the line to be on the Lyrics ruler (drag its number there). Until it is, they are greyed
out, and hovering says why.

## Timing the words

The typed words can be timed to the vocal word by word, with no speech model. The words land on
the vocal's clips, in the same place Transcribe Lyrics puts what it hears, so they move with the
clips and everything that reads word times (LRC and MIDI export, Send Chords to the Chords Ruler)
uses them.

1. **Type the lyrics** in the Lyrics tab.
2. **Put the lines on the Lyrics ruler.** Drag each line's number there, or select a range and
   use Place lines on ruler…, or tap them: **Timing › Tap Line Starts…** (below).
3. **Timing › Time Words to Vocal** (also AI › Time Words to Vocal). Each placed line's words are
   spread over the notes sung inside it on the vocal track (the selected audio track, else the
   track marked Vocal). It uses each word's syllables (from the dictionary) and what the voice
   does: a word starts where the voice stops and starts again (a consonant or breath), where the
   level dips (a syllable sung again, or through an l, m, n or r), or, last choice, where the pitch
   moves. A word sung over many notes (a run) keeps them all until the next word's start. A word
   starts at the consonant just before its note and ends where its voice stops, never in a
   breath. Every word it places is a **guess**.
4. **Check them** in the Vocal Editor's word lane, under its ruler (or the strip under the track
   in the Edit window). Guesses are drawn dim with a dashed edge and a "?"; the lane says how many
   are still guessed. Words you tap or edit are drawn solid: they are **confirmed**. Right-click ›
   Mark as Checked confirms a guess that is right.

**Tap to time** (Timing › Tap Words…, or AI › Tap Words to Time…) is the dependable way. The
Lyrics tab shows the word to tap, large, with the next ones after it. Play the song (Play from
line starts two seconds before the line) and press **J** on each word; hold J for the word's
length to give it its end. **Backspace** takes the last tap back, **R** starts the line again,
**H** plays at half speed, **Esc** (or Done) stops. Each finished line is one undo step. Taps are
moved earlier by the latency setting (how late a tap lands after the sound) and then to the
nearest note start within the snap setting; snap 0 keeps the raw taps. **Line starts** mode takes
one tap per line: the lines are placed on the ruler and their words spread inside them.

**Fixing words by hand** (word lane or strip): drag a word's edge to move it; a neighbour whose
edge touches it follows, **Alt** moves only this edge. Edges snap to note starts and ends and to
breaths; **Ctrl** (Cmd on a Mac) turns the snap off. Drag the middle of a word to move it whole;
**Alt+←/→** nudges the selected word by 10 ms (Shift: 50 ms). Double-click retypes a word.
**Delete** removes the selected words. The right-click menu: Audition (plays just that word),
Retype…, Split (at the playhead, or in two), Merge with Next Word, Mark as Checked, Delete,
Insert Word at Playhead…, Select (the first or last word of every line, every occurrence of a
word, the guessed words, all, or Find…), and Time Words to Vocal. Clicking a word puts the
playhead at it and makes its time the edit selection; in the Vocal Editor its notes are selected
too. While the song plays, the word being sung lights up in the lanes and at the top of the
Lyrics tab.

**Where the words live.** On the clips of the track's playing playlist, in each clip's own audio
time: moving, trimming, splitting or duplicating a clip takes its words along. On a comped track
each comp piece keeps the words sung in it. A duplicated track has the same words at the same
times; a harmony or double track with its own audio is timed on its own (its notes differ).
Timing a line again replaces its guessed words but leaves a line with hand-timed words alone
(the command's `force` replaces them). **Transcribe Lyrics never silently replaces typed or
hand-timed words**: it stops and says so, unless "Replace words typed or timed by hand" is on in
its dialog.

**How good is the guess?** On synthetic singing with known word times (clear consonants, runs,
two words on one note, pickups, a held last word, a breath mid-line) word starts land within
about 5–10 ms (median) and ends within 15–20 ms. Where words are sung legato into each other
inside a run, it cannot know which note starts the next word, and a word can be off by half a
second. On real singing, especially long gospel runs, the accuracy is not known until checked by
ear: check every line, and tap the lines it gets wrong.

## Import and export

The **Sheet** menu at the top of the Lyrics tab:

- **Export as Text...**: the words, with headings, without chords.
- **Export as Text with Chords...**: each line's chords on a line of their own above its words,
  lined up for a monospaced font.
- **Export as ChordPro...** (`.cho`): `{title: }` from the session name, each heading as a section
  (`{start_of_verse: Verse 1}` … `{end_of_verse}`, and `{start_of_chorus}`, `{start_of_bridge}`;
  other headings such as Intro use ChordPro's named sections, `{start_of_intro: Intro}`), chords
  inline.
- **Import Text or ChordPro...** replaces the sheet, and Undo brings the old one back. ChordPro
  sections and labels (`{start_of_chorus}`, `{soc}`, `{c: Bridge}`) become headings. Metadata
  (`{title:}`, `{key:}` and the like) and `#` comment lines are left out. Old `[Verse 1]` headings
  become `{Verse 1}`. Chords written on a line above the words are put inline. Lines the sheet
  already has keep their place on the Lyrics ruler.

A track's transcribed lyrics export separately (File › Export › Lyrics (LRC)... and Lyrics as
Text..., and the MIDI file export): those exports never include chords.

For scripts and agents the commands are `lyrics.sheet_set`, `lyrics.sheet_get`,
`lyrics.chords_to_ruler`, `lyrics.chords_from_ruler`, `lyrics.export_sheet_text`,
`lyrics.export_sheet_chords`, `lyrics.export_chordpro` and `lyrics.import_sheet` (see
[control-protocol.md](control-protocol.md)).
