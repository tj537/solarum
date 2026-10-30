//! Your own sounds: the library on disk (imported or recorded audio, each with its edit and its
//! track), the edits rendered into playable sounds, slicing, the microphone, and packing sounds into
//! a project file so it plays the same on another computer.

use crate::sampler::{SampleBuf, SAMPLE_SLOTS, SAMPLE_SR};
use crate::song::Voice;
use crate::synth::Svf;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Longer imports and recordings are cut here
pub const MAX_SECONDS: f32 = 30.0;
/// File types the import takes
pub const EXTENSIONS: [&str; 8] = ["wav", "wave", "aif", "aiff", "mp3", "flac", "ogg", "oga"];

/// How a sound is cut and shaped – everything is undoable and nothing touches the audio on disk
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// Seconds into the source
    pub start: f32,
    /// Seconds into the source; 0 = to the end
    pub end: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    /// dB; ignored while `normalize` is on
    pub gain: f32,
    pub normalize: bool,
    pub reverse: bool,
    /// Semitones
    pub pitch: f32,
    /// Hz; 20 = off
    pub low_cut: f32,
    /// Hz; 20 000 = off
    pub high_cut: f32,
    /// 0 = clean … 1 = 4 bits at a few kHz
    pub crush: f32,
}

impl Default for Edit {
    fn default() -> Self {
        Edit {
            start: 0.0,
            end: 0.0,
            fade_in: 0.0,
            fade_out: 0.005,
            gain: 0.0,
            normalize: true,
            reverse: false,
            pitch: 0.0,
            low_cut: 20.0,
            high_cut: 20_000.0,
            crush: 0.0,
        }
    }
}

impl Edit {
    /// Repair what a file can bring in: no NaN or infinity, every value in its range
    pub fn sanitize(&mut self) {
        let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        self.start = fix(self.start, 0.0, MAX_SECONDS, 0.0);
        self.end = fix(self.end, 0.0, MAX_SECONDS, 0.0);
        self.fade_in = fix(self.fade_in, 0.0, MAX_SECONDS, 0.0);
        self.fade_out = fix(self.fade_out, 0.0, MAX_SECONDS, 0.005);
        self.gain = fix(self.gain, -48.0, 24.0, 0.0);
        self.pitch = fix(self.pitch, -24.0, 24.0, 0.0);
        self.low_cut = fix(self.low_cut, 20.0, 20_000.0, 20.0);
        self.high_cut = fix(self.high_cut, 20.0, 20_000.0, 20_000.0);
        self.crush = fix(self.crush, 0.0, 1.0, 0.0);
    }
}

/// One sound in the library
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub id: u64,
    /// The audio it plays (copies of a sound share it)
    pub source: u64,
    pub name: String,
    /// The one track it plays on
    pub voice: Voice,
    pub edit: Edit,
    /// Unix seconds, for the newest-first list
    pub added: u64,
    /// Just recorded or imported, not saved yet: not in the list (and gone at the next start)
    pub draft: bool,
}

impl Default for Entry {
    fn default() -> Self {
        Entry {
            id: 0,
            source: 0,
            name: String::new(),
            voice: Voice::Rim,
            edit: Edit::default(),
            added: 0,
            draft: false,
        }
    }
}

/// The library's index file
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    entries: Vec<Entry>,
}

/// Audio ready to draw: the samples and min/max per block of `BLOCK` for fast waveforms
pub struct Source {
    pub data: Vec<f32>,
    pub peaks: Vec<(f32, f32)>,
}

pub const BLOCK: usize = 64;

impl Source {
    pub fn new(data: Vec<f32>) -> Self {
        let peaks = data
            .chunks(BLOCK)
            .map(|c| c.iter().fold((0.0f32, 0.0f32), |(lo, hi), &x| (lo.min(x), hi.max(x))))
            .collect();
        Source { data, peaks }
    }

    pub fn seconds(&self) -> f32 {
        self.data.len() as f32 / SAMPLE_SR
    }
}

pub struct Library {
    /// `None` in tests and the offscreen renderer: nothing is written
    dir: Option<PathBuf>,
    pub entries: Vec<Entry>,
    pub sources: HashMap<u64, Arc<Source>>,
    /// Rendered sounds by entry, with the edit they were made from
    rendered: HashMap<u64, (Edit, Arc<SampleBuf>)>,
}

