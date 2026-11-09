//! Captures the default input (microphone or interface) through PipeWire, as mono.
//!
//! With no target, WirePlumber links the stream to the default source and re-links it when
//! the default input changes.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;

use crate::piano;
use crate::pitch::Lowpass;

/// Mono samples of history kept: well over the analysis window, so the analysis thread can
/// fall behind for a while without losing anything.
pub const HISTORY: usize = 1 << 15;

pub struct Capture {
    /// The input as it came in, for the level meter.
    raw: Vec<f32>,
    /// The input low-passed for pitch detection (see `pitch::PREFILTER_HZ`).
    filtered: Vec<f32>,
    lowpass: Lowpass,
    pos: usize,
    /// Total samples captured; also the clock the pitch history is timed by.
    pub written: u64,
    rate: u32,
    pub received: bool,
    pub error: Option<String>,
}

impl Capture {
    pub fn new(rate: u32) -> Self {
        Self {
            raw: vec![0.0; HISTORY],
            filtered: vec![0.0; HISTORY],
            lowpass: Lowpass::new(rate),
            pos: 0,
            written: 0,
            rate,
            received: false,
            error: None,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    fn set_rate(&mut self, rate: u32) {
        if rate > 0 && rate != self.rate {
            self.rate = rate;
            self.lowpass = Lowpass::new(rate);
        }
    }

    pub fn push(&mut self, sample: f32) {
        self.raw[self.pos] = sample;
        self.filtered[self.pos] = self.lowpass.run(sample);
        self.pos = (self.pos + 1) % HISTORY;
        self.written += 1;
    }

    /// Where in the rings the `n` samples ending `end` samples into the capture start, if
    /// they're all held: captured already, and not yet overwritten.
    fn start_of(&self, end: u64, n: usize) -> Option<usize> {
        let n = n as u64;
        if end > self.written || end < n || self.written - (end - n) > HISTORY as u64 {
            return None;
        }
        // `pos` is where sample number `written` goes; step back to `end - n`.
        let back = (self.written - (end - n)) as usize;
        Some((self.pos + HISTORY - back) % HISTORY)
    }

    /// Copies the low-passed samples that end `end` samples in, oldest first; false if they
    /// aren't all held.
    pub fn window_ending(&self, end: u64, out: &mut [f32]) -> bool {
        let Some(start) = self.start_of(end, out.len()) else { return false };
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.filtered[(start + i) % HISTORY];
        }
        true
    }

    /// Copies the input as it came in, the samples that end `end` samples in, oldest first;
    /// false if they aren't all held. For hearing yourself (`piano::Ear`).
    pub fn raw_ending(&self, end: u64, out: &mut [f32]) -> bool {
        let Some(start) = self.start_of(end, out.len()) else { return false };
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.raw[(start + i) % HISTORY];
        }
        true
    }

    /// The level of the `n` input samples that end `end` samples in, dBFS RMS.
    pub fn level_ending(&self, end: u64, n: usize) -> Option<f32> {
        let start = self.start_of(end, n)?;
        let sum: f32 = (0..n).map(|i| self.raw[(start + i) % HISTORY].powi(2)).sum();
        Some(10.0 * (sum / n.max(1) as f32).max(1e-12).log10())
    }
}

pub type Shared = Arc<Mutex<Capture>>;

/// The test signal asked for with `GUITAR_TEST_SIGNAL` (`guitar`, `voice` or `quiet`).
pub fn test_signal() -> Option<String> {
    std::env::var("GUITAR_TEST_SIGNAL").ok().filter(|v| !v.is_empty() && v != "0")
}

