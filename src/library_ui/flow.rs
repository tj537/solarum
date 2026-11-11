//! The new-sound flow around them: the start page (the ways to a new sound, the sound of
//! the day), recording, and the bar that leads back and on.

use super::*;

/// … and for Luna's sound of the day
const DAY_ID: u64 = u64::MAX - 2;

impl LibraryUi {
    pub(super) fn open_start(&mut self) {
        self.flow = Some(Step::Start);
        self.drawing = None;
        self.recorder = None;
        self.breed = None;
    }

    /// One step back; from the start, out of the flow
    pub(super) fn flow_back(&mut self, lib: &mut Library, out: &mut Vec<Out>) {
        self.stop(out);
        self.flow = match self.flow {
            Some(Step::Finish(id)) => {
                lib.remove(id);
                Some(
                    if (self.back_to == Step::Draw && self.drawing.is_none()) || (self.back_to == Step::Breed && self.breed.is_none()) {
                        Step::Start
                    } else {
                        self.back_to
                    },
                )
            }
            // a drawing painted from a bred sound goes back to its family
            Some(Step::Draw) if self.breed.is_some() => {
                self.drawing = None;
                Some(Step::Breed)
            }
            Some(Step::Draw) | Some(Step::Record) | Some(Step::Breed) => {
                self.drawing = None;
                self.recorder = None;
                self.breed = None;
                Some(Step::Start)
            }
            _ => None,
        };
    }

    /// Leave the flow and throw away what was made
    fn flow_cancel(&mut self, lib: &mut Library, out: &mut Vec<Out>) {
        self.stop(out);
        if let Some(Step::Finish(id)) = self.flow {
            lib.remove(id);
        }
        self.flow = None;
        self.drawing = None;
        self.recorder = None;
        self.breed = None;
    }

    /// Open the record step (for the snapshots)
    #[cfg(test)]
    pub fn show_record(&mut self) {
        self.open = true;
        self.flow = Some(Step::Record);
    }

    /// Open the start page (for the snapshots)
    #[cfg(test)]
    pub fn show_start(&mut self) {
        self.open = true;
        self.open_start();
    }

