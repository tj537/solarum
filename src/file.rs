//! `.solarum` files: the groove and the favorites (with their names) in one readable
//! text file (RON), to keep or to share. Older files have four favorites ("scenes")
//! and scene lengths and drops – they still open.

use crate::app::SLOTS;
use crate::song::Song;
use serde::{Deserialize, Serialize};

pub const EXTENSION: &str = "solarum";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SolarumFile {
    /// Raised when the format changes in a way older versions can't read
    pub format: u32,
    pub song: Song,
    /// The favorites 1–12 (the name "scenes" stays so older files open)
    pub scenes: Vec<Option<Song>>,
    pub names: Vec<String>,
    /// Your own sounds in the eight slots (library ids, 0 = empty) and their audio
    pub sample_slots: Vec<u64>,
    pub samples: crate::library::Packed,
}

impl Default for SolarumFile {
    fn default() -> Self {
        SolarumFile {
            format: 1,
            song: Song::default(),
            scenes: vec![None; SLOTS],
            names: vec![String::new(); SLOTS],
            sample_slots: Vec::new(),
            samples: Default::default(),
        }
    }
}

impl SolarumFile {
    pub fn to_text(&self) -> String {
        let pretty = ron::ser::PrettyConfig::new().depth_limit(3);
        ron::ser::to_string_pretty(self, pretty).unwrap_or_default()
    }

    /// Read a file, repairing anything out of range
    pub fn from_text(text: &str) -> Result<Self, String> {
        let mut f: SolarumFile = ron::from_str(text).map_err(|e| format!("not a Solarum file ({e})"))?;
        f.song.sanitize();
        f.scenes.resize(SLOTS, None);
        f.names.resize(SLOTS, String::new());
        for s in f.scenes.iter_mut().flatten() {
            s.sanitize();
        }
        Ok(f)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        std::fs::write(path, self.to_text()).map_err(|e| e.to_string())
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::from_text(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen;
    use crate::rng::Rng;
    use crate::song::Vibe;

    #[test]
    fn round_trips() {
        let mut f = SolarumFile {
            song: gen::generate(Vibe::Dub, &Default::default(), &mut Rng::new(1)),
            ..Default::default()
        };
        f.scenes[2] = Some(gen::generate(Vibe::Hard, &Default::default(), &mut Rng::new(2)));
        f.song.tracks[3].tune = -5;
        f.song.tracks[3].lock = true;
        let back = SolarumFile::from_text(&f.to_text()).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn repairs_and_rejects() {
        assert!(SolarumFile::from_text("hello").is_err());
        // Missing fields fall back to defaults, broken values are repaired, old fields are ignored
        let f = SolarumFile::from_text("(song: (bpm: 999.0), scene_bars: (0, 8, 8, 200))").unwrap();
        assert_eq!(f.song.bpm, 180.0);
        // A file from before 0.4.2 has four favorites: they come in as 1–4 of twelve
        let old = SolarumFile::from_text("(scenes: [None, Some(()), None, None])").unwrap();
        assert_eq!(old.scenes.len(), SLOTS);
        assert!(old.scenes[1].is_some() && old.scenes[4..].iter().all(|s| s.is_none()));
        assert_eq!(old.names.len(), SLOTS);
    }
}
