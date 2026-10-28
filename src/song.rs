//! The song: eight tracks of up to 16 steps, tempo, key and the four macros.
//! Everything is `Copy`, so the UI and the audio thread can swap whole songs
//! without allocating.

use serde::{Deserialize, Serialize};

pub const TRACKS: usize = 8;
pub const STEPS: usize = 16;
/// The notes the step menu offers, in semitones from the root: an octave below to two above
pub const NOTES: std::ops::RangeInclusive<i8> = -12..=24;
/// Every note a step can hold: two octaves below the root to three above
pub const NOTE_LIMITS: std::ops::RangeInclusive<i8> = -24..=36;
pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The name of a note `semis` above the MIDI note `root`, e.g. "F#"
pub fn note_name(root: u8, semis: i8) -> &'static str {
    NOTE_NAMES[(root as i32 + semis as i32).rem_euclid(12) as usize]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Voice {
    Kick,
    Clap,
    Rim,
    Hat,
    OpenHat,
    Ride,
    Bass,
    Acid,
}

impl Voice {
    pub const ALL: [Voice; TRACKS] = [
        Voice::Kick,
        Voice::Clap,
        Voice::Rim,
        Voice::Hat,
        Voice::OpenHat,
        Voice::Ride,
        Voice::Bass,
        Voice::Acid,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Voice::Kick => "Kick",
            Voice::Clap => "Clap",
            Voice::Rim => "Rim",
            Voice::Hat => "Hat",
            Voice::OpenHat => "Open hat",
            Voice::Ride => "Ride",
            Voice::Bass => "Bass",
            Voice::Acid => "Acid",
        }
    }

    /// Synth tracks play notes; drum tracks ignore `Step::note`.
    pub fn is_synth(self) -> bool {
        matches!(self, Voice::Bass | Voice::Acid)
    }

    pub fn index(self) -> usize {
        Voice::ALL.iter().position(|&v| v == self).unwrap_or(0)
    }

    /// The sounds each track can switch between. The first one is the classic
    /// default, so saved songs from before sounds existed play as they did.
    pub fn sounds(self) -> &'static [&'static str] {
        match self {
            // Built-in sounds by what they feel like in space (`synth.rs` and `gen.rs` list their
            // makings in the same order: 909, 808, hard, deep, punch, rumble, distort …)
            Voice::Kick => &["Pulsar", "Gravity", "Impact", "Abyss", "Meteor", "Core", "Supernova"],
            Voice::Clap => &["Flare", "Comet", "Solar wind", "Spark", "Static", "Old signal"],
            Voice::Rim => &["Ping", "Beacon", "Probe", "Crater", "Sonar", "Moon drum", "Laser"],
            Voice::Hat => &["Stardust", "Ion", "Asteroids", "Quartz", "Satellite", "Frost"],
            Voice::OpenHat => &["Nebula", "Aurora", "Plasma", "Tail", "Ring"],
            Voice::Ride => &["Orbit", "Halo", "Eclipse", "Big Bang", "Milky Way"],
            Voice::Bass => &["Rover", "Deep field", "Wormhole", "Quasar", "Bounce", "Magnetar", "Black hole"],
            Voice::Acid => &[
                "Acid rain",
                "Acid moon",
                "Starlight",
                "Constellation",
                "Mothership",
                "Cosmos",
                "Cathedral",
                "Hyperdrive",
            ],
        }
    }

    /// One line about what the track is for – shown when hovering its name
    pub fn blurb(self) -> &'static str {
        match self {
            Voice::Kick => "The heartbeat: four on the floor",
            Voice::Clap => "The backbeat on 2 and 4",
            Voice::Rim => "Percussion that bounces around the beat",
            Voice::Hat => "Closed hi-hat: drives the groove forward",
            Voice::OpenHat => "Open hi-hat: the off-beat \"tsss\"",
            Voice::Ride => "Ride cymbal: shimmer on top",
            Voice::Bass => "Bassline in the song's key",
            Voice::Acid => "Synth line: 303 acid, chord stabs, plucks, a rave hoover, pads, organ or supersaw",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    pub on: bool,
    /// Louder, and for the acid line: brighter and snappier
    pub accent: bool,
    /// Semitones above the song's root (synth tracks only)
    pub note: i8,
    /// Glide into this note without retriggering (acid only)
    pub slide: bool,
    /// Probability in percent that the step plays
    pub chance: u8,
    /// A ghost note: played soft, at half the velocity
    pub ghost: bool,
}

impl Default for Step {
    fn default() -> Self {
        Step {
            on: false,
            accent: false,
            note: 0,
            slide: false,
            chance: 100,
            ghost: false,
        }
    }
}

