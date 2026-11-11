//! Drawing a sound: a board you paint its shape on, with undo and a play-as-you-draw preview.

use super::*;

/// What the pen does on the drawing board
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Tool {
    Pen,
    Erase,
}

impl LibraryUi {
    /// The drawing becomes a sound (a draft) – the board stays, for Back
    pub(super) fn draw_next(&mut self, lib: &mut Library, out: &mut Vec<Out>) {
        let Some(d) = &self.drawing else { return };
        if d.pitch.iter().all(Option::is_none) {
            return;
        }
        let data = library::render_drawing(d);
        let n = lib.entries.iter().filter(|e| e.name.starts_with(self.draw_name)).count() + 1;
        let id = lib.add(&format!("{} {n}", self.draw_name), self.draw_voice, data);
        lib.set_draft(id, true);
        self.stop(out);
        self.finish_with(id, Step::Draw);
    }

    /// Open the board with a track's template (for the snapshots)
    #[cfg(test)]
    pub fn draw_template(&mut self, v: Voice) {
        self.open = true;
        self.start_drawing(Some((library::template(v), template_name(v))), v, true);
    }

    pub(super) fn start_drawing(&mut self, from: Option<(library::Drawing, &'static str)>, voice: Voice, template: bool) {
        let (d, name) = from.unwrap_or((library::Drawing::default(), "Drawing"));
        self.template = template.then(|| d.clone());
        self.drawing = Some(d);
        self.draw_name = name;
        self.draw_voice = voice;
        self.draw_undo.clear();
        self.draw_redo.clear();
        self.tool = Tool::Pen;
        self.pen = None;
        self.flow = Some(Step::Draw);
    }

    /// Hear the drawing as it is now
    pub(super) fn play_drawing(&mut self, out: &mut Vec<Out>, now: f64) {
        let Some(d) = &self.drawing else { return };
        if d.pitch.iter().all(Option::is_none) {
            return;
        }
        let buf = Arc::new(SampleBuf {
            data: library::render_drawing(d),
            pitch: 0.0,
        });
        out.push(Out::Audition(Some(buf.clone()), self.looped));
        self.playing = Some(Playing {
            id: 0,
            buf,
            from: 0.0,
            to: d.length,
            reverse: false,
            whole: false,
            since: now,
        });
    }

    pub(super) fn draw_undo_step(&mut self, back: bool) {
        let Some(d) = self.drawing.clone() else { return };
        let (from, to) = if back {
            (&mut self.draw_undo, &mut self.draw_redo)
        } else {
            (&mut self.draw_redo, &mut self.draw_undo)
        };
        if let Some(prev) = from.pop() {
            to.push(d);
            self.drawing = Some(prev);
        }
    }

    fn draw_checkpoint(&mut self, d: &library::Drawing) {
        self.draw_undo.push(d.clone());
        if self.draw_undo.len() > 60 {
            self.draw_undo.remove(0);
        }
        self.draw_redo.clear();
    }

    /// The drawing board: paint a line (higher is higher, gaps are silence) and under it how
    /// loud it is; pick the track, how it sounds and how long it is, hear it, keep it
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_board(&mut self, ui: &mut Ui, r: Rect, playhead: Option<f64>, blocked: bool, out: &mut Vec<Out>, now: f64) {
        let Some(mut d) = self.drawing.clone() else { return };
        let before = d.clone();
        let col = track_color(self.draw_voice);
        let p = ui.painter().clone();
        let inner = r.shrink(18.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
        // The name, and the track it's for
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.draw_name).font(bold(22.0)).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for &v in Voice::ALL.iter().rev() {
                    if crate::widgets::chip_sized(ui, v.name(), self.draw_voice == v, track_color(v), None)
                        .on_hover_text(format!("It goes on the {} track", v.name()))
                        .clicked()
                    {
                        self.draw_voice = v;
                    }
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new("FOR").font(bold(11.0)).color(FAINT));
            });
        });

        // The board: the line on top, its loudness below
        let board_h = (ui.available_height() - 180.0).clamp(200.0, 470.0);
        let (board, resp) = ui.allocate_exact_size(vec2(inner.width(), board_h), Sense::click_and_drag());
        let lane_gap = 8.0;
        let loud_h = (board_h * 0.26).clamp(56.0, 120.0);
        let top = Rect::from_min_max(board.min, pos2(board.right(), board.bottom() - loud_h - lane_gap));
        let low = Rect::from_min_max(pos2(board.left(), board.bottom() - loud_h), board.max);
        p.rect_filled(top, 14.0, SPACE);
        p.rect_filled(low, 12.0, SPACE);
        for k in 1..7 {
            let y = top.bottom() - top.height() * k as f32 / 7.0;
            p.line_segment(
                [pos2(top.left() + 8.0, y), pos2(top.right() - 8.0, y)],
                stroke(1.0, alpha(LINE, 0.25)),
            );
        }
        p.text(top.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, "PITCH", bold(10.5), FAINT);
        p.text(low.left_top() + vec2(12.0, 8.0), Align2::LEFT_TOP, "LOUDNESS", bold(10.5), FAINT);
        p.text(
            top.right_top() + vec2(-12.0, 10.0),
            Align2::RIGHT_TOP,
            "higher is higher · gaps are silence · right button erases",
            body(11.0),
            FAINT,
        );
        let n = d.pitch.len();
        let col_x = |c: usize| board.left() + (c as f32 + 0.5) / n as f32 * board.width();
        let val_y = |v: f32| top.bottom() - 10.0 - v * (top.height() - 20.0);
        let lvl_y = |v: f32| low.bottom() - 6.0 - v * (low.height() - 26.0);
        // Loudness: bars where the line sounds, a faint outline where it's silent
        let bar_w = (board.width() / n as f32).max(1.0);
        for c in 0..n {
            let x = col_x(c);
            let y = lvl_y(d.level[c]);
            let on = d.pitch[c].is_some();
            p.line_segment(
                [pos2(x, low.bottom() - 6.0), pos2(x, y)],
                stroke(bar_w, alpha(col, if on { 0.38 } else { 0.08 })),
            );
        }
        let pts: Vec<Pos2> = (0..n).map(|c| pos2(col_x(c), lvl_y(d.level[c]))).collect();
        p.add(Shape::line(pts, stroke(1.5, alpha(col, 0.9))));
        // The line: one stroke per run of drawn columns, with a soft glow under it
        let mut runs: Vec<Vec<Pos2>> = Vec::new();
        let mut run = Vec::new();
        for (c, v) in d.pitch.iter().enumerate() {
            match v {
                Some(v) => run.push(pos2(col_x(c), val_y(*v))),
                None if !run.is_empty() => runs.push(std::mem::take(&mut run)),
                None => {}
            }
        }
        if !run.is_empty() {
            runs.push(run);
        }
        for run in runs {
            if run.len() == 1 {
                p.circle_filled(run[0], 3.0, col);
                continue;
            }
            p.add(Shape::line(run.clone(), stroke(9.0, mix(SPACE, col, 0.18))));
            p.add(Shape::line(run, stroke(3.0, col)));
        }
        if d.pitch.iter().all(Option::is_none) {
            p.text(
                top.center(),
                Align2::CENTER_CENTER,
                format!("{}  draw here", ph::PENCIL_SIMPLE),
                bold(16.0),
                FAINT,
            );
        }
        // The playhead while it plays
        if let (Some(pl), Some(pos)) = (&self.playing, playhead) {
            if pl.id == 0 {
                let x = board.left() + (pos as f32 / SAMPLE_SR / d.length).min(1.0) * board.width();
                p.line_segment([pos2(x, board.top()), pos2(x, board.bottom())], stroke(1.5, TEXT));
            }
        }
        let mut stroke_done = false;
        if !blocked {
            let erase = self.tool == Tool::Erase || ui.input(|i| i.pointer.secondary_down());
            if let Some(pos) = resp
                .interact_pointer_pos()
                .filter(|_| resp.is_pointer_button_down_on() || resp.dragged())
            {
                // the lane is the one the stroke began in
                let lane = self.pen.map_or(u8::from(pos.y > top.bottom() + lane_gap / 2.0), |(_, _, l)| l);
                if self.pen.is_none() {
                    self.draw_checkpoint(&d);
                }
                let c = (((pos.x - board.left()) / board.width() * n as f32) as isize).clamp(0, n as isize - 1) as usize;
                let v = if lane == 0 {
                    (top.bottom() - 10.0 - pos.y) / (top.height() - 20.0)
                } else {
                    (low.bottom() - 6.0 - pos.y) / (low.height() - 26.0)
                }
                .clamp(0.0, 1.0);
                let (c0, v0, _) = self.pen.unwrap_or((c, v, lane));
                let (a, b) = if c0 <= c { (c0, c) } else { (c, c0) };
                for k in a..=b {
                    let f = if b == a { 1.0 } else { (k - a) as f32 / (b - a) as f32 };
                    let f = if c0 <= c { f } else { 1.0 - f };
                    let x = v0 + (v - v0) * f;
                    if lane == 0 {
                        d.pitch[k] = if erase { None } else { Some(x) };
                    } else {
                        d.level[k] = if erase { 0.0 } else { x };
                    }
                }
                self.pen = Some((c, v, lane));
            } else if self.pen.take().is_some() {
                stroke_done = true;
            }
            resp.on_hover_cursor(if self.tool == Tool::Erase {
                egui::CursorIcon::NotAllowed
            } else {
                egui::CursorIcon::Crosshair
            });
        }

        // Play, loop; the pen and the eraser; undo, reset, clear; cancel or keep it
        let mut play = stroke_done;
        let mut replay = false;
        let mut undo = None;
        ui.horizontal(|ui| {
            let playing = self.playing.as_ref().is_some_and(|pl| pl.id == 0);
            let (pr, presp) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::click());
            ui.painter().circle_filled(
                pr.center(),
                23.0,
                if presp.hovered() || playing { col } else { mix(PANEL, col, 0.6) },
            );
            ui.painter().text(
                pr.center(),
                Align2::CENTER_CENTER,
                if playing { ph::STOP } else { ph::PLAY },
                body(20.0),
                BG,
            );
            if presp
                .on_hover_text("Hear it (Space)")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                || self.space
            {
                if playing {
                    self.stop(out);
                } else {
                    play = true;
                }
            }
            if chip(ui, &format!("{}  Loop", ph::REPEAT), self.looped, col)
                .on_hover_text("Over and over – you hear every stroke at once")
                .clicked()
            {
                self.looped = !self.looped;
                replay = playing;
            }
            ui.add_space(10.0);
            if chip(ui, &format!("{}  Pen", ph::PENCIL_SIMPLE), self.tool == Tool::Pen, col)
                .on_hover_text("Paint the line and its loudness")
                .clicked()
            {
                self.tool = Tool::Pen;
            }
            if chip(ui, &format!("{}  Eraser", ph::ERASER), self.tool == Tool::Erase, col)
                .on_hover_text("Wipe out a part – silence (the right mouse button erases too)")
                .clicked()
            {
                self.tool = Tool::Erase;
            }
            ui.add_space(10.0);
            ui.add_enabled_ui(!self.draw_undo.is_empty(), |ui| {
                if chip(ui, ph::ARROW_COUNTER_CLOCKWISE, false, TEXT)
                    .on_hover_text("Undo (⌘Z)")
                    .clicked()
                {
                    undo = Some(true);
                }
            });
            ui.add_enabled_ui(!self.draw_redo.is_empty(), |ui| {
                if chip(ui, ph::ARROW_CLOCKWISE, false, TEXT).on_hover_text("Redo (⇧⌘Z)").clicked() {
                    undo = Some(false);
                }
            });
            if let Some(t) = self.template.clone() {
                if chip(ui, &format!("{}  Reset", ph::ARROWS_COUNTER_CLOCKWISE), false, TEXT)
                    .on_hover_text("Back to the template")
                    .clicked()
                    && d != t
                {
                    self.draw_checkpoint(&d);
                    d = t;
                    play = true;
                }
            }
            if chip(ui, &format!("{}  Clear", ph::BROOM), false, TEXT)
                .on_hover_text("Start again on an empty board")
                .clicked()
            {
                self.draw_checkpoint(&d);
                d.pitch = vec![None; n];
                d.level = vec![1.0; n];
                self.stop(out);
            }
        });

        // How it sounds and how long: the editor's tile size
        ui.add_space(2.0);
        let gap = 10.0;
        let tw = ((inner.width() - gap * 4.0) / 5.0).max(80.0);
        let th = ui.available_height().clamp(76.0, 104.0);
        let size = vec2(tw, th);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (w, name) in [(0u8, "SOFT"), (1, "BRIGHT"), (2, "AIR")] {
                if action_button(ui, size, Tile::Wave(w), ph::WAVE_SINE, name, d.wave == w, 0.0, col).clicked() {
                    d.wave = w;
                    play = true;
                }
            }
            // LENGTH as a slider: click or drag along it
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let hot = resp.hovered() || resp.dragged();
            let p = ui.painter_at(rect);
            let area = crate::controls::card(&p, rect, "LENGTH", &fmt_secs(d.length), col, hot);
            if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
                let v = ((pos.x - area.left()) / area.width()).clamp(0.0, 1.0);
                d.length = 0.05 + v * 3.95;
            }
            if resp.double_clicked() {
                d.length = 1.0;
            }
            picture(&p, area, Tile::Length, ((d.length - 0.05) / 3.95).clamp(0.0, 1.0), col, 0.0);
            let resp = resp
                .on_hover_text("Click or drag: shorter to the left, longer to the right · double-click: 1 s")
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            if resp.drag_started() {
                self.draw_checkpoint(&before);
            }
            if resp.drag_stopped() || resp.clicked() {
                play = true;
            }
        });
        if replay {
            play = true;
        }
        if self.drawing.is_none() {
            return;
        }
        if d.wave != before.wave {
            // a wave tile is one step back
            self.draw_checkpoint(&before);
        }
        self.drawing = Some(d);
        if let Some(back) = undo {
            self.draw_undo_step(back);
            play = true;
        }
        if play {
            self.play_drawing(out, now);
        }
    }
}
