//! guitar: a pitch monitor for tuning a guitar and singing, built with gpui.
//!
//! Tune: the nearest string of the chosen tuning (or the nearest note, chromatic), a needle
//! in cents, the strings with a tick once each is in tune, and the last seconds of cents.
//! Sing: the nearest note (or the one played on the piano) and the pitch scrolling over a
//! tenor's treble-8 staff, with a note head per held note, or over a piano roll; coloured by
//! how close each held note is to the note it's nearest. A keyboard below plays reference
//! notes to sing against.

mod audio;
mod glyphs;
mod music;
mod piano;
mod pitch;
mod settings;
mod source;
mod track;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, fill, point, prelude::*, px, relative, rgb, size, App, Application, Bounds, ClickEvent, ContentMask,
    Context, FocusHandle, FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, Path, PathBuilder, Pixels, Point,
    ScrollWheelEvent, Size, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use music::{Accidental, Key, Target, Tuning, KEYS, TUNINGS};
use piano::Piano;
use settings::{Mode, Settings, SingView};
use track::Frame;

const TEXT_FONT: &str = "Noto Sans";
/// Numbers, note names and symbols (♯ ♭ ¢ ✓ ↑ ↓); monospaced digits don't jitter.
const MONO_FONT: &str = "JetBrainsMono Nerd Font";

// From the Coral terminal palette, as in viz.
const BG: u32 = 0x0d0d0d;
const OK: u32 = 0x8faf7a;
const WARN: u32 = 0xd9b08c;
const DANGER: u32 = 0xe0645a;

/// Seconds of pitch shown by the sing view's piano roll and the tuner's strip.
const SING_SECS: f64 = 10.0;
const STRIP_SECS: f64 = 8.0;
/// The needle eases towards each reading with this time constant (s).
const NEEDLE_TAU: f32 = 0.08;
/// A string within this many cents is in tune (green), within `TUNE_NEAR` close (amber).
const TUNE_ZONE: f32 = 3.0;
const TUNE_NEAR: f32 = 10.0;
/// A string gets its tick after this long in tune, and loses it when played this far off.
const TUNED_HOLD: f64 = 0.5;
const UNTUNED: f32 = 8.0;
/// Sung notes held within this many cents are on pitch, within `SING_NEAR` close.
const SING_ZONE: f32 = 10.0;
const SING_NEAR: f32 = 25.0;
/// Frames either side averaged when colouring a sung note, about one vibrato cycle in all,
/// so a note with even vibrato around the pitch still counts as on it.
const SING_SMOOTH: usize = 9;
/// The gauge spans ±this many cents.
const GAUGE_CENTS: f32 = 50.0;
/// A reading older than this (s of input) is no longer live.
const LIVE: f64 = 0.1;
/// The staff's steps (lines and spaces) shown at least, D2 to G5: the treble-8 staff with
/// room for a baritone's lows and a tenor's highs on ledger lines.
const STAFF_STEPS: (f32, f32) = (15.0, 39.0);
/// A sung note gets a note head once held this long (s); pieces of one note this close
/// together are one note.
const MIN_NOTE: f64 = 0.12;
const NOTE_JOIN: f64 = 0.08;
/// A sung note keeps its name until the smoothed pitch strays this far from it (semitones),
/// so a voice hovering near halfway between two notes (G a little under 50 cents flat,
/// say) stays one note instead of flickering between G and F♯.
const NOTE_STICK: f32 = 0.75;
/// How long a piano key looks pressed after a click.
const PRESS: Duration = Duration::from_millis(160);
/// How often the window redraws while nothing moves: keeps the level meter going and
/// notices new sound.
const IDLE_TICK: Duration = Duration::from_millis(50);

fn white(a: f32) -> Hsla {
    Hsla { h: 0.0, s: 0.0, l: 1.0, a }
}

fn hex(c: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(c).into();
    h.a = a;
    h
}

/// How close a pitch is: on it, near it, or off.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    On,
    Near,
    Off,
}

impl Class {
    fn of(cents: f32, zone: f32, near: f32) -> Class {
        match cents.abs() {
            c if c <= zone => Class::On,
            c if c <= near => Class::Near,
            _ => Class::Off,
        }
    }

    fn color(self) -> Hsla {
        hex(
            match self {
                Class::On => OK,
                Class::Near => WARN,
                Class::Off => DANGER,
            },
            1.0,
        )
    }
}

fn fmt_cents(c: f32) -> String {
    match c.round() as i32 {
        0 => "0¢".into(),
        r if r > 0 => format!("+{r}¢"),
        r => format!("−{}¢", -r),
    }
}

fn ordinal(n: usize) -> String {
    match n {
        1 => "1st".into(),
        2 => "2nd".into(),
        3 => "3rd".into(),
        n => format!("{n}th"),
    }
}

#[derive(Clone, Copy)]
struct Reading {
    hz: f32,
    target: Target,
}

struct Monitor {
    track: track::Shared,
    /// The default input's name; none while a test signal stands in for it.
    input: Option<source::Shared>,
    test_signal: Option<String>,
    settings: Settings,
    focus: FocusHandle,
    /// The frames the views can show, copied from the track each frame.
    frames: Vec<Frame>,
    /// Input time at the right edge of the views.
    now: f64,
    level_db: f32,
    received: bool,
    error: Option<String>,
    started: Instant,
    last_render: Instant,
    /// The latest reading; kept (dimmed) after the sound stops.
    reading: Option<Reading>,
    /// Whether `reading` is from sound coming in right now.
    live: bool,
    /// Cents from the target, eased.
    needle: Option<f32>,
    /// Sing view: the note at the piano roll's vertical centre, eased to follow the voice.
    center: f32,
    /// Paused at this input time.
    frozen: Option<f64>,
    /// The string being tuned, when chosen by hand rather than the nearest.
    locked: Option<usize>,
    /// Per string: since when it has been in tune, and whether it got its tick.
    in_tune_since: [Option<f64>; 6],
    tuned: [bool; 6],
    /// Whether the last frame asked for another; when not, the idle tick redraws.
    animating: bool,
    piano: Piano,
    /// The note last played on the piano: the sing view measures against it until cleared.
    target: Option<i32>,
    pressed: Option<(i32, Instant)>,
    /// The staff's steps shown (lo, hi), eased to take in what's been sung.
    staff_range: (f32, f32),
    /// Where the staff was laid out last frame: its glyphs are placed from it.
    staff_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Piano keys to press by themselves (s after start, MIDI), from `GUITAR_TEST_PRESS`,
    /// for tests: the headless test sway has no pointer to click with.
    scripted: Vec<(f64, i32)>,
}

impl Monitor {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(IDLE_TICK).await;
            let alive = this.update(cx, |m, cx| {
                if !m.animating {
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();

        let test_signal = audio::test_signal();
        let voices = piano::Voices::default();
        let mut scripted: Vec<(f64, i32)> =
            std::env::var("GUITAR_TEST_PRESS").unwrap_or_default().split_whitespace().filter_map(parse_press).collect();
        scripted.sort_by(|a, b| b.0.total_cmp(&a.0));
        Self {
            scripted,
            track: track::start(audio::start(voices.clone())),
            piano: Piano::new(voices, test_signal.is_none()),
            target: None,
            pressed: None,
            staff_range: STAFF_STEPS,
            staff_bounds: Rc::default(),
            input: test_signal.is_none().then(source::start),
            test_signal,
            settings: Settings::load(),
            focus: cx.focus_handle(),
            frames: Vec::new(),
            now: 0.0,
            level_db: -120.0,
            received: false,
            error: None,
            started: Instant::now(),
            last_render: Instant::now(),
            reading: None,
            live: false,
            needle: None,
            center: 60.0,
            frozen: None,
            locked: None,
            in_tune_since: [None; 6],
            tuned: [false; 6],
            animating: true,
        }
    }

    fn tuning(&self) -> &'static Tuning {
        &TUNINGS[music::tuning_index(&self.settings.tuning)]
    }

    fn flats(&self) -> bool {
        self.settings.mode == Mode::Tune && self.tuning().flats
    }

    fn key(&self) -> &'static Key {
        &KEYS[music::key_index(&self.settings.key)]
    }

    fn target_for(&self, midi: f32) -> Target {
        match self.settings.mode {
            Mode::Tune => Target::for_reading(midi, self.tuning(), self.locked),
            Mode::Sing => Target { midi: self.target.unwrap_or(midi.round() as i32), string: None },
        }
    }

