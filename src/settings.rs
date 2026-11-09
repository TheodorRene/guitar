//! Settings kept between runs, in `$XDG_STATE_HOME/guitar/settings.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{music, warmup};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Tuning the guitar: string targets, a strict needle, cents over time.
    #[default]
    Tune,
    /// Singing: nearest note, and the pitch drawn over a piano roll.
    Sing,
    /// Warm-ups: a pattern played, then sung back note by note, up the range and down.
    Warmup,
    /// The circle of fifths: the keys, the chosen key's chords and notes.
    Circle,
}

/// What the sing view draws the pitch on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SingView {
    /// A treble-8 (tenor) staff with note heads.
    #[default]
    Staff,
    /// A piano roll, one row per semitone.
    Roll,
}

/// What the sing view has under the staff: a fretboard or a keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Instrument {
    /// The fretboard of the tuning, for finding a note on the guitar.
    #[default]
    Guitar,
    Piano,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    /// A name from `music::TUNINGS`.
    pub tuning: String,
    /// Reference pitch for A4, Hz.
    pub a4: f32,
    /// Semitones shown top to bottom in the piano roll.
    pub span: f32,
    pub sing_view: SingView,
    /// A name from `music::KEYS`: the staff's key signature and how notes are spelled.
    pub key: String,
    pub instrument: Instrument,
    pub clef: music::Clef,
    /// The piano's notes ring on as a drone until the next (see `piano::Voice::held`).
    pub drone: bool,
    /// A name from `warmup::PATTERNS`, and the range warmed up through (MIDI).
    pub warmup_pattern: String,
    pub warmup_low: i32,
    pub warmup_high: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Tune,
            tuning: music::TUNINGS[0].name.into(),
            a4: music::A4_DEFAULT,
            span: 24.0,
            sing_view: SingView::Staff,
            key: music::KEYS[0].name.into(),
            instrument: Instrument::Guitar,
            clef: music::Clef::Treble8,
            drone: false,
            warmup_pattern: warmup::PATTERNS[0].name.into(),
            warmup_low: warmup::LOW,
            warmup_high: warmup::HIGH,
        }
    }
}

/// A warm-up range on the piano's keys and at least `warmup::MIN_SPAN` wide, keeping the
/// low end where it can.
pub fn warmup_range(low: i32, high: i32) -> (i32, i32) {
    let low = low.clamp(crate::piano::LOW, crate::piano::HIGH - warmup::MIN_SPAN);
    (low, high.clamp(low + warmup::MIN_SPAN, crate::piano::HIGH))
}

pub const SPAN_RANGE: std::ops::RangeInclusive<f32> = 12.0..=48.0;

fn path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("guitar/settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        let mut s: Settings = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        s.a4 = s.a4.clamp(*music::A4_RANGE.start(), *music::A4_RANGE.end());
        s.span = s.span.clamp(*SPAN_RANGE.start(), *SPAN_RANGE.end());
        (s.warmup_low, s.warmup_high) = warmup_range(s.warmup_low, s.warmup_high);
        s
    }

    /// Writes the settings (to a temporary file first, so a crash never leaves half a file).
    pub fn save(&self) {
        let Some(path) = path() else { return };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().expect("settings dir"))?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(self).expect("settings serialise"))?;
            std::fs::rename(tmp, &path)
        };
        if let Err(e) = write() {
            eprintln!("guitar: couldn't save settings to {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{ "mode": "sing" }"#).unwrap();
        assert_eq!(s.mode, Mode::Sing);
        assert_eq!(s.a4, 440.0);
        assert_eq!(s.tuning, "Standard");
        assert_eq!((s.sing_view, s.key.as_str()), (SingView::Staff, "C"));
        assert_eq!((s.instrument, s.clef), (Instrument::Guitar, music::Clef::Treble8));
        let bass: Settings = serde_json::from_str(r#"{ "clef": "bass" }"#).unwrap();
        assert_eq!(bass.clef, music::Clef::Bass);
        let treble: Settings = serde_json::from_str(r#"{ "clef": "treble" }"#).unwrap();
        assert_eq!(treble.clef, music::Clef::Treble);
        assert_eq!((s.warmup_low, s.warmup_high, s.warmup_pattern.as_str()), (48, 67, "Five-note scale"));
        let warm: Settings = serde_json::from_str(r#"{ "mode": "warmup" }"#).unwrap();
        assert_eq!(warm.mode, Mode::Warmup);
        assert_eq!(warmup_range(70, 60), (60, 72));
        assert_eq!(warmup_range(50, 55), (50, 62));
    }
}
