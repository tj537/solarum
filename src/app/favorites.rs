//! Favorites (the 12 named slots), the patterns strip, and song mode built from the favorites.

use super::*;

impl SolarumApp {
    /// Click on a pattern: an empty one keeps what you hear, a full one switches to it –
    /// while playing on the next bar, with a clap roll before
    pub(super) fn slot_clicked(&mut self, i: usize, save: bool, now: f64) {
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
    pub(super) fn fav_name(&self, i: usize) -> String {
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
    pub(super) fn rename_window(&mut self, ctx: &egui::Context) {
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
    pub(super) fn send_arrangement(&self) {
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
    pub(super) fn start_song(&mut self, form: usize, now: f64) {
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

    pub(super) fn stop_song(&mut self, now: f64) {
        self.song_form = None;
        self.send_arrangement();
        self.say("Song off – the pattern keeps playing", now);
    }

    /// The strip under the orbit: twelve favorites in two rows, and AUTO SONG
    pub(super) fn patterns_ui(&mut self, ui: &mut Ui, rect: Rect, live: &Live, now: f64) {
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
    pub(super) fn song_menu(&mut self, ui: &mut Ui, resp: egui::Response, now: f64) {
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
    pub(super) fn song_strip(&mut self, ui: &mut Ui, rect: Rect, live: &Live, now: f64) {
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
}