/// Starts capturing on a background thread that owns the PipeWire main loop. With a test
/// signal it makes one up instead (see `Synth`), and mixes the piano's `voices` into it as
/// if the microphone heard them.
pub fn start(voices: piano::Voices) -> Shared {
    let shared = Arc::new(Mutex::new(Capture::new(48_000)));
    let thread_shared = shared.clone();
    if let Some(kind) = test_signal() {
        let signal = match kind.as_str() {
            "guitar" => Signal::Guitar,
            "quiet" => Signal::Quiet,
            "warmup" => Signal::Warmup,
            _ => Signal::Voice,
        };
        thread::Builder::new()
            .name("test-signal".into())
            .spawn(move || synthesize(thread_shared, signal, voices))
            .expect("failed to spawn test signal thread");
        return shared;
    }
    thread::Builder::new()
        .name("pipewire".into())
        .spawn(move || {
            if let Err(e) = run(thread_shared.clone()) {
                thread_shared.lock().unwrap().error = Some(e.to_string());
            }
        })
        .expect("failed to spawn pipewire thread");
    shared
}

fn run(shared: Shared) -> Result<()> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Production",
        *pw::keys::NODE_NAME => "guitar",
        *pw::keys::APP_NAME => "guitar",
        // Small quanta, so new samples arrive every few milliseconds.
        *pw::keys::NODE_LATENCY => "256/48000",
    };
    let stream = pw::stream::StreamBox::new(&core, "guitar", props)?;

    let format_shared = shared.clone();
    let _listener = stream
        .add_local_listener_with_user_data(1u32)
        .param_changed(move |_, channels, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
                return;
            };
            if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                return;
            }
            let mut info = spa::param::audio::AudioInfoRaw::new();
            if info.parse(param).is_ok() {
                *channels = info.channels().max(1);
                format_shared.lock().unwrap().set_rate(info.rate());
            }
        })
        .process(move |stream, channels| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else { return };

            let offset = data.chunk().offset() as usize;
            let size = data.chunk().size() as usize;
            let channels = *channels as usize;
            let Some(bytes) = data.data() else { return };
            let Some(bytes) = bytes.get(offset..offset + size) else { return };

            // Downmix interleaved f32 frames to mono.
            let mut capture = shared.lock().unwrap();
            for frame in bytes.chunks_exact(4 * channels) {
                let sum: f32 = frame.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).sum();
                capture.push(sum / channels as f32);
            }
            capture.received = true;
        })
        .register()?;

    // Ask for f32 and leave rate/channels open so we get the source's native format.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).expect("valid format pod")];

    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    mainloop.run();
    Ok(())
}

/// Feeds the capture from a `Synth`, plus the piano, in real time in PipeWire-sized chunks.
fn synthesize(shared: Shared, signal: Signal, voices: piano::Voices) {
    const CHUNK: u64 = 480;
    let mut synth = Synth::new(signal);
    let mut heard = vec![0.0; CHUNK as usize];
    let start = Instant::now();
    let mut n = 0u64;
    loop {
        let due = start + Duration::from_secs_f64((n + CHUNK) as f64 / Synth::RATE as f64);
        thread::sleep(due.saturating_duration_since(Instant::now()));
        heard.fill(0.0);
        piano::render(&mut voices.lock().unwrap(), &mut heard);
        let mut capture = shared.lock().unwrap();
        for &h in &heard {
            capture.push(synth.next() + h);
        }
        n += CHUNK;
        capture.received = true;
    }
}

#[derive(Clone, Copy)]
pub enum Signal {
    Guitar,
    Voice,
    /// A singer doing the five-note warm-up (see `warmup_sung`).
    Warmup,
    /// Just the hiss: for trying the piano.
    Quiet,
}

/// A made-up input in place of the microphone, for tests and screenshots: silent, and the
/// same picture every run. `Guitar` plucks each open string of standard tuning in turn,
/// some of them out of tune; `Voice` sings a phrase with vibrato and glides, a few notes
/// sharp or flat, with rests between lines; `Quiet` is just the hiss they all have.
pub struct Synth {
    signal: Signal,
    n: u64,
    seed: u32,
    phase: f64,
}

/// The test guitar's strings, (MIDI, cents off), each plucked for `PLUCK_SECS`, then a
/// second of quiet.
pub const STRINGS: [(f64, f64); 6] = [(40.0, -14.0), (45.0, 6.0), (50.0, 0.0), (55.0, 22.0), (59.0, -3.0), (64.0, 1.0)];
pub const PLUCK_SECS: f64 = 2.5;

