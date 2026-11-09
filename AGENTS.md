# AGENTS.md

Notes for coding agents working on guitar. The user-facing overview is in `README.md`.
The sibling project `../viz` (a gpui music visualiser by the same user) is the reference
for gpui 0.2 patterns and the Nix setup; its `AGENTS.md` has more gpui gotchas.

## Working with the user

- They play guitar and sing; this is their tuner and singing pitch monitor. They sing tenor
  in a choir (male, sometimes more baritone): the staff is treble-8 and the piano runs F2–C5.
- **The staff's look, from their feedback (2026-10-05):** note heads are small markers
  (`head_radii`), not sheet-music size, which looked like big blobs; and no accidentals
  beside heads (clutter, and they weren't sure they need them): out-of-key notes sit between
  lines instead. `music::Spelled::shown` still says which accidental the sheet would write.
- **They practise choir parts with the guitar in hand (2026-10-23):** finding a note from
  the sheet on the neck was the hard part, hence the fretboard under the staff (the default,
  over the piano) and clicking the staff; the Circle tab is for seeing keys and their chords.
- **Warm-ups (2026-11-07, their ask: "voice exercises, like SingStar-ish"):** they chose
  choir warm-up patterns, written on the staff (not SingStar bars), waiting for each note
  rather than keeping tempo, the piano playing the pattern first and then quiet while they
  sing (call and response: no leak into the mic), ±25¢ held 0.3 s to count, C3–G4 by
  default, a tab of its own, and only coloured heads as feedback (no scores or summary).
- **Iterate in small steps:** build one feature, verify it, then stop and report. Their
  ideas shift once they see things running.
- **After every change:** run the tests, then `git add -A && nix profile upgrade guitar`, so
  the installed app (launcher and `guitar` on the PATH) is current.
- **Don't disturb them:** never open a window on their screen unasked (use the headless
  runner below), and never play sound: the piano is silent whenever a test signal runs.
  Reading the microphone briefly for a check is fine if you say so.
- **Hardware and setup:** Ubuntu 26.04, sway on Wayland, PipeWire, Intel Arc B390 (Panther
  Lake), built-in "sof-soundwire Microphones" as the default input.

## Build and test

```sh
nix develop -c cargo build --release
nix develop -c cargo test --release
nix develop -c cargo clippy --release --all-targets
git add -A && nix build                    # flakes only see tracked files; runs the tests too
```

