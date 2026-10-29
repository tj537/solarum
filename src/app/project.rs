//! Project files: the session, new/open/save, the unsaved-changes prompt, the home screen.

use super::*;

impl SolarumApp {
    /// Save or open a .solarum file
    /// Back to the start screen (the recent projects read again)
    pub(super) fn open_home(&mut self) {
        self.recent_cache = crate::home::load_recent(&self.recent);
        // Continue goes back to this project as it is now
        let now = self.session();
        if now.has_work() {
            self.last_session = Some(now);
        }
        self.home = true;
    }

    pub(super) fn home_action(&mut self, action: crate::home::Action, now: f64) {
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
    pub(super) fn session(&self) -> Session {
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
    pub(super) fn new_project(&mut self) {
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
        (self.filter, self.filter_res) = (0.0, 0.0);
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
    pub(super) fn mark_saved(&mut self) {
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
    pub(super) fn dirty(&self) -> bool {
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
    pub(super) fn unsaved_window(&mut self, ctx: &egui::Context, now: f64) {
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
    pub(super) fn project_name(&self) -> String {
        self.project
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned())
    }

    /// A project saved or opened goes to the top of the recent list
    pub(super) fn remember(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(12);
        self.project = Some(path.to_path_buf());
    }

    /// Save: to the project's file if it has one (`as_new`: always ask where)
    pub(super) fn save_file(&mut self, as_new: bool, now: f64) {
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

    pub(super) fn open_file(&mut self, path: Option<PathBuf>, now: f64) {
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

    pub(super) fn load_file(&mut self, f: SolarumFile, name: &str, now: f64) {
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
}