/// A new id: the clock mixed with a counter, never 0
pub fn new_id() -> u64 {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let c = COUNT.fetch_add(1, Ordering::Relaxed) as u64;
    (t ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Library {
    /// The library in Solarum's data folder (`samples/`)
    pub fn open() -> Self {
        let dir = eframe::storage_dir("Solarum").map(|d| d.join("samples"));
        let mut lib = Library::in_memory();
        let Some(dir) = dir else { return lib };
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(text) = std::fs::read_to_string(dir.join("library.ron")) {
            if let Ok(index) = ron::from_str::<Index>(&text) {
                lib.entries = index.entries;
                for e in &mut lib.entries {
                    e.edit.sanitize();
                }
            }
        }
        for e in &lib.entries {
            if let std::collections::hash_map::Entry::Vacant(slot) = lib.sources.entry(e.source) {
                if let Ok(data) = decode_file(&dir.join(format!("{}.wav", e.source))) {
                    slot.insert(Arc::new(Source::new(data)));
                }
            }
        }
        lib.dir = Some(dir);
        // Drafts nobody saved
        let drafts: Vec<u64> = lib.entries.iter().filter(|e| e.draft).map(|e| e.id).collect();
        for id in drafts {
            lib.remove(id);
        }
        lib
    }

    pub fn set_draft(&mut self, id: u64, draft: bool) {
        if let Some(e) = self.get_mut(id) {
            e.draft = draft;
            self.save();
        }
    }

    pub fn in_memory() -> Self {
        Library {
            dir: None,
            entries: Vec::new(),
            sources: HashMap::new(),
            rendered: HashMap::new(),
        }
    }

    /// Write the index (cheap; called after every change that should last)
    pub fn save(&self) {
        let Some(dir) = &self.dir else { return };
        let index = Index {
            entries: self.entries.clone(),
        };
        if let Ok(text) = ron::ser::to_string_pretty(&index, Default::default()) {
            let _ = crate::file::write_atomic(&dir.join("library.ron"), text.as_bytes());
        }
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    pub fn source(&self, e: &Entry) -> Option<&Arc<Source>> {
        self.sources.get(&e.source)
    }

    /// Store new audio and make it a sound; returns its id
    pub fn add(&mut self, name: &str, voice: Voice, mut data: Vec<f32>) -> u64 {
        data.truncate((MAX_SECONDS * SAMPLE_SR) as usize);
        let source = new_id();
        if let Some(dir) = &self.dir {
            let pcm: Vec<i16> = data.iter().map(|&s| crate::wav::to_i16(s)).collect();
            let _ = crate::file::write_atomic(
                &dir.join(format!("{source}.wav")),
                &crate::wav::encode_mono_i16(&pcm, SAMPLE_SR as u32),
            );
        }
        let mut edit = Edit::default();
        let (s, e) = trim_points(&data);
        edit.start = s;
        edit.end = e;
        self.sources.insert(source, Arc::new(Source::new(data)));
        let id = new_id();
        self.entries.insert(
            0,
            Entry {
                id,
                source,
                name: name.to_string(),
                voice,
                edit,
                added: now_secs(),
                draft: false,
            },
        );
        self.save();
        id
    }

    /// Remove a sound; its audio goes too once no other sound cuts from it
    pub fn remove(&mut self, id: u64) {
        let Some(i) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        let e = self.entries.remove(i);
        self.rendered.remove(&id);
        if !self.entries.iter().any(|o| o.source == e.source) {
            self.sources.remove(&e.source);
            if let Some(dir) = &self.dir {
                let _ = std::fs::remove_file(dir.join(format!("{}.wav", e.source)));
            }
        }
        self.save();
    }

    /// The sound as it plays: rendered from its edit (cached until the edit changes)
    pub fn sound(&mut self, id: u64) -> Option<Arc<SampleBuf>> {
        let e = self.get(id)?;
        if let Some((edit, buf)) = self.rendered.get(&id) {
            if *edit == e.edit {
                return Some(buf.clone());
            }
        }
        let edit = e.edit;
        let src = self.sources.get(&e.source)?;
        let buf = Arc::new(SampleBuf {
            data: render(&src.data, &edit),
            pitch: edit.pitch,
        });
        self.rendered.insert(id, (edit, buf.clone()));
        Some(buf)
    }

    /// Pack the project's sounds (and the audio they cut from) for its file
    pub fn pack(&self, ids: &[u64]) -> Packed {
        let entries: Vec<Entry> = ids.iter().filter_map(|&id| self.get(id).cloned()).collect();
        let mut sources = Vec::new();
        for e in &entries {
            if sources.iter().any(|(id, _): &(u64, String)| *id == e.source) {
                continue;
            }
            if let Some(src) = self.sources.get(&e.source) {
                sources.push((e.source, pack_audio(&src.data)));
            }
        }
        Packed { entries, sources }
    }

    /// Bring a project's sounds into the library (those already there stay as they are).
    /// A project holds at most `SAMPLE_SLOTS` sounds; a file with more is cut off there.
    pub fn unpack(&mut self, p: &Packed) {
        let mut changed = false;
        for (id, text) in p.sources.iter().take(SAMPLE_SLOTS) {
            if self.sources.contains_key(id) {
                continue;
            }
            let Some(data) = unpack_audio(text) else { continue };
            if let Some(dir) = &self.dir {
                let pcm: Vec<i16> = data.iter().map(|&s| crate::wav::to_i16(s)).collect();
                let _ = crate::file::write_atomic(&dir.join(format!("{id}.wav")), &crate::wav::encode_mono_i16(&pcm, SAMPLE_SR as u32));
            }
            self.sources.insert(*id, Arc::new(Source::new(data)));
        }
        for e in p.entries.iter().take(SAMPLE_SLOTS) {
            if self.get(e.id).is_none() && self.sources.contains_key(&e.source) {
                let mut e = e.clone();
                e.edit.sanitize();
                self.entries.insert(0, e);
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }
}

/// A project's own sounds inside its file: the entries and their audio (16-bit, base64)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Packed {
    pub entries: Vec<Entry>,
    pub sources: Vec<(u64, String)>,
}

/// The project's slots: which library sound each of the eight plays (0 = empty)
pub type SlotIds = [u64; SAMPLE_SLOTS];

// ───────────────────────────── Rendering an edit ─────────────────────────────

/// The edit applied to the audio: cut, reversed, filtered, crushed, levelled, faded
pub fn render(src: &[f32], e: &Edit) -> Vec<f32> {
    let (a, b) = span(src.len(), e);
    let mut x: Vec<f32> = src[a..b].to_vec();
    if e.reverse {
        x.reverse();
    }
    let sr = SAMPLE_SR;
    if e.low_cut > 25.0 {
        let mut f = Svf::new(e.low_cut, 0.7, sr);
        for s in &mut x {
            *s = f.run(*s).hp;
        }
    }
    if e.high_cut < 19_000.0 {
        let mut f = Svf::new(e.high_cut, 0.7, sr);
        for s in &mut x {
            *s = f.run(*s).lp;
        }
    }
    if e.crush > 0.001 {
        let hold = 1.0 + e.crush * e.crush * 14.0;
        let levels = 2f32.powf(16.0 - e.crush * 12.0);
        let (mut held, mut t) = (0.0, hold);
        for s in &mut x {
            t += 1.0;
            if t >= hold {
                t -= hold;
                held = (*s * levels).round() / levels;
            }
            *s = held;
        }
    }
    let gain = if e.normalize {
        let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        if peak > 1e-5 {
            0.89 / peak
        } else {
            1.0
        }
    } else {
        10f32.powf(e.gain / 20.0)
    };
    let n = x.len();
    let fin = ((e.fade_in * sr) as usize).min(n);
    let fout = ((e.fade_out * sr) as usize).min(n);
    for (i, s) in x.iter_mut().enumerate() {
        let mut g = gain;
        if i < fin {
            g *= i as f32 / fin as f32;
        }
        if n - i <= fout {
            g *= (n - i) as f32 / fout as f32;
        }
        // `clamp` lets NaN through, and one NaN in the engine's filters silences everything
        let v = *s * g;
        *s = if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
    }
    x
}

/// The edit's start and end as sample indices (always at least one sample)
pub fn span(len: usize, e: &Edit) -> (usize, usize) {
    let a = ((e.start.max(0.0) * SAMPLE_SR) as usize).min(len.saturating_sub(1));
    let b = if e.end <= 0.0 {
        len
    } else {
        ((e.end * SAMPLE_SR) as usize).min(len)
    };
    (a, b.max(a + 1).min(len.max(a + 1)))
}

/// Where the sound starts and ends without the silence around it (seconds)
pub fn trim_points(x: &[f32]) -> (f32, f32) {
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak < 1e-4 {
        return (0.0, 0.0);
    }
    let floor = peak * 0.015; // about -36 dB under the peak
    let first = x.iter().position(|s| s.abs() > floor).unwrap_or(0);
    let last = x.iter().rposition(|s| s.abs() > floor).unwrap_or(x.len() - 1);
    let pre = (0.004 * SAMPLE_SR) as usize;
    let post = (0.03 * SAMPLE_SR) as usize;
    let a = first.saturating_sub(pre);
    let b = (last + post).min(x.len());
    (a as f32 / SAMPLE_SR, if b >= x.len() { 0.0 } else { b as f32 / SAMPLE_SR })
}

/// Guess a track from a file name (kick, snare, hat …); unknown sounds become percussion
pub fn guess_voice(name: &str) -> Voice {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["kick", "bd", "bassdrum"]) {
        Voice::Kick
    } else if has(&["open", "oh"]) && has(&["hat", "hh", "oh"]) {
        Voice::OpenHat
    } else if has(&["hat", "hh", "shaker"]) {
        Voice::Hat
    } else if has(&["clap", "snare", "sd", "snap"]) {
        Voice::Clap
    } else if has(&["ride", "crash", "cymbal"]) {
        Voice::Ride
    } else if has(&["bass", "sub", "808"]) {
        Voice::Bass
    } else if has(&["synth", "chord", "stab", "pad", "lead", "key", "pluck", "vox", "vocal"]) {
        Voice::Acid
    } else {
        Voice::Rim
    }
}

// ───────────────────────────── Drawn sounds ─────────────────────────────

/// Columns of a drawing: left to right is time
pub const DRAW_COLUMNS: usize = 256;

/// A sound drawn by hand: in every column the pitch (0 = low … 1 = high) or nothing (silence)
#[derive(Clone, PartialEq)]
pub struct Drawing {
    pub pitch: Vec<Option<f32>>,
    /// How loud every column is (0 … 1) – painted in the lane under the line
    pub level: Vec<f32>,
    /// 0 = soft (a sine with a little overtone), 1 = bright (a saw), 2 = air (filtered noise)
    pub wave: u8,
    /// Seconds
    pub length: f32,
}

impl Default for Drawing {
    fn default() -> Self {
        Drawing {
            pitch: vec![None; DRAW_COLUMNS],
            level: vec![1.0; DRAW_COLUMNS],
            wave: 0,
            length: 1.0,
        }
    }
}

/// The frequency a height in the drawing stands for: 40 Hz at the bottom, about 5 kHz on top
pub fn drawn_hz(p: f32) -> f32 {
    40.0 * 2f32.powf(p.clamp(0.0, 1.0) * 7.0)
}

/// Turn a drawing into audio: the line is the pitch, gaps are silence, every edge softened
pub fn render_drawing(d: &Drawing) -> Vec<f32> {
    let sr = SAMPLE_SR;
    let n = (d.length.clamp(0.05, MAX_SECONDS) * sr) as usize;
    let cols = d.pitch.len().max(1);
    let glide = 1.0 - (-1.0 / (0.004 * sr)).exp();
    let fade = 1.0 - (-1.0 / (0.006 * sr)).exp();
    let (mut hz, mut amp, mut phase) = (0.0f32, 0.0f32, 0.0f32);
    let mut bp = Svf::new(1000.0, 4.0, sr);
    let mut rng = crate::rng::Rng::new(7);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let c = (i * cols / n).min(cols - 1);
        // the loudness glides from column to column, so a painted decay sounds smooth
        let f = (i * cols) as f32 / n as f32 - c as f32;
        let lv = |k: usize| d.level.get(k.min(cols - 1)).copied().unwrap_or(1.0);
        let level = lv(c) + (lv(c + 1) - lv(c)) * f;
        let (want_hz, want_amp) = match d.pitch[c] {
            Some(p) => (drawn_hz(p), level.clamp(0.0, 1.0)),
            None => (hz, 0.0),
        };
        if hz == 0.0 {
            hz = want_hz;
        }
        hz += (want_hz - hz) * glide;
        amp += (want_amp - amp) * fade;
        phase = (phase + hz / sr) % 1.0;
        let x = match d.wave {
            1 => (2.0 * phase - 1.0) * 0.6,
            2 => {
                // narrow and whistling down low, wide and hissing up high (hats, claps)
                let q = (5.0 - hz / 1200.0).max(0.9);
                bp.set(hz.clamp(60.0, 12_000.0), q, sr);
                bp.run(rng.noise()).bp * 2.5
            }
            _ => (phase * TAU).sin() + (phase * TAU * 2.0).sin() * 0.25,
        };
        out.push(crate::synth::sat(x * amp * 0.8));
    }
    out
}

/// A starting point for every track, ready to paint over: the line, its loudness, the wave
/// and the length that make it sound like a kick, a clap, a hat …
pub fn template(v: Voice) -> Drawing {
    let n = DRAW_COLUMNS;
    let x = |i: usize| i as f32 / (n - 1) as f32;
    let decay = |k: f32| (0..n).map(|i| (-x(i) * k).exp()).collect::<Vec<f32>>();
    let flat = |p: f32| vec![Some(p); n];
    let (pitch, level, wave, length) = match v {
        // A fast fall from a click to the sub, fading out
        Voice::Kick => (
            (0..n).map(|i| Some(0.07 + 0.42 * (-x(i) * 14.0).exp())).collect(),
            (0..n).map(|i| (1.0 - x(i)).powf(1.6)).collect(),
            0,
            0.5,
        ),
        // Three quick slaps and a tail of air
        Voice::Clap => (
            flat(0.78),
            (0..n)
                .map(|i| {
                    let t = x(i);
                    if t < 0.18 {
                        let k = (t / 0.06).fract();
                        (1.0 - k * 1.6).max(0.15)
                    } else {
                        0.8 * (-(t - 0.18) * 6.0).exp()
                    }
                })
                .collect(),
            2,
            0.35,
        ),
        Voice::Rim => (flat(0.62), decay(9.0), 1, 0.12),
        Voice::Hat => (flat(0.98), decay(10.0), 2, 0.08),
        Voice::OpenHat => (flat(0.96), decay(4.0), 2, 0.45),
        Voice::Ride => ((0..n).map(|i| Some(0.9 - x(i) * 0.04)).collect(), decay(3.0), 2, 1.4),
        // A round low note that lets go at the end
        Voice::Bass => (
            flat(0.18),
            (0..n)
                .map(|i| {
                    if x(i) < 0.75 {
                        1.0 - x(i) * 0.3
                    } else {
                        0.78 * (1.0 - x(i)) / 0.25
                    }
                })
                .collect(),
            0,
            0.4,
        ),
        // A buzzing note that slides up
        Voice::Acid => (
            (0..n)
                .map(|i| Some(if x(i) < 0.55 { 0.45 } else { 0.45 + (x(i) - 0.55) * 0.35 }))
                .collect(),
            (0..n).map(|i| 1.0 - x(i) * 0.5).collect(),
            1,
            0.3,
        ),
    };
    Drawing {
        pitch,
        level,
        wave,
        length,
    }
}

// ───────────────────────────── Bred sounds ─────────────────────────────

/// A sound as a handful of genes – bred, not drawn: you pick the one you like, and its
/// children come out a little different. They become a drawing, so a bred sound can still be
/// painted over.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Genes {
    /// The wave, as in `Drawing::wave`
    pub wave: u8,
    /// Seconds
    pub length: f32,
    /// The pitch at the start and the one it falls (or rises) to, 0 … 1 like a drawing
    pub start: f32,
    pub end: f32,
    /// How fast it gets there (0.5 = slowly, 30 = at once)
    pub fall: f32,
    /// A wobble in the pitch: how deep (0 … 0.15) and how many times over the sound
    pub wobble: f32,
    pub rate: f32,
    /// How it comes in (share of the length) and dies away (0 = holds, 14 = snaps shut)
    pub attack: f32,
    pub decay: f32,
    /// Quick slaps at the start, like a clap (1 = one hit)
    pub hits: u8,
    /// Holes cut into the tail, 0 (none) … 0.7 (stuttering)
    pub chop: f32,
}

impl Genes {
    /// Where every track's family starts: close to its template
    pub fn of(v: Voice) -> Genes {
        let g = |wave, length, start, end, fall, attack, decay, hits| Genes {
            wave,
            length,
            start,
            end,
            fall,
            wobble: 0.0,
            rate: 6.0,
            attack,
            decay,
            hits,
            chop: 0.0,
        };
        match v {
            Voice::Kick => g(0, 0.5, 0.49, 0.07, 14.0, 0.0, 3.0, 1),
            Voice::Clap => g(2, 0.35, 0.78, 0.78, 4.0, 0.0, 6.0, 3),
            Voice::Rim => g(1, 0.12, 0.64, 0.6, 8.0, 0.0, 9.0, 1),
            Voice::Hat => g(2, 0.08, 0.98, 0.97, 4.0, 0.0, 10.0, 1),
            Voice::OpenHat => g(2, 0.45, 0.96, 0.95, 4.0, 0.0, 4.0, 1),
            Voice::Ride => g(2, 1.4, 0.9, 0.86, 1.0, 0.0, 3.0, 1),
            Voice::Bass => g(0, 0.4, 0.2, 0.18, 20.0, 0.02, 1.0, 1),
            Voice::Acid => g(1, 0.3, 0.45, 0.6, 1.5, 0.01, 1.5, 1),
        }
    }

    /// A child: every gene moves a little (`amount` 0 … 1 says how far), now and then the
    /// wave or the number of hits changes
    pub fn child(&self, rng: &mut crate::rng::Rng, amount: f32) -> Genes {
        let a = amount.clamp(0.0, 1.0);
        let mut g = *self;
        let mut nudge = |x: f32, span: f32, lo: f32, hi: f32| (x + rng.noise() * a * span).clamp(lo, hi);
        g.start = nudge(g.start, 0.5, 0.0, 1.0);
        g.end = nudge(g.end, 0.5, 0.0, 1.0);
        g.attack = nudge(g.attack, 0.12, 0.0, 0.3);
        g.chop = nudge(g.chop - 0.05 * a, 0.5, 0.0, 0.7);
        g.wobble = nudge(g.wobble - 0.02 * a, 0.12, 0.0, 0.15);
        // the speeds and the length move by ratios, not steps
        let mut scale = |x: f32, octaves: f32, lo: f32, hi: f32| (x * 2f32.powf(rng.noise() * a * octaves)).clamp(lo, hi);
        g.length = scale(g.length, 1.6, 0.05, 2.0);
        g.fall = scale(g.fall, 3.0, 0.5, 30.0);
        g.rate = scale(g.rate, 2.5, 1.0, 24.0);
        g.decay = scale(g.decay.max(0.3), 2.0, 0.3, 14.0);
        if rng.chance(a * 0.3) {
            g.wave = rng.below(3) as u8;
        }
        if rng.chance(a * 0.35) {
            g.hits = (g.hits as i32 + if rng.chance(0.5) { 1 } else { -1 }).clamp(1, 4) as u8;
        }
        g
    }

    /// The genes as a drawing – a line and its loudness
    pub fn drawing(&self) -> Drawing {
        let n = DRAW_COLUMNS;
        let x = |i: usize| i as f32 / (n - 1) as f32;
        let pitch = (0..n)
            .map(|i| {
                let t = x(i);
                let p = self.end + (self.start - self.end) * (-t * self.fall).exp() + self.wobble * (t * self.rate * TAU).sin();
                Some(p.clamp(0.0, 1.0))
            })
            .collect();
        let slap = 0.06;
        let level = (0..n)
            .map(|i| {
                let t = x(i);
                let body = if t < self.attack {
                    t / self.attack
                } else {
                    (-(t - self.attack) * self.decay).exp()
                };
                let mut lv = if self.hits > 1 && t < slap * self.hits as f32 {
                    (1.0 - (t / slap).fract() * 1.6).max(0.15)
                } else {
                    body
                };
                if self.chop > 0.0 && t > 0.15 && (t * 16.0).fract() < self.chop {
                    lv = 0.0;
                }
                // always let go at the very end
                lv * ((1.0 - t) * 14.0).min(1.0)
            })
            .collect();
        Drawing {
            pitch,
            level,
            wave: self.wave,
            length: self.length,
        }
    }

    /// The biggest way this one differs from `from`, in a word ("Deeper", "Snappier" …)
    pub fn differs(&self, from: &Genes) -> &'static str {
        if self.wave != from.wave {
            return ["Rounder", "Buzzier", "Airier"][self.wave.min(2) as usize];
        }
        if self.hits != from.hits {
            return if self.hits > from.hits { "More hits" } else { "Fewer hits" };
        }
        let pitch = |g: &Genes| (g.start + g.end) * 0.5;
        let ways = [
            ((pitch(self) - pitch(from)) * 3.0, "Higher", "Deeper"),
            ((self.length / from.length).log2(), "Longer", "Shorter"),
            ((self.decay / from.decay).log2() * 0.7, "Snappier", "Softer"),
            ((self.chop - from.chop) * 3.0, "Choppier", "Smoother"),
            ((self.wobble - from.wobble) * 12.0, "Wobblier", "Steadier"),
            (
                ((self.start - self.end) - (from.start - from.end)).abs() * 2.5,
                "Wilder drop",
                "Wilder drop",
            ),
        ];
        let (d, up, down) = ways
            .iter()
            .copied()
            .max_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
            .unwrap_or((0.0, "Close", "Close"));
        if d.abs() < 0.08 {
            "Close"
        } else if d > 0.0 {
            up
        } else {
            down
        }
    }
}