    fn midi(&self, hz: f32) -> f32 {
        music::midi_of(hz, self.settings.a4)
    }

    /// Takes in what the analysis has found since the last frame.
    fn update(&mut self) {
        while self.scripted.last().is_some_and(|&(at, _)| self.started.elapsed().as_secs_f64() >= at) {
            let (_, midi) = self.scripted.pop().expect("checked");
            self.press(midi);
        }
        let wall = Instant::now();
        let dt = wall.duration_since(self.last_render).as_secs_f32().min(0.1);
        self.last_render = wall;
        {
            let track = self.track.lock().unwrap();
            self.received = track.received;
            self.error.clone_from(&track.error);
            self.level_db = track.frames.back().map_or(-120.0, |f| f.level_db);
            if self.frozen.is_none() {
                // Frames come every 10 ms; scroll on the wall clock in between.
                self.now = match (track.frames.back(), track.newest_at) {
                    (Some(f), Some(at)) => f.t + wall.duration_since(at).as_secs_f64().min(0.05),
                    _ => 0.0,
                };
                let from = self.now - SING_SECS.max(STRIP_SECS) - 0.5;
                let start = track.frames.partition_point(|f| f.t < from);
                self.frames.clear();
                self.frames.extend(track.frames.range(start..).copied());
            }
        }

        let latest = match self.frozen {
            Some(_) => None,
            None => self.frames.last().filter(|f| f.t > self.now - LIVE).and_then(|f| f.hz),
        };
        self.live = latest.is_some();
        if let Some(hz) = latest {
            let target = self.target_for(self.midi(hz));
            let cents = target.cents(self.midi(hz));
            let same = self.reading.is_some_and(|r| r.target == target);
            self.needle = Some(match self.needle {
                Some(n) if same => n + (cents - n) * (1.0 - (-dt / NEEDLE_TAU).exp()),
                _ => cents,
            });
            self.reading = Some(Reading { hz, target });
        }
        self.follow_tuning();
        if self.settings.mode == Mode::Sing && self.frozen.is_none() {
            self.follow_voice(dt);
            self.follow_staff(dt);
        }
        self.animating = self.frozen.is_none() && self.frames.iter().any(|f| f.hz.is_some());
    }

    /// Ticks a string once it has stayed in tune for a moment; takes it away if it drifts.
    fn follow_tuning(&mut self) {
        let playing = self.reading.filter(|_| self.live && self.settings.mode == Mode::Tune).and_then(|r| r.target.string);
        for s in 0..self.in_tune_since.len() {
            if playing != Some(s) {
                self.in_tune_since[s] = None;
            }
        }
        let (Some(s), Some(needle)) = (playing, self.needle) else { return };
        if needle.abs() <= TUNE_ZONE {
            let since = *self.in_tune_since[s].get_or_insert(self.now);
            if self.now - since >= TUNED_HOLD {
                self.tuned[s] = true;
            }
        } else {
            self.in_tune_since[s] = None;
            if needle.abs() > UNTUNED {
                self.tuned[s] = false;
            }
        }
    }

    /// Eases the piano roll to keep what's been sung on screen: centred on the range sung
    /// lately, or on the latest note when that range is too wide to fit.
    fn follow_voice(&mut self, dt: f32) {
        let (mut lo, mut hi, mut newest) = (f32::MAX, f32::MIN, None);
        for f in self.frames.iter().filter(|f| f.t >= self.now - SING_SECS) {
            if let Some(hz) = f.hz {
                let m = self.midi(hz);
                (lo, hi, newest) = (lo.min(m), hi.max(m), Some(m));
            }
        }
        let Some(newest) = newest else { return };
        let span = self.settings.span;
        let want = if hi - lo <= span - 3.0 { (lo + hi) / 2.0 } else { newest };
        self.center += (want - self.center) * (1.0 - (-dt / 0.6).exp());
    }

    /// Widens the staff to take in what's been sung lately (and the piano's note), easing
    /// back to the usual range once it's gone.
    fn follow_staff(&mut self, dt: f32) {
        let key = self.key();
        let (mut lo, mut hi) = STAFF_STEPS;
        let sung = self.frames.iter().filter(|f| f.t >= self.now - SING_SECS).filter_map(|f| f.hz);
        for step in sung.map(|hz| music::staff_position(self.midi(hz), key)).chain(self.target.map(|m| music::staff_position(m as f32, key))) {
            (lo, hi) = (lo.min(step - 2.5), hi.max(step + 2.5));
        }
        let ease = 1.0 - (-dt / 0.4).exp();
        self.staff_range.0 += (lo - self.staff_range.0) * ease;
        self.staff_range.1 += (hi - self.staff_range.1) * ease;
    }

    fn reset_ticks(&mut self) {
        self.tuned = [false; 6];
        self.in_tune_since = [None; 6];
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.settings.mode != mode {
            self.settings.mode = mode;
            self.needle = None;
            self.reading = None;
            self.settings.save();
        }
    }

    fn next_tuning(&mut self, step: isize) {
        let i = music::tuning_index(&self.settings.tuning) as isize;
        let next = (i + step).rem_euclid(TUNINGS.len() as isize) as usize;
        self.settings.tuning = TUNINGS[next].name.into();
        self.locked = None;
        self.needle = None;
        self.reading = None;
        self.reset_ticks();
        self.settings.save();
    }

    fn nudge_a4(&mut self, by: f32) {
        let a4 = (self.settings.a4 + by).clamp(*music::A4_RANGE.start(), *music::A4_RANGE.end());
        if a4 != self.settings.a4 {
            self.settings.a4 = a4;
            self.reset_ticks();
            self.settings.save();
        }
    }

    /// Locks onto a string, or back to the nearest string if it was already locked.
    fn toggle_lock(&mut self, string: usize) {
        if string < self.tuning().strings.len() {
            self.locked = if self.locked == Some(string) { None } else { Some(string) };
            self.needle = None;
        }
    }

    fn next_key(&mut self, step: isize) {
        let i = music::key_index(&self.settings.key) as isize;
        self.settings.key = KEYS[(i + step).rem_euclid(KEYS.len() as isize) as usize].name.into();
        self.settings.save();
    }

    fn toggle_sing_view(&mut self) {
        self.settings.sing_view = match self.settings.sing_view {
            SingView::Staff => SingView::Roll,
            SingView::Roll => SingView::Staff,
        };
        self.settings.save();
    }

    /// Plays a piano key and makes it the note to sing against.
    fn press(&mut self, midi: i32) {
        self.target = Some(midi);
        self.pressed = Some((midi, Instant::now()));
        self.needle = None;
        self.piano.play(music::hz_of(midi, self.settings.a4));
    }

    fn clear_target(&mut self) {
        self.target = None;
        self.needle = None;
    }

    fn toggle_freeze(&mut self) {
        self.frozen = match self.frozen {
            Some(_) => None,
            None => Some(self.now),
        };
    }

    fn zoom(&mut self, factor: f32) {
        let span = (self.settings.span * factor).clamp(*settings::SPAN_RANGE.start(), *settings::SPAN_RANGE.end());
        if span != self.settings.span {
            self.settings.span = span;
            self.settings.save();
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let shift = ev.keystroke.modifiers.shift;
        let key = ev.keystroke.key.as_str();
        match key {
            "tab" => self.set_mode(if self.settings.mode == Mode::Tune { Mode::Sing } else { Mode::Tune }),
            "t" => self.next_tuning(if shift { -1 } else { 1 }),
            "up" | "=" | "+" => self.nudge_a4(1.0),
            "down" | "-" => self.nudge_a4(-1.0),
            "0" | "a" => self.locked = None,
            "1" | "2" | "3" | "4" | "5" | "6" if self.settings.mode == Mode::Tune => {
                // Strings are numbered from the thinnest; ours are stored lowest first.
                let number: usize = key.parse().expect("digit");
                if let Some(i) = self.tuning().strings.len().checked_sub(number) {
                    self.toggle_lock(i);
                }
            }
            "v" if self.settings.mode == Mode::Sing => self.toggle_sing_view(),
            "k" if self.settings.mode == Mode::Sing => self.next_key(if shift { -1 } else { 1 }),
            "x" | "backspace" => self.clear_target(),
            "[" => self.zoom(1.15),
            "]" => self.zoom(1.0 / 1.15),
            "space" => self.toggle_freeze(),
            "q" | "escape" => cx.quit(),
            _ => return,
        }
        cx.notify();
    }

    fn header(&self, cx: &Context<Self>) -> impl IntoElement {
        let mode = self.settings.mode;
        let tab = |id: &'static str, label: &'static str, m: Mode| {
            div()
                .id(id)
                .px(px(16.))
                .py(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .when(mode == m, |d| d.bg(white(0.12)).text_color(white(0.95)))
                .when(mode != m, |d| d.text_color(white(0.5)).hover(|s| s.bg(white(0.05))))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.set_mode(m);
                    cx.notify();
                }))
        };
        let tabs = div()
            .flex()
            .gap(px(4.))
            .p(px(3.))
            .rounded(px(10.))
            .bg(white(0.04))
            .child(tab("tab-tune", "Tune", Mode::Tune))
            .child(tab("tab-sing", "Sing", Mode::Sing));

