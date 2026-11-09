# guitar

A pitch monitor for tuning a guitar and singing: a native [gpui](https://gpui.rs) window
that listens to the default input (laptop mic or audio interface) through PipeWire.

- **Tune** — the nearest string of the chosen tuning, its target note, a needle in cents,
  which way to turn the peg, and a button per string that gets a ✓ once that string has
  held in tune for half a second. Below, the last 8 s of cents, so you can watch a string
  settle while you turn it. Tunings: Standard, Chromatic, Drop D, E♭ standard,
  D standard, Drop C, DADGAD, Open D, Open G.
- **Sing** — the nearest note, and your pitch over the last 10 s on one of two views:
  - **Staff** (default): a tenor's treble-8 staff (treble clef with the 8 below: it sounds an
    octave lower than written), with the key signature of the key you pick. Your pitch line
    runs across it, so you see vibrato and how sharp or flat you were, and each note you
    hold gets a small note head where it starts, with ledger lines when it needs them. Notes
    in the key sit on their line or space; the others, which would need a ♯ or ♭, sit
    between them. The staff widens to take in high or low notes. For baritone or bass parts,
    switch it to the **bass clef**, or to the plain **treble clef** for soprano and alto parts
    (the Treble 8 / Treble / Bass buttons, or C to go round them): written as they sound,
    G2 B2 D3 F3 A3 and E4 G4 B4 D5 F5, each with the key signature in its usual places.
  - **Roll**: a piano roll, one row per semitone; the row you're singing lights up and the
    view follows your range.

  Notes held within 10¢ are green, within 25¢ amber, further off red, judged on the pitch
  averaged over about one vibrato cycle, so even vibrato around a note counts as on it.
  Octave numbers are the sounding pitch (middle C is C4), as in the tuner.

  Below is a **fretboard** (or a piano, with the Guitar / Piano buttons or G), to find on the
  guitar what's on the sheet. It's the chosen tuning (standard for the chromatic tuner), high E
  on top as in tab, open strings to fret 12. The note you're singing or playing lights up,
  brightest and named where it's lowest on the neck (first position for a tenor's range), and
  faintly at its other places. **Click a line or space on the staff** to hear the key's note
  there, as written on the sheet with the key signature: the fretboard rings where to play it.

  Clicking a fret, a piano key or the staff plays the note: it becomes the target (shown above the note, ✕ or X to clear), the needle and cents then
  measure you against it ("sing higher ↑"), and a dashed line marks it on the staff or roll.
  The fret or key you're singing lights up. With speakers, the microphone hears the piano too, so its
  note shows up for a moment; headphones avoid that.

  The piano (it plays every note, whichever is shown) plays one note at a time: a new key
  fades the last one out, so notes don't ring on into each other. Chords come from piano
  mode, below.

  **Piano mode** (M, or the "Play with keys" button) plays it from the computer keyboard, like
  GarageBand: the home row A S D F G H J K L Ø Æ ' is the white keys from C3 up to G4, the row
  above (W E, T Y U, O P, ¨) the black keys, and Z / X move it an octave down or up. On the
  US layout the keys after L are ; ' \ and ]. Each key shows its letter for the layout in
  use. While it's on, letters only play notes, and the piano shows (with the letters on it)
  even if the fretboard was chosen; M or Esc turns it off. **Chords:** a key pressed while
  others are still held joins them and they ring together (A + D + G is C E G); a key
  pressed on its own cuts them, as before. All the chord's notes become targets: singing any
  of them counts, each gets a dashed line on the staff and a mark on the keys or fretboard,
  and the pill above the note names the chord ("Chord Am · A2 C3 E3", "C/E" for an
  inversion) when it's a triad or a seventh chord.

  **With headphones on**, two buttons help you sing by ear. **Drone** (D) keeps the piano's
  note (or chord) sounding softly after the strike, until the next note, X, or Drone again:
  against it you hear yourself beat when you're out of tune and lock in when you're on.
  **Hear me** (H) puts the microphone in your headphones as it comes in, a few milliseconds
  late, for closed headphones that shut your own voice out. It's never remembered between
  runs (with speakers on next time it would feed back), and it turns itself off, with a red
  note, if what it plays stays very loud for half a second: a sign it's feeding back. Both
  are off outside Sing.