/// Four children of `parent`: two close, one a bit further, one a wild jump. Rounds go
/// calmer as you go (`round` 0 = the first), so the family settles on what you like.
pub fn litter(parent: &Genes, round: usize, rng: &mut crate::rng::Rng) -> [Genes; 4] {
    let a = (0.55 * 0.75f32.powi(round as i32)).max(0.1);
    let mut kids = [
        parent.child(rng, a * 0.6),
        parent.child(rng, a),
        parent.child(rng, a * 1.4),
        parent.child(rng, (a * 2.4).min(1.0)),
    ];
    // shuffled, so the wild one isn't always last
    for i in (1..4).rev() {
        kids.swap(i, rng.below(i as u32 + 1) as usize);
    }
    kids
}

/// One of Luna's three sounds of the day: a track and a sound bred a few rounds from its
/// template, all from the day (days since 1970), so everyone gets the same ones on the same day
pub struct DaySound {
    pub day: u64,
    pub voice: Voice,
    pub genes: Genes,
    pub name: String,
    /// What Luna says about it
    pub says: &'static str,
}

const DAY_WORDS: [&str; 16] = [
    "Solar", "Lunar", "Neon", "Cosmic", "Velvet", "Static", "Orbit", "Nova", "Comet", "Pulsar", "Quasar", "Nebula", "Ion", "Plasma",
    "Eclipse", "Stardust",
];

