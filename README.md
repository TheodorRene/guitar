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
    between them. The staff widens to take in high or low notes.
  - **Roll**: a piano roll, one row per semitone; the row you're singing lights up and the
    view follows your range.

  Notes held within 10¢ are green, within 25¢ amber, further off red, judged on the pitch
  averaged over about one vibrato cycle, so even vibrato around a note counts as on it.
  Octave numbers are the sounding pitch (middle C is C4), as in the tuner.

  Below is a **piano** from a baritone's low F2 to a tenor's high C5. Click a key to hear it:
  it becomes the target (shown above the note, ✕ or X to clear), the needle and cents then
  measure you against it ("sing higher ↑"), and a dashed line marks it on the staff or roll.
  The key you're singing lights up. With speakers, the microphone hears the piano too, so its
  note shows up for a moment; headphones avoid that.

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
| Tab | switch Tune / Sing |
| V | Sing: staff or piano roll (or the Staff / Roll buttons) |
| K / Shift+K | Sing: next / previous key, round the circle of fifths (or click the key) |
| X, Backspace | clear the piano's target note |
| T / Shift+T | next / previous tuning (or click the tuning) |
| 1–6 | lock onto that string (6 = low E), again or 0 to go back to the nearest; or click a string |
| ↑ ↓ | A4 reference ±1 Hz (415–466), or the − + buttons |
| Scroll, [ ] | zoom the piano roll (12–48 semitones) |
| Space | pause / resume |
| Q, Esc | quit |

Mode, tuning, A4, the sing view, key and zoom are kept in
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
