//! The audio engine: sequencer clock, voices, mixer and master effects.
//!
//! It runs on the audio thread. The UI talks to it with [`Cmd`] messages and
//! reads the playhead and hit lights from [`Shared`] atomics; when the engine
//! itself changes the song (auto mode, recording, song mode) it sends the new
//! song back as an [`Event`]. The same engine renders WAV exports offline.

use crate::form::{pattern_slot, Part, FORMS};
use crate::fx::{Delay, Reverb, Rumble};
use crate::gen::{self, Change};
use crate::master::{Drive, Master};
use crate::rng::Rng;
use crate::sampler::{slot_of, Audition, SampleBuf, Sampler, Slots};
use crate::song::{Song, Vibe, Voice, STEPS, TRACKS};
use crate::synth::*;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

mod arrange;

use arrange::*;
pub use arrange::{curve, Fill, DROP_ZONE, FILLS};

pub enum Cmd {
    Song(Box<Song>),
    Play,
    Stop,
    /// Play a sound now (pads, MIDI); `record` writes it into the pattern
    Hit {
        voice: Voice,
        vel: f32,
        record: bool,
    },
    /// −1 (low-pass) … 0 (off) … +1 (high-pass)
    Filter(f32),
    /// The filter's resonance, 0 (soft) … 1 (singing)
    FilterRes(f32),
    /// A live effect held down (true) or let go
    Fx(Fx, bool),
    Drop,
    /// Semitones for bass and acid (MIDI keyboard)
    Transpose(i8),
    /// Song mode: play a form with the patterns (None: stop). The same form again only
    /// updates the patterns.
    Arrange(Option<Box<Arrangement>>),
    /// Song mode: go to this section on the next bar
    Jump(usize),
    /// A fill by hand: in the second half of this bar, or of the next one when this bar's
    /// is under way – into a waiting pattern it always comes now
    Fill(Fill),
    /// Switch to this pattern (slot `usize`) on the next bar, with a fill before
    Queue(Box<Song>, usize),
    /// Record everything that plays from now on (true) or end the recording (false)
    Capture(bool),
    /// A click on every beat, higher on the one – for building a beat by hand
    Metronome(bool),
    /// Your own sounds in the project's slots (`sampler::CUSTOM` + slot in `Track::sound`)
    Samples(Box<Slots>),
    /// The library plays a sound (looped or once), `None` stops it
    Audition(Option<Arc<SampleBuf>>, bool),
    /// The auto mode builds on what's there: the empty tracks of this groove come in one per
    /// bar (kick first), then it goes on as usual
    Grow(Box<Song>),
    /// What you kept and undid in SOLAR OS – the auto mode's director listens to it
    Taste(crate::ear::Taste),
    /// How much pressure the auto mode puts on: −1 (chill) … 1 (turned up)
    Drive(f32),
}

pub enum Event {
    /// The engine's song, what changed, and how many `Cmd::Song` it had taken in by then –
    /// the UI ignores a song older than its own last edit
    Song(Box<Song>, Option<Change>, u64, Option<crate::ear::Why>),
    /// Song mode records what plays: the next piece of interleaved stereo
    Audio(Vec<f32>),
    /// The recording is complete (song over or stopped, plus the tail)
    Recorded,
}

impl Event {
    #[cfg(test)]
    pub fn change(self) -> Option<Change> {
        match self {
            Event::Song(_, c, ..) => c,
            _ => None,
        }
    }
}

/// Samples per recorded piece sent to the UI
const REC_CHUNK: usize = 32_768;

/// The live effects you hold down
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fx {
    Stutter,
    Echo,
    Wash,
    Crush,
    Gate,
    Tape,
}

impl Fx {
    pub const ALL: [Fx; 6] = [Fx::Stutter, Fx::Echo, Fx::Wash, Fx::Crush, Fx::Gate, Fx::Tape];

    pub fn name(self) -> &'static str {
        match self {
            Fx::Stutter => "STUTTER",
            Fx::Echo => "ECHO",
            Fx::Wash => "WASH",
            Fx::Crush => "CRUSH",
            Fx::Gate => "GATE",
            Fx::Tape => "TAPE STOP",
        }
    }

    /// Keyboard key (shown on the button)
    pub fn key(self) -> &'static str {
        match self {
            Fx::Stutter => "Q",
            Fx::Echo => "W",
            Fx::Wash => "E",
            Fx::Crush => "X",
            Fx::Gate => "C",
            Fx::Tape => "V",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Fx::Stutter => "Repeats two steps over and over",
            Fx::Echo => "Throws everything into a long echo",
            Fx::Wash => "Drowns the beat in a huge reverb",
            Fx::Crush => "Lo-fi: fewer bits, lower sample rate",
            Fx::Gate => "Chops the sound into 16th notes",
            Fx::Tape => "Slows down like a turntable switched off",
        }
    }

    pub fn index(self) -> usize {
        Fx::ALL.iter().position(|&f| f == self).unwrap_or(0)
    }
}

/// Song mode: a form (`form::FORMS`) played with the saved patterns
#[derive(Clone)]
pub struct Arrangement {
    pub patterns: [Option<Song>; 4],
    pub form: usize,
    /// What plays where no pattern is saved
    pub fallback: Song,
}

/// `section_start` of a song that starts on the next bar
const PENDING: u64 = u64::MAX;

/// Bands of the spectrum the visualizer shows (centre frequencies in Hz)
pub const BANDS: [f32; 8] = [55.0, 120.0, 260.0, 550.0, 1_200.0, 2_600.0, 5_500.0, 11_000.0];

/// Energy below which a track drops out (Voice::ALL order). The energy is the
/// arrangement, not the volume: below `BREAK_ZONE` it's a break (no kick, no bass),
/// then kick, bass, open hat and ride come in – from 70 everything plays, ghost hats
/// fill it up towards 85 – and from `DROP_ZONE` up it drops.
pub const LAYERS: [f32; TRACKS] = [0.3, 0.2, 0.1, 0.0, 0.55, 0.7, 0.4, 0.0];

/// Below this energy the groove is a break: kick and bass out, the sound closes and
/// opens up into the reverb the lower it goes
pub const BREAK_ZONE: f32 = 0.3;

/// How far the energy may move per step when Solarum steers it (curve, song mode)
const GLIDE: f32 = 0.02;

/// Lock-free state the UI reads every frame
pub struct Shared {
    pub playing: AtomicBool,
    /// Global index of the step that played last
    pub step: AtomicU64,
    /// Counts hits per track – the UI flashes when the number changes
    pub hits: [AtomicU32; TRACKS],
    /// Output peak (f32 bits)
    pub level: AtomicU32,
    /// Build-up progress in thousandths, 0 = no build-up
    pub build: AtomicU32,
    /// Song mode: section playing now + 1 (0 = no song) and the bar it started on
    pub part: AtomicU32,
    pub part_start: AtomicU64,
    pub transpose: AtomicI32,
    /// Loudness per `BANDS` band (f32 bits), for the visualizer
    pub bands: [AtomicU32; 8],
    /// The auto mode's cycle, for the log: the bar it started, its length in bars, the bar
    /// in it where the breakdown starts (`NO_BREAK`: a roller) and its heat in thousandths
    pub cycle_start: AtomicU64,
    /// What the engine itself turns, for the pads (f32 bits): the acid with the auto
    /// mode's sweep, and the filter (the breakdown's sweep, the hand's, smoothed)
    pub acid_now: AtomicU32,
    pub filter_now: AtomicU32,
    pub cycle_len: AtomicU32,
    pub cycle_break: AtomicU32,
    pub heat: AtomicU32,
    /// How low the cycle's breakdown goes (energy in thousandths), so the UI can draw the curve
    pub cycle_depth: AtomicU32,
    /// Auto mode: the bar the next change lands on (`u64::MAX`: none planned)
    pub next_change: AtomicU64,
    /// The drive the energy slider set (f32 bits, −1…1), for the map's curve
    pub drive: AtomicU32,
    /// Set by the UI: while SOLAR OS plays, the energy may add ghost hats and thin out the
    /// percussion. By hand (auto off, no song) the pattern always plays exactly as written.
    pub touches: AtomicBool,
    /// Where the library's audition is in its sound (`SAMPLE_SR` samples), `u64::MAX` when silent
    pub audition: AtomicU64,
    /// Set when the output went NaN or infinite: the engine plays silence and the UI builds a new one
    pub broken: AtomicBool,
}