const DAY_SAYS: [&str; 8] = [
    "Yo, cooked this one fresh for you today.",
    "Bro, this hits different. Trust.",
    "Found it behind the moon. It's yours, fam.",
    "No cap, I bred this all night.",
    "Today's pick. Put it on a track and send it.",
    "Ayo, this one's straight from the mothership.",
    "Lowkey my favourite yet. Don't tell the others.",
    "Fresh out the oven, brother. Still warm.",
];

impl DaySound {
    /// The day's three: three different tracks, each with its own name and line
    pub fn three(day: u64) -> [DaySound; 3] {
        let seed = day.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5011_A6D0;
        let mut rng = crate::rng::Rng::new(seed);
        let mut voices = Voice::ALL;
        let mut words: Vec<usize> = (0..DAY_WORDS.len()).collect();
        let mut says: Vec<usize> = (0..DAY_SAYS.len()).collect();
        for i in (1..voices.len()).rev() {
            voices.swap(i, rng.below(i as u32 + 1) as usize);
        }
        for list in [&mut words, &mut says] {
            for i in (1..list.len()).rev() {
                list.swap(i, rng.below(i as u32 + 1) as usize);
            }
        }
        std::array::from_fn(|k| {
            let voice = voices[k];
            let mut rng = crate::rng::Rng::new(seed ^ (k as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03));
            let mut genes = Genes::of(voice);
            for round in 0..4 {
                let kids = litter(&genes, round, &mut rng);
                genes = kids[rng.below(4) as usize];
            }
            DaySound {
                day,
                voice,
                genes,
                name: format!("{} {}", DAY_WORDS[words[k]], voice.name().to_lowercase()),
                says: DAY_SAYS[says[k]],
            }
        })
    }