        let tuning = self.tuning();
        let notes: Vec<&str> = tuning.strings.iter().map(|&m| music::name(m, tuning.flats)).collect();
        let tuning_pill = div()
            .id("tuning")
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(white(0.05))
            .hover(|s| s.bg(white(0.09)))
            .cursor_pointer()
            .text_size(px(14.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tuning.name))
            .when(!notes.is_empty(), |d| d.child(div().font_family(MONO_FONT).text_color(white(0.45)).child(notes.join(" "))))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.next_tuning(1);
                cx.notify();
            }));

        let step = |id: &'static str, label: &'static str, by: f32| {
            div()
                .id(id)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(white(0.05))
                .hover(|s| s.bg(white(0.12)))
                .cursor_pointer()
                .font_family(MONO_FONT)
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.nudge_a4(by);
                    cx.notify();
                }))
        };
        let a4 = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(13.))
            .child(div().text_color(white(0.45)).child("A4"))
            .child(step("a4-down", "−", -1.0))
            .child(div().w(px(64.)).flex().justify_center().font_family(MONO_FONT).child(format!("{:.0} Hz", self.settings.a4)))
            .child(step("a4-up", "+", 1.0));

        let view = self.settings.sing_view;
        let view_tab = |id: &'static str, label: &'static str, v: SingView| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(5.))
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(13.))
                .when(view == v, |d| d.bg(white(0.1)).text_color(white(0.9)))
                .when(view != v, |d| d.text_color(white(0.45)).hover(|s| s.bg(white(0.05))))
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if this.settings.sing_view != v {
                        this.toggle_sing_view();
                    }
                    cx.notify();
                }))
        };
        let views = div()
            .flex()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(9.))
            .bg(white(0.04))
            .child(view_tab("view-staff", "Staff", SingView::Staff))
            .child(view_tab("view-roll", "Roll", SingView::Roll));
        let key = self.key();
        let key_pill = div()
            .id("key")
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(white(0.05))
            .hover(|s| s.bg(white(0.09)))
            .cursor_pointer()
            .text_size(px(14.))
            .child(div().text_color(white(0.45)).child("Key"))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} major", key.name)))
            .child(div().text_color(white(0.45)).child(key.relative_minor))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.next_key(1);
                cx.notify();
            }));

        let input = match (&self.test_signal, &self.input) {
            (Some(kind), _) => Some(format!("Test signal: {kind}")),
            (None, Some(name)) => name.lock().unwrap().clone(),
            _ => None,
        };

        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(tabs)
            .when(mode == Mode::Tune, |d| d.child(tuning_pill))
            .when(mode == Mode::Sing, |d| d.child(views).child(key_pill))
            .child(div().flex_1())
            .children(input.map(|name| div().max_w(px(280.)).truncate().text_size(px(12.)).text_color(white(0.4)).child(name)))
            .child(level_meter(self.level_db))
            .child(a4)
    }

    /// The note being aimed for, big, with its octave; green when on it.
    fn note(&self, height: f32, zone: f32) -> impl IntoElement {
        // In the sing view, the piano's note shows before anything is sung. Nothing at all
        // yet: an empty space the note's size, so the layout doesn't jump.
        let midi = match self.settings.mode {
            Mode::Sing => self.target.or(self.reading.map(|r| r.target.midi)),
            Mode::Tune => self.reading.map(|r| r.target.midi),
        };
        let (name, octave) = match midi {
            Some(m) if self.settings.mode == Mode::Sing => {
                let s = music::spell(m, self.key());
                (s.name(), s.octave.to_string())
            }
            Some(m) => (music::name(m, self.flats()).to_string(), music::octave(m).to_string()),
            None => (" ".to_string(), String::new()),
        };
        let color = match (self.live, self.needle) {
            (true, Some(n)) if n.abs() <= zone => hex(OK, 1.0),
            (true, _) => white(0.95),
            _ => white(0.28),
        };
        div()
            .flex()
            .items_end()
            .font_family(MONO_FONT)
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .child(div().text_size(px(height)).line_height(px(height * 1.1)).child(name))
            .child(div().text_size(px(height * 0.3)).mb(px(height * 0.2)).text_color(color.opacity(0.6)).child(octave))
    }

    /// Cents off and frequency; in the tuner, the target's frequency and which way to turn.
    fn cents_line(&self, zone: f32, near: f32, big: f32) -> impl IntoElement {
        let line = div().h(px(big * 1.4)).flex().items_center().gap(px(18.)).font_family(MONO_FONT);
        let Some(r) = self.reading else { return line };
        let n = self.needle.unwrap_or(0.0);
        let color = if self.live { Class::of(n, zone, near).color() } else { white(0.28) };
        let tuning = self.settings.mode == Mode::Tune;
        let aiming = tuning || self.target.is_some();
        let hint = match () {
            _ if !self.live || !aiming => "",
            _ if n.abs() <= zone && tuning => "in tune",
            _ if n.abs() <= zone => "on it",
            _ if n < 0.0 && tuning => "tune up ↑",
            _ if n < 0.0 => "sing higher ↑",
            _ if tuning => "tune down ↓",
            _ => "sing lower ↓",
        };
        let target_hz = music::hz_of(r.target.midi, self.settings.a4);
        line.child(div().text_size(px(big)).text_color(color).child(fmt_cents(n)))
            .child(div().text_size(px(big * 0.6)).text_color(white(if self.live { 0.6 } else { 0.28 })).child(format!("{:.1} Hz", r.hz)))
            .when(aiming, |d| d.child(div().text_size(px(big * 0.5)).text_color(white(0.35)).child(format!("→ {target_hz:.1} Hz"))))
            .child(div().text_size(px(big * 0.55)).text_color(color).child(hint))
    }

    fn tune_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let tuning = self.tuning();
        let caption = match self.reading.map(|r| r.target.string) {
            Some(Some(s)) => {
                let lock = if self.locked == Some(s) { " · locked" } else { "" };
                format!("{} string{lock}", ordinal(music::string_number(tuning, s)))
            }
            Some(None) => "Chromatic".into(),
            None if tuning.strings.is_empty() => "Play a note".into(),
            None => "Play a string".into(),
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .child(div().mt(px(6.)).text_size(px(15.)).text_color(white(0.45)).child(caption))
            .child(self.note(150.0, TUNE_ZONE))
            .child(self.cents_line(TUNE_ZONE, TUNE_NEAR, 30.0))
            .child(div().w_full().max_w(px(880.)).child(gauge(self.needle, self.live, TUNE_ZONE, TUNE_NEAR, 56.0)))
            .when(!tuning.strings.is_empty(), |d| d.child(self.strings_row(cx)))
            .child(self.strip())
    }

    /// One button per string: lights up while played, ticked once in tune, click to lock.
    fn strings_row(&self, cx: &Context<Self>) -> impl IntoElement {
        let tuning = self.tuning();
        let playing = self.reading.filter(|_| self.live).and_then(|r| r.target.string);
        div().flex().gap(px(10.)).children(tuning.strings.iter().enumerate().map(|(i, &midi)| {
            let active = playing == Some(i);
            let n = self.needle.unwrap_or(0.0);
            let (status, color) = match () {
                _ if active && n.abs() <= TUNE_ZONE => ("✓".to_string(), hex(OK, 1.0)),
                _ if active => (format!("{} {}", if n < 0.0 { "↑" } else { "↓" }, fmt_cents(n)), Class::of(n, TUNE_ZONE, TUNE_NEAR).color()),
                _ if self.tuned[i] => ("✓".to_string(), hex(OK, 1.0)),
                _ => (music::string_number(tuning, i).to_string(), white(0.3)),
            };
            let border = match () {
                _ if active => white(0.55),
                _ if self.locked == Some(i) => hex(WARN, 0.9),
                _ => white(0.07),
            };
            div()
                .id(("string", i))
                .w(px(84.))
                .h(px(70.))
                .rounded(px(12.))
                .border_2()
                .border_color(border)
                .bg(white(if active { 0.08 } else { 0.03 }))
                .hover(|s| s.bg(white(0.09)))
                .cursor_pointer()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(2.))
                .font_family(MONO_FONT)
                .child(
                    div()
                        .flex()
                        .items_end()
                        .font_weight(FontWeight::BOLD)
                        .child(div().text_size(px(24.)).child(music::name(midi, tuning.flats)))
                        .child(div().text_size(px(12.)).mb(px(4.)).text_color(white(0.5)).child(music::octave(midi).to_string())),
                )
                .child(div().text_size(px(13.)).text_color(color).child(status))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle_lock(i);
                    cx.notify();
                }))
        }))
    }

    /// Cents from the target over the last seconds, so a string settling (or wobbling)
    /// while its peg turns shows as a line.
    fn strip(&self) -> impl IntoElement {
        let now = self.now;
        let points: Vec<(f64, Option<(f32, i32)>)> = self
            .frames
            .iter()
            .filter(|f| f.t >= now - STRIP_SECS - 0.05)
            .map(|f| {
                (
                    f.t,
                    f.hz.map(|hz| {
                        let target = self.target_for(self.midi(hz));
                        (target.cents(self.midi(hz)), target.midi)
                    }),
                )
            })
            .collect();
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let mid = b.origin.y + b.size.height / 2.0;
                let half = b.size.height / 2.0 - px(6.);
                let y_of = |c: f32| mid - half * (c.clamp(-GAUGE_CENTS, GAUGE_CENTS) / GAUGE_CENTS);
                let x_of = |t: f64| b.origin.x + b.size.width * (1.0 - ((now - t) / STRIP_SECS) as f32);
                window.paint_layer(b, |window| {
                    let zone = Bounds { origin: point(b.origin.x, y_of(TUNE_ZONE)), size: size(b.size.width, y_of(-TUNE_ZONE) - y_of(TUNE_ZONE)) };
                    window.paint_quad(fill(zone, hex(OK, 0.10)));
                    for (c, a) in [(0.0, 0.18), (25.0, 0.05), (-25.0, 0.05)] {
                        window.paint_quad(fill(Bounds { origin: point(b.origin.x, y_of(c)), size: size(b.size.width, px(1.)) }, white(a)));
                    }
                });
                let mut runs = Runs::default();
                let mut prev: Option<(f64, i32)> = None;
                for &(t, p) in &points {
                    match p {
                        Some((cents, target)) => {
                            if prev.is_some_and(|(pt, pm)| t - pt > 0.03 || pm != target) {
                                runs.gap();
                            }
                            runs.push(point(x_of(t), y_of(cents)), Class::of(cents, TUNE_ZONE, TUNE_NEAR));
                            prev = Some((t, target));
                        }
                        None => {
                            runs.gap();
                            prev = None;
                        }
                    }
                }
                runs.paint(window, 2.5);
            },
        )
        .size_full();
        let label = |text: &'static str| div().absolute().left(px(10.)).text_size(px(11.)).font_family(MONO_FONT).text_color(white(0.25)).child(text);
        div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(90.))
            .rounded(px(12.))
            .overflow_hidden()
            .bg(white(0.02))
            .child(paint)
            .child(label("+50¢").top(px(4.)))
            .child(label("−50¢").bottom(px(4.)))
    }

    fn sing_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let sung = Sung::new(&self.frames, self.now, self.settings.a4, self.live);
        let target = self.target.map(|m| {
            let s = music::spell(m, self.key());
            div()
                .id("target")
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(3.))
                .rounded(px(6.))
                .bg(white(0.06))
                .hover(|s| s.bg(white(0.1)))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(white(0.6))
                .child(format!("Target {}{}", s.name(), s.octave))
                .child(div().font_family(MONO_FONT).child("✕"))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.clear_target();
                    cx.notify();
                }))
        });
        let readout = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .child(div().h(px(22.)).flex().children(target))
            .child(self.note(84.0, SING_ZONE))
            .child(self.cents_line(SING_ZONE, SING_NEAR, 22.0));
        let view = match self.settings.sing_view {
            SingView::Staff => self.staff(&sung).into_any_element(),
            SingView::Roll => self.piano_roll(&sung).into_any_element(),
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(28.))
                    .px(px(8.))
                    .child(readout)
                    .child(div().flex_1().child(gauge(self.needle, self.live, SING_ZONE, SING_NEAR, 40.0))),
            )
            .child(view)
            .child(self.keyboard(&sung, cx))
    }

    /// The pitch over the last seconds on a piano roll, newest at the right; the row of the
    /// note being sung lights up.
    fn piano_roll(&self, sung: &Sung) -> impl IntoElement {
        let (now, span) = (self.now, self.settings.span);
        let (lo, hi) = (self.center - span / 2.0, self.center + span / 2.0);
        let current = self.reading.filter(|_| self.live).map(|r| r.target.midi);
        let target = self.target;
        let points = sung.points.clone();
        let classes = sung.classes.clone();
        let rows = (lo.floor() as i32)..=(hi.ceil() as i32);
        // Every natural note, or only the Cs when the rows get thin.
        let labels: Vec<(i32, f32)> = rows
            .clone()
            .filter(|&n| !music::is_black(n) && (span <= 30.0 || n.rem_euclid(12) == 0))
            .map(|n| (n, (hi - n as f32 - 0.5) / span))
            .filter(|&(_, top)| top > -0.2 / span && top < 1.0 - 0.8 / span)
            .collect();

        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let y_of = |m: f32| b.origin.y + b.size.height * ((hi - m) / span);
                let x_of = |t: f64| b.origin.x + b.size.width * (1.0 - ((now - t) / SING_SECS) as f32);
                let bottom = b.origin.y + b.size.height;
                window.paint_layer(b, |window| {
                    for n in rows {
                        let y0 = y_of(n as f32 + 0.5).max(b.origin.y);
                        let y1 = y_of(n as f32 - 0.5).min(bottom);
                        if y1 <= y0 {
                            continue;
                        }
                        let row = Bounds { origin: point(b.origin.x, y0), size: size(b.size.width, y1 - y0) };
                        if current == Some(n) {
                            window.paint_quad(fill(row, hex(OK, 0.13)));
                        } else if !music::is_black(n) {
                            window.paint_quad(fill(row, white(0.03)));
                        }
                        // A firmer line between each B and the C above it.
                        if n.rem_euclid(12) == 0 && y1 < bottom {
                            window.paint_quad(fill(Bounds { origin: point(b.origin.x, y1 - px(0.5)), size: size(b.size.width, px(1.)) }, white(0.12)));
                        }
                    }
                    for s in ((now - SING_SECS).ceil() as i64)..=(now.floor() as i64) {
                        let x = x_of(s as f64);
                        window.paint_quad(fill(Bounds { origin: point(x, b.origin.y), size: size(px(1.), b.size.height) }, white(0.035)));
                    }
                });
                if let Some(m) = target {
                    paint_target_line(window, point(b.origin.x, y_of(m as f32)), b.size.width);
                }

                let mut runs = Runs::default();
                let mut prev: Option<(f64, f32)> = None;
                for (&(t, m), &class) in points.iter().zip(&classes) {
                    match m {
                        Some(m) if (lo..=hi).contains(&m) => {
                            // Break the line at a leap, so note changes read as steps.
                            if prev.is_some_and(|(pt, pm)| t - pt > 0.03 || (m - pm).abs() > 2.0) {
                                runs.gap();
                            }
                            runs.push(point(x_of(t), y_of(m)), class);
                            prev = Some((t, m));
                        }
                        _ => {
                            runs.gap();
                            prev = None;
                        }
                    }
                }
                runs.paint(window, 3.0);

                // The voice now: a dot at the right edge.
                if let Some(&(t, Some(m))) = points.last() {
                    if now - t < LIVE && (lo..=hi).contains(&m) {
                        let color = classes.last().map_or(white(1.0), |c| c.color());
                        paint_dot(window, point(x_of(t), y_of(m)), color);
                    }
                }
            },
        )
        .size_full();

        div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(120.))
            .rounded(px(12.))
            .overflow_hidden()
            .bg(white(0.015))
            .child(paint)
            .children(labels.into_iter().map(|(n, top)| {
                div()
                    .absolute()
                    .left(px(10.))
                    .top(relative(top))
                    .h(relative(1.0 / span))
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .font_family(MONO_FONT)
                    .text_color(white(if n.rem_euclid(12) == 0 { 0.6 } else { 0.3 }))
                    .child(format!("{}{}", music::name(n, false), music::octave(n)))
            }))
    }

    /// The pitch on a tenor's treble-8 staff: the clef and key signature on the left, then
    /// time running to the right edge. Each held note gets a note head where it's written,
    /// with its accidental and ledger lines; the pitch line runs through them.
    fn staff(&self, sung: &Sung) -> impl IntoElement {
        let key = self.key();
        let signature = key.signature();
        let (range, now) = (self.staff_range, self.now);
        let heads: Vec<Head> = sung
            .notes
            .iter()
            .map(|n| Head {
                start: n.start,
                step: music::staff_position(n.midi as f32, key),
                class: Class::of(n.cents, SING_ZONE, SING_NEAR),
                live: n.live,
            })
            .collect();
        let trace: Vec<TracePoint> = sung
            .points
            .iter()
            .zip(&sung.classes)
            .map(|(&(t, m), &c)| (t, m.map(|m| (m, music::staff_position(m, key))), c))
            .collect();
        let target = self.target.map(|m| music::staff_position(m as f32, key));

        let laid_out = self.staff_bounds.clone();
        let paint_heads = heads.clone();
        let signature = signature.clone();
        let paint = canvas(
            move |b, _, _| laid_out.set(b),
            move |b, _, window, _| {
                let g = StaffGeom::new(b.size, range, signature.len(), now);
                let space = g.space();
                let at = |x: f32, y: f32| point(b.origin.x + px(x), b.origin.y + px(y));
                let rect = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: at(x, y), size: size(px(w), px(h)) };
                paint_glyph(window, &glyphs::G_CLEF, at(CLEF_LEFT, g.y(CLEF_STEP)), space, (1.0, 1.0), white(0.75));
                let mut x = g.signature_left();
                for &(step, sign) in &signature {
                    let (glyph, scale) = (accidental_glyph(sign), accidental_scale(sign));
                    paint_glyph(window, glyph, at(x, g.y(step as f32)), space, scale, white(0.75));
                    x += glyph_width(glyph, space, scale.0) + SIGNATURE_GAP * space;
                }
                window.paint_layer(b, |window| {
                    for &line in &music::STAFF_LINES {
                        window.paint_quad(fill(rect(10.0, g.y(line as f32) - 0.75, g.w - 10.0, 1.5), white(0.4)));
                    }
                    for s in ((now - SING_SECS).ceil() as i64)..=(now.floor() as i64) {
                        let x = g.x(s as f64);
                        if x > g.margin {
                            window.paint_quad(fill(rect(x, 0.0, 1.0, g.h), white(0.03)));
                        }
                    }
                });
                let time = rect(g.margin, 0.0, g.w - g.margin, g.h);
                window.with_content_mask(Some(ContentMask { bounds: time }), |window| {
                    if let Some(step) = target {
                        paint_target_line(window, at(g.margin, g.y(step)), px(g.w - g.margin));
                    }
                    window.paint_layer(time, |window| {
                        let reach = 1.9 * head_radii(space).0;
                        for h in &paint_heads {
                            let x = g.head_x(h.start);
                            // A head between steps takes the ledger lines of the step further out.
                            let step = if h.step < music::STAFF_LINES[0] as f32 { h.step.floor() } else { h.step.ceil() };
                            for line in music::ledger_lines(step as i32) {
                                window.paint_quad(fill(rect(x - reach, g.y(line as f32) - 0.75, 2.0 * reach, 1.5), white(0.4)));
                            }
                        }
                    });

                    let mut runs = Runs::default();
                    let mut prev: Option<(f64, f32)> = None;
                    for &(t, p, class) in &trace {
                        match p {
                            Some((m, step)) if (g.lo..=g.hi).contains(&step) => {
                                if prev.is_some_and(|(pt, pm)| t - pt > 0.03 || (m - pm).abs() > 2.0) {
                                    runs.gap();
                                }
                                runs.push(at(g.x(t), g.y(step)), class);
                                prev = Some((t, m));
                            }
                            _ => {
                                runs.gap();
                                prev = None;
                            }
                        }
                    }
                    runs.paint(window, 2.5);

                    let (rx, ry) = head_radii(space);
                    for h in &paint_heads {
                        let centre = at(g.head_x(h.start), g.y(h.step));
                        let color = h.class.color();
                        if h.live {
                            if let Some(glow) = note_head(centre, 1.7 * rx, 1.7 * ry) {
                                window.paint_path(glow, color.opacity(0.18));
                            }
                        }
                        if let Some(head) = note_head(centre, rx, ry) {
                            window.paint_path(head, color.opacity(if h.live { 1.0 } else { 0.85 }));
                        }
                    }
                    if let Some(&(t, Some((_, step)), class)) = trace.last() {
                        if now - t < LIVE {
                            paint_dot(window, at(g.x(t), g.y(step)), class.color());
                        }
                    }
                });
            },
        )
        .size_full();

        // The clef's little 8 is text, so a div over the canvas (text in a canvas makes other
        // text flicker), placed from last frame's layout.
        let g = StaffGeom::new(self.staff_bounds.get().size, range, key.signature().len(), now);
        let space = g.space();
        let eight = (space > 2.0).then(|| {
            div()
                .absolute()
                .left(px(CLEF_LEFT + glyph_width(&glyphs::G_CLEF, space, 1.0) * 0.52 - space / 2.0))
                .top(px(g.y(CLEF_STEP) + CLEF_EIGHT * space))
                .w(px(space))
                .flex()
                .justify_center()
                .text_size(px(0.95 * space))
                .line_height(px(space))
                .text_color(white(0.75))
                .child("8")
        });

        div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(160.))
            .rounded(px(12.))
            .overflow_hidden()
            .bg(white(0.015))
            .child(paint)
            .children(eight)
    }

    /// A keyboard from a baritone's low F to a tenor's high C: click a key to hear it and
    /// sing against it. The key being sung lights up in how close it is.
    fn keyboard(&self, sung: &Sung, cx: &Context<Self>) -> impl IntoElement {
        let singing = sung.now_singing().filter(|_| self.live);
        let pressed = self.pressed.filter(|(_, at)| at.elapsed() < PRESS).map(|(m, _)| m);
        let target = self.target;
        let whites: Vec<i32> = (piano::LOW..=piano::HIGH).filter(|&m| !music::is_black(m)).collect();
        let n = whites.len() as f32;
        let fill_of = |m: i32, black: bool| match () {
            _ if singing.is_some_and(|(s, _)| s == m) => singing.map(|(_, c)| c.color()).unwrap(),
            _ if pressed == Some(m) => white(if black { 0.4 } else { 0.62 }),
            _ if black => hex(0x1d1d1d, 1.0),
            _ => white(0.86),
        };
        let marker = |m: i32, black: bool| {
            (target == Some(m)).then(|| div().size(px(8.)).mb(px(2.)).rounded_full().bg(if black { white(0.85) } else { hex(BG, 0.85) }))
        };
        let press = |m: i32| {
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.press(m);
                cx.notify();
            })
        };
        let white_keys = whites.iter().map(|&m| {
            div()
                .id(("white-key", m as usize))
                .flex_1()
                .h_full()
                .rounded_bl(px(5.))
                .rounded_br(px(5.))
                .bg(fill_of(m, false))
                .cursor_pointer()
                .flex()
                .flex_col()
                .justify_end()
                .items_center()
                .pb(px(5.))
                .children(marker(m, false))
                .when(m.rem_euclid(12) == 0, |d| {
                    d.child(div().text_size(px(10.)).text_color(hex(BG, 0.6)).child(format!("C{}", music::octave(m))))
                })
                .on_mouse_down(MouseButton::Left, press(m))
        });
        let black_keys = (piano::LOW..=piano::HIGH).filter(|&m| music::is_black(m)).map(|m| {
            // Centred on the line between the white keys either side.
            let whites_below = whites.iter().filter(|&&w| w < m).count() as f32;
            div()
                .id(("black-key", m as usize))
                .absolute()
                .top_0()
                .left(relative((whites_below - 0.3) / n))
                .w(relative(0.6 / n))
                .h(relative(0.6))
                .rounded_bl(px(4.))
                .rounded_br(px(4.))
                .border_1()
                .border_color(white(0.1))
                .bg(fill_of(m, true))
                .cursor_pointer()
                .occlude()
                .flex()
                .flex_col()
                .justify_end()
                .items_center()
                .pb(px(4.))
                .children(marker(m, true))
                .on_mouse_down(MouseButton::Left, press(m))
        });
        div()
            .relative()
            .w_full()
            .h(px(86.))
            .flex_none()
            .child(div().size_full().flex().gap(px(2.)).children(white_keys))
            .children(black_keys)
    }

    fn footer(&self) -> impl IntoElement {
        let status = if let Some(e) = &self.error {
            Some((format!("No audio input: {e}"), hex(DANGER, 1.0)))
        } else if !self.received && self.started.elapsed() > Duration::from_millis(1500) {
            Some(("Waiting for sound from the default input…".to_string(), hex(WARN, 1.0)))
        } else if self.frozen.is_some() {
            Some(("Paused · Space to resume".to_string(), hex(WARN, 1.0)))
        } else {
            None
        };
        let hints = match self.settings.mode {
            Mode::Tune => "Tab sing · T tuning · 1–6 lock a string, 0 nearest · ↑↓ A4 · Space pause · Q quit",
            Mode::Sing => match self.settings.sing_view {
                SingView::Staff => "Tab tune · V roll · K key · click the piano to hear a note, X clears it · ↑↓ A4 · Space pause · Q quit",
                SingView::Roll => "Tab tune · V staff · K key · Scroll or [ ] zoom · click the piano to hear a note, X clears it · Space pause · Q quit",
            },
        };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .text_size(px(12.))
            .children(status.map(|(text, color)| div().text_color(color).child(text)))
            .child(div().flex_1())
            .child(div().text_color(white(0.3)).child(hints))
    }
}

