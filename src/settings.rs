//! Settings kept between runs, in `$XDG_STATE_HOME/guitar/settings.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::music;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Tuning the guitar: string targets, a strict needle, cents over time.
    #[default]
    Tune,
    /// Singing: nearest note, and the pitch drawn over a piano roll.
    Sing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    /// A name from `music::TUNINGS`.
    pub tuning: String,
    /// Reference pitch for A4, Hz.
    pub a4: f32,
    /// Semitones shown top to bottom in the sing view.
    pub span: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: Mode::Tune, tuning: music::TUNINGS[0].name.into(), a4: music::A4_DEFAULT, span: 24.0 }
    }
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
    }
}