- **Screenshots without the user's screen or mic:** `nix develop -c tools/gui-headless.sh
  -m sing -g voice -r 1200x800 -s 8` runs the app in its own headless sway on a synthetic
  signal and saves `shot.png` (the path is printed). `-g guitar` plucks each string of
  standard tuning, some detuned; `-g quiet` is only hiss (for the piano); `-g mic` uses the
  real default input. `-e '"key": "E♭"'` etc. sets settings; `-p '2:57 3:m 3.5:h'` presses
  piano keys (seconds:MIDI) and keys (seconds:gpui key name, through `handle_key`; `^name`
  lets go, through `handle_key_up`, e.g. `-p '2:m 2.5:a 2.52:d 2.55:g 3:^a 3:^d 3:^g'` for a
  C chord), as `GUITAR_TEST_PRESS`, because the headless sway has no pointer or keyboard.
  For the packaged build: `env -u LD_LIBRARY_PATH -u VK_DRIVER_FILES tools/gui-headless.sh …
  result/bin/guitar`.
- **The synthetic signals** (`GUITAR_TEST_SIGNAL=guitar|voice|warmup|quiet`, `audio::Synth`)
  are silent and identical every run, with light hiss. The voice sings "Twinkle" in C, then
  a tenor line down to C3 (a ledger line) and up through F♯3. `warmup` sings the five-note
  warm-up in step with a session started at 1 s: `-m warmup -g warmup -p '1:enter' -s 9.6`
  catches the first time through mid-way, `-s 21.3` the second (D♭ major). With a test
  signal the piano's voices are mixed into the made-up input, as if the mic heard them. The
  analysis tests in `track.rs` push the signals through the real capture → filter →
  detector → voicer path and check the readings against `Synth::truth`; `main.rs` checks
  the voice's held notes come out as its tune; `warmup.rs` runs the warm-up singer, piano
  mixed in, through a `Session` and checks each note is heard in its tuning.

## Architecture

One binary crate.

| Module | Role |
|---|---|
| `audio.rs` | PipeWire capture of the default source, downmixed to mono; two rings: raw (level) and low-passed (detection); `Synth`, the test signals (`warmup`: a singer doing the five-note warm-up every 13 s, some notes off by `WARMUP_CENTS`) |
| `pitch.rs` | `Lowpass` (1 kHz biquad prefilter) and `Detector` (McLeod Pitch Method, FFT autocorrelation, parabolic peak) |
| `warmup.rs` | warm-ups: `PATTERNS` (semitones from the root), `roots` (a semitone up each time through the range, then down), `key_of` (each time written in its root's major key); `Session::step` on the frame clock: `Playing` (asks for `Action::Play` every `NOTE_SECS`, then `Stop`), `Listening` (from `SETTLE` after the stop, so the piano's tail isn't heard as singing; `hit`: the last `HOLD` s voiced within `WANDER`¢, median within `TOLERANCE`¢), `Resting`, `Finished`; `replay`, `skip` |
| `track.rs` | analysis thread: `Analyser` takes a window every 10 ms (`take` under the capture's lock, `analyse` after), `Voicer` gates on clarity/level with hysteresis and takes a 3-frame median; 20 s of `Frame`s in `Track` |
| `music.rs` | MIDI/Hz/cents, note names (sharps, or flats for E♭ tuning), `TUNINGS`, `Target` (locked string, nearest string, or nearest note); staff notation: `KEYS`, `spell` (a note in a key, with the accidental to write), steps (one per line or space, C0 = 0, sounding pitch), `staff_position` (in-key notes exactly on their step, others in proportion), `Clef` (treble-8, treble or bass: `lines`, the clef's `line`), `ledger_lines`, key signatures (treble-8's, an octave up for treble, down for bass), `note_at_step` (the key's note on a line or space, for staff clicks); `fret_positions` (lowest fret first); the circle: `fifths_position`, `Key::scale`, `Key::accidentals`, `DEGREES`; `chord_name` (triads and sevenths, any voicing) |
| `piano.rs` | reference notes: `Voice` (decaying partials), `render`, `Piano::play` (cuts what's ringing) and `Piano::add` (a chord); with `set_hold` notes are `held`, a drone (partials settle to `SUSTAIN`, exactly harmonic) until cut; `Ear` mixes the raw capture into the output a buffer (+32 samples) behind, with a howl guard (`EarSwitch::howled`); a PipeWire playback stream (128-sample buffers, for the ear) started on the first press or the ear, paused after 3 s quiet with the ear off (loop timer), woken by a `pw::channel` |
| `glyphs.rs` | G and F clefs, ♯, ♭, ♮ outlines from Noto Music, generated by `tools/glyphs.py` (don't edit) |
| `settings.rs` | mode, tuning, A4, piano-roll span, sing view (staff or roll), clef, key, instrument (fretboard or piano under the staff), drone (the ear is never saved), warm-up pattern and range (`warmup_range` keeps it on the piano and an octave wide); `$XDG_STATE_HOME/guitar/settings.json` |
| `source.rs` | default input's name from `wpctl inspect @DEFAULT_AUDIO_SOURCE@`, every 3 s |
| `main.rs` | gpui `Monitor` view: `update` copies frames and eases the needle, piano-roll centre and staff range; Tune (note, gauge, strings, cents strip), Sing (note, gauge, staff or piano roll, fretboard or keyboard), Warm-up (`follow_warmup` steps the `Session` and carries out its actions through `press`/`Piano::stop`; `warmup_staff` lays the pattern out a column a note, sharing `paint_staff_frame`, `head_ledgers` and `clef_eight` with the sing staff; `staff_key` is the pattern's key there) and Circle (circle of fifths: rings as an image, labels laid out from `CircleGeom` and last frame's `circle_bounds`; key panel) views. The fretboard spaces frets as on a guitar (`fret_x`); its cells and the staff (`StaffGeom::step_at`) are clickable and `press` the note. Keys go through `handle_key`; piano mode (`typing`) maps `TYPING_KEYS` GarageBand-style from `typing_base` (C3, Z/X ±1 octave) and swallows every other letter so no shortcut fires mid-tune. `Sung` is the sing views' data per frame: points, smoothed pitch, colours and `held_notes` (runs nearest one semitone ≥ 120 ms). The staff lays out from `StaffGeom` and paints glyphs with `paint_glyph`; canvases paint quads and paths, text is always divs |

### Conventions

- **Colours:** the Coral palette as in viz: `OK` green, `WARN` amber, `DANGER` red on
  `#0d0d0d`. `Class::{On, Near, Off}` maps cents to them.
- **Fonts:** Noto Sans for text, JetBrainsMono Nerd Font for note names, numbers and
  symbols (♯ ♭ ¢ ✓ ↑ ↓ −; Noto Sans lacks some).
- **Time:** frames are timed by the input's sample clock (`Frame::t`); the views scroll on
  the wall clock between frames (`Monitor::now`).
- **Redraws:** the window animates while any voiced frame is on screen; otherwise a 50 ms
  background tick redraws (level meter, new sound).

## Gotchas