impl Render for Monitor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update();
        if self.animating {
            window.request_animation_frame();
        }
        let body = match self.settings.mode {
            Mode::Tune => self.tune_view(cx).into_any_element(),
            Mode::Sing => self.sing_view(cx).into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let dy = f32::from(ev.delta.pixel_delta(px(16.)).y);
                if this.settings.mode == Mode::Sing && dy != 0.0 {
                    this.zoom((1.0 - dy * 0.004).clamp(0.8, 1.25));
                    cx.notify();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(20.))
            .bg(rgb(BG))
            .font_family(TEXT_FONT)
            .text_color(white(0.92))
            .child(self.header(cx))
            .child(body)
            .child(self.footer())
    }
}

/// A horizontal needle gauge over ±50 cents, with the in-tune zone shaded.
fn gauge(needle: Option<f32>, live: bool, zone: f32, near: f32, height: f32) -> impl IntoElement {
    let track = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let x_of = |c: f32| b.origin.x + b.size.width * ((c.clamp(-GAUGE_CENTS, GAUGE_CENTS) + GAUGE_CENTS) / (2.0 * GAUGE_CENTS));
            let (top, h) = (b.origin.y, b.size.height);
            window.paint_layer(b, |window| {
                let shade = Bounds { origin: point(x_of(-zone), top), size: size(x_of(zone) - x_of(-zone), h) };
                window.paint_quad(fill(shade, hex(OK, 0.14)).corner_radii(px(4.)));
                for c in (-50..=50).step_by(5) {
                    let (len, w, a) = match c {
                        0 => (1.0, 2.0, 0.6),
                        c if c % 25 == 0 => (0.6, 2.0, 0.35),
                        c if c % 10 == 0 => (0.4, 1.0, 0.25),
                        _ => (0.22, 1.0, 0.15),
                    };
                    let tick = h * len;
                    window.paint_quad(fill(
                        Bounds { origin: point(x_of(c as f32) - px(w / 2.0), top + (h - tick) / 2.0), size: size(px(w), tick) },
                        white(a),
                    ));
                }
            });
            if let Some(n) = needle {
                let color = if live { Class::of(n, zone, near).color() } else { white(0.25) };
                let x = x_of(n);
                if live {
                    let glow = Bounds { origin: point(x - px(10.), top - px(4.)), size: size(px(20.), h + px(8.)) };
                    window.paint_quad(fill(glow, color.opacity(0.18)).corner_radii(px(10.)));
                }
                let core = Bounds { origin: point(x - px(2.5), top - px(7.)), size: size(px(5.), h + px(14.)) };
                window.paint_quad(fill(core, color).corner_radii(px(2.5)));
            }
        },
    )
    .w_full()
    .h(px(height));

    // Labels in boxes as wide as the track's side padding, so with `justify_between` their
    // centres land on the −50, −25, 0, +25 and +50 ticks.
    let labels = div()
        .flex()
        .justify_between()
        .w_full()
        .text_size(px(12.))
        .text_color(white(0.35))
        .children(["−50", "−25", "0", "+25", "+50"].map(|l| div().w(px(40.)).flex().justify_center().child(l)));
    let symbol = |s: &'static str| div().w(px(24.)).text_size(px(22.)).text_color(white(0.35)).child(s);
    div()
        .flex()
        .items_center()
        .font_family(MONO_FONT)
        .child(symbol("♭"))
        .child(div().flex_1().flex().flex_col().gap(px(10.)).pt(px(8.)).child(div().px(px(20.)).child(track)).child(labels))
        .child(symbol("♯"))
}

