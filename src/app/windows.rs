//! Windows: settings, WAV export, help, and the FACE visual in its own viewport.

use super::*;

impl SolarumApp {
    /// FACE in its own window: drag it to the projector; double-click or F goes fullscreen,
    /// Esc leaves fullscreen, closing the window ends it
    pub(super) fn face_viewport(&mut self, ctx: &egui::Context, live: &Live, now: f64) {
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

    pub(super) fn toggle_visual(&mut self, ctx: &egui::Context) {
        self.visual = !self.visual;
        if self.with_devices {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.visual));
        }
    }

    /// Settings: volume, sound output, MIDI, update check, and the way to the code
    /// Luna's window: its level, where the stardust came from, its skin – opened from Luna
    /// itself (the start screen, the main screen, the level-up toast)
    pub(super) fn luna_window(&mut self, ctx: &egui::Context) {
        let mut open = self.luna_open;
        egui::Window::new("Luna")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                let note = |ui: &mut Ui, text: &str| {
                    ui.label(egui::RichText::new(text).font(body(12.0)).color(DIM));
                };
                // Luna: how far it has come, and its skin – what's not unlocked yet shows its level
                let lv = self.luna.level();
                let (into, next) = self.luna.progress();
                let t = ui.input(|i| i.time) as f32;
                egui::Frame::new()
                    .fill(SPACE)
                    .stroke(stroke(1.0, alpha(self.luna.ink(), 0.45)))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 10.0);
                        ui.horizontal_top(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(84.0, 96.0), egui::Sense::hover());
                            let c = r.center() - vec2(0.0, 10.0);
                            crate::face::halo(ui.painter(), c, 36.0, self.luna.ink(), 0.25);
                            self.luna.draw(ui.painter(), c, 34.0, t, 1.0, (t * 2.0).sin().max(0.0));
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(format!("Luna  ·  Level {lv}")).font(bold(16.0)).color(TEXT));
                                let (bar, _) = ui.allocate_exact_size(vec2(300.0, 8.0), egui::Sense::hover());
                                ui.painter().rect_filled(bar, 4.0, PANEL_HI);
                                let fill = Rect::from_min_size(bar.min, vec2(bar.width() * into, bar.height()));
                                ui.painter().rect_filled(fill, 4.0, self.luna.ink());
                                let dust = match next {
                                    Some(n) => format!("{} {} / {n} stardust to level {}", ph::SPARKLE, self.luna.dust, lv + 1),
                                    None => format!("{} {} stardust – all unlocked", ph::SPARKLE, self.luna.dust),
                                };
                                ui.label(egui::RichText::new(dust).font(bold(12.0)).color(self.luna.ink()));
                                let (total, today) = (self.luna.total, self.luna.today_tally(crate::luna::today()));
                                note(
                                    ui,
                                    &format!("From playing {} · breeding {} · keeping {}", total.play, total.pick, total.keep),
                                );
                                note(
                                    ui,
                                    &format!(
                                        "Today: breeding {} of {} · keeping {} of {} · playing {}{}",
                                        today.pick,
                                        crate::luna::PICK_DAY,
                                        today.keep,
                                        crate::luna::KEEP_DAY,
                                        today.play,
                                        if today.play >= crate::luna::PLAY_FULL {
                                            " (slower now)"
                                        } else {
                                            ""
                                        }
                                    ),
                                );
                                note(
                                    ui,
                                    "Stardust comes from playing (one every 20 seconds – after an hour a day, every 80), \
                                 picking a bred sound (+2) and keeping or saving a sound (+5). Every level brings Luna something new.",
                                );
                            });
                        });
                        ui.add_space(6.0);
                        let mut luna = self.luna;
                        let row = |ui: &mut Ui, title: &str, items: &[(&str, usize, Color32)], picked: &mut u8| {
                            ui.horizontal_wrapped(|ui| {
                                ui.add_sized(
                                    vec2(70.0, 26.0),
                                    egui::Label::new(egui::RichText::new(title).font(bold(11.0)).color(FAINT)),
                                );
                                for (k, &(name, need, col)) in items.iter().enumerate() {
                                    let open = need <= lv;
                                    let text = if open {
                                        name.to_owned()
                                    } else {
                                        format!("{}  {name} · {need}", ph::LOCK_SIMPLE)
                                    };
                                    ui.add_enabled_ui(open, |ui| {
                                        let resp = widgets::chip(ui, &text, *picked as usize == k, col);
                                        let resp = if open {
                                            resp
                                        } else {
                                            resp.on_disabled_hover_text(format!("Unlocks at level {need}"))
                                        };
                                        if resp.clicked() {
                                            *picked = k as u8;
                                        }
                                    });
                                }
                            });
                        };
                        let ink = luna.ink();
                        let antennas: Vec<_> = crate::luna::ANTENNAS.iter().map(|a| (a.0, a.1, ink)).collect();
                        let extras: Vec<_> = crate::luna::EXTRAS.iter().map(|e| (e.0, e.1, ink)).collect();
                        row(ui, "ANTENNA", &antennas, &mut luna.antenna);
                        row(ui, "COLOUR", &crate::luna::COLORS, &mut luna.color);
                        row(ui, "EXTRA", &extras, &mut luna.extra);
                        self.luna = luna;
                        ui.add_space(4.0);
                        ui.checkbox(&mut self.luna_desk, "Show Luna on the main screen (bottom right)");
                        if self.luna.preview {
                            note(
                                ui,
                                "Preview: everything unlocked to try it on (SOLARUM_LUNA_ALL) – your stardust stays as it is.",
                            );
                        }
                    });
                ui.add_space(12.0);
            });
        self.luna_open = open;
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
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
                             How often Luna changes something and AUTO FX are under SET in its title bar.",
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

    /// A recorded song: save it as a WAV or let it go
    pub(super) fn export_window(&mut self, ctx: &egui::Context, now: f64) {
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

    pub(super) fn help_window(&mut self, ctx: &egui::Context) {
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
                    row(ui, ph::PLAY, "1 · Press play", "Solarum starts with an empty sky: paint planets on the orbits and press Space or the sun. Or switch on the computer under the tracks – Luna takes over: it keeps your beat, adds what's missing one track per bar and plays a set; its little OS has the vibe, SHUFFLE (Enter), SET, OFF and a prompt (type help). ⌘/Ctrl + Z undoes anything – the first undo brings back your last groove.");
                    row(ui, ph::PLANET, "2 · Read the planets", "Every track is an orbit round the sun, once round is one bar of 16 steps – it starts at the back, where the faint arrow at the top begins, and runs the way it points. Every step that plays is a planet; the beam sweeps round and lights up each one it passes. Bass and synth planets float higher or lower with their note. A ring like Saturn's: accent (louder). A coloured circle around it: it only plays sometimes. A trail to the planet before: the synth slides into it. A faded orbit: that track is muted. The sun is the play button. CLASSIC at the top shows the same as pads and a step grid (Tab switches); the older RADAR view can be switched on in the settings.");
                    row(ui, ph::HAND_POINTING, "3 · Change the groove", "Click a dot to switch it on or off, drag to paint. Right-click a dot (or a tile in CLASSIC, the pads and step grid – Tab) for its menu: on, accent, slide, how often it plays and – on bass and synth – its note. The mouse wheel on a bass or synth dot moves its note too.");
                    row(ui, ph::LIST, "4 · Tracks", "S solos a track, the speaker mutes it, the dice rolls a new rhythm for it (right-click: clear), the lock keeps shuffle and the auto mode away from it. The coloured pill is its sound – click it for the list, tune and decay.");
                    row(ui, ph::MUSIC_NOTES, "5 · Mood & pitch", "MOOD (in the tempo box's menu at the top) picks the notes and chords of bass and synth – from Dark and Hypnotic to Euphoric, Sunny or Mysterious. On Auto every shuffle picks one that suits the vibe; a mood you pick stays. KEY (top right) moves the whole song to another key.");
                    row(ui, ph::LIGHTNING, "6 · Energy & drop", "The energy bar is the arrangement, not the volume (← →): under 30 a break with no kick and bass, then the tracks come in one by one, from 70 everything plays (a track the energy takes out is muted – the mute button shows it); by hand it starts full. DROP (B) builds up for two bars and drops.");
                    row(ui, ph::WAVEFORM, "7 · Effects & sound", "Hold STUTTER (Q), ECHO (W), WASH (E), CRUSH (X), GATE (C) or TAPE STOP (V) and combine them; The filter pad: left low-pass, right high-pass, up resonance – it springs back when you let go. Top right: the character pad (up DARK, right ACID), SPACE, PUMP, KEY (the song's key – the planets' notes stay steps of the scale) and DRIVE, and the EQ curve – drag its points. Double-click resets.");
                    row(ui, ph::INFINITY, "8 · Auto mode", "The computer under the mood is the auto mode: its power button switches it on, Luna says hello and keeps the groove moving – something changes every 1 to 16 bars (SET in its title bar, where AUTO FX is too; OFF switches it off). Type to Luna (help opens its manual): shuffle, vibe acid, drop, like, nope, taste, chill, turnup, fx off, every 8, mood dark, bpm 135, status, joke, off. Every change is picked by ear from six tries, and the groove comes back to its theme before the drop. With AUTO FX the ENERGY slider under its menu lifts or lowers the whole set – hotter, busier, a touch faster, or the other way. Thumb its last move up or down and it learns your taste; a track you edit stays yours for two phrases. With AUTO FX on it also steers the energy and plays breakdowns, build-ups and drops by itself – the map shows the cycle ahead. With AUTO FX off the energy bar is yours and no track is ever taken out.");
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
