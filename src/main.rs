//! guitar: a pitch monitor for tuning a guitar and singing, built with gpui.
//!
//! Tune: the nearest string of the chosen tuning (or the nearest note, chromatic), a needle
//! in cents, the strings with a tick once each is in tune, and the last seconds of cents.
//! Sing: the nearest note and a scrolling piano roll of the pitch, coloured by how close
//! each held note is to the note it's nearest.

mod audio;
mod music;
mod pitch;
mod settings;
mod source;
mod track;

use std::time::{Duration, Instant};

use gpui::{
    canvas, div, fill, point, prelude::*, px, relative, rgb, size, App, Application, Bounds, ClickEvent, Context,
    FocusHandle, FontWeight, Hsla, KeyDownEvent, PathBuilder, Pixels, Point, ScrollWheelEvent, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};

use music::{Target, Tuning, TUNINGS};
use settings::{Mode, Settings};
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

        let test_signal = std::env::var("GUITAR_TEST_SIGNAL").ok().filter(|v| !v.is_empty() && v != "0");
        Self {
            track: track::start(audio::start()),
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

    fn target_for(&self, midi: f32) -> Target {
        match self.settings.mode {
            Mode::Tune => Target::for_reading(midi, self.tuning(), self.locked),
            Mode::Sing => Target { midi: midi.round() as i32, string: None },
        }
    }

    fn midi(&self, hz: f32) -> f32 {
        music::midi_of(hz, self.settings.a4)
    }

    /// Takes in what the analysis has found since the last frame.
    fn update(&mut self) {
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
            .child(div().flex_1())
            .children(input.map(|name| div().max_w(px(280.)).truncate().text_size(px(12.)).text_color(white(0.4)).child(name)))
            .child(level_meter(self.level_db))
            .child(a4)
    }

    /// The note being aimed for, big, with its octave; green when on it.
    fn note(&self, height: f32, zone: f32) -> impl IntoElement {
        // Nothing heard yet: an empty space the note's size, so the layout doesn't jump.
        let (name, octave) = match self.reading {
            Some(r) => (music::name(r.target.midi, self.flats()), music::octave(r.target.midi).to_string()),
            None => (" ", String::new()),
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
        let hint = match () {
            _ if !self.live || !tuning => "",
            _ if n.abs() <= zone => "in tune",
            _ if n < 0.0 => "tune up ↑",
            _ => "tune down ↓",
        };
        let target_hz = self.settings.a4 * 2f32.powf((r.target.midi as f32 - 69.0) / 12.0);
        line.child(div().text_size(px(big)).text_color(color).child(fmt_cents(n)))
            .child(div().text_size(px(big * 0.6)).text_color(white(if self.live { 0.6 } else { 0.28 })).child(format!("{:.1} Hz", r.hz)))
            .when(tuning, |d| d.child(div().text_size(px(big * 0.5)).text_color(white(0.35)).child(format!("→ {target_hz:.1} Hz"))))
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

    fn sing_view(&self) -> impl IntoElement {
        let readout = div()
            .w(px(250.))
            .flex()
            .flex_col()
            .child(self.note(84.0, SING_ZONE))
            .child(self.cents_line(SING_ZONE, SING_NEAR, 22.0));
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
            .child(self.piano_roll())
    }

    /// The pitch over the last seconds on a piano roll, newest at the right; the row of the
    /// note being sung lights up.
    fn piano_roll(&self) -> impl IntoElement {
        let (now, span) = (self.now, self.settings.span);
        let (lo, hi) = (self.center - span / 2.0, self.center + span / 2.0);
        let current = self.reading.filter(|_| self.live).map(|r| r.target.midi);
        let points: Vec<(f64, Option<f32>)> = self
            .frames
            .iter()
            .filter(|f| f.t >= now - SING_SECS - 0.05)
            .map(|f| (f.t, f.hz.map(|hz| self.midi(hz))))
            .collect();
        let classes = sing_classes(&points);
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
                        let at = point(x_of(t), y_of(m));
                        for (r, a) in [(px(11.), 0.18), (px(5.), 1.0)] {
                            let dot = Bounds { origin: point(at.x - r, at.y - r), size: size(r * 2.0, r * 2.0) };
                            window.paint_quad(fill(dot, color.opacity(a)).corner_radii(r));
                        }
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
            Mode::Sing => "Tab tune · Scroll or [ ] zoom · ↑↓ A4 · Space pause · Q quit",
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
            Mode::Sing => self.sing_view().into_any_element(),
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

/// How close each sung frame is to a note, judged on the pitch averaged over about a
/// vibrato cycle (`SING_SMOOTH` frames either side, within the same note).
fn sing_classes(points: &[(f64, Option<f32>)]) -> Vec<Class> {
    (0..points.len())
        .map(|i| {
            let Some(m) = points[i].1 else { return Class::Off };
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
            let avg = sum / n;
            Class::of((avg - avg.round()) * 100.0, SING_ZONE, SING_NEAR)
        })
        .collect()
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

    #[test]
    fn vibrato_around_a_note_counts_as_on_it() {
        // ±40 cents of vibrato at 5.5 Hz around A4, sampled at 100 frames a second.
        let points: Vec<(f64, Option<f32>)> = (0..200)
            .map(|i| {
                let t = i as f64 / 100.0;
                (t, Some(69.0 + 0.4 * (std::f64::consts::TAU * 5.5 * t).sin() as f32))
            })
            .collect();
        let classes = sing_classes(&points);
        let on = classes[20..180].iter().filter(|&&c| c == Class::On).count();
        assert!(on > 150, "only {on} of 160 frames on pitch");
        // Held 30 cents sharp is off.
        let sharp: Vec<(f64, Option<f32>)> = (0..50).map(|i| (i as f64 / 100.0, Some(69.3))).collect();
        assert!(sing_classes(&sharp).iter().all(|&c| c == Class::Off));
    }
}
