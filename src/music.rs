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

/// The frequency of a MIDI note, with A4 at `a4` Hz.
pub fn hz_of(midi: i32, a4: f32) -> f32 {
    a4 * 2f32.powf((midi as f32 - 69.0) / 12.0)
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

// Staff notation. Notes are spelled in a key (letter, alteration, octave) and placed in
// steps, one per line or space: step = octave·7 + letter, C0 = 0. Everything here is in
// sounding pitch; the treble-8 clef writes it an octave higher, so its lines (E4 G4 B4 D5 F5
// written) sound E3 G3 B3 D4 F4.

const LETTERS: [&str; 7] = ["C", "D", "E", "F", "G", "A", "B"];
const NATURAL_PC: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// The treble-8 staff's lines, bottom to top (E3 G3 B3 D4 F4).
pub const STAFF_LINES: [i32; 5] = [23, 25, 27, 29, 31];

pub struct Key {
    pub name: &'static str,
    /// The tonic's pitch class and letter (C = 0).
    tonic: i32,
    letter: usize,
    pub relative_minor: &'static str,
}

/// Major keys round the circle of fifths: sharps, then flats.
pub const KEYS: &[Key] = &[
    Key { name: "C", tonic: 0, letter: 0, relative_minor: "A minor" },
    Key { name: "G", tonic: 7, letter: 4, relative_minor: "E minor" },
    Key { name: "D", tonic: 2, letter: 1, relative_minor: "B minor" },
    Key { name: "A", tonic: 9, letter: 5, relative_minor: "F♯ minor" },
    Key { name: "E", tonic: 4, letter: 2, relative_minor: "C♯ minor" },
    Key { name: "B", tonic: 11, letter: 6, relative_minor: "G♯ minor" },
    Key { name: "F♯", tonic: 6, letter: 3, relative_minor: "D♯ minor" },
    Key { name: "D♭", tonic: 1, letter: 1, relative_minor: "B♭ minor" },
    Key { name: "A♭", tonic: 8, letter: 5, relative_minor: "F minor" },
    Key { name: "E♭", tonic: 3, letter: 2, relative_minor: "C minor" },
    Key { name: "B♭", tonic: 10, letter: 6, relative_minor: "G minor" },
    Key { name: "F", tonic: 5, letter: 3, relative_minor: "D minor" },
];

pub fn key_index(name: &str) -> usize {
    KEYS.iter().position(|k| k.name == name).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Accidental {
    Sharp,
    Flat,
    Natural,
}

impl Key {
    /// The key's alteration of each letter, C to B: +1 sharp, −1 flat, 0 natural.
    pub fn alterations(&self) -> [i32; 7] {
        let mut alt = [0; 7];
        for (degree, interval) in MAJOR.iter().enumerate() {
            let letter = (self.letter + degree) % 7;
            let a = (self.tonic + interval - NATURAL_PC[letter]).rem_euclid(12);
            alt[letter] = if a > 6 { a - 12 } else { a };
        }
        alt
    }

    fn in_key(&self, midi: i32) -> bool {
        let alt = self.alterations();
        (0..7).any(|l| (NATURAL_PC[l] + alt[l]).rem_euclid(12) == midi.rem_euclid(12))
    }

    /// The key signature as written on the treble-8 staff, left to right: (step, sign).
    pub fn signature(&self) -> Vec<(i32, Accidental)> {
        // Sharps go F C G D A E B, flats B E A D G C F, at their usual treble-clef places.
        const SHARPS: [(usize, i32); 7] = [(3, 31), (0, 28), (4, 32), (1, 29), (5, 26), (2, 30), (6, 27)];
        const FLATS: [(usize, i32); 7] = [(6, 27), (2, 30), (5, 26), (1, 29), (4, 25), (0, 28), (3, 24)];
        let alt = self.alterations();
        let sharps = SHARPS.iter().filter(|(l, _)| alt[*l] == 1).map(|&(_, s)| (s, Accidental::Sharp));
        let flats = FLATS.iter().filter(|(l, _)| alt[*l] == -1).map(|&(_, s)| (s, Accidental::Flat));
        sharps.chain(flats).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spelled {
    /// 0 = C … 6 = B.
    pub letter: usize,
    /// −1 flat, 0 natural, +1 sharp.
    pub alter: i32,
    pub octave: i32,
    /// The accidental written before the note, given the key signature.
    pub shown: Option<Accidental>,
}

impl Spelled {
    pub fn step(&self) -> i32 {
        self.octave * 7 + self.letter as i32
    }

    /// E.g. "F♯": the letter and its alteration, without the octave.
    pub fn name(&self) -> String {
        let sign = match self.alter {
            1 => "♯",
            -1 => "♭",
            _ => "",
        };
        format!("{}{sign}", LETTERS[self.letter])
    }
}

/// How `midi` is written in `key`: as the key's own note when it is one; otherwise a white
/// key the signature alters gets a natural, and a black key is a sharp in sharp keys (and C)
/// or a flat in flat keys.
pub fn spell(midi: i32, key: &Key) -> Spelled {
    let alt = key.alterations();
    let pc = midi.rem_euclid(12);
    let (letter, alter) = if let Some(l) = (0..7).find(|&l| (NATURAL_PC[l] + alt[l]).rem_euclid(12) == pc) {
        (l, alt[l])
    } else if let Some(l) = (0..7).find(|&l| NATURAL_PC[l] == pc) {
        (l, 0)
    } else if alt.iter().all(|&a| a >= 0) {
        ((0..7).find(|&l| NATURAL_PC[l] + 1 == pc).expect("a black key is a white key raised"), 1)
    } else {
        ((0..7).find(|&l| NATURAL_PC[l] - 1 == pc).expect("a black key is a white key lowered"), -1)
    };
    let shown = (alter != alt[letter]).then_some(match alter {
        1 => Accidental::Sharp,
        -1 => Accidental::Flat,
        _ => Accidental::Natural,
    });
    // The octave is the letter's: E♯4 sounds as F4, C♭4 as B3.
    Spelled { letter, alter, octave: (midi - alter).div_euclid(12) - 1, shown }
}

/// Where a (fractional) pitch sits on the staff, in steps: exactly on its line or space for
/// the key's own notes, and in proportion between them for anything else, so the pitch
/// line passes through every in-key note head.
pub fn staff_position(midi: f32, key: &Key) -> f32 {
    let mut lo = midi.floor() as i32;
    while !key.in_key(lo) {
        lo -= 1;
    }
    let mut hi = lo + 1;
    while !key.in_key(hi) {
        hi += 1;
    }
    spell(lo, key).step() as f32 + (midi - lo as f32) / (hi - lo) as f32
}

/// The ledger lines a note at `step` needs, beyond the treble-8 staff.
pub fn ledger_lines(step: i32) -> Vec<i32> {
    let (bottom, top) = (STAFF_LINES[0], STAFF_LINES[4]);
    let odd = |s: &i32| s.rem_euclid(2) == bottom.rem_euclid(2);
    if step < bottom {
        (step..bottom).filter(odd).collect()
    } else {
        (top + 1..=step).filter(odd).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> &'static Key {
        &KEYS[key_index(name)]
    }

    fn written(midi: i32, k: &str) -> (String, i32, Option<Accidental>) {
        let s = spell(midi, key(k));
        (s.name(), s.octave, s.shown)
    }

    #[test]
    fn notes_are_spelled_in_the_key() {
        use Accidental::*;
        assert_eq!(written(61, "C"), ("C♯".into(), 4, Some(Sharp)));
        assert_eq!(written(66, "G"), ("F♯".into(), 4, None), "in the key: no accidental");
        assert_eq!(written(65, "G"), ("F".into(), 4, Some(Natural)));
        assert_eq!(written(70, "F"), ("B♭".into(), 4, None));
        assert_eq!(written(61, "F"), ("D♭".into(), 4, Some(Flat)), "flat keys spell black keys as flats");
        assert_eq!(written(62, "D♭"), ("D".into(), 4, Some(Natural)));
        assert_eq!(written(65, "F♯"), ("E♯".into(), 4, None), "F♯ major's seventh is E♯");
        assert_eq!(written(48, "C"), ("C".into(), 3, None));
    }

    #[test]
    fn key_signatures_in_writing_order() {
        let steps = |k: &str| key(k).signature();
        assert!(steps("C").is_empty());
        assert_eq!(steps("D"), [(31, Accidental::Sharp), (28, Accidental::Sharp)]);
        assert_eq!(steps("E♭").iter().map(|s| s.0).collect::<Vec<_>>(), [27, 30, 26]);
        assert_eq!(steps("F♯").len(), 6);
        assert_eq!(steps("D♭").len(), 5);
    }

    #[test]
    fn staff_positions_on_treble_8() {
        let c = key("C");
        // The lines sound E3 G3 B3 D4 F4.
        let lines: Vec<i32> = [52, 55, 59, 62, 65].iter().map(|&m| spell(m, c).step()).collect();
        assert_eq!(lines, STAFF_LINES);
        // C♯ sits halfway between C and D; in-key notes exactly on their step.
        assert_eq!(staff_position(61.0, c), spell(60, c).step() as f32 + 0.5);
        assert_eq!(staff_position(64.0, c), spell(64, c).step() as f32);
        assert_eq!(staff_position(64.5, c), spell(64, c).step() as f32 + 0.5, "E to F is one semitone, one step");
        // In G, F♯ is on the F line.
        assert_eq!(staff_position(66.0, key("G")), STAFF_LINES[4] as f32);
    }

    #[test]
    fn ledger_lines_below_and_above() {
        assert!(ledger_lines(27).is_empty());
        assert!(ledger_lines(22).is_empty(), "D3 hangs below the staff");
        assert_eq!(ledger_lines(21), [21], "C3");
        assert_eq!(ledger_lines(18), [19, 21], "G2");
        assert_eq!(ledger_lines(33), [33], "A4");
        assert_eq!(ledger_lines(36), [33, 35], "D5");
    }

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
