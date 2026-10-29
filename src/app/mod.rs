//! The window: top bar with the vibes, track list, the orbit (or pads) with the patterns
//! below, sound knobs, auto mode and live effects.

use crate::audio::{self, Audio};
use crate::controls;
use crate::engine::{Arrangement, Cmd, Event, Fx, Shared};
use crate::file::{self, SolarumFile};
use crate::form::FORMS;
use crate::gen;
use crate::library::{Library, SlotIds};
use crate::library_ui::{LibraryUi, Out as LibOut};
use crate::midi::{Midi, MidiIn};
use crate::orbit::{self, Live, Paint};
use crate::pads;
use crate::rng::Rng;
use crate::sampler::{slot_of, SampleBuf, Slots, CUSTOM, SAMPLE_SLOTS};
use crate::song::{Song, Track, Vibe, Voice, MOODS, STEPS, TRACKS};
use crate::theme::*;
use crate::update::{self, Release};
use crate::visual;
use crate::widgets::{self, chip, concrete, fit_text, glow};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Key, Rect, Sense, Ui};
use egui_phosphor::regular as ph;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
mod devices;
mod favorites;
mod history;
mod live;
mod project;
mod solar_os;
mod sync;
mod top_bar;
mod tracks;
mod windows;

/// Favorites: saved grooves (the first four are the patterns of AUTO SONG)
pub const SLOTS: usize = 12;
const UNDO_LIMIT: usize = 60;
const FX_KEYS: [Key; 6] = [Key::Q, Key::W, Key::E, Key::X, Key::C, Key::V];

/// A stored song (undo step, saved slot) with the current volume and auto-mode settings –
/// those belong to the moment, not to the groove
fn keep_live_settings(mut song: Song, now: &Song) -> Song {
    song.master = now.master;
    song.auto = now.auto;
    song.auto_bars = now.auto_bars;
    song.curve = now.curve;
    song.auto_fx = now.auto_fx;
    song.eq = now.eq;
    song
}

/// The same groove, whatever the live settings (energy, volume …) are
fn same_groove(a: &Song, b: &Song) -> bool {
    a.tracks == b.tracks && a.root == b.root && a.scale == b.scale && a.prog == b.prog && a.vibe == b.vibe
}

/// How much Solarum uses an effect by itself right now: every build-up runs the gate,
/// harder towards the drop – the button lights up and flickers with the chop
fn auto_fx(fx: Fx, live: &Live) -> f32 {
    match (fx, live.build) {
        (Fx::Gate, Some(p)) if live.playing => {
            let chop = if live.pos.fract() < 0.5 { 1.0 } else { 0.55 };
            (p * 1.3).clamp(0.25, 1.0) * chop
        }
        _ => 0.0,
    }
}

fn fx_icon(fx: Fx) -> &'static str {
    match fx {
        Fx::Stutter => ph::REPEAT,
        Fx::Echo => ph::WAVEFORM,
        Fx::Wash => ph::DROP,
        Fx::Crush => ph::CPU,
        Fx::Gate => ph::BARCODE,
        Fx::Tape => ph::VINYL_RECORD,
    }
}

fn fx_color(fx: Fx) -> Color32 {
    match fx {
        Fx::Stutter => track_color(Voice::Rim),
        Fx::Echo => track_color(Voice::OpenHat),
        Fx::Wash => track_color(Voice::Ride),
        Fx::Crush => track_color(Voice::Clap),
        Fx::Gate => track_color(Voice::Hat),
        Fx::Tape => track_color(Voice::Bass),
    }
}

/// The gap between the controls in the right panel, across and down
const GAP: f32 = 8.0;