    /// Today's three (UTC)
    pub fn today() -> [DaySound; 3] {
        DaySound::three(now_secs() / 86_400)
    }

    /// The day as "Oct 30"
    pub fn date(&self) -> String {
        // days since 1970 → year, month, day (Howard Hinnant's civil_from_days)
        let z = self.day as i64 + 719_468;
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {d}", MONTHS[(m - 1) as usize])
    }
}

// ───────────────────────────── Files ─────────────────────────────

/// Any audio file → mono at `SAMPLE_SR`, at most `MAX_SECONDS`
pub fn decode_file(path: &Path) -> Result<Vec<f32>, String> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|_| "not an audio file Solarum can read".to_string())?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio in it")?;
    let track_id = track.id;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio in it")?;
    let sr = params.sample_rate.unwrap_or(44_100) as f32;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|_| "this kind of audio isn't supported".to_string())?;
    let limit = (MAX_SECONDS * sr) as usize + 1;
    let mut mono = Vec::new();
    let mut frame: Vec<f32> = Vec::new();
    while mono.len() < limit {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) | Err(Error::IoError(_)) | Err(Error::ResetRequired) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let ch = decoded.spec().channels().count().max(1);
        frame.resize(decoded.samples_interleaved(), 0.0);
        decoded.copy_to_slice_interleaved(&mut frame);
        mono.extend(frame.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32));
    }
    if mono.is_empty() {
        return Err("the file is empty".into());
    }
    let mut out = resample(&mono, sr, SAMPLE_SR);
    out.truncate((MAX_SECONDS * SAMPLE_SR) as usize);
    Ok(out)
}