    /// Every way to a new sound: a track's template to paint over – or a blank board, the
    /// mic, a file
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_page(
        &mut self,
        ui: &mut Ui,
        r: Rect,
        lib: &mut Library,
        playhead: Option<f64>,
        blocked: bool,
        out: &mut Vec<Out>,
        now: f64,
    ) {
        let inner = r.shrink(22.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        if blocked {
            ui.disable();
        }
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        ui.label(egui::RichText::new("How do you want to start?").font(bold(26.0)).color(TEXT));
        ui.label(
            egui::RichText::new("Start from nothing – or pick a track and paint over its sound until it's yours.")
                .font(body(13.0))
                .color(DIM),
        );
        ui.add_space(10.0);
        let gap = 12.0;
        let low_h = 136.0;
        let t = ui.input(|i| i.time) as f32;

        // The easiest ways, on top: half Luna's sounds of the day, half breeding your own
        let top_h = 168.0;
        let (row, _) = ui.allocate_exact_size(vec2(inner.width(), top_h), Sense::hover());
        let half = (row.width() - gap) / 2.0;
        let day_r = Rect::from_min_size(row.min, vec2(half, top_h));
        let br = Rect::from_min_size(pos2(day_r.right() + gap, row.top()), vec2(half, top_h));
        self.day_card(&mut ui, day_r, lib, playhead, blocked, out, now);

        let resp = ui.interact(br, ui.id().with("breed-card"), Sense::click());
        let hot = resp.hovered() && !blocked;
        let p = ui.painter_at(br.expand(6.0));
        p.rect_stroke(
            br.expand(3.0),
            15.0,
            stroke(5.0, alpha(ACID, if hot { 0.22 } else { 0.1 })),
            egui::StrokeKind::Inside,
        );
        p.rect_filled(br, 12.0, mix(PAD, ACID, if hot { 0.14 } else { 0.07 }));
        p.rect_stroke(br, 12.0, stroke(1.5, ACID), egui::StrokeKind::Inside);
        let pic = Rect::from_min_size(br.min + vec2(12.0, 12.0), vec2((br.width() * 0.36).min(220.0), br.height() - 24.0));
        p.rect_filled(pic, 9.0, SPACE);
        family_picture(&p, pic.shrink(12.0), hot, t);
        let x = pic.right() + 18.0;
        let badge = p.layout_no_wrap("NEW".into(), bold(10.0), BG);
        let badge_r = Rect::from_min_size(pos2(x, br.top() + 22.0), badge.size() + vec2(14.0, 6.0));
        p.rect_filled(badge_r, badge_r.height() / 2.0, ACID);
        p.galley(badge_r.center() - badge.size() / 2.0, badge, BG);
        p.text(pos2(x, br.top() + 54.0), Align2::LEFT_CENTER, "Breed a sound", bold(20.0), TEXT);
        let sub_w = br.right() - x - 18.0;
        for (k, line) in [
            "Hear four, click the one you like best –",
            "it gets four children. A few rounds, no knobs.",
        ]
        .into_iter()
        .enumerate()
        {
            let line = fit_text(&p, line, body(13.0), sub_w);
            p.text(
                pos2(x, br.top() + 80.0 + k as f32 * 19.0),
                Align2::LEFT_CENTER,
                line,
                body(13.0),
                DIM,
            );
        }
        let go = p.layout_no_wrap(format!("Start breeding  {}", ph::ARROW_RIGHT), bold(14.0), BG);
        let go_r = Rect::from_min_size(
            br.right_bottom() - go.size() - vec2(36.0 + 16.0, 18.0 + 14.0),
            go.size() + vec2(36.0, 18.0),
        );
        p.rect_filled(go_r, go_r.height() / 2.0, if hot { Color32::WHITE } else { ACID });
        p.galley(go_r.center() - go.size() / 2.0, go, BG);
        if hot {
            ui.ctx().request_repaint();
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
            self.start_breeding(self.filter.unwrap_or(Voice::Kick), crate::rng::Rng::from_clock(), now);
        }
        ui.add_space(gap);

        // From nothing: draw, record, import
        let bw = (inner.width() - gap * 2.0) / 3.0;
        let mut way = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (k, title, sub) in [
                (0, "Blank board", "Paint any line, hear it"),
                (1, "Record", "Voice, door – anything"),
                (2, "Import", "WAV, MP3, FLAC, AIFF …"),
            ] {
                let (br, resp) = ui.allocate_exact_size(vec2(bw, low_h - 24.0), Sense::click());
                let hot = resp.hovered() && !blocked;
                let col = if k == 1 { RED } else { ACID };
                let p = ui.painter_at(br);
                p.rect_filled(br, 12.0, if hot { mix(PAD, col, 0.08) } else { PAD });
                p.rect_stroke(
                    br,
                    12.0,
                    stroke(if hot { 1.5 } else { 1.0 }, if hot { col } else { LINE }),
                    egui::StrokeKind::Inside,
                );
                // the picture on the left, in its own little window into space
                let pic = Rect::from_min_size(br.min + vec2(10.0, 10.0), vec2((br.width() * 0.4).min(150.0), br.height() - 20.0));
                p.rect_filled(pic, 9.0, SPACE);
                way_picture(&p, pic.shrink(10.0), k, col, hot, t);
                let x = pic.right() + 16.0;
                let w = br.right() - x - 14.0;
                p.text(pos2(x, br.top() + 30.0), Align2::LEFT_CENTER, title, bold(17.0), TEXT);
                let sub = fit_text(&p, sub, body(12.0), w);
                p.text(pos2(x, br.top() + 54.0), Align2::LEFT_CENTER, sub, body(12.0), DIM);
                p.text(
                    pos2(br.right() - 14.0, br.bottom() - 16.0),
                    Align2::RIGHT_CENTER,
                    ph::ARROW_RIGHT,
                    body(15.0),
                    if hot { col } else { FAINT },
                );
                if hot {
                    ui.ctx().request_repaint();
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
                    way = Some(k);
                }
            }
        });
        match way {
            Some(0) => self.start_drawing(None, self.filter.unwrap_or(Voice::Rim), false),
            Some(1) => {
                self.flow = Some(Step::Record);
                self.inputs = library::inputs();
            }
            Some(2) => {
                if let Some(paths) = rfd::FileDialog::new().add_filter("Audio", &library::EXTENSIONS).pick_files() {
                    self.import(lib, &paths, now);
                }
            }
            _ => {}
        }

