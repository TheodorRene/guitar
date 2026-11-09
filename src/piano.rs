//! The piano: plays reference notes to sing against, through PipeWire.
//!
//! Each key press plays a note with a piano-like tone: a fast attack, then partials that ring
//! and fade, the higher ones sooner. One note at a time: a new key fades the last one out
//! quickly, since notes ringing on into each other sounded dissonant against the voice;
//! unless it's played as part of a chord (`add`), which keeps the others ringing.
//! The playback stream starts on the first press and
//! pauses once the piano has been quiet a few seconds, so the output can sleep. With a test
//! signal running, nothing is played: the voices are mixed into the made-up input instead
//! (see `audio::Synth`), as if the microphone heard them.
//!
//! Two things for singing with headphones on: with `hold`, notes don't die away but settle
//! into a soft drone that lasts until the next note (out of tune against it, you hear it
//! beat); and the ear (`Ear`) mixes the microphone into the output as it comes in, so you
//! hear yourself, with a guard that turns it off if it starts to feed back.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;

use crate::audio::{self, Capture};

pub const RATE: u32 = 48_000;
/// The keyboard's range: a baritone's low F to a tenor's high C.
pub const LOW: i32 = 41;
pub const HIGH: i32 = 72;
/// How long a note rings, s, the last `RELEASE` of it fading out.
const LENGTH: f64 = 2.2;
const RELEASE: f64 = 0.3;
/// How fast a note fades when the next key is pressed, s: quick, but not a click.
const CUT: f64 = 0.08;
/// (amplitude, decay per second) of each partial, fundamental first.
const PARTIALS: [(f64, f64); 7] = [(1.0, 1.3), (0.5, 1.9), (0.3, 2.6), (0.2, 3.4), (0.12, 4.2), (0.08, 5.2), (0.05, 6.2)];
/// Piano strings are slightly stiff, so partial k sits a little above k·f.
const STIFFNESS: f64 = 0.0002;
/// A held note settles to this much of each partial: a drone, softer than the strike.
const SUSTAIN: f64 = 0.35;
const GAIN: f64 = 0.22;
const MAX_VOICES: usize = 8;
/// Pause the playback stream after this long without a note.
const IDLE: Duration = Duration::from_secs(3);

pub struct Voice {
    hz: f64,
    n: u64,
    /// The sample at which a newer note cut this one short.
    cut: Option<u64>,
    /// Rings on as a drone until cut, its partials exactly harmonic so beats are clean.
    held: bool,
}

impl Voice {
    fn new(hz: f64, held: bool) -> Self {
        Self { hz, n: 0, cut: None, held }
    }

    fn sample(&self) -> f64 {
        let t = self.n as f64 / RATE as f64;
        let cut = self.cut.map_or(1.0, |c| (1.0 - (self.n - c) as f64 / (CUT * RATE as f64)).max(0.0));
        let (release, sustain, stiffness) =
            if self.held { (1.0, SUSTAIN, 0.0) } else { (((LENGTH - t) / RELEASE).clamp(0.0, 1.0), 0.0, STIFFNESS) };
        let envelope = (t / 0.004).min(1.0) * release * cut;
        let tone: f64 = PARTIALS
            .iter()
            .enumerate()
            .map(|(k, &(amp, decay))| {
                let h = (k + 1) as f64;
                let f = self.hz * h * (1.0 + stiffness * h * h).sqrt();
                amp * (sustain + (1.0 - sustain) * (-decay * t).exp()) * (std::f64::consts::TAU * f * t).sin()
            })
            .sum();
        envelope * tone
    }

    fn done(&self) -> bool {
        let cut_done = self.cut.is_some_and(|c| (self.n - c) as f64 >= CUT * RATE as f64);
        cut_done || (!self.held && self.n as f64 >= LENGTH * RATE as f64)
    }
}

pub type Voices = Arc<Mutex<Vec<Voice>>>;