/// Linear resampling – fine for one-shots and voice
pub fn resample(x: &[f32], from: f32, to: f32) -> Vec<f32> {
    if (from - to).abs() < 0.5 || x.is_empty() {
        return x.to_vec();
    }
    let step = from as f64 / to as f64;
    let n = (x.len() as f64 / step) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let j = p as usize;
            let f = (p - j as f64) as f32;
            let a = x[j.min(x.len() - 1)];
            let b = x[(j + 1).min(x.len() - 1)];
            a + (b - a) * f
        })
        .collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn pack_audio(x: &[f32]) -> String {
    let bytes: Vec<u8> = x.iter().flat_map(|&s| crate::wav::to_i16(s).to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unpack_audio(text: &str) -> Option<Vec<f32>> {
    let mut val = [0u8; 256];
    for (i, &c) in B64.iter().enumerate() {
        val[c as usize] = i as u8 + 1;
    }
    // A sound is at most `MAX_SECONDS` long (16-bit): a file can't make it longer
    let max = (MAX_SECONDS * SAMPLE_SR) as usize * 2;
    let mut bytes = Vec::with_capacity((text.len() * 3 / 4).min(max));
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes().filter(|&c| c != b'=') {
        if bytes.len() >= max {
            break;
        }
        let v = val[c as usize];
        if v == 0 {
            return None;
        }
        acc = acc << 6 | (v - 1) as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
        }
    }
    Some(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b) as f32 / 32767.0)
            .collect(),
    )
}

// ───────────────────────────── The microphone ─────────────────────────────

/// A running recording from an input device
pub struct Recorder {
    _stream: cpal::Stream,
    take: Arc<Mutex<Take>>,
    level: Arc<AtomicU32>,
    sr: f32,
}

/// Samples per peak in `Take::peaks`
const PEAK_BLOCK: usize = 256;

/// What the microphone brought so far, with the peak of every `PEAK_BLOCK` samples, so the
/// waveform can be drawn while it records without going through (or holding the lock over)
/// every sample. Both are allocated for the longest take up front: the input callback never
/// allocates.
struct Take {
    data: Vec<f32>,
    peaks: Vec<f32>,
    /// The peak of the block not yet full
    open: f32,
    limit: usize,
}

impl Take {
    fn new(limit: usize) -> Self {
        Take {
            data: Vec::with_capacity(limit),
            peaks: Vec::with_capacity(limit / PEAK_BLOCK + 1),
            open: 0.0,
            limit,
        }
    }

    fn push(&mut self, s: f32) {
        if self.data.len() >= self.limit {
            return;
        }
        self.data.push(s);
        self.open = self.open.max(s.abs());
        if self.data.len().is_multiple_of(PEAK_BLOCK) {
            self.peaks.push(self.open);
            self.open = 0.0;
        }
    }

    /// At most `max` peaks over the whole take so far
    fn waveform(&self, max: usize) -> Vec<f32> {
        let partial = !self.data.len().is_multiple_of(PEAK_BLOCK);
        let n = self.peaks.len() + partial as usize;
        let step = n.div_ceil(max.max(1)).max(1);
        let all = self.peaks.iter().copied().chain(partial.then_some(self.open));
        let mut out = Vec::with_capacity(n / step + 1);
        let mut it = all.peekable();
        while it.peek().is_some() {
            out.push(it.by_ref().take(step).fold(0.0f32, f32::max));
        }
        out
    }
}

/// The names of the inputs (microphones, interfaces)
pub fn inputs() -> Vec<String> {
    use cpal::traits::HostTrait;
    cpal::default_host()
        .input_devices()
        .map(|d| d.filter_map(|d| crate::audio::device_name(&d)).collect())
        .unwrap_or_default()
}