/// `Shared::cycle_break` of a cycle without a breakdown
pub const NO_BREAK: u32 = u32::MAX;

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Shared {
            playing: AtomicBool::new(false),
            step: AtomicU64::new(0),
            hits: Default::default(),
            level: AtomicU32::new(0),
            build: AtomicU32::new(0),
            part: AtomicU32::new(0),
            part_start: AtomicU64::new(0),
            transpose: AtomicI32::new(0),
            bands: Default::default(),
            cycle_start: AtomicU64::new(0),
            acid_now: AtomicU32::new(0.5f32.to_bits()),
            filter_now: AtomicU32::new(0),
            cycle_len: AtomicU32::new(32),
            cycle_break: AtomicU32::new(NO_BREAK),
            touches: AtomicBool::new(true),
            heat: AtomicU32::new(500),
            cycle_depth: AtomicU32::new(200),
            next_change: AtomicU64::new(u64::MAX),
            drive: AtomicU32::new(0),
            audition: AtomicU64::new(u64::MAX),
            broken: AtomicBool::new(false),
        })
    }

    pub fn bands(&self) -> [f32; 8] {
        std::array::from_fn(|i| f32::from_bits(self.bands[i].load(Ordering::Relaxed)))
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn build_progress(&self) -> Option<f32> {
        match self.build.load(Ordering::Relaxed) {
            0 => None,
            p => Some(p as f32 / 1000.0),
        }
    }
}

// Mixer: pan (−1…1), delay send, reverb send – per track, in Voice::ALL order
const PAN: [f32; TRACKS] = [0.0, 0.0, 0.3, -0.25, 0.22, -0.35, 0.0, 0.12];
const DELAY_SEND: [f32; TRACKS] = [0.0, 0.18, 0.4, 0.12, 0.18, 0.22, 0.0, 0.35];
const VERB_SEND: [f32; TRACKS] = [0.04, 0.55, 0.4, 0.15, 0.3, 0.35, 0.0, 0.25];

/// The mix into the master: the voices add up to well over full scale, the master wants headroom
const MIX_TRIM: f32 = 0.5;
/// Crush: the sample rate it holds at, and its steps (on the square root of the level, about 6 bits)
const CRUSH_RATE: f32 = 9000.0;
const CRUSH_STEPS: f32 = 24.0;
/// Tape stop: seconds to stand still, and to spin back up when let go
const TAPE_STOP: f32 = 0.8;
const TAPE_START: f32 = 0.35;
/// How much of the mix goes into the reverb at full wash
const WASH_SEND: f32 = 1.2;

/// A stable pseudo-random number 0…1 for a key – same key, same number
fn hash01(key: u64) -> f32 {
    let mut z = key.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
}

pub struct Engine {
    sr: f32,
    pub song: Song,
    playing: bool,
    breakdown: Option<Breakdown>,
    plan: Plan,
    /// Auto mode: the bar the current cycle started on and its length in bars
    cycle_start: u64,
    cycle_len: u64,
    /// Auto mode: how hot this cycle of the set is (0…1) – it climbs over a few cycles to a
    /// peak, then the set breathes out and climbs again
    heat: f32,
    /// What the auto mode changed lately
    recent: gen::Recent,
    /// The theme: the tracks as the cycle began – the groove wanders off and comes back
    home: [crate::song::Track; TRACKS],
    /// What you like (from SOLAR OS)
    taste: crate::ear::Taste,
    /// chill (−1) … turnup (1): lifts or lowers the set's heat and how busy the groove gets
    drive: f32,
    /// Tracks you just edited by hand stay yours until this bar
    hands_off: [u64; TRACKS],
    /// Why the director picked the change that goes out next
    why: Option<crate::ear::Why>,
    fill: Option<(u64, Fill)>,
    last_fill: Option<Fill>,
    arrangement: Option<Box<Arrangement>>,
    /// Song mode: the section playing, the bar it started on (`PENDING`: next bar) and a jump
    section: usize,
    section_start: u64,
    jump: Option<usize>,
    /// A pattern waiting for the next bar
    queued: Option<(Box<Song>, usize)>,
    /// The slot whose pattern plays (as far as the engine changed it)
    pattern_now: Option<usize>,
    /// The bar playing now (chord progression)
    bar_now: u64,
    phase: f64,
    last_fired: Option<u64>,
    half_fired: Option<u64>,
    rng: Rng,
    auto_rng: Rng,

    kick: Kick,
    clap: Clap,
    rim: Rim,
    hat: Hat,
    open_hat: Hat,
    ride: Hat,
    samples: Slots,
    samplers: [Sampler; TRACKS],
    audition: Audition,
    bass: Bass,
    acid: Acid,
    boom: Boom,

    delay: Delay,
    reverb: Reverb,
    rumble: Rumble,
    filter_l: Svf,
    filter_r: Svf,
    build_hp_l: Svf,
    build_hp_r: Svf,
    tone_l: Svf,
    tone_r: Svf,
    riser: Svf,
    crash: f32,
    filter: f32,
    filter_target: f32,
    /// Resonance of the filter: where the hand wants it and where it is (smoothed)
    res_target: f32,
    res: f32,
    /// Held live effects, in `Fx::ALL` order
    fx: [bool; 6],
    echo_amount: f32,
    /// Echo throw from a fill, fades out by itself
    throw: f32,
    wash: f32,
    /// The wash's room: grows with it, shrinks slowly after, so the tail rings out
    wash_room: f32,
    /// Keeps kick and bass out of the wash, so it drowns instead of booming
    wash_hp: Svf,
    crush: f32,
    /// Crush's own sample clock (it holds at CRUSH_RATE, whatever the device runs at)
    crush_count: f32,
    crush_hold: (f32, f32),
    /// Rounds off the steps of the held signal a little
    crush_lp: (f32, f32),
    gate: f32,
    gate_env: f32,
    /// Tape stop: the last two seconds of output, read back slower and slower
    tape: Vec<f32>,
    tape_w: usize,
    tape_r: f64,
    tape_rate: f32,
    taping: bool,
    /// How much of the output comes from the tape: 1 while it stops and spins back up, then fades to the live mix
    tape_mix: f32,
    /// Pump: everything ducks under the kick
    pump_env: f32,
    eq_low: [Svf; 2],
    eq_mid: [Svf; 2],
    eq_high: [Svf; 2],
    /// Spectrum for the visualizer: one band-pass and envelope per band
    band_filters: [Svf; 8],
    band_env: [f32; 8],
    band_peak: [f32; 8],
    stutter: Option<u64>,
    build: Option<Build>,
    /// Build-up filter amount; it lets go quickly after the drop instead of jumping
    build_cut: f32,
    riser_gain: f32,
    /// Where the energy curve (auto, song) wants to go – the energy glides there step by step
    energy_goal: f32,
    /// The energy the layers follow: taken on every beat, so tracks come and go in time
    layer_energy: f32,
    /// The energy smoothed for the sound (break filter, space, bass brightness)
    energy_smooth: f32,
    break_lp_l: Svf,
    break_lp_r: Svf,
    /// Auto mode: drops since the last new chapter
    drops_since_chapter: u32,
    /// The metronome: on, and samples since its last click (and whether that was the one)
    metronome: bool,
    click: f32,
    click_one: bool,
    /// What the auto mode is growing into (`Cmd::Grow`): one empty track comes in per bar
    grow: Option<Box<Song>>,
    /// Tracks the auto mode's energy muted – it unmutes them when the energy comes back
    energy_out: [bool; TRACKS],
    /// The layer energy on the last beat (auto mode: a track is muted when the energy crosses its layer)
    last_layer: f32,
    /// AUTO FX moving the mix: where you put the knobs, how far off they are now and where they head
    knob_home: [f32; KNOBS],
    knob_off: [f32; KNOBS],
    knob_goal: [f32; KNOBS],
    /// The knobs in the last song sent to the UI – a different value coming back is a hand on it
    knob_sent: std::cell::Cell<[f32; KNOBS]>,
    /// A mute the energy set that the UI hasn't sent back yet: its songs may still carry the
    /// old one (they cross the engine's on the way), so until it agrees, the engine's stays
    mute_pending: [Option<bool>; TRACKS],
    /// `Cmd::Song`s taken in so far (sent back with every song)
    songs_in: u64,
    transpose: i8,
    master: Master,
    peak: f32,