/// Mixes the next `out.len()` samples of every voice into `out`, and drops finished voices.
pub fn render(voices: &mut Vec<Voice>, out: &mut [f32]) {
    for o in out.iter_mut() {
        let mut sum = 0.0;
        for v in voices.iter_mut() {
            sum += v.sample();
            v.n += 1;
        }
        // Soft-clip, so chords stay clean of hard clipping.
        *o += (GAIN * sum).tanh() as f32;
    }
    voices.retain(|v| !v.done());
}

/// How loud the microphone comes through in the ear: as it is.
const EAR_GAIN: f32 = 1.0;
/// Samples of slack the ear keeps behind the input, beyond one output buffer, so a slightly
/// late input buffer doesn't run it dry.
const EAR_SLACK: usize = 32;
/// Feedback guard: the ear turns itself off when what it plays stays this loud (RMS) for
/// `HOWL_SECS`: a voice through a laptop microphone doesn't, a howl does.
const HOWL_RMS: f32 = 0.35;
const HOWL_SECS: f64 = 0.5;

/// Hearing yourself: the newest input, mixed into the output buffer by buffer.
#[derive(Default)]
pub struct Ear {
    /// The next input sample to play (by `Capture::written`'s count).
    cursor: Option<u64>,
    /// Loud samples in a row (see `HOWL_RMS`).
    loud: usize,
    buf: Vec<f32>,
}

impl Ear {
    /// Adds the input that came in since the last call to `out`, staying a buffer behind
    /// the newest sample (jumping ahead if it falls further behind); false if it's been too
    /// loud for too long, for the caller to turn it off.
    pub fn mix(&mut self, capture: &Capture, out: &mut [f32]) -> bool {
        let n = out.len();
        let written = capture.written;
        if capture.rate() != RATE || n == 0 || written < (n + EAR_SLACK) as u64 {
            return true;
        }
        let cursor = match self.cursor {
            Some(c) if c + n as u64 <= written && written - c <= (3 * n + EAR_SLACK) as u64 => c,
            _ => written - (n + EAR_SLACK) as u64,
        };
        self.cursor = Some(cursor + n as u64);
        self.buf.resize(n, 0.0);
        if !capture.raw_ending(cursor + n as u64, &mut self.buf) {
            return true;
        }
        let mut power = 0.0;
        for (o, &x) in out.iter_mut().zip(&self.buf) {
            let y = EAR_GAIN * x;
            power += y * y;
            *o += y.tanh();
        }
        self.loud = if (power / n as f32).sqrt() > HOWL_RMS { self.loud + n } else { 0 };
        (self.loud as f64) < HOWL_SECS * RATE as f64
    }
}

/// Whether the ear is on, and whether the feedback guard turned it off; shared with the
/// output thread.
#[derive(Default)]
pub struct EarSwitch {
    pub on: AtomicBool,
    pub howled: AtomicBool,
}

pub struct Piano {
    voices: Voices,
    capture: audio::Shared,
    pub ear: Arc<EarSwitch>,
    /// New notes ring on as a drone (see `Voice::held`).
    hold: bool,
    /// Whether to play through PipeWire (not with a test signal).
    audible: bool,
    output: Option<pw::channel::Sender<()>>,
}

impl Piano {
    pub fn new(voices: Voices, capture: audio::Shared, audible: bool) -> Self {
        Self { voices, capture, ear: Arc::default(), hold: false, audible, output: None }
    }

    /// Whether notes ring on as a drone; off, a drone ringing fades out.
    pub fn set_hold(&mut self, on: bool) {
        self.hold = on;
        if !on {
            for v in self.voices.lock().unwrap().iter_mut().filter(|v| v.held && v.cut.is_none()) {
                v.cut = Some(v.n);
            }
        }
    }

    /// Fades out everything ringing.
    pub fn stop(&mut self) {
        for v in self.voices.lock().unwrap().iter_mut().filter(|v| v.cut.is_none()) {
            v.cut = Some(v.n);
        }
    }

