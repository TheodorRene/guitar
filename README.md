# guitar

A pitch monitor for tuning a guitar and singing: a native [gpui](https://gpui.rs) window
that listens to the default input (laptop mic or audio interface) through PipeWire.

- **Tune** — the nearest string of the chosen tuning, its target note, a needle in cents,
  which way to turn the peg, and a button per string that gets a ✓ once that string has
  held in tune for half a second. Below, the last 8 s of cents, so you can watch a string
  settle while you turn it. Tunings: Standard, Chromatic, Drop D, E♭ standard,
  D standard, Drop C, DADGAD, Open D, Open G.
- **Sing** — the nearest note and a scrolling piano roll of your pitch over the last 10 s.
  Notes held within 10¢ are green, within 25¢ amber, further off red, judged on the
  pitch averaged over about one vibrato cycle, so even vibrato around a note counts as on
  it. The row of the note you're singing lights up and the view follows your range.

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
| T / Shift+T | next / previous tuning (or click the tuning) |
| 1–6 | lock onto that string (6 = low E), again or 0 to go back to the nearest; or click a string |
| ↑ ↓ | A4 reference ±1 Hz (415–466), or the − + buttons |
| Scroll, [ ] | zoom the piano roll (12–48 semitones) |
| Space | pause / resume |
| Q, Esc | quit |

Mode, tuning, A4 and zoom are kept in `~/.local/state/guitar/settings.json`.

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