        // Eight tracks, two rows of four
        ui.add_space(14.0);
        ui.label(egui::RichText::new("OR PAINT OVER A TRACK").font(bold(11.0)).color(FAINT));
        let grid_h = (ui.available_height() - 6.0).max(160.0);
        let cw = (inner.width() - gap * 3.0) / 4.0;
        let ch = ((grid_h - gap) / 2.0).min(210.0);
        let mut picked = None;
        for row in Voice::ALL.chunks(4) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &v in row {
                    let (cr, resp) = ui.allocate_exact_size(vec2(cw, ch), Sense::click());
                    let col = track_color(v);
                    let hot = resp.hovered() && !blocked;
                    let p = ui.painter_at(cr);
                    p.rect_filled(cr, 12.0, if hot { mix(PAD, col, 0.10) } else { PAD });
                    p.rect_stroke(
                        cr,
                        12.0,
                        stroke(if hot { 1.5 } else { 1.0 }, if hot { col } else { LINE }),
                        egui::StrokeKind::Inside,
                    );
                    p.circle_filled(pos2(cr.left() + 18.0, cr.top() + 20.0), 5.0, col);
                    p.text(
                        pos2(cr.left() + 30.0, cr.top() + 20.0),
                        Align2::LEFT_CENTER,
                        v.name(),
                        bold(15.0),
                        TEXT,
                    );
                    let pic = Rect::from_min_max(pos2(cr.left() + 14.0, cr.top() + 38.0), pos2(cr.right() - 14.0, cr.bottom() - 30.0));
                    thumb_drawing(&p, pic, &library::template(v), col);
                    p.text(
                        pos2(cr.right() - 14.0, cr.bottom() - 15.0),
                        Align2::RIGHT_CENTER,
                        format!("Paint it  {}", ph::ARROW_RIGHT),
                        bold(12.0),
                        if hot { col } else { FAINT },
                    );
                    if hot {
                        // a little sweep over the picture while you look at it
                        let x = pic.left() + (t * 0.6).fract() * pic.width();
                        p.line_segment([pos2(x, pic.top()), pos2(x, pic.bottom())], stroke(1.0, alpha(TEXT, 0.35)));
                        ui.ctx().request_repaint();
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
                        picked = Some(v);
                    }
                }
            });
            ui.add_space(gap - 6.0);
        }
        if let Some(v) = picked {
            self.start_drawing(Some((library::template(v), template_name(v))), v, true);
            self.play_drawing(out, now);
        }
    }

    /// The bar over a new sound: Cancel on the left, where you are in the middle (1 Start ›
    /// 2 Draw or Record › 3 Finish), Back and the step's one big button on the right
    pub(super) fn flow_bar(&mut self, ui: &mut Ui, top: Rect, lib: &mut Library, step: Step, out: &mut Vec<Out>, now: f64) {
        let mut bar = ui.new_child(
            UiBuilder::new()
                .max_rect(top)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if chip(&mut bar, &format!("{}  Cancel", ph::X), false, TEXT)
            .on_hover_text("Throw it away – back to your sounds")
            .clicked()
        {
            self.flow_cancel(lib, out);
            return;
        }
        bar.add_space(14.0);
        bar.label(egui::RichText::new("NEW SOUND").font(display(34.0)).color(TEXT));

        // Where you are: three steps, the done ones ticked
        let make = match (step, self.back_to) {
            (Step::Draw, _) | (Step::Finish(_), Step::Draw) => "Draw",
            (Step::Record, _) | (Step::Finish(_), Step::Record) => "Record",
            (Step::Breed, _) | (Step::Finish(_), Step::Breed) => "Breed",
            _ => "Make",
        };
        let at = match step {
            Step::Start => 0,
            Step::Draw | Step::Record | Step::Breed => 1,
            Step::Finish(_) => 2,
        };
        let p = ui.painter();
        let names = ["Start", make, "Finish"];
        let galleys: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(k, n)| {
                let text = if k < at {
                    format!("{}  {n}", ph::CHECK)
                } else {
                    format!("{}  {n}", k + 1)
                };
                p.layout_no_wrap(text, bold(13.0), Color32::PLACEHOLDER)
            })
            .collect();
        let sep = 34.0;
        let total: f32 = galleys.iter().map(|g| g.size().x + 28.0).sum::<f32>() + sep * 2.0;
        let mut x = top.center().x - total / 2.0;
        for (k, g) in galleys.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, top.center().y - 15.0), vec2(g.size().x + 28.0, 30.0));
            let (fill, ink) = match k.cmp(&at) {
                std::cmp::Ordering::Equal => (ACID, BG),
                std::cmp::Ordering::Less => (mix(PAD, ACID, 0.18), ACID),
                std::cmp::Ordering::Greater => (PAD, FAINT),
            };
            p.rect_filled(r, 15.0, fill);
            p.galley(r.center() - g.size() / 2.0, g, ink);
            x = r.right();
            if k < 2 {
                p.text(
                    pos2(x + sep / 2.0, r.center().y),
                    Align2::CENTER_CENTER,
                    ph::CARET_RIGHT,
                    body(14.0),
                    FAINT,
                );
                x += sep;
            }
        }

        bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match step {
                Step::Draw => {
                    let drawn = self.drawing.as_ref().is_some_and(|d| d.pitch.iter().any(Option::is_some));
                    if pill(ui, &format!("Next  {}", ph::ARROW_RIGHT), drawn)
                        .on_hover_text("Make it a sound – then cut and shape it")
                        .clicked()
                    {
                        self.draw_next(lib, out);
                    }
                }
                Step::Breed => {
                    let picked = self.breed.as_ref().is_some_and(|b| !b.picks.is_empty());
                    if pill(ui, &format!("Keep it  {}", ph::ARROW_RIGHT), picked)
                        .on_hover_text("Your last pick becomes a sound – then cut and shape it")
                        .clicked()
                    {
                        self.breed_next(lib, out);
                    }
                }
                Step::Finish(id)
                    if pill(ui, &format!("{}  Save", ph::FLOPPY_DISK), true)
                        .on_hover_text("Into your sounds")
                        .clicked() =>
                {
                    self.stop(out);
                    self.save_draft(lib, id, now);
                    out.push(Out::Dust(crate::luna::Source::Keep));
                }
                _ => {}
            }
            if step != Step::Start
                && chip(ui, &format!("{}  Back", ph::ARROW_LEFT), false, TEXT)
                    .on_hover_text("One step back (Esc)")
                    .clicked()
            {
                self.flow_back(lib, out);
            }
        });
    }

    /// Luna with the day's three sounds, one at a time: ‹ › (or the dots) go through them,
    /// click the sound to hear it, Keep it puts it in your sounds
    #[allow(clippy::too_many_arguments)]
    fn day_card(&mut self, ui: &mut Ui, br: Rect, lib: &mut Library, playhead: Option<f64>, blocked: bool, out: &mut Vec<Out>, now: f64) {
        let today = library::DaySound::today();
        if self.day.first().is_none_or(|(d, _)| d.day != today[0].day) {
            self.day = today
                .into_iter()
                .map(|d| {
                    let b = bred_buf(&d.genes);
                    (d, b)
                })
                .collect();
            self.day_at = 0;
        }
        let at = self.day_at.min(self.day.len().saturating_sub(1));
        let Some((d, buf)) = self.day.get(at) else { return };
        let (voice, genes, name, says, date) = (d.voice, d.genes, d.name.clone(), d.says, d.date());
        let buf = buf.clone();
        let count = self.day.len();
        let col = track_color(voice);
        let kept = lib.entries.iter().find(|e| !e.draft && e.name == name).map(|e| e.id);
        let t = ui.input(|i| i.time) as f32;
        let p = ui.painter_at(br.expand(6.0));
        let hot = ui.rect_contains_pointer(br) && !blocked;

        // a deep-space card with a lime rim and a few stars of its own
        p.rect_stroke(
            br.expand(3.0),
            15.0,
            stroke(5.0, alpha(ACID, if hot { 0.2 } else { 0.08 })),
            egui::StrokeKind::Inside,
        );
        p.rect_filled(br, 12.0, mix(SPACE, PAD, 0.55));
        p.rect_stroke(
            br,
            12.0,
            stroke(1.5, alpha(ACID, if hot { 1.0 } else { 0.7 })),
            egui::StrokeKind::Inside,
        );
        for i in 0..16 {
            let x = br.left() + crate::widgets::hash(i * 7 + 1) * br.width();
            let y = br.top() + 8.0 + crate::widgets::hash(i * 7 + 2) * (br.height() - 16.0);
            let tw = 0.2 + 0.35 * ((t * 1.3 + i as f32).sin() * 0.5 + 0.5);
            p.circle_filled(pos2(x, y), 1.0, alpha(TEXT, tw));
        }

        // Luna, waving now and then (all the time while you look), and what it made
        let luna = pos2(br.left() + 46.0, br.top() + 50.0);
        let wave = if hot {
            (t * 6.0).sin() * 0.5 + 0.5
        } else {
            ((t % 7.0 - 6.0).max(0.0) * TAU).sin().abs()
        };
        crate::face::halo(&p, luna, 34.0, self.luna.ink(), 0.25);
        self.luna.draw(&p, luna, 30.0, t, 1.0, wave);
        let x = br.left() + 92.0;
        p.text(
            pos2(x, br.top() + 22.0),
            Align2::LEFT_CENTER,
            format!("LUNA'S SOUNDS OF THE DAY  ·  {}", date.to_uppercase()),
            bold(11.0),
            ACID,
        );

        // ‹ 1 2 3 › top right: one dot per sound, in its track's colour
        let mut go = None;
        let ny = br.top() + 24.0;
        let arrow = |p: &egui::Painter, c: Pos2, icon: &str, hot: bool| {
            p.circle_filled(c, 13.0, if hot { PANEL_HI } else { alpha(PANEL_HI, 0.5) });
            p.text(c, Align2::CENTER_CENTER, icon, body(13.0), if hot { TEXT } else { DIM });
        };
        let next_c = pos2(br.right() - 26.0, ny);
        let dots_w = count as f32 * 16.0;
        let prev_c = pos2(next_c.x - 26.0 - dots_w - 8.0, ny);
        for (c, icon, step) in [(prev_c, ph::CARET_LEFT, count - 1), (next_c, ph::CARET_RIGHT, 1)] {
            let resp = ui.interact(
                Rect::from_center_size(c, vec2(26.0, 26.0)),
                ui.id().with(("day-nav", step)),
                Sense::click(),
            );
            arrow(&p, c, icon, resp.hovered() && !blocked);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
                go = Some((at + step) % count);
            }
        }
        for k in 0..count {
            let c = pos2(prev_c.x + 13.0 + 4.0 + 8.0 + k as f32 * 16.0, ny);
            let resp = ui.interact(
                Rect::from_center_size(c, vec2(14.0, 22.0)),
                ui.id().with(("day-dot", k)),
                Sense::click(),
            );
            let dc = track_color(self.day[k].0.voice);
            if k == at {
                p.circle_filled(c, 5.0, dc);
            } else {
                p.circle_stroke(c, 4.0, stroke(1.5, if resp.hovered() { dc } else { alpha(dc, 0.6) }));
            }
            if resp.on_hover_text(self.day[k].0.name.clone()).clicked() && !blocked {
                go = Some(k);
            }
        }

        p.circle_filled(pos2(x + 6.0, br.top() + 50.0), 5.0, col);
        let title_w = prev_c.x - 20.0 - (x + 18.0);
        let title = fit_text(&p, &name, bold(20.0), title_w);
        p.text(pos2(x + 18.0, br.top() + 50.0), Align2::LEFT_CENTER, title, bold(20.0), TEXT);
        let line = fit_text(&p, &format!("“{says}”"), body(13.0), br.right() - 20.0 - x);
        p.text(pos2(x, br.top() + 76.0), Align2::LEFT_CENTER, line, body(13.0), DIM);

        // The sound (click to hear it) and Keep it
        let row = Rect::from_min_max(pos2(br.left() + 14.0, br.top() + 96.0), br.right_bottom() - vec2(16.0, 14.0));
        let mut pill_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if blocked {
            pill_ui.disable();
        }
        let mut keep = false;
        let mut show = None;
        match kept {
            None => {
                if pill(&mut pill_ui, &format!("{}  Keep it", ph::PLUS), true)
                    .on_hover_text(format!("Into your sounds – for the {} track", voice.name()))
                    .clicked()
                {
                    keep = true;
                }
            }
            Some(id) => {
                if chip(&mut pill_ui, &format!("{}  In your sounds", ph::CHECK), true, ACID)
                    .on_hover_text("Show it in your sounds")
                    .clicked()
                {
                    show = Some(id);
                }
            }
        }
        let win = Rect::from_min_max(row.min, pos2(pill_ui.min_rect().left() - 14.0, row.bottom()));
        let win_resp = ui.interact(win, ui.id().with("day-sound"), Sense::click());
        let win_hot = win_resp.hovered() && !blocked;
        p.rect_filled(win, 9.0, SPACE);
        p.rect_stroke(
            win,
            9.0,
            stroke(1.0, if win_hot { col } else { alpha(LINE, 0.7) }),
            egui::StrokeKind::Inside,
        );
        let play_c = pos2(win.left() + 28.0, win.center().y);
        let is_playing = self.playing.as_ref().is_some_and(|pl| pl.id == DAY_ID + at as u64);
        p.circle_filled(play_c, 17.0, if win_hot { Color32::WHITE } else { col });
        p.text(
            play_c + vec2(if is_playing { 0.0 } else { 1.5 }, 0.0),
            Align2::CENTER_CENTER,
            if is_playing { ph::STOP } else { ph::PLAY },
            body(15.0),
            BG,
        );
        let pic = Rect::from_min_max(pos2(play_c.x + 30.0, win.top() + 8.0), win.right_bottom() - vec2(12.0, 8.0));
        thumb_drawing(&p, pic, &genes.drawing(), col);
        if let (true, Some(pos)) = (is_playing, playhead) {
            let x = pic.left() + (pos as f32 / SAMPLE_SR / genes.length).min(1.0) * pic.width();
            p.line_segment([pos2(x, win.top() + 5.0), pos2(x, win.bottom() - 5.0)], stroke(1.5, TEXT));
        }
        if hot || is_playing {
            ui.ctx().request_repaint();
        } else {
            // the stars twinkle and Luna waves now and then
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }

        if let Some(k) = go {
            self.day_at = k;
            // the next one plays right away – that's what you went there for
            let b = self.day[k].1.clone();
            self.audition(DAY_ID + k as u64, b, out, now);
        } else if win_resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
            if is_playing {
                self.stop(out);
            } else {
                self.audition(DAY_ID + at as u64, buf.clone(), out, now);
            }
        }
        if keep {
            let id = lib.add(&name, voice, buf.data.clone());
            self.unsaved = true;
            self.selected = Some(id);
            self.say(format!("Kept – {name} is in your sounds{}", self.dust_note()), now);
            out.push(Out::Dust(crate::luna::Source::Keep));
        }
        if let Some(id) = show {
            self.stop(out);
            self.flow = None;
            self.filter = None;
            self.selected = Some(id);
        }
    }

    /// Recording, right on the screen: the input, a big red button, what comes in
    pub(super) fn record_page(&mut self, ui: &mut Ui, r: Rect, lib: &mut Library, now: f64) {
        let w = (r.width() - 80.0).min(760.0);
        let inner = Rect::from_center_size(r.center(), vec2(w, (r.height() - 60.0).min(420.0)));
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Center)));
        ui.spacing_mut().item_spacing = vec2(8.0, 12.0);
        {
            {
                let ui = &mut ui;
                ui.label(egui::RichText::new("Record a sound").font(bold(26.0)).color(TEXT));
                ui.label(
                    egui::RichText::new(format!(
                        "Your voice, a door, a glass – anything. Up to {MAX_SECONDS:.0} seconds; the silence is trimmed for you."
                    ))
                    .font(body(13.0))
                    .color(DIM),
                );
                ui.add_enabled_ui(self.recorder.is_none(), |ui| {
                    // As tall as the dropdown, so "Input" lines up with its middle
                    ui.spacing_mut().interact_size.y = 28.0;
                    ui.horizontal(|ui| {
                        // A row starts on the left: pad it so it sits centred like the rest
                        let label = ui.fonts(|f| f.layout_no_wrap("Input".into(), bold(12.0), DIM).size().x);
                        let row = label + ui.spacing().item_spacing.x + 300.0;
                        ui.add_space(((ui.available_width() - row) / 2.0).max(0.0));
                        ui.label(egui::RichText::new("Input").font(bold(12.0)).color(DIM));
                        egui::ComboBox::from_id_salt("rec_input")
                            .selected_text(self.input.clone().unwrap_or_else(|| "Default input".into()))
                            .width(300.0)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.input.is_none(), "Default input").clicked() {
                                    self.input = None;
                                }
                                for name in &self.inputs {
                                    if ui.selectable_label(self.input.as_ref() == Some(name), name).clicked() {
                                        self.input = Some(name.clone());
                                    }
                                }
                            });
                    });
                });
                // Level and what came in so far
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 130.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, 8.0, mix(SPACE, PANEL, 0.3));
                p.rect_stroke(r, 8.0, stroke(1.0, alpha(LINE, 0.6)), egui::StrokeKind::Inside);
                if let Some(rec) = &self.recorder {
                    let peaks = rec.snapshot(r.width() as usize);
                    let w = r.width() / (MAX_SECONDS * 20.0);
                    let secs = rec.seconds();
                    let n = peaks.len().max(1) as f32;
                    for (i, &v) in peaks.iter().enumerate() {
                        let x = r.left() + (i as f32 / n) * secs * 20.0 * w;
                        let h = v.min(1.0) * r.height() * 0.45;
                        p.line_segment([pos2(x, r.center().y - h), pos2(x, r.center().y + h)], stroke(1.0, RED));
                    }
                    let lvl = rec.level().min(1.0);
                    let meter = Rect::from_min_size(pos2(r.left(), r.bottom() + 4.0), vec2(r.width() * lvl, 4.0));
                    ui.painter().rect_filled(meter, 2.0, if lvl > 0.95 { RED } else { ACID });
                } else {
                    p.line_segment(
                        [pos2(r.left() + 16.0, r.center().y), pos2(r.right() - 16.0, r.center().y)],
                        stroke(1.0, alpha(RED, 0.35)),
                    );
                    p.text(
                        r.center() - vec2(0.0, 18.0),
                        Align2::CENTER_CENTER,
                        "What you record shows up here",
                        body(12.0),
                        FAINT,
                    );
                }
                ui.add_space(10.0);
                {
                    // One big red button: press to record, press again to stop
                    let rec_on = self.recorder.is_some();
                    let (br, bresp) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::click());
                    let hot = bresp.hovered();
                    let pulse = if rec_on {
                        ((now * 3.0).sin() as f32 * 0.5 + 0.5) * 6.0
                    } else {
                        0.0
                    };
                    ui.painter().circle_filled(br.center(), 40.0 + pulse, alpha(RED, 0.18));
                    ui.painter()
                        .circle_filled(br.center(), 34.0, if hot { mix(RED, Color32::WHITE, 0.2) } else { RED });
                    if rec_on {
                        ui.painter()
                            .rect_filled(Rect::from_center_size(br.center(), vec2(24.0, 24.0)), 4.0, TEXT);
                    } else {
                        ui.painter().circle_filled(br.center(), 12.0, TEXT);
                    }
                    let tip = if rec_on { "Stop" } else { "Start recording" };
                    let stop_now = self.recorder.as_ref().is_some_and(|r| r.seconds() >= MAX_SECONDS - 0.05);
                    let clicked = bresp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                    let line = match &self.recorder {
                        Some(rec) => format!("{:.1} s / {MAX_SECONDS:.0} s", rec.seconds()),
                        None => "Press to record".into(),
                    };
                    ui.label(egui::RichText::new(line).font(bold(14.0)).color(TEXT));
                    if clicked || stop_now {
                        match self.recorder.take() {
                            Some(rec) => {
                                let data = rec.finish();
                                if data.len() < 400 {
                                    self.say("That was too short", now);
                                } else {
                                    let n = lib.entries.iter().filter(|e| e.name.starts_with("Signal")).count() + 1;
                                    let voice = self.filter.unwrap_or(Voice::Rim);
                                    let id = lib.add(&format!("Signal {n}"), voice, data);
                                    lib.set_draft(id, true);
                                    self.finish_with(id, Step::Record);
                                }
                            }
                            None => match Recorder::start(self.input.as_deref()) {
                                Ok(r) => self.recorder = Some(r),
                                Err(e) => self.say(
                                    format!("Can't record: {e}. On a Mac, allow Solarum in Privacy & Security › Microphone."),
                                    now,
                                ),
                            },
                        }
                    }
                }
            }
        }
    }
}