    /// Mixes the microphone into the output, or stops.
    pub fn set_ear(&mut self, on: bool) {
        self.ear.howled.store(false, Ordering::Relaxed);
        self.ear.on.store(on, Ordering::Relaxed);
        if on {
            self.wake();
        }
    }

    /// Plays a note, fading out whatever was ringing.
    pub fn play(&mut self, hz: f32) {
        self.start(hz, true);
    }

    /// Plays a note alongside the ones ringing: a chord.
    pub fn add(&mut self, hz: f32) {
        self.start(hz, false);
    }

    fn start(&mut self, hz: f32, cut: bool) {
        {
            let mut voices = self.voices.lock().unwrap();
            if voices.len() >= MAX_VOICES {
                voices.remove(0);
            }
            if cut {
                for v in voices.iter_mut().filter(|v| v.cut.is_none()) {
                    v.cut = Some(v.n);
                }
            }
            voices.push(Voice::new(hz as f64, self.hold));
        }
        self.wake();
    }

    /// Starts the output, or wakes it from a pause.
    fn wake(&mut self) {
        if !self.audible {
            return;
        }
        match &self.output {
            Some(wake) => {
                let _ = wake.send(());
            }
            None => {
                let (wake, woken) = pw::channel::channel();
                let (voices, capture, ear) = (self.voices.clone(), self.capture.clone(), self.ear.clone());
                thread::Builder::new()
                    .name("piano".into())
                    .spawn(move || {
                        if let Err(e) = run(voices, capture, ear, woken) {
                            eprintln!("guitar: piano output failed: {e}");
                        }
                    })
                    .expect("failed to spawn piano thread");
                self.output = Some(wake);
            }
        }
    }
}