    shared: Arc<Shared>,
    events: Option<Sender<Event>>,
    /// Song mode recording: on while a song plays, then `rec_tail` more frames for the reverb
    recording: bool,
    rec: Vec<f32>,
    /// Frames of tail still to record once the recording ends (None: not ending)
    rec_tail: Option<u32>,
    /// The whole session is being recorded (`Cmd::Capture`), not just a song
    capturing: bool,
}

impl Engine {
    pub fn new(sr: f32, song: Song, shared: Arc<Shared>, events: Option<Sender<Event>>, seed: u64) -> Self {
        // A new engine (after the sound device changed) starts untransposed: say so
        shared.transpose.store(0, Ordering::Relaxed);
        Engine {
            sr,
            song,
            playing: false,
            breakdown: None,
            plan: PLANS[0],
            cycle_start: 0,
            cycle_len: 32,
            heat: 0.5,
            recent: Default::default(),
            home: song.tracks,
            taste: Default::default(),
            drive: 0.0,
            hands_off: [0; TRACKS],
            why: None,
            fill: None,
            last_fill: None,
            arrangement: None,
            section: 0,
            section_start: PENDING,
            jump: None,
            queued: None,
            pattern_now: None,
            bar_now: 0,
            phase: 0.0,
            last_fired: None,
            half_fired: None,
            rng: Rng::new(seed),
            auto_rng: Rng::new(seed ^ 0xA5A5),
            kick: Kick::new(),
            clap: Clap::new(sr),
            rim: Rim::new(sr),
            hat: Hat::new(sr, &HATS[0]),
            open_hat: Hat::new(sr, &OPEN_HATS[0]),
            ride: Hat::new(sr, &RIDES[0]),
            samples: Default::default(),
            samplers: Default::default(),
            audition: Audition::default(),
            bass: Bass::new(sr),
            acid: Acid::new(sr),
            boom: Boom::default(),
            delay: Delay::new(sr),
            reverb: Reverb::new(sr),
            rumble: Rumble::new(sr),
            filter_l: Svf::new(1000.0, 0.8, sr),
            filter_r: Svf::new(1000.0, 0.8, sr),
            build_hp_l: Svf::new(20.0, 0.7, sr),
            build_hp_r: Svf::new(20.0, 0.7, sr),
            tone_l: Svf::new(18_000.0, 0.6, sr),
            tone_r: Svf::new(18_000.0, 0.6, sr),
            riser: Svf::new(500.0, 3.0, sr),
            crash: 0.0,
            filter: 0.0,
            filter_target: 0.0,
            res_target: 0.0,
            res: 0.0,
            fx: [false; 6],
            echo_amount: 0.0,
            throw: 0.0,
            wash: 0.0,
            wash_room: 0.0,
            wash_hp: Svf::new(280.0, 0.7, sr),
            crush: 0.0,
            crush_count: 0.0,
            crush_hold: (0.0, 0.0),
            crush_lp: (0.0, 0.0),
            gate: 0.0,
            gate_env: 1.0,
            tape: vec![0.0; (sr * 2.0) as usize * 2],
            tape_w: 0,
            tape_r: 0.0,
            tape_rate: 1.0,
            taping: false,
            tape_mix: 0.0,
            pump_env: 0.0,
            eq_low: [Svf::new(110.0, 0.7, sr), Svf::new(110.0, 0.7, sr)],
            eq_mid: [Svf::new(1_000.0, 0.8, sr), Svf::new(1_000.0, 0.8, sr)],
            eq_high: [Svf::new(7_000.0, 0.7, sr), Svf::new(7_000.0, 0.7, sr)],
            band_filters: BANDS.map(|f| Svf::new(f, 1.4, sr)),
            band_env: [0.0; 8],
            band_peak: [0.0; 8],
            stutter: None,
            build: None,
            build_cut: 0.0,
            riser_gain: 0.0,
            energy_goal: song.macros.energy,
            layer_energy: song.macros.energy,
            energy_smooth: song.macros.energy,
            break_lp_l: Svf::new(18_000.0, 0.7, sr),
            break_lp_r: Svf::new(18_000.0, 0.7, sr),
            drops_since_chapter: 0,
            grow: None,
            metronome: false,
            click: f32::MAX,
            click_one: false,
            energy_out: [false; TRACKS],
            last_layer: 1.0,
            knob_home: knobs(&song),
            knob_off: [0.0; KNOBS],
            knob_goal: [0.0; KNOBS],
            knob_sent: std::cell::Cell::new(knobs(&song)),
            mute_pending: [None; TRACKS],
            songs_in: 0,
            transpose: 0,
            master: Master::new(sr),
            peak: 0.0,
            shared,
            events,
            recording: false,
            rec: Vec::new(),
            rec_tail: None,
            capturing: false,
        }
    }