/// The input level in dBFS, −70 to 0, with a tick where sound is loud enough to read.
fn level_meter(db: f32) -> impl IntoElement {
    let x_of = |db: f32| ((db + 70.0) / 70.0).clamp(0.0, 1.0);
    let fraction = x_of(db);
    let gate = x_of(track::LEVEL_ON);
    let color = match db {
        d if d > -3.0 => DANGER,
        d if d > -12.0 => WARN,
        _ => OK,
    };
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let track = Bounds { origin: point(b.origin.x, b.origin.y + px(4.)), size: size(b.size.width, px(6.)) };
            window.paint_quad(fill(track, white(0.08)).corner_radii(px(3.)));
            let level = Bounds { origin: track.origin, size: size(b.size.width * fraction, px(6.)) };
            window.paint_quad(fill(level, hex(color, 0.9)).corner_radii(px(3.)));
            let tick = Bounds { origin: point(b.origin.x + b.size.width * gate, b.origin.y + px(1.)), size: size(px(1.), px(12.)) };
            window.paint_quad(fill(tick, white(0.4)));
        },
    )
    .w(px(90.))
    .h(px(14.))
}

/// A polyline in runs of one colour each, to stroke with a soft glow.
#[derive(Default)]
struct Runs {
    runs: Vec<(Vec<Point<Pixels>>, Class)>,
    open: bool,
}

