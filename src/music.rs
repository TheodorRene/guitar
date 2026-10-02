//! Notes, tunings and cents. Pitches are MIDI note numbers (A4 = 69), fractional where
//! measured, relative to an adjustable A4 reference.

const SHARPS: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const FLATS: [&str; 12] = ["C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B"];

pub const A4_DEFAULT: f32 = 440.0;
pub const A4_RANGE: std::ops::RangeInclusive<f32> = 415.0..=466.0;

/// The (fractional) MIDI note of a frequency, with A4 at `a4` Hz.
pub fn midi_of(hz: f32, a4: f32) -> f32 {
    69.0 + 12.0 * (hz / a4).log2()
}

/// A note's name without the octave, e.g. "F♯".
pub fn name(midi: i32, flats: bool) -> &'static str {
    let names = if flats { &FLATS } else { &SHARPS };
    names[midi.rem_euclid(12) as usize]
}

/// Scientific octave number: middle C (MIDI 60) is C4.
pub fn octave(midi: i32) -> i32 {
    midi.div_euclid(12) - 1
}

/// Whether the note is a black key on a piano.
pub fn is_black(midi: i32) -> bool {
    matches!(midi.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

pub struct Tuning {
    pub name: &'static str,
    /// Open strings, lowest (6th) first. Empty for the chromatic tuner.
    pub strings: &'static [i32],
    /// Name notes with flats (E♭ standard), not sharps.
    pub flats: bool,
}

pub const TUNINGS: &[Tuning] = &[
    Tuning { name: "Standard", strings: &[40, 45, 50, 55, 59, 64], flats: false },
    Tuning { name: "Chromatic", strings: &[], flats: false },
    Tuning { name: "Drop D", strings: &[38, 45, 50, 55, 59, 64], flats: false },
    Tuning { name: "E♭ standard", strings: &[39, 44, 49, 54, 58, 63], flats: true },
    Tuning { name: "D standard", strings: &[38, 43, 48, 53, 57, 62], flats: false },
    Tuning { name: "Drop C", strings: &[36, 43, 48, 53, 57, 62], flats: false },
    Tuning { name: "DADGAD", strings: &[38, 45, 50, 55, 57, 62], flats: false },
    Tuning { name: "Open D", strings: &[38, 45, 50, 54, 57, 62], flats: false },
    Tuning { name: "Open G", strings: &[38, 43, 50, 55, 59, 62], flats: false },
];

pub fn tuning_index(name: &str) -> usize {
    TUNINGS.iter().position(|t| t.name == name).unwrap_or(0)
}

/// What a reading is being compared with: a string of the tuning, or the nearest note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub midi: i32,
    /// Index into the tuning's strings (0 = lowest), when tuning a string.
    pub string: Option<usize>,
}

impl Target {
    /// The target for a reading at `midi`: `locked` string if set, else the nearest string,
    /// else (chromatic) the nearest note.
    pub fn for_reading(midi: f32, tuning: &Tuning, locked: Option<usize>) -> Target {
        let string = locked.filter(|&s| s < tuning.strings.len()).or_else(|| nearest_string(tuning, midi));
        match string {
            Some(s) => Target { midi: tuning.strings[s], string: Some(s) },
            None => Target { midi: midi.round() as i32, string: None },
        }
    }

    pub fn cents(&self, midi: f32) -> f32 {
        (midi - self.midi as f32) * 100.0
    }
}

/// The string whose open note is closest to `midi`.
pub fn nearest_string(tuning: &Tuning, midi: f32) -> Option<usize> {
    (0..tuning.strings.len()).min_by(|&a, &b| {
        let d = |s: usize| (midi - tuning.strings[s] as f32).abs();
        d(a).total_cmp(&d(b))
    })
}

/// Guitarists number strings from the thinnest: the lowest of six is the 6th.
pub fn string_number(tuning: &Tuning, index: usize) -> usize {
    tuning.strings.len() - index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_octaves() {
        assert_eq!((name(69, false), octave(69)), ("A", 4));
        assert_eq!((name(60, false), octave(60)), ("C", 4));
        assert_eq!((name(40, false), octave(40)), ("E", 2));
        assert_eq!((name(39, true), octave(39)), ("E♭", 2));
        assert_eq!((name(59, false), octave(59)), ("B", 3));
        assert!(is_black(61) && !is_black(64));
    }

    #[test]
    fn midi_follows_the_reference() {
        assert!((midi_of(440.0, 440.0) - 69.0).abs() < 1e-5);
        assert!((midi_of(82.4069, 440.0) - 40.0).abs() < 1e-4);
        assert!((midi_of(432.0, 432.0) - 69.0).abs() < 1e-5);
        // 440 Hz against A4 = 442 is about 8 cents flat.
        assert!(((midi_of(440.0, 442.0) - 69.0) * 100.0 + 7.85).abs() < 0.05);
    }

    #[test]
    fn targets_pick_the_nearest_string_unless_one_is_locked() {
        let standard = &TUNINGS[tuning_index("Standard")];
        let t = Target::for_reading(44.3, standard, None);
        assert_eq!(t, Target { midi: 45, string: Some(1) });
        assert!((t.cents(44.3) + 70.0).abs() < 1e-3);
        // A low E tuned a whole tone flat still belongs to the low E when locked.
        assert_eq!(Target::for_reading(38.0, standard, Some(0)).midi, 40);
        assert_eq!(string_number(standard, 0), 6);

        let chromatic = &TUNINGS[tuning_index("Chromatic")];
        assert_eq!(Target::for_reading(61.4, chromatic, Some(2)), Target { midi: 61, string: None });
    }
}
