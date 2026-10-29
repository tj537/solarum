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

/// Favorites: saved grooves (the first four are the patterns of AUTO SONG)
pub const SLOTS: usize = 12;
const UNDO_LIMIT: usize = 60;
const FX_KEYS: [Key; 6] = [Key::Q, Key::W, Key::E, Key::X, Key::C, Key::V];
/// A press of an effect shorter than this is a tap: it locks the effect on (or off again)
const FX_TAP: f64 = 0.25;

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
    pub(crate) filter: f32,
    /// The filter pad's resonance (springs back with it)
    filter_res: f32,
    /// The filter stays where it is instead of springing back (double-click on the pad)
    pub(crate) filter_lock: bool,
    /// Live effects held with the keyboard / the mouse, and what the engine was told
    pub fx_key: [bool; 6],
    /// Each effect picture's own clock (it only runs on hover or while the effect is on),
    /// when the effect was pressed last (for the burst) and whether it was on last frame
    fx_phase: [f32; 6],
    fx_pressed_at: [f64; 6],
    fx_was_on: [bool; 6],
    fx_mouse: [bool; 6],
    fx_sent: [bool; 6],
    /// Effects tapped on: they stay on until the next tap; whether each was held last frame
    /// and since when (a short press is a tap)
    pub(crate) fx_latch: [bool; 6],
    fx_held: [bool; 6],
    fx_down_at: [f64; 6],
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
            filter_lock: false,
            fx_key: [false; 6],
            fx_phase: [0.0; 6],
            fx_pressed_at: [-10.0; 6],
            fx_was_on: [false; 6],
            fx_mouse: [false; 6],
            fx_sent: [false; 6],
            fx_latch: [false; 6],
            fx_held: [false; 6],
            fx_down_at: [0.0; 6],
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

    fn connect_midi(&mut self, wake: impl Fn() + Clone + Send + 'static) {
        if let Some(m) = self.midi.take() {
            m.close();
        }
        self.midi_ports = MidiIn::port_count();
        let m = MidiIn::connect(&self.tx, &self.midi_tx, self.record.clone(), wake);
        if !m.names.is_empty() {
            self.say(format!("{} connected", m.names.join(", ")), 0.0);
        }
        self.midi = Some(m);
    }

    /// Start (or restart) the audio output – after the device changed or failed
    fn restart_audio(&mut self, ctx: &egui::Context, now: f64) {
        let was_playing = self.playing();
        let (tx, rx) = channel();
        let (ev_tx, events) = channel();
        self.audio_failed.store(false, Ordering::Relaxed);
        self.audio = Err("Reconnecting…".into()); // close the old stream first
        self.audio = audio::start(self.song, rx, ev_tx, self.shared.clone(), self.audio_failed.clone());
        self.songs_sent = 0;
        self.tx = tx;
        self.events = events;
        self.sent = self.song;
        self.samples_sent = Default::default();
        self.fx_sent = [false; 6];
        self.audio_device = audio::default_device_name();
        if was_playing {
            self.send(Cmd::Play);
        }
        if self.filter != 0.0 {
            self.send(Cmd::Filter(self.filter));
        }
        self.send_arrangement();
        // A session recording goes on in the new engine
        if self.capture {
            self.send(Cmd::Capture(true));
        }
        // MIDI talks to the engine directly – reconnect it to the new one
        let c = ctx.clone();
        self.connect_midi(move || c.request_repaint());
        match &self.audio {
            Ok(a) => {
                let name = a.device.clone();
                self.say(format!("Sound now on {name}"), now);
            }
            Err(e) => {
                let e = e.clone();
                self.say(format!("No sound: {e}"), now);
            }
        }
    }

    fn undo(&mut self, now: f64) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.song);
            self.song = keep_live_settings(prev, &self.song);
            self.restoring = true;
            self.slot_now = None;
            self.say("Undo", now);
        }
    }

    fn redo(&mut self, now: f64) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.song);
            self.song = keep_live_settings(next, &self.song);
            self.restoring = true;
            self.slot_now = None;
            self.say("Redo", now);
        }
    }

    /// Click on a pattern: an empty one keeps what you hear, a full one switches to it –
    /// while playing on the next bar, with a clap roll before
    fn slot_clicked(&mut self, i: usize, save: bool, now: f64) {
        if save || self.slots[i].is_none() {
            self.slots[i] = Some(self.song);
            self.slot_now = Some(i);
            self.say(format!("Saved as favorite {} – changes now go into it", i + 1), now);
        } else if let Some(s) = self.slots[i] {
            if self.playing() {
                self.queued_slot = Some(i);
                self.send(Cmd::Queue(Box::new(keep_live_settings(s, &self.song)), i));
                self.say(format!("{} on the next bar", self.fav_name(i)), now);
            } else {
                self.song = keep_live_settings(s, &self.song);
                self.slot_now = Some(i);
                self.say(format!("{} · {}", i + 1, self.fav_name(i)), now);
            }
        }
        if self.song_form.is_some() {
            self.send_arrangement();
        }
    }

    fn clear_slot(&mut self, i: usize, now: f64) {
        self.slots[i] = None;
        if self.slot_now == Some(i) {
            self.slot_now = None;
        }
        if self.song_form.is_some() {
            self.send_arrangement();
        }
        self.names[i].clear();
        self.say(format!("Favorite {} cleared", i + 1), now);
    }

    /// The name shown on a favorite: yours, or else its vibe
    fn fav_name(&self, i: usize) -> String {
        if !self.names[i].is_empty() {
            self.names[i].clone()
        } else {
            self.slots[i].map_or(String::new(), |s| s.vibe.name().to_string())
        }
    }

    /// The favorites AUTO SONG can use (the forms play 1, then 2)
    fn song_favorites(&self) -> [Option<Song>; 4] {
        std::array::from_fn(|i| self.slots[i])
    }

    /// Naming a favorite
    fn rename_window(&mut self, ctx: &egui::Context) {
        let Some(i) = self.renaming else { return };
        let mut done = None;
        egui::Window::new(format!("Name favorite {}", i + 1))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut self.rename_buf).char_limit(24).desired_width(220.0));
                field.request_focus();
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                let esc = ui.input(|i| i.key_pressed(Key::Escape));
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("Save").font(bold(13.0))).clicked() || enter {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() || esc {
                        done = Some(false);
                    }
                });
            });
        match done {
            Some(true) => {
                self.names[i] = self.rename_buf.trim().to_string();
                self.renaming = None;
            }
            Some(false) => self.renaming = None,
            None => {}
        }
    }

    /// Tell the engine about the song (or that there is none)
    fn send_arrangement(&self) {
        let a = self.song_form.map(|form| {
            Box::new(Arrangement {
                patterns: self.song_favorites(),
                form,
                fallback: self.song_fallback,
            })
        });
        self.send(Cmd::Arrange(a));
    }

    /// Song mode: play a form with the patterns, from the top
    fn start_song(&mut self, form: usize, now: f64) {
        // Everything you hear from here on is recorded, to save as a WAV at the end
        // (already the case when the whole session is being recorded)
        if !self.capture {
            self.recording.clear();
            self.rec_live = true;
            self.rec_sr = self.audio.as_ref().map_or(44_100, |a| a.sample_rate);
        }
        self.song.auto = false;
        self.song.curve = true;
        self.song_fallback = self.song;
        self.song_form = Some(form);
        self.send_arrangement();
        if !self.playing() {
            self.send(Cmd::Play);
        }
        let f = &FORMS[form];
        let mins = f.bars() as f32 * 4.0 / self.song.bpm;
        self.say(format!("Song: {} – about {:.0} minutes", f.name, mins.max(1.0)), now);
    }

    fn stop_song(&mut self, now: f64) {
        self.song_form = None;
        self.send_arrangement();
        self.say("Song off – the pattern keeps playing", now);
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

    fn toggle_solo(&mut self, i: usize, now: f64) {
        match self.solo {
            Some((t, before)) if t == i => {
                for (tr, m) in self.song.tracks.iter_mut().zip(before) {
                    tr.mute = m;
                }
                self.solo = None;
                self.say("Solo off", now);
            }
            other => {
                let before = other.map_or_else(|| self.song.tracks.map(|t| t.mute), |(_, b)| b);
                for (j, tr) in self.song.tracks.iter_mut().enumerate() {
                    tr.mute = j != i;
                }
                self.solo = Some((i, before));
                self.say(format!("Solo: {}", Voice::ALL[i].name()), now);
            }
        }
    }

    /// Another sound for a track. The synth gets a new line when the sound needs another kind
    fn set_sound(&mut self, i: usize, sound: u8, now: f64) {
        let voice = Voice::ALL[i];
        let old = self.song.tracks[i].sound;
        if old == sound {
            return;
        }
        self.song.tracks[i].sound = sound;
        let name = self.sound_label(i);
        let built_in = |s: u8| slot_of(s).is_none();
        // With SOLAR OS on, a sound of another family gets a fitting line – by hand your line stays
        if voice == Voice::Acid && self.song.auto && built_in(old) && built_in(sound) && gen::synth_family(old) != gen::synth_family(sound)
        {
            self.roll_track(i);
            self.say(format!("{name}: new line to fit the sound (undo keeps the old one)"), now);
        } else {
            self.say(format!("{}: {name}", voice.name()), now);
        }
        if !self.playing() {
            self.preview = Some(voice);
        }
    }

    /// The name of a track's sound – a built-in one, or yours from the library
    fn sound_label(&self, i: usize) -> String {
        let voice = Voice::ALL[i];
        match slot_of(self.song.tracks[i].sound) {
            Some(k) => self
                .library
                .get(self.sample_slots[k])
                .map_or("Missing sound".into(), |e| e.name.clone()),
            None => self.song.sound_name(voice).to_string(),
        }
    }

    /// The sound value for a library sound: its slot, or a free one, or one no track plays any
    /// more; with all eight in use the SOUNDS screen asks which to replace
    pub fn use_sample(&mut self, id: u64, now: f64) -> Option<u8> {
        if let Some(k) = self.sample_slots.iter().position(|&s| s == id) {
            return Some(CUSTOM + k as u8);
        }
        let in_use = |k: usize| {
            let on = |song: &Song| song.tracks.iter().any(|t| slot_of(t.sound) == Some(k));
            on(&self.song) || self.slots.iter().flatten().any(on)
        };
        let free = self
            .sample_slots
            .iter()
            .position(|&s| s == 0)
            .or_else(|| (0..SAMPLE_SLOTS).find(|&k| !in_use(k)));
        match free {
            Some(k) => {
                self.sample_slots[k] = id;
                self.sync_samples();
                Some(CUSTOM + k as u8)
            }
            None => {
                self.say(format!("All {SAMPLE_SLOTS} of your sounds are playing – pick one to replace"), now);
                self.lib_ui.ask_replace(id);
                None
            }
        }
    }

    /// Keep the engine's slots in step with the project's (and the edits in the library), and
    /// tracks only on sounds that belong to them
    fn sync_samples(&mut self) {
        let want: Slots = std::array::from_fn(|k| match self.sample_slots[k] {
            0 => None,
            id => self.library.sound(id),
        });
        let same = want.iter().zip(&self.samples_sent).all(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        });
        if !same {
            let old = std::mem::replace(&mut self.samples_sent, want.clone());
            self.retired.extend(old.into_iter().flatten());
            self.send(Cmd::Samples(Box::new(want)));
        }
        // Freed here once the engine let go (it never frees on the audio thread)
        self.retired.retain(|b| Arc::strong_count(b) > 1);
        // A sound that left the project, or moved to another track, leaves the tracks it was on
        let owner: [Option<Voice>; SAMPLE_SLOTS] = std::array::from_fn(|k| self.library.get(self.sample_slots[k]).map(|e| e.voice));
        let fix = |song: &mut Song| {
            for (t, v) in song.tracks.iter_mut().zip(Voice::ALL) {
                if let Some(k) = slot_of(t.sound) {
                    if owner[k] != Some(v) {
                        t.sound = 0;
                    }
                }
            }
        };
        fix(&mut self.song);
        for s in self.slots.iter_mut().flatten() {
            fix(s);
        }
    }

    /// Right-click on a track's name: by hand, what SOLAR OS would do to it
    fn track_menu(&mut self, ui: &mut Ui, i: usize, now: f64) {
        let voice = Voice::ALL[i];
        ui.set_min_width(240.0);
        ui.label(egui::RichText::new(voice.name()).font(bold(13.0)).color(track_color(voice)));
        if self.song.tracks[i].lock {
            ui.label(egui::RichText::new("Locked – unlock it to change it").color(DIM));
            return;
        }
        let name = voice.name();
        let mut said = None;
        if ui.button(format!("{}  New rhythm", ph::DICE_FIVE)).clicked() {
            self.roll_track(i);
            said = Some(format!("New {} rhythm", name.to_lowercase()));
        }
        if voice.is_synth() || voice == Voice::Bass {
            if ui.button(format!("{}  New notes, same rhythm", ph::MUSIC_NOTES)).clicked() {
                gen::new_notes(&mut self.song, voice, &mut self.rng);
                said = Some(format!("{name}: new notes on the same rhythm"));
            }
        } else if voice != Voice::Kick {
            if ui.button(format!("{}  Add ghost notes", ph::GHOST)).clicked() {
                said = Some(if gen::add_ghosts(&mut self.song, voice, &mut self.rng) {
                    format!("{name}: soft ghost notes in between")
                } else {
                    format!("{name}: no room for ghost notes")
                });
            }
            if self.song.tracks[i].steps.iter().any(|s| s.ghost) && ui.button(format!("{}  Remove ghost notes", ph::ERASER)).clicked() {
                gen::remove_ghosts(&mut self.song, voice);
                said = Some(format!("{name}: ghost notes out"));
            }
            if ui
                .button(format!("{}  Thin out – plain steps play half the time", ph::FUNNEL))
                .clicked()
            {
                gen::set_chance(&mut self.song, voice, 50);
                said = Some(format!("{name}: thinned out – accents always play"));
            }
            if self.song.tracks[i].steps.iter().any(|s| s.on && s.chance < 100)
                && ui.button(format!("{}  Every step always", ph::CHECK_CIRCLE)).clicked()
            {
                gen::set_chance(&mut self.song, voice, 100);
                said = Some(format!("{name}: every step plays"));
            }
        }
        if ui.button(format!("{}  Clear", ph::TRASH)).clicked() {
            for st in self.song.tracks[i].steps.iter_mut() {
                *st = crate::song::Step::default();
            }
            said = Some(format!("{name} cleared"));
        }
        if let Some(text) = said {
            self.song.tracks[i].mute = false;
            self.say(text, now);
            ui.close_menu();
        }
    }

    /// A new rhythm for one track (keeps its sound, volume and mute)
    fn roll_track(&mut self, i: usize) {
        if self.song.tracks[i].lock {
            return;
        }
        let voice = Voice::ALL[i];
        let fresh = gen::track_for(voice, self.song.vibe, self.song.macros.energy, &self.song, &mut self.rng);
        let t = &mut self.song.tracks[i];
        *t = Track {
            mute: t.mute,
            volume: t.volume,
            ..fresh
        };
    }

    /// The dice on every track that's on (and not locked): new patterns, same vibe, key and
    /// sounds; returns how many rolled
    fn dice_all(&mut self) -> usize {
        let on: Vec<usize> = (0..TRACKS)
            .filter(|&i| !self.song.tracks[i].mute && !self.song.tracks[i].lock)
            .collect();
        for &i in &on {
            self.roll_track(i);
        }
        on.len()
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// What plays, for the log
    fn log_state(&self) -> crate::log::State {
        use crate::log::{State, Where};
        let live = self.fake_live.as_ref().map(|l| l.playing);
        if !live.unwrap_or_else(|| self.playing()) {
            return State::Stopped;
        }
        let bar = self.shared.step.load(Ordering::Relaxed) / STEPS as u64;
        if self.song_form.is_some() {
            State::Song
        } else if self.song.auto && self.song.auto_fx {
            State::Auto(Where::read(&self.shared, bar))
        } else {
            State::Bar(bar)
        }
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

    /// Save or open a .solarum file
    /// Back to the start screen (the recent projects read again)
    fn open_home(&mut self) {
        self.recent_cache = crate::home::load_recent(&self.recent);
        // Continue goes back to this project as it is now
        let now = self.session();
        if now.has_work() {
            self.last_session = Some(now);
        }
        self.home = true;
    }

    fn home_action(&mut self, action: crate::home::Action, now: f64) {
        use crate::home::Action;
        // Leaving a project with changes that aren't saved: ask first
        let leaves = matches!(action, Action::New | Action::Open | Action::Recent(_));
        if leaves && self.dirty() && self.pending.is_none() {
            self.pending = Some(action);
            return;
        }
        match action {
            Action::New => {
                self.new_project();
                self.home = false;
                self.say("A new project: an empty sky, no favorites. Paint planets, or switch Solari on", now);
            }
            Action::Continue => {
                if let Some(last) = self.last_session.take() {
                    self.restore(last);
                    self.say("Back where you left off", now);
                }
                self.home = false;
            }
            Action::Open => self.open_file(None, now),
            Action::Recent(i) => {
                if let Some(path) = self.recent.get(i).cloned() {
                    self.open_file(Some(path), now);
                }
            }
            Action::Forget(i) => {
                if i < self.recent.len() {
                    self.recent.remove(i);
                    self.recent_cache = crate::home::load_recent(&self.recent);
                }
            }
            Action::Settings => self.settings = true,
            Action::Help => self.help = true,
            Action::Sounds => self.lib_ui.show_for(None),
        }
    }

    /// The project as it stands
    fn session(&self) -> Session {
        Session {
            song: self.song,
            slots: self.slots,
            names: self.names.clone(),
            project: self.project.clone(),
            samples: self.sample_slots,
        }
    }

    /// Back to a project as it was (Continue)
    fn restore(&mut self, s: Session) {
        self.song = keep_live_settings(s.song, &self.song);
        self.song.auto = false;
        self.slots = s.slots;
        self.names = s.names;
        self.project = s.project;
        self.sample_slots = s.samples;
        self.slot_now = None;
        self.queued_slot = None;
        self.mark_saved_if_unchanged_on_disk();
    }

    /// A new project: an empty sky, no favorites, not saved anywhere yet
    fn new_project(&mut self) {
        // Everything from scratch: no locked tracks carried over, default tempo, sounds,
        // volumes and knobs, the computer off, the energy full – only the volume stays
        let mut fresh = Song::empty();
        fresh.vibe = Vibe::Custom;
        fresh.set_mood(crate::song::NEUTRAL);
        fresh.mood_hold = true;
        fresh.macros.energy = 1.0;
        fresh.master = self.song.master;
        self.song = fresh;
        self.solo = None;
        (self.filter, self.filter_res, self.filter_lock) = (0.0, 0.0, false);
        self.fx_latch = [false; 6];
        self.send(Cmd::Filter(0.0));
        self.send(Cmd::FilterRes(0.0));
        // A new project has its own history
        self.undo.clear();
        self.redo.clear();
        self.slots = [None; SLOTS];
        self.names = std::array::from_fn(|_| String::new());
        self.slot_now = None;
        self.queued_slot = None;
        self.song_form = None;
        self.send_arrangement();
        self.project = None;
        self.sample_slots = [0; SAMPLE_SLOTS];
        self.mark_saved();
        self.log = crate::log::Log::new();
        self.log.push("Empty sky. Paint planets on the orbits, or switch me on below.", 0.0);
    }

    /// What's here now counts as saved
    fn mark_saved(&mut self) {
        self.saved = Some((self.song.tracks, self.slots, self.names.clone()));
        self.saved_samples = self.sample_slots;
    }

    /// Continue: the project counts as saved if its file still holds the same
    fn mark_saved_if_unchanged_on_disk(&mut self) {
        self.saved = self.project.as_ref().and_then(|p| SolarumFile::load(p).ok()).map(|f| {
            let mut slots = [None; SLOTS];
            let mut names: [String; SLOTS] = std::array::from_fn(|_| String::new());
            for i in 0..SLOTS {
                slots[i] = f.scenes.get(i).copied().flatten();
                names[i] = f.names.get(i).cloned().unwrap_or_default();
            }
            (f.song.tracks, slots, names)
        });
    }

    /// Changes since the last save or open (an empty new project has none)
    fn dirty(&self) -> bool {
        let now = self.session();
        match &self.saved {
            Some((tracks, slots, names)) => {
                now.song.tracks.map(|t| (t.steps, t.sound, t.mute)) != tracks.map(|t| (t.steps, t.sound, t.mute))
                    || &now.slots != slots
                    || &now.names != names
                    || now.samples != self.saved_samples
            }
            None => now.has_work(),
        }
    }

    /// Asked before leaving a project with unsaved changes: save, don't save, or stay
    fn unsaved_window(&mut self, ctx: &egui::Context, now: f64) {
        let Some(action) = self.pending.take() else { return };
        let name = self.project_name();
        let mut answer = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(format!("{name} has changes that aren't saved.")).font(bold(14.0)));
                ui.label(egui::RichText::new("Save them before you go on?").font(body(12.5)).color(DIM));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("Save").font(bold(13.0))).clicked() {
                        answer = Some(0);
                    }
                    if ui.button(egui::RichText::new("Don't save").font(bold(13.0))).clicked() {
                        answer = Some(1);
                    }
                    if ui.button(egui::RichText::new("Cancel").font(bold(13.0))).clicked() {
                        answer = Some(2);
                    }
                });
            });
        match answer {
            Some(0) => {
                self.save_file(false, now);
                if !self.dirty() {
                    self.mark_saved();
                    self.home_action(action, now);
                }
            }
            Some(1) => {
                self.mark_saved();
                self.home_action(action, now);
            }
            Some(_) => {}
            None => self.pending = Some(action),
        }
    }

    /// The project's name: its file's, or "Untitled"
    fn project_name(&self) -> String {
        self.project
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned())
    }

    /// A project saved or opened goes to the top of the recent list
    fn remember(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(12);
        self.project = Some(path.to_path_buf());
    }

    /// Save: to the project's file if it has one (`as_new`: always ask where)
    fn save_file(&mut self, as_new: bool, now: f64) {
        if let Some(path) = self.project.clone().filter(|_| !as_new) {
            self.write_project(path, now);
            return;
        }
        let name = format!(
            "{}-{:.0}bpm.{}",
            self.song.vibe.name().to_lowercase().replace(' ', "-"),
            self.song.bpm,
            file::EXTENSION
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("Solarum", &[file::EXTENSION])
            .save_file()
        else {
            return;
        };
        self.write_project(path, now);
    }

    fn write_project(&mut self, path: PathBuf, now: f64) {
        let f = SolarumFile {
            song: self.song,
            scenes: self.slots.to_vec(),
            names: self.names.to_vec(),
            sample_slots: self.sample_slots.to_vec(),
            samples: self.library.pack(&self.sample_slots),
            ..Default::default()
        };
        match f.save(&path) {
            Ok(()) => {
                self.say(format!("Saved {}", path.file_name().unwrap_or_default().to_string_lossy()), now);
                self.remember(&path);
                self.mark_saved();
            }
            Err(e) => self.say(format!("Could not save: {e}"), now),
        }
    }

    fn open_file(&mut self, path: Option<PathBuf>, now: f64) {
        let path = path.or_else(|| rfd::FileDialog::new().add_filter("Solarum", &[file::EXTENSION]).pick_file());
        let Some(path) = path else { return };
        match SolarumFile::load(&path) {
            Ok(f) => {
                self.load_file(f, &path.file_name().unwrap_or_default().to_string_lossy(), now);
                self.remember(&path);
                self.mark_saved();
                self.home = false;
            }
            Err(e) => self.say(format!("Could not open it: {e}"), now),
        }
    }

    fn load_file(&mut self, f: SolarumFile, name: &str, now: f64) {
        self.song = keep_live_settings(f.song, &self.song);
        self.song.auto = f.song.auto;
        for i in 0..SLOTS {
            self.slots[i] = f.scenes.get(i).copied().flatten();
            self.names[i] = f.names.get(i).cloned().unwrap_or_default();
        }
        self.library.unpack(&f.samples);
        self.sample_slots = std::array::from_fn(|k| f.sample_slots.get(k).copied().unwrap_or(0));
        self.slot_now = None;
        self.queued_slot = None;
        self.song_form = None;
        self.solo = None;
        self.send_arrangement();
        self.say(format!("Opened {name} – undo goes back"), now);
    }

    fn hit(&mut self, voice: Voice) {
        let record = self.record.load(Ordering::Relaxed);
        self.send(Cmd::Hit { voice, vel: 0.9, record });
    }

    // ───────────── Per-frame state from the engine ─────────────

    fn sync(&mut self, ctx: &egui::Context, now: f64, dt: f32) -> Live {
        while let Ok(ev) = self.events.try_recv() {
            let (song, change, ack) = match ev {
                Event::Song(song, change, ack) => (song, change, ack),
                Event::Audio(piece) => {
                    if self.rec_live {
                        self.recording.extend(piece.iter().map(|&s| crate::wav::to_i16(s)));
                        // An hour is plenty (about 350 MB in memory)
                        if self.capture && self.recording.len() >= self.rec_sr as usize * 2 * 3600 {
                            self.toggle_capture(now);
                        }
                    }
                    continue;
                }
                Event::Recorded => {
                    self.capture = false;
                    // At least two seconds: worth keeping
                    if std::mem::take(&mut self.rec_live) && self.recording.len() > self.rec_sr as usize * 4 {
                        self.export = Export::Recorded;
                    }
                    continue;
                }
            };
            // A song the engine sent before it had our last edit would undo that edit
            if ack >= self.songs_sent {
                self.song = *song;
                self.sent = self.song;
                // The lit favorite only stays lit while the groove is exactly what's saved
                let loaded = matches!(change, Some(gen::Change::Scene(_) | gen::Change::Part(..)));
                if let Some(i) = self.slot_now.filter(|_| !loaded) {
                    if !self.slots[i].is_some_and(|s| same_groove(&s, &self.song)) {
                        self.slot_now = None;
                    }
                }
            }
            if let Some(c) = change {
                match c {
                    gen::Change::Scene(i) => {
                        self.slot_now = Some(i);
                        self.queued_slot = None;
                    }
                    gen::Change::Part(_, _, slot) => {
                        self.slot_now = slot;
                        self.queued_slot = None;
                    }
                    gen::Change::SongEnd => self.song_form = None,
                    // A drop that starts a new chapter says so instead of "Drop"
                    gen::Change::Drop | gen::Change::NewChapter => self.log.note_drop(),
                    _ => {}
                }
                let text = match c {
                    gen::Change::Scene(i) => format!("{} · {}", i + 1, self.fav_name(i)),
                    _ => c.describe(),
                };
                self.say(text, now);
            }
        }
        while let Ok(m) = self.midi_rx.try_recv() {
            match m {
                Midi::Macro(0, v) => self.set_energy(v, now),
                Midi::Macro(i, v) => {
                    let m = &mut self.song.macros;
                    *[&mut m.energy, &mut m.dark, &mut m.acid, &mut m.space][i] = v;
                }
                Midi::Filter(x) => self.filter = x,
                Midi::Transpose(Some(t)) if t != 0 => self.say(format!("Transposed {t:+}"), now),
                _ => {}
            }
        }
        if let Some(rx) = &self.update_rx {
            if let Ok(r) = rx.try_recv() {
                self.say(format!("Solarum {} is out – click the green button at the top", r.version), now);
                self.new_version = Some(r);
                self.update_rx = None;
            }
        }
        if let Some(live) = &self.fake_live {
            return live.clone();
        }
        for i in 0..TRACKS {
            let n = self.shared.hits[i].load(Ordering::Relaxed);
            if n != self.hit_seen[i] {
                self.hit_seen[i] = n;
                self.hit_age[i] = 0.0;
            } else {
                self.hit_age[i] += dt;
            }
        }
        let playing = self.playing();
        let step = self.shared.step.load(Ordering::Relaxed);
        let part = match self.shared.part.load(Ordering::Relaxed) {
            0 => None,
            k => Some((k as usize - 1, self.shared.part_start.load(Ordering::Relaxed))),
        };
        if step != self.step_seen {
            self.step_seen = step;
            self.step_time = now;
        }
        let step_dur = 15.0 / self.song.bpm as f64;
        let pos = step as f64 + ((now - self.step_time) / step_dur).min(0.999);
        if playing || self.hit_age.iter().any(|&a| a < 1.0) || self.filter != 0.0 {
            ctx.request_repaint();
        } else if self.capture {
            // The recording timer keeps counting while the music is paused
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        Live {
            playing,
            pos,
            hit_age: self.hit_age,
            build: self.shared.build_progress(),
            bar: step / STEPS as u64,
            part: part
                .filter(|_| self.song_form.is_some())
                .map(|(i, start)| (i, (pos / STEPS as f64 - start as f64).max(0.0))),
            bands: self.shared.bands(),
            level: self.shared.level(),
        }
    }

    fn keys(&mut self, ctx: &egui::Context, now: f64) {
        // Naming a favorite or typing to Solari: the keys belong to the text field
        if self.renaming.is_some() || self.typing {
            return;
        }
        // No text fields here: never let a focused button eat Space or Enter
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        // Window lost focus while an effect key was held? Let go, or the effect would stick
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false)))) {
            self.fx_key = [false; 6];
            self.key_down = [false; TRACKS];
        }
        let tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab));
        if tab {
            // Planets → (radar →) classic → planets
            self.view = match self.view {
                View::Planets if self.radar => View::Orbit,
                View::Planets => View::Pads,
                View::Orbit => View::Pads,
                View::Pads => View::Planets,
            };
        }
        let events = ctx.input(|i| i.events.clone());
        let pad_keys = [Key::A, Key::S, Key::D, Key::F, Key::G, Key::H, Key::J, Key::K];
        let num_keys = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
        ];
        for e in events {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = e
            else {
                continue;
            };
            if pressed && (modifiers.command || modifiers.ctrl) {
                match key {
                    Key::Z if modifiers.shift => self.redo(now),
                    Key::Z => self.undo(now),
                    Key::Y => self.redo(now),
                    Key::S => self.save_file(modifiers.shift, now),
                    _ => {}
                }
                continue;
            }
            if modifiers.command || modifiers.ctrl {
                continue;
            }
            if pressed && key == Key::Escape {
                if self.visual {
                    self.toggle_visual(ctx);
                }
                self.help = false;
                self.settings = false;
                // … and lets go of everything locked on
                self.fx_latch = [false; 6];
                self.filter_lock = false;
                continue;
            }
            if let Some(i) = pad_keys.iter().position(|&k| k == key) {
                if pressed && !repeat {
                    self.hit(Voice::ALL[i]);
                }
                self.key_down[i] = pressed;
                continue;
            }
            if let Some(i) = FX_KEYS.iter().position(|&k| k == key) {
                if !repeat {
                    self.fx_key[i] = pressed;
                }
                continue;
            }
            if !pressed || repeat {
                continue;
            }
            if let Some(i) = num_keys.iter().position(|&k| k == key) {
                self.song.tracks[i].mute = !self.song.tracks[i].mute;
                continue;
            }
            match key {
                Key::Space => self.toggle_play(),
                Key::Enter => self.shuffle(self.song.vibe, now),
                Key::B => self.drop_now(now),
                Key::ArrowRight => self.set_energy(self.song.macros.energy + 0.05, now),
                Key::ArrowLeft => self.set_energy(self.song.macros.energy - 0.05, now),
                Key::P => self.show(ctx, false),
                Key::O => self.show(ctx, true),
                Key::ArrowUp => self.song.bpm = (self.song.bpm + 1.0).min(180.0),
                Key::ArrowDown => self.song.bpm = (self.song.bpm - 1.0).max(90.0),
                Key::R => self.toggle_capture(now),
                _ => {}
            }
        }
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

    fn top_bar(&mut self, ui: &mut Ui, now: f64) {
        ui.horizontal_centered(|ui| {
            ui.add_space(6.0);
            if ui
                .add(
                    egui::Label::new(
                        egui::RichText::new("SOLARUM")
                            .font(display(30.0))
                            .color(TEXT)
                            .extra_letter_spacing(2.0),
                    )
                    .sense(Sense::click()),
                )
                .on_hover_text("The start screen: new, open, recent projects")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.open_home();
            }
            ui.label(
                egui::RichText::new(format!("{}{}", self.project_name(), if self.dirty() { "  •" } else { "" }))
                    .font(bold(12.0))
                    .color(DIM),
            );
            ui.add_space(18.0);
            ui.add_space(10.0);
            if let Some(r) = &self.new_version {
                let text = format!("{}  Update to {}", ph::ARROW_CIRCLE_UP, r.version);
                if chip(ui, &text, true, ACID)
                    .on_hover_text("A new version is on GitHub – opens the download page")
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(update::RELEASES_URL));
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let icon_button = |ui: &mut Ui, icon: &str, tip: &str| {
                    ui.add(egui::Button::new(egui::RichText::new(icon).size(18.0)).frame(false))
                        .on_hover_text(tip)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                };
                if icon_button(ui, ph::QUESTION, "How it works").clicked() {
                    self.help = !self.help;
                }
                if icon_button(ui, ph::GEAR_SIX, "Settings: volume, sound output, MIDI, updates").clicked() {
                    self.settings = !self.settings;
                }
                let mut action = None;
                ui.add_space(10.0);
                ui.menu_button(egui::RichText::new(format!("{}  File", ph::FOLDER_SIMPLE)).font(bold(13.0)), |ui| {
                    ui.set_min_width(230.0);
                    for (i, label) in [
                        format!("{}  New project", ph::PLUS),
                        format!("{}  Save   (⌘/Ctrl + S)", ph::FLOPPY_DISK),
                        format!("{}  Save as…", ph::FLOPPY_DISK_BACK),
                        format!("{}  Open…", ph::FOLDER_OPEN),
                        format!("{}  Start screen", ph::HOUSE),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if ui.button(egui::RichText::new(label).font(bold(13.0))).clicked() {
                            action = Some(i);
                            ui.close_menu();
                        }
                    }
                    ui.label(
                        egui::RichText::new(
                            "A project is one .solarum file: the groove and its twelve favorites. A new project starts without favorites.",
                        )
                        .font(body(11.0))
                        .color(FAINT),
                    );
                });
                match action {
                    Some(0) => self.home_action(crate::home::Action::New, now),
                    Some(1) => self.save_file(false, now),
                    Some(2) => self.save_file(true, now),
                    Some(3) => self.home_action(crate::home::Action::Open, now),
                    Some(4) => self.open_home(),
                    _ => {}
                }
                // Some air between the menu and the editing buttons
                ui.add_space(14.0);
                for (icon, tip, is_redo) in [
                    (ph::ARROW_CLOCKWISE, "Redo (⌘/Ctrl + Shift + Z)", true),
                    (ph::ARROW_COUNTER_CLOCKWISE, "Undo (⌘/Ctrl + Z)", false),
                ] {
                    let enabled = if is_redo { !self.redo.is_empty() } else { !self.undo.is_empty() };
                    let b = egui::Button::new(egui::RichText::new(icon).size(18.0)).frame(false);
                    if ui.add_enabled(enabled, b).on_hover_text(tip).clicked() {
                        if is_redo {
                            self.redo(now);
                        } else {
                            self.undo(now);
                        }
                    }
                }
                ui.add_space(10.0);
                // Clear: an empty sky (undo brings it back)
                let empty = self.song.tracks.iter().all(|t| t.hits() == 0 || t.lock);
                let b = egui::Button::new(egui::RichText::new(ph::BROOM).size(18.0)).frame(false);
                if ui
                    .add_enabled(!empty, b)
                    .on_hover_text("Clear: every step off (locked tracks stay) – undo brings it back")
                    .clicked()
                {
                    self.clear(now);
                }
                // No sound: say so right here (volume, device and MIDI are in the settings)
                // (tests and screenshots run without a sound card)
                if let Err(e) = &self.audio {
                    if self.with_devices {
                        let e = e.clone();
                        ui.label(
                            egui::RichText::new(format!("{}  No sound", ph::SPEAKER_SLASH))
                                .font(bold(13.0))
                                .color(RED),
                        )
                        .on_hover_text(e);
                    }
                }
                ui.add_space(8.0);
                // Record: everything you hear, to a WAV (red with a timer while it runs)
                let text = if self.capture {
                    let secs = self.recording.len() as u32 / 2 / self.rec_sr.max(1);
                    format!("{}  {}:{:02}", ph::RECORD, secs / 60, secs % 60)
                } else {
                    ph::RECORD.to_string()
                };
                let (rec, more) = widgets::rec_split(ui, &text, self.capture || self.song_form.is_some(), now);
                if rec
                    .on_hover_text(if self.capture {
                        "Recording – click to stop and save it as a WAV (R)"
                    } else {
                        "Record what you hear – auto mode, jamming, songs, effects – until you click again, then save it as a WAV (R)"
                    })
                    .clicked()
                {
                    self.toggle_capture(now);
                }
                self.song_menu(ui, more, now);
                ui.add_space(4.0);
                // The two visualizers behind one button: a menu with a picture of each
                let showing = self.face_window || self.visual;
                let vis = chip(ui, &format!("{}  {}", ph::SPARKLE, ph::CARET_DOWN), showing, ACID)
                    .on_hover_text("Visualizers for the big screen: LIGHTS (P) and FACE (O)");
                let popup = ui.make_persistent_id("visualizers");
                if vis.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(popup));
                }
                let mut pick = None;
                egui::popup_below_widget(ui, popup, &vis, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                    ui.horizontal(|ui| {
                        for face in [false, true] {
                            let (r, resp) = ui.allocate_exact_size(vec2(190.0, 150.0), Sense::click());
                            controls::visualizer_card(ui.painter(), r, face, resp.hovered(), now as f32);
                            let tip = if face {
                                "FACE: a low-poly head that nods in time and screams on the drop – in its own window for the projector (O)"
                            } else {
                                "LIGHTS: the whole screen becomes a light show – spectrum ring, shockwaves, stars (P)"
                            };
                            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                pick = Some(face);
                            }
                        }
                    });
                });
                if let Some(face) = pick {
                    let ctx = ui.ctx().clone();
                    self.show(&ctx, face);
                }
                ui.add_space(4.0);
                // The view: PLANETS (the main one), CLASSIC (pads and grid), and RADAR (the old
                // orbit) when the settings ask for it
                let order: &[View] = if self.radar {
                    &[View::Planets, View::Orbit, View::Pads]
                } else {
                    &[View::Planets, View::Pads]
                };
                let picked: Vec<bool> = order.iter().map(|&v| self.view == v).collect();
                // In a small window only the view you're in has its name
                let compact = ui.ctx().screen_rect().width() < 1250.0;
                let views: Vec<(String, &str)> = order
                    .iter()
                    .zip(&picked)
                    .map(|(v, &on)| {
                        let (icon, name, tip) = match v {
                            View::Planets => (
                                ph::PLANET,
                                "PLANETS",
                                "Planets: every track an orbit round the sun, every step a planet (Tab)",
                            ),
                            View::Orbit => (ph::CIRCLES_THREE, "RADAR", "Radar: the tracks as rings with a sweeping beam (Tab)"),
                            View::Pads => (ph::GRID_FOUR, "CLASSIC", "Classic: pads and a step grid (Tab)"),
                        };
                        let text = if compact && !on {
                            icon.to_string()
                        } else {
                            format!("{icon}  {name}")
                        };
                        (text, tip)
                    })
                    .collect();
                if let Some(i) = widgets::segmented(ui, &views, &picked, ACID) {
                    self.view = order[i];
                }
                ui.add_space(12.0);
                self.bpm_box(ui, now);
            });
        });
    }

    /// The presets: a dropdown of vibes (picking one rolls a new groove in it) and SHUFFLE
    /// The vibes, for the terminal's VIBE button: a pick rolls a new groove in it
    fn vibe_list(&mut self, ui: &mut Ui, now: f64) {
        let vibe = self.song.vibe;
        ui.set_min_width(340.0);
        ui.label(
            egui::RichText::new("What should Solari play?")
                .font(egui::FontId::monospace(12.0))
                .color(ACID),
        );
        ui.add_space(4.0);
        let fmt = |font, color| egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        };
        let mut picked = None;
        for v in Vibe::ALL.into_iter().filter(|&v| v != Vibe::Custom) {
            let mut job = egui::text::LayoutJob::default();
            let on = v == vibe;
            job.append(&format!("> {}", v.name()), 0.0, fmt(egui::FontId::monospace(13.0), ACID));
            job.append(
                &format!("   {}", v.blurb().to_lowercase()),
                0.0,
                fmt(egui::FontId::monospace(11.0), ACID.gamma_multiply(0.55)),
            );
            if ui.selectable_label(on, job).clicked() {
                picked = Some(v);
            }
        }
        let mut job = egui::text::LayoutJob::default();
        job.append("> Surprise me", 0.0, fmt(egui::FontId::monospace(13.0), ACID));
        job.append(
            "   Solari picks",
            0.0,
            fmt(egui::FontId::monospace(11.0), ACID.gamma_multiply(0.55)),
        );
        if ui.selectable_label(false, job).clicked() {
            let all: Vec<Vibe> = Vibe::ALL.into_iter().filter(|&v| v != Vibe::Custom && v != vibe).collect();
            picked = Some(*self.rng.pick(&all));
        }
        if let Some(v) = picked {
            self.shuffle(v, now);
            self.bunk(format!("{} it is, bro. {}", v.name(), v.blurb()), now);
        }
    }

    /// Solari says something in the log
    fn bunk(&mut self, text: impl AsRef<str>, now: f64) {
        self.log.push(text.as_ref(), now);
    }

    /// A command typed into the terminal – Solari runs the auto mode, so it answers
    fn command(&mut self, line: &str, now: f64) {
        let line = line.trim().to_lowercase();
        if line.is_empty() {
            return;
        }
        self.log.push(format!("$ {line}"), now);
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        let arg = rest.join(" ");
        let pick = |i: u32, n: usize| (now as u32).wrapping_mul(2_654_435_761).wrapping_add(i) as usize % n;
        let find_vibe = |name: &str| {
            Vibe::ALL
                .into_iter()
                .find(|v| !name.is_empty() && v.name().to_lowercase().starts_with(name))
        };
        match first {
            "help" | "?" | "commands" => self.bunk(
                "Here's what I know, bro: shuffle · seed tom · vibe acid · vibes · drop · fx on/off · every 8 · mood dark · bpm 135 · status · joke · off",
                now,
            ),
            "shuffle" | "roll" | "again" | "next" | "new" => {
                let n = self.dice_all();
                if n == 0 {
                    self.bunk("Every track is off or locked, bro. Nothing to roll.", now);
                    return;
                }
                const QUIPS: [&str; 4] = [
                    "Rolled it, bro. Smells like fresh techno.",
                    "New groove, same orbit. Let's get it.",
                    "Shook the dice, brother. Four to the floor, no cap.",
                    "There you go, fam. Tell me if it slaps.",
                ];
                self.bunk(QUIPS[pick(1, QUIPS.len())], now);
            }
            "seed" => {
                // A word of your own, or one Solari picks so you can come back to it
                let word = if arg.is_empty() {
                    const WORDS: [&str; 8] = ["nebula", "pulsar", "comet", "orbit", "quasar", "aurora", "meteor", "eclipse"];
                    format!("{}-{}", WORDS[pick(3, WORDS.len())], pick(4, 90) + 10)
                } else {
                    arg.clone()
                };
                self.seeded(&word, now);
                self.bunk(
                    format!(
                        "Seed \"{word}\" in {}. Same seed, same vibe – same beat, any time, bro.",
                        self.song.vibe.name()
                    ),
                    now,
                );
            }
            "vibes" | "list" => {
                let names: Vec<&str> = Vibe::ALL.iter().map(|v| v.name()).collect();
                self.bunk(names.join(" · "), now);
            }
            "vibe" | "play" | "style" => match find_vibe(&arg) {
                Some(v) => {
                    self.shuffle(v, now);
                    self.bunk(format!("{} it is, bro. {}", v.name(), v.blurb()), now);
                }
                None => self.bunk("Which one, bro? Type vibes for the list.", now),
            },
            "drop" | "boom" => {
                if self.playing() {
                    self.drop_now(now);
                    self.bunk("Hold on to something, brother. Here it comes.", now);
                } else {
                    self.bunk("Press play first, fam – I can't drop silence.", now);
                }
            }
            "fx" => {
                let on = match arg.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => !self.song.auto_fx,
                };
                self.set_auto_fx(on, now);
            }
            "every" | "change" => match arg.trim_end_matches(" bars").parse::<u8>() {
                Ok(n) if (1..=16).contains(&n) => {
                    self.song.auto_bars = n;
                    self.bunk(
                        if n == 1 {
                            "Something new every bar. Busy night, bro.".to_string()
                        } else {
                            format!("Say less. Something new every {n} bars.")
                        },
                        now,
                    );
                }
                _ => self.bunk("Every how many bars, bro? 1 to 16.", now),
            },
            "mood" => match MOODS.iter().position(|m| !arg.is_empty() && m.0.to_lowercase().starts_with(&arg)) {
                Some(k) => {
                    self.song.set_mood(k);
                    self.song.mood_hold = true;
                    gen::refit_notes(&mut self.song, &mut self.rng);
                    self.bunk(format!("{}. {}", MOODS[k].0, MOODS[k].1), now);
                }
                None if arg == "auto" => {
                    self.song.mood_hold = false;
                    self.bunk("Bet. I'll pick the mood with every shuffle.", now);
                }
                None => {
                    let names: Vec<&str> = MOODS.iter().map(|m| m.0).collect();
                    self.bunk(format!("Moods: {} – or auto", names.join(" · ")), now);
                }
            },
            "bpm" | "tempo" => match arg.parse::<f32>() {
                Ok(b) => {
                    self.song.bpm = b.clamp(90.0, 180.0);
                    self.bunk(format!("{:.0} BPM.", self.song.bpm), now);
                }
                Err(_) => self.bunk(format!("We're at {:.0} BPM, bro. Say bpm 135 to change it.", self.song.bpm), now),
            },
            "status" | "where" => {
                let mood = self.song.mood().map_or("", |k| MOODS[k].0);
                let text = format!(
                    "{} · {} · {mood} · {:.0} BPM · energy {:.0}",
                    self.song.vibe.name(),
                    self.song.root_name(),
                    self.song.bpm,
                    self.song.macros.energy * 100.0
                );
                self.bunk(text, now);
            }
            "joke" => {
                const JOKES: [&str; 4] = [
                    "Why did the kick quit, bro? It kept getting beat.",
                    "How many techno heads to change a bulb? One. And again. And again. And again.",
                    "Asked the hi-hat to chill. It said: tss. Disrespectful.",
                    "My favourite chord? Same as yesterday, brother. It's techno.",
                ];
                self.bunk(JOKES[pick(2, JOKES.len())], now);
            }
            "hi" | "hello" | "hey" | "solari" | "bunk" | "who" => self.bunk("Yo bro! I'm Solari, I run the set down here. Type help if you're lost.", now),
            "off" | "exit" | "bye" | "quit" | "shutdown" => {
                self.bunk("Peace out, brother. Lights out.", now);
                self.power(now);
            }
            _ => match find_vibe(first) {
                // A vibe's name alone works too
                Some(v) => {
                    self.shuffle(v, now);
                    self.bunk(format!("{} it is, bro. {}", v.name(), v.blurb()), now);
                }
                None => self.bunk("Didn't catch that, bro. I only speak techno – try help.", now),
            },
        }
    }

    /// AUTO FX on or off (the gear, or `fx` in the terminal)
    fn set_auto_fx(&mut self, on: bool, now: f64) {
        if self.song.auto_fx == on {
            return;
        }
        self.song.auto_fx = on;
        if !on && self.song.auto {
            // The bar decides now: everything Solarum took out comes back
            for t in self.song.tracks.iter_mut().filter(|t| !t.lock) {
                t.mute = false;
            }
        }
        self.say(
            if on {
                "Auto FX on – Solarum plays breaks and drops by itself"
            } else {
                "Auto FX off – the energy bar is yours: low = break, 85 and up = everything, DROP drops"
            },
            now,
        );
    }

    /// The computer's power switch: the auto mode on or off (or a running song stopped)
    fn power(&mut self, now: f64) {
        self.power_at = now;
        if self.song_form.is_some() {
            self.stop_song(now);
        } else {
            self.toggle_auto(now);
            if self.song.auto {
                let hello = crate::log::HELLOS[(now as usize) % crate::log::HELLOS.len()];
                self.bunk(hello, now);
                self.start_growing(now);
                // Solari takes over: the music starts right away
                if !self.playing() {
                    self.toggle_play();
                }
            }
        }
    }

    /// The auto mode comes on: Solari keeps what's there and grows the rest – every empty track
    /// comes in on its own bar (an empty sky builds up from the kick)
    fn start_growing(&mut self, now: f64) {
        let empty = self.song.tracks.iter().all(|t| t.hits() == 0);
        if self.song.vibe == Vibe::Custom {
            let vibe = Vibe::ALL[(now as usize).wrapping_mul(7) % Vibe::ALL.len()];
            self.song.vibe = vibe;
            if empty {
                let (lo, hi) = vibe.bpm();
                self.song.bpm = ((lo + hi) / 2.0).round();
            }
        }
        // Neutral is only the start: the auto mode picks a mood of the vibe and changes it later
        if self.song.mood() == Some(crate::song::NEUTRAL) {
            let moods = gen::moods_for(self.song.vibe);
            self.song.set_mood(moods[(now as usize).wrapping_mul(13) % moods.len()]);
            self.song.mood_hold = false;
            gen::refit_notes(&mut self.song, &mut self.rng);
        }
        let target = gen::grow_target(&self.song, &mut self.rng);
        if target.tracks == self.song.tracks {
            self.bunk("This beat is lowkey fire, bro. I'll keep it moving.", now);
            return;
        }
        self.send(Cmd::Grow(Box::new(target)));
        let vibe = self.song.vibe.name().to_lowercase();
        self.bunk(
            if empty {
                format!("Empty sky, fam. I'll cook something {vibe}, one track per bar.")
            } else {
                "Your beat stays, brother. I'll fill in the rest, one track per bar.".to_string()
            },
            now,
        );
    }

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

    /// The tempo box: drag or scroll for the tempo, click for the menu – tempo, metronome
    /// and the mood. It shows the mood under the BPM, and a dot while the metronome ticks.
    fn bpm_box(&mut self, ui: &mut Ui, now: f64) {
        let (rect, resp) = ui.allocate_exact_size(vec2(176.0, 42.0), Sense::click_and_drag());
        if resp.dragged() {
            let d = resp.drag_delta();
            self.song.bpm = (self.song.bpm + (d.x - d.y) * 0.15).clamp(90.0, 180.0);
        }
        if resp.hovered() {
            let s = ui.input(|i| i.raw_scroll_delta.y);
            let n = orbit::scroll_notches(&mut self.bpm_scroll, s, 24.0);
            if n != 0 {
                self.song.bpm = (self.song.bpm.round() + n as f32).clamp(90.0, 180.0);
            }
        }
        if !resp.dragged() {
            self.song.bpm = self.song.bpm.round();
        }
        let mood = self.song.mood().map_or("–", |k| MOODS[k].0);
        {
            let p = ui.painter();
            let hot = resp.hovered() || resp.dragged();
            p.rect_filled(rect, 10.0, if hot { PANEL_HI } else { PANEL });
            p.rect_stroke(rect, 10.0, stroke(1.0, LINE), egui::StrokeKind::Inside);
            p.text(
                rect.left_center() + vec2(12.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{:.0}", self.song.bpm),
                display(26.0),
                TEXT,
            );
            let x = rect.left() + 62.0;
            p.text(pos2(x, rect.top() + 9.0), Align2::LEFT_TOP, "BPM", bold(10.0), DIM);
            let label = if self.song.mood_hold {
                mood.to_string()
            } else {
                format!("{mood} · auto")
            };
            let label = fit_text(p, &label, bold(11.5), rect.right() - 34.0 - x);
            p.text(pos2(x, rect.bottom() - 8.0), Align2::LEFT_BOTTOM, label, bold(11.5), ACID);
            if self.metronome {
                let beat = (self.shared.step.load(Ordering::Relaxed) / 4) % 4;
                let on = self.playing();
                p.circle_filled(pos2(x + 34.0, rect.top() + 15.0), 3.0, if on && beat == 0 { ACID } else { DIM });
            }
            // The arrow in its own corner, like the record button's
            let split = rect.right() - 28.0;
            p.line_segment([pos2(split, rect.top() + 9.0), pos2(split, rect.bottom() - 9.0)], stroke(1.0, LINE));
            p.text(
                pos2(split + 14.0, rect.center().y),
                Align2::CENTER_CENTER,
                ph::CARET_DOWN,
                body(12.0),
                DIM,
            );
        }
        let resp = resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Tempo, metronome and mood – click for the menu, drag or scroll for the tempo (↑/↓)");
        let popup = ui.make_persistent_id("tempo_mood");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            self.tempo_menu(ui, now)
        });
    }

    /// The tempo box's menu: the tempo, the metronome, and the moods as tiles with their chords
    fn tempo_menu(&mut self, ui: &mut Ui, now: f64) {
        ui.set_width(372.0);
        let label = |ui: &mut Ui, t: &str| {
            ui.label(egui::RichText::new(t).font(bold(10.5)).color(FAINT).extra_letter_spacing(1.2));
        };
        label(ui, "TEMPO");
        // (the number is in the box above – here just the controls: − slider +, metronome)
        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("−").font(bold(14.0)))
                .on_hover_text("1 BPM slower")
                .clicked()
            {
                self.song.bpm = (self.song.bpm - 1.0).max(90.0);
            }
            ui.spacing_mut().slider_width = 220.0;
            ui.add(egui::Slider::new(&mut self.song.bpm, 90.0..=180.0).show_value(false).step_by(1.0));
            if ui
                .button(egui::RichText::new("+").font(bold(14.0)))
                .on_hover_text("1 BPM faster")
                .clicked()
            {
                self.song.bpm = (self.song.bpm + 1.0).min(180.0);
            }
            let on = self.metronome;
            if chip(ui, ph::METRONOME, on, ACID)
                .on_hover_text(if on {
                    "Metronome on – click to stop it"
                } else {
                    "Metronome: a click on every beat, higher on the one"
                })
                .clicked()
            {
                self.metronome = !on;
                self.send(Cmd::Metronome(!on));
            }
        });
        ui.add_space(6.0);
        label(ui, "MOOD");
        // Neutral, Auto, then the moods: each a tile with its four chords as steps
        let mut order: Vec<Option<usize>> = vec![Some(crate::song::NEUTRAL), None];
        order.extend((0..crate::song::NEUTRAL).map(Some));
        let current = self.song.mood();
        let mut picked = None;
        for row in order.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for &m in row {
                    let (r, resp) = ui.allocate_exact_size(vec2(116.0, 50.0), Sense::click());
                    let on = match m {
                        None => !self.song.mood_hold,
                        Some(k) => self.song.mood_hold && current == Some(k),
                    };
                    let hot = resp.hovered();
                    let p = ui.painter();
                    p.rect_filled(
                        r,
                        8.0,
                        if on {
                            mix(PANEL, ACID, 0.2)
                        } else if hot {
                            PANEL_HI
                        } else {
                            mix(PANEL, PANEL_HI, 0.4)
                        },
                    );
                    p.rect_stroke(r, 8.0, stroke(1.0, if on { ACID } else { LINE }), egui::StrokeKind::Inside);
                    let (name, tip) = match m {
                        None => (
                            "Auto",
                            "The vibe picks a fitting mood with every shuffle, the auto mode changes it now and then",
                        ),
                        Some(k) => (MOODS[k].0, MOODS[k].1),
                    };
                    p.text(
                        r.left_top() + vec2(9.0, 7.0),
                        Align2::LEFT_TOP,
                        name,
                        bold(12.5),
                        if on { ACID } else { TEXT },
                    );
                    let ink = if on { ACID } else { mix(PANEL_HI, TEXT, 0.45) };
                    match m {
                        // Auto: a little dice of moods
                        None => {
                            p.text(r.left_bottom() + vec2(9.0, -6.0), Align2::LEFT_BOTTOM, ph::SHUFFLE, body(15.0), ink);
                        }
                        Some(k) => {
                            let prog = crate::song::PROGRESSIONS[MOODS[k].3 as usize % crate::song::PROGRESSIONS.len()].1;
                            for (j, d) in prog.iter().enumerate() {
                                let h = 4.0 + (*d + 2) as f32 * 2.2;
                                let x = r.left() + 9.0 + j as f32 * 11.0;
                                p.rect_filled(
                                    Rect::from_min_max(pos2(x, r.bottom() - 7.0 - h), pos2(x + 8.0, r.bottom() - 7.0)),
                                    1.5,
                                    ink,
                                );
                            }
                            p.text(
                                r.right_bottom() - vec2(8.0, 6.0),
                                Align2::RIGHT_BOTTOM,
                                MOODS[k].2.name(),
                                body(10.0),
                                DIM,
                            );
                        }
                    }
                    if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        picked = Some(m);
                    }
                }
            });
        }
        match picked {
            Some(None) => {
                self.song.mood_hold = false;
                self.say("Mood: the vibe picks it", now);
            }
            Some(Some(k)) => {
                self.song.mood_hold = true;
                if current != Some(k) {
                    self.song.set_mood(k);
                    if self.song.auto {
                        gen::refit_notes(&mut self.song, &mut self.rng);
                        // The synth carries the mood – a mood with it muted would hardly be heard
                        let synth = self.song.track_mut(Voice::Acid);
                        if !synth.lock && k != crate::song::NEUTRAL {
                            synth.mute = false;
                        }
                    } else {
                        // By hand your lines and mutes stay: every note just moves into the new scale
                        gen::snap_notes(&mut self.song);
                    }
                }
                self.say(format!("Mood: {}", MOODS[k].0), now);
            }
            None => {}
        }
    }

    fn tracks_panel(&mut self, ui: &mut Ui, live: &Live, now: f64) {
        let w = ui.available_width();
        // The rows leave room for the log below the mood
        let reserve = 360.0;
        let row_h = ((ui.available_height() - reserve) / TRACKS as f32).clamp(40.0, 54.0);
        for (i, voice) in Voice::ALL.into_iter().enumerate() {
            let color = track_color(voice);
            let (row, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
            let soloed = self.solo.is_some_and(|(t, _)| t == i);
            let flash = live.flash(i);
            let muted = self.song.tracks[i].mute;
            let locked = self.song.tracks[i].lock;
            let p = ui.painter().clone();
            if resp.hovered() {
                p.rect_filled(row, 10.0, PANEL_HI);
            }
            let top = row.top() + (row_h - 42.0) / 2.0;
            let dot = pos2(row.left() + 11.0, top + 12.0);
            glow(&p, dot, 5.0, color, if muted { 0.0 } else { 0.3 + flash });
            p.circle_filled(dot, 5.0, if muted { FAINT } else { color });
            let name_rect = p.text(
                pos2(row.left() + 24.0, top + 3.0),
                Align2::LEFT_TOP,
                voice.name(),
                bold(14.0),
                if soloed {
                    ACID
                } else if muted {
                    FAINT
                } else {
                    TEXT
                },
            );
            let name_hit = Rect::from_min_max(row.left_top() + vec2(0.0, top - row.top()), name_rect.right_bottom());
            let name_resp = ui
                .allocate_rect(name_hit, Sense::click())
                .on_hover_text(format!("{}\nClick to hear it · right-click for more", voice.blurb()))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if name_resp.clicked() {
                self.hit(voice);
            }
            name_resp.context_menu(|ui| self.track_menu(ui, i, now));
            // Buttons on the right: solo, mute, dice, lock
            let mut x = row.right() - 2.0;
            let mut icon_btn = |ui: &mut Ui, icon: &str, tip: &str, on: bool, col: Color32| -> egui::Response {
                let r = Rect::from_min_size(pos2(x - 25.0, top), vec2(25.0, 22.0));
                x -= 26.0;
                let resp = ui.allocate_rect(r, Sense::click()).on_hover_text(tip);
                let hot = resp.hovered();
                if on {
                    ui.painter().rect_filled(r, 7.0, mix(PANEL, col, 0.22));
                } else if hot {
                    ui.painter().rect_filled(r, 7.0, mix(PANEL_HI, Color32::WHITE, 0.06));
                }
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    icon,
                    if icon.len() == 1 { bold(12.5) } else { body(15.0) },
                    if on {
                        col
                    } else if hot {
                        TEXT
                    } else {
                        DIM
                    },
                );
                resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            };
            let lock = icon_btn(
                ui,
                if locked { ph::LOCK_SIMPLE } else { ph::LOCK_SIMPLE_OPEN },
                if locked {
                    "Locked: shuffle, vibes, the dice and the auto mode leave this track alone.\nYou can still edit it by hand. Click to unlock"
                } else {
                    "Lock this track: shuffle, vibes, the dice and the auto mode will leave it alone"
                },
                locked,
                TEXT,
            );
            if lock.clicked() {
                self.song.tracks[i].lock = !locked;
                self.say(
                    format!("{} {}", voice.name(), if locked { "unlocked" } else { "locked – stays as it is" }),
                    now,
                );
            }
            let dice_tip = if locked {
                "Locked – unlock it to roll a new rhythm".to_string()
            } else {
                format!(
                    "New {} rhythm\nRolls a fresh pattern for this track only – the sound stays.\nRight-click: clear all steps",
                    voice.name().to_lowercase()
                )
            };
            let dice = icon_btn(ui, if locked { "" } else { ph::DICE_FIVE }, &dice_tip, false, ACID);
            if locked {
                p.text(
                    dice.rect.center(),
                    Align2::CENTER_CENTER,
                    ph::DICE_FIVE,
                    body(15.0),
                    alpha(DIM, 0.3),
                );
            }
            if !locked && dice.secondary_clicked() {
                for st in self.song.tracks[i].steps.iter_mut() {
                    *st = crate::song::Step::default();
                }
                self.say(format!("{} cleared", voice.name()), now);
            }
            if !locked && dice.clicked() {
                self.roll_track(i);
                self.song.tracks[i].mute = false;
                self.say(format!("New {} rhythm", voice.name().to_lowercase()), now);
            }
            if icon_btn(
                ui,
                if muted { ph::SPEAKER_SLASH } else { ph::SPEAKER_HIGH },
                &format!("Mute ({})", i + 1),
                muted,
                RED,
            )
            .clicked()
            {
                self.song.tracks[i].mute = !muted;
            }
            if icon_btn(ui, "S", "Solo: hear only this track (click again to end)", soloed, ACID).clicked() {
                self.toggle_solo(i, now);
            }

            // Second line: sound, volume
            let y = top + 23.0;
            let sound_rect = Rect::from_min_size(pos2(row.left() + 22.0, y), vec2(84.0, 18.0));
            let sound = ui.allocate_rect(sound_rect, Sense::click());
            let hot = sound.hovered();
            p.rect_filled(sound_rect, 9.0, mix(PANEL, color, if hot { 0.3 } else { 0.18 }));
            p.text(
                sound_rect.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                crate::widgets::fit_text(&p, &self.sound_label(i), bold(11.0), sound_rect.width() - 24.0),
                bold(11.0),
                if muted { DIM } else { mix(color, TEXT, 0.35) },
            );
            p.text(
                sound_rect.right_center() - vec2(7.0, 0.0),
                Align2::RIGHT_CENTER,
                ph::CARET_DOWN,
                body(10.0),
                DIM,
            );
            let popup = ui.make_persistent_id(("sound", i));
            if sound.hovered() {
                let s = ui.input(|inp| inp.raw_scroll_delta.y);
                let n = orbit::scroll_notches(&mut self.sound_scroll, s, 24.0);
                if n != 0 {
                    let count = voice.sounds().len() as i32;
                    let next = (self.song.tracks[i].sound as i32 - n.signum()).rem_euclid(count);
                    self.set_sound(i, next as u8, now);
                }
            }
            let sound = sound
                .on_hover_text("Sound: click for the list, scroll to flip through")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if sound.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup));
            }
            let mut picked = None;
            let mut picked_sample = None;
            egui::popup_below_widget(ui, popup, &sound, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                ui.set_min_width(170.0);
                ui.label(
                    egui::RichText::new(format!("{} SOUND", voice.name().to_uppercase()))
                        .font(bold(10.5))
                        .color(FAINT),
                );
                for (k, name) in voice.sounds().iter().enumerate() {
                    if ui
                        .selectable_label(self.song.tracks[i].sound as usize == k, egui::RichText::new(*name).font(bold(13.0)))
                        .clicked()
                    {
                        picked = Some(k as u8);
                    }
                }
                // Yours: every library sound of this track (picking one gives it a project slot)
                let mine: Vec<(u64, String)> = self
                    .library
                    .entries
                    .iter()
                    .filter(|e| e.voice == voice && !e.draft)
                    .map(|e| (e.id, e.name.clone()))
                    .collect();
                if !mine.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("YOUR SOUNDS").font(bold(10.5)).color(FAINT));
                    let now_id = slot_of(self.song.tracks[i].sound).map(|k| self.sample_slots[k]);
                    for (id, name) in mine {
                        let text = egui::RichText::new(format!("{}  {name}", ph::WAVEFORM)).font(bold(13.0));
                        if ui.selectable_label(now_id == Some(id), text).clicked() {
                            picked_sample = Some(id);
                        }
                    }
                }
            });
            if let Some(k) = picked {
                self.set_sound(i, k, now);
            }
            if let Some(id) = picked_sample {
                if let Some(sound) = self.use_sample(id, now) {
                    self.set_sound(i, sound, now);
                }
            }
            // Shaped sounds show it on the pill
            let t = &self.song.tracks[i];
            if t.tune != 0 || (t.decay - 0.5).abs() > 0.02 {
                p.circle_filled(sound_rect.right_top() + vec2(-3.0, 3.0), 3.0, ACID);
            }
            // Volume
            let vx = sound_rect.right() + 8.0;
            let vr = Rect::from_min_max(pos2(vx, y + 2.0), pos2(row.right() - 6.0, y + 16.0));
            let mut vol = self.song.tracks[i].volume;
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(vr), |ui| {
                widgets::bar_slider(ui, &mut vol, vr.width(), color).on_hover_text("Volume");
            });
            self.song.tracks[i].volume = vol;
        }

        ui.add_space(16.0);
        let energy = (self.song.auto && self.song.auto_fx && self.song_form.is_none()).then_some(self.song.macros.energy);
        self.computer(ui, energy, now);
    }

    /// The auto mode's settings, from SET in SOLAR OS: how often it changes
    /// something, and AUTO FX
    fn auto_settings(&mut self, ui: &mut Ui, now: f64) {
        // SOLAR OS green, like the VIBE menu
        ui.set_min_width(260.0);
        let mono = egui::FontId::monospace;
        let bars = self.song.auto_bars;
        let title = if bars == 1 {
            "Change something every bar".to_string()
        } else {
            format!("Change something every {bars} bars")
        };
        ui.label(egui::RichText::new(title).font(mono(12.0)).color(ACID));
        ui.add_space(2.0);
        let all: Vec<u8> = (1..=16).collect();
        if let Some(b) = widgets::stops(ui, &all, bars, 250.0, ACID) {
            self.song.auto_bars = b;
        }
        ui.add_space(4.0);
        let fx = self.song.auto_fx;
        let mut job = egui::text::LayoutJob::default();
        let fmt = |font, color| egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        };
        job.append(&format!("> AUTO FX {}", if fx { "on" } else { "off" }), 0.0, fmt(mono(13.0), ACID));
        job.append(
            "   breaks, build-ups and drops by itself",
            0.0,
            fmt(mono(11.0), ACID.gamma_multiply(0.55)),
        );
        let toggled = ui.selectable_label(fx, job);
        if toggled.clicked() {
            self.song.auto_fx = !fx;
        }
        if toggled
            .on_hover_text(
                "On: Solarum arranges by itself – the energy glides, breakdowns, build-ups with effects \
                 (low-pass, gate, echo) and drops. The energy shows in the log.\n\
                 Off: the energy bar is yours – low = break, 85 and up = everything, DROP drops. The auto mode \
                 still changes patterns, sounds and notes, but never takes a track out.",
            )
            .clicked()
        {
            if fx && self.song.auto {
                // The bar decides now: everything Solarum took out comes back
                for t in self.song.tracks.iter_mut().filter(|t| !t.lock) {
                    t.mute = false;
                }
            }
            self.say(
                if fx {
                    "Auto FX off – the energy bar is yours: low = break, 85 and up = everything, DROP drops"
                } else {
                    "Auto FX on – Solarum plays breaks and drops by itself"
                },
                now,
            );
        }
    }

    /// The auto mode is the computer: SOLAR OS, the terminal under the mood. Off, its screen
    /// is dark with a power button; switched on it boots, Solari says hello, and it's a little
    /// operating system: a title bar (SET for the settings, OFF), a menu (the vibe, SHUFFLE),
    /// the log, and a prompt Solari answers.
    fn computer(&mut self, ui: &mut Ui, energy: Option<f32>, now: f64) {
        let on = self.song.auto || self.song_form.is_some();
        let w = ui.available_width();
        let mut switch = false;
        // Set into the concrete: a dark well, a thick bezel with four screws, the screen inside
        let h_all = ui.available_height().max(if on { 160.0 } else { 80.0 });
        let (outer, _) = ui.allocate_exact_size(vec2(w, h_all), Sense::hover());
        widgets::bezel_frame(ui.painter(), outer, 13.0, 4.0);
        let screen = outer.shrink(6.0);
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(screen));
        let ui = &mut inner;
        let w = screen.width();
        if on {
            let (chrome, prompt) = (44.0, 24.0);
            let h = screen.height();
            let rect = self.log.ui(ui, self.log_state(), energy, h, chrome, prompt, now);
            switch |= self.os_chrome(ui, rect, now);
            self.os_prompt(ui, rect, now);
            // Logged in: Solari, the mascot, waves and says hello, then the log takes over
            let since = (now - self.power_at) as f32;
            if (0.0..2.0).contains(&since) {
                let p = ui.painter_at(rect);
                let fade = (1.0 - (since - 1.6).max(0.0) / 0.4).clamp(0.0, 1.0);
                p.rect_filled(rect, 10.0, mix(BG, Color32::BLACK, 0.55).gamma_multiply(fade));
                let size = (rect.height() * 0.32).min(64.0);
                let c = rect.center() - vec2(0.0, size * 0.45);
                crate::log::robot(&p, c, size, since, ACID.gamma_multiply(fade));
                let hello = crate::log::HELLOS[(self.power_at as usize) % crate::log::HELLOS.len()];
                let typed: String = hello.chars().take(((since - 0.3).max(0.0) * 50.0) as usize).collect();
                p.text(
                    pos2(rect.center().x, c.y + size * 1.05),
                    Align2::CENTER_TOP,
                    typed,
                    egui::FontId::monospace(11.0),
                    ACID.gamma_multiply(fade),
                );
                ui.ctx().request_repaint();
            }
            // Booting: a line of light opens into the picture
            let k = ((now - self.power_at) / 0.3) as f32;
            if k < 1.0 {
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 10.0, Color32::from_black_alpha((255.0 * (1.0 - k)) as u8));
                let h = (rect.height() * k * k).max(2.0);
                let band = Rect::from_center_size(rect.center(), vec2(rect.width() * (0.3 + 0.7 * k.sqrt()), h));
                p.rect_filled(band, 0.0, ACID.gamma_multiply(0.5 * (1.0 - k)));
                ui.ctx().request_repaint();
            }
        } else {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, screen.height()), Sense::click());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 10.0, Color32::from_black_alpha(130));
            p.rect_stroke(rect, 10.0, stroke(1.0, mix(PANEL, ACID, 0.18)), egui::StrokeKind::Inside);
            // The glass: a faint diagonal sheen
            let sheen = vec![
                rect.left_top() + vec2(rect.width() * 0.62, 0.0),
                rect.left_top() + vec2(rect.width() * 0.74, 0.0),
                rect.left_top() + vec2(rect.width() * 0.34, rect.height()),
                rect.left_top() + vec2(rect.width() * 0.22, rect.height()),
            ];
            p.add(egui::Shape::convex_polygon(sheen, Color32::from_white_alpha(1), egui::Stroke::NONE));
            // Switching off: the picture collapses into a line, then a dot
            let k = ((now - self.power_at) / 0.4) as f32;
            if k < 1.0 {
                let w = rect.width() * (1.0 - k).powi(2);
                p.rect_filled(
                    Rect::from_center_size(rect.center(), vec2(w.max(3.0), 2.0)),
                    1.0,
                    ACID.gamma_multiply(1.0 - k),
                );
                ui.ctx().request_repaint();
            } else {
                let hot = resp.hovered();
                let c = rect.center() - vec2(0.0, 12.0);
                if hot {
                    glow(&p, c, 22.0, ACID, 0.12);
                }
                p.circle_filled(c, 26.0, mix(PANEL, BG, 0.6));
                p.circle_stroke(c, 26.0, stroke(2.0, alpha(ACID, if hot { 0.75 } else { 0.55 })));
                p.text(
                    c,
                    Align2::CENTER_CENTER,
                    ph::POWER,
                    body(24.0),
                    alpha(ACID, if hot { 0.85 } else { 0.75 }),
                );
                p.text(
                    c + vec2(0.0, 40.0),
                    Align2::CENTER_CENTER,
                    "SOLAR OS",
                    bold(10.5),
                    if hot { TEXT } else { DIM },
                );
                p.text(
                    c + vec2(0.0, 55.0),
                    Align2::CENTER_CENTER,
                    "Switch on and Solarum plays a set",
                    body(10.5),
                    FAINT,
                );
            }
            switch |= resp
                .on_hover_text("Switch on the auto mode: Solarum plays a whole set by itself")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked();
        }
        if switch {
            self.power(now);
        }
    }

    /// SOLAR OS's window chrome: the title bar (SET, OFF) and the menu (VIBE, SHUFFLE).
    /// True when OFF was clicked.
    fn os_chrome(&mut self, ui: &mut Ui, rect: Rect, now: f64) -> bool {
        let p = ui.painter().clone();
        let mono = |s: f32| egui::FontId::monospace(s);
        let key = |ui: &mut Ui, r: Rect, id: &str, text: &str, strong: bool| {
            let resp = ui.interact(r, ui.id().with(id), Sense::click());
            let hot = resp.hovered();
            let fill = if hot {
                0.3
            } else if strong {
                0.14
            } else {
                0.0
            };
            if fill > 0.0 {
                p.rect_filled(r, 3.0, ACID.gamma_multiply(fill));
            }
            p.text(r.center(), Align2::CENTER_CENTER, text, mono(10.0), ACID);
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        };
        // Title bar
        let bar = Rect::from_min_max(rect.min + vec2(1.0, 1.0), pos2(rect.right() - 1.0, rect.top() + 20.0));
        p.rect_filled(
            bar,
            egui::CornerRadius {
                nw: 9,
                ne: 9,
                sw: 0,
                se: 0,
            },
            ACID.gamma_multiply(0.1),
        );
        let led = pos2(bar.left() + 10.0, bar.center().y);
        glow(&p, led, 3.5, ACID, 0.5);
        p.circle_filled(led, 3.0, ACID);
        let title = if self.song_form.is_some() { "SOLAR OS · SONG" } else { "SOLAR OS" };
        p.text(led + vec2(9.0, 0.0), Align2::LEFT_CENTER, title, mono(10.5), ACID);
        let off_r = Rect::from_min_max(
            pos2(bar.right() - 36.0, bar.top() + 2.0),
            pos2(bar.right() - 4.0, bar.bottom() - 2.0),
        );
        let set_r = Rect::from_min_max(pos2(off_r.left() - 36.0, off_r.top()), pos2(off_r.left() - 4.0, off_r.bottom()));
        let off = key(ui, off_r, "os_off", "OFF", false).on_hover_text("Switch the auto mode off (or type off)");
        let set = key(ui, set_r, "os_set", "SET", false).on_hover_text("Settings: how often Solari changes something, AUTO FX");
        let popup = ui.make_persistent_id("auto_bars");
        if set.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup_below_widget(ui, popup, &set, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            self.auto_settings(ui, now)
        });
        // Menu
        let row = Rect::from_min_max(
            pos2(rect.left() + 8.0, bar.bottom() + 4.0),
            pos2(rect.right() - 8.0, bar.bottom() + 20.0),
        );
        let vibe_r = Rect::from_min_size(row.min, vec2(row.width() * 0.62 - 3.0, row.height()));
        let shuf_r = Rect::from_min_max(pos2(vibe_r.right() + 6.0, row.top()), row.max);
        let vibe = key(ui, vibe_r, "os_vibe", "PICK A GENRE", true)
            .on_hover_text(format!("Now: {} – pick one and Solari rolls a groove in it", self.song.vibe.name()));
        // A little arrow: it opens a list
        let c = pos2(vibe_r.right() - 10.0, vibe_r.center().y);
        p.add(egui::Shape::convex_polygon(
            vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)],
            ACID,
            egui::Stroke::NONE,
        ));
        let shuf = key(ui, shuf_r, "os_shuffle", "SHUFFLE", true).on_hover_text("Rolls the dice on every track that's on");
        let popup = ui.make_persistent_id("term_vibes");
        if vibe.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup_below_widget(ui, popup, &vibe, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            self.vibe_list(ui, now)
        });
        if shuf.clicked() {
            self.command("shuffle", now);
        }
        off.clicked()
    }

    /// SOLAR OS's prompt, at the bottom of the screen: type to Solari
    fn os_prompt(&mut self, ui: &mut Ui, rect: Rect, now: f64) {
        let mono = egui::FontId::monospace(11.0);
        let line = Rect::from_min_max(
            pos2(rect.left() + 10.0, rect.bottom() - 24.0),
            pos2(rect.right() - 10.0, rect.bottom() - 6.0),
        );
        let p = ui.painter();
        p.line_segment(
            [line.left_top() - vec2(0.0, 2.0), line.right_top() - vec2(0.0, 2.0)],
            stroke(1.0, ACID.gamma_multiply(0.25)),
        );
        p.text(line.left_center(), Align2::LEFT_CENTER, "SOLAR>", mono.clone(), ACID);
        let field = Rect::from_min_max(pos2(line.left() + 44.0, line.top()), line.max);
        let mut sent = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(field), |ui| {
            // A calm cursor
            let cursor = &mut ui.style_mut().visuals.text_cursor;
            cursor.on_duration = 0.9;
            cursor.off_duration = 0.7;
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.cmd)
                    .frame(false)
                    .font(mono)
                    .text_color(ACID)
                    .hint_text(egui::RichText::new("type help").color(ACID.gamma_multiply(0.35)))
                    .desired_width(field.width()),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                sent = Some(std::mem::take(&mut self.cmd));
                resp.request_focus();
            }
            self.typing = resp.has_focus();
        });
        if let Some(line) = sent {
            self.command(&line, now);
        }
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
        let tip = if self.filter_lock {
            "Filter, held: drag to move it – it stays. Double-click (or Esc) lets it spring back"
        } else {
            "Filter: drag left for low-pass, right for high-pass, up for resonance – let go and it springs back.\nDouble-click: it stays where it is"
        };
        controls::filter_pad(
            ui,
            &mut self.filter,
            &mut self.filter_res,
            &mut self.filter_lock,
            engine_filter,
            w,
            filter_h,
        )
        .on_hover_text(tip);
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
                    let latched = self.fx_latch[i];
                    let active = self.fx_mouse[i] || self.fx_key[i] || latched;
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
                    // Locked on: a pin in the corner
                    if latched {
                        ui.painter()
                            .text(r.left_top() + vec2(8.0, 6.0), Align2::LEFT_TOP, ph::PUSH_PIN, bold(12.0), SPACE);
                    }
                    let since = (now - self.fx_pressed_at[i]) as f32;
                    if since < 0.5 {
                        controls::fx_burst(ui.painter(), r, fx_color(fx), since / 0.5);
                        ui.ctx().request_repaint();
                    }
                    let how = if latched {
                        "locked on – tap to let go"
                    } else {
                        "hold to play, tap to lock it on"
                    };
                    resp.on_hover_text(format!("{}\n{how} ({})", fx.blurb(), fx.key()))
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

    /// The strip under the orbit: twelve favorites in two rows, and AUTO SONG
    fn patterns_ui(&mut self, ui: &mut Ui, rect: Rect, live: &Live, now: f64) {
        let p = ui.painter().clone();
        // A star instead of a title – the tooltip tells the rest
        let left_w = 40.0;
        let lit = self.slot_now.is_some();
        p.text(
            pos2(rect.left() + (left_w - 8.0) / 2.0, rect.center().y),
            Align2::CENTER_CENTER,
            ph::STAR,
            body(22.0),
            if lit { ACID } else { FAINT },
        );
        let info = ui.allocate_rect(Rect::from_min_size(rect.min, vec2(left_w - 8.0, rect.height())), Sense::hover());
        info.on_hover_text(
            "Favorites: twelve memory slots for grooves.\n\
             • Click an empty one (+) to keep the groove you hear.\n\
             • Click a full one to switch to it – while playing it comes in on the next bar, with a clap roll.\n\
             • The lit one plays exactly as saved and keeps your changes; once the groove changes (auto mode) it goes dark.\n\
             • Right-click: save over it, rename or clear it.\n\
             • AUTO SONG (the arrow next to record) plays a whole track from favorite 1 (and favorite 2 after the first drop).",
        );

        let cards = Rect::from_min_max(pos2(rect.left() + left_w, rect.top()), pos2(rect.right() - 4.0, rect.bottom()));
        let (cols, gap) = (6, 6.0);
        let cw = (cards.width() - gap * (cols - 1) as f32) / cols as f32;
        let ch = (cards.height() - 8.0 - gap) / 2.0;
        for i in 0..SLOTS {
            let (row, col) = (i / cols, i % cols);
            let r = Rect::from_min_size(
                pos2(cards.left() + col as f32 * (cw + gap), cards.top() + 4.0 + row as f32 * (ch + gap)),
                vec2(cw, ch),
            );
            let resp = ui.allocate_rect(r, Sense::click());
            let filled = self.slots[i].is_some();
            let current = self.slot_now == Some(i);
            let queued = self.queued_slot == Some(i);
            let hot = resp.hovered();
            let num = (i + 1).to_string();
            let name = self.fav_name(i);
            if filled {
                if current && live.playing {
                    widgets::glow_rect(&p, r, 8.0, ACID, 0.04 + live.flash(0) * 0.08);
                }
                p.rect_filled(
                    r,
                    8.0,
                    // The same dark glass as the pads; the lit one only gets a lime line
                    if hot { mix(PANEL, PANEL_HI, 0.6) } else { mix(PANEL, BG, 0.4) },
                );
                // Waiting for the next bar: the border blinks
                let border = if queued {
                    alpha(ACID, 0.4 + 0.6 * ((now * 8.0).sin() * 0.5 + 0.5) as f32)
                } else if current {
                    ACID
                } else {
                    if hot {
                        alpha(TEXT, 0.35)
                    } else {
                        LINE
                    }
                };
                p.rect_stroke(
                    r,
                    8.0,
                    stroke(if current || queued { 1.5 } else { 1.0 }, border),
                    egui::StrokeKind::Inside,
                );
                let label = if queued { "next bar…".to_string() } else { name.clone() };
                let (num_col, name_col) = (if current { ACID } else { TEXT }, TEXT);
                // A tiny picture of the groove: its steps as grey dots, one row per track
                if cw >= 150.0 {
                    if let Some(song) = &self.slots[i] {
                        let (dx, dy) = (3.6, (r.height() - 14.0) / TRACKS as f32);
                        let x0 = r.right() - 10.0 - dx * 15.0;
                        for (t, track) in song.tracks.iter().enumerate() {
                            for (k, st) in track.steps.iter().enumerate() {
                                let at = pos2(x0 + k as f32 * dx, r.top() + 7.0 + (t as f32 + 0.5) * dy);
                                let c = if st.on {
                                    alpha(TEXT, if current { 0.75 } else { 0.55 })
                                } else {
                                    alpha(TEXT, 0.08)
                                };
                                p.circle_filled(at, 1.1, c);
                            }
                        }
                    }
                }
                if cw >= 110.0 {
                    // Wide: number and name side by side
                    p.text(r.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, &num, display(15.0), num_col);
                    let room = if cw >= 150.0 { cw - 34.0 - 3.6 * 15.0 - 18.0 } else { cw - 34.0 };
                    let label = fit_text(&p, &label, bold(11.0), room);
                    p.text(r.left_center() + vec2(28.0, 0.0), Align2::LEFT_CENTER, label, bold(11.0), name_col);
                } else {
                    // Narrow: number on top, the name below it with the whole width
                    p.text(r.left_top() + vec2(7.0, 4.0), Align2::LEFT_TOP, &num, display(13.0), num_col);
                    let label = fit_text(&p, &label, bold(10.0), cw - 12.0);
                    p.text(r.left_bottom() + vec2(7.0, -5.0), Align2::LEFT_BOTTOM, label, bold(10.0), name_col);
                }
            } else {
                p.rect_filled(r, 8.0, Color32::from_black_alpha(if hot { 35 } else { 25 }));
                p.rect_stroke(
                    r,
                    8.0,
                    stroke(1.0, if hot { alpha(TEXT, 0.3) } else { LINE }),
                    egui::StrokeKind::Inside,
                );
                p.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    ph::PLUS,
                    body(20.0),
                    if hot { TEXT } else { DIM },
                );
            }
            let tip = match (filled, current) {
                (false, _) => format!("Empty – click to keep the groove you hear now as favorite {num}"),
                (true, true) => {
                    format!("{num} · {name} is playing – your changes are kept in it.\nRight-click: save over it, rename or clear")
                }
                (true, false) => {
                    format!("{num} · {name}\nClick: switch to it (on the next bar)\nRight-click: save over it, rename or clear")
                }
            };
            let resp = resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                self.slot_clicked(i, false, now);
            }
            if filled {
                resp.context_menu(|ui| {
                    if !current && self.playing() && ui.button(format!("{}  Switch with a clap roll", ph::HANDS_CLAPPING)).clicked() {
                        self.slot_clicked(i, false, now);
                        self.send(Cmd::Fill(crate::engine::Fill::Roll));
                        ui.close_menu();
                    }
                    if ui.button(format!("{}  Save what you hear here", ph::FLOPPY_DISK)).clicked() {
                        self.slot_clicked(i, true, now);
                        ui.close_menu();
                    }
                    if ui.button(format!("{}  Rename…", ph::PENCIL_SIMPLE)).clicked() {
                        self.rename_buf = self.fav_name(i);
                        self.renaming = Some(i);
                        ui.close_menu();
                    }
                    if ui.button(format!("{}  Clear", ph::TRASH)).clicked() {
                        self.clear_slot(i, now);
                        ui.close_menu();
                    }
                });
            }
        }
    }

    /// AUTO SONG, from the arrow next to record: a whole track out of favorites 1 and 2,
    /// recorded as it plays
    fn song_menu(&mut self, ui: &mut Ui, resp: egui::Response, now: f64) {
        let on = self.song_form.is_some();
        let popup = ui.make_persistent_id("forms");
        let resp = resp
            .on_hover_text(if on {
                "AUTO SONG is playing – click to stop it"
            } else {
                "AUTO SONG: play (and record) a whole track from your favorites"
            })
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        let full = self.song_favorites().map(|s| s.is_some());
        egui::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(380.0);
            ui.label(
                egui::RichText::new("AUTO SONG")
                    .font(bold(13.0))
                    .color(TEXT)
                    .extra_letter_spacing(1.2),
            );
            ui.label(
                egui::RichText::new(
                    "A whole track, recorded as it plays. Favorite 1 starts – quietly, then track by track. The first drop comes early; favorite 2 takes over after it and favorite 1 returns for the last one (with one favorite, each drop is a new chapter of it). Solarum steers the energy, varies the grooves, breaks and drops – you can jam along.",
                )
                .font(body(11.0))
                .color(FAINT),
            );
            ui.add_space(4.0);
            if on {
                let stop = egui::RichText::new(format!("{}  Stop the song – the groove keeps playing", ph::STOP)).font(bold(13.0));
                if ui.selectable_label(false, stop).clicked() {
                    self.stop_song(now);
                }
                ui.separator();
            }
            for (k, f) in FORMS.iter().enumerate() {
                let mins = f.bars() as f32 * 4.0 / self.song.bpm;
                let parts: Vec<String> = f
                    .sections
                    .iter()
                    .map(|sec| {
                        let letter = crate::form::pattern_slot(sec.pattern, full).map_or("·".to_string(), |i| (i + 1).to_string());
                        format!("{} {letter}", sec.part.name())
                    })
                    .collect();
                let mut job = egui::text::LayoutJob::default();
                let fmt = |font, color| egui::TextFormat {
                    font_id: font,
                    color,
                    ..Default::default()
                };
                job.append(f.name, 0.0, fmt(bold(13.5), ACID));
                job.append(&format!("   ~{:.0} min · {}\n", mins.max(1.0), f.blurb), 0.0, fmt(body(11.0), DIM));
                job.append(&parts.join(" · "), 0.0, fmt(body(10.5), FAINT));
                if ui.selectable_label(self.song_form == Some(k), job).clicked() {
                    self.start_song(k, now);
                }
            }
        });
    }

    /// Song mode: the parts of the form, where it is, click to jump
    fn song_strip(&mut self, ui: &mut Ui, rect: Rect, live: &Live, now: f64) {
        let Some(form) = self.song_form.map(|f| &FORMS[f % FORMS.len()]) else {
            return;
        };
        let p = ui.painter().clone();
        let left_w = 96.0;
        p.text(rect.left_center() + vec2(4.0, -7.0), Align2::LEFT_CENTER, "SONG", bold(11.0), FAINT);
        p.text(
            rect.left_center() + vec2(4.0, 8.0),
            Align2::LEFT_CENTER,
            form.name,
            bold(12.0),
            ACID,
        );
        let full = self.song_favorites().map(|s| s.is_some());
        let area = Rect::from_min_max(pos2(rect.left() + left_w, rect.top()), rect.max);
        let gap = 3.0;
        let n = form.sections.len() as f32;
        let unit = (area.width() - gap * (n - 1.0)) / form.bars() as f32;
        let mut x = area.left();
        for (i, sec) in form.sections.iter().enumerate() {
            let w = unit * sec.bars as f32;
            let r = Rect::from_min_size(pos2(x, area.top()), vec2(w, area.height()));
            x += w + gap;
            let col = match sec.part {
                crate::form::Part::Intro | crate::form::Part::Outro => DIM,
                crate::form::Part::Groove => ACID,
                crate::form::Part::Break => Color32::from_rgb(69, 200, 255),
                crate::form::Part::Peak => RED,
            };
            let now_here = live.part.filter(|&(k, _)| k == i);
            let resp = ui.allocate_rect(r, Sense::click());
            let hot = resp.hovered();
            p.rect_filled(
                r,
                7.0,
                mix(
                    PANEL,
                    col,
                    if now_here.is_some() {
                        0.28
                    } else if hot {
                        0.2
                    } else {
                        0.12
                    },
                ),
            );
            if let Some((_, bars_in)) = now_here {
                let done = (bars_in as f32 / sec.bars as f32).clamp(0.0, 1.0);
                p.rect_filled(
                    Rect::from_min_size(r.min, vec2(r.width() * done, r.height())),
                    7.0,
                    mix(PANEL, col, 0.4),
                );
            }
            p.rect_stroke(
                r,
                7.0,
                stroke(
                    if now_here.is_some() { 1.5 } else { 1.0 },
                    alpha(col, if now_here.is_some() { 1.0 } else { 0.4 }),
                ),
                egui::StrokeKind::Inside,
            );
            let letter = crate::form::pattern_slot(sec.pattern, full).map_or(String::new(), |k| (k + 1).to_string());
            // The longest label that fits: "Peak C", "Peak", smaller "Peak", "P"
            let name = sec.part.name();
            let fits = |text: &str, size: f32| p.layout_no_wrap(text.into(), bold(size), TEXT).size().x + 10.0 <= w;
            let (label, size) = [
                (format!("{name} {letter}"), 11.0),
                (name.to_string(), 11.0),
                (name.to_string(), 9.0),
            ]
            .into_iter()
            .find(|(t, size)| fits(t, *size))
            .unwrap_or((name[..1].to_string(), 11.0));
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                label,
                bold(size),
                if now_here.is_some() { TEXT } else { alpha(col, 0.9) },
            );
            let tip = format!(
                "{} · {} bars{}\nClick: jump here on the next bar",
                sec.part.name(),
                sec.bars,
                if letter.is_empty() {
                    String::new()
                } else {
                    format!(" · pattern {letter}")
                }
            );
            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.send(Cmd::Jump(i));
                self.say(format!("{} on the next bar", sec.part.name()), now);
            }
        }
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
                    row(ui, ph::INFINITY, "8 · Auto mode", "The computer under the mood is the auto mode: its power button switches it on, Solari says hello and keeps the groove moving – something changes every 1 to 16 bars (SET in its title bar, where AUTO FX is too; OFF switches it off). Type to Solari: shuffle, vibe acid, drop, fx off, every 8, mood dark, bpm 135, status, joke, off. With AUTO FX on it also steers the energy and plays breakdowns, build-ups and drops by itself. With AUTO FX off the energy bar is yours and no track is ever taken out.");
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

        // MIDI keyboard or headphones plugged in or out? Check every few seconds
        if now > self.midi_scan_at && self.with_devices {
            self.midi_scan_at = now + 3.0;
            if MidiIn::port_count() != self.midi_ports {
                let c = ctx.clone();
                self.connect_midi(move || c.request_repaint());
            }
            let device = audio::default_device_name();
            if self.audio_failed.load(Ordering::Relaxed) || (device.is_some() && device != self.audio_device) {
                self.restart_audio(ctx, now);
            }
        }
        if self.with_devices {
            // Keep the 3-second checks running even when nothing moves
            ctx.request_repaint_after(std::time::Duration::from_secs(3));
        }

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
            concrete(&ctx.layer_painter(egui::LayerId::background()), ctx.screen_rect());
            let playhead = match self.shared.audition.load(Ordering::Relaxed) {
                u64::MAX => None,
                p => Some(p as f64),
            };
            let mut outs = Vec::new();
            egui::CentralPanel::default()
                .frame(frame(Color32::TRANSPARENT, 0.0))
                .show(ctx, |ui| {
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
        // Live effects: held with a key or the mouse. A tap locks the effect on until the next
        // tap; holding it while it's locked lets it go when you let go
        for i in 0..Fx::ALL.len() {
            let held = self.fx_key[i] || self.fx_mouse[i];
            if held && !self.fx_held[i] {
                self.fx_down_at[i] = now;
            } else if !held && self.fx_held[i] {
                let tap = now - self.fx_down_at[i] < FX_TAP;
                self.fx_latch[i] = tap && !self.fx_latch[i];
            }
            self.fx_held[i] = held;
            let on = held || self.fx_latch[i];
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
mod tests {
    use super::*;
    use crate::offscreen::Offscreen;

    fn frame(app: &mut SolarumApp, snap: &mut Offscreen, t: f64) {
        snap.frame(t, |ctx| app.ui(ctx));
    }

    #[test]
    fn undo_and_redo_a_shuffle() {
        let mut snap = Offscreen::new(1360.0, 860.0);
        install(&snap.ctx);
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        frame(&mut app, &mut snap, 1.0);
        let original = app.song;
        app.shuffle(Vibe::Industrial, 2.0);
        frame(&mut app, &mut snap, 2.0);
        let shuffled = app.song;
        assert_ne!(shuffled, original);

        app.undo(3.0);
        frame(&mut app, &mut snap, 3.0);
        assert_eq!(app.song.tracks, original.tracks);
        app.redo(4.0);
        frame(&mut app, &mut snap, 4.0);
        assert_eq!(app.song.tracks, shuffled.tracks);
        // Redo history survives the restore itself, and a new edit clears it
        app.undo(5.0);
        frame(&mut app, &mut snap, 5.0);
        assert_eq!(app.redo.len(), 1);
        app.song.tracks[0].mute = true;
        frame(&mut app, &mut snap, 7.0);
        assert!(app.redo.is_empty());
    }

    #[test]
    fn a_tap_locks_an_effect_and_a_hold_does_not() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        let press = |app: &mut SolarumApp, down: f64, up: f64| {
            app.fx_key[1] = true;
            app.finish_frame(down);
            assert!(app.fx_sent[1]);
            app.fx_key[1] = false;
            app.finish_frame(up);
        };
        // A tap: it stays on; the next tap lets it go
        press(&mut app, 1.0, 1.1);
        assert!(app.fx_latch[1] && app.fx_sent[1]);
        press(&mut app, 2.0, 2.1);
        assert!(!app.fx_latch[1] && !app.fx_sent[1]);
        // Held: only while held; held while locked: off when let go
        press(&mut app, 3.0, 4.0);
        assert!(!app.fx_sent[1]);
        press(&mut app, 5.0, 5.1);
        press(&mut app, 6.0, 7.0);
        assert!(!app.fx_latch[1] && !app.fx_sent[1]);
    }

    #[test]
    fn quick_edits_are_one_undo_step() {
        let mut snap = Offscreen::new(1360.0, 860.0);
        install(&snap.ctx);
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        frame(&mut app, &mut snap, 1.0);
        for (i, t) in [2.0, 2.1, 2.2, 2.3].into_iter().enumerate() {
            app.song.macros.energy = 0.1 * i as f32;
            frame(&mut app, &mut snap, t);
        }
        assert_eq!(app.undo.len(), 1);
    }

    #[test]
    fn slots_save_and_load() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        let first = app.song;
        app.slot_clicked(0, false, 0.0); // empty → save
        assert_eq!(app.slots[0], Some(first));
        app.shuffle(Vibe::Acid, 0.0);
        assert_eq!(app.slot_now, None, "a shuffle leaves the scene alone");
        app.song.master = 0.3;
        app.slot_clicked(0, false, 0.0); // full → load
        assert_eq!(app.song.tracks, first.tracks);
        assert_eq!(app.song.master, 0.3, "volume belongs to the moment");
    }

    #[test]
    fn the_lit_scene_keeps_changes() {
        let mut snap = Offscreen::new(1360.0, 860.0);
        install(&snap.ctx);
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        frame(&mut app, &mut snap, 1.0);
        app.slot_clicked(1, false, 1.0);
        frame(&mut app, &mut snap, 1.1);
        app.song.tracks[2].steps[5].on = !app.song.tracks[2].steps[5].on;
        frame(&mut app, &mut snap, 2.0);
        assert_eq!(app.slots[1].unwrap().tracks[2], app.song.tracks[2]);
        // Undo doesn't write into a scene
        app.undo(3.0);
        frame(&mut app, &mut snap, 3.0);
        assert_eq!(app.slot_now, None);
        assert_ne!(app.slots[1].unwrap().tracks[2], app.song.tracks[2]);
    }

    /// The app wired to a real engine (no sound card): the engine renders on demand
    fn with_engine() -> (SolarumApp, crate::engine::Engine, Receiver<Cmd>) {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        let (tx, rx) = channel();
        let (ev_tx, events) = channel();
        app.tx = tx;
        app.events = events;
        let engine = crate::engine::Engine::new(44_100.0, app.song, app.shared.clone(), Some(ev_tx), 1);
        (app, engine, rx)
    }

    #[test]
    fn the_first_start_is_hypnotic_and_waits_for_play() {
        let (mut app, mut e, rx) = with_engine();
        let ctx = egui::Context::default();
        let mut buf = vec![0.0; 2 * 44_100];
        e.drain(&rx);
        e.render(&mut buf);
        app.sync(&ctx, 0.0, 0.016);
        assert_eq!(app.song.vibe, Vibe::Hypnotic);
        assert!(!app.playing() && !app.song.auto, "silent until play or Space");
        assert!(buf.iter().all(|s| s.abs() < 1e-3), "not a sound before play");
    }

    #[test]
    fn a_set_recording_ends_in_the_save_dialog() {
        let (mut app, mut e, rx) = with_engine();
        let ctx = egui::Context::default();
        let mut buf = vec![0.0; 2 * 44_100];
        let mut tick = |app: &mut SolarumApp, e: &mut crate::engine::Engine, secs: usize| {
            for _ in 0..secs {
                e.drain(&rx);
                e.render(&mut buf);
                app.sync(&ctx, 0.0, 0.016);
            }
        };
        app.toggle_capture(0.0);
        app.toggle_play();
        tick(&mut app, &mut e, 3);
        app.toggle_play(); // stop the music, then the recording
        tick(&mut app, &mut e, 1);
        app.toggle_capture(0.0);
        tick(&mut app, &mut e, 3);
        assert!(matches!(app.export, Export::Recorded), "the save dialog is up");
        // Three seconds of music and two of tail – the second while it was stopped isn't in it
        let secs = app.recording.len() as f32 / 2.0 / app.rec_sr as f32;
        assert!((4.5..5.6).contains(&secs), "{secs} s recorded");
    }

    #[test]
    fn a_favorite_goes_dark_once_the_groove_changes() {
        let (mut app, mut e, rx) = with_engine();
        let ctx = egui::Context::default();
        app.slot_clicked(0, true, 0.0);
        app.finish_frame(0.0);
        e.drain(&rx);
        assert_eq!(app.slot_now, Some(0));
        // Only the energy moves: still the saved groove (a build-up makes the engine send its song)
        e.handle(Cmd::Play);
        e.song.macros.energy = 0.2;
        e.handle(Cmd::Drop);
        app.sync(&ctx, 0.1, 0.016);
        assert!(app.song.macros.energy < 0.3, "the engine's song came in");
        assert_eq!(app.slot_now, Some(0), "energy is not the groove");
        // The auto mode changes a pattern: it's not the saved groove any more
        e.handle(Cmd::Stop);
        e.song.tracks[2].steps[5].on = !e.song.tracks[2].steps[5].on;
        e.handle(Cmd::Play);
        e.handle(Cmd::Drop);
        app.sync(&ctx, 0.2, 0.016);
        assert_eq!(app.slot_now, None);
    }

    #[test]
    fn song_mode_and_auto_mode_take_turns() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        // A song needs no saved patterns – it plays what you hear
        app.song.auto = true;
        app.start_song(1, 0.0);
        assert_eq!(app.song_form, Some(1));
        assert!(!app.song.auto && app.song.curve);
        app.toggle_auto(0.0);
        assert_eq!(app.song_form, None);
        app.start_song(0, 0.0);
        app.stop_song(0.0);
        assert_eq!(app.song_form, None);
    }

    #[test]
    fn shuffle_rolls_only_the_tracks_that_are_on() {
        let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
        app.shuffle(Vibe::Peak, 0.0);
        app.song.tracks[1].mute = true;
        app.song.tracks[2].lock = true;
        let before = app.song;
        app.command("shuffle", 0.0);
        assert_eq!(app.song.tracks[1].steps, before.tracks[1].steps, "a muted track stays");
        assert_eq!(app.song.tracks[2].steps, before.tracks[2].steps, "a locked track stays");
        assert_eq!((app.song.vibe, app.song.root, app.song.bpm), (before.vibe, before.root, before.bpm));
        assert!((0..8).any(|i| app.song.tracks[i].steps != before.tracks[i].steps));
    }

    #[test]
    fn a_seed_always_rolls_the_same_beat() {
        let mut a = SolarumApp::headless(Song::default(), View::Planets, false);
        let mut b = SolarumApp::headless(Song::default(), View::Planets, false);
        b.shuffle(Vibe::Acid, 0.0);
        b.song.vibe = a.song.vibe;
        a.command("seed tom", 0.0);
        b.command("seed Tom", 1.0);
        assert_eq!(a.song.tracks.map(|t| (t.steps, t.sound)), b.song.tracks.map(|t| (t.steps, t.sound)));
        assert_eq!(a.song.bpm, b.song.bpm);
        a.command("seed anna", 0.0);
        assert_ne!(a.song.tracks.map(|t| t.steps), b.song.tracks.map(|t| t.steps));
    }

    #[test]
    fn new_synth_sound_gets_a_fitting_line() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        let i = Voice::Acid.index();
        app.song.tracks[i].sound = 0;
        let before = app.song.tracks[i].steps;
        app.set_sound(i, 1, 0.0); // 303 saw → square: same line
        assert_eq!(app.song.tracks[i].steps, before);
        app.set_sound(i, 2, 0.0); // → stab: a new line
        assert_eq!(app.song.tracks[i].sound, 2);
        assert!(app.song.tracks[i].hits() <= 6, "stabs are sparse");
    }

    #[test]
    fn bunk_runs_the_auto_mode_from_the_terminal() {
        let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
        // An empty sky: switching the computer on picks a vibe, folds the sound away and lets
        // the engine grow a groove (one track per bar – see the engine's tests)
        app.clear(0.0);
        app.sound_open = true;
        app.power(1.0);
        assert!(app.song.auto && !app.sound_open);
        assert_ne!(app.song.vibe, Vibe::Custom);
        assert!(app.log.lines().any(|l| l.contains("one track per bar")));
        app.command("vibe acid", 2.0);
        assert_eq!(app.song.vibe, Vibe::Acid);
        app.command("Hard", 3.0);
        assert_eq!(app.song.vibe, Vibe::Hard);
        app.command("every 4", 4.0);
        assert_eq!(app.song.auto_bars, 4);
        app.command("mood sun", 5.0);
        assert_eq!(app.song.mood(), Some(7));
        assert!(app.song.mood_hold);
        app.command("fx off", 6.0);
        assert!(!app.song.auto_fx);
        app.command("bpm 140", 7.0);
        assert_eq!(app.song.bpm, 140.0);
        app.command("what is this", 8.0);
        assert!(app.log.lines().last().is_some_and(|l| l.contains("try help")));
        app.command("off", 9.0);
        assert!(!app.song.auto && app.sound_open);
    }

    #[test]
    fn favorites_belong_to_their_project() {
        let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
        app.new_project();
        assert!(!app.dirty(), "an empty new project has nothing to save");
        app.slot_clicked(0, true, 0.0);
        app.names[0] = "Intro".into();
        assert!(app.dirty());
        // Leaving it asks first; "don't save" goes on to a new, empty project
        app.home_action(crate::home::Action::New, 1.0);
        assert!(app.pending.is_some() && app.slots[0].is_some(), "it waits for the answer");
        let action = app.pending.take().unwrap();
        app.mark_saved();
        app.home_action(action, 1.0);
        assert!(app.slots.iter().all(|s| s.is_none()) && app.names.iter().all(|n| n.is_empty()));
        assert!(app.project.is_none());
        // The start screen keeps what was there for Continue
        app.slot_clicked(2, true, 2.0);
        app.open_home();
        app.new_project();
        app.home_action(crate::home::Action::Continue, 3.0);
        assert!(app.slots[2].is_some(), "Continue brings the favorites back");
    }

    #[test]
    fn saved_and_opened_projects_lead_the_recent_list() {
        let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
        for name in ["a", "b", "a", "c"] {
            app.remember(std::path::Path::new(&format!("/tmp/{name}.solarum")));
        }
        let names: Vec<String> = app
            .recent
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["c", "a", "b"], "newest first, no doubles");
        assert_eq!(
            app.project.as_deref(),
            Some(std::path::Path::new("/tmp/c.solarum")),
            "Save writes there now"
        );
        for i in 0..20 {
            app.remember(std::path::Path::new(&format!("/tmp/{i}.solarum")));
        }
        assert_eq!(app.recent.len(), 12);
    }

    #[test]
    fn shuffle_keeps_a_mood_picked_by_hand() {
        let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
        app.song.set_mood(7);
        app.song.mood_hold = true;
        for _ in 0..5 {
            app.shuffle(Vibe::Hard, 0.0);
            assert_eq!(app.song.mood(), Some(7));
        }
        app.song.mood_hold = false;
        app.shuffle(Vibe::Hard, 0.0);
        assert!(app.song.mood().is_some_and(|k| gen::moods_for(Vibe::Hard).contains(&k)));
    }

    #[test]
    fn shuffle_keeps_locked_tracks() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        app.song.tracks[6].lock = true;
        app.song.tracks[6].tune = 3;
        let bass = app.song.tracks[6];
        for vibe in Vibe::ALL {
            app.shuffle(vibe, 0.0);
            assert_eq!(app.song.tracks[6], bass);
        }
        app.roll_track(6);
        assert_eq!(app.song.tracks[6], bass, "the dice leaves it alone too");
    }

    #[test]
    fn energy_by_hand_unless_auto_fx_steers_it() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        app.song.auto = true;
        app.song.auto_fx = true;
        let before = app.song.macros.energy;
        app.set_energy(0.2, 0.0);
        assert_eq!(app.song.macros.energy, before, "AUTO FX steers it");
        app.song.auto_fx = false;
        app.set_energy(0.2, 0.0);
        assert_eq!(app.song.macros.energy, 0.2);
        app.set_energy(3.0, 0.0);
        assert_eq!(app.song.macros.energy, 1.0);
    }

    #[test]
    fn solarum_file_loads_everything() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        let mut f = SolarumFile {
            song: gen::generate(Vibe::Electro, &Default::default(), &mut Rng::new(3)),
            ..Default::default()
        };
        f.scenes[1] = Some(gen::generate(Vibe::Dub, &Default::default(), &mut Rng::new(4)));
        f.names[1] = "Sunday".into();
        app.song.master = 0.4;
        app.load_file(SolarumFile::from_text(&f.to_text()).unwrap(), "test.solarum", 0.0);
        assert_eq!(app.song.tracks, f.song.tracks);
        assert_eq!(app.song.master, 0.4, "volume belongs to the moment");
        assert_eq!(app.slots.to_vec(), f.scenes);
        assert_eq!(app.names[1], "Sunday");
    }

    #[test]
    fn solo_and_back() {
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        let before = app.song.tracks.map(|t| t.mute);
        app.toggle_solo(2, 0.0);
        for (i, t) in app.song.tracks.iter().enumerate() {
            assert_eq!(t.mute, i != 2);
        }
        app.toggle_solo(5, 0.0); // switch solo to another track
        assert!(!app.song.tracks[5].mute && app.song.tracks[2].mute);
        app.toggle_solo(5, 0.0);
        assert_eq!(app.song.tracks.map(|t| t.mute), before);
    }
}
