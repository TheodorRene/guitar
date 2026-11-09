//! Warm-ups, as a choir does them: the piano plays a short pattern (a five-note scale, an
//! arpeggio …), then it's sung back note by note, each note waiting until it's held in
//! tune; then the same a semitone higher, up the singer's range and back down.
//!
//! Everything is timed by the input's clock (`Frame::t`), so a session runs the same in a
//! test as in the app; the app carries out what `Session::step` asks of the piano.

use crate::music::{self, Key, KEYS};

pub struct Pattern {
    pub name: &'static str,
    /// Semitones above the root, in the order sung.
    pub steps: &'static [i32],
}

pub const PATTERNS: &[Pattern] = &[
    Pattern { name: "Five-note scale", steps: &[0, 2, 4, 5, 7, 5, 4, 2, 0] },
    Pattern { name: "Arpeggio", steps: &[0, 4, 7, 12, 7, 4, 0] },
    Pattern { name: "Octave leap", steps: &[0, 12, 7, 4, 0] },
    Pattern { name: "Scale", steps: &[0, 2, 4, 5, 7, 9, 11, 12, 11, 9, 7, 5, 4, 2, 0] },
];

pub fn pattern_index(name: &str) -> usize {
    PATTERNS.iter().position(|p| p.name == name).unwrap_or(0)
}

/// The range warmed up through, by default: C3 to G4, a tenor's comfortable span. It stays
/// on the piano's keys and at least an octave wide, for the patterns that span one.
pub const LOW: i32 = 48;
pub const HIGH: i32 = 67;
pub const MIN_SPAN: i32 = 12;

/// The piano plays the pattern a note every `NOTE_SECS`, lets the last ring `LAST_SECS`,
/// then cuts it; listening starts `SETTLE` later, once the analysis no longer hears it
/// (its window, the median and the cut's fade). After the last note sung, `REST` before
/// the next.
pub const NOTE_SECS: f64 = 0.4;
pub const LAST_SECS: f64 = 0.8;
pub const SETTLE: f64 = 0.4;
pub const REST: f64 = 1.0;
/// A note counts as sung once the last `HOLD` s of pitch all stay within `WANDER` cents of
/// it (no gaps, no other note) and their median is within `TOLERANCE`: the median, so
/// vibrato around the note counts as on it, and the glide into it (smeared over a few
/// frames by the analysis) doesn't pull it towards the note before.
pub const HOLD: f64 = 0.3;
pub const TOLERANCE: f32 = 25.0;
pub const WANDER: f32 = 75.0;

/// The roots, one per time through: a semitone up each time, from where the pattern's
/// lowest note is `low` to where its highest is `high`, then back down.
pub fn roots(steps: &[i32], low: i32, high: i32) -> Vec<i32> {
    let (bottom, top) = (steps.iter().min().copied().unwrap_or(0), steps.iter().max().copied().unwrap_or(0));
    let first = low - bottom;
    let last = (high - top).max(first);
    (first..=last).chain((first..last).rev()).collect()
}

/// The major key on `root`, for writing the pattern: its own key signature each time.
pub fn key_of(root: i32) -> &'static Key {
    &KEYS[music::fifths_position(root)]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// The piano playing the pattern from `from`; `played` notes of it so far.
    Playing { from: f64, played: usize },
    /// Waiting for the next note to be sung, in pitch from `from` on.
    Listening { from: f64 },
    /// All sung; the next time through starts `REST` after `from`.
    Resting { from: f64 },
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Play(i32),
    /// Cut the piano, so it isn't heard as the singing.
    Stop,
}

pub struct Session {
    pub pattern: usize,
    pub roots: Vec<i32>,
    /// Which time through, an index into `roots`.
    pub rep: usize,
    pub phase: Phase,
    /// The notes sung so far this time: cents off (see `hit`), or none if skipped.
    pub sung: Vec<Option<f32>>,
}

impl Session {
    pub fn new(pattern: usize, low: i32, high: i32, now: f64) -> Self {
        let roots = roots(PATTERNS[pattern].steps, low, high);
        Self { pattern, roots, rep: 0, phase: Phase::Playing { from: now, played: 0 }, sung: Vec::new() }
    }

    pub fn root(&self) -> i32 {
        self.roots[self.rep.min(self.roots.len() - 1)]
    }