impl Synth {
    pub const RATE: u32 = 48_000;

    pub fn new(signal: Signal) -> Self {
        Self { signal, n: 0, seed: 0x2545_f491, phase: 0.0 }
    }

    pub fn next(&mut self) -> f32 {
        let t = self.n as f64 / Self::RATE as f64;
        self.n += 1;
        let x = match self.signal {
            Signal::Guitar => pluck(t),
            Signal::Voice | Signal::Warmup => {
                let (hz, amp) = if matches!(self.signal, Signal::Voice) { sung(t) } else { warmup_sung(t) };
                self.phase = (self.phase + hz / Self::RATE as f64).fract();
                amp * voice_wave(self.phase)
            }
            Signal::Quiet => 0.0,
        };
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        x + 0.002 * (self.seed as f32 / u32::MAX as f32 * 2.0 - 1.0)
    }

    /// The pitch being played or sung at `t` (Hz), and how loud, for checking readings.
    #[cfg(test)]
    pub fn truth(&self, t: f64) -> (f64, f32) {
        match self.signal {
            Signal::Guitar => plucked(t).map_or((0.0, 0.0), |(hz, since)| (hz, (-since * 1.3).exp() as f32)),
            Signal::Voice => sung(t),
            Signal::Warmup => warmup_sung(t),
            Signal::Quiet => (0.0, 0.0),
        }
    }
}

fn freq(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
}

/// The string ringing at `t`: its frequency and how long since it was plucked.
fn plucked(t: f64) -> Option<(f64, f64)> {
    let cycle = t % (PLUCK_SECS * STRINGS.len() as f64 + 1.0);
    let i = (cycle / PLUCK_SECS) as usize;
    let &(midi, cents) = STRINGS.get(i)?;
    Some((freq(midi + cents / 100.0), cycle - i as f64 * PLUCK_SECS))
}

fn pluck(t: f64) -> f32 {
    let Some((f0, since)) = plucked(t) else { return 0.0 };
    let attack = (since / 0.003).min(1.0);
    // Higher harmonics die away faster; the fundamental is weaker than the next two.
    let amps = [0.35, 1.0, 0.8, 0.55, 0.4, 0.3, 0.2, 0.12];
    let sum: f64 = amps
        .iter()
        .enumerate()
        .map(|(k, a)| {
            let h = (k + 1) as f64;
            a * (-since * (0.8 + 0.5 * h)).exp() * (std::f64::consts::TAU * f0 * h * since).sin()
        })
        .sum();
    (0.18 * attack * sum) as f32
}

/// The sung pitch (Hz) and loudness at `t`: "Twinkle twinkle" in C, then a tenor's line
/// down to a low C (a ledger line on treble-8) and back up through F♯, half a second a note,
/// gliding into each, with vibrato after the onset, and a rest after each line.
fn sung(t: f64) -> (f64, f32) {
    // (MIDI, cents off) per note; 0 = rest.
    const TUNE: [(f64, f64); 24] = [
        (60.0, 0.0), (60.0, 5.0), (67.0, -8.0), (67.0, 0.0), (69.0, 18.0), (69.0, 10.0), (67.0, 0.0), (0.0, 0.0),
        (65.0, -25.0), (65.0, -5.0), (64.0, 0.0), (64.0, 3.0), (62.0, -12.0), (62.0, 0.0), (60.0, 2.0), (0.0, 0.0),
        (55.0, 0.0), (52.0, 4.0), (48.0, -6.0), (50.0, 0.0), (52.0, 8.0), (54.0, -3.0), (55.0, 0.0), (0.0, 0.0),
    ];
    const NOTE: f64 = 0.5;
    let pos = t % (NOTE * TUNE.len() as f64);
    let i = (pos / NOTE) as usize;
    let since = pos - i as f64 * NOTE;
    let (midi, cents) = TUNE[i];
    let prev = TUNE[(i + TUNE.len() - 1) % TUNE.len()];
    if midi == 0.0 {
        return (freq(prev.0), 0.0);
    }
    let (from, from_cents) = if prev.0 == 0.0 { (midi, cents) } else { prev };
    let glide = (since / 0.06).min(1.0);
    let base = from + from_cents / 100.0 + (midi + cents / 100.0 - from - from_cents / 100.0) * glide;
    let vibrato = 0.3 * ((since - 0.15) / 0.2).clamp(0.0, 1.0) * (std::f64::consts::TAU * 5.5 * since).sin();
    // Each note swells in and fades a little.
    let env = (since / 0.03).min(1.0) * (1.0 - 0.3 * since / NOTE);
    (freq(base + vibrato), env as f32)
}