impl Runs {
    fn push(&mut self, p: Point<Pixels>, class: Class) {
        match self.runs.last_mut() {
            Some((points, c)) if self.open && *c == class => points.push(p),
            // A new colour starts where the last one ended, so the line stays joined.
            Some((points, _)) if self.open => {
                let last = *points.last().expect("runs aren't empty");
                self.runs.push((vec![last, p], class));
            }
            _ => self.runs.push((vec![p], class)),
        }
        self.open = true;
    }

    fn gap(&mut self) {
        self.open = false;
    }

    fn paint(&self, window: &mut Window, width: f32) {
        for (w, alpha) in [(width * 3.5, 0.12), (width, 1.0)] {
            for (points, class) in &self.runs {
                let Some((first, rest)) = points.split_first().filter(|(_, rest)| !rest.is_empty()) else { continue };
                let mut path = PathBuilder::stroke(px(w));
                path.move_to(*first);
                for &p in rest {
                    path.line_to(p);
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, class.color().opacity(alpha));
                }
            }
        }
    }
}

/// The pitch averaged over about a vibrato cycle (`SING_SMOOTH` frames either side, within
/// the same note), per frame.
fn smooth(points: &[(f64, Option<f32>)]) -> Vec<Option<f32>> {
    (0..points.len())
        .map(|i| {
            let m = points[i].1?;
            let (mut sum, mut n) = (m, 1.0);
            for dir in [-1isize, 1] {
                for k in 1..=SING_SMOOTH as isize {
                    match points.get((i as isize + dir * k) as usize).and_then(|p| p.1) {
                        Some(v) if (v - m).abs() < 1.0 => {
                            sum += v;
                            n += 1.0;
                        }
                        _ => break,
                    }
                }
            }
            Some(sum / n)
        })
        .collect()
}