fn run(voices: Voices, capture: audio::Shared, ear: Arc<EarSwitch>, woken: pw::channel::Receiver<()>) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "guitar-piano",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::NODE_NAME => "guitar-piano",
            *pw::keys::APP_NAME => "guitar",
            *pw::keys::AUDIO_CHANNELS => "2",
            // Small buffers, for the ear: what you hear of yourself comes a few ms late.
            *pw::keys::NODE_LATENCY => "128/48000",
        },
    )?;

    let render_voices = voices.clone();
    let render_ear = ear.clone();
    let mut hearing = Ear::default();
    let mut mono = Vec::new();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let requested = buffer.requested() as usize;
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else { return };
            const STRIDE: usize = 2 * 4;
            let frames = match data.data() {
                Some(bytes) => {
                    // Only what the graph asked for: filling the whole buffer adds latency.
                    let room = bytes.len() / STRIDE;
                    let frames = if requested > 0 { requested.min(room) } else { room.min(1024) };
                    mono.clear();
                    mono.resize(frames, 0.0);
                    render(&mut render_voices.lock().unwrap(), &mut mono);
                    if render_ear.on.load(Ordering::Relaxed) {
                        if !hearing.mix(&capture.lock().unwrap(), &mut mono) {
                            render_ear.on.store(false, Ordering::Relaxed);
                            render_ear.howled.store(true, Ordering::Relaxed);
                        }
                    } else {
                        hearing.cursor = None;
                    }
                    for (frame, &x) in bytes.as_chunks_mut::<STRIDE>().0.iter_mut().zip(&mono) {
                        let b = x.to_le_bytes();
                        frame[..4].copy_from_slice(&b);
                        frame[4..].copy_from_slice(&b);
                    }
                    frames
                }
                None => 0,
            };
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = STRIDE as _;
            *chunk.size_mut() = (STRIDE * frames) as _;
        })
        .register()?;

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(2);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).expect("valid format pod")];
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // Pause when quiet for a while; a key press wakes it.
    let active = Rc::new(Cell::new(true));
    let quiet_since = Rc::new(Cell::new(None::<Instant>));
    let (idle_stream, idle_active) = (stream.clone(), active.clone());
    let timer = mainloop.loop_().add_timer(move |_| {
        if !idle_active.get() || !voices.lock().unwrap().is_empty() || ear.on.load(Ordering::Relaxed) {
            quiet_since.set(None);
            return;
        }
        let since = quiet_since.get().unwrap_or_else(Instant::now);
        quiet_since.set(Some(since));
        if since.elapsed() >= IDLE {
            let _ = idle_stream.set_active(false);
            idle_active.set(false);
        }
    });
    timer.update_timer(Some(Duration::from_millis(500)), Some(Duration::from_millis(500))).into_result()?;
    let _woken = woken.attach(mainloop.loop_(), move |()| {
        if !active.get() {
            let _ = stream.set_active(true);
            active.set(true);
        }
    });

    mainloop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{window_for, Detector};

    fn piano() -> Piano {
        Piano::new(Voices::default(), Arc::new(Mutex::new(Capture::new(RATE))), false)
    }

    fn note(hz: f64, secs: f64) -> Vec<f32> {
        let mut voices = vec![Voice::new(hz, false)];
        let mut out = vec![0.0; (secs * RATE as f64) as usize];
        render(&mut voices, &mut out);
        out
    }

    #[test]
    fn a_note_reads_at_its_pitch_and_dies_away() {
        let mut d = Detector::new(RATE, window_for(RATE));
        let n = d.window();
        for hz in [87.31, 146.83, 196.0, 261.63, 523.25] {
            let x = note(hz, LENGTH + 0.1);
            for at in [0.1, 0.8, 1.6] {
                let start = (at * RATE as f64) as usize;
                let p = d.detect(&x[start..start + n]).expect("a pitch");
                let cents = 1200.0 * (p.hz as f64 / hz).log2();
                assert!(cents.abs() < 2.0, "{hz} Hz at {at} s read {:.2} Hz", p.hz);
            }
            let peak = |from: f64, to: f64| {
                x[(from * RATE as f64) as usize..(to * RATE as f64) as usize].iter().fold(0.0f32, |m, v| m.max(v.abs()))
            };
            assert!(peak(0.0, 0.1) > 0.1 && peak(0.0, 0.1) < 0.6, "{hz}: loud but not clipping");
            assert!(peak(LENGTH, LENGTH + 0.1) == 0.0, "{hz}: silent after it ends");
        }
    }

    #[test]
    fn a_new_note_fades_the_last_one_out() {
        let mut piano = piano();
        let mut out = vec![0.0; RATE as usize / 2];
        piano.play(220.0);
        render(&mut piano.voices.lock().unwrap(), &mut out);
        piano.play(330.0);
        out.fill(0.0);
        render(&mut piano.voices.lock().unwrap(), &mut out);
        assert_eq!(piano.voices.lock().unwrap().len(), 1, "only the new note is left");
        let mut d = Detector::new(RATE, window_for(RATE));
        let p = d.detect(&out[out.len() - d.window()..]).expect("a pitch");
        assert!((p.hz - 330.0).abs() < 1.0, "heard {} Hz", p.hz);
    }

    #[test]
    fn notes_added_to_a_chord_ring_together() {
        let mut piano = piano();
        for hz in [130.81, 164.81, 196.0] {
            piano.add(hz);
        }
        let mut out = vec![0.0; RATE as usize / 2];
        render(&mut piano.voices.lock().unwrap(), &mut out);
        assert_eq!(piano.voices.lock().unwrap().len(), 3, "C E G all still ringing");
        assert!(out.iter().fold(0.0f32, |m, v| m.max(v.abs())) < 0.95, "soft-clipped, not hard");
        piano.play(246.94);
        render(&mut piano.voices.lock().unwrap(), &mut out);
        assert_eq!(piano.voices.lock().unwrap().len(), 1, "a note on its own cuts the chord");
    }

    #[test]
    fn finished_voices_are_dropped_and_old_ones_make_room() {
        let mut voices = vec![Voice::new(220.0, false)];
        let mut out = vec![0.0; (LENGTH * RATE as f64) as usize + 1];
        render(&mut voices, &mut out);
        assert!(voices.is_empty());

        let mut piano = piano();
        for i in 0..MAX_VOICES + 3 {
            piano.play(100.0 + i as f32);
        }
        let voices = piano.voices.lock().unwrap();
        assert_eq!(voices.len(), MAX_VOICES);
        assert_eq!(voices[0].hz, 103.0, "the oldest went first");
    }

    #[test]
    fn a_held_note_settles_into_a_drone_until_cut() {
        let mut piano = piano();
        piano.set_hold(true);
        piano.play(196.0);
        let mut out = vec![0.0; 10 * RATE as usize];
        render(&mut piano.voices.lock().unwrap(), &mut out);
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let (strike, late) = (rms(&out[..RATE as usize / 10]), rms(&out[out.len() - RATE as usize..]));
        assert!(late > 0.03 && late < strike, "softer than the strike ({strike:.3}) but still there: {late:.3}");
        let mut d = Detector::new(RATE, window_for(RATE));
        let p = d.detect(&out[out.len() - d.window()..]).expect("a pitch");
        assert!((1200.0 * (p.hz / 196.0).log2()).abs() < 1.0, "the drone reads {} Hz", p.hz);

        piano.set_hold(false);
        out.fill(0.0);
        render(&mut piano.voices.lock().unwrap(), &mut out[..RATE as usize / 5]);
        assert!(piano.voices.lock().unwrap().is_empty(), "letting go of hold fades it out");
        assert!(out[RATE as usize / 10..].iter().all(|&v| v == 0.0));

        piano.play(220.0);
        render(&mut piano.voices.lock().unwrap(), &mut out);
        assert!(piano.voices.lock().unwrap().is_empty(), "without hold, notes die away again");
    }

    #[test]
    fn the_ear_plays_the_input_a_buffer_behind() {
        let mut capture = Capture::new(RATE);
        let mut ear = Ear::default();
        let mut n = 0u64;
        let mut input = |c: &mut Capture, k: usize| {
            for _ in 0..k {
                c.push(0.001 * (n % 1000) as f32);
                n += 1;
            }
        };
        input(&mut capture, 4096);
        let mut out = vec![0.0; 128];
        let mut played = Vec::new();
        for _ in 0..50 {
            input(&mut capture, 128);
            out.fill(0.0);
            assert!(ear.mix(&capture, &mut out));
            played.extend_from_slice(&out);
        }
        // A ramp in, a ramp out, as many samples behind as the buffer and the slack.
        let behind = (128 + EAR_SLACK) as u64;
        for (i, &y) in played.iter().enumerate() {
            let x = 0.001 * ((4096 + 128 + i as u64 - behind) % 1000) as f32;
            assert!((y - x.tanh()).abs() < 1e-6, "sample {i}: {y} for {x}");
        }
        // Fallen far behind (the output stalled): it jumps back to just behind the input.
        input(&mut capture, 4096);
        out.fill(0.0);
        ear.mix(&capture, &mut out);
        assert_eq!(ear.cursor, Some(capture.written - EAR_SLACK as u64));
    }

    #[test]
    fn the_ear_turns_itself_off_when_it_howls() {
        let mut capture = Capture::new(RATE);
        let mut ear = Ear::default();
        let mut out = vec![0.0; 128];
        let mut blocks = 0;
        for i in 0.. {
            for k in 0..128 {
                capture.push(0.8 * (std::f32::consts::TAU * 1000.0 * (i * 128 + k) as f32 / RATE as f32).sin());
            }
            if !ear.mix(&capture, &mut out) {
                break;
            }
            blocks += 1;
            assert!(blocks < RATE as usize, "never turned off");
        }
        let secs = (blocks * 128) as f64 / RATE as f64;
        assert!((HOWL_SECS - 0.01..HOWL_SECS + 0.01).contains(&secs), "off after {secs:.2} s");
    }
}
