//! The track list: solo, sounds and samples per track, the track menu, rolling tracks.

use super::*;

impl SolarumApp {
    pub(super) fn toggle_solo(&mut self, i: usize, now: f64) {
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
    pub(super) fn set_sound(&mut self, i: usize, sound: u8, now: f64) {
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
    pub(super) fn sync_samples(&mut self) {
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
    pub(super) fn roll_track(&mut self, i: usize) {
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
    pub(super) fn dice_all(&mut self) -> usize {
        let on: Vec<usize> = (0..TRACKS)
            .filter(|&i| !self.song.tracks[i].mute && !self.song.tracks[i].lock)
            .collect();
        for &i in &on {
            self.roll_track(i);
        }
        on.len()
    }

    pub(super) fn tracks_panel(&mut self, ui: &mut Ui, live: &Live, now: f64) {
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
}