    pub fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Song(s) => {
                self.songs_in += 1;
                // With the energy curve on (auto or song mode), the engine owns the energy
                let energy = self.song.macros.energy;
                let fx_was = self.song.auto_fx;
                // AUTO FX moves the mix: a knob you turned is yours (its new home), the others
                // keep where the engine has them
                let (now, sent, theirs) = (knobs(&self.song), self.knob_sent.get(), knobs(&s));
                let mut keep = theirs;
                for i in 0..KNOBS {
                    if (theirs[i] - sent[i]).abs() > 1e-4 || self.knob_off[i] == 0.0 {
                        self.knob_home[i] = theirs[i];
                        self.knob_off[i] = 0.0;
                        self.knob_goal[i] = 0.0;
                    } else {
                        keep[i] = now[i];
                    }
                }
                let steps_before = self.song.tracks.map(|t| t.steps);
                // One or two tracks edited by hand while the auto mode plays: they're yours for
                // two phrases (more at once is a new groove, not an edit)
                let edited = self.song.tracks.map(|t| (t.steps, t.len, t.sound));
                let theirs_tracks = s.tracks.map(|t| (t.steps, t.len, t.sound));
                let n = edited.iter().zip(&theirs_tracks).filter(|(a, b)| a != b).count();
                if self.song.auto && (1..=2).contains(&n) {
                    for (i, (a, b)) in edited.iter().zip(&theirs_tracks).enumerate() {
                        if a != b {
                            // Yours – and what you made is the theme it comes back to
                            self.hands_off[i] = self.bar_now + 16;
                            self.home[i] = s.tracks[i];
                        }
                    }
                }
                self.song = *s;
                set_knobs(&mut self.song, keep);
                // The energy's mutes win over a song from the UI that hasn't seen them yet
                let mut corrected = false;
                for ((t, pending), before) in self.song.tracks.iter_mut().zip(&mut self.mute_pending).zip(&steps_before) {
                    // Another groove on this track (a new project, a pattern): the energy lets go
                    if t.steps != *before {
                        *pending = None;
                    }
                    if let Some(v) = *pending {
                        if t.mute == v {
                            *pending = None;
                        } else {
                            t.mute = v;
                            corrected = true;
                        }
                    }
                }
                if self.steers_energy() {
                    self.song.macros.energy = energy;
                }
                // AUTO FX switched off: the bar decides now – a running breakdown lets go
                // (the UI has already brought every track back)
                if fx_was && !self.song.auto_fx && self.song.auto {
                    self.breakdown = None;
                }
                self.song.sanitize();
                // Auto mode switched off in the middle of a breakdown: bring the beat back
                if self.breakdown.is_some() && !self.song.auto && self.arrangement.is_none() {
                    self.end_breakdown();
                }
                // Stopped, the energy still mutes what it takes out – so it shows before you play
                if !self.playing {
                    self.layer_energy = self.song.macros.energy;
                    corrected |= self.energy_mutes();
                }
                if corrected {
                    self.send_song(None);
                }
            }
            Cmd::Taste(t) => self.taste = t,
            Cmd::Drive(d) => {
                self.drive = d.clamp(-1.0, 1.0);
                self.shared.drive.store(self.drive.to_bits(), Ordering::Relaxed);
                // The energy heads for the lifted curve right away, not on the next bar
                if self.song.auto && self.song.auto_fx && self.arrangement.is_none() && self.playing {
                    self.energy_goal = self.curve_target(self.bar_now.saturating_sub(self.cycle_start));
                }
                // The curve and the map follow at once; the energy glides there
                self.publish_cycle();
            }
            Cmd::Play => {
                if !self.playing {
                    self.playing = true;
                    self.phase = 0.0;
                    self.last_fired = None;
                    self.half_fired = None;
                    self.fill = None;
                    self.plan = PLANS[0];
                    self.cycle_start = 0;
                    self.cycle_len = 32;
                    self.heat = 0.5;
                    self.publish_cycle();
                }
            }
            Cmd::Stop => {
                self.playing = false;
                self.arrangement = None;
                self.end_recording();
                self.queued = None;
                self.shared.part.store(0, Ordering::Relaxed);
                if self.breakdown.is_some() {
                    self.end_breakdown();
                }
                self.build = None;
                self.stutter = None;
                self.fill = None;
                self.shared.build.store(0, Ordering::Relaxed);
            }
            Cmd::Hit { voice, vel, record } => {
                // Pads play the root – the synth one octave up, where its lines live
                let note = if voice == Voice::Acid { 12 } else { 0 };
                self.trigger(voice, vel, false, note, 0, false);
                if record && self.playing {
                    self.record(voice);
                }
            }
            Cmd::Filter(x) => self.filter_target = x.clamp(-1.0, 1.0),
            Cmd::FilterRes(r) => self.res_target = r.clamp(0.0, 1.0),
            Cmd::Fx(fx, on) => {
                self.fx[fx.index()] = on;
                if fx == Fx::Stutter {
                    self.stutter = if on { Some(self.last_fired.map_or(0, |s| s + 1)) } else { None };
                }
            }
            Cmd::Drop => {
                if self.playing && self.build.is_none() {
                    self.start_build();
                }
            }
            Cmd::Transpose(t) => {
                self.transpose = t;
                self.shared.transpose.store(t as i32, Ordering::Relaxed);
            }
            Cmd::Arrange(Some(a)) => {
                let same = self.arrangement.as_ref().is_some_and(|old| old.form == a.form);
                if !same {
                    if self.breakdown.is_some() {
                        self.end_breakdown();
                    }
                    self.section = 0;
                    self.section_start = PENDING;
                    self.jump = None;
                }
                self.arrangement = Some(a);
            }
            Cmd::Arrange(None) => {
                if self.arrangement.take().is_some() && self.breakdown.is_some() {
                    self.end_breakdown();
                }
                self.end_recording();
                self.shared.part.store(0, Ordering::Relaxed);
            }
            Cmd::Jump(i) => self.jump = Some(i),
            Cmd::Fill(f) => {
                if self.playing {
                    let next = self.last_fired.map_or(0, |s| s + 1);
                    let (bar, in_bar) = (next / STEPS as u64, next % STEPS as u64);
                    let late = in_bar >= 8 && self.queued.is_none();
                    self.fill = Some((bar + late as u64, f));
                }
            }
            Cmd::Metronome(on) => self.metronome = on,
            Cmd::Samples(slots) => self.samples = *slots,
            Cmd::Audition(buf, looped) => {
                self.audition.looped = None;
                match buf {
                    Some(b) => {
                        self.audition.voice.trigger(&b, 0.0, 1.0, None, self.sr);
                        if looped {
                            self.audition.looped = Some(b);
                        }
                    }
                    None => self.audition.voice = Sampler::default(),
                }
            }
            Cmd::Grow(target) => {
                self.grow = Some(target);
                // A fresh cycle once it's grown, and enough energy for everything to play
                self.cycle_start = u64::MAX;
                self.energy_goal = self.song.macros.energy.max(0.8);
                self.song.macros.energy = self.energy_goal;
                self.layer_energy = self.energy_goal;
            }
            Cmd::Capture(true) => {
                if self.events.is_some() {
                    self.finish_recording();
                    self.recording = true;
                    self.capturing = true;
                    self.rec = Vec::with_capacity(REC_CHUNK + 4096);
                }
            }
            Cmd::Capture(false) => {
                self.capturing = false;
                self.end_recording();
            }
            Cmd::Queue(s, slot) => {
                if self.playing {
                    self.queued = Some((s, slot));
                    // With SOLAR OS on a clap roll leads into it, unless a build-up is on – by
                    // hand the pattern just switches on the one
                    let bar = self.last_fired.map_or(0, |i| i / STEPS as u64);
                    if self.build.is_none() && self.song.auto {
                        self.fill = Some((bar, Fill::Roll));
                    }
                } else {
                    self.load_pattern(*s, Some(slot));
                    self.send_song(Some(Change::Scene(slot)));
                }
            }
        }
    }

    pub fn drain(&mut self, rx: &Receiver<Cmd>) {
        while let Ok(cmd) = rx.try_recv() {
            self.handle(cmd);
        }
    }

    fn step_len(&self) -> f64 {
        // Wider than `song::BPM` on purpose: only a guard against dividing by nothing (the
        // UI and files keep to `BPM`; tests run at 200 to stay short)
        self.sr as f64 * 60.0 / self.song.bpm.clamp(60.0, 200.0) as f64 / 4.0
    }

    fn send_song(&mut self, change: Option<Change>) {
        self.knob_sent.set(knobs(&self.song));
        let why = self.why.take();
        if let Some(tx) = &self.events {
            let _ = tx.send(Event::Song(Box::new(self.song), change, self.songs_in, why));
        }
    }

    /// Write a live hit into the nearest step of the pattern
    fn record(&mut self, voice: Voice) {
        let idx = self.phase.floor() as u64;
        let target = if self.phase.fract() < 0.5 { idx } else { idx + 1 };
        let t = self.song.track_mut(voice);
        let si = t.step_at(target);
        t.steps[si].on = true;
        t.steps[si].chance = 100;
        t.mute = false;
        // What you record should be heard: lift the energy just enough to let its track in
        // (unless Solarum steers the energy – then it comes in when the energy gets there)
        let layer = LAYERS[voice.index()];
        if self.song.macros.energy < layer && !self.steers_energy() {
            self.song.macros.energy = layer + 0.01;
            self.layer_energy = self.song.macros.energy;
        }
        // Recorded right after the step already played? Don't wait a whole loop to hear it
        self.send_song(None);
    }

    /// The song is over (or the session recording ends): two more seconds for the tail,
    /// then the recording is complete. A session recording goes on until it's switched off.
    fn end_recording(&mut self) {
        if self.recording && !self.capturing && self.rec_tail.is_none() {
            self.rec_tail = Some((self.sr * 2.0) as u32);
        }
    }

    /// Send what's left of the recording and say it's complete
    fn finish_recording(&mut self) {
        if !self.recording {
            return;
        }
        self.recording = false;
        self.rec_tail = None;
        if let Some(tx) = &self.events {
            let _ = tx.send(Event::Audio(std::mem::take(&mut self.rec)));
            let _ = tx.send(Event::Recorded);
        }
    }

    /// Recording (a song, or the whole session): keep what was just rendered
    fn keep_recording(&mut self, out: &[f32]) {
        if !self.recording {
            return;
        }
        // A set recording only takes what plays: armed while stopped, paused while paused
        if self.capturing && !self.playing && self.rec_tail.is_none() {
            return;
        }
        let frames = out.len() / 2;
        let take = match &mut self.rec_tail {
            Some(left) => {
                let n = frames.min(*left as usize);
                *left -= n as u32;
                n
            }
            None => frames,
        };
        self.rec.extend_from_slice(&out[..take * 2]);
        if self.rec.len() >= REC_CHUNK {
            if let Some(tx) = &self.events {
                let full = std::mem::replace(&mut self.rec, Vec::with_capacity(REC_CHUNK + 4096));
                let _ = tx.send(Event::Audio(full));
            }
        }
        if self.rec_tail == Some(0) {
            self.finish_recording();
        }
    }

    fn fire(&mut self, idx: u64) {
        if idx.is_multiple_of(STEPS as u64) {
            self.new_bar(idx);
        }
        if self.metronome && idx.is_multiple_of(4) {
            self.click = 0.0;
            self.click_one = idx.is_multiple_of(STEPS as u64);
        }
        // Solarum steering the energy: it glides towards the curve instead of jumping
        if self.steers_energy() {
            let e = self.song.macros.energy;
            let next = e + (self.energy_goal - e).clamp(-GLIDE, GLIDE);
            if (next - e).abs() > 1e-4 {
                self.song.macros.energy = next;
                if idx.is_multiple_of(4) {
                    self.send_song(None);
                }
            }
        }
        // Tracks come and go on the beat
        if idx.is_multiple_of(4) || self.build.is_some() {
            self.layer_energy = self.song.macros.energy;
        }
        if idx.is_multiple_of(4) && self.energy_mutes() {
            self.send_song(None);
        }
        if self.drift_knobs() && idx.is_multiple_of(4) {
            self.send_song(None);
        }
        self.shared.step.store(idx, Ordering::Relaxed);
        let build = self.build_progress(idx as f64);
        let gap = self.in_drop_gap(idx as f64);
        let play_idx = match self.stutter {
            Some(origin) if idx >= origin => origin + (idx - origin) % 2,
            _ => idx,
        };
        let in_bar = idx % STEPS as u64;
        let fill = self.fill.filter(|(b, _)| *b == idx / STEPS as u64).map(|(_, f)| f);
        let step_len = self.step_len() as f32;
        let energy = self.layer_energy;
        let touches = self.touches();
        // The kick roll of a build-up comes in even when the energy or a breakdown took the kick out
        let kick = Voice::Kick.index();
        let kick_by_hand = self.breakdown.as_ref().map_or(self.song.tracks[kick].mute, |b| {
            if b.out.contains(&Voice::Kick) {
                b.before[kick]
            } else {
                self.song.tracks[kick].mute
            }
        }) && !self.energy_out[kick];
        // A hi-hat can't be open and closed at once: the closed one leaves the open one's steps alone
        let oh = self.song.track(Voice::OpenHat);
        // (What the energy takes out is muted – `energy_mutes` – so the mutes are all there is)
        let open_now = !oh.mute && oh.steps[oh.step_at(play_idx)].on;
        for (ti, voice) in Voice::ALL.into_iter().enumerate() {
            let track = self.song.tracks[ti];
            let roll = build.is_some() && voice == Voice::Kick && !kick_by_hand;
            if !roll && track.mute {
                continue;
            }
            if fill == Some(Fill::Gap) && in_bar >= 12 && matches!(voice, Voice::Kick | Voice::Bass) {
                continue;
            }
            // Build-up: the groove itself turns into it (`build_play`), silence in the gap
            let mut track_idx = play_idx;
            let mut lift = 0;
            if let (Some(p), Some(start)) = (build, self.build.as_ref().map(|b| b.start)) {
                if gap {
                    continue;
                }
                match build_play(voice, p, idx, start) {
                    BuildPlay::Pattern => {}
                    BuildPlay::Rest => continue,
                    BuildPlay::Hit(v) => {
                        self.trigger(voice, track.volume * v, v > 0.85, 0, 0, false);
                        continue;
                    }
                    BuildPlay::Loop { at, up } => {
                        track_idx = at;
                        lift = up;
                    }
                }
            }
            if voice == Voice::Hat && open_now && build.is_none() {
                continue;
            }
            let si = track.step_at(track_idx);
            let step = track.steps[si];
            let mut vel = track.volume * if step.accent { 1.0 } else { 0.72 } * if step.ghost { 0.5 } else { 1.0 };
            // With SOLAR OS on, energy thins out the percussion (below 45) or adds ghost hats (above 60);
            // the choice changes every two bars
            let perc = matches!(voice, Voice::Hat | Voice::Ride | Voice::Rim | Voice::OpenHat);
            let h = hash01(ti as u64 * 7919 + si as u64 * 104_729 + (play_idx / 32) * 15_485_863);
            let mut on = step.on;
            if on && touches && perc && !step.accent && energy < 0.45 && h < (0.45 - energy) * 1.8 {
                on = false;
            }
            // (only on a hat that plays something – an empty track stays silent)
            if !step.on && touches && voice == Voice::Hat && energy > 0.7 && track.hits() > 0 && h < (energy - 0.7) * 0.8 {
                on = true;
                vel *= 0.5;
            }
            if !on || (step.on && step.chance < 100 && self.rng.below(100) >= step.chance as u32) {
                continue;
            }
            let next = track.steps[(si + 1) % track.len.max(1) as usize];
            let legato = voice == Voice::Acid && next.on && next.slide;
            let gate = if voice == Voice::Acid && gen::synth_family(track.sound) == 4 {
                // A pad holds its chord until the next one
                let len = track.len.max(1) as usize;
                let gap = (1..=len).find(|k| track.steps[(si + k) % len].on).unwrap_or(len);
                step_len * (gap as f32 - 0.1)
            } else if legato {
                step_len * 1.05
            } else {
                step_len * 0.5
            };
            self.trigger(voice, vel, step.accent, step.note + lift, gate as u32, step.slide && step.on);
        }
        // Build-up: a clap roll that gets faster and louder
        if let Some(p) = build.filter(|_| !gap) {
            if p < 0.5 && idx.is_multiple_of(2) || p >= 0.5 {
                let v = 0.35 + 0.55 * p;
                self.clap_hit(v, 0.08);
                self.count_hit(Voice::Clap);
            }
        }
        if build.is_none() {
            self.play_fill(fill, in_bar);
        }
    }

    fn play_fill(&mut self, fill: Option<Fill>, s: u64) {
        let sr = self.sr;
        match fill {
            Some(Fill::Roll) if s >= 8 && (s >= 12 || s.is_multiple_of(2)) => {
                self.clap_hit(0.25 + 0.06 * (s - 8) as f32, 0.07);
                self.count_hit(Voice::Clap);
            }
            Some(Fill::Toms) if s >= 10 => {
                self.rim
                    .trigger(0.55 + 0.05 * (s - 10) as f32, 1.45 - (s - 10) as f32 * 0.1, 3, 1.0);
                self.count_hit(Voice::Rim);
            }
            Some(Fill::OpenHats) if s >= 8 && s.is_multiple_of(2) => {
                let m = &OPEN_HATS[self.song.track(Voice::OpenHat).sound as usize % OPEN_HATS.len()];
                self.open_hat.set_model(m, 1.0, sr);
                self.open_hat.trigger(0.3 + 0.04 * (s - 8) as f32, 0.12 * m.length, sr);
                self.count_hit(Voice::OpenHat);
            }
            Some(Fill::Throw) if s == 12 => {
                self.throw = 1.0;
                self.clap_hit(0.7, 0.15);
                self.count_hit(Voice::Clap);
            }
            _ => {}
        }
    }

    /// A clap for build-ups and fills, in the clap track's sound and tuning
    fn clap_hit(&mut self, vel: f32, length: f32) {
        let t = self.song.track(Voice::Clap);
        let (sound, tune) = (t.sound, t.tune_ratio());
        self.clap.trigger(vel, length, self.sr, sound, tune);
    }

    fn count_hit(&self, v: Voice) {
        self.shared.hits[v.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// Start a sound. `note` in semitones above the root (synths only)
    fn trigger(&mut self, voice: Voice, vel: f32, accent: bool, note: i8, gate: u32, slide: bool) {
        let sr = self.sr;
        let m = self.song.macros;
        let track = *self.song.track(voice);
        let (tune, decay) = (track.tune_ratio(), track.decay_factor());
        let gate = if gate == 0 { self.step_len() as f32 * 0.6 } else { gate as f32 };
        // Bass and synth follow the chord progression, bar by bar
        let sc = self.song.scale;
        let note = if voice.is_synth() {
            sc.shift(note, self.song.prog_degree(self.bar_now))
        } else {
            note
        };
        let pitch = (self.song.root as i32 + note as i32 + self.transpose as i32 + track.tune as i32) as f32;
        let sound = track.sound;
        if let Some(k) = slot_of(sound) {
            // One of your own sounds: it plays out like a one-shot (a decay below the middle
            // shortens it); bass and synth follow the notes and the key (a sound counts as C)
            if let Some(buf) = self.samples[k].clone() {
                let key = ((self.song.root as i32 - 36 + 6).rem_euclid(12) - 6) as f32;
                let semis = track.tune as f32
                    + if voice.is_synth() {
                        key + pitch - self.song.root as f32 - track.tune as f32
                    } else {
                        0.0
                    };
                let len = buf.data.len() as f32 * sr / crate::sampler::SAMPLE_SR;
                let hold = (decay < 1.0).then_some(len * decay);
                // Levelled to sit like the built-in sounds (a normalized sound is far louder)
                let vel = vel * crate::sampler::LEVEL[voice.index()];
                self.samplers[voice.index()].trigger(&buf, semis, vel, hold, sr);
                if voice == Voice::Kick {
                    self.pump_env = 1.0;
                }
            }
            self.count_hit(voice);
            return;
        }
        let pick = |n: usize| sound as usize % n;
        match voice {
            Voice::Kick => {
                let k = &KICKS[pick(KICKS.len())];
                self.kick.set_model(k);
                self.kick.tune = (47.0 + (self.song.root % 12) as f32 * 0.6) * k.tune * tune;
                let hard = if matches!(self.song.vibe, Vibe::Industrial | Vibe::Hard) {
                    1.5
                } else {
                    0.0
                };
                self.kick.drive = (2.5 + hard) * k.drive + k.drive_add;
                self.kick.trigger(vel, (0.11 + m.dark * 0.2) * k.length * decay, sr);
                self.pump_env = 1.0;
            }
            Voice::Clap => self.clap.trigger(vel * 1.3, (0.1 + m.space * 0.15) * decay, sr, sound, tune),
            Voice::Rim => self.rim.trigger(vel * 1.4, tune * if accent { 1.06 } else { 1.0 }, sound, decay),
            Voice::Hat => {
                let h = &HATS[pick(HATS.len())];
                self.hat.set_model(h, tune, sr);
                self.open_hat.choke(sr);
                self.hat.trigger(vel * 1.05, (0.03 + m.energy * 0.035) * h.length * decay, sr);
            }
            Voice::OpenHat => {
                let h = &OPEN_HATS[pick(OPEN_HATS.len())];
                self.open_hat.set_model(h, tune, sr);
                self.open_hat.trigger(vel * 0.85, (0.18 + m.energy * 0.2) * h.length * decay, sr);
            }
            Voice::Ride => {
                let h = &RIDES[pick(RIDES.len())];
                self.ride.set_model(h, tune, sr);
                self.ride.trigger(vel * 0.9, (0.45 + m.space * 0.4) * h.length * decay, sr);
            }
            Voice::Bass => self.bass.trigger(pitch, vel, gate * decay.min(1.5), sr, sound, decay),
            Voice::Acid => {
                // Stab chords: root, third and fifth of the scale on this note
                let base = sc.shift(note, 0) as f32;
                let chord = [0.0, sc.shift(note, 2) as f32 - base, sc.shift(note, 4) as f32 - base];
                let gate = gate * decay.min(1.5);
                if slide && sound < 2 {
                    self.acid.slide(pitch, accent, gate, sr);
                } else {
                    self.acid.trigger(pitch, vel, accent, gate, sr, sound, chord, decay);
                }
            }
        }
        self.count_hit(voice);
    }

    /// Fill an interleaved stereo buffer
    pub fn render(&mut self, out: &mut [f32]) {
        let sr = self.sr;
        let m = self.song.macros;
        // (The energy changes what plays, never how loud)
        let vol: [f32; TRACKS] = std::array::from_fn(|i| if self.song.tracks[i].mute { 0.0 } else { 1.0 });
        let step_len = self.step_len();
        let delay_time = (step_len * 3.0) as f32;
        let space = m.space;
        let dark = m.dark;
        // Break zone: the lower the energy, the more closed and spacious the sound
        let es_coef = 1.0 - (-1.0 / (0.12 * sr)).exp();
        let break_space = 1.0 + ((BREAK_ZONE - self.energy_smooth) / BREAK_ZONE).clamp(0.0, 1.0) * 1.2;
        // Darkness closes the master tone filter – all the way down to a muffled 1.5 kHz, but
        // only in its upper half: where DARK starts (0.45) the hats keep their air
        let tone = 18_000.0 * 0.1f32.powf(dark.powi(3) * 1.08);
        self.tone_l.set(tone, 0.6, sr);
        self.tone_r.tune_like(&self.tone_l);
        // Chord stabs live in the echo (dub)
        let mut delay_send = DELAY_SEND;
        if self.song.track(Voice::Acid).sound == 2 {
            delay_send[Voice::Acid.index()] = 0.75;
        }
        // The auto mode and songs turn the acid knob themselves: a slow sweep over 16 bars
        let acid = if (self.song.auto || self.arrangement.is_some()) && self.playing {
            (m.acid + 0.22 * (self.phase as f32 / 256.0 * std::f32::consts::TAU).sin()).clamp(0.0, 1.0)
        } else {
            m.acid
        };
        self.shared.acid_now.store(acid.to_bits(), Ordering::Relaxed);
        // (Stutter works on the steps, in `fire`)
        let [_, echo_on, wash_on, crush_on, gate_on, tape_on] = self.fx;
        // Pump: the duck lasts a bit more than a 16th
        let pump_coef = (-1.0 / (step_len as f32 * 1.3)).exp();
        let pump = m.pump;
        // Master EQ: ±EQ_DB per band (more only made it clip)
        let eq = self.song.eq;
        let db = |v: f32| 10f32.powf((v - 0.5) * 2.0 * crate::song::EQ_DB / 20.0) - 1.0;
        let (g_low, g_mid, g_high) = (db(eq.low), db(eq.mid), db(eq.high));
        let eq_on = g_low.abs() + g_mid.abs() + g_high.abs() > 0.01;
        let band_coef = (-1.0 / (0.03 * sr)).exp();
        let wash_release = (-1.0 / (2.5 * sr)).exp();
        // Crush fades in and out over about 30 ms; its held signal is rounded off a little above 5 kHz
        let crush_coef = 1.0 - (-1.0 / (0.03 * sr)).exp();
        let crush_lp = 1.0 - (-std::f32::consts::TAU * 5500.0 / sr).exp();
        let drive = Drive::new(eq.drive);
        // The gate's edges take about 2 ms, at any sample rate
        let gate_coef = 1.0 - (-1.0 / (0.002 * sr)).exp();

        for frame in out.as_chunks_mut::<2>().0 {
            // ── Clock ──
            if self.playing {
                self.phase += 1.0 / step_len;
                let idx = self.phase.floor() as u64;
                let frac = self.phase - idx as f64;
                let swing = if idx % 2 == 1 { self.song.swing as f64 * 0.5 } else { 0.0 };
                if self.last_fired != Some(idx) && frac >= swing {
                    self.last_fired = Some(idx);
                    self.fire(idx);
                }
                // Last quarter of the build-up: 32nd-note rolls (waiting for its bar shows as just begun)
                if self.build.is_some() {
                    let p = self.build_progress(self.phase).unwrap_or(0.0);
                    self.shared.build.store((p * 1000.0).max(1.0) as u32, Ordering::Relaxed);
                    if p > 0.75 && frac >= 0.5 && self.half_fired != Some(idx) && !self.in_drop_gap(self.phase) {
                        self.half_fired = Some(idx);
                        self.clap_hit(0.5 + 0.4 * p, 0.06);
                    }
                }
            }

            // ── Voices ──
            let kick_raw = self.kick.next(sr, &mut self.rng) + self.samplers[0].next();
            let duck = self.kick.envelope();
            self.pump_env *= pump_coef;
            // Everything but the kick breathes with it
            let pg = 1.0 - pump * 0.85 * self.pump_env.min(1.0).powf(0.6);
            let (synth_l, synth_r) = self.acid.next(sr, acid);
            let synth_gain = vol[7] * (1.0 - duck * 0.3) * pg;
            let v = [
                kick_raw * vol[0],
                self.clap.next(sr, &mut self.rng) * vol[1] * pg,
                self.rim.next(sr, &mut self.rng) * vol[2] * pg,
                self.hat.next(sr, &mut self.rng) * vol[3] * pg,
                self.open_hat.next(sr, &mut self.rng) * vol[4] * pg,
                self.ride.next(sr, &mut self.rng) * vol[5] * pg,
                self.bass.next(sr, self.energy_smooth, dark) * vol[6] * 2.0 * (1.0 - duck * 0.7) * pg,
                (synth_l + synth_r) * 0.5 * synth_gain,
            ];
            let mut v = v;
            for (i, x) in v.iter_mut().enumerate().skip(1) {
                // Bass and synth duck under the kick like their own sounds
                let duck = match i {
                    6 => 1.0 - duck * 0.7,
                    7 => 1.0 - duck * 0.3,
                    _ => 1.0,
                };
                *x += self.samplers[i].next() * vol[i] * pg * duck;
            }
            let (mut l, mut r) = (0.0f32, 0.0f32);
            let (mut dl, mut dr, mut rv) = (0.0f32, 0.0f32, 0.0f32);
            let echo_send = self.echo_amount;
            for i in 0..TRACKS {
                let x = v[i];
                let p = PAN[i];
                l += x * (1.0 - p).min(1.0);
                r += x * (1.0 + p).min(1.0);
                let ds = delay_send[i] * space * 2.0 + if i == 0 || i == 6 { 0.0 } else { echo_send };
                dl += x * ds * (1.0 - p * 0.5);
                dr += x * ds * (1.0 + p * 0.5);
                rv += x * VERB_SEND[i] * (0.15 + space * 1.6) * break_space;
            }
            // What the synth's chords have left and right of their middle
            let synth_side = (synth_l - synth_r) * 0.5 * synth_gain;
            l += synth_side;
            r -= synth_side;
            // Rumble and the drop
            let rumble = self.rumble.run(kick_raw * vol[0], duck) * dark * 1.3;
            l += rumble;
            r += rumble;
            let boom = self.boom.next(sr) * 0.8;
            l += boom;
            r += boom;
            // The metronome's click: a short blip, higher on the one
            if self.click < sr * 0.04 {
                let t = self.click / sr;
                let f = if self.click_one { 1760.0 } else { 1320.0 };
                let v = (std::f32::consts::TAU * f * t).sin() * (-t / 0.008).exp() * 0.22;
                l += v;
                r += v;
                self.click += 1.0;
            }
            if self.crash > 1e-4 {
                let n = self.rng.noise() * self.crash;
                rv += n * 0.9;
                l += n * 0.12;
                r += self.rng.noise() * self.crash * 0.12;
                self.crash *= 0.99995;
            }
            // Build-up: a noise riser sweeping up, fading out in the gap before the drop
            let build = self.build_progress(self.phase).filter(|_| self.playing);
            let riser_want = if build.is_some() && !self.in_drop_gap(self.phase) {
                1.0
            } else {
                0.0
            };
            self.riser_gain += (riser_want - self.riser_gain) * 0.0015;
            match build {
                Some(p) => self.build_cut = p,
                // After the drop the build-up filter lets go within a few milliseconds
                None => self.build_cut *= 0.998,
            }
            if self.riser_gain > 1e-4 {
                let p = self.build_cut;
                self.riser.set(400.0 * 30f32.powf(p), 2.5, sr);
                let noise = self.rng.noise();
                let n = self.riser.run(noise).bp * p * p * 0.5 * self.riser_gain;
                l += n;
                r += n;
                rv += n;
            }

            // ── Sends ──
            self.throw *= 0.99997;
            let echo_want = if echo_on { 0.9 } else { 0.0 } + self.throw * 0.6;
            self.echo_amount += (echo_want - self.echo_amount) * 0.0005;
            let fb = 0.25 + space * 0.4 + self.echo_amount * 0.35;
            let (el, er) = self.delay.run(dl, dr, delay_time, fb.min(0.85));
            // Wash: everything into the reverb, the dry beat steps back
            self.wash += ((if wash_on { 1.0 } else { 0.0 }) - self.wash) * 0.0008;
            self.wash_room = if self.wash > self.wash_room {
                self.wash
            } else {
                self.wash_room * wash_release
            };
            if self.wash > 1e-4 {
                rv += self.wash_hp.run((l + r) * 0.5).hp * self.wash * WASH_SEND;
            }
            let duck_dry = 1.0 - self.wash * 0.55;
            l = l * duck_dry + el * pg;
            r = r * duck_dry + er * pg;
            let size = (0.5 + space * 0.46 + self.wash_room * 0.5).min(0.98);
            let (rl, rr) = self.reverb.run(rv, size, 0.3 + dark * 0.5);
            l += rl * pg;
            r += rr * pg;

            // ── Master ──
            // With no hand on the filter, a breakdown may sweep it open by itself
            let auto_filter = match &self.breakdown {
                Some(b) if b.sweep && self.playing => {
                    let p = ((self.phase - b.start as f64) / b.len as f64).clamp(0.0, 1.0) as f32;
                    -0.75 * (1.0 - p)
                }
                _ => 0.0,
            };
            let target = if self.filter_target.abs() > 0.01 {
                self.filter_target
            } else {
                auto_filter
            };
            self.filter += (target - self.filter) * 0.002;
            self.res += (self.res_target - self.res) * 0.002;
            // Q 1.6 at rest (as it always was), up to about 16 – a singing, whistling filter –
            // with the resonance all the way up
            let q = 1.6 + 14.0 * self.res.powf(1.3);
            let f = self.filter;
            if f < -0.02 {
                let fc = 18_000.0 * 0.012f32.powf(-f);
                self.filter_l.set(fc, q, sr);
                self.filter_r.tune_like(&self.filter_l);
                l = self.filter_l.run(l).lp;
                r = self.filter_r.run(r).lp;
            } else if f > 0.02 {
                let fc = 20.0 * 120f32.powf(f);
                self.filter_l.set(fc, q, sr);
                self.filter_r.tune_like(&self.filter_l);
                l = self.filter_l.run(l).hp;
                r = self.filter_r.run(r).hp;
            }
            if self.build_cut > 0.002 {
                // A light high-pass opens upwards under the gate
                let fc = 20.0 * 80f32.powf(self.build_cut * 0.6);
                self.build_hp_l.set(fc, 0.9, sr);
                self.build_hp_r.tune_like(&self.build_hp_l);
                l = self.build_hp_l.run(l).hp;
                r = self.build_hp_r.run(r).hp;
            }
            l = self.tone_l.run(l).lp;
            r = self.tone_r.run(r).lp;
            self.energy_smooth += (m.energy - self.energy_smooth) * es_coef;
            let brk = ((BREAK_ZONE - self.energy_smooth) / BREAK_ZONE).clamp(0.0, 1.0);
            if brk > 0.001 {
                let fc = 18_000.0 * 0.12f32.powf(brk);
                self.break_lp_l.set(fc, 0.8, sr);
                self.break_lp_r.tune_like(&self.break_lp_l);
                l = self.break_lp_l.run(l).lp;
                r = self.break_lp_r.run(r).lp;
            }

            // Crush: one fixed lo-fi sound (a low sample rate, few bits) faded in and out, so switching
            // it never steps through other settings. The bits are companded: quiet tails get grainy
            // instead of dropping out.
            self.crush += ((if crush_on { 1.0 } else { 0.0 }) - self.crush) * crush_coef;
            if self.crush > 1e-3 {
                self.crush_count += CRUSH_RATE / sr;
                if self.crush_count >= 1.0 {
                    self.crush_count -= 1.0;
                    let q = |x: f32| {
                        let y = (x.abs().sqrt() * CRUSH_STEPS).round() / CRUSH_STEPS;
                        (y * y).copysign(x)
                    };
                    self.crush_hold = (q(l), q(r));
                }
                self.crush_lp.0 += (self.crush_hold.0 - self.crush_lp.0) * crush_lp;
                self.crush_lp.1 += (self.crush_hold.1 - self.crush_lp.1) * crush_lp;
                let c = self.crush;
                let m = c * c * (3.0 - 2.0 * c);
                l += (self.crush_lp.0 - l) * m;
                r += (self.crush_lp.1 - r) * m;
            } else {
                self.crush_lp = (l, r);
            }
            // Gate: open for the first half of every 16th (a gate build-up chops harder and harder)
            let build_gate = build.map_or(0.0, |p| (p * 1.3).min(1.0));
            let gate_want = if gate_on { 1.0 } else { build_gate };
            self.gate += (gate_want - self.gate) * 0.002;
            if self.gate > 0.002 {
                let open = !self.playing || self.phase.fract() < 0.5;
                self.gate_env += ((if open { 1.0 } else { 0.0 }) - self.gate_env) * gate_coef;
                let g = 1.0 - self.gate * (1.0 - self.gate_env);
                l *= g;
                r *= g;
            }

            // Master EQ: low shelf, mid bell, high shelf
            if eq_on {
                for (ch, x) in [&mut l, &mut r].into_iter().enumerate() {
                    let lo = self.eq_low[ch].run(*x).lp;
                    let mid = self.eq_mid[ch].run(*x).bp / 0.8;
                    let hi = self.eq_high[ch].run(*x).hp;
                    *x += lo * g_low + mid * g_mid + hi * g_high;
                }
            }
            (l, r) = self.master.run(l * MIX_TRIM, r * MIX_TRIM, drive, self.song.master);
            // A NaN left in a filter or the reverb would keep everything silent (or loud noise
            // on some devices): play silence and let the UI build a fresh engine
            if !(l.is_finite() && r.is_finite()) {
                l = 0.0;
                r = 0.0;
                self.shared.broken.store(true, Ordering::Relaxed);
            }

            // Tape stop: read the recent output back slower and slower until it stands still. Let go
            // and it spins back up, landing on the live beat, then hands over to it.
            let n = self.tape.len() / 2;
            let w = self.tape_w;
            self.tape[2 * w] = l;
            self.tape[2 * w + 1] = r;
            self.tape_w = (w + 1) % n;
            if tape_on {
                if !self.taping {
                    self.taping = true;
                    // Pressed again while it spins up: slow down from where it is
                    if self.tape_mix <= 0.0 {
                        self.tape_r = w as f64;
                        self.tape_rate = 1.0;
                    }
                }
                self.tape_mix = 1.0;
                self.tape_rate = (self.tape_rate - 1.0 / (TAPE_STOP * sr)).max(0.0);
            } else if self.tape_mix > 0.0 {
                if self.taping {
                    self.taping = false;
                    // Stood still (and silent): start the spin-up just far enough back that it
                    // catches up with the live output when it's back at full speed
                    if self.tape_rate == 0.0 {
                        let lag = (TAPE_START * sr * 0.5) as usize;
                        self.tape_r = ((w + n - lag) % n) as f64;
                    }
                }
                if self.tape_rate < 1.0 {
                    self.tape_rate = (self.tape_rate + 1.0 / (TAPE_START * sr)).min(1.0);
                } else {
                    self.tape_mix = (self.tape_mix - 1.0 / (0.02 * sr)).max(0.0);
                }
            }
            if self.tape_mix > 0.0 {
                let i = self.tape_r.floor() as usize % n;
                let j = (i + 1) % n;
                let f = self.tape_r.fract() as f32;
                let fade = (self.tape_rate * 3.0).min(1.0);
                let tl = (self.tape[2 * i] * (1.0 - f) + self.tape[2 * j] * f) * fade;
                let tr = (self.tape[2 * i + 1] * (1.0 - f) + self.tape[2 * j + 1] * f) * fade;
                l += (tl - l) * self.tape_mix;
                r += (tr - r) * self.tape_mix;
                self.tape_r = (self.tape_r + self.tape_rate as f64) % n as f64;
            }

            // The library's audition, after every effect
            let a = self.audition.next(sr) * 0.9;
            l += a;
            r += a;
            self.peak = self.peak.max(l.abs().max(r.abs()));
            frame[0] = l;
            frame[1] = r;

            // Spectrum for the visualizer
            let mono = (l + r) * 0.5;
            for i in 0..BANDS.len() {
                let y = self.band_filters[i].run(mono).bp.abs();
                self.band_env[i] = y.max(self.band_env[i] * band_coef);
                self.band_peak[i] = self.band_peak[i].max(self.band_env[i]);
            }
        }
        for i in 0..BANDS.len() {
            self.shared.bands[i].store(self.band_peak[i].to_bits(), Ordering::Relaxed);
            self.band_peak[i] = 0.0;
        }
        // Peak meter with a slow fall
        let shown = f32::from_bits(self.shared.level.load(Ordering::Relaxed));
        let fallen = (shown * 0.85).max(self.peak);
        self.shared.level.store(fallen.to_bits(), Ordering::Relaxed);
        let pos = self.audition.voice.position().map_or(u64::MAX, |p| p as u64);
        self.shared.audition.store(pos, Ordering::Relaxed);
        self.peak = 0.0;
        self.shared.playing.store(self.playing, Ordering::Relaxed);
        self.shared.filter_now.store(self.filter.to_bits(), Ordering::Relaxed);
        self.keep_recording(out);
    }
}

/// Render `bars` bars of the song (plus a reverb tail) into interleaved stereo (tests, demo loops)
#[cfg(test)]
pub fn render_offline(song: &Song, bars: u32, sr: u32, seed: u64) -> Vec<f32> {
    let mut e = Engine::new(sr as f32, *song, Shared::new(), None, seed);
    e.handle(Cmd::Play);
    let frames = (e.step_len() * (bars as usize * STEPS) as f64) as usize + sr as usize * 2;
    let mut out = vec![0.0f32; frames * 2];
    let stop_at = (e.step_len() * (bars as usize * STEPS) as f64) as usize * 2;
    for (i, chunk) in out.chunks_mut(512).enumerate() {
        if i * 512 >= stop_at {
            e.handle(Cmd::Stop);
        }
        e.render(chunk);
    }
    out
}

#[cfg(test)]
mod levels;
#[cfg(test)]
mod tests;