    pub fn key(&self) -> &'static Key {
        key_of(self.root())
    }

    /// This time through's notes.
    pub fn notes(&self) -> Vec<i32> {
        notes(self.pattern, self.root())
    }

    /// The note to sing now, while listening.
    pub fn target(&self) -> Option<i32> {
        match self.phase {
            Phase::Listening { .. } => self.notes().get(self.sung.len()).copied(),
            _ => None,
        }
    }

    /// Hear this time through again, from the top.
    pub fn replay(&mut self, now: f64) {
        if self.phase != Phase::Finished {
            self.sung.clear();
            self.phase = Phase::Playing { from: now, played: 0 };
        }
    }

    /// Give up on the note being waited for.
    pub fn skip(&mut self, now: f64) {
        if matches!(self.phase, Phase::Listening { .. }) {
            self.sung.push(None);
            self.next_note(now);
        }
    }

    fn next_note(&mut self, now: f64) {
        self.phase = if self.sung.len() >= self.notes().len() { Phase::Resting { from: now } } else { Phase::Listening { from: now } };
    }

    /// Moves on to `now`, given the pitch heard lately (input time, MIDI when voiced, oldest
    /// first); returns what the piano should do.
    pub fn step(&mut self, now: f64, pitch: &[(f64, Option<f32>)]) -> Vec<Action> {
        let mut actions = Vec::new();
        let notes = self.notes();
        match self.phase {
            Phase::Playing { from, mut played } => {
                while played < notes.len() && now >= from + played as f64 * NOTE_SECS {
                    actions.push(Action::Play(notes[played]));
                    played += 1;
                }
                let end = from + (notes.len() - 1) as f64 * NOTE_SECS + LAST_SECS;
                self.phase = if played == notes.len() && now >= end {
                    actions.push(Action::Stop);
                    Phase::Listening { from: now + SETTLE }
                } else {
                    Phase::Playing { from, played }
                };
            }
            Phase::Listening { from } => {
                let heard = &pitch[pitch.partition_point(|p| p.0 < from)..];
                if let Some(cents) = hit(heard, notes[self.sung.len()]) {
                    self.sung.push(Some(cents));
                    self.next_note(now);
                }
            }
            Phase::Resting { from } if now >= from + REST => {
                self.rep += 1;
                self.sung.clear();
                self.phase = if self.rep >= self.roots.len() { Phase::Finished } else { Phase::Playing { from: now, played: 0 } };
            }
            Phase::Resting { .. } | Phase::Finished => {}
        }
        actions
    }
}

/// A pattern's notes from `root`.
pub fn notes(pattern: usize, root: i32) -> Vec<i32> {
    PATTERNS[pattern].steps.iter().map(|s| root + s).collect()
}