- **Nix and the GPU:** gpui `dlopen`s vulkan-loader, wayland and libxkbcommon; they're
  added to the rpath in `postFixup`. `VK_DRIVER_FILES` points at nixpkgs' Mesa (the host's
  drivers use another libc). The dev shell sets both through env vars.
- **gpui 0.2 (crates.io):** built with `default-features = false, features = ["wayland"]`.
  Never shape text in a canvas (it made other text flicker in viz); overlay divs instead,
  e.g. the piano-roll labels use `top(relative(..))`. Many overlapping quads go in a
  `paint_layer`.
- **Pitch tuning numbers:** the 1 kHz prefilter cut tail jitter on a fading low string from
  3.6 to 0.5¢ RMS; a 4096-sample window and stricter hold thresholds were tried and were
  worse. The median's one-frame lag is why the voice test compares against `t - 10 ms`.
- **Launcher entry:** the package installs `share/applications/guitar.desktop` (Exec is the
  store path, substituted at build) and `share/icons/hicolor/scalable/apps/guitar.svg`. Sway
  (started by GDM) doesn't search `~/.nix-profile/share`, so both are symlinked into
  `~/.local/share/applications` and `~/.local/share/icons/…` (done 2026-10-02, as for viz).
  The links go through `~/.nix-profile`, so upgrades keep them valid. The window's app id
  `guitar` matches `StartupWMClass`.
- **The headless runner** must never `wait` on the sway it started (it only exits via the
  trap). Stop stray test sways by their config path, never `pkill sway`.
- **Keys on the user's Norwegian layout** (they switch between "Norwegian (no dead keys)" and
  "English (US)"): gpui names a key by its ASCII letter, or where it sits on a US keyboard
  when it types none, so Ø, Æ, ¨ arrive as `;`, `'`, `]` (`guess_ascii` in its Linux
  keystroke code). The key after Æ types `'` itself, so `typed_offset` tells it from Æ by
  the typed character (`key_char`) and the layout name (`cx.keyboard_layout()`); the piano
  labels follow the layout too. `is_held` marks auto-repeat.
- **The piano is one note at a time** (their call: overlapping notes clashed): `Piano::play`
  cuts the ringing note with an 80 ms fade. **Except chords (2026-11-03, their ask):** in
  piano mode a key pressed while other note keys are held (`held_keys`, from key-up events)
  goes through `Piano::add` and joins `targets`; the sing view measures against the nearest
  target, and `music::chord_name` names it. The mouse still plays one note.
- **"The window flickers when the piano plays" (2026-10-15)** was the readout reacting: the
  mic hears the piano (speakers, or their open-back headphones), and for ~0.13 s after a key
  the analysis still hears the last note, which against the new target flashed red ("−219¢
  sing higher", red key). `PRESS_SETTLE` holds the readout, needle and lit key for 250 ms.
  Found with a burst of headless screenshots (grim every ~45 ms) laid side by side; that's
  the way to check anything that changes too fast for one screenshot.
- **Big filled paths make text flicker on the real GPU (2026-10-23):** the circle of fifths
  first drew its 24 wedges with `paint_path`, and the user saw it (and its small labels)
  flicker; the headless sway doesn't show it, so screenshots can't catch it. As in viz, the
  fix is no big paths: the rings are a `RenderImage` (`circle_pixels`, rebuilt only when the
  key or size changes, the old one freed with `drop_image`), the dots are quads.
- **Fonts without Latin letters can't be used:** gpui drops any font with no glyph for
  `m` (`load_family` in its Linux text system), so `font_family("Noto Music")` silently
  falls back and the 𝄞 shows as a box. Hence `glyphs.rs`: outlines as paths, scaled so a
  staff space is 244 font units (Noto Music's U+1D11A). Its accidentals are big and its
  sharp wide for a staff; `accidental_scale` narrows them. Its F clef (U+1D122) sits lower
  than its staff glyph, so its anchor is the F line midway between its dots (644.5).
- **No pointer in the headless sway:** `swaymsg seat … cursor` succeeds but the app never gets
  a pointer, and `wlrctl` makes a new virtual pointer per call, so clicks don't land either.
  Script presses with `-p` instead.
- **Drone and Hear me (2026-11-07, their ask: "feedback while singing, in my headphones"):**
  the drone leaks into the mic through their open-back headphones. The track test shows
  singing still reads true with the drone 18 dB under the piano's own level (at −6 dB the
  drone wins), but in rests a drone leaking above the −58 dBFS gate reads as sung, exactly
  on target (in the test, even 40 dB down). Untested with their real leak; if it shows, the next step is gating
  on what the piano is playing. The test signals mix the drone in at full level, so
  headless screenshots with a drone show garbled readings; that's the test, not the app.
- **The piano's real output is untested by agents:** a private PipeWire (no devices, a null
  sink) runs, but without a session manager streams get no ports to link, and WirePlumber
  would take over the real sound card. The user hears it first.
