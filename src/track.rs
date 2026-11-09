//! Runs the pitch detector over the input 100 times a second and keeps the readings.
//!
//! Each frame looks at the newest window of input (about 43 ms). A frame counts as voiced
//! when it is loud and periodic enough; once voiced, lower thresholds keep it so while a
//! string rings out. A median over the last three frames removes single-frame slips (an
//! octave jump during a pluck's attack, say), at the cost of showing a moving pitch one
//! frame late.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::{self, Capture};
use crate::pitch::{window_for, Detector, Pitch};

pub const FRAMES_PER_SEC: u32 = 100;
/// History kept, in seconds: more than any view shows.
const KEEP_SECS: f64 = 20.0;

/// Clarity (NSDF peak) a frame needs to become voiced, and to stay voiced.
const CLARITY_ON: f32 = 0.88;
const CLARITY_HOLD: f32 = 0.75;
/// Level (dBFS over the window) a frame needs to become voiced, and to stay voiced.
pub const LEVEL_ON: f32 = -58.0;
const LEVEL_HOLD: f32 = -66.0;

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Seconds of input before the end of this frame's window (the input's own clock).
    pub t: f64,
    /// The pitch, when voiced.
    pub hz: Option<f32>,
    pub level_db: f32,
}

pub struct Track {
    pub frames: VecDeque<Frame>,
    /// When the newest frame was made, so views can scroll smoothly between frames.
    pub newest_at: Option<Instant>,
    pub received: bool,
    pub error: Option<String>,
}

pub type Shared = Arc<Mutex<Track>>;

pub fn start(capture: audio::Shared) -> Shared {
    let track = Arc::new(Mutex::new(Track { frames: VecDeque::new(), newest_at: None, received: false, error: None }));
    let shared = track.clone();
    thread::Builder::new()
        .name("pitch".into())
        .spawn(move || run(capture, shared))
        .expect("failed to spawn pitch thread");
    track
}

fn run(capture: audio::Shared, track: Shared) {
    let mut analyser = Analyser::new(48_000, 0);
    loop {
        thread::sleep(Duration::from_millis(4));
        let (due, received, error) = {
            let c = capture.lock().unwrap();
            if c.rate() != analyser.rate {
                analyser = Analyser::new(c.rate(), c.written);
                track.lock().unwrap().frames.clear();
            }
            (analyser.take(&c), c.received, c.error.clone())
        };
        let frames: Vec<Frame> = due.into_iter().map(|w| analyser.analyse(w)).collect();

        let mut t = track.lock().unwrap();
        t.received = received;
        t.error = error;
        if !frames.is_empty() {
            t.frames.extend(frames);
            t.newest_at = Some(Instant::now());
            let newest = t.frames.back().map_or(0.0, |f| f.t);
            while t.frames.front().is_some_and(|f| f.t < newest - KEEP_SECS) {
                t.frames.pop_front();
            }
        }
    }
}

/// A frame's input, copied out of the capture.
pub struct Window {
    end: u64,
    samples: Vec<f32>,
    level_db: f32,
}

/// Turns the capture into frames: a window every hop, detected and voiced.
pub struct Analyser {
    rate: u32,
    detector: Detector,
    voicer: Voicer,
    hop: u64,
    next_end: u64,
}

impl Analyser {
    /// Starts with the frame ending `from` samples into the capture.
    pub fn new(rate: u32, from: u64) -> Self {
        let detector = Detector::new(rate, window_for(rate));
        let next_end = from.max(detector.window() as u64);
        Self { rate, detector, voicer: Voicer::default(), hop: (rate / FRAMES_PER_SEC) as u64, next_end }
    }

    /// Seconds from a frame's window centre to its end, as the window is long.
    #[cfg(test)]
    pub fn delay(&self) -> f64 {
        self.detector.window() as f64 / 2.0 / self.rate as f64
    }

    /// Copies out the windows of every frame that is due: cheap, so it's done under the
    /// capture's lock, and the detection after.
    pub fn take(&mut self, c: &Capture) -> Vec<Window> {
        // Fallen far behind (a long stall): skip to the present.
        if c.written > self.next_end + audio::HISTORY as u64 / 2 {
            self.next_end = c.written;
        }
        let n = self.detector.window();
        let mut due = Vec::new();
        while self.next_end <= c.written {
            let mut samples = vec![0.0; n];
            if let (true, Some(level_db)) = (c.window_ending(self.next_end, &mut samples), c.level_ending(self.next_end, n)) {
                due.push(Window { end: self.next_end, samples, level_db });
            }
            self.next_end += self.hop;
        }
        due
    }