/// A seed from any text (FNV-1a), the same on every computer
fn seed_of(text: &str) -> u64 {
    text.trim()
        .to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum View {
    Orbit,
    Pads,
    Planets,
}

/// The recording of a song (SONG): offered for saving when it ends
enum Export {
    Closed,
    Recorded,
    Saving(Receiver<Result<PathBuf, String>>),
}

/// A project as it stands: the groove, its twelve favorites with their names, and the file it
/// belongs to (None: not saved yet)
#[derive(Clone)]
pub struct Session {
    pub song: Song,
    pub slots: [Option<Song>; SLOTS],
    pub names: [String; SLOTS],
    pub project: Option<PathBuf>,
    /// Your own sounds in the project's slots (library ids)
    pub samples: SlotIds,
}

impl Session {
    /// Something worth coming back to: a step somewhere, or a favorite
    fn has_work(&self) -> bool {
        self.song.tracks.iter().any(|t| t.hits() > 0) || self.slots.iter().any(|s| s.is_some())
    }
}

/// What a project held when it was last saved: its tracks, favorites and their names
type Saved = ([Track; TRACKS], [Option<Song>; SLOTS], [String; SLOTS]);

/// What happens when Solarum opens
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum OnStart {
    Wait,
    Play,
}

pub struct SolarumApp {
    pub song: Song,
    sent: Song,
    tx: Sender<Cmd>,
    events: Receiver<Event>,
    pub shared: Arc<Shared>,
    audio: Result<Audio, String>,
    midi: Option<MidiIn>,
    midi_rx: Receiver<Midi>,
    midi_tx: Sender<Midi>,
    midi_ports: usize,
    midi_scan_at: f64,
    record: Arc<AtomicBool>,
    pub view: View,
    paint: Paint,
    rng: Rng,

    hit_seen: [u32; TRACKS],
    hit_age: [f32; TRACKS],
    step_seen: u64,
    step_time: f64,
    /// What Solarum says, where the auto mode is, lines from the ship (under the mood)
    pub log: crate::log::Log,
    filter: f32,
    /// The filter pad's resonance (springs back with it)
    filter_res: f32,
    /// Live effects held with the keyboard / the mouse, and what the engine was told
    pub fx_key: [bool; 6],
    /// Each effect picture's own clock (it only runs on hover or while the effect is on),
    /// when the effect was pressed last (for the burst) and whether it was on last frame
    fx_phase: [f32; 6],
    fx_pressed_at: [f64; 6],
    fx_was_on: [bool; 6],
    fx_mouse: [bool; 6],
    fx_sent: [bool; 6],
    key_down: [bool; TRACKS],
    export: Export,
    /// `Cmd::Song`s sent to this engine – its songs carry how many it had seen
    songs_sent: u64,
    /// Song mode: the performance as 16-bit stereo, its sample rate, and whether it's still coming in
    recording: Vec<i16>,
    rec_sr: u32,
    rec_live: bool,
    /// The whole session is being recorded (the record-the-set button)
    pub capture: bool,
    pub help: bool,
    audio_failed: Arc<AtomicBool>,
    audio_device: Option<String>,
    /// False in tests: never touch the sound card or MIDI there
    with_devices: bool,
    bpm_scroll: f32,
    sound_scroll: f32,
    /// Undo history: songs before each edit (grouped when edits come quickly)
    undo: Vec<Song>,
    redo: Vec<Song>,
    last_edit: f64,
    /// Set by undo/redo so the restored song doesn't count as a new edit
    restoring: bool,
    /// Four patterns; the active one (`slot_now`) keeps every edit
    pub slots: [Option<Song>; SLOTS],
    /// Names for the favorites (empty: the vibe's name is shown)
    pub names: [String; SLOTS],
    /// The favorite being renamed, and the name typed so far
    renaming: Option<usize>,
    rename_buf: String,
    pub slot_now: Option<usize>,
    /// A pattern picked while playing, waiting for the next bar
    pub queued_slot: Option<usize>,
    /// Song mode: the form playing (index into `form::FORMS`)
    pub song_form: Option<usize>,
    /// Song mode: what plays where no pattern is saved (the groove when the song started)
    song_fallback: Song,
    /// The settings window is open
    pub settings: bool,
    /// Visualizer: the whole window is a light show
    pub visual: bool,
    /// …and it's the face, not the lights
    pub face: bool,
    /// Solo: the soloed track and everyone's mute state before
    solo: Option<(usize, [bool; TRACKS])>,
    /// Play this track once the engine has the new song (sound preview)
    preview: Option<Voice>,
    update_rx: Option<Receiver<Release>>,
    pub new_version: Option<Release>,
    pub check_updates: bool,
    /// The Radar view (the old orbit) is offered next to Planets and Classic
    pub radar: bool,
    /// The sound section is shown (the SOUND heading folds it away)
    pub sound_open: bool,
    /// Settings: the log's facts and tips, what happens when Solarum opens
    /// (`OnStart`), and whether a shuffle keeps the tempo
    /// The metronome clicks on every beat
    metronome: bool,
    /// The LIVE section is shown (its heading folds it away)
    live_open: bool,
    /// The terminal's command line, and whether it has the keyboard
    cmd: String,
    typing: bool,
    /// The window into space (last frame) – the settings float over it
    space_rect: Rect,
    /// The start screen is up (at the start, and from the logo)
    pub home: bool,
    /// Projects saved or opened, newest first, and what the start screen read of them
    recent: Vec<PathBuf>,
    pub recent_cache: Vec<crate::home::Recent>,
    /// The groove from last time (the start begins with an empty sky; Continue brings it back)
    pub last_session: Option<Session>,
    /// The project as last saved or opened – the top bar shows a dot when it has changed
    saved: Option<Saved>,
    /// Leaving a project with unsaved changes (New, Open, a recent one, Solari): what waits
    /// for the answer
    pending: Option<crate::home::Action>,
    /// The file the project was last saved to or opened from – Save writes there
    project: Option<PathBuf>,
    /// When the computer (the auto mode) was switched on or off – for its animation
    pub power_at: f64,
    pub on_start: OnStart,
    pub keep_tempo: bool,
    /// What Solari learned you like (the thumbs in SOLAR OS) – saved with the settings
    pub taste: crate::ear::Taste,
    /// The auto mode's last change, its track as it was before, and when it came – for the thumbs
    pub last_auto: Option<(gen::Change, crate::song::Track, f64)>,
    /// Solari's manual (type help) is open next to SOLAR OS
    pub help_open: bool,
    /// chill (−1) … turnup (1), in thirds: how much pressure the auto mode puts on
    pub drive: f32,
    /// Where chill / turnup glide the tempo and (when the energy is yours) the energy, and
    /// the value last set on the way – a hand on it ends the glide
    glide_bpm: Option<(f32, f32)>,
    glide_energy: Option<(f32, f32)>,
    /// FACE runs in a window of its own (for a projector), next to the main one
    face_window: bool,
    /// For screenshots: pretend to play at this position
    pub fake_live: Option<Live>,
    /// Your own sounds: the library, its screen, the project's slots (and as last saved)
    pub library: Library,
    pub lib_ui: LibraryUi,
    pub sample_slots: SlotIds,
    saved_samples: SlotIds,
    /// What the engine has in its slots, and buffers it may still hold (dropped once it let go)
    samples_sent: Slots,
    retired: Vec<Arc<SampleBuf>>,
    /// When Solari drops its next line while it plays (0: not planned yet)
    solari_drop_at: f64,
}

impl SolarumApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install(&cc.egui_ctx);
        // eframe brings the window back as it was – quit inside a visualizer, and that was
        // fullscreen. Solarum always starts in its window; only a visualizer goes fullscreen.
        cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        let mut song: Song = cc.storage.and_then(|s| eframe::get_value(s, "song")).unwrap_or_default();
        song.sanitize();
        let view = cc.storage.and_then(|s| eframe::get_value(s, "view")).unwrap_or(View::Planets);
        let mut app = Self::headless(song, view, true);
        // (Before 0.4.2 there were four slots)
        let slots = cc.storage.and_then(|s| {
            eframe::get_value::<[Option<Song>; SLOTS]>(s, "slots").or_else(|| {
                eframe::get_value::<[Option<Song>; 4]>(s, "slots").map(|old| {
                    let mut all = [None; SLOTS];
                    all[..4].copy_from_slice(&old);
                    all
                })
            })
        });
        if let Some(mut slots) = slots {
            for s in slots.iter_mut().flatten() {
                s.sanitize();
            }
            app.slots = slots;
        }
        if let Some(names) = cc.storage.and_then(|s| eframe::get_value::<Vec<String>>(s, "fav_names")) {
            for (slot, name) in app.names.iter_mut().zip(names) {
                *slot = name;
            }
        }
        app.slot_now = cc
            .storage
            .and_then(|s| eframe::get_value::<Option<usize>>(s, "slot_now"))
            .flatten()
            .filter(|&i| i < SLOTS && app.slots[i].is_some());
        // First start: show how it works
        let seen: bool = cc.storage.and_then(|s| eframe::get_value(s, "seen_help")).unwrap_or(false);
        app.help = !seen;
        app.check_updates = cc.storage.and_then(|s| eframe::get_value(s, "check_updates")).unwrap_or(true);
        app.radar = cc.storage.and_then(|s| eframe::get_value(s, "radar")).unwrap_or(false);
        // A new session starts with an empty sky and everything by hand; the last groove is
        // one undo away
        // What was there last time waits behind Continue; Solarum starts with a new, empty
        // project – no groove, no favorites
        app.project = cc
            .storage
            .and_then(|s| eframe::get_value::<Option<String>>(s, "project"))
            .flatten()
            .map(PathBuf::from);
        app.library = Library::open();
        app.sample_slots = cc.storage.and_then(|s| eframe::get_value(s, "sample_slots")).unwrap_or_default();
        let last = app.session();
        app.new_project();
        app.log = crate::log::Log::new();
        app.log.push("Empty sky. Paint planets on the orbits, or switch me on below.", 0.0);
        // The start screen with the recent projects
        app.recent = cc
            .storage
            .and_then(|s| eframe::get_value::<Vec<String>>(s, "recent"))
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        app.recent_cache = crate::home::load_recent(&app.recent);
        app.home = true;
        if last.has_work() {
            app.last_session = Some(last);
        }
        app.sound_open = true;
        let get = |key: &str| cc.storage.and_then(|s| eframe::get_value::<bool>(s, key));
        app.log.facts = get("log_facts").unwrap_or(true);
        app.keep_tempo = get("keep_tempo").unwrap_or(false);
        app.taste = cc.storage.and_then(|s| eframe::get_value(s, "taste")).unwrap_or_default();
        app.send(Cmd::Taste(app.taste));
        app.shared.touches.store(get("touches").unwrap_or(true), Ordering::Relaxed);
        app.on_start = cc.storage.and_then(|s| eframe::get_value(s, "on_start")).unwrap_or(OnStart::Wait);
        // Straight into the music, if the settings say so
        if app.on_start != OnStart::Wait && app.audio.is_ok() {
            app.send(Cmd::Song(Box::new(app.song)));
            app.sent = app.song;
            app.send(Cmd::Play);
        }
        if !app.radar && app.view == View::Orbit {
            app.view = View::Planets;
        }
        let ctx = cc.egui_ctx.clone();
        if app.check_updates {
            let c = ctx.clone();
            app.update_rx = Some(update::check(move || c.request_repaint()));
        }
        app.connect_midi(move || ctx.request_repaint());
        app
    }

    /// Without a window (and optionally without sound) – used by `new` and the screenshot tests
    pub fn headless(song: Song, view: View, with_audio: bool) -> Self {
        let (tx, rx) = channel();
        let (ev_tx, events) = channel();
        let (midi_tx, midi_rx) = channel();
        let shared = Shared::new();
        let audio_failed = Arc::new(AtomicBool::new(false));
        let audio = if with_audio {
            audio::start(song, rx, ev_tx, shared.clone(), audio_failed.clone())
        } else {
            drop((rx, ev_tx));
            Err("Sound is off".into())
        };
        SolarumApp {
            song,
            sent: song,
            tx,
            events,
            shared,
            audio,
            midi: None,
            midi_rx,
            midi_tx,
            midi_ports: 0,
            midi_scan_at: 0.0,
            record: Arc::new(AtomicBool::new(false)),
            view,
            paint: Paint::default(),
            rng: Rng::from_clock(),
            hit_seen: [0; TRACKS],
            hit_age: [9.0; TRACKS],
            step_seen: u64::MAX,
            step_time: 0.0,
            log: {
                let mut log = crate::log::Log::new();
                log.push("Press play – or pick a vibe above to roll a new groove", 0.0);
                log
            },
            filter: 0.0,
            filter_res: 0.0,
            fx_key: [false; 6],
            fx_phase: [0.0; 6],
            fx_pressed_at: [-10.0; 6],
            fx_was_on: [false; 6],
            fx_mouse: [false; 6],
            fx_sent: [false; 6],
            key_down: [false; TRACKS],
            export: Export::Closed,
            songs_sent: 0,
            recording: Vec::new(),
            rec_sr: 44_100,
            rec_live: false,
            capture: false,
            help: false,
            audio_device: if with_audio { audio::default_device_name() } else { None },
            audio_failed,
            with_devices: with_audio,
            bpm_scroll: 0.0,
            sound_scroll: 0.0,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: -10.0,
            restoring: false,
            slots: [None; SLOTS],
            names: std::array::from_fn(|_| String::new()),
            renaming: None,
            rename_buf: String::new(),
            slot_now: None,
            queued_slot: None,
            song_form: None,
            song_fallback: song,
            visual: false,
            face: false,
            settings: false,
            solo: None,
            preview: None,
            update_rx: None,
            new_version: None,
            check_updates: true,
            radar: view == View::Orbit,
            sound_open: false,
            face_window: false,
            power_at: -10.0,
            home: false,
            recent: Vec::new(),
            recent_cache: Vec::new(),
            last_session: None,
            saved: None,
            pending: None,
            project: None,
            cmd: String::new(),
            metronome: false,
            live_open: true,
            typing: false,
            space_rect: Rect::from_min_size(pos2(280.0, 70.0), vec2(800.0, 600.0)),
            on_start: OnStart::Wait,
            keep_tempo: false,
            taste: Default::default(),
            last_auto: None,
            help_open: false,
            drive: 0.0,
            glide_bpm: None,
            glide_energy: None,
            fake_live: None,
            library: Library::in_memory(),
            lib_ui: LibraryUi::default(),
            sample_slots: [0; SAMPLE_SLOTS],
            saved_samples: [0; SAMPLE_SLOTS],
            samples_sent: Default::default(),
            retired: Vec::new(),
            solari_drop_at: 0.0,
        }
    }

    fn toggle_auto(&mut self, now: f64) {
        self.song.auto = !self.song.auto;
        // The computer takes over the sound; by hand it's all yours again
        self.sound_open = !self.song.auto;
        if self.song.auto && self.song_form.is_some() {
            self.song_form = None;
            self.send_arrangement();
        }
        self.say(
            if self.song.auto {
                "Auto mode on – sit back"
            } else {
                "Auto mode off"
            },
            now,
        );
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    fn say(&mut self, text: impl Into<String>, now: f64) {
        self.log.push(text, now);
    }

    fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    fn toggle_play(&mut self) {
        if self.playing() {
            // Stopping ends a song too
            self.song_form = None;
            self.queued_slot = None;
        }
        self.send(if self.playing() { Cmd::Stop } else { Cmd::Play });
        // Show the new state right away (the engine confirms on its next buffer)
        self.shared.playing.store(!self.playing(), Ordering::Relaxed);
    }

    fn shuffle(&mut self, vibe: Vibe, now: f64) {
        self.roll(vibe, None, now);
    }

    /// A groove from a word: the same seed in the same vibe always rolls the same beat
    /// (locked tracks stay as they are)
    fn seeded(&mut self, seed: &str, now: f64) {
        self.roll(self.song.vibe, Some(seed_of(seed)), now);
    }

    /// A new groove – by chance, or from a seed (then without looking at the groove before,
    /// and with the tempo the seed brings unless the settings keep it)
    fn roll(&mut self, vibe: Vibe, seed: Option<u64>, now: f64) {
        let keep_bpm = self.keep_tempo || (seed.is_none() && self.playing() && vibe == self.song.vibe);
        let keep = self.song.tracks.map(|t| t.lock.then_some(t));
        let mut seeded = seed.map(Rng::new);
        let mut s = match &mut seeded {
            Some(rng) => gen::generate_keeping(vibe, &self.song.macros, rng, &keep),
            None => gen::shuffle(vibe, &self.song.macros, &mut self.rng, &keep, &self.song),
        };
        s = keep_live_settings(s, &self.song);
        for (t, old) in s.tracks.iter_mut().zip(&self.song.tracks) {
            t.volume = old.volume;
        }
        if keep_bpm {
            s.bpm = self.song.bpm;
        }
        if self.song.mood_hold {
            s.mood_hold = true;
            s.scale = self.song.scale;
            s.prog = self.song.prog;
            gen::refit_notes(&mut s, seeded.as_mut().unwrap_or(&mut self.rng));
        }
        self.song = s;
        self.solo = None;
        // A new groove is not the scene it was rolled in – keep the scene as it was
        let keep = if self.slot_now.take().is_some() || self.slots.iter().any(|s| s.is_some()) {
            " · click an empty pattern to keep it"
        } else {
            ""
        };
        self.say(
            format!("{} · {} · {} BPM{keep}", vibe.name(), self.song.root_name(), self.song.bpm),
            now,
        );
    }

    /// Clear: back to an empty sky with every setting at its start – sound, EQ, pitch,
    /// volumes, mood (neutral); the tempo, the volume, the auto mode and locked tracks stay
    fn clear(&mut self, now: f64) {
        let mut s = Song::empty();
        s.vibe = Vibe::Custom;
        (s.bpm, s.master, s.auto, s.auto_bars, s.auto_fx) = (
            self.song.bpm,
            self.song.master,
            self.song.auto,
            self.song.auto_bars,
            self.song.auto_fx,
        );
        s.set_mood(crate::song::NEUTRAL);
        s.mood_hold = true;
        s.macros.energy = 1.0;
        for (t, old) in s.tracks.iter_mut().zip(&self.song.tracks) {
            if old.lock {
                *t = *old;
            }
        }
        self.song = s;
        self.solo = None;
        self.slot_now = None;
        self.say("Cleared – an empty sky, everything reset. Undo brings it back", now);
    }

    /// Solarum steers the energy itself: the auto mode with AUTO FX on
    fn energy_locked(&self) -> bool {
        self.song_form.is_some() || (self.song.auto && self.song.auto_fx)
    }

    /// Why the energy bar doesn't move
    fn energy_hint(&self) -> &'static str {
        if self.song_form.is_some() {
            "The song steers the energy – stop AUTO SONG to take it"
        } else {
            "Solarum steers the energy – switch AUTO FX off to take it"
        }
    }

    /// Energy by hand – in a song it takes over until the next part; in the auto mode with
    /// AUTO FX the energy is Solarum's
    fn set_energy(&mut self, e: f32, now: f64) {
        if self.energy_locked() {
            self.say(self.energy_hint(), now);
            return;
        }
        self.song.macros.energy = e.clamp(0.0, 1.0);
        self.song.curve = false;
    }

    /// LIGHTS or FACE: opens that visualizer, switches to it, or closes it if it's on
    fn show(&mut self, ctx: &egui::Context, face: bool) {
        // FACE opens a window of its own, so Solarum stays usable beside it
        if face && self.with_devices {
            self.face_window = !self.face_window;
            return;
        }
        if self.visual && self.face != face {
            self.face = face;
        } else {
            self.face = face;
            self.toggle_visual(ctx);
        }
    }
}

