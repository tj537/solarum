//! The audio engine: sequencer clock, voices, mixer and master effects.
//!
//! It runs on the audio thread. The UI talks to it with [`Cmd`] messages and
//! reads the playhead and hit lights from [`Shared`] atomics; when the engine
//! itself changes the song (auto mode, recording, song mode) it sends the new
//! song back as an [`Event`]. The same engine renders WAV exports offline.

use crate::form::{pattern_slot, Part, FORMS};
use crate::gen::{self, Change};
use crate::rng::Rng;
use crate::sampler::{slot_of, Audition, SampleBuf, Sampler, Slots};
use crate::song::{Song, Vibe, Voice, STEPS, TRACKS};
use crate::synth::*;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

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

/// A build-up: two bars from `start`. On top of the groove turning into it, a gate chops
/// harder and harder under a light high-pass
struct Build {
    start: u64,
    len: u64,
}

/// Crush: the sample rate it holds at, and its steps (on the square root of the level, about 6 bits)
const CRUSH_RATE: f32 = 9000.0;
const CRUSH_STEPS: f32 = 24.0;
/// Tape stop: seconds to stand still, and to spin back up when let go
const TAPE_STOP: f32 = 0.8;
const TAPE_START: f32 = 0.35;
/// How much of the mix goes into the reverb at full wash
const WASH_SEND: f32 = 1.2;

/// What a track plays at step `idx` of a build-up that started at `start` (progress `p`)
#[derive(Clone, Copy, PartialEq, Debug)]
enum BuildPlay {
    /// Its own pattern
    Pattern,
    Rest,
    /// A hit at this velocity, whatever the pattern says
    Hit(f32),
    /// The pattern looping a short piece from step `at`, `up` semitones higher
    Loop {
        at: u64,
        up: i8,
    },
}

/// The build-up made from the groove: in the first bar everything plays on, the kick
/// already rolling in quarters. In the second bar the kick rolls in 8ths, then 16ths,
/// the hats run in 16ths, the open hat rests, and bass, synth and percussion loop the
/// bar's first beat, then half a beat – the synths an octave up for the last stretch.
fn build_play(voice: Voice, p: f32, idx: u64, start: u64) -> BuildPlay {
    let second = start + STEPS as u64;
    let late = idx >= second;
    let last = p >= 0.75;
    match voice {
        Voice::Kick => {
            let every = if !late {
                4
            } else if !last {
                2
            } else {
                1
            };
            if (idx - start).is_multiple_of(every) {
                BuildPlay::Hit(0.6 + 0.4 * p)
            } else {
                BuildPlay::Rest
            }
        }
        Voice::Hat if late => BuildPlay::Hit(0.45 + 0.45 * (p - 0.5) * 2.0),
        Voice::OpenHat if late => BuildPlay::Rest,
        Voice::Bass | Voice::Acid | Voice::Rim | Voice::Ride if late => {
            let len = if last { 2 } else { 4 };
            let synth = matches!(voice, Voice::Bass | Voice::Acid);
            BuildPlay::Loop {
                at: second + (idx - second) % len,
                up: if last && synth { 12 } else { 0 },
            }
        }
        _ => BuildPlay::Pattern,
    }
}

/// Steps of silence before a drop lands (half a beat)
const DROP_GAP: u64 = 2;

/// Where a song's energy is about to drop (the slim song bar shows it) – by hand there's no
/// drop zone any more, DROP drops
pub const DROP_ZONE: f32 = 0.9;

/// How the auto mode arranges one 32-bar cycle
#[derive(Clone, Copy, PartialEq, Debug)]
struct Plan {
    /// Bar of the cycle where the breakdown starts (it lasts until the build-up at bar 30);
    /// 30: no breakdown, the groove rolls straight into the build-up
    breakdown_at: u64,
    /// Tracks that leave for the breakdown
    out: &'static [Voice],
    /// A low-pass filter opens slowly through the breakdown
    sweep: bool,
    /// The energy at the bottom of the breakdown: what else the layers take out
    depth: f32,
}

const PLANS: [Plan; 5] = [
    // Classic: kick and bass out for six bars
    Plan {
        breakdown_at: 24,
        out: &[Voice::Kick, Voice::Bass],
        sweep: false,
        depth: 0.2,
    },
    // Short and filtered: only the kick leaves, for two bars – the bass rolls on
    Plan {
        breakdown_at: 28,
        out: &[Voice::Kick],
        sweep: true,
        depth: 0.45,
    },
    // Stripped: just hats, percussion and the synth line
    Plan {
        breakdown_at: 24,
        out: &[Voice::Kick, Voice::Bass, Voice::Clap, Voice::Ride],
        sweep: false,
        depth: 0.12,
    },
    // Filtered: no kick, everything else under a slowly opening filter
    Plan {
        breakdown_at: 24,
        out: &[Voice::Kick],
        sweep: true,
        depth: 0.35,
    },
    // Roller: no breakdown – the groove rolls on into a build-up and drops on itself
    Plan {
        breakdown_at: 30,
        out: &[],
        sweep: false,
        depth: 0.0,
    },
];

/// Which plans suit a cycle of the set: calm cycles roll or filter, the peak strips down
fn plans_for(heat: f32) -> &'static [usize] {
    if heat >= 0.9 {
        &[0, 2, 0]
    } else if heat < 0.5 {
        &[1, 3, 4]
    } else {
        &[0, 1, 3, 4]
    }
}

/// The knobs AUTO FX plays with: eight track volumes, dark and space, the master EQ
const KNOBS: usize = TRACKS + 5;

/// How far each knob may wander from where you put it
const KNOB_RANGE: [f32; KNOBS] = [0.1, 0.12, 0.12, 0.14, 0.1, 0.14, 0.14, 0.12, 0.08, 0.1, 0.05, 0.04, 0.05];

fn knobs(s: &Song) -> [f32; KNOBS] {
    let mut k = [0.0; KNOBS];
    for (v, t) in k.iter_mut().zip(&s.tracks) {
        *v = t.volume;
    }
    k[TRACKS..].copy_from_slice(&[s.macros.dark, s.macros.space, s.eq.low, s.eq.mid, s.eq.high]);
    k
}

fn set_knobs(s: &mut Song, k: [f32; KNOBS]) {
    for (t, v) in s.tracks.iter_mut().zip(k) {
        t.volume = v;
    }
    [s.macros.dark, s.macros.space, s.eq.low, s.eq.mid, s.eq.high] = k[TRACKS..].try_into().unwrap();
}

struct Breakdown {
    start: u64,
    len: u64,
    out: &'static [Voice],
    /// Mute state of every track before the breakdown
    before: [bool; TRACKS],
    sweep: bool,
}

/// A one-bar fill at the end of an 8-bar phrase (auto mode, or by hand: `Cmd::Fill`)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fill {
    /// Clap roll into the next phrase
    Roll,
    /// Kick and bass drop out for the last beat
    Gap,
    /// Falling toms
    Toms,
    /// Open hats on every 8th
    OpenHats,
    /// The clap thrown into the echo
    Throw,
}

pub const FILLS: [Fill; 5] = [Fill::Roll, Fill::Gap, Fill::Toms, Fill::OpenHats, Fill::Throw];