    pub fn analyse(&mut self, w: Window) -> Frame {
        let hz = self.voicer.push(self.detector.detect(&w.samples), w.level_db);
        Frame { t: w.end as f64 / self.rate as f64, hz, level_db: w.level_db }
    }
}

/// Decides which frames are voiced and smooths them (see the module notes).
#[derive(Default)]
pub struct Voicer {
    recent: [Option<f32>; 3],
    voiced: bool,
}

impl Voicer {
    pub fn push(&mut self, raw: Option<Pitch>, level_db: f32) -> Option<f32> {
        let (clarity, level) = if self.voiced { (CLARITY_HOLD, LEVEL_HOLD) } else { (CLARITY_ON, LEVEL_ON) };
        let hz = raw.filter(|p| p.clarity >= clarity && level_db >= level).map(|p| p.hz);
        self.voiced = hz.is_some();
        self.recent.rotate_left(1);
        self.recent[2] = hz;
        let mut voiced: Vec<f32> = self.recent.iter().flatten().copied().collect();
        // A single voiced frame on its own is a blip: it takes two of the last three.
        match (hz, voiced.len()) {
            (Some(_), 3) => {
                voiced.sort_by(f32::total_cmp);
                Some(voiced[1])
            }
            (Some(h), 2) => Some(h),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{Signal, Synth, PLUCK_SECS, STRINGS};

    fn p(hz: f32) -> Option<Pitch> {
        Some(Pitch { hz, clarity: 0.95 })
    }

    #[test]
    fn a_lone_blip_is_dropped_and_an_octave_slip_is_smoothed_over() {
        let mut v = Voicer::default();
        assert_eq!(v.push(p(110.0), -30.0), None, "one frame is a blip");
        assert_eq!(v.push(None, -30.0), None);
        assert_eq!(v.push(p(110.0), -30.0), Some(110.0), "two of the last three");
        assert_eq!(v.push(p(110.0), -30.0), Some(110.0));
        assert_eq!(v.push(p(220.0), -30.0), Some(110.0), "octave slip");
        assert_eq!(v.push(p(110.0), -30.0), Some(110.0));
    }

    #[test]
    fn quiet_or_unclear_frames_are_unvoiced_but_a_ringing_note_holds_on() {
        let mut v = Voicer::default();
        assert_eq!(v.push(p(110.0), -70.0), None, "too quiet to start");
        assert_eq!(v.push(Some(Pitch { hz: 110.0, clarity: 0.8 }), -30.0), None, "too unclear to start");
        for _ in 0..3 {
            v.push(p(110.0), -30.0);
        }
        // Fading out: quieter and less clear than it could start at, but still held.
        assert_eq!(v.push(Some(Pitch { hz: 110.0, clarity: 0.8 }), -62.0), Some(110.0));
        assert_eq!(v.push(Some(Pitch { hz: 110.0, clarity: 0.7 }), -62.0), None);
    }

    /// Runs `secs` of `next` through the capture and analysis as the app does: (time of the
    /// window's centre, voiced pitch) per frame.
    fn readings(mut next: impl FnMut() -> f32, secs: f64) -> Vec<(f64, Option<f32>)> {
        let mut capture = Capture::new(Synth::RATE);
        let mut analyser = Analyser::new(Synth::RATE, 0);
        let mut out = Vec::new();
        while capture.written < (secs * Synth::RATE as f64) as u64 {
            for _ in 0..480 {
                capture.push(next());
            }
            for w in analyser.take(&capture) {
                let f = analyser.analyse(w);
                out.push((f.t - analyser.delay(), f.hz));
            }
        }
        out
    }

    fn cents(hz: f32, truth: f64) -> f32 {
        (1200.0 * (hz as f64 / truth).log2()) as f32
    }

    #[test]
    fn the_test_guitar_reads_steadily_as_each_string_and_its_detuning() {
        let mut synth = Synth::new(Signal::Guitar);
        let frames = readings(|| synth.next(), 16.0);
        let truth = Synth::new(Signal::Guitar);
        for i in 0..STRINGS.len() {
            // From just after the pluck to when the next string comes in, fading tail included.
            let start = i as f64 * PLUCK_SECS;
            let note: Vec<_> = frames.iter().filter(|(t, _)| (start + 0.1..start + PLUCK_SECS - 0.05).contains(t)).collect();
            let voiced: Vec<f32> = note.iter().filter_map(|&&(t, hz)| Some(cents(hz?, truth.truth(t).0))).collect();
            assert!(voiced.len() * 20 >= note.len() * 19, "string {i}: {} of {} frames voiced", voiced.len(), note.len());
            let worst = voiced.iter().fold(0.0f32, |m, c| m.max(c.abs()));
            assert!(worst < 2.5, "string {i}: up to {worst:.2} cents out");
        }
        // The quiet second after the sixth string: nothing voiced.
        let quiet = 6.0 * PLUCK_SECS;
        assert!(frames.iter().filter(|(t, _)| (quiet + 0.1..quiet + 0.9).contains(t)).all(|(_, hz)| hz.is_none()));
    }

    #[test]
    fn the_test_voice_is_followed_through_glides_and_vibrato() {
        let mut synth = Synth::new(Signal::Voice);
        let frames = readings(|| synth.next(), 8.0);
        let truth = Synth::new(Signal::Voice);
        // On a moving pitch, the median of three frames gives the middle one: a frame late.
        let late = 1.0 / FRAMES_PER_SEC as f64;
        let sung: Vec<_> = frames.iter().filter(|(t, _)| truth.truth(*t).1 > 0.2).collect();
        let mut errors: Vec<f32> = sung.iter().filter_map(|&&(t, hz)| Some(cents(hz?, truth.truth(t - late).0).abs())).collect();
        assert!(errors.len() * 10 >= sung.len() * 9, "only {} of {} sung frames voiced", errors.len(), sung.len());
        errors.sort_by(f32::total_cmp);
        let p = |q: f32| errors[((errors.len() - 1) as f32 * q) as usize];
        assert!(p(0.5) < 1.5 && p(0.95) < 8.0, "median {:.1}, 95th {:.1} cents", p(0.5), p(0.95));
        // Rests are unvoiced.
        assert!(frames.iter().filter(|(t, _)| (3.6..3.9).contains(t)).all(|(_, hz)| hz.is_none()));
    }

    #[test]
    fn steady_tones_read_true_through_the_prefilter_from_low_b_to_high_e() {
        for hz in [61.74, 82.41, 329.63, 659.26, 1318.51] {
            let mut n = 0u64;
            let frames = readings(
                || {
                    n += 1;
                    0.3 * (std::f64::consts::TAU * hz * n as f64 / Synth::RATE as f64).sin() as f32
                },
                1.0,
            );
            let read: Vec<f32> = frames.iter().filter(|(t, _)| *t > 0.2).filter_map(|(_, f)| *f).collect();
            assert!(read.len() > 70, "{hz} Hz: only {} frames voiced", read.len());
            assert!(read.iter().all(|&f| cents(f, hz).abs() < 0.5), "{hz} Hz read as {:?}", &read[..3]);
        }
    }

    #[test]
    fn the_voice_reads_true_over_a_drone_leaking_into_the_microphone() {
        // A drone an octave under the tune's start, 18 dB down on the piano's own level:
        // far more than open-back headphones leak into a laptop's microphone. (At -6 dB
        // the drone wins.) In the rests the drone reads as sung: there it's all there is.
        let voices = crate::piano::Voices::default();
        let mut piano = crate::piano::Piano::new(voices.clone(), std::sync::Arc::new(std::sync::Mutex::new(Capture::new(Synth::RATE))), false);
        piano.set_hold(true);
        piano.play(130.81);
        let mut synth = Synth::new(Signal::Voice);
        let mut leak = Vec::new();
        let frames = readings(
            || {
                if leak.is_empty() {
                    leak.resize(480, 0.0);
                    crate::piano::render(&mut voices.lock().unwrap(), &mut leak);
                }
                synth.next() + 0.125 * leak.pop().unwrap()
            },
            12.0,
        );
        let truth = Synth::new(Signal::Voice);
        let late = 1.0 / FRAMES_PER_SEC as f64;
        let sung: Vec<_> = frames.iter().filter(|(t, _)| *t > 1.0 && truth.truth(*t).1 > 0.2).collect();
        let right = sung.iter().filter(|&&&(t, hz)| hz.is_some_and(|h| cents(h, truth.truth(t - late).0).abs() < 10.0)).count();
        assert!(right * 10 >= sung.len() * 9, "{right} of {} sung frames read right", sung.len());
    }
}