impl eframe::App for SolarumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "song", &self.song);
        eframe::set_value(storage, "taste", &self.taste);
        eframe::set_value(storage, "view", &self.view);
        eframe::set_value(storage, "slots", &self.slots);
        eframe::set_value(storage, "sample_slots", &self.sample_slots);
        eframe::set_value(storage, "project", &self.project.as_ref().map(|p| p.display().to_string()));
        eframe::set_value(storage, "fav_names", &self.names.to_vec());
        eframe::set_value(storage, "slot_now", &self.slot_now);
        eframe::set_value(storage, "check_updates", &self.check_updates);
        eframe::set_value(storage, "radar", &self.radar);
        eframe::set_value(storage, "log_facts", &self.log.facts);
        eframe::set_value(storage, "keep_tempo", &self.keep_tempo);
        eframe::set_value(storage, "touches", &self.shared.touches.load(Ordering::Relaxed));
        let recent: Vec<String> = self.recent.iter().map(|p| p.display().to_string()).collect();
        eframe::set_value(storage, "recent", &recent);
        eframe::set_value(storage, "on_start", &self.on_start);
        eframe::set_value(storage, "seen_help", &true);
    }
}

impl SolarumApp {
    /// One frame of the whole window
    pub fn ui(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let live = self.sync(ctx, now, dt);
        if !self.home && !self.lib_ui.open {
            self.keys(ctx, now);
        }
        // While Solari plays, it drops a line now and then (every one to two and a half minutes)
        if self.song.auto && self.playing() {
            if self.solari_drop_at == 0.0 {
                self.solari_drop_at = now + 45.0 + self.rng.f32() as f64 * 45.0;
            } else if now >= self.solari_drop_at {
                let line = *self.rng.pick(&crate::log::DROPS);
                self.bunk(line, now);
                self.solari_drop_at = now + 60.0 + self.rng.f32() as f64 * 90.0;
            }
        } else {
            self.solari_drop_at = 0.0;
        }
        let state = self.log_state();
        self.log.tick(state, now, &self.song);

        self.check_devices(ctx, now);

        // A .solarum file dropped onto the window
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        let audio: Vec<PathBuf> = dropped
            .iter()
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| crate::library::EXTENSIONS.contains(&e.to_lowercase().as_str()))
            })
            .cloned()
            .collect();
        if !audio.is_empty() {
            // Audio goes into the library (and opens it)
            self.lib_ui.open = true;
            self.lib_ui.import(&mut self.library, &audio, now);
        }
        if let Some(path) = dropped.into_iter().find(|p| p.extension().is_some_and(|e| e == file::EXTENSION)) {
            self.open_file(Some(path), now);
        }

        if self.face_window {
            self.face_viewport(ctx, &live, now);
        }
        let frame = |fill: Color32, margin: f32| egui::Frame::new().fill(fill).inner_margin(margin);
        if self.lib_ui.open {
            // Out in deep space, like the start screen and the view from the ship: stars drift past
            let bg = ctx.layer_painter(egui::LayerId::background());
            bg.rect_filled(ctx.screen_rect(), 0.0, SPACE);
            let playhead = match self.shared.audition.load(Ordering::Relaxed) {
                u64::MAX => None,
                p => Some(p as f64),
            };
            let mut outs = Vec::new();
            egui::CentralPanel::default()
                .frame(frame(Color32::TRANSPARENT, 0.0))
                .show(ctx, |ui| {
                    widgets::star_field(ui, &bg, ctx.screen_rect(), "sounds", 0.25, 0.8);
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
                    outs = self.lib_ui.show(ui, &mut self.library, &mut self.sample_slots, playhead, now);
                });
            for o in outs {
                match o {
                    LibOut::Back => {
                        self.lib_ui.open = false;
                        self.send(Cmd::Audition(None, false));
                    }
                    LibOut::Audition(buf, looped) => {
                        if let Some(b) = &buf {
                            self.retired.push(b.clone());
                        }
                        self.send(Cmd::Audition(buf, looped));
                    }
                    LibOut::Assign(voice, k) => {
                        self.song.track_mut(voice).sound = CUSTOM + k as u8;
                    }
                    LibOut::Use(id) => {
                        if let (Some(sound), Some(v)) = (self.use_sample(id, now), self.library.get(id).map(|e| e.voice)) {
                            self.song.track_mut(v).sound = sound;
                        }
                    }
                }
            }
            self.sync_samples();
            self.finish_frame(now);
            return;
        }
        self.sync_samples();
        if self.home {
            concrete(&ctx.layer_painter(egui::LayerId::background()), ctx.screen_rect());
            let update = self
                .new_version
                .as_ref()
                .map(|r| format!("{}  Version {} is out", ph::ARROW_CIRCLE_UP, r.version));
            let mut act = None;
            egui::CentralPanel::default()
                .frame(frame(Color32::TRANSPARENT, 0.0))
                .show(ctx, |ui| {
                    let name = self.last_session.as_ref().map(|l| {
                        l.project
                            .as_ref()
                            .and_then(|p| p.file_stem())
                            .map_or("Untitled".to_string(), |s| s.to_string_lossy().into_owned())
                    });
                    act = crate::home::home(
                        ui,
                        self.library.entries.len(),
                        &self.recent_cache,
                        self.last_session.as_ref().map(|l| &l.song).zip(name.as_deref()),
                        update.as_deref(),
                    );
                });
            if let Some(a) = act {
                self.home_action(a, now);
            }
            self.settings_window(ctx);
            self.help_window(ctx);
            self.unsaved_window(ctx, now);
            self.finish_frame(now);
            return;
        }
        if self.visual {
            egui::CentralPanel::default().frame(frame(SPACE, 0.0)).show(ctx, |ui| {
                let rect = ui.max_rect();
                if self.face {
                    crate::face::face(ui, rect, &self.song, &live, now);
                } else {
                    visual::visual(ui, rect, &self.song, &live, now);
                }
                // Double-click anywhere goes back
                if ui.interact(rect, ui.id().with("visual"), Sense::click()).double_clicked() {
                    self.toggle_visual(ctx);
                }
            });
            ctx.request_repaint();
            self.finish_frame(now);
            return;
        }
        // Solarum concrete behind everything; the panels sit on it
        concrete(&ctx.layer_painter(egui::LayerId::background()), ctx.screen_rect());
        let clear = Color32::TRANSPARENT;
        egui::TopBottomPanel::top("top")
            .show_separator_line(false)
            .exact_height(58.0)
            .frame(frame(clear, 6.0))
            .show(ctx, |ui| self.top_bar(ui, now));
        egui::SidePanel::left("tracks")
            .show_separator_line(false)
            .exact_width(263.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(clear).inner_margin(egui::Margin {
                left: 17,
                right: 8,
                top: 10,
                bottom: 17,
            }))
            .show(ctx, |ui| self.tracks_panel(ui, &live, now));
        egui::SidePanel::right("live")
            .show_separator_line(false)
            .exact_width(286.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(clear).inner_margin(egui::Margin {
                left: 8,
                right: 20,
                top: 10,
                bottom: 20,
            }))
            .show(ctx, |ui| self.live_panel(ui, &live, now));
        egui::CentralPanel::default().frame(frame(clear, 10.0)).show(ctx, |ui| {
            let area = ui.available_rect_before_wrap();
            self.space_rect = area;
            let scenes_h = 100.0;
            let song_h = if self.song_form.is_some() { 34.0 } else { 0.0 };
            let frame = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - scenes_h - song_h - 6.0));
            // A window into deep space, set into the concrete like the computer: stars drifting past
            let rect = widgets::bezel_frame(ui.painter(), frame, 24.0, 12.0);
            {
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 16.0, SPACE);
                let speed = if live.playing { 0.6 + self.song.macros.energy * 0.9 } else { 0.3 };
                // (kept clear of the rounded corners)
                widgets::star_field(ui, &p, rect.shrink(18.0), "space", speed, 1.0);
                if !live.playing {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
                }
            }
            // The cut edge: a dark rim and a faint light edge below it
            ui.painter()
                .rect_stroke(rect, 16.0, stroke(2.0, alpha(Color32::BLACK, 0.6)), egui::StrokeKind::Inside);
            ui.painter().rect_stroke(
                rect.expand(1.5),
                17.0,
                stroke(1.0, alpha(Color32::WHITE, 0.06)),
                egui::StrokeKind::Outside,
            );
            let inner = rect.shrink(18.0);
            match self.view {
                View::Orbit => {
                    let out = orbit::orbit(ui, inner, &mut self.song, &live, &mut self.paint);
                    if out.toggle_play {
                        self.toggle_play();
                    }
                }
                View::Planets => {
                    let out = crate::planets::planets(ui, inner, &mut self.song, &live, &mut self.paint);
                    if out.toggle_play {
                        self.toggle_play();
                    }
                }
                View::Pads => {
                    let out = pads::pads(ui, inner, &mut self.song, &live, &mut self.paint, &self.key_down);
                    for v in out.hits {
                        self.hit(v);
                    }
                }
            }
            let strip = Rect::from_min_max(pos2(area.left(), area.bottom() - scenes_h), area.max);
            self.patterns_ui(ui, strip, &live, now);
            if song_h > 0.0 {
                let r = Rect::from_min_max(pos2(area.left(), strip.top() - song_h + 2.0), pos2(area.right(), strip.top() - 4.0));
                self.song_strip(ui, r, &live, now);
            }
        });

        self.export_window(ctx, now);
        self.rename_window(ctx);
        self.settings_window(ctx);
        self.help_window(ctx);
        self.unsaved_window(ctx, now);
        self.finish_frame(now);
    }

    /// End of every frame: tell the engine what changed
    fn finish_frame(&mut self, now: f64) {
        // Live effects: held with a key or the mouse
        for i in 0..Fx::ALL.len() {
            let on = self.fx_key[i] || self.fx_mouse[i];
            if on != self.fx_sent[i] {
                self.fx_sent[i] = on;
                self.send(Cmd::Fx(Fx::ALL[i], on));
            }
        }

        if self.song != self.sent {
            if self.restoring {
                self.restoring = false;
            } else {
                // Quick successive edits (a knob turn, painting dots) are one undo step
                if now - self.last_edit > 0.8 {
                    self.undo.push(self.sent);
                    if self.undo.len() > UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                }
                self.redo.clear();
                self.last_edit = now;
                // The lit scene keeps every change
                if let Some(i) = self.slot_now {
                    if self.slots[i].is_some() {
                        self.slots[i] = Some(self.song);
                        if self.song_form.is_some() {
                            self.send_arrangement();
                        }
                    }
                }
            }
            self.sent = self.song;
            self.send(Cmd::Song(Box::new(self.song)));
            self.songs_sent += 1;
        }
        if let Some(v) = self.preview.take() {
            self.send(Cmd::Hit {
                voice: v,
                vel: 0.85,
                record: false,
            });
        }
    }
}

#[cfg(test)]
mod tests;