impl Fill {
    pub fn name(self) -> &'static str {
        match self {
            Fill::Roll => "Clap roll",
            Fill::Gap => "Gap – kick and bass out for a beat",
            Fill::Toms => "Falling toms",
            Fill::OpenHats => "Open hats",
            Fill::Throw => "Clap into the echo",
        }
    }
}
/// Fills that close 16 bars, and the lighter ones for the 8 bars in between
const BIG_FILLS: [Fill; 3] = [Fill::Roll, Fill::Toms, Fill::Throw];
const SMALL_FILLS: [Fill; 2] = [Fill::Gap, Fill::OpenHats];

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
    limiter: f32,
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
            limiter: 1.0,
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

    fn start_build(&mut self) {
        // Two bars of build-up from the next bar on, so it always starts and lands on the one
        let next = self.last_fired.map_or(0, |s| s + 1);
        self.build = Some(Build {
            start: next.div_ceil(STEPS as u64) * STEPS as u64,
            len: 2 * STEPS as u64,
        });
        self.send_song(Some(Change::Build));
    }

    /// A build-up the arrangement starts now (auto mode, song mode)
    fn auto_build(&mut self, idx: u64) {
        self.build = Some(Build {
            start: idx,
            len: 2 * STEPS as u64,
        });
        self.send_song(Some(Change::Build));
    }

    /// 0…1 through the build-up; None before it starts
    fn build_progress(&self, idx: f64) -> Option<f32> {
        self.build
            .as_ref()
            .filter(|b| idx >= b.start as f64)
            .map(|b| ((idx - b.start as f64) / b.len as f64).clamp(0.0, 1.0) as f32)
    }

    /// The breath before the drop: the last half beat of the build-up
    fn in_drop_gap(&self, idx: f64) -> bool {
        self.build.as_ref().is_some_and(|b| idx >= (b.start + b.len - DROP_GAP) as f64)
    }

    /// The last bar of a phrase: a fill – a big one every 16 bars, never the same twice in a row
    fn phrase_end(&mut self, bar: u64, big: bool) {
        let last = self.last_fill;
        let fills: &[Fill] = if big { &BIG_FILLS } else { &SMALL_FILLS };
        let n = fills.iter().filter(|&&f| Some(f) != last).count() as u32;
        let f = fills
            .iter()
            .copied()
            .filter(|&f| Some(f) != last)
            .nth(self.auto_rng.below(n) as usize);
        self.fill = f.map(|f| (bar, f));
        self.last_fill = f;
    }

    /// The auto mode with AUTO FX: it plays with the mix (`drift_knobs`)
    fn drifts_mix(&self) -> bool {
        self.song.auto && self.song.auto_fx && self.arrangement.is_none()
    }

    /// A track whose layer the energy falls below is muted (the mute button shows it) – by
    /// hand, in the auto mode and in songs alike; when the energy comes back, the tracks it
    /// muted come back too. Only crossings count, so a track you unmute stays on. True when
    /// a mute changed.
    fn energy_mutes(&mut self) -> bool {
        let mut changed = false;
        let (before, e) = (self.last_layer, self.layer_energy);
        self.last_layer = e;
        let held: &[Voice] = self.breakdown.as_ref().map_or(&[], |b| b.out);
        for (i, t) in self.song.tracks.iter_mut().enumerate() {
            if held.contains(&Voice::ALL[i]) {
                continue;
            }
            let layer = LAYERS[i];
            if before >= layer && e < layer && !t.mute && !t.lock {
                t.mute = true;
                self.energy_out[i] = true;
                self.mute_pending[i] = Some(true);
                changed = true;
            } else if e >= layer && self.energy_out[i] {
                self.energy_out[i] = false;
                if t.mute {
                    t.mute = false;
                    self.mute_pending[i] = Some(false);
                    changed = true;
                }
            }
        }
        changed
    }

    /// AUTO FX plays with the mix: every knob glides towards its goal (back home when the
    /// auto mode or AUTO FX is off). True when a knob moved.
    fn drift_knobs(&mut self) -> bool {
        if !(self.drifts_mix() && self.playing) {
            self.knob_goal = [0.0; KNOBS];
        }
        if self.knob_off == self.knob_goal {
            return false;
        }
        let mut k = knobs(&self.song);
        for i in 0..KNOBS {
            // Across its whole range in about two bars
            let rate = KNOB_RANGE[i] / 32.0;
            let off = &mut self.knob_off[i];
            *off += (self.knob_goal[i] - *off).clamp(-rate, rate);
            if (self.knob_goal[i] - *off).abs() < 1e-5 {
                *off = self.knob_goal[i];
            }
            k[i] = (self.knob_home[i] + *off).clamp(0.0, 1.0);
        }
        set_knobs(&mut self.song, k);
        true
    }

    /// New goals for a couple of knobs: a track a bit louder or softer, the room a bit
    /// bigger, the tone a bit darker – never far from where you put them
    fn new_knob_goals(&mut self, n: usize) {
        for _ in 0..n {
            let i = self.auto_rng.below(KNOBS as u32) as usize;
            let r = KNOB_RANGE[i];
            // Now and then back home
            self.knob_goal[i] = if self.auto_rng.chance(0.25) {
                0.0
            } else {
                (self.auto_rng.f32() * 2.0 - 1.0) * r
            };
        }
    }

    /// The energy may play with the pattern (ghost hats, thinner percussion): only while
    /// SOLAR OS plays (auto or song mode) and the setting allows it
    fn touches(&self) -> bool {
        (self.song.auto || self.arrangement.is_some()) && self.shared.touches.load(Ordering::Relaxed)
    }

    /// Solarum moves the energy: the auto mode with AUTO FX, or a song
    fn steers_energy(&self) -> bool {
        (self.song.auto && self.song.auto_fx) || self.arrangement.is_some()
    }

    fn start_breakdown(&mut self, idx: u64) {
        let plan = self.plan;
        let before = self.song.tracks.map(|t| t.mute);
        for &v in plan.out {
            self.song.track_mut(v).mute = true;
        }
        self.breakdown = Some(Breakdown {
            start: idx,
            len: (self.cycle_len - 2).saturating_sub(self.breakdown_at()).max(1) * STEPS as u64,
            out: plan.out,
            before,
            sweep: plan.sweep,
        });
        self.send_song(Some(Change::Breakdown));
    }

    /// Bring back what the breakdown took out
    fn restore_breakdown(&mut self) {
        if let Some(b) = self.breakdown.take() {
            for &v in b.out {
                self.song.track_mut(v).mute = b.before[v.index()];
            }
        }
    }

    fn end_breakdown(&mut self) {
        self.restore_breakdown();
        self.send_song(None);
    }

    fn drop_now(&mut self) {
        self.build = None;
        self.restore_breakdown();
        self.shared.build.store(0, Ordering::Relaxed);
        self.song.track_mut(Voice::Kick).mute = false;
        self.song.track_mut(Voice::Bass).mute = false;
        self.boom.trigger();
        self.crash = 1.0;
        // Energy turned down before the drop? It comes back with it
        let m = &mut self.song.macros;
        m.energy = m.energy.max(0.85);
        let mut change = Change::Drop;
        if self.song.auto {
            let peaked = self.heat >= 0.9;
            self.new_cycle(self.bar_now);
            // A new chapter only every few drops – the set keeps its identity for a while –
            // and most likely as the set breathes out after its peak
            let n = self.drops_since_chapter;
            if (n >= 2 && (peaked || self.auto_rng.chance(0.35))) || n >= 4 {
                // Tracks you're working on sit the new chapter out
                let locks = self.song.tracks.map(|t| t.lock);
                for (t, &until) in self.song.tracks.iter_mut().zip(&self.hands_off) {
                    t.lock |= until > self.bar_now;
                }
                change = gen::new_chapter(&mut self.song, &mut self.auto_rng, peaked);
                for (t, l) in self.song.tracks.iter_mut().zip(locks) {
                    t.lock = l;
                }
                self.drops_since_chapter = 0;
            } else {
                self.auto_change(true, 0.0);
                self.drops_since_chapter += 1;
            }
            // What the drop brought is the new cycle's theme
            self.home = self.song.tracks;
            if self.song.auto_fx {
                self.song.macros.energy = self.curve_target(0);
            }
        }
        // The drop lands at once; from here the curve glides on
        self.energy_goal = self.song.macros.energy;
        self.layer_energy = self.song.macros.energy;
        self.send_song(Some(change));
    }

    /// Put a pattern in place, keeping the live settings (volume, EQ, energy …); a running
    /// break keeps its tracks out
    fn load_pattern(&mut self, mut s: Song, slot: Option<usize>) {
        s.master = self.song.master;
        s.auto = self.song.auto && self.arrangement.is_none();
        s.auto_bars = self.song.auto_bars;
        s.curve = self.song.curve;
        s.auto_fx = self.song.auto_fx;
        s.eq = self.song.eq;
        s.macros.energy = self.song.macros.energy;
        s.sanitize();
        if let Some(b) = &mut self.breakdown {
            b.before = s.tracks.map(|t| t.mute);
            for &v in b.out {
                s.track_mut(v).mute = true;
            }
        }
        self.song = s;
        self.pattern_now = slot;
    }

    /// Song mode at a bar start: the next section when one is over (or on a jump), the
    /// energy curve, a break's build-up, fills
    fn arrange_bar(&mut self, idx: u64, bar: u64, dropped: bool) {
        let Some(a) = &self.arrangement else { return };
        let form = &FORMS[a.form % FORMS.len()];
        let full = a.patterns.map(|p| p.is_some());
        let over = self.section_start == PENDING
            || self.jump.is_some()
            || bar >= self.section_start + form.sections[self.section.min(form.sections.len() - 1)].bars as u64;
        if over {
            let next = match self.jump.take() {
                Some(j) => j,
                None if self.section_start == PENDING => self.section,
                None => self.section + 1,
            };
            if next >= form.sections.len() {
                self.end_song();
            } else {
                self.enter_section(next, idx, bar, dropped);
            }
            return;
        }
        let sec = form.sections[self.section];
        let (played, n) = (bar - self.section_start, sec.bars as u64);
        let next = form.sections.get(self.section + 1).copied();
        let into_break = next.is_some_and(|nx| nx.part == Part::Break);
        self.energy_goal = sec.part.energy(played, n);
        // The last two bars before a break already ease off, so the break doesn't cut in
        if into_break && played + 2 >= n {
            self.energy_goal = self.energy_goal.min(0.6);
        }
        // Grooves and peaks keep moving: a gentle change every 8 bars
        if matches!(sec.part, Part::Groove | Part::Peak) && played > 0 && played.is_multiple_of(8) {
            if let Some(c) = gen::vary(&mut self.song, &mut self.auto_rng) {
                self.send_song(Some(c));
            }
        }
        if sec.part == Part::Break && played + 2 == n && self.build.is_none() {
            self.auto_build(idx);
        } else if played + 1 == n && sec.part != Part::Break {
            // A fill into a break, a clap roll into another favorite
            let other = next.is_some_and(|nx| pattern_slot(nx.pattern, full) != self.pattern_now);
            if into_break {
                self.fill = Some((bar, *self.auto_rng.pick(&FILLS)));
            } else if other {
                self.fill = Some((bar, Fill::Roll));
            }
        } else if matches!(sec.part, Part::Groove | Part::Peak)
            && (played + 1).is_multiple_of(8)
            && played + 1 < n
            && self.auto_rng.chance(0.7)
        {
            self.phrase_end(bar, (played + 1).is_multiple_of(16));
        }
    }

    fn enter_section(&mut self, i: usize, idx: u64, bar: u64, dropped: bool) {
        let Some(a) = &self.arrangement else { return };
        let sec = FORMS[a.form % FORMS.len()].sections[i];
        let slot = pattern_slot(sec.pattern, a.patterns.map(|p| p.is_some()));
        let pattern = slot.and_then(|k| a.patterns[k]).unwrap_or(a.fallback);
        let first = self.section_start == PENDING;
        // A song records itself – unless the whole session is being recorded anyway
        if first && self.events.is_some() && !self.capturing {
            self.finish_recording();
            self.recording = true;
            self.rec = Vec::with_capacity(REC_CHUNK + 4096);
        }
        // A jump out of a break: kick and bass come back, its build-up is off
        if self.breakdown.is_some() {
            self.restore_breakdown();
        }
        if !dropped && self.build.is_some() {
            self.build = None;
            self.shared.build.store(0, Ordering::Relaxed);
        }
        self.section = i;
        self.section_start = bar;
        let same = !first && slot == self.pattern_now;
        if !same {
            self.load_pattern(pattern, slot);
        }
        match sec.part {
            Part::Break => {
                let before = self.song.tracks.map(|t| t.mute);
                let out: &'static [Voice] = &[Voice::Kick, Voice::Bass];
                for &v in out {
                    self.song.track_mut(v).mute = true;
                }
                self.breakdown = Some(Breakdown {
                    start: idx,
                    len: sec.bars as u64 * STEPS as u64,
                    out,
                    before,
                    sweep: false,
                });
            }
            Part::Peak if !dropped => self.drop_now(),
            _ => {}
        }
        // After the drop with the same favorite: a new chapter of it, so it feels fresh
        if sec.part == Part::Peak && same {
            gen::new_chapter(&mut self.song, &mut self.auto_rng, false);
        }
        self.song.curve = true;
        self.energy_goal = sec.part.energy(0, sec.bars as u64);
        if sec.part == Part::Peak {
            self.song.macros.energy = self.energy_goal;
            self.layer_energy = self.energy_goal;
        }
        self.shared.part.store(i as u32 + 1, Ordering::Relaxed);
        self.shared.part_start.store(bar, Ordering::Relaxed);
        self.send_song(Some(Change::Part(sec.part, i, slot)));
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

    fn end_song(&mut self) {
        self.handle(Cmd::Stop);
        self.send_song(Some(Change::SongEnd));
    }

    /// Auto mode's energy: lands high on the drop and settles, climbs a step every phrase
    /// towards the breakdown, falls to the plan's depth and creeps up through it, then
    /// climbs through the build-up. The heat of the cycle sets how high it all goes.
    fn curve_target(&self, b: u64) -> f32 {
        curve(b, self.cycle_len, self.breakdown_at(), self.plan.depth, self.heat_now(), self.drive)
    }

    /// The set's heat with your drive on top (chill cools it, turnup heats it)
    fn heat_now(&self) -> f32 {
        (self.heat + 0.3 * self.drive).clamp(0.0, 1.0)
    }

    /// Bar of the auto cycle where the breakdown starts – as far before its end as the plan says
    fn breakdown_at(&self) -> u64 {
        self.cycle_len - (32 - self.plan.breakdown_at)
    }

    /// The plan has a breakdown (a roller doesn't)
    fn has_breakdown(&self) -> bool {
        self.breakdown_at() < self.cycle_len - 2
    }

    /// A new auto cycle from `bar` on: the set's heat moves on (up a step, or down after
    /// the peak), and with it another kind of breakdown – never the same twice in a row –
    /// and the cycle's length: calm cycles groove longer, the peak comes quick
    fn new_cycle(&mut self, bar: u64) {
        self.cycle_start = bar;
        // How the groove sounds now is the theme of this cycle
        self.home = self.song.tracks;
        let r = &mut self.auto_rng;
        self.heat = if self.heat >= 0.9 {
            0.3 + 0.2 * r.f32()
        } else {
            (self.heat + 0.15 + 0.2 * r.f32()).min(1.0)
        };
        let hot = (self.heat + 0.3 * self.drive).clamp(0.0, 1.0);
        let last = self.plan;
        let options = plans_for(hot);
        let n = options.iter().filter(|&&i| PLANS[i] != last).count() as u32;
        let i = options.iter().copied().filter(|&i| PLANS[i] != last).nth(r.below(n) as usize);
        self.plan = PLANS[i.unwrap_or(0)];
        let calm = matches!(self.song.vibe.rules(), Vibe::Hypnotic | Vibe::Minimal | Vibe::Dub | Vibe::Melodic);
        let lens: &[u64] = match (calm, hot) {
            (_, h) if h >= 0.9 => &[32],
            (true, h) if h < 0.5 => &[48, 64],
            (true, _) => &[32, 48, 64, 48],
            (false, h) if h < 0.5 => &[32, 48],
            (false, _) => &[32, 32, 48],
        };
        self.cycle_len = *r.pick(lens);
        self.publish_cycle();
    }

    /// Tell the UI where the auto cycle is going (for the log)
    fn publish_cycle(&self) {
        let sh = &self.shared;
        sh.cycle_start.store(self.cycle_start, Ordering::Relaxed);
        sh.cycle_len.store(self.cycle_len as u32, Ordering::Relaxed);
        let brk = if self.has_breakdown() {
            self.breakdown_at() as u32
        } else {
            NO_BREAK
        };
        sh.cycle_break.store(brk, Ordering::Relaxed);
        sh.heat.store((self.heat_now() * 1000.0) as u32, Ordering::Relaxed);
        sh.cycle_depth.store((self.plan.depth * 1000.0) as u32, Ordering::Relaxed);
    }

    /// The auto mode's next change, picked by ear (`crate::ear`): `progress` is how far into
    /// the groove part of the cycle it lands
    fn auto_change(&mut self, big: bool, progress: f32) -> Option<Change> {
        let aim = crate::ear::Aim {
            big,
            mutes: self.song.auto_fx,
            progress,
            heat: self.heat_now(),
            hands_off: self.hands_off.map(|until| until > self.bar_now),
            push: self.drive,
        };
        let (change, why) = crate::ear::choose(&mut self.song, &mut self.auto_rng, &aim, &mut self.recent, &self.home, &self.taste)?;
        self.why = why;
        Some(change)
    }

    /// Tell the UI on which bar the next change lands
    fn publish_next_change(&self, bar: u64) {
        let every = self.song.auto_bars.max(1) as u64;
        let groove = if self.song.auto_fx { self.breakdown_at() } else { self.cycle_len };
        let b = bar.saturating_sub(self.cycle_start);
        let next = (b + 1).next_multiple_of(every);
        let at = if next < groove { self.cycle_start + next } else { u64::MAX };
        self.shared.next_change.store(at, Ordering::Relaxed);
    }

    /// Bar starts: drops, a waiting pattern, song mode, and in auto mode the arrangement
    fn new_bar(&mut self, idx: u64) {
        let bar = idx / STEPS as u64;
        self.bar_now = bar;
        let mut dropped = false;
        if let Some(b) = &self.build {
            if idx >= b.start + b.len {
                self.drop_now();
                dropped = true;
            }
        }
        // A pattern picked during the bar comes in now
        if let Some((s, slot)) = self.queued.take() {
            self.load_pattern(*s, Some(slot));
            self.send_song(Some(Change::Scene(slot)));
        }
        if self.arrangement.is_some() {
            self.arrange_bar(idx, bar, dropped);
            return;
        }
        if dropped || !self.song.auto {
            return;
        }
        // Growing: one empty track comes in per bar, nothing else changes meanwhile
        if let Some(target) = self.grow.take() {
            const ORDER: [Voice; TRACKS] = [
                Voice::Kick,
                Voice::Hat,
                Voice::Clap,
                Voice::Bass,
                Voice::Rim,
                Voice::OpenHat,
                Voice::Acid,
                Voice::Ride,
            ];
            let next = ORDER.into_iter().find(|&v| {
                let t = self.song.track(v);
                !t.lock && t.hits() == 0 && target.track(v).hits() > 0
            });
            if let Some(v) = next {
                let src = *target.track(v);
                let t = self.song.track_mut(v);
                (t.steps, t.len, t.sound, t.mute) = (src.steps, src.len, src.sound, false);
                self.grow = Some(target);
                self.send_song(Some(Change::Unmuted(v)));
                return;
            }
        }
        if bar == 0 {
            return;
        }
        // A cycle of 32–64 bars: groove (a change every few bars, a fill every 8), breakdown,
        // 2 bars build-up, drop. Every drop – also one by hand – starts the next cycle.
        // Everything counts from the cycle's start, so changes land on phrases.
        if bar < self.cycle_start || bar - self.cycle_start >= self.cycle_len {
            self.new_cycle(bar);
        }
        let b = bar - self.cycle_start;
        let at = self.breakdown_at();
        // With AUTO FX on, Solarum arranges: energy, breakdown, build-up, drop. Without it the
        // energy bar is yours (low = break, 90 = drop) and the auto mode only changes patterns,
        // sounds and notes – it never takes a track out.
        let fx = self.song.auto_fx;
        if fx {
            self.energy_goal = self.curve_target(b);
            if bar.is_multiple_of(2) {
                self.new_knob_goals(if b < 8 { 1 } else { 2 });
            }
            if b == at && self.has_breakdown() && self.build.is_none() {
                self.start_breakdown(idx);
            } else if b == self.cycle_len - 2 && self.build.is_none() {
                self.auto_build(idx);
            }
        }
        // Without AUTO FX there's no breakdown: the groove changes all through the cycle
        let groove = if fx { at } else { self.cycle_len };
        let every = self.song.auto_bars.max(1) as u64;
        if b < groove && b.is_multiple_of(every) {
            // The big change opens a phrase; in between come small touches
            let big = b.is_multiple_of(8) || every >= 8;
            if let Some(change) = self.auto_change(big, b as f32 / groove.max(1) as f32) {
                self.send_song(Some(change));
            }
        }
        self.publish_next_change(bar);
        let next = b + 1;
        if fx && next == at && self.has_breakdown() {
            // The clap thrown into the echo leads into the breakdown
            self.fill = Some((bar, Fill::Throw));
            self.last_fill = Some(Fill::Throw);
        } else if next < groove && next.is_multiple_of(8) {
            let big = next.is_multiple_of(16);
            if self.auto_rng.chance(if big { 0.9 } else { 0.7 }) {
                self.phrase_end(bar, big);
            }
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
            if !step.on && touches && voice == Voice::Hat && energy > 0.6 && track.hits() > 0 && h < (energy - 0.6) * 1.5 {
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
        // Darkness closes the master tone filter – all the way down to a muffled 1.5 kHz
        let tone = 18_000.0 * 0.1f32.powf(dark * dark * 1.08);
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
            let v = [
                kick_raw * vol[0],
                self.clap.next(sr, &mut self.rng) * vol[1] * pg,
                self.rim.next(sr, &mut self.rng) * vol[2] * pg,
                self.hat.next(sr, &mut self.rng) * vol[3] * pg,
                self.open_hat.next(sr, &mut self.rng) * vol[4] * pg,
                self.ride.next(sr, &mut self.rng) * vol[5] * pg,
                self.bass.next(sr, self.energy_smooth, dark) * vol[6] * 2.0 * (1.0 - duck * 0.7) * pg,
                self.acid.next(sr, acid) * vol[7] * (1.0 - duck * 0.3) * pg,
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
            // Glue: saturation with the DRIVE knob
            let drive = 1.34 + (eq.drive - 0.3) * 1.6;
            l = sat(l * drive * 0.7);
            r = sat(r * drive * 0.7);
            // Brick-wall-ish limiter
            let pk = l.abs().max(r.abs());
            let want = if pk > 0.95 { 0.95 / pk } else { 1.0 };
            self.limiter = if want < self.limiter {
                want
            } else {
                self.limiter + (want - self.limiter) * 0.0002
            };
            let g = self.limiter * self.song.master;
            l *= g;
            r *= g;
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

/// The auto mode's energy curve over a cycle of `len` bars with its breakdown at bar `at`
/// (`len - 2` or later: a roller without one) going down to `depth`, at the set's `heat` –
/// the engine steers by it, SOLAR OS draws it. `drive` (−1…1, the energy slider) lifts or
/// lowers all of it.
pub fn curve(b: u64, len: u64, at: u64, depth: f32, h: f32, drive: f32) -> f32 {
    (plan_curve(b, len, at, depth, h) + 0.25 * drive).clamp(0.05, 1.0)
}

fn plan_curve(b: u64, len: u64, at: u64, depth: f32, h: f32) -> f32 {
    let build = len.saturating_sub(2);
    let has_breakdown = at < build;
    let drop = 0.9 + 0.08 * h;
    let settle = 0.6 + 0.15 * h;
    let top = settle + 0.08 + 0.1 * h;
    if b < 8 {
        drop - (drop - settle) * b as f32 / 8.0
    } else if b < at {
        // Terraces: a new level on every phrase, the top in the last one
        let phrases = (at - 8).div_ceil(8).max(1);
        let k = ((b - 8) / 8 + 1).min(phrases);
        settle + (top - settle) * k as f32 / phrases as f32
    } else if b < build {
        let p = (b - at) as f32 / (build - at) as f32;
        depth + 0.15 * p * p
    } else {
        let from = if has_breakdown { (depth + 0.3).max(0.5) } else { top - 0.15 };
        from + (b - build) as f32 * 0.2
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
mod tests {
    use super::*;
    use crate::song::Vibe;
    use std::collections::HashSet;

    fn stats(buf: &[f32]) -> (f32, f32) {
        let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let rms = (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt();
        (peak, rms)
    }

    /// Counts the allocations of the thread that asks (to prove the audio callback makes none)
    struct Counting;
    thread_local! {
        static ALLOCS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }
    unsafe impl std::alloc::GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
            let _ = ALLOCS.try_with(|c| c.set(c.get().map(|n| n + 1)));
            unsafe { std::alloc::System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(p, l) }
        }
    }
    #[global_allocator]
    static COUNTING: Counting = Counting;

    #[test]
    fn the_auto_mode_renders_without_allocating() {
        // Auto mode with AUTO FX over many cycles: patterns, sounds, breakdowns, drops – all
        // on the audio thread, so not one allocation (no events: those box the song)
        for vibe in [Vibe::Hard, Vibe::Dub, Vibe::Minimal] {
            let mut song = gen::generate(vibe, &Default::default(), &mut Rng::new(3));
            song.auto = true;
            song.auto_bars = 2;
            song.bpm = 180.0;
            let mut e = Engine::new(8_000.0, song, Shared::new(), None, 5);
            e.handle(Cmd::Play);
            let mut buf = vec![0.0; 512];
            ALLOCS.with(|c| c.set(Some(0)));
            for _ in 0..4000 {
                e.render(&mut buf);
            }
            let n = ALLOCS.with(|c| c.replace(None)).unwrap();
            assert_eq!(n, 0, "{vibe:?}: {n} allocations on the audio thread");
        }
    }

    #[test]
    fn a_nan_sound_plays_silence_and_asks_for_a_new_engine() {
        use crate::sampler::{SampleBuf, CUSTOM};
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
        song.track_mut(Voice::Clap).sound = CUSTOM;
        let mut slots: crate::sampler::Slots = Default::default();
        slots[0] = Some(Arc::new(SampleBuf {
            data: vec![f32::NAN; 4000],
            pitch: 0.0,
        }));
        let shared = Shared::new();
        let mut e = Engine::new(44_100.0, song, shared.clone(), None, 5);
        e.handle(Cmd::Samples(Box::new(slots)));
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * e.step_len() as usize * 16];
        e.render(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite()));
        assert!(shared.broken.load(Ordering::Relaxed));
    }

    #[test]
    fn your_own_sounds_play_on_their_track_without_allocating() {
        use crate::sampler::{SampleBuf, CUSTOM};
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
        for t in song.tracks.iter_mut() {
            t.mute = true;
        }
        let clap = song.track_mut(Voice::Clap);
        clap.mute = false;
        clap.sound = CUSTOM + 3;
        let buf = Arc::new(SampleBuf {
            data: (0..4000).map(|i| (i as f32 * 0.07).sin() * 0.8).collect(),
            pitch: 0.0,
        });
        let mut slots: crate::sampler::Slots = Default::default();
        slots[3] = Some(buf.clone());
        let mut e = Engine::new(44_100.0, song, Shared::new(), None, 5);
        e.handle(Cmd::Samples(Box::new(slots)));
        e.handle(Cmd::Play);
        let mut out = vec![0.0; 1024];
        let mut peak = 0.0f32;
        ALLOCS.with(|c| c.set(Some(0)));
        for _ in 0..200 {
            e.render(&mut out);
            peak = out.iter().fold(peak, |m, x| m.max(x.abs()));
        }
        let n = ALLOCS.with(|c| c.replace(None)).unwrap();
        assert_eq!(n, 0, "{n} allocations on the audio thread");
        assert!(peak > 0.1 && peak <= 1.0, "{peak}");
        // The audition plays while stopped, and says where it is
        e.handle(Cmd::Stop);
        e.handle(Cmd::Audition(Some(buf), false));
        e.render(&mut out);
        assert!(e.shared.audition.load(Ordering::Relaxed) < 4000);
    }

    #[test]
    fn renders_loud_clean_audio_for_every_vibe() {
        for (i, vibe) in Vibe::ALL.into_iter().enumerate() {
            let song = gen::generate(vibe, &Default::default(), &mut Rng::new(i as u64));
            let buf = render_offline(&song, 4, 44_100, 1);
            assert!(buf.iter().all(|x| x.is_finite()));
            let (peak, rms) = stats(&buf);
            assert!(peak <= 1.0, "{vibe:?} clips: {peak}");
            assert!(rms > 0.05, "{vibe:?} too quiet: {rms}");
        }
    }

    #[test]
    fn steps_fire_on_time() {
        let mut song = Song::empty();
        song.bpm = 120.0;
        let shared = Shared::new();
        let mut e = Engine::new(48_000.0, song, shared.clone(), None, 1);
        e.handle(Cmd::Play);
        // 120 bpm → one 16th = 0.125 s = 6000 samples
        let mut buf = vec![0.0; 47_000 * 2];
        e.render(&mut buf);
        assert_eq!(shared.step.load(Ordering::Relaxed), 7);
        e.render(&mut buf[..2 * 2000]);
        assert_eq!(shared.step.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn by_hand_the_pattern_plays_exactly_as_written() {
        // One hat and one rim, energy high and low: no ghost hats, nothing thinned out
        for energy in [0.95, 0.35] {
            let mut song = Song::empty();
            song.bpm = 200.0;
            song.macros.energy = energy;
            song.track_mut(Voice::Hat).steps[2] = crate::song::Step::hit();
            song.track_mut(Voice::Rim).steps[6] = crate::song::Step::hit();
            let shared = Shared::new();
            let mut e = Engine::new(22_050.0, song, shared.clone(), None, 3);
            e.handle(Cmd::Play);
            let bars = 8;
            let mut buf = vec![0.0; 2 * (e.step_len() * (STEPS * bars) as f64) as usize];
            e.render(&mut buf);
            let hits = |v: Voice| shared.hits[v.index()].load(Ordering::Relaxed) as usize;
            assert_eq!(hits(Voice::Hat), bars, "energy {energy}");
            assert_eq!(hits(Voice::Rim), bars, "energy {energy}");
        }
    }

    #[test]
    fn a_fill_by_hand_lands_in_the_second_half_of_a_bar() {
        let mut song = Song::empty();
        song.bpm = 200.0;
        let shared = Shared::new();
        let mut e = Engine::new(22_050.0, song, shared.clone(), None, 3);
        e.handle(Cmd::Fill(Fill::Roll));
        assert_eq!(e.fill, None, "not while stopped");
        e.handle(Cmd::Play);
        let step = e.step_len() as usize;
        let mut buf = vec![0.0; 2 * step * 3];
        e.render(&mut buf);
        e.handle(Cmd::Fill(Fill::Roll));
        assert_eq!(e.fill, Some((0, Fill::Roll)), "early in the bar: this one");
        let mut buf = vec![0.0; 2 * step * 7];
        e.render(&mut buf);
        e.handle(Cmd::Fill(Fill::Toms));
        assert_eq!(e.fill, Some((1, Fill::Toms)), "late in the bar: the next one");
        // The roll played: claps although the clap track is empty
        assert!(shared.hits[Voice::Clap.index()].load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn auto_mode_arranges_breakdown_build_and_drop() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        song.auto = true;
        song.auto_bars = 4;
        song.bpm = 200.0; // fast, to keep the test short
        let mut e = Engine::new(22_050.0, song, Shared::new(), Some(tx), 9);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 33];
        e.render(&mut buf);
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert!(changes.contains(&Change::Breakdown), "{changes:?}");
        assert!(changes.contains(&Change::Build));
        assert!(matches!(changes.last(), Some(Change::Drop | Change::NewChapter)), "{changes:?}");
        // Changes every 4 bars before the breakdown
        assert!(changes.len() >= 5 + 3, "{changes:?}");
        assert!(!e.song.track(Voice::Kick).mute);
    }

    #[test]
    fn breakdown_ends_when_auto_mode_stops() {
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        song.auto = true;
        song.bpm = 200.0;
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 9);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 25 + 200];
        e.render(&mut buf); // into bar 24: breakdown
        assert!(e.song.track(Voice::Kick).mute);
        let mut off = e.song;
        off.auto = false;
        e.handle(Cmd::Song(Box::new(off)));
        assert!(!e.song.track(Voice::Kick).mute && !e.song.track(Voice::Bass).mute);

        // Same when the music stops
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 9);
        e.handle(Cmd::Play);
        e.render(&mut buf);
        e.handle(Cmd::Stop);
        assert!(!e.song.track(Voice::Kick).mute);
    }

    #[test]
    fn manual_drop_builds_two_bars_from_the_next_bar() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Shared::new();
        let mut e = Engine::new(22_050.0, Song::default(), shared.clone(), Some(tx), 2);
        e.handle(Cmd::Play);
        let step = e.step_len() as usize;
        let mut buf = vec![0.0; 2 * step * 5];
        e.render(&mut buf); // into bar 0
        e.handle(Cmd::Drop);
        assert!(e.build.is_some());
        // Waiting for bar 1: the beat plays on, the button already shows it
        let kicks = |s: &Shared| s.hits[Voice::Kick.index()].load(Ordering::Relaxed);
        let before = kicks(&shared);
        let mut buf = vec![0.0; 2 * step * 10];
        e.render(&mut buf);
        assert!(kicks(&shared) > before, "kick until the bar is over");
        assert!(shared.build_progress().is_some());
        assert_eq!(e.build_progress(e.phase), None);
        // Build-up through bars 1 and 2, the drop on bar 3
        let mut buf = vec![0.0; 2 * step * 32];
        e.render(&mut buf);
        assert!(e.build.is_some(), "still building on step 47");
        let mut buf = vec![0.0; 2 * step * 2];
        e.render(&mut buf);
        assert!(e.build.is_none(), "drop on step 48");
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert_eq!(changes, vec![Change::Build, Change::Drop]);
    }

    #[test]
    fn build_up_turns_the_groove_into_a_roll_and_breathes_before_the_drop() {
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(4));
        song.macros.energy = 0.9;
        for v in Voice::ALL {
            song.track_mut(v).mute = false;
        }
        let shared = Shared::new();
        let mut e = Engine::new(22_050.0, song, shared.clone(), None, 1);
        e.handle(Cmd::Play);
        let step = e.step_len() as usize;
        let hits = |v: Voice| shared.hits[v.index()].load(Ordering::Relaxed);
        // Windows end in the middle of a step, away from the step boundaries
        let mut buf = vec![0.0; 2 * (step * 15 + step / 2)];
        e.render(&mut buf);
        e.handle(Cmd::Drop);
        let count = |e: &mut Engine, steps: usize, v: Voice| {
            let before = hits(v);
            let mut buf = vec![0.0; 2 * step * steps];
            e.render(&mut buf);
            assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
            hits(v) - before
        };
        // Kick roll: quarters in the first bar, 8ths, then 16ths – up to the gap
        assert_eq!(count(&mut e, 16, Voice::Kick), 4);
        assert_eq!(count(&mut e, 8, Voice::Kick), 4);
        assert_eq!(count(&mut e, 6, Voice::Kick), 6);
        // The gap: nothing new starts in the last half beat
        let before = Voice::ALL.map(hits);
        let mut buf = vec![0.0; 2 * step * 2];
        e.render(&mut buf);
        assert_eq!(Voice::ALL.map(hits), before, "silence before the drop");
        assert!(count(&mut e, 4, Voice::Kick) > 0 && e.build.is_none(), "the drop");
    }

    #[test]
    fn build_up_loops_the_riff_and_lifts_it() {
        let start = 32;
        assert_eq!(build_play(Voice::Acid, 0.2, 36, start), BuildPlay::Pattern);
        assert_eq!(build_play(Voice::Acid, 0.6, 54, start), BuildPlay::Loop { at: 50, up: 0 });
        assert_eq!(build_play(Voice::Bass, 0.8, 59, start), BuildPlay::Loop { at: 49, up: 12 });
        assert_eq!(build_play(Voice::Rim, 0.8, 59, start), BuildPlay::Loop { at: 49, up: 0 });
        assert_eq!(build_play(Voice::OpenHat, 0.6, 50, start), BuildPlay::Rest);
        assert!(matches!(build_play(Voice::Hat, 0.6, 51, start), BuildPlay::Hit(_)));
    }

    #[test]
    fn stopped_the_energy_still_shows_what_it_mutes() {
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        song.macros.energy = 1.0;
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        let mut s = e.song;
        s.macros.energy = 0.25;
        e.handle(Cmd::Song(Box::new(s)));
        assert!(
            e.song.track(Voice::Kick).mute && e.song.track(Voice::Bass).mute,
            "break: kick and bass out"
        );
        assert!(!e.song.track(Voice::Hat).mute);
        let mut s = e.song;
        s.macros.energy = 1.0;
        e.handle(Cmd::Song(Box::new(s)));
        assert!(!e.song.track(Voice::Kick).mute, "and back");
    }

    #[test]
    fn energy_mutes_survive_songs_that_crossed_them() {
        // The UI keeps sending songs while the fader moves; some still carry the mutes from
        // before the engine changed them – they mustn't leave tracks muted at 100
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        song.macros.energy = 1.0;
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        let ui = e.song; // what the UI has: nothing muted
        for v in [0.6, 0.3, 0.1] {
            let mut s = ui;
            s.macros.energy = v;
            e.handle(Cmd::Song(Box::new(s)));
        }
        assert!(e.song.track(Voice::Kick).mute, "down: the kick is out");
        // Back up, still with the UI's old (unmuted) tracks, then once with the engine's
        let mut s = ui;
        s.macros.energy = 1.0;
        e.handle(Cmd::Song(Box::new(s)));
        let mut s = e.song;
        s.macros.energy = 1.0;
        e.handle(Cmd::Song(Box::new(s)));
        assert!(
            e.song.tracks.iter().all(|t| !t.mute) || e.song.tracks.iter().zip(&song.tracks).all(|(a, b)| a.mute == b.mute),
            "at 100 everything is back: {:?}",
            e.song.tracks.map(|t| t.mute)
        );
    }

    #[test]
    fn a_new_groove_drops_the_energys_old_mutes() {
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        song.macros.energy = 1.0;
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        let mut s = e.song;
        s.macros.energy = 0.1;
        e.handle(Cmd::Song(Box::new(s)));
        assert!(e.song.track(Voice::Kick).mute);
        // A new project: other steps, nothing muted, energy full – nothing may stay muted
        let mut fresh = Song::empty();
        fresh.macros.energy = 1.0;
        fresh.track_mut(Voice::Kick).steps[0].on = true;
        e.handle(Cmd::Song(Box::new(fresh)));
        assert!(e.song.tracks.iter().all(|t| !t.mute), "{:?}", e.song.tracks.map(|t| t.mute));
    }

    #[test]
    fn energy_by_hand_never_drops_by_itself() {
        // No drop zone any more: the energy all the way up is just everything playing
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * e.step_len() as usize];
        e.render(&mut buf);
        for v in [0.6, 0.95, 1.0] {
            let mut s = e.song;
            s.macros.energy = v;
            e.handle(Cmd::Song(Box::new(s)));
            e.render(&mut buf);
        }
        assert!(e.build.is_none());
        e.handle(Cmd::Drop);
        assert!(e.build.is_some(), "DROP drops");
    }

    #[test]
    fn recording_writes_into_the_pattern() {
        let mut song = Song::empty();
        song.bpm = 120.0;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut e = Engine::new(48_000.0, song, Shared::new(), Some(tx), 1);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * 6000 * 3 + 2 * 1000];
        e.render(&mut buf); // a little into step 3
        e.handle(Cmd::Hit {
            voice: Voice::Clap,
            vel: 1.0,
            record: true,
        });
        assert!(e.song.track(Voice::Clap).steps[3].on);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn a_recorded_hit_is_heard() {
        let mut song = Song::empty();
        song.macros.energy = 0.6;
        let shared = Shared::new();
        let mut e = Engine::new(22_050.0, song, shared.clone(), None, 1);
        e.handle(Cmd::Play);
        let step = e.step_len() as usize;
        let mut buf = vec![0.0; 2 * step * 3 + 100];
        e.render(&mut buf);
        // The ride only plays from energy 70 – recording it lets it in
        e.handle(Cmd::Hit {
            voice: Voice::Ride,
            vel: 0.9,
            record: true,
        });
        assert!(e.song.macros.energy >= LAYERS[Voice::Ride.index()]);
        let before = shared.hits[Voice::Ride.index()].load(Ordering::Relaxed);
        let mut buf = vec![0.0; 2 * step * 16];
        e.render(&mut buf);
        assert_eq!(
            shared.hits[Voice::Ride.index()].load(Ordering::Relaxed) - before,
            1,
            "the loop plays it"
        );
    }

    #[test]
    fn auto_mode_varies_and_always_brings_the_beat_back() {
        // Several cycles: different lengths, breakdowns, build-ups, fills, chapters – and
        // outside a breakdown the kick always plays
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(8));
        song.auto = true;
        song.auto_bars = 2;
        song.bpm = 180.0;
        let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        let (mut kinds, mut lens) = (HashSet::new(), HashSet::new());
        for b in 0..(32 * 10) {
            e.render(&mut buf);
            assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
            if e.breakdown.is_none() && e.build.is_none() && b > 0 {
                assert!(!e.song.track(Voice::Kick).mute, "kick plays (bar {b})");
            }
            kinds.insert(e.plan.breakdown_at * 100 + e.plan.out.len() as u64 * 10 + e.plan.sweep as u64);
            lens.insert(e.cycle_len);
        }
        assert!(kinds.len() >= 2, "different breakdowns");
        assert!(lens.len() >= 2, "different cycle lengths: {lens:?}");
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert!(changes.iter().any(|c| matches!(c, Change::NewSound(..))), "{changes:?}");
        assert!(changes.iter().filter(|c| matches!(c, Change::Drop | Change::NewChapter)).count() >= 5);
    }

    #[test]
    fn a_track_you_edit_while_the_auto_mode_plays_stays_yours_for_two_phrases() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Acid, &Default::default(), &mut Rng::new(8));
        song.auto = true;
        song.auto_bars = 1;
        song.bpm = 180.0;
        let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        for _ in 0..4 {
            e.render(&mut buf);
        }
        let mut mine = e.song;
        let s = &mut mine.track_mut(Voice::Bass).steps[3];
        s.on = !s.on;
        e.handle(Cmd::Song(Box::new(mine)));
        let _ = rx.try_iter().count();
        for _ in 0..15 {
            e.render(&mut buf);
            assert_eq!(e.song.track(Voice::Bass).steps, mine.track(Voice::Bass).steps);
        }
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert!(changes.iter().filter(|c| c.voice().is_some()).count() >= 8, "{changes:?}");
        assert!(changes.iter().all(|c| c.voice() != Some(Voice::Bass)), "{changes:?}");
        // What you made is the theme now: the auto mode comes back to it, not to the old line
        assert_eq!(e.home[Voice::Bass.index()].steps, mine.track(Voice::Bass).steps);
    }

    #[test]
    fn turnup_heats_the_set_and_chill_cools_it() {
        let sh = Shared::new();
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
        let mut e = Engine::new(11_025.0, song, sh.clone(), None, 1);
        e.handle(Cmd::Play);
        let (heat, mid) = (sh.heat.load(Ordering::Relaxed), e.curve_target(12));
        e.handle(Cmd::Drive(1.0));
        assert!(sh.heat.load(Ordering::Relaxed) > heat && e.curve_target(12) > mid);
        e.handle(Cmd::Drive(-1.0));
        assert!(sh.heat.load(Ordering::Relaxed) < heat && e.curve_target(12) < mid);
    }

    #[test]
    fn the_director_explains_some_of_its_moves() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
        song.auto = true;
        song.auto_bars = 2;
        song.bpm = 180.0;
        let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 6);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        for _ in 0..(32 * 6) {
            e.render(&mut buf);
        }
        let whys: Vec<_> = rx
            .try_iter()
            .filter_map(|ev| if let Event::Song(_, _, _, w) = ev { w } else { None })
            .collect();
        assert!(whys.len() >= 3, "{whys:?}");
        assert!(whys.contains(&crate::ear::Why::Home), "the theme comes back now and then: {whys:?}");
    }

    #[test]
    fn without_auto_fx_the_auto_mode_never_drops_by_itself() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(8));
        song.auto = true;
        song.auto_fx = false;
        song.bpm = 180.0;
        let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 70];
        e.render(&mut buf);
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert!(
            !changes
                .iter()
                .any(|c| matches!(c, Change::Breakdown | Change::Build | Change::Drop)),
            "{changes:?}"
        );
        assert!(changes
            .iter()
            .any(|c| matches!(c, Change::NewPattern(_) | Change::NewSound(..) | Change::Shifted(_))));
        assert!(
            !changes.iter().any(|c| matches!(c, Change::Muted(_) | Change::Unmuted(_))),
            "the bar decides what plays"
        );
        assert_eq!(e.song.macros.energy, song.macros.energy, "the energy is yours");
    }

    #[test]
    fn at_85_everything_plays_and_the_engine_counts_edits() {
        assert!(LAYERS.iter().all(|&l| l < 0.85), "{LAYERS:?}");
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
        song.macros.energy = 0.85;
        for t in song.tracks.iter_mut() {
            t.mute = false;
        }
        let shared = Shared::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut e = Engine::new(22_050.0, song, shared.clone(), Some(tx), 1);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * (e.step_len() * STEPS as f64 * 2.0) as usize];
        e.render(&mut buf);
        for (i, t) in song.tracks.iter().enumerate() {
            if t.hits() > 0 {
                assert!(shared.hits[i].load(Ordering::Relaxed) > 0, "{:?} plays at 85", Voice::ALL[i]);
            }
        }
        // Every song the engine sends says how many edits it had taken in
        e.handle(Cmd::Song(Box::new(song)));
        e.handle(Cmd::Drop);
        let acks: Vec<u64> = rx
            .try_iter()
            .filter_map(|ev| if let Event::Song(_, _, n, _) = ev { Some(n) } else { None })
            .collect();
        assert_eq!(acks.last(), Some(&1));
    }

    #[test]
    fn a_drop_by_hand_pushes_the_next_auto_drop_back() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(8));
        song.auto = true;
        song.bpm = 180.0;
        let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 20 + 200];
        e.render(&mut buf); // into bar 20, just before the auto breakdown
        e.handle(Cmd::Drop); // build-up in bars 21–22, drop on 23
        let mut buf = vec![0.0; 2 * bar * 3];
        e.render(&mut buf);
        assert_eq!(e.cycle_start, 23, "the drop starts a new cycle");
        let _ = rx.try_iter().count();
        let mut buf = vec![0.0; 2 * bar * 20];
        e.render(&mut buf);
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert!(!changes.contains(&Change::Breakdown), "no breakdown right after: {changes:?}");
        assert!(!changes.contains(&Change::Drop));
    }

    #[test]
    fn the_set_climbs_to_a_peak_and_breathes_out() {
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
        let mut e = Engine::new(11_025.0, song, Shared::new(), None, 7);
        let (mut peaks, mut releases) = (0, 0);
        for c in 0..60 {
            let (heat, plan) = (e.heat, e.plan);
            e.new_cycle(c * 32);
            assert_ne!(e.plan, plan, "never the same breakdown twice in a row");
            if heat >= 0.9 {
                assert!(e.heat < 0.5, "after the peak the set breathes out: {}", e.heat);
                releases += 1;
            } else {
                assert!(e.heat > heat, "it climbs: {heat} → {}", e.heat);
            }
            if e.heat >= 0.9 {
                peaks += 1;
                assert_eq!(e.cycle_len, 32, "the peak comes quick");
                assert!(e.has_breakdown() && e.plan.depth <= 0.2, "the peak strips down: {:?}", e.plan);
            }
        }
        assert!(peaks >= 10 && releases >= 10, "{peaks} peaks, {releases} releases");
    }

    #[test]
    fn the_energy_climbs_in_phrases_and_the_heat_sets_how_high() {
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
        let mut e = Engine::new(11_025.0, song, Shared::new(), None, 7);
        e.cycle_len = 48;
        e.plan = PLANS[0];
        e.heat = 0.5;
        // One level per phrase, a step up from phrase to phrase
        for b in 8..e.breakdown_at() {
            let phrase = (b - 8) / 8 * 8 + 8;
            assert_eq!(e.curve_target(b), e.curve_target(phrase), "bar {b}");
        }
        assert!(e.curve_target(16) > e.curve_target(8));
        // In the breakdown it creeps up, and the build-up climbs from there
        let at = e.breakdown_at();
        assert!(e.curve_target(at) < BREAK_ZONE && e.curve_target(at + 5) > e.curve_target(at));
        assert!(e.curve_target(e.cycle_len - 1) > e.curve_target(e.cycle_len - 2));
        let warm = e.curve_target(at - 1);
        e.heat = 1.0;
        assert!(e.curve_target(at - 1) > warm + 0.05, "a hot cycle runs higher");
        assert!(e.curve_target(at - 1) < 1.0 && e.curve_target(0) < 1.0);
    }

    #[test]
    fn a_roller_cycle_builds_and_drops_without_a_breakdown() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        song.auto = true;
        song.bpm = 200.0;
        let mut e = Engine::new(22_050.0, song, Shared::new(), Some(tx), 9);
        e.handle(Cmd::Play);
        e.plan = PLANS[4];
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 33];
        e.render(&mut buf);
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert!(!changes.contains(&Change::Breakdown), "{changes:?}");
        assert!(changes.contains(&Change::Build));
        assert!(
            changes.iter().any(|c| matches!(c, Change::Drop | Change::NewChapter)),
            "{changes:?}"
        );
    }

    #[test]
    fn live_effects_stay_clean() {
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * 22_050];
        e.render(&mut buf);
        for fx in Fx::ALL {
            e.handle(Cmd::Fx(fx, true));
            e.render(&mut buf);
            assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0), "{fx:?}");
            if fx == Fx::Tape {
                // Fully stopped after a second
                let tail = &buf[buf.len() - 2000..];
                assert!(tail.iter().all(|x| x.abs() < 1e-3), "tape stop ends in silence");
            }
            e.handle(Cmd::Fx(fx, false));
            e.render(&mut buf);
        }
        let (peak, rms) = stats(&buf);
        assert!(peak > 0.1 && rms > 0.02, "back to normal after the effects");
    }

    fn patterns(bpm: f32) -> [Song; 3] {
        [Vibe::Dub, Vibe::Peak, Vibe::Hard].map(|v| {
            let mut s = gen::generate(v, &Default::default(), &mut Rng::new(v as u64 + 1));
            s.bpm = bpm;
            s
        })
    }

    #[test]
    fn a_song_plays_its_parts_with_the_patterns() {
        let (tx, rx) = std::sync::mpsc::channel();
        let [a, b, c] = patterns(180.0);
        let mut e = Engine::new(11_025.0, a, Shared::new(), Some(tx), 1);
        e.handle(Cmd::Arrange(Some(Box::new(Arrangement {
            patterns: [Some(a), Some(b), Some(c), None],
            form: 0,
            fallback: a,
        }))));
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        let mut kick_in_break = None;
        for _ in 0..(FORMS[0].bars() + 2) {
            e.render(&mut buf);
            assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
            if e.breakdown.is_some() {
                kick_in_break = Some(e.song.track(Voice::Kick).mute);
            }
        }
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        let parts: Vec<Part> = changes
            .iter()
            .filter_map(|c| if let Change::Part(p, ..) = c { Some(*p) } else { None })
            .collect();
        assert_eq!(
            parts,
            vec![
                Part::Intro,
                Part::Groove,
                Part::Break,
                Part::Peak,
                Part::Break,
                Part::Peak,
                Part::Outro
            ]
        );
        // Favorite 1 until the first drop, favorite 2 after it, favorite 1 for the last drop
        let slots: Vec<Option<usize>> = changes
            .iter()
            .filter_map(|c| if let Change::Part(_, _, slot) = c { Some(*slot) } else { None })
            .collect();
        assert_eq!(slots, vec![Some(0), Some(0), Some(0), Some(1), Some(1), Some(0), Some(0)]);
        assert_eq!(changes.iter().filter(|c| **c == Change::Drop).count(), 2, "two drops");
        // The grooves keep moving
        assert!(changes.iter().any(|c| matches!(
            c,
            Change::NewPattern(_) | Change::NewSound(..) | Change::NewNotes(_) | Change::Shifted(_) | Change::Length(..)
        )));
        assert_eq!(kick_in_break, Some(true), "no kick in the break");
        let build = changes.iter().position(|c| *c == Change::Build).expect("a build-up");
        let drop = changes.iter().position(|c| *c == Change::Drop).expect("a drop");
        assert!(build < drop);
        assert_eq!(changes.last(), Some(&Change::SongEnd));
        assert!(!e.playing && e.arrangement.is_none(), "the song ends");
    }

    #[test]
    fn a_song_is_recorded_with_its_tail() {
        let (tx, rx) = std::sync::mpsc::channel();
        let [a, b, _] = patterns(180.0);
        let sr = 11_025.0;
        let mut e = Engine::new(sr, a, Shared::new(), Some(tx), 1);
        e.handle(Cmd::Arrange(Some(Box::new(Arrangement {
            patterns: [Some(a), Some(b), None, None],
            form: 0,
            fallback: a,
        }))));
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        for _ in 0..(FORMS[0].bars() + 3) {
            e.render(&mut buf);
        }
        let (mut samples, mut done) = (0, false);
        for ev in rx.try_iter() {
            match ev {
                Event::Audio(piece) => {
                    assert!(!done, "nothing after the end");
                    samples += piece.len();
                }
                Event::Recorded => done = true,
                Event::Song(..) => {}
            }
        }
        assert!(done, "the recording is complete");
        let frames = samples / 2;
        let song = FORMS[0].bars() as usize * bar;
        let tail = (sr * 2.0) as usize;
        assert!(
            frames >= song + tail - bar && frames <= song + tail + bar,
            "{frames} vs {}",
            song + tail
        );
    }

    #[test]
    fn a_session_is_recorded_across_stops_until_switched_off() {
        let (tx, rx) = std::sync::mpsc::channel();
        let sr = 11_025.0;
        let mut e = Engine::new(sr, Song::default(), Shared::new(), Some(tx), 1);
        e.handle(Cmd::Capture(true));
        e.handle(Cmd::Play);
        let second = vec![0.0; 2 * sr as usize];
        let mut buf = second.clone();
        e.render(&mut buf);
        e.handle(Cmd::Stop); // a pause in the jam – the recording waits, it doesn't end
        e.render(&mut buf);
        e.handle(Cmd::Play);
        e.render(&mut buf);
        e.handle(Cmd::Capture(false));
        for _ in 0..3 {
            e.render(&mut buf); // the tail, then it's done
        }
        let (mut samples, mut done) = (0, 0);
        for ev in rx.try_iter() {
            match ev {
                Event::Audio(p) => samples += p.len(),
                Event::Recorded => done += 1,
                Event::Song(..) => {}
            }
        }
        assert_eq!(done, 1);
        let secs = samples as f32 / 2.0 / sr;
        assert!((3.9..=4.1).contains(&secs), "2 s played (the pause left out) + 2 s tail: {secs}");
    }

    #[test]
    fn a_jump_goes_to_a_part_on_the_next_bar() {
        let (tx, rx) = std::sync::mpsc::channel();
        let [a, b, _] = patterns(180.0);
        let mut e = Engine::new(11_025.0, a, Shared::new(), Some(tx), 1);
        e.handle(Cmd::Arrange(Some(Box::new(Arrangement {
            patterns: [Some(a), Some(b), None, None],
            form: 1,
            fallback: a,
        }))));
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 2 + 100];
        e.render(&mut buf);
        e.handle(Cmd::Jump(3)); // the first peak
        let mut buf = vec![0.0; 2 * bar];
        e.render(&mut buf);
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert!(changes.contains(&Change::Drop), "a peak starts with a drop: {changes:?}");
        // The first peak plays favorite 2
        assert!(changes.contains(&Change::Part(Part::Peak, 3, Some(1))), "{changes:?}");
        assert_eq!(e.section, 3);
        assert!(e.song.macros.energy > 0.9);
    }

    #[test]
    fn a_pattern_picked_while_playing_comes_in_on_the_next_bar() {
        let (tx, rx) = std::sync::mpsc::channel();
        let [a, b, _] = patterns(130.0);
        let mut e = Engine::new(22_050.0, a, Shared::new(), Some(tx), 1);
        e.handle(Cmd::Play);
        let step = e.step_len() as usize;
        let mut buf = vec![0.0; 2 * step * 5];
        e.render(&mut buf);
        e.handle(Cmd::Queue(Box::new(b), 1));
        assert_eq!(e.song.tracks.map(|t| t.steps), a.tracks.map(|t| t.steps), "not yet");
        assert_eq!(e.fill, None, "by hand no clap roll leads into it");
        let mut buf = vec![0.0; 2 * step * 12];
        e.render(&mut buf);
        assert_eq!(e.song.tracks.map(|t| t.steps), b.tracks.map(|t| t.steps));
        let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
        assert_eq!(changes, vec![Change::Scene(1)]);
        // With SOLAR OS on, a clap roll leads into the next one
        e.song.auto = true;
        e.handle(Cmd::Queue(Box::new(a), 0));
        assert!(matches!(e.fill, Some((_, Fill::Roll))), "a clap roll leads into it");
    }

    #[test]
    fn energy_strips_the_groove_and_the_drop_brings_it_back() {
        let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(4));
        song.macros.energy = 0.0;
        let shared = Shared::new();
        let mut e = Engine::new(22_050.0, song, shared.clone(), None, 1);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        e.render(&mut buf);
        let hits = |v: Voice| shared.hits[v.index()].load(Ordering::Relaxed);
        assert_eq!(hits(Voice::Kick), 0, "no kick at zero energy");
        assert!(hits(Voice::Hat) > 0, "hats stay");
        e.handle(Cmd::Drop);
        let mut buf = vec![0.0; 2 * bar * 2 + 400];
        e.render(&mut buf);
        assert!(e.song.macros.energy >= 0.85, "the drop brings the energy back");
        let before = hits(Voice::Kick);
        let mut buf = vec![0.0; 2 * bar];
        e.render(&mut buf);
        assert!(hits(Voice::Kick) >= before + 4);
    }

    #[test]
    fn auto_fx_moves_the_energy_until_you_switch_it_off() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(4));
        song.auto = true;
        song.bpm = 180.0;
        let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 1);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 27 + 100];
        e.render(&mut buf); // into the breakdown – the energy glides down over two bars
        assert!(e.song.macros.energy < 0.3, "low in the breakdown: {}", e.song.macros.energy);
        // It glided: no step bigger than the glide between two updates sent to the UI
        let sent: Vec<f32> = rx
            .try_iter()
            .filter_map(|ev| if let Event::Song(s, ..) = ev { Some(s.macros.energy) } else { None })
            .collect();
        assert!(sent.iter().any(|&x| x > 0.85), "high after the start: {sent:?}");
        for w in sent.windows(2) {
            assert!((w[1] - w[0]).abs() <= 4.0 * GLIDE + 1e-3, "a jump: {} → {}", w[0], w[1]);
        }
        // The UI can't override it while the curve is on …
        let mut ui_song = e.song;
        ui_song.macros.energy = 0.9;
        e.handle(Cmd::Song(Box::new(ui_song)));
        assert!(e.song.macros.energy < 0.3);
        // … but with AUTO FX off the energy is the UI's (and no breakdown holds tracks out)
        ui_song.auto_fx = false;
        e.handle(Cmd::Song(Box::new(ui_song)));
        assert_eq!(e.song.macros.energy, 0.9);
        assert!(e.breakdown.is_none());
    }

    #[test]
    fn auto_fx_mutes_what_the_energy_takes_out_and_moves_the_mix() {
        let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(4));
        song.auto = true;
        song.bpm = 180.0;
        let home = knobs(&song);
        let mut e = Engine::new(11_025.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 27 + 100];
        e.render(&mut buf); // into the breakdown: the energy is low
                            // What the low energy takes out is muted – the mute button shows it
        for (i, t) in e.song.tracks.iter().enumerate() {
            if LAYERS[i] > e.layer_energy && !t.lock {
                assert!(t.mute, "{:?} muted at {}", Voice::ALL[i], e.layer_energy);
            }
        }
        let out = e.energy_out;
        assert!(out.iter().any(|&o| o), "the energy muted something");
        // The mix moved a little, never far from where it was put
        let k = knobs(&e.song);
        assert!(k.iter().zip(&home).any(|(a, b)| (a - b).abs() > 0.01), "the knobs moved");
        for i in 0..KNOBS {
            assert!((k[i] - home[i]).abs() <= KNOB_RANGE[i] + 1e-4, "knob {i}: {} vs {}", k[i], home[i]);
        }
        // Auto mode off and the energy up again by hand: what it muted comes back, the knobs
        // glide home
        e.send_song(None);
        let mut s = e.song;
        s.auto = false;
        s.macros.energy = 0.9;
        e.handle(Cmd::Song(Box::new(s)));
        let mut buf = vec![0.0; 2 * bar * 4];
        e.render(&mut buf);
        for (i, &o) in out.iter().enumerate() {
            assert!(!o || !e.song.tracks[i].mute, "{:?} back", Voice::ALL[i]);
        }
        let k = knobs(&e.song);
        for i in 0..KNOBS {
            assert!((k[i] - home[i]).abs() < 1e-4, "knob {i} home: {} vs {}", k[i], home[i]);
        }
    }

    #[test]
    fn a_knob_you_turn_stays_where_you_put_it() {
        let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(4));
        song.auto = true;
        let mut e = Engine::new(11_025.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar * 12];
        e.render(&mut buf);
        e.send_song(None);
        let mut s = e.song;
        s.macros.space = 0.9;
        e.handle(Cmd::Song(Box::new(s)));
        assert_eq!(e.song.macros.space, 0.9);
        assert_eq!(e.knob_home[TRACKS + 1], 0.9);
    }

    #[test]
    fn the_auto_mode_grows_an_empty_sky_one_track_per_bar() {
        let mut song = Song::empty();
        song.auto = true;
        let target = gen::grow_target(&gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2)), &mut Rng::new(3));
        let mut e = Engine::new(11_025.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Grow(Box::new(target)));
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        // Just short of the second bar
        let mut buf = vec![0.0; 2 * (bar - 64)];
        let filled = |e: &Engine| e.song.tracks.iter().filter(|t| t.hits() > 0).count();
        e.render(&mut buf);
        assert_eq!(filled(&e), 1, "the kick first");
        assert!(e.song.track(Voice::Kick).hits() > 0);
        let mut buf = vec![0.0; 2 * bar];
        e.render(&mut buf);
        assert_eq!(filled(&e), 2);
        for _ in 0..8 {
            e.render(&mut buf);
        }
        assert_eq!(filled(&e), target.tracks.iter().filter(|t| t.hits() > 0).count());
        assert!(e.grow.is_none(), "grown – the auto mode goes on as usual");
    }

    #[test]
    fn an_empty_pattern_is_silent_unless_the_metronome_ticks() {
        let mut song = Song::empty();
        song.macros.energy = 1.0;
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * 22_050];
        e.render(&mut buf);
        assert!(buf.iter().all(|x| x.abs() < 1e-3), "no ghost hats on an empty hat");
        e.handle(Cmd::Metronome(true));
        e.render(&mut buf);
        assert!(buf.iter().any(|x| x.abs() > 0.05), "the metronome clicks");
    }

    #[test]
    fn a_set_recording_waits_for_play_and_pauses_with_it() {
        let (tx, rx) = std::sync::mpsc::channel();
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
        let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 1);
        let mut buf = vec![0.0; 2 * 11_025];
        e.handle(Cmd::Capture(true));
        e.render(&mut buf);
        assert!(e.rec.is_empty(), "armed, nothing recorded while stopped");
        e.handle(Cmd::Play);
        e.render(&mut buf);
        let after_play = e.rec.len();
        assert!(after_play > 0, "it records once playing");
        e.handle(Cmd::Stop);
        e.handle(Cmd::Capture(true));
        e.render(&mut buf);
        e.render(&mut buf);
        assert!(
            e.rec.len() <= after_play + buf.len(),
            "paused: no more than a moment after the stop"
        );
        drop(rx);
    }

    #[test]
    fn a_resonant_filter_stays_clean() {
        let song = gen::generate(Vibe::Acid, &Default::default(), &mut Rng::new(3));
        for x in [-0.7, 0.5] {
            let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
            e.handle(Cmd::Play);
            e.handle(Cmd::Filter(x));
            e.handle(Cmd::FilterRes(1.0));
            let mut buf = vec![0.0; 2 * 22_050 * 3];
            e.render(&mut buf);
            assert!(e.res > 0.99);
            let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(buf.iter().all(|v| v.is_finite()) && peak <= 1.0, "filter {x}: peak {peak}");
        }
    }

    #[test]
    fn master_section_and_shaping_stay_clean() {
        let mut song = gen::generate(Vibe::Hard, &Default::default(), &mut Rng::new(9));
        song.macros.pump = 1.0;
        song.eq = crate::song::MasterEq {
            low: 1.0,
            mid: 1.0,
            high: 1.0,
            drive: 1.0,
        };
        song.prog = 3;
        for t in song.tracks.iter_mut() {
            t.tune = 12;
            t.decay = 1.0;
        }
        let buf = render_offline(&song, 4, 22_050, 1);
        let (peak, rms) = stats(&buf);
        assert!(buf.iter().all(|x| x.is_finite()) && peak <= 1.0 && rms > 0.05, "{peak} {rms}");
        for t in song.tracks.iter_mut() {
            t.tune = -12;
            t.decay = 0.0;
        }
        song.eq = crate::song::MasterEq {
            low: 0.0,
            mid: 0.0,
            high: 0.0,
            drive: 0.0,
        };
        let buf = render_offline(&song, 4, 22_050, 1);
        let (peak, _) = stats(&buf);
        assert!(buf.iter().all(|x| x.is_finite()) && peak <= 1.0);
    }

    #[test]
    fn spectrum_follows_the_music() {
        let shared = Shared::new();
        let mut e = Engine::new(22_050.0, Song::default(), shared.clone(), None, 1);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * 2048];
        e.render(&mut buf);
        let bands = shared.bands();
        assert!(bands.iter().all(|b| b.is_finite()));
        assert!(bands[0] > 0.01, "the kick shows in the lowest band: {bands:?}");
    }

    #[test]
    fn every_sound_plays_cleanly() {
        let base = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(6));
        for v in Voice::ALL {
            for s in 0..v.sounds().len() {
                let mut song = base;
                song.track_mut(v).sound = s as u8;
                song.track_mut(v).mute = false;
                let buf = render_offline(&song, 2, 22_050, 1);
                assert!(buf.iter().all(|x| x.is_finite()));
                let (peak, _) = stats(&buf);
                assert!(peak <= 1.0, "{v:?} {s}: {peak}");
            }
        }
    }

    #[test]
    fn wash_drowns_without_booming_and_rings_out() {
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        let db = |b: &[f32]| 20.0 * stats(b).1.log10();
        e.render(&mut buf);
        e.render(&mut buf);
        let dry = db(&buf);
        e.handle(Cmd::Fx(Fx::Wash, true));
        e.render(&mut buf);
        e.render(&mut buf);
        let wet = db(&buf);
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        assert!(wet < dry + 2.5, "the wash doesn't pile up: {dry:.1} → {wet:.1} dB");
        // Let go and stop: the room keeps ringing for a while
        e.handle(Cmd::Fx(Fx::Wash, false));
        e.handle(Cmd::Stop);
        e.render(&mut buf);
        e.render(&mut buf);
        assert!(db(&buf) > -45.0, "the tail rings out: {:.1} dB", db(&buf));
    }

    #[test]
    fn tape_stop_spins_back_up_onto_the_live_beat() {
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
        let sr = 22_050.0;
        let mut live = Engine::new(sr, song, Shared::new(), None, 1);
        let mut e = Engine::new(sr, song, Shared::new(), None, 1);
        live.handle(Cmd::Play);
        e.handle(Cmd::Play);
        let mut a = vec![0.0; 2 * 1024];
        let mut b = a.clone();
        let blocks = |secs: f32| (secs * sr / 1024.0) as usize;
        for _ in 0..blocks(0.5) {
            live.render(&mut a);
            e.render(&mut b);
        }
        e.handle(Cmd::Fx(Fx::Tape, true));
        for _ in 0..blocks(1.5) {
            live.render(&mut a);
            e.render(&mut b);
        }
        assert!(stats(&b).1 < 1e-4, "it stands still");
        e.handle(Cmd::Fx(Fx::Tape, false));
        let mut out = Vec::new();
        for _ in 0..blocks(0.6) {
            live.render(&mut a);
            e.render(&mut b);
            out.extend_from_slice(&b);
        }
        assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        assert!(stats(&out[..out.len() / 2]).1 > 0.01, "it spins back up");
        assert_eq!(a, b, "and is back on the live beat");
    }

    #[test]
    fn crush_keeps_the_level() {
        let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        let bar = (e.step_len() * STEPS as f64) as usize;
        let mut buf = vec![0.0; 2 * bar];
        let db = |b: &[f32]| 20.0 * stats(b).1.log10();
        e.render(&mut buf);
        e.render(&mut buf);
        let dry = db(&buf);
        e.handle(Cmd::Fx(Fx::Crush, true));
        e.render(&mut buf);
        e.render(&mut buf);
        let wet = db(&buf);
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        assert!((wet - dry).abs() < 2.0, "{dry:.1} → {wet:.1} dB");
    }

    #[test]
    fn stutter_repeats_two_steps() {
        let mut song = Song::empty();
        song.bpm = 120.0;
        song.track_mut(Voice::Rim).steps[5].on = true;
        let shared = Shared::new();
        let mut e = Engine::new(48_000.0, song, shared.clone(), None, 1);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * 6000 * 4 + 20];
        e.render(&mut buf); // steps 0…4 fired
        e.handle(Cmd::Fx(Fx::Stutter, true)); // loops steps 5 and 6
        let before = shared.hits[Voice::Rim.index()].load(Ordering::Relaxed);
        let mut buf = vec![0.0; 2 * 6000 * 8];
        e.render(&mut buf);
        let rims = shared.hits[Voice::Rim.index()].load(Ordering::Relaxed) - before;
        assert_eq!(rims, 4, "step 5 repeats every other step");
    }
}

