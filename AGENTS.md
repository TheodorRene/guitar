# AGENTS.md

Notes for coding agents working on guitar. The user-facing overview is in `README.md`.
The sibling project `../viz` (a gpui music visualiser by the same user) is the reference
for gpui 0.2 patterns and the Nix setup; its `AGENTS.md` has more gpui gotchas.

## Working with the user

- They play guitar and sing; this is their tuner and singing pitch monitor.
- **Iterate in small steps:** build one feature, verify it, then stop and report. Their
  ideas shift once they see things running.
- **After every change:** run the tests, then `git add -A && nix profile upgrade guitar`, so
  the installed app (launcher and `guitar` on the PATH) is current.
- **Don't disturb them:** never open a window on their screen unasked (use the headless
  runner below), and never play sound. Reading the microphone briefly for a check is fine
  if you say so.
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
  standard tuning, some detuned; `-g mic` uses the real default input. For the packaged
  build: `env -u LD_LIBRARY_PATH -u VK_DRIVER_FILES tools/gui-headless.sh … result/bin/guitar`.
- **The synthetic signals** (`GUITAR_TEST_SIGNAL=guitar|voice`, `audio::Synth`) are silent
  and identical every run, with light hiss. The analysis tests in `track.rs` push them
  through the real capture → filter → detector → voicer path and check the readings
  against `Synth::truth`.

## Architecture

One binary crate.

| Module | Role |
|---|---|
| `audio.rs` | PipeWire capture of the default source, downmixed to mono; two rings: raw (level) and low-passed (detection); `Synth`, the test signals |
| `pitch.rs` | `Lowpass` (1 kHz biquad prefilter) and `Detector` (McLeod Pitch Method, FFT autocorrelation, parabolic peak) |
| `track.rs` | analysis thread: `Analyser` takes a window every 10 ms (`take` under the capture's lock, `analyse` after), `Voicer` gates on clarity/level with hysteresis and takes a 3-frame median; 20 s of `Frame`s in `Track` |
| `music.rs` | MIDI/Hz/cents, note names (sharps, or flats for E♭ tuning), `TUNINGS`, `Target` (locked string, nearest string, or nearest note) |
| `settings.rs` | mode, tuning, A4, piano-roll span; `$XDG_STATE_HOME/guitar/settings.json` |
| `source.rs` | default input's name from `wpctl inspect @DEFAULT_AUDIO_SOURCE@`, every 3 s |
| `main.rs` | gpui `Monitor` view: `update` copies frames and eases the needle and piano-roll centre; Tune (note, gauge, strings, cents strip) and Sing (note, gauge, piano roll) views; canvases paint quads and stroked paths, text is always divs |

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