impl Step {
    pub fn hit() -> Self {
        Step {
            on: true,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub steps: [Step; STEPS],
    /// 1…16 – shorter tracks loop earlier (polyrhythms)
    pub len: u8,
    pub mute: bool,
    /// 0…1
    pub volume: f32,
    /// Index into `Voice::sounds()`
    pub sound: u8,
    /// Semitones, −12…12
    pub tune: i8,
    /// 0…1, 0.5 = the sound's own length
    pub decay: f32,
    /// Locked tracks are left alone by shuffle, the dice and the auto mode
    pub lock: bool,
}

impl Default for Track {
    fn default() -> Self {
        Track {
            steps: [Step::default(); STEPS],
            len: STEPS as u8,
            mute: false,
            volume: 0.8,
            sound: 0,
            tune: 0,
            decay: 0.5,
            lock: false,
        }
    }
}

impl Track {
    /// A whole bar: a shorter loop (from older files) written out as its repeats. Every
    /// track is 16 steps – an 8-step loop sounds the same as its two copies in a bar.
    pub fn expand(&mut self) {
        let len = fit_len(self.len) as usize;
        for i in len..STEPS {
            self.steps[i] = self.steps[i % len];
        }
        self.len = STEPS as u8;
    }

    pub fn hits(&self) -> usize {
        self.steps[..self.len as usize].iter().filter(|s| s.on).count()
    }

    /// Index of the step that plays at global step `pos`
    pub fn step_at(&self, pos: u64) -> usize {
        (pos % self.len.max(1) as u64) as usize
    }

    /// Pitch factor from `tune`
    pub fn tune_ratio(&self) -> f32 {
        2f32.powf(self.tune as f32 / 12.0)
    }

    /// Length factor from `decay`: about ⅓ … 3 ×
    pub fn decay_factor(&self) -> f32 {
        2f32.powf((self.decay - 0.5) * 3.2)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Vibe {
    Dub,
    Minimal,
    Melodic,
    Hypnotic,
    Electro,
    Peak,
    Acid,
    Rave,
    Industrial,
    Hard,
    /// Starts empty – you build the groove; shuffle and auto mode use hypnotic rules
    Custom,
}

impl Vibe {
    /// From calm to hard – the order of the chips in the vibe bar
    pub const ALL: [Vibe; 10] = [
        Vibe::Dub,
        Vibe::Minimal,
        Vibe::Melodic,
        Vibe::Hypnotic,
        Vibe::Electro,
        Vibe::Peak,
        Vibe::Acid,
        Vibe::Rave,
        Vibe::Industrial,
        Vibe::Hard,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Vibe::Dub => "Dub",
            Vibe::Minimal => "Minimal",
            Vibe::Melodic => "Melodic",
            Vibe::Hypnotic => "Hypnotic",
            Vibe::Electro => "Electro",
            Vibe::Peak => "Peak time",
            Vibe::Acid => "Acid",
            Vibe::Rave => "Rave",
            Vibe::Industrial => "Industrial",
            Vibe::Hard => "Hard",
            Vibe::Custom => "Custom",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Vibe::Dub => "Deep chord stabs drowning in echo",
            Vibe::Minimal => "Sparse, clicky, lots of air",
            Vibe::Melodic => "Plucked arpeggios over a warm sub",
            Vibe::Hypnotic => "Rolling bass, deep and steady",
            Vibe::Electro => "Broken 808 kicks, snares and cowbells",
            Vibe::Peak => "Big claps, rides, full energy",
            Vibe::Acid => "Squelchy 303 lines on top",
            Vibe::Rave => "90s hoovers, stabs and a reese bass",
            Vibe::Industrial => "Fast, dark and distorted",
            Vibe::Hard => "Distorted kicks and a donk bass, 150 BPM",
            Vibe::Custom => "An empty pattern – paint your own groove",
        }
    }

    /// Typical tempo range
    pub fn bpm(self) -> (f32, f32) {
        match self {
            Vibe::Dub => (118.0, 124.0),
            Vibe::Minimal => (124.0, 128.0),
            Vibe::Melodic => (122.0, 126.0),
            Vibe::Hypnotic => (128.0, 132.0),
            Vibe::Electro => (126.0, 132.0),
            Vibe::Peak => (132.0, 138.0),
            Vibe::Acid => (130.0, 136.0),
            Vibe::Rave => (138.0, 145.0),
            Vibe::Industrial => (138.0, 146.0),
            Vibe::Hard => (148.0, 156.0),
            Vibe::Custom => (126.0, 132.0),
        }
    }

    /// The vibe whose rules shuffle and the auto mode follow (custom plays like hypnotic)
    pub fn rules(self) -> Vibe {
        if self == Vibe::Custom {
            Vibe::Hypnotic
        } else {
            self
        }
    }

    /// Everything but electro keeps the kick on every beat
    pub fn four_on_the_floor(self) -> bool {
        self != Vibe::Electro
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Scale {
    Minor,
    Phrygian,
    Dorian,
    Harmonic,
    Pentatonic,
    Major,
}

impl Scale {
    #[cfg(test)]
    pub const ALL: [Scale; 6] = [
        Scale::Minor,
        Scale::Phrygian,
        Scale::Dorian,
        Scale::Harmonic,
        Scale::Pentatonic,
        Scale::Major,
    ];

    pub fn intervals(self) -> &'static [i8] {
        match self {
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Harmonic => &[0, 2, 3, 5, 7, 8, 11],
            Scale::Pentatonic => &[0, 3, 5, 7, 10],
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scale::Minor => "minor",
            Scale::Phrygian => "phrygian",
            Scale::Dorian => "dorian",
            Scale::Harmonic => "harmonic",
            Scale::Pentatonic => "pentatonic",
            Scale::Major => "major",
        }
    }

    /// Closest note of the scale at or below `semis`, then `steps` scale degrees further
    pub fn shift(self, semis: i8, steps: i32) -> i8 {
        let iv = self.intervals();
        let n = iv.len() as i32;
        let d = self.degree(semis) + steps;
        let (o, k) = (d.div_euclid(n), d.rem_euclid(n));
        (o * 12 + iv[k as usize] as i32).clamp(*NOTE_LIMITS.start() as i32, *NOTE_LIMITS.end() as i32) as i8
    }

    /// How many scale degrees the closest note at or below `semis` lies above the root
    /// (0 = the root, negative below it)
    pub fn degree(self, semis: i8) -> i32 {
        let iv = self.intervals();
        let octave = (semis as i32).div_euclid(12);
        let within = (semis as i32).rem_euclid(12) as i8;
        octave * iv.len() as i32 + iv.iter().rposition(|&i| i <= within).unwrap_or(0) as i32
    }
}

/// Chord progressions over four bars: scale degrees the bass and synth move by, bar by bar
pub const PROGRESSIONS: [(&str, [i32; 4]); 7] = [
    ("none", [0, 0, 0, 0]),
    ("i → VI", [0, 0, -2, -2]),
    ("i → iv", [0, 0, 3, 3]),
    ("i → VI → III → VII", [0, -2, 2, -1]),
    ("i → VII → VI → VII", [0, -1, -2, -1]),
    ("i → iv → v → i", [0, 3, 4, 0]),
    ("i → II (dark)", [0, 1, 0, 1]),
];

fn yes() -> bool {
    true
}

/// The loop lengths that stay in step with the bar (16, 8, 4, 2, 1) – any other length
/// (older saves had 12, 14, 15 …) is rounded to the nearest of them
pub fn fit_len(len: u8) -> u8 {
    match len {
        0..=1 => 1,
        2 => 2,
        3..=5 => 4,
        6..=11 => 8,
        _ => 16,
    }
}

/// Moods: a scale and a chord progression (index into `PROGRESSIONS`) that go together
pub const MOODS: [(&str, &str, Scale, u8); 10] = [
    ("Dark", "Tense and menacing – one dark step up and back", Scale::Phrygian, 6),
    ("Hypnotic", "One chord, round and round", Scale::Pentatonic, 0),
    ("Deep", "Warm and soulful", Scale::Dorian, 2),
    ("Sad", "Melancholic, two chords that long for each other", Scale::Minor, 1),
    ("Euphoric", "Big uplifting chords, hands in the air", Scale::Minor, 3),
    ("Dreamy", "Soft and floating, rolling down and back up", Scale::Dorian, 4),
    ("Epic", "Dramatic, climbing for three bars, then home", Scale::Harmonic, 5),
    ("Sunny", "Bright and happy – techno in a major key", Scale::Major, 2),
    ("Mysterious", "Exotic and dark, one chord that never settles", Scale::Harmonic, 0),
    (
        "Neutral",
        "Plain minor, no chord changes – the start, before any colour",
        Scale::Minor,
        0,
    ),
];

/// The mood Solarum starts in (and a clear goes back to): plain, no chord changes
pub const NEUTRAL: usize = 9;

/// The master section: three-band EQ and drive, each 0…1 (0.5 = flat)
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MasterEq {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
    pub drive: f32,
}

impl Default for MasterEq {
    fn default() -> Self {
        MasterEq {
            low: 0.5,
            mid: 0.5,
            high: 0.5,
            drive: 0.3,
        }
    }
}

/// The four big knobs, each 0…1
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Macros {
    pub energy: f32,
    pub dark: f32,
    pub acid: f32,
    pub space: f32,
    /// Everything ducks under the kick
    pub pump: f32,
}

impl Default for Macros {
    fn default() -> Self {
        Macros {
            energy: 0.6,
            dark: 0.45,
            acid: 0.5,
            space: 0.35,
            pump: 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Song {
    pub tracks: [Track; TRACKS],
    pub bpm: f32,
    /// 0…1: how late the off-16ths land
    pub swing: f32,
    /// MIDI note of the bass root (e.g. 41 = F2)
    pub root: u8,
    pub scale: Scale,
    pub vibe: Vibe,
    pub macros: Macros,
    pub master: f32,
    /// Auto mode: the machine keeps changing the groove
    pub auto: bool,
    /// Bars between two auto changes (1–16)
    pub auto_bars: u8,
    /// Auto mode moves the energy itself (off once you take the energy fader)
    pub curve: bool,
    /// Automatic build-ups get effects (low-pass, gate, echo) and phrase ends sometimes an
    /// effect moment – off: classic build-ups and plain fills only
    #[serde(default = "yes")]
    pub auto_fx: bool,
    /// Index into `PROGRESSIONS`
    pub prog: u8,
    /// The mood was picked by hand: shuffle and the auto mode keep it (off: the vibe picks one)
    pub mood_hold: bool,
    pub eq: MasterEq,
}

impl Default for Song {
    fn default() -> Self {
        let mut song = crate::gen::generate(Vibe::Hypnotic, &Macros::default(), &mut crate::rng::Rng::new(909));
        // The first beat anyone hears should have a clap on 2 and 4
        let clap = song.track_mut(Voice::Clap);
        clap.steps[4] = Step::hit();
        clap.steps[12] = Step::hit();
        song
    }
}

impl Song {
    pub fn empty() -> Self {
        let mut tracks = [Track::default(); TRACKS];
        tracks[Voice::Kick.index()].volume = 0.9;
        tracks[Voice::Ride.index()].volume = 0.55;
        tracks[Voice::Rim.index()].volume = 0.65;
        Song {
            tracks,
            bpm: 130.0,
            swing: 0.0,
            root: 41,
            scale: Scale::Minor,
            vibe: Vibe::Hypnotic,
            macros: Macros::default(),
            master: 0.8,
            auto: false,
            auto_bars: 4,
            curve: true,
            auto_fx: true,
            prog: 0,
            mood_hold: false,
            eq: MasterEq::default(),
        }
    }

    pub fn track(&self, v: Voice) -> &Track {
        &self.tracks[v.index()]
    }

    pub fn track_mut(&mut self, v: Voice) -> &mut Track {
        &mut self.tracks[v.index()]
    }

    /// Repair values that would break the app (old or hand-edited saved state)
    pub fn sanitize(&mut self) {
        for (t, v) in self.tracks.iter_mut().zip(Voice::ALL) {
            t.expand();
            if t.sound as usize >= v.sounds().len() && crate::sampler::slot_of(t.sound).is_none() {
                t.sound = 0;
            }
            t.volume = if t.volume.is_finite() { t.volume.clamp(0.0, 1.0) } else { 0.8 };
            t.tune = t.tune.clamp(-12, 12);
            t.decay = if t.decay.is_finite() { t.decay.clamp(0.0, 1.0) } else { 0.5 };
            for s in &mut t.steps {
                s.chance = s.chance.min(100);
                s.note = s.note.clamp(*NOTE_LIMITS.start(), *NOTE_LIMITS.end());
            }
        }
        let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        self.bpm = fix(self.bpm, 90.0, 180.0, 130.0);
        self.swing = fix(self.swing, 0.0, 1.0, 0.0);
        self.master = fix(self.master, 0.0, 1.0, 0.8);
        let m = &mut self.macros;
        m.energy = fix(m.energy, 0.0, 1.0, 0.6);
        m.dark = fix(m.dark, 0.0, 1.0, 0.45);
        m.acid = fix(m.acid, 0.0, 1.0, 0.5);
        m.space = fix(m.space, 0.0, 1.0, 0.35);
        m.pump = fix(m.pump, 0.0, 1.0, 0.0);
        let eq = &mut self.eq;
        eq.low = fix(eq.low, 0.0, 1.0, 0.5);
        eq.mid = fix(eq.mid, 0.0, 1.0, 0.5);
        eq.high = fix(eq.high, 0.0, 1.0, 0.5);
        eq.drive = fix(eq.drive, 0.0, 1.0, 0.3);
        if self.prog as usize >= PROGRESSIONS.len() {
            self.prog = 0;
        }
        self.root = self.root.clamp(36, 47);
        self.auto_bars = self.auto_bars.clamp(1, 16);
    }

    pub fn note_name(&self) -> &'static str {
        // "#" rather than "♯": the UI fonts have no sharp sign
        const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        NAMES[self.root as usize % 12]
    }

    pub fn root_name(&self) -> String {
        format!("{} {}", self.note_name(), self.scale.name())
    }

    /// Scale degrees bass and synth move by in global bar `bar`
    pub fn prog_degree(&self, bar: u64) -> i32 {
        PROGRESSIONS[self.prog as usize % PROGRESSIONS.len()].1[(bar % 4) as usize]
    }

    /// The mood whose scale and chords the song uses, if any
    pub fn set_mood(&mut self, k: usize) {
        self.scale = MOODS[k].2;
        self.prog = MOODS[k].3;
    }

    pub fn mood(&self) -> Option<usize> {
        MOODS
            .iter()
            .position(|&(_, _, scale, prog)| scale == self.scale && prog == self.prog)
    }

    pub fn sound_name(&self, v: Voice) -> &'static str {
        v.sounds()[self.track(v).sound as usize % v.sounds().len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_shift_stays_in_scale() {
        let s = Scale::Minor;
        assert_eq!(s.shift(0, 1), 2);
        assert_eq!(s.shift(0, 2), 3);
        assert_eq!(s.shift(0, 7), 12);
        assert_eq!(s.shift(0, -1), -2);
        assert_eq!(s.shift(1, 0), 0); // C♯ is not in C minor → snaps down to C
        for sc in Scale::ALL {
            for n in -12..24 {
                for d in -3..4 {
                    let r = sc.shift(n, d).rem_euclid(12);
                    assert!(sc.intervals().contains(&r), "{sc:?} {n} {d} → {r}");
                }
            }
        }
        assert_eq!(Scale::Pentatonic.shift(0, 5), 12);
    }

    #[test]
    fn degrees_count_from_the_root() {
        let s = Scale::Minor;
        assert_eq!(s.degree(0), 0);
        assert_eq!(s.degree(3), 2);
        assert_eq!(s.degree(12), 7);
        assert_eq!(s.degree(-2), -1);
        assert_eq!(s.degree(1), 0); // C♯ counts as C
        for sc in Scale::ALL {
            for d in -10..=14 {
                assert_eq!(sc.degree(sc.shift(0, d)), d, "{sc:?} {d}");
            }
        }
    }

    #[test]
    fn sanitize_repairs_broken_values() {
        let mut s = Song::default();
        s.tracks[2].len = 0;
        s.tracks[4].len = 12;
        s.tracks[5].len = 6;
        s.tracks[5].steps = [Step::default(); STEPS];
        s.tracks[5].steps[1] = Step::hit();
        s.tracks[3].volume = f32::NAN;
        s.bpm = 9999.0;
        s.root = 3;
        s.auto_bars = 40;
        s.tracks[7].sound = 99;
        s.tracks[1].decay = f32::INFINITY;
        s.tracks[1].tune = 99;
        s.prog = 200;
        s.eq.low = f32::NAN;
        s.sanitize();
        // Every track is a whole bar; a shorter loop is written out as its repeats (6 → 8)
        assert!(s.tracks.iter().all(|t| t.len == 16));
        let on: Vec<usize> = (0..16).filter(|&i| s.tracks[5].steps[i].on).collect();
        assert_eq!(on, [1, 9]);
        assert_eq!(s.tracks[1].decay, 0.5);
        assert_eq!(s.tracks[1].tune, 12);
        assert_eq!(s.prog, 0);
        assert_eq!(s.eq.low, 0.5);
        assert_eq!(s.tracks[3].volume, 0.8);
        assert_eq!(s.bpm, 180.0);
        assert_eq!(s.root, 36);
        assert_eq!(s.auto_bars, 16);
        assert_eq!(s.tracks[7].sound, 0);
        let mut ok = Song::default();
        let before = ok;
        ok.sanitize();
        assert_eq!(ok, before);
    }

    #[test]
    fn song_round_trips_through_json_shape() {
        // serde derives must cover every field (saved state)
        let song = Song::default();
        let copy = song;
        assert_eq!(song, copy);
        assert_eq!(Voice::ALL.len(), TRACKS);
        for (i, v) in Voice::ALL.iter().enumerate() {
            assert_eq!(v.index(), i);
        }
    }
}