#[cfg(test)]
mod levels {
    use super::*;
    use crate::song::Vibe;

    /// Prints the loudness of each track – and each of its sounds – on its own, for balancing the mix:
    /// `cargo test --release print_levels -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_levels() {
        let base = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(11));
        for (i, v) in Voice::ALL.into_iter().enumerate() {
            for sound in 0..v.sounds().len() {
                let mut s = base;
                for (j, t) in s.tracks.iter_mut().enumerate() {
                    t.mute = j != i;
                }
                s.tracks[i].sound = sound as u8;
                if !s.tracks[i].steps.iter().any(|st| st.on) {
                    // the groove left this track empty: off-beats, so every sound gets measured
                    for st in s.tracks[i].steps.iter_mut().skip(2).step_by(4) {
                        st.on = true;
                    }
                }
                s.macros.space = 0.0;
                s.macros.dark = 0.0;
                s.macros.energy = 1.0; // the ride only plays from 70
                let buf = render_offline(&s, 4, 44_100, 1);
                let n = buf.len() - 44_100 * 4; // without the tail
                let rms = (buf[..n].iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt();
                let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                println!(
                    "{:10} {:11} rms {:6.1} dB  peak {:6.1} dB",
                    v.name(),
                    v.sounds()[sound],
                    20.0 * rms.log10(),
                    20.0 * peak.log10()
                );
            }
        }
    }
}
