//! The top bar: file menu, transport, the vibe dropdown, the BPM box and the tempo menu.

use super::*;

impl SolarumApp {
    pub(super) fn top_bar(&mut self, ui: &mut Ui, now: f64) {
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
    pub(super) fn vibe_list(&mut self, ui: &mut Ui, now: f64) {
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

    /// The tempo box: drag or scroll for the tempo, click for the menu – tempo, metronome
    /// and the mood. It shows the mood under the BPM, and a dot while the metronome ticks.
    fn bpm_box(&mut self, ui: &mut Ui, now: f64) {
        let (rect, resp) = ui.allocate_exact_size(vec2(176.0, 42.0), Sense::click_and_drag());
        if resp.dragged() {
            let d = resp.drag_delta();
            self.song.bpm = (self.song.bpm + (d.x - d.y) * 0.15).clamp(*crate::song::BPM.start(), *crate::song::BPM.end());
        }
        if resp.hovered() {
            let s = ui.input(|i| i.raw_scroll_delta.y);
            let n = orbit::scroll_notches(&mut self.bpm_scroll, s, 24.0);
            if n != 0 {
                self.song.bpm = (self.song.bpm.round() + n as f32).clamp(*crate::song::BPM.start(), *crate::song::BPM.end());
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
                self.song.bpm = (self.song.bpm - 1.0).max(*crate::song::BPM.start());
            }
            ui.spacing_mut().slider_width = 220.0;
            ui.add(
                egui::Slider::new(&mut self.song.bpm, crate::song::BPM)
                    .show_value(false)
                    .step_by(1.0),
            );
            if ui
                .button(egui::RichText::new("+").font(bold(14.0)))
                .on_hover_text("1 BPM faster")
                .clicked()
            {
                self.song.bpm = (self.song.bpm + 1.0).min(*crate::song::BPM.end());
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
}