impl Recorder {
    /// Start recording from the named input (or the default one)
    pub fn start(input: Option<&str>) -> Result<Recorder, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = input
            .and_then(|name| {
                host.input_devices()
                    .ok()?
                    .find(|d| crate::audio::device_name(d).is_some_and(|n| n == name))
            })
            .or_else(|| host.default_input_device())
            .ok_or("no microphone found")?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let ch = config.channels().max(1) as usize;
        let sr = config.sample_rate() as f32;
        let shared = Arc::new(Mutex::new(Take::new((MAX_SECONDS * sr) as usize)));
        let level = Arc::new(AtomicU32::new(0));
        let (d, l) = (shared.clone(), level.clone());
        let take = move |frames: &mut dyn Iterator<Item = f32>| {
            let mut peak = 0.0f32;
            if let Ok(mut t) = d.lock() {
                for s in frames {
                    peak = peak.max(s.abs());
                    t.push(s);
                }
            }
            let old = f32::from_bits(l.load(Ordering::Relaxed));
            l.store(peak.max(old * 0.9).to_bits(), Ordering::Relaxed);
        };
        let err = |e| eprintln!("recording: {e}");
        let cfg: cpal::StreamConfig = config.clone().into();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &cfg,
                move |x: &[f32], _: &_| take(&mut x.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32)),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &cfg,
                move |x: &[i16], _: &_| take(&mut x.chunks(ch).map(|f| f.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / ch as f32)),
                err,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &cfg,
                move |x: &[u16], _: &_| {
                    take(
                        &mut x
                            .chunks(ch)
                            .map(|f| f.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).sum::<f32>() / ch as f32),
                    )
                },
                err,
                None,
            ),
            f => return Err(format!("unsupported input format {f}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Recorder {
            _stream: stream,
            take: shared,
            level,
            sr,
        })
    }

    /// Peak level lately (0…1)
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn seconds(&self) -> f32 {
        self.take.lock().map(|t| t.data.len()).unwrap_or(0) as f32 / self.sr
    }

    /// The waveform so far (for drawing while it records): reads only the block peaks, so
    /// the microphone never waits for the screen
    pub fn snapshot(&self, max: usize) -> Vec<f32> {
        self.take.lock().map(|t| t.waveform(max)).unwrap_or_default()
    }

    /// Stop; the audio at `SAMPLE_SR`
    pub fn finish(self) -> Vec<f32> {
        let d = self.take.lock().map(|mut t| std::mem::take(&mut t.data)).unwrap_or_default();
        resample(&d, self.sr, SAMPLE_SR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits() -> Vec<f32> {
        // Four decaying hits, a quarter second apart, in a little noise
        let mut rng = crate::rng::Rng::new(3);
        (0..(SAMPLE_SR as usize))
            .map(|i| {
                let t = (i % 11_025) as f32 / SAMPLE_SR;
                (t * 900.0).sin() * (-t / 0.03).exp() * 0.8 + rng.noise() * 0.001
            })
            .collect()
    }

    #[test]
    fn the_sounds_of_the_day_change_daily_and_stay_clean() {
        let a = DaySound::three(20_391);
        assert_eq!(a[0].date(), "Oct 30");
        assert_eq!(DaySound::three(0)[0].date(), "Jan 1");
        assert_eq!(a.each_ref().map(|d| d.genes), DaySound::three(20_391).each_ref().map(|d| d.genes));
        let days: Vec<[DaySound; 3]> = (20_000..20_030).map(DaySound::three).collect();
        assert!(days.windows(2).all(|w| w[0][0].genes != w[1][0].genes));
        for d in &days {
            // three different tracks, three names
            assert!(d[0].voice != d[1].voice && d[1].voice != d[2].voice && d[0].voice != d[2].voice);
            assert!(d[0].name != d[1].name && d[1].name != d[2].name && d[0].name != d[2].name);
            for s in d {
                let x = render_drawing(&s.genes.drawing());
                let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(peak <= 1.0 && peak > 0.05, "{}: peak {peak}", s.name);
            }
        }
        assert!(Voice::ALL.iter().all(|&v| days.iter().any(|d| d.iter().any(|s| s.voice == v))));
    }

    #[test]
    fn bred_sounds_differ_stay_clean_and_repeat() {
        for v in Voice::ALL {
            let mut rng = crate::rng::Rng::new(11);
            let mut parent = Genes::of(v);
            for round in 0..6 {
                let kids = litter(&parent, round, &mut rng);
                for k in &kids {
                    let x = render_drawing(&k.drawing());
                    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                    assert!(x.iter().all(|s| s.is_finite()) && peak <= 1.0, "{v:?} round {round}: peak {peak}");
                    assert!(peak > 0.05, "{v:?} round {round}: silent");
                }
                assert!(kids.iter().any(|k| *k != parent), "{v:?}: no child changed");
                parent = kids[round % 4];
            }
        }
        // the same seed breeds the same family
        let a = litter(&Genes::of(Voice::Kick), 0, &mut crate::rng::Rng::new(5));
        let b = litter(&Genes::of(Voice::Kick), 0, &mut crate::rng::Rng::new(5));
        assert_eq!(a, b);
    }

    #[test]
    fn every_track_has_a_template_that_sounds() {
        for v in Voice::ALL {
            let d = template(v);
            assert_eq!(d.pitch.len(), DRAW_COLUMNS);
            assert_eq!(d.level.len(), DRAW_COLUMNS);
            let x = render_drawing(&d);
            let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.1 && peak <= 1.0, "{}: peak {peak}", v.name());
            // drums die away: the last tenth is much softer than the start
            if !v.is_synth() {
                let tail = &x[x.len() * 9 / 10..];
                let end = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!(end < peak * 0.5, "{} doesn't decay", v.name());
            }
        }
    }

    #[test]
    fn loudness_lane_shapes_the_sound() {
        let mut d = template(Voice::Bass);
        d.level = vec![0.0; DRAW_COLUMNS];
        assert!(render_drawing(&d).iter().all(|s| s.abs() < 1e-3), "silent at zero loudness");
    }

    #[test]
    fn render_cuts_and_levels() {
        let x = hits();
        let e = Edit {
            start: 0.25,
            end: 0.5,
            ..Edit::default()
        };
        let y = render(&x, &e);
        assert_eq!(y.len(), 11_025);
        let peak = y.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.89).abs() < 0.01);
        assert!(y.last().unwrap().abs() < 1e-3, "faded out");
        let r = render(
            &x,
            &Edit {
                reverse: true,
                crush: 1.0,
                ..e
            },
        );
        assert_eq!(r.len(), y.len());
        assert!(r.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn trims_silence() {
        let mut x = vec![0.0; 4410];
        x.extend(hits().into_iter().take(4410));
        x.extend(vec![0.0; 44_100]);
        let (a, b) = trim_points(&x);
        assert!((a - 0.096).abs() < 0.002, "{a}");
        assert!(b > 0.1 && b < 0.3, "{b}");
    }

    #[test]
    fn audio_survives_packing() {
        let x: Vec<f32> = (0..1001).map(|i| (i as f32 * 0.01).sin() * 0.7).collect();
        let y = unpack_audio(&pack_audio(&x)).unwrap();
        assert_eq!(y.len(), x.len());
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-3));
    }

    #[test]
    fn a_packed_project_brings_its_sounds() {
        let mut lib = Library::in_memory();
        let id = lib.add("Kick me", Voice::Kick, hits());
        let p = lib.pack(&[id]);
        let mut other = Library::in_memory();
        other.unpack(&p);
        assert_eq!(other.get(id).map(|e| e.name.as_str()), Some("Kick me"));
        assert!(other.sound(id).is_some_and(|b| !b.data.is_empty()));
    }

    #[test]
    fn a_hostile_project_is_cut_down_and_repaired() {
        // Audio longer than any sound may be, and edits no knob can make
        let long = pack_audio(&vec![0.5; (MAX_SECONDS * SAMPLE_SR) as usize * 3]);
        assert_eq!(unpack_audio(&long).map(|x| x.len()), Some((MAX_SECONDS * SAMPLE_SR) as usize));
        let edit = Edit {
            gain: 99.0,
            pitch: f32::NAN,
            low_cut: f32::NAN,
            fade_in: -3.0,
            normalize: false,
            ..Default::default()
        };
        let p = Packed {
            entries: (1..=SAMPLE_SLOTS as u64 + 5)
                .map(|id| Entry {
                    id,
                    source: 7,
                    edit,
                    ..Default::default()
                })
                .collect(),
            sources: (0..SAMPLE_SLOTS as u64 + 5).map(|id| (id + 7, pack_audio(&hits()))).collect(),
        };
        let mut lib = Library::in_memory();
        lib.unpack(&p);
        assert_eq!(lib.sources.len(), SAMPLE_SLOTS);
        assert_eq!(lib.entries.len(), SAMPLE_SLOTS);
        let e = lib.get(1).unwrap().edit;
        assert_eq!((e.gain, e.pitch, e.low_cut, e.fade_in), (24.0, 0.0, 20.0, 0.0));
        let buf = lib.sound(1).unwrap();
        assert!(buf.data.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn a_take_keeps_its_peaks_without_allocating_more() {
        let mut t = Take::new(PEAK_BLOCK * 10 + 7);
        let (cap, peaks_cap) = (t.data.capacity(), t.peaks.capacity());
        for i in 0..PEAK_BLOCK * 20 {
            // One loud sample in the fourth block
            t.push(if i == PEAK_BLOCK * 3 + 5 { -0.9 } else { 0.1 });
        }
        assert_eq!(t.data.len(), PEAK_BLOCK * 10 + 7, "stops at the limit");
        assert_eq!((t.data.capacity(), t.peaks.capacity()), (cap, peaks_cap));
        // Ten full blocks and the partial one
        let w = t.waveform(100);
        assert_eq!(w.len(), 11);
        assert_eq!(w[3], 0.9);
        assert!(w.iter().enumerate().all(|(i, &p)| i == 3 || p == 0.1));
        // Fewer columns: each is the loudest of its blocks
        assert_eq!(t.waveform(4), vec![0.1, 0.9, 0.1, 0.1]);
        assert!(Take::new(100).waveform(10).is_empty());
    }

    #[test]
    fn a_broken_edit_renders_silence_not_nan() {
        let edit = Edit {
            gain: f32::NAN,
            normalize: false,
            ..Default::default()
        };
        assert!(render(&hits(), &edit).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn drawings_sound_and_gaps_are_silent() {
        let mut d = Drawing::default();
        for p in d.pitch.iter_mut().take(DRAW_COLUMNS / 2) {
            *p = Some(0.5);
        }
        let x = render_drawing(&d);
        assert_eq!(x.len(), SAMPLE_SR as usize);
        let half = x.len() / 2;
        assert!(x[..half - 1000].iter().any(|s| s.abs() > 0.3));
        assert!(x[half + 2000..].iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn names_guess_the_track() {
        assert_eq!(guess_voice("BD_909_hard.wav"), Voice::Kick);
        assert_eq!(guess_voice("open hat 3"), Voice::OpenHat);
        assert_eq!(guess_voice("HH closed"), Voice::Hat);
        assert_eq!(guess_voice("my snare"), Voice::Clap);
        assert_eq!(guess_voice("door knock"), Voice::Rim);
    }

    #[test]
    fn decodes_a_wav_file() {
        let dir = std::env::temp_dir().join(format!("solarum-test-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let pcm: Vec<i16> = (0..22_050).map(|i| ((i as f32 * 0.05).sin() * 16_000.0) as i16).collect();
        std::fs::write(&path, crate::wav::encode_mono_i16(&pcm, 22_050)).unwrap();
        let x = decode_file(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // One second, now at 44.1 kHz
        assert!((x.len() as i32 - 44_100).abs() < 4, "{}", x.len());
    }
}