/// Whether the end of `pitch` holds `target` (see `HOLD`): if so, how many cents off it
/// was (the median).
pub fn hit(pitch: &[(f64, Option<f32>)], target: i32) -> Option<f32> {
    let (first, last) = (pitch.first()?.0, pitch.last()?.0);
    let start = last - HOLD;
    // Not long enough yet (a frame's slack, as frames come every 10 ms).
    if first > start + 0.015 {
        return None;
    }
    let mut cents = Vec::new();
    for &(_, m) in &pitch[pitch.partition_point(|p| p.0 < start)..] {
        let c = (m? - target as f32) * 100.0;
        if c.abs() > WANDER {
            return None;
        }
        cents.push(c);
    }
    cents.sort_by(f32::total_cmp);
    let median = cents[cents.len() / 2];
    (median.abs() <= TOLERANCE).then_some(median)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{Capture, Signal, Synth, WARMUP_CENTS};
    use crate::track::Analyser;

    #[test]
    fn the_roots_climb_the_range_and_come_back_down() {
        let r = roots(PATTERNS[0].steps, 48, 67);
        assert_eq!(r.len(), 25);
        assert_eq!((r[0], r[12], r[24]), (48, 60, 48));
        // The octave patterns stop an octave under the top.
        assert_eq!(*roots(PATTERNS[1].steps, 48, 67).iter().max().unwrap(), 55);
        // Too narrow a range still gets sung once.
        assert_eq!(roots(PATTERNS[3].steps, 48, 55), [48]);
        assert_eq!(key_of(49).name, "D♭");
        assert_eq!(key_of(54).name, "F♯");
    }

    fn held(from: f64, secs: f64, midi: impl Fn(f64) -> Option<f32>) -> Vec<(f64, Option<f32>)> {
        (0..(secs * 100.0) as usize).map(|i| from + i as f64 * 0.01).map(|t| (t, midi(t))).collect()
    }

    #[test]
    fn a_note_counts_once_held_close_enough_vibrato_and_all() {
        let vibrato = held(0.0, 0.5, |t| Some(55.0 + 0.3 * (std::f64::consts::TAU * 5.5 * t).sin() as f32));
        assert!(hit(&vibrato, 55).is_some_and(|c| c.abs() < 8.0));
        assert_eq!(hit(&held(0.0, 0.2, |_| Some(55.0)), 55), None, "not long enough");
        assert_eq!(hit(&held(0.0, 0.5, |_| Some(55.3)), 55), None, "30 cents sharp");
        assert!(hit(&held(0.0, 0.5, |_| Some(54.8)), 55).is_some_and(|c| (c + 20.0).abs() < 0.01));
        // A scoop up into the note is left out of how close it was.
        let scoop = held(0.0, 0.5, |t| Some(if t < 0.25 { 54.4 } else { 55.1 }));
        assert!(hit(&scoop, 55).is_some_and(|c| (c - 10.0).abs() < 0.01));
        assert_eq!(hit(&held(0.0, 0.5, |t| (t < 0.3 || t > 0.32).then_some(55.0)), 55), None, "a dropout");
        // Coming from another note: counts once the last of it has gone by.
        let slide = held(0.0, 0.6, |t| Some(if t < 0.25 { 53.0 } else { 55.0 }));
        assert_eq!(hit(&slide[..50], 55), None);
        assert!(hit(&slide, 55).is_some());
    }

    #[test]
    fn a_session_plays_listens_and_moves_up() {
        let mut s = Session::new(0, 48, 67, 0.0);
        let mut played = Vec::new();
        let mut t = 0.0;
        while matches!(s.phase, Phase::Playing { .. }) {
            played.extend(s.step(t, &[]));
            t += 0.01;
        }
        let mut expected: Vec<Action> = notes(0, 48).into_iter().map(Action::Play).collect();
        expected.push(Action::Stop);
        assert_eq!(played, expected);
        // Sing each note half a second, once the piano has gone; the third one too flat, so
        // it's skipped.
        t += SETTLE;
        let mut pitch = Vec::new();
        for (i, &m) in s.notes().iter().enumerate() {
            let off = if i == 2 { -0.4 } else { 0.05 };
            for _ in 0..50 {
                pitch.push((t, Some(m as f32 + off)));
                s.step(t, &pitch);
                t += 0.01;
            }
            if i == 2 {
                assert_eq!(s.sung.len(), 2, "waits for the flat note");
                s.skip(t);
            }
        }
        assert!(matches!(s.phase, Phase::Resting { .. }));
        assert_eq!(s.sung.len(), 9);
        assert_eq!(s.sung[2], None);
        assert!(s.sung.iter().enumerate().filter(|(i, _)| *i != 2).all(|(_, c)| c.is_some_and(|c| (c - 5.0).abs() < 0.1)));
        s.step(t + REST, &pitch);
        assert_eq!((s.rep, s.root(), s.sung.len()), (1, 49, 0));
        assert!(matches!(s.phase, Phase::Playing { .. }));
    }

    /// The test singer (`Signal::Warmup`) through the capture and analysis as in the app,
    /// with the piano's notes mixed in as if the microphone heard them: each time through is
    /// heard as sung, in the singer's tuning, and the piano never counts as singing.
    #[test]
    fn the_test_singer_warms_up_over_the_piano() {
        let voices = crate::piano::Voices::default();
        let mut piano = crate::piano::Piano::new(voices.clone(), std::sync::Arc::new(std::sync::Mutex::new(Capture::new(Synth::RATE))), false);
        let mut synth = Synth::new(Signal::Warmup);
        let mut capture = Capture::new(Synth::RATE);
        let mut analyser = Analyser::new(Synth::RATE, 0);
        let mut heard = vec![0.0; 480];
        let mut pitch: Vec<(f64, Option<f32>)> = Vec::new();
        let mut session: Option<Session> = None;
        let mut done = Vec::new();
        while capture.written < 41 * Synth::RATE as u64 {
            heard.fill(0.0);
            crate::piano::render(&mut voices.lock().unwrap(), &mut heard);
            for &h in &heard {
                capture.push(synth.next() + h);
            }
            for w in analyser.take(&capture) {
                let f = analyser.analyse(w);
                pitch.push((f.t, f.hz.map(|hz| music::midi_of(hz, 440.0))));
                // Started a second in, as the headless runner presses Enter.
                let s = session.get_or_insert_with(|| Session::new(0, LOW, HIGH, f.t.max(1.0)));
                let before = (s.rep, s.sung.clone());
                for a in s.step(f.t, &pitch) {
                    match a {
                        Action::Play(m) => piano.play(music::hz_of(m, 440.0)),
                        Action::Stop => piano.stop(),
                    }
                }
                if s.rep != before.0 {
                    done.push(before.1);
                }
            }
        }
        assert_eq!(done.len(), 3, "times through in 41 s");
        for sung in &done {
            assert_eq!(sung.len(), WARMUP_CENTS.len());
            for (c, want) in sung.iter().zip(WARMUP_CENTS) {
                let c = c.expect("sung");
                assert!((c - want as f32).abs() < 6.0, "{c:.1} cents, sung {want}: {sung:?}");
            }
        }
    }
}
