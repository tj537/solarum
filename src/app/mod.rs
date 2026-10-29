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
mod project;
mod solar_os;
mod sync;
mod top_bar;
mod tracks;

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

    /// Record the session: everything you hear until it's switched off, then save it as a WAV
    fn toggle_capture(&mut self, now: f64) {
        if self.capture {
            self.capture = false;
            self.send(Cmd::Capture(false));
        } else {
            self.recording.clear();
            self.rec_live = true;
            self.rec_sr = self.audio.as_ref().map_or(44_100, |a| a.sample_rate);
            self.capture = true;
            self.send(Cmd::Capture(true));
            if !self.playing() {
                self.say("Recording armed – it starts when you press play", now);
            }
        }
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

    /// FACE in its own window: drag it to the projector; double-click or F goes fullscreen,
    /// Esc leaves fullscreen, closing the window ends it
    fn face_viewport(&mut self, ctx: &egui::Context, live: &Live, now: f64) {
        let id = egui::ViewportId::from_hash_of("face");
        let builder = egui::ViewportBuilder::default()
            .with_title("Solarum – Face")
            .with_inner_size([960.0, 600.0])
            .with_min_inner_size([320.0, 200.0]);
        let mut open = true;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(SPACE)).show(ctx, |ui| {
                let rect = ui.max_rect();
                crate::face::face(ui, rect, &self.song, live, now);
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                let toggle = ui.interact(rect, ui.id().with("face_window"), Sense::click()).double_clicked()
                    || ctx.input(|i| i.key_pressed(Key::F))
                    || (full && ctx.input(|i| i.key_pressed(Key::Escape)));
                if toggle {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
                }
            });
            if ctx.input(|i| i.viewport().close_requested()) {
                open = false;
            }
            ctx.request_repaint();
        });
        self.face_window = open;
    }

    fn toggle_visual(&mut self, ctx: &egui::Context) {
        self.visual = !self.visual;
        if self.with_devices {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.visual));
        }
    }

    fn drop_now(&mut self, now: f64) {
        if self.playing() {
            self.send(Cmd::Drop);
        } else {
            self.say("Start the beat first – then build up to the drop", now);
        }
    }

    // ───────────── Panels ─────────────

    /// Settings: volume, sound output, MIDI, update check, and the way to the code
    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // Like the help: a group per row – an icon, a title, its controls, a line about it
                let group = |ui: &mut Ui, icon: &str, title: &str, body: &mut dyn FnMut(&mut Ui)| {
                    ui.horizontal_top(|ui| {
                        ui.label(egui::RichText::new(icon).size(20.0).color(ACID));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(title).font(bold(14.0)));
                            body(ui);
                        });
                    });
                    ui.add_space(8.0);
                };
                let note = |ui: &mut Ui, text: &str| {
                    ui.label(egui::RichText::new(text).font(body(12.0)).color(DIM));
                };
                let max_h = ctx.screen_rect().height() - 200.0;
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    ui.set_width(440.0);
                    let sound = match &self.audio {
                        Ok(a) => format!(
                            "{} · {} Hz – headphones are picked up within a few seconds",
                            a.device, a.sample_rate
                        ),
                        Err(e) => format!("No sound: {e}"),
                    };
                    group(ui, ph::SPEAKER_HIGH, "Sound", &mut |ui| {
                        ui.horizontal(|ui| {
                            widgets::bar_slider(ui, &mut self.song.master, 300.0, TEXT);
                            ui.label(egui::RichText::new(format!("{:.0}", self.song.master * 100.0)).font(bold(12.0)));
                        });
                        note(ui, &sound);
                    });

                    let names = self.midi.as_ref().filter(|m| m.is_connected()).map(|m| m.names.join(", "));
                    group(ui, ph::PIANO_KEYS, "MIDI keyboard", &mut |ui| {
                        ui.label(
                            egui::RichText::new(names.as_deref().unwrap_or("None connected – plug one in, Solarum finds it"))
                                .font(bold(12.5))
                                .color(if names.is_some() { ACID } else { DIM }),
                        );
                        note(
                            ui,
                            "Keys below middle C play the sounds (C kick, D clap, E rim, F hat, G open hat, A ride, B bass). \
                             From middle C up: transpose bass and synth. Mod wheel = acid, pitch bend = filter, sustain = echo, \
                             knobs CC 70–73 (or 74/71/91/93) = energy, dark, acid, space.",
                        );
                    });

                    group(ui, ph::PLAY, "Playing", &mut |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("When Solarum opens").font(body(12.5)).color(DIM));
                            for (k, name, tip) in [
                                (OnStart::Wait, "Wait", "Nothing plays until you press play"),
                                (OnStart::Play, "Play", "The groove starts right away"),
                            ] {
                                ui.selectable_value(&mut self.on_start, k, name).on_hover_text(tip);
                            }
                        });
                        ui.checkbox(&mut self.keep_tempo, "Keep the tempo when shuffling");
                        note(ui, "Solarum always starts with an empty sky and the auto mode off.");
                    });

                    let mut touches = self.shared.touches.load(Ordering::Relaxed);
                    group(ui, ph::ROBOT, "SOLAR OS", &mut |ui| {
                        ui.checkbox(&mut self.log.facts, "Facts about the set and tips in the log");
                        ui.checkbox(&mut touches, "Ghost hats and thinner percussion from the energy");
                        note(
                            ui,
                            "With SOLAR OS off, Solarum plays exactly what you programmed and changes nothing by itself. \
                             How often Solari changes something and AUTO FX are under SET in its title bar.",
                        );
                    });
                    self.shared.touches.store(touches, Ordering::Relaxed);

                    let mut radar = self.radar;
                    group(ui, ph::CIRCLES_THREE, "Views", &mut |ui| {
                        ui.checkbox(&mut radar, "Show the legacy view (Radar)");
                        note(ui, "The tracks as rings with a sweeping beam – the view before Planets.");
                    });
                    if radar != self.radar {
                        self.radar = radar;
                        if !radar && self.view == View::Orbit {
                            self.view = View::Planets;
                        }
                    }

                    group(ui, ph::ARROW_CIRCLE_UP, "Updates", &mut |ui| {
                        ui.checkbox(&mut self.check_updates, "Tell me when a new version is out");
                        note(ui, "Asks GitHub once at start – nothing else is sent.");
                    });

                    // Support: a small card with a heart and the Ko-fi button
                    egui::Frame::new()
                        .fill(SPACE)
                        .stroke(stroke(1.0, ACID.gamma_multiply(0.35)))
                        .corner_radius(12.0)
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width() - 10.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(ph::HEART).size(26.0).color(ACID));
                                ui.add_space(4.0);
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new("Like Solarum?").font(bold(15.0)).color(TEXT));
                                    ui.label(
                                        egui::RichText::new("Help keep SOLARUM alive – free for everyone, fuelled by coffee.")
                                            .font(body(12.0))
                                            .color(DIM),
                                    );
                                });
                            });
                            ui.add_space(8.0);
                            let btn = egui::Button::new(
                                egui::RichText::new(format!("{}  Support on Ko-fi", ph::COFFEE))
                                    .font(bold(13.0))
                                    .color(SPACE),
                            )
                            .fill(ACID)
                            .corner_radius(8.0)
                            .min_size(vec2(0.0, 30.0));
                            if ui.add(btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(update::KOFI_URL));
                            }
                        });
                    ui.add_space(10.0);

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(concat!("Solarum ", env!("CARGO_PKG_VERSION")))
                                .font(body(11.5))
                                .color(FAINT),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.hyperlink_to(format!("{}  github.com/tj537/solarum", ph::GITHUB_LOGO), update::REPO_URL);
                        });
                    });
                });
            });
        self.settings = open;
    }

    fn live_panel(&mut self, ui: &mut Ui, live: &Live, now: f64) {
        let w = ui.available_width();
        let tall = ui.available_height() > 740.0;
        // Room to breathe between the controls – less of it in a short window
        let air = if tall { 8.0 } else { 3.0 };
        // One gap between every control, across and down
        ui.spacing_mut().item_spacing.y = GAP;
        // Nine knobs, always there: the sound, then pitch and the master's drive, then the EQ
        let caret = if self.sound_open { ph::CARET_DOWN } else { ph::CARET_RIGHT };
        let fold = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("SOUND  {caret}"))
                        .font(bold(11.0))
                        .color(FAINT)
                        .extra_letter_spacing(1.6),
                )
                .sense(Sense::click()),
            )
            .on_hover_text(if self.sound_open { "Fold the knobs away" } else { "Show the knobs" })
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if fold.clicked() {
            self.sound_open = !self.sound_open;
        }
        ui.add_space(6.0);
        // What the controls below need, so the character pad gets the room that's left
        let in_log = self.song.auto && self.song.auto_fx && self.song_form.is_none();
        let gap = ui.spacing().item_spacing.y;
        let energy_h = if in_log {
            0.0
        } else if self.energy_locked() {
            30.0
        } else if tall {
            58.0
        } else {
            50.0
        };
        // With the sound open everything below gets a little more compact
        let (filter_h, bh, drop_h) = match (self.sound_open, tall) {
            (false, true) => (116.0, 58.0, 96.0),
            (false, false) => (88.0, 46.0, 72.0),
            (true, true) => (68.0, 40.0, 56.0),
            (true, false) => (64.0, 36.0, 48.0),
        };
        // LIVE (folded or not) below the sound; what's left over lets LIVE's controls grow a
        // little, each up to a sensible size
        let (card_h, eq_h) = if tall { (50.0, 40.0) } else { (46.0, 36.0) };
        let live_h = if self.live_open {
            energy_h + filter_h + 3.0 * bh + drop_h + 6.0 * gap + 14.0
        } else {
            0.0
        };
        let sound_need = if self.sound_open {
            120.0 + 2.0 * card_h + eq_h + 18.0 + 4.0 * gap
        } else {
            0.0
        };
        let extra = (ui.available_height() - 30.0 - live_h - sound_need - 2.0 * gap).max(0.0);
        let filter_h = filter_h + (extra * 0.3).min(150.0 - filter_h).max(0.0);
        let bh = bh + (extra * 0.45 / 3.0).min(70.0 - bh).max(0.0);
        let drop_h = drop_h + (extra * 0.25).min(120.0 - drop_h).max(0.0);
        let below = 30.0 + energy_h + filter_h + 3.0 * bh + drop_h + 6.0 * gap + 14.0 + 2.0 * gap;
        if self.sound_open {
            let m = self.song.macros;
            let dm = crate::song::Macros::default();
            let xy_h =
                (ui.available_height() - below - 2.0 * card_h - (eq_h + 18.0) - 4.0 * gap).clamp(if tall { 80.0 } else { 64.0 }, 120.0);
            // The auto mode and songs sweep the acid: the puck shows what plays
            let sweeping = (self.song.auto || self.song_form.is_some()) && self.playing();
            let shown_acid = sweeping.then(|| f32::from_bits(self.shared.acid_now.load(Ordering::Relaxed)));
            // Character: dark × acid
            let [mut dark, mut acid] = [m.dark, m.acid];
            controls::character(ui, &mut dark, &mut acid, [dm.dark, dm.acid], shown_acid, w, xy_h)
                .on_hover_text("Drag: up = darker (longer kick, rumble), right = more acid (the synth filter) · double-click resets");
            // Turning up ACID while the synth sits muted: bring it in, or the pad seems broken
            if acid > m.acid + 0.001 && self.song.track(Voice::Acid).mute && self.solo.is_none() {
                self.song.track_mut(Voice::Acid).mute = false;
                self.say(format!("Synth in: {}", self.song.sound_name(Voice::Acid)), now);
            }
            let mm = &mut self.song.macros;
            [mm.dark, mm.acid] = [dark, acid];
            let tr = self.shared.transpose.load(Ordering::Relaxed);
            let pitch_text = if tr != 0 {
                format!("{}{tr:+}", self.song.note_name())
            } else {
                self.song.note_name().to_string()
            };
            // Two by two: space and pump as faders with what they do, pitch on a keyboard,
            // drive as a wave – all dragged up or down
            let cw = (w - GAP) / 2.0;
            let mut space = m.space;
            let mut pump = m.pump;
            let mut semis = self.song.root - 36;
            let mut drive = self.song.eq.drive;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                controls::space_card(ui, &mut space, dm.space, track_color(Voice::OpenHat), cw, card_h)
                    .on_hover_text("Space: reverb and echo on everything\nDrag up or down · double-click resets");
                controls::pump_card(ui, &mut pump, dm.pump, track_color(Voice::Clap), cw, card_h)
                    .on_hover_text("Pump: everything ducks under the kick and breathes with it\nDrag up or down · double-click resets");
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                controls::pitch_card(ui, &mut semis, 5, &pitch_text, track_color(Voice::Bass), cw, card_h).on_hover_text(
                    "Key: what key the whole song is in – bass and synth move with it, drums stay.\nThe planets' own notes are steps of the scale, so the melody stays the same.\nDrag along it · double-click resets",
                );
                controls::drive_card(ui, &mut drive, crate::song::MasterEq::default().drive, RED, cw, card_h)
                    .on_hover_text("Drive: saturation on the whole mix – warmer and louder\nDrag up or down · double-click resets");
            });
            self.song.macros.space = space;
            self.song.macros.pump = pump;
            self.song.root = 36 + semis.min(11);
            self.song.eq.drive = drive;
            // The master EQ: drag the points
            let e = &mut self.song.eq;
            let mut bands = [e.low, e.mid, e.high];
            let colors = [track_color(Voice::Bass), track_color(Voice::Rim), track_color(Voice::Hat)];
            controls::eq_curve(ui, &mut bands, colors, w, eq_h).on_hover_text(
                "Master EQ: drag a point up or down (±5 dB) – low below ~110 Hz, mid around 1 kHz, high above ~7 kHz. Double-click: flat",
            );
            [e.low, e.mid, e.high] = bands;
        }

        ui.add_space(air * 0.5);
        let caret = if self.live_open { ph::CARET_DOWN } else { ph::CARET_RIGHT };
        let fold = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("LIVE  {caret}"))
                        .font(bold(11.0))
                        .color(FAINT)
                        .extra_letter_spacing(1.6),
                )
                .sense(Sense::click()),
            )
            .on_hover_text(if self.live_open {
                "Fold the live controls away – the keys still work"
            } else {
                "Show the live controls"
            })
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if fold.clicked() {
            self.live_open = !self.live_open;
        }
        if self.live_open {
            self.live_controls(ui, live, now, w, tall, energy_h, filter_h, bh, drop_h);
        }
    }

    /// LIVE: energy, the filter pad, the effects and DROP
    #[allow(clippy::too_many_arguments)]
    fn live_controls(
        &mut self,
        ui: &mut Ui,
        live: &Live,
        now: f64,
        w: f32,
        tall: bool,
        energy_h: f32,
        filter_h: f32,
        bh: f32,
        drop_h: f32,
    ) {
        let in_log = self.song.auto && self.song.auto_fx && self.song_form.is_none();
        let _ = energy_h;
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        // Energy: the big lever of the arrangement – while the auto mode steers it, it shows in the log
        let mut e = self.song.macros.energy;
        // A song or the auto mode with AUTO FX steer the energy: then it's a slim display
        let locked = self.energy_locked();
        let h = if locked {
            30.0
        } else if tall {
            58.0
        } else {
            50.0
        };
        let label = if self.song_form.is_some() { "SONG" } else { "AUTO" };
        if !in_log {
            let out = widgets::energy_fader(ui, &mut e, w, h, locked.then_some(label));
            if out.changed {
                self.set_energy(e, now);
            }
            if out.locked_clicked {
                self.say(self.energy_hint(), now);
            }
        }
        // What the engine's filter does (smoothed, or a breakdown sweeping it), for the pad
        let engine_filter = if self.playing() {
            f32::from_bits(self.shared.filter_now.load(Ordering::Relaxed))
        } else {
            0.0
        };
        let before = (self.filter, self.filter_res);
        controls::filter_pad(ui, &mut self.filter, &mut self.filter_res, engine_filter, w, filter_h)
            .on_hover_text("Filter: drag left for low-pass, right for high-pass, up for resonance – let go and it springs back");
        if self.filter != before.0 {
            self.send(Cmd::Filter(self.filter));
        }
        if self.filter_res != before.1 {
            self.send(Cmd::FilterRes(self.filter_res));
        }
        // Effects: hold to use
        let gap = GAP;
        // Two side by side, three rows
        let bw = (w - gap) / 2.0;
        for row in 0..3 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for col in 0..2 {
                    let i = row * 2 + col;
                    let fx = Fx::ALL[i];
                    let (r, resp) = ui.allocate_exact_size(vec2(bw, bh), Sense::click_and_drag());
                    self.fx_mouse[i] = resp.is_pointer_button_down_on();
                    let active = self.fx_mouse[i] || self.fx_key[i];
                    let lit = auto_fx(fx, live);
                    // The picture stands still until the mouse is on it; it runs faster while
                    // the effect is on, and a press sends a burst out of the button
                    if active && !self.fx_was_on[i] {
                        self.fx_pressed_at[i] = now;
                    }
                    self.fx_was_on[i] = active;
                    let speed = if active {
                        4.0
                    } else if lit > 0.1 {
                        3.0
                    } else if resp.hovered() {
                        1.0
                    } else {
                        0.0
                    };
                    if speed > 0.0 {
                        self.fx_phase[i] += dt * speed;
                        ui.ctx().request_repaint();
                    }
                    let t = self.fx_phase[i];
                    let boost = if active { 1.0 } else { 0.0 };
                    widgets::fx_button(
                        ui,
                        r,
                        &resp,
                        fx_icon(fx),
                        fx.name(),
                        fx.key(),
                        fx_color(fx),
                        active,
                        lit,
                        |p, rr, ink| controls::fx_art(p, rr, i, ink, t, boost),
                    );
                    let since = (now - self.fx_pressed_at[i]) as f32;
                    if since < 0.5 {
                        controls::fx_burst(ui.painter(), r, fx_color(fx), since / 0.5);
                        ui.ctx().request_repaint();
                    }
                    resp.on_hover_text(format!("{} – hold ({})", fx.blurb(), fx.key()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            });
        }
        // The drop, right under the effects
        let (r, resp) = ui.allocate_exact_size(vec2(w, drop_h), Sense::click());
        let building = live.build;
        let p = ui.painter();
        let hot = resp.hovered();
        let pulse = building.map_or(0.0, |b| ((now * (4.0 + b as f64 * 12.0)).sin() * 0.5 + 0.5) as f32);
        widgets::glow_rect(
            p,
            r,
            14.0,
            RED,
            if building.is_some() {
                0.6 + pulse * 0.8
            } else if hot {
                0.5
            } else {
                0.2
            },
        );
        p.rect_filled(
            r,
            14.0,
            if building.is_some() {
                mix(mix(PANEL, RED, 0.3), RED, pulse * 0.5)
            } else if hot {
                mix(PAD, RED, 0.3)
            } else {
                mix(PAD, RED, 0.18)
            },
        );
        if let Some(b) = building {
            let fill = Rect::from_min_size(r.min, vec2(r.width() * b, r.height()));
            p.rect_filled(fill, 14.0, mix(PANEL, RED, 0.6));
        }
        // Warning stripes, like on a hangar door – they run while it builds up
        {
            let clip = ui.painter_at(r.shrink(2.0));
            let run = building.map_or(0.0, |_| (now * 60.0 % 24.0) as f32);
            let mut x = r.left() - r.height() + run - 24.0;
            while x < r.right() {
                clip.line_segment(
                    [pos2(x, r.bottom()), pos2(x + r.height(), r.top())],
                    stroke(8.0, RED.gamma_multiply(0.09)),
                );
                x += 24.0;
            }
        }
        let p = ui.painter();
        p.rect_stroke(r, 14.0, stroke(1.5, RED), egui::StrokeKind::Inside);
        let text = if building.is_some() {
            format!("{}  BUILDING…", ph::LIGHTNING)
        } else {
            format!("{}  DROP", ph::LIGHTNING)
        };
        p.text(r.center(), Align2::CENTER_CENTER, text, bold((drop_h * 0.3).min(24.0)), TEXT);
        p.text(
            r.right_top() + vec2(-9.0, 7.0),
            Align2::RIGHT_TOP,
            "B",
            bold(9.5),
            alpha(TEXT, 0.35),
        );
        let resp = resp
            .on_hover_text("Two bars of build-up from the next bar, then the drop (B)\nRight-click: a fill")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.drop_now(now);
        }
        resp.context_menu(|ui| {
            ui.label(egui::RichText::new("Fill – in this bar's second half, or the next one's").color(DIM));
            for f in crate::engine::FILLS {
                if ui.button(f.name()).clicked() {
                    self.send(Cmd::Fill(f));
                    self.say(
                        if self.playing() {
                            format!("Fill: {}", f.name())
                        } else {
                            "Fills play while the music runs".into()
                        },
                        now,
                    );
                    ui.close_menu();
                }
            }
        });
    }

    /// A recorded song: save it as a WAV or let it go
    fn export_window(&mut self, ctx: &egui::Context, now: f64) {
        let mut save = false;
        let mut close = false;
        match &self.export {
            Export::Closed => return,
            Export::Recorded => {
                let secs = self.recording.len() as f32 / 2.0 / self.rec_sr as f32;
                egui::Window::new("Your recording is ready")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}:{:02} · everything you heard, live moves included · {} Hz stereo",
                                secs as u32 / 60,
                                secs as u32 % 60,
                                self.rec_sr
                            ))
                            .color(DIM),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui
                                .button(egui::RichText::new(format!("{}  Save WAV…", ph::DOWNLOAD_SIMPLE)).font(bold(14.0)))
                                .clicked()
                            {
                                save = true;
                            }
                            if ui.button("Discard").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Export::Saving(rx) => match rx.try_recv() {
                Ok(Ok(path)) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    self.say(format!("Saved {name}"), now);
                    close = true;
                }
                Ok(Err(e)) => {
                    self.say(format!("Could not save: {e}"), now);
                    close = true;
                }
                Err(_) => {
                    egui::Window::new("Saving")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                        .show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Writing the WAV…");
                            });
                        });
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            },
        }
        if save {
            let name = format!(
                "solarum-{}-{:.0}bpm.wav",
                self.song.vibe.name().to_lowercase().replace(' ', "-"),
                self.song.bpm
            );
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(&name)
                .add_filter("WAV audio", &["wav"])
                .save_file()
            {
                let (tx, rx) = channel();
                let samples = std::mem::take(&mut self.recording);
                let sr = self.rec_sr;
                std::thread::spawn(move || {
                    let _ = tx.send(crate::wav::write_i16(&path, &samples, sr).map(|_| path).map_err(|e| e.to_string()));
                });
                self.export = Export::Saving(rx);
            }
        } else if close {
            self.export = Export::Closed;
            self.recording = Vec::new();
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.help;
        egui::Window::new("How Solarum works")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let row = |ui: &mut Ui, icon: &str, title: &str, text: &str| {
                    ui.horizontal_top(|ui| {
                        ui.label(egui::RichText::new(icon).size(20.0).color(ACID));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(title).font(bold(14.0)));
                            ui.label(egui::RichText::new(text).font(body(12.5)).color(DIM));
                        });
                    });
                    ui.add_space(4.0);
                };
                let max_h = ctx.screen_rect().height() - 200.0;
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    row(ui, ph::PLAY, "1 · Press play", "Solarum starts with an empty sky: paint planets on the orbits and press Space or the sun. Or switch on the computer under the tracks – Solari takes over: it keeps your beat, adds what's missing one track per bar and plays a set; its little OS has the vibe, SHUFFLE (Enter), SET, OFF and a prompt (type help). ⌘/Ctrl + Z undoes anything – the first undo brings back your last groove.");
                    row(ui, ph::PLANET, "2 · Read the planets", "Every track is an orbit round the sun, once round is one bar of 16 steps – it starts at the back, where the faint arrow at the top begins, and runs the way it points. Every step that plays is a planet; the beam sweeps round and lights up each one it passes. Bass and synth planets float higher or lower with their note. A ring like Saturn's: accent (louder). A coloured circle around it: it only plays sometimes. A trail to the planet before: the synth slides into it. A faded orbit: that track is muted. The sun is the play button. CLASSIC at the top shows the same as pads and a step grid (Tab switches); the older RADAR view can be switched on in the settings.");
                    row(ui, ph::HAND_POINTING, "3 · Change the groove", "Click a dot to switch it on or off, drag to paint. Right-click a dot (or a tile in CLASSIC, the pads and step grid – Tab) for its menu: on, accent, slide, how often it plays and – on bass and synth – its note. The mouse wheel on a bass or synth dot moves its note too.");
                    row(ui, ph::LIST, "4 · Tracks", "S solos a track, the speaker mutes it, the dice rolls a new rhythm for it (right-click: clear), the lock keeps shuffle and the auto mode away from it. The coloured pill is its sound – click it for the list, tune and decay.");
                    row(ui, ph::MUSIC_NOTES, "5 · Mood & pitch", "MOOD (in the tempo box's menu at the top) picks the notes and chords of bass and synth – from Dark and Hypnotic to Euphoric, Sunny or Mysterious. On Auto every shuffle picks one that suits the vibe; a mood you pick stays. KEY (top right) moves the whole song to another key.");
                    row(ui, ph::LIGHTNING, "6 · Energy & drop", "The energy bar is the arrangement, not the volume (← →): under 30 a break with no kick and bass, then the tracks come in one by one, from 70 everything plays (a track the energy takes out is muted – the mute button shows it); by hand it starts full. DROP (B) builds up for two bars and drops.");
                    row(ui, ph::WAVEFORM, "7 · Effects & sound", "Hold STUTTER (Q), ECHO (W), WASH (E), CRUSH (X), GATE (C) or TAPE STOP (V) and combine them; The filter pad: left low-pass, right high-pass, up resonance – it springs back when you let go. Top right: the character pad (up DARK, right ACID), SPACE, PUMP, KEY (the song's key – the planets' notes stay steps of the scale) and DRIVE, and the EQ curve – drag its points. Double-click resets.");
                    row(ui, ph::INFINITY, "8 · Auto mode", "The computer under the mood is the auto mode: its power button switches it on, Solari says hello and keeps the groove moving – something changes every 1 to 16 bars (SET in its title bar, where AUTO FX is too; OFF switches it off). Type to Solari (help opens its manual): shuffle, vibe acid, drop, like, nope, taste, chill, turnup, fx off, every 8, mood dark, bpm 135, status, joke, off. Every change is picked by ear from six tries, and the groove comes back to its theme before the drop. With AUTO FX the ENERGY slider under its menu lifts or lowers the whole set – hotter, busier, a touch faster, or the other way. Thumb its last move up or down and it learns your taste; a track you edit stays yours for two phrases. With AUTO FX on it also steers the energy and plays breakdowns, build-ups and drops by itself – the map shows the cycle ahead. With AUTO FX off the energy bar is yours and no track is ever taken out.");
                    row(ui, ph::STACK, "9 · Favorites & auto song", "Click + under the space window to keep what you hear (twelve slots), click a full one to switch on the next bar, right-click to rename, save over or clear. AUTO SONG (the arrow next to the record button) makes a whole track from favorite 1 – a quiet intro, two or three drops, favorite 2 after the first – while you jam along. Click a part in the strip to jump there.");
                    row(ui, ph::RECORD, "10 · Record", "The red record button at the top (R) records everything you hear until you click it again – then save it as a WAV. An AUTO SONG records itself.");
                    row(ui, ph::SPARKLE, "11 · Visualizers", "Two fullscreen shows for a projector, with a small energy bar at the bottom: LIGHTS (P) – a spectrum ring, shockwaves and stars – and FACE (O) – a low-poly head that nods in time, glitches in a build-up, and on the drop screams with laser eyes, cracks and bursts apart. All keys keep working. Esc or a double-click goes back.");
                    row(ui, ph::FOLDER_SIMPLE, "12 · Files & settings", "File → Save keeps the groove and your favorites in a .solarum file; open it from the menu or drag it onto the window. The gear holds volume, sound output, MIDI keyboard and the update check.");
                    row(ui, ph::KEYBOARD, "Keys", "Space play · Enter shuffle · A–K pads · 1–8 mute · ← → energy · ↑ ↓ tempo · B drop · Q W E  X C V effects (hold) · R record · Tab planets/radar/classic · P lights · O face · ⌘/Ctrl Z undo · Esc close");
                });
            });
        self.help = open;
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