- **Warm-up** — vocal warm-ups as a choir does them. Press **Start** (Enter): the piano plays
  a short pattern (five-note scale, arpeggio, octave leap or the full scale; click it or P
  for the next), then it's your turn to sing it back, note by note, on the staff. Each note
  waits until you hold it within 25 cents for 0.3 s (vibrato's fine: it goes by the median),
  with your pitch drawn through its column; then its head turns green (within 10 cents) or
  amber, and the next note is up. Then the same a semitone higher, written in its own key,
  up through your **Range** (C3 to G4 to start: the − + buttons move either end) and back
  down. R plays the pattern again; N skips a note you can't reach (its head turns red).
  The piano is quiet while you sing, so it can't be heard as your voice. Hear me works here
  too.
- **Circle** — the circle of fifths: major keys round the outside, a fifth apart, each with
  its sharps or flats, and their relative minors inside. The chosen key (shared with Sing) is
  lit with its six chords: I, IV and V outside, ii, iii and vi inside, with their numerals.
  Dots on the rim mark its seven notes, and the note you're playing or singing lights up
  there. Beside it: the key signature, the scale and the chords spelled out, and the note
  heard. Click a key (or its minor) to choose it.

## Run

```sh
nix run                        # from this directory
nix profile install .          # or install it: `guitar`, plus a launcher entry
```

The launcher entry (`share/applications/guitar.desktop`) is in `~/.nix-profile/share`; if
your launcher doesn't look there, symlink it into `~/.local/share/applications`.

## Keys

| Key | |
|---|---|
| Tab / Shift+Tab | next / previous of Tune, Sing, Warm-up, Circle |
| Enter | Warm-up: start / stop (or the Start / Stop button) |
| P / Shift+P | Warm-up: next / previous pattern (or click the pattern) |
| R | Warm-up: hear the pattern again |
| N | Warm-up: skip the note you're on |
| M | Sing: piano mode on / off (letters play the piano, see above) |
| V | Sing: staff or piano roll (or the Staff / Roll buttons) |
| C | Sing, Warm-up: treble-8, treble or bass clef (or the Treble 8 / Treble / Bass buttons) |
| G | Sing, Warm-up: fretboard or piano under it (or the Guitar / Piano buttons) |
| D | Sing: drone on / off: the piano's note rings on softly (or the Drone button) |
| H | Sing, Warm-up: hear me on / off: the microphone in your headphones (or the Hear me button) |
| K / Shift+K | Sing, Circle: next / previous key, round the circle of fifths (or click the key) |
| X, Backspace | clear the piano's target note |
| T / Shift+T | next / previous tuning (or click the tuning) |
| 1–6 | lock onto that string (6 = low E), again or 0 to go back to the nearest; or click a string |
| ↑ ↓ | A4 reference ±1 Hz (415–466), or the − + buttons |
| Scroll, [ ] | zoom the piano roll (12–48 semitones) |
| Space | pause / resume |
| Q, Esc | quit |

Mode, tuning, A4, the sing view, clef, key, instrument and zoom are kept in
`~/.local/state/guitar/settings.json`.

The level meter's tick marks how loud the input must be for a note to register (−58 dBFS).
If notes don't register, move closer or turn the input up (`wpctl set-volume
@DEFAULT_AUDIO_SOURCE@ 80%`); the header shows which input is being heard.

## How it reads pitch

Every 10 ms the newest 43 ms of input, low-passed at 1 kHz to drop hiss, goes through the
McLeod Pitch Method: the normalised square difference function, then the first of its
peaks that comes within 90% of the highest. That finds the period, so a low string whose
second harmonic is louder than its fundamental still reads at its fundamental. Frames
count as voiced when they're loud and periodic enough (with lower thresholds to carry on,
so a ringing string holds), and a 3-frame median removes the odd octave slip. On the
built-in test signals it reads plucked strings to within 2.5¢ right through their fade and
a voice with vibrato to a median of under 1¢.
