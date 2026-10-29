//! Each frame's exchange with the engine and the keyboard: engine events, the log state, pad hits, keys.

use super::*;

impl SolarumApp {
    /// What plays, for the log
    pub(super) fn log_state(&self) -> crate::log::State {
        use crate::log::{State, Where};
        let live = self.fake_live.as_ref().map(|l| l.playing);
        if !live.unwrap_or_else(|| self.playing()) {
            return State::Stopped;
        }
        let bar = self.shared.step.load(Ordering::Relaxed) / STEPS as u64;
        if self.song_form.is_some() {
            State::Song
        } else if self.song.auto && self.song.auto_fx {
            State::Auto(Where::read(&self.shared, bar, self.song.auto_bars))
        } else {
            State::Bar(bar)
        }
    }

    pub(super) fn hit(&mut self, voice: Voice) {
        let record = self.record.load(Ordering::Relaxed);
        self.send(Cmd::Hit { voice, vel: 0.9, record });
    }

    // ───────────── Per-frame state from the engine ─────────────

    pub(super) fn sync(&mut self, ctx: &egui::Context, now: f64, dt: f32) -> Live {
        while let Ok(ev) = self.events.try_recv() {
            let (song, change, ack, why) = match ev {
                Event::Song(song, change, ack, why) => (song, change, ack, why),
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
            let before = self.song;
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
                // The auto mode's own moves get a thumb up or down in SOLAR OS
                if let (true, true, Some(v)) = (self.song.auto, c.kind().is_some(), c.voice()) {
                    self.last_auto = Some((c, *before.track(v), now));
                }
                let text = match (c, why) {
                    (gen::Change::Scene(i), _) => format!("{} · {}", i + 1, self.fav_name(i)),
                    (_, Some(w)) => format!("{} – {}", c.describe(), w.describe()),
                    _ => c.describe(),
                };
                self.say(text, now);
            }
        }
        // chill / turnup: tempo and energy glide over a few bars, unless you take over
        let glide = |value: &mut f32, g: &mut Option<(f32, f32)>, per_s: f32| {
            let Some((goal, last)) = *g else { return };
            if (*value - last).abs() > 1e-3 {
                *g = None;
                return;
            }
            let step = per_s * dt;
            *value = if (goal - *value).abs() <= step {
                goal
            } else {
                *value + step * (goal - *value).signum()
            };
            *g = (*value != goal).then_some((goal, *value));
        };
        glide(&mut self.song.bpm, &mut self.glide_bpm, 0.6);
        glide(&mut self.song.macros.energy, &mut self.glide_energy, 0.03);
        if self.glide_bpm.is_some() || self.glide_energy.is_some() {
            ctx.request_repaint();
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

    pub(super) fn keys(&mut self, ctx: &egui::Context, now: f64) {
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
                Key::ArrowUp => self.song.bpm = (self.song.bpm + 1.0).min(*crate::song::BPM.end()),
                Key::ArrowDown => self.song.bpm = (self.song.bpm - 1.0).max(*crate::song::BPM.start()),
                Key::R => self.toggle_capture(now),
                _ => {}
            }
        }
    }
}