/// The test singer's warm-up: the five-note scale once every `WARMUP_CYCLE` s, starting
/// `WARMUP_AT` s into it (after the piano has played it, when the session started a second
/// in), `WARMUP_NOTE` s a note, legato with a quick glide, a little vibrato, some notes off
/// by `WARMUP_CENTS`. A semitone higher each time, C3 up to C4 and back, as the app goes.
pub const WARMUP_CENTS: [f64; 9] = [0.0, 0.0, 15.0, 0.0, -18.0, 0.0, 0.0, 6.0, 0.0];
const WARMUP_CYCLE: f64 = 13.0;
const WARMUP_AT: f64 = 6.0;
const WARMUP_NOTE: f64 = 0.7;

fn warmup_sung(t: f64) -> (f64, f32) {
    let rep = (t / WARMUP_CYCLE) as i32 % 24;
    let root = 48.0 + rep.min(24 - rep) as f64;
    let pos = t % WARMUP_CYCLE - WARMUP_AT;
    let steps = crate::warmup::PATTERNS[0].steps;
    if !(0.0..WARMUP_NOTE * steps.len() as f64).contains(&pos) {
        return (freq(root), 0.0);
    }
    let i = (pos / WARMUP_NOTE) as usize;
    let since = pos - i as f64 * WARMUP_NOTE;
    let at = |i: usize| root + steps[i] as f64 + WARMUP_CENTS[i] / 100.0;
    let from = at(i.saturating_sub(1));
    let base = from + (at(i) - from) * (since / 0.06).min(1.0);
    let vibrato = 0.12 * ((since - 0.15) / 0.2).clamp(0.0, 1.0) * (std::f64::consts::TAU * 5.5 * since).sin();
    let env = (pos / 0.03).min(1.0) * (1.0 - 0.2 * since / WARMUP_NOTE);
    (freq(base + vibrato), env as f32)
}

/// One period of a voice-like wave: a rich harmonic series falling off with frequency.
fn voice_wave(phase: f64) -> f32 {
    let tau = std::f64::consts::TAU;
    let amps = [1.0, 0.7, 0.5, 0.35, 0.25, 0.18, 0.12, 0.08];
    let sum: f64 = amps.iter().enumerate().map(|(k, a)| a * (tau * (k + 1) as f64 * phase).sin()).sum();
    (0.12 * sum) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_come_from_the_right_place_in_the_ring() {
        // Silence, then full scale for the last 100 samples, after the ring has wrapped.
        let mut c = Capture::new(48_000);
        for i in 0..(HISTORY as u64 + 100) {
            c.push(if i >= HISTORY as u64 { 1.0 } else { 0.0 });
        }
        let level = |end: u64| c.level_ending(end, 100).unwrap();
        assert!(level(c.written).abs() < 1e-4);
        assert!((level(c.written - 50) + 3.0103).abs() < 1e-3, "half and half");
        assert!(level(c.written - 100) < -100.0);
        let mut out = [0.0; 4];
        assert!(c.window_ending(c.written - 10, &mut out));
        assert!(out.iter().all(|&x| x > 0.5), "filtered samples from after the step: {out:?}");
        // Not captured yet, and long overwritten.
        assert!(!c.window_ending(c.written + 1, &mut out));
        assert!(!c.window_ending(50, &mut out));
        assert_eq!(c.level_ending(50, 4), None);
    }
}