/// How close each sung frame is to a note, judged on the smoothed pitch, so a note with
/// even vibrato around it still counts as on it.
fn sing_classes(smoothed: &[Option<f32>]) -> Vec<Class> {
    smoothed.iter().map(|s| s.map_or(Class::Off, |m| Class::of((m - m.round()) * 100.0, SING_ZONE, SING_NEAR))).collect()
}

/// A held note found in the pitch: its time, the semitone it's nearest, how far off on
/// average, and whether it's still being sung.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    start: f64,
    end: f64,
    midi: i32,
    cents: f32,
    live: bool,
}

/// Runs where the smoothed pitch stays within `NOTE_STICK` of the semitone it started
/// nearest, held at least `MIN_NOTE`. Glides and blips between them are dropped, and a note
/// broken briefly (a dropout) is joined back up. Each is named, and its tuning judged, on
/// its middle: the glide into and out of it is left out.
fn held_notes(points: &[(f64, Option<f32>)], smoothed: &[Option<f32>]) -> Vec<Held> {
    // (note so far, named after where it started; its pitch frame by frame)
    let mut runs: Vec<(Held, Vec<f32>)> = Vec::new();
    let mut last: Option<f64> = None;
    for (&(t, raw), &avg) in points.iter().zip(smoothed) {
        let (Some(raw), Some(avg)) = (raw, avg) else {
            last = None;
            continue;
        };
        match runs.last_mut() {
            Some((h, pitch)) if (avg - h.midi as f32).abs() < NOTE_STICK && last.is_some_and(|l| t - l <= 0.035) => {
                h.end = t;
                pitch.push(raw);
            }
            _ => runs.push((Held { start: t, end: t, midi: avg.round() as i32, cents: 0.0, live: false }, vec![raw])),
        }
        last = Some(t);
    }
    // The middle of a run: without the smoothing's reach at either end, if that leaves enough.
    let middle_mean = |pitch: &[f32]| {
        let trim = if pitch.len() > 4 * SING_SMOOTH { SING_SMOOTH } else { 0 };
        let middle = &pitch[trim..pitch.len() - trim];
        middle.iter().sum::<f32>() / middle.len() as f32
    };
    let mut notes: Vec<(Held, Vec<f32>)> = Vec::new();
    for (mut h, pitch) in runs.into_iter().filter(|(h, _)| h.end - h.start >= MIN_NOTE) {
        h.midi = middle_mean(&pitch).round() as i32;
        match notes.last_mut() {
            Some((p, ppitch)) if p.midi == h.midi && h.start - p.end <= NOTE_JOIN => {
                p.end = h.end;
                ppitch.extend(pitch);
            }
            _ => notes.push((h, pitch)),
        }
    }
    notes
        .into_iter()
        .map(|(mut h, pitch)| {
            h.cents = (middle_mean(&pitch) - h.midi as f32) * 100.0;
            h
        })
        .collect()
}

/// What the sing views draw, worked out once a frame: per frame its time, the pitch (MIDI)
/// when voiced, the smoothed pitch and its colour; and the held notes.
struct Sung {
    points: Vec<(f64, Option<f32>)>,
    smoothed: Vec<Option<f32>>,
    classes: Vec<Class>,
    notes: Vec<Held>,
}

impl Sung {
    fn new(frames: &[Frame], now: f64, a4: f32, live: bool) -> Self {
        // A second before the view, so a note already under way when it enters is known.
        let points: Vec<(f64, Option<f32>)> = frames
            .iter()
            .filter(|f| f.t >= now - SING_SECS - 1.0)
            .map(|f| (f.t, f.hz.map(|hz| music::midi_of(hz, a4))))
            .collect();
        let smoothed = smooth(&points);
        let classes = sing_classes(&smoothed);
        let mut notes = held_notes(&points, &smoothed);
        let newest = points.last().filter(|p| p.1.is_some()).map(|p| p.0);
        if let (true, Some(newest), Some(last)) = (live, newest, notes.last_mut()) {
            last.live = last.end >= newest - 0.02;
        }
        Self { points, smoothed, classes, notes }
    }

    /// The note being sung right now, and how close.
    fn now_singing(&self) -> Option<(i32, Class)> {
        let m = (*self.smoothed.last()?)?;
        Some((m.round() as i32, *self.classes.last()?))
    }
}

/// A frame of the staff's pitch line: time, (pitch, staff position) when voiced, colour.
type TracePoint = (f64, Option<(f32, f32)>, Class);

/// A note head to draw: where and when, its colour, and whether it's sounding.
#[derive(Clone, Copy)]
struct Head {
    start: f64,
    /// Where the pitch line puts its semitone: on a line or space for notes in the key,
    /// between them for the rest (no accidentals: the user found them clutter).
    step: f32,
    class: Class,
    live: bool,
}

/// The clef's left edge (px), and the line it curls round: G3 on treble-8.
const CLEF_LEFT: f32 = 16.0;
const CLEF_STEP: f32 = 25.0;
/// Where the clef's little 8 starts, in spaces below the G line: just under its tail.
const CLEF_EIGHT: f32 = 2.75;
/// Noto Music's accidentals are drawn for text: big beside its staff, and its sharp is
/// wide. These (x, y) scales bring them to engraving size and shape, narrow enough to fit
/// between notes sung in quick succession.
fn accidental_scale(sign: Accidental) -> (f32, f32) {
    match sign {
        Accidental::Sharp => (0.45, 0.7),
        Accidental::Flat => (0.7, 0.7),
        Accidental::Natural => (0.55, 0.7),
    }
}
/// Space between the key signature's accidentals, in staff spaces.
const SIGNATURE_GAP: f32 = 0.2;
/// A note head's half width and half height (px) for `space` px a staff space: well under
/// sheet-music size (a full staff space high looked like blobs), and no wider than a small
/// dot on a big window.
fn head_radii(space: f32) -> (f32, f32) {
    let rx = (0.3 * space).min(12.0);
    (rx, 0.72 * rx)
}

fn accidental_glyph(sign: Accidental) -> &'static glyphs::Glyph {
    match sign {
        Accidental::Sharp => &glyphs::SHARP,
        Accidental::Flat => &glyphs::FLAT,
        Accidental::Natural => &glyphs::NATURAL,
    }
}

/// A glyph's width in px at `space` px a staff space, scaled across by `sx`.
fn glyph_width(glyph: &glyphs::Glyph, space: f32, sx: f32) -> f32 {
    glyph.width * sx * space / glyphs::SPACE
}

