//! `.solarum` files: the groove and the favorites (with their names) in one readable
//! text file (RON), to keep or to share. Older files have four favorites ("scenes")
//! and scene lengths and drops – they still open.

use crate::app::SLOTS;
use crate::song::Song;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

pub const EXTENSION: &str = "solarum";

/// The largest file Solarum opens: a project with every slot a different sound of the
/// longest length is about 115 MB of Base64
const MAX_BYTES: u64 = 160 << 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SolarumFile {
    /// Raised when the format changes in a way older versions can't read
    pub format: u32,
    pub song: Song,
    /// The favorites 1–12 (the name "scenes" stays so older files open)
    pub scenes: Vec<Option<Song>>,
    pub names: Vec<String>,
    /// Your own sounds in the project's slots (library ids, 0 = empty) and their audio
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
        f.sample_slots.truncate(crate::sampler::SAMPLE_SLOTS);
        f.scenes.resize(SLOTS, None);
        f.names.resize(SLOTS, String::new());
        for s in f.scenes.iter_mut().flatten() {
            s.sanitize();
        }
        Ok(f)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        write_atomic(path, self.to_text().as_bytes()).map_err(|e| e.to_string())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        // Files get shared: refuse one too big to be a project before reading it all
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_BYTES {
            return Err("too big for a Solarum file".into());
        }
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::from_text(&text)
    }
}

/// Write a file so a crash or a full disk never leaves half of it: into a temporary file
/// beside it, flushed to the disk, then renamed over the old one (the old one stays on failure)
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let written = std::fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(bytes).and_then(|_| f.sync_all()))
        .and_then(|_| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
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

    #[test]
    fn saving_replaces_the_file_whole_and_leaves_no_temporary_one() {
        let dir = std::env::temp_dir().join(format!("solarum-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("set.solarum");
        std::fs::write(&path, "old").unwrap();
        let f = SolarumFile::default();
        f.save(&path).unwrap();
        let back = SolarumFile::load(&path).unwrap();
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        // A folder where the file should go can't be replaced: the save fails, nothing is left behind
        std::fs::create_dir(dir.join("x.solarum")).unwrap();
        assert!(f.save(&dir.join("x.solarum")).is_err());
        std::fs::remove_dir(dir.join("x.solarum")).unwrap();
        let after: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back, f);
        assert_eq!(left, vec![std::ffi::OsString::from("set.solarum")]);
        assert_eq!(after, left);
    }

    #[test]
    fn refuses_a_file_too_big_to_be_a_project() {
        let dir = std::env::temp_dir().join(format!("solarum-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.solarum");
        // Sparse: the size is there without writing 160 MB
        std::fs::File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
        let err = SolarumFile::load(&path).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("too big"), "{err}");
    }
}