/// Fills a staff glyph with its anchor (left edge, at the height of its line or space) at
/// `at`, sized for `space` px a staff space and scaled by `(sx, sy)`.
fn paint_glyph(window: &mut Window, glyph: &glyphs::Glyph, at: Point<Pixels>, space: f32, (sx, sy): (f32, f32), color: Hsla) {
    let (kx, ky) = (sx * space / glyphs::SPACE, sy * space / glyphs::SPACE);
    let p = |x: f32, y: f32| point(at.x + px((x - glyph.anchor.0) * kx), at.y - px((y - glyph.anchor.1) * ky));
    let mut path = PathBuilder::fill();
    for seg in glyph.segs {
        match *seg {
            glyphs::Seg::M(x, y) => path.move_to(p(x, y)),
            glyphs::Seg::L(x, y) => path.line_to(p(x, y)),
            glyphs::Seg::Q(cx, cy, x, y) => path.curve_to(p(x, y), p(cx, cy)),
            glyphs::Seg::Z => path.close(),
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Where things go on the staff panel, from its size: the steps `lo..hi` top to bottom, the
/// clef and key signature in a margin on the left, and time from the margin to the right
/// edge (now).
#[derive(Clone, Copy)]
struct StaffGeom {
    w: f32,
    h: f32,
    lo: f32,
    hi: f32,
    margin: f32,
    now: f64,
}

impl StaffGeom {
    fn new(size: Size<Pixels>, (lo, hi): (f32, f32), signature: usize, now: f64) -> Self {
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        let mut g = Self { w, h, lo, hi, margin: 0.0, now };
        // Key signatures are all sharps or all flats; the widest glyph sets the pitch.
        let widest = [Accidental::Sharp, Accidental::Flat].map(|s| glyph_width(accidental_glyph(s), g.space(), accidental_scale(s).0));
        let each = widest[0].max(widest[1]) + SIGNATURE_GAP * g.space();
        g.margin = g.signature_left() + each * signature as f32 + 0.8 * g.space();
        g
    }

    /// Where the key signature starts, after the clef.
    fn signature_left(&self) -> f32 {
        CLEF_LEFT + glyph_width(&glyphs::G_CLEF, self.space(), 1.0) + 0.4 * self.space()
    }

    /// The distance between two staff lines.
    fn space(&self) -> f32 {
        2.0 * self.h / (self.hi - self.lo)
    }

    fn y(&self, step: f32) -> f32 {
        self.h * (self.hi - step) / (self.hi - self.lo)
    }

    fn x(&self, t: f64) -> f32 {
        self.margin + (self.w - self.margin) * (1.0 - ((self.now - t) / SING_SECS) as f32)
    }

    /// A note head's centre for a note starting at `t`: its left edge on the onset.
    fn head_x(&self, t: f64) -> f32 {
        self.x(t) + head_radii(self.space()).0
    }
}

/// A filled note head: an oval with these half width and height, tilted up to the right.
fn note_head(centre: Point<Pixels>, rx: f32, ry: f32) -> Option<Path<Pixels>> {
    let (sin, cos) = (-0.35f32).sin_cos();
    let points: Vec<Point<Pixels>> = (0..28)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 28.0;
            let (ex, ey) = (rx * a.cos(), ry * a.sin());
            point(centre.x + px(ex * cos - ey * sin), centre.y + px(ex * sin + ey * cos))
        })
        .collect();
    let mut path = PathBuilder::fill();
    path.add_polygon(&points, true);
    path.build().ok()
}

/// The piano's note: a dashed line across the view at its pitch.
fn paint_target_line(window: &mut Window, from: Point<Pixels>, width: Pixels) {
    let mut path = PathBuilder::stroke(px(1.5)).dash_array(&[px(7.), px(6.)]);
    path.move_to(from);
    path.line_to(point(from.x + width, from.y));
    if let Ok(path) = path.build() {
        window.paint_path(path, white(0.55));
    }
}

/// The voice now: a glowing dot.
fn paint_dot(window: &mut Window, at: Point<Pixels>, color: Hsla) {
    for (r, a) in [(px(11.), 0.18), (px(5.), 1.0)] {
        let dot = Bounds { origin: point(at.x - r, at.y - r), size: size(r * 2.0, r * 2.0) };
        window.paint_quad(fill(dot, color.opacity(a)).corner_radii(r));
    }
}

/// A scripted press, "seconds:midi".
fn parse_press(s: &str) -> Option<(f64, i32)> {
    let (at, midi) = s.split_once(':')?;
    Some((at.parse().ok()?, midi.parse().ok()?))
}

fn main() {
    // Tests open it under another app id, so a sway rule can keep it off the user's screen.
    let app_id = std::env::var("GUITAR_APP_ID").unwrap_or_else(|_| "guitar".into());
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1000.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("Guitar".into()), ..Default::default() }),
                app_id: Some(app_id),
                window_min_size: Some(size(px(700.), px(540.))),
                ..Default::default()
            },
            |window, cx| {
                let monitor = cx.new(Monitor::new);
                window.focus(&monitor.read(cx).focus);
                monitor
            },
        )
        .unwrap();
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_read_with_a_real_minus_sign() {
        assert_eq!(fmt_cents(0.4), "0¢");
        assert_eq!(fmt_cents(3.6), "+4¢");
        assert_eq!(fmt_cents(-12.2), "−12¢");
    }

    #[test]
    fn runs_split_by_colour_but_stay_joined() {
        let mut r = Runs::default();
        let p = |x: f32| point(px(x), px(0.));
        r.push(p(0.), Class::On);
        r.push(p(1.), Class::On);
        r.push(p(2.), Class::Off);
        r.gap();
        r.push(p(5.), Class::Off);
        let shapes: Vec<(usize, bool)> = r.runs.iter().map(|(pts, c)| (pts.len(), *c == Class::On)).collect();
        assert_eq!(shapes, [(2, true), (2, false), (1, false)]);
        assert_eq!(r.runs[1].0[0], p(1.), "the new colour starts at the last point");
    }

    /// Frames at 100 a second: (seconds, MIDI or rest) pieces laid end to end.
    fn track_of(pieces: &[(f64, Option<f32>)]) -> Vec<(f64, Option<f32>)> {
        let mut t = 0.0;
        let mut out = Vec::new();
        for &(secs, m) in pieces {
            for _ in 0..(secs * 100.0).round() as usize {
                out.push((t, m));
                t += 0.01;
            }
        }
        out
    }

    #[test]
    fn held_notes_skip_blips_and_rejoin_a_note_broken_briefly() {
        let points = track_of(&[
            (0.4, Some(60.0)),
            (0.05, Some(70.0)), // a blip
            (0.4, Some(60.1)),
            (0.2, None),
            (0.3, Some(64.2)),
            (0.03, None), // a dropout
            (0.3, Some(64.2)),
        ]);
        let notes = held_notes(&points, &points.iter().map(|p| p.1).collect::<Vec<_>>());
        let summary: Vec<(i32, i32)> = notes.iter().map(|n| (n.midi, n.cents.round() as i32)).collect();
        assert_eq!(summary, [(60, 5), (64, 20)]);
        assert!((notes[0].end - notes[0].start - 0.84).abs() < 0.02, "one note across the blip");
    }

    #[test]
    fn a_voice_hovering_between_two_notes_stays_one_note() {
        // G3 sung 35–65 cents flat, wandering slowly across the halfway mark to F♯3, as in
        // the user's first try at matching the piano's G3.
        let points: Vec<(f64, Option<f32>)> = (0..200)
            .map(|i| {
                let t = i as f64 / 100.0;
                (t, Some(54.5 + 0.15 * (std::f64::consts::TAU * 1.3 * t).sin() as f32))
            })
            .collect();
        let notes = held_notes(&points, &smooth(&points));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(notes[0].midi, 55);
        assert!((notes[0].cents + 50.0).abs() < 5.0, "{:.1} cents", notes[0].cents);
        // A real change of note still starts a new one.
        let steps = track_of(&[(0.5, Some(55.0)), (0.5, Some(57.0))]);
        assert_eq!(held_notes(&steps, &smooth(&steps)).iter().map(|n| n.midi).collect::<Vec<_>>(), [55, 57]);
    }

    #[test]
    fn the_test_voice_comes_out_as_its_tune() {
        let mut synth = audio::Synth::new(audio::Signal::Voice);
        let mut capture = audio::Capture::new(48_000);
        let mut analyser = track::Analyser::new(48_000, 0);
        let mut frames = Vec::new();
        while capture.written < 48_000 * 12 {
            for _ in 0..480 {
                capture.push(synth.next());
            }
            frames.extend(analyser.take(&capture).into_iter().map(|w| analyser.analyse(w)));
        }
        let sung = Sung::new(&frames, 12.0, 440.0, false);
        let notes: Vec<i32> = sung.notes.iter().map(|n| n.midi).collect();
        // From 1 s in: G4 A4 G4, F4 E4 D4 C4, then G3 E3 C3 D3 E3 F♯3 G3 (repeats join up).
        assert_eq!(notes, [67, 69, 67, 65, 64, 62, 60, 55, 52, 48, 50, 52, 54, 55]);
        let a4 = sung.notes[1];
        assert!((a4.cents - 14.0).abs() < 4.0, "A4 sung 18 then 10 cents sharp: {:.1}", a4.cents);
    }

    #[test]
    fn vibrato_around_a_note_counts_as_on_it() {
        // ±40 cents of vibrato at 5.5 Hz around A4, sampled at 100 frames a second.
        let points: Vec<(f64, Option<f32>)> = (0..200)
            .map(|i| {
                let t = i as f64 / 100.0;
                (t, Some(69.0 + 0.4 * (std::f64::consts::TAU * 5.5 * t).sin() as f32))
            })
            .collect();
        let classes = sing_classes(&smooth(&points));
        let on = classes[20..180].iter().filter(|&&c| c == Class::On).count();
        assert!(on > 150, "only {on} of 160 frames on pitch");
        // Held 30 cents sharp is off.
        let sharp: Vec<(f64, Option<f32>)> = (0..50).map(|i| (i as f64 / 100.0, Some(69.3))).collect();
        assert!(sing_classes(&smooth(&sharp)).iter().all(|&c| c == Class::Off));
    }
}
