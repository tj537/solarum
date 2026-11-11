//! The library and its editor: the list of sounds (filtered by track, the project's slots)
//! and the editor – the waveform you cut by its edges and fades, and the picture tiles.

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Handle {
    Start,
    End,
    FadeIn,
    FadeOut,
    Pan,
}

impl LibraryUi {
    pub(super) fn list(&mut self, ui: &mut Ui, r: Rect, lib: &mut Library, slots: &mut SlotIds, out: &mut Vec<Out>, now: f64) {
        let inner = r.shrink(12.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        // Filter by track: the track colours (only the tracks that have sounds)
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
            if chip(ui, "All", self.filter.is_none(), TEXT).clicked() {
                self.filter = None;
            }
            for v in Voice::ALL {
                let has = lib.entries.iter().any(|e| !e.draft && e.voice == v) || self.filter == Some(v);
                if has && chip(ui, v.name(), self.filter == Some(v), track_color(v)).clicked() {
                    self.filter = if self.filter == Some(v) { None } else { Some(v) };
                }
            }
        });
        if lib.entries.len() > 10 {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(format!("{}  Search", ph::MAGNIFYING_GLASS))
                    .desired_width(f32::INFINITY),
            );
        }
        let q = self.search.to_lowercase();
        let shown: Vec<u64> = lib
            .entries
            .iter()
            .filter(|e| !e.draft && self.filter.is_none_or(|v| e.voice == v) && (q.is_empty() || e.name.to_lowercase().contains(&q)))
            .map(|e| e.id)
            .collect();
        let list_h = ui.available_height().max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_h)
            .min_scrolled_height(list_h)
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                if shown.is_empty() {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new(if lib.entries.is_empty() {
                            "No sounds yet – make one on the right"
                        } else {
                            "No sounds on this track yet"
                        })
                        .font(body(13.0))
                        .color(FAINT),
                    );
                }
                for id in &shown {
                    self.row(ui, lib, slots, *id, out, now);
                }
            });
    }

    fn row(&mut self, ui: &mut Ui, lib: &mut Library, slots: &SlotIds, id: u64, out: &mut Vec<Out>, now: f64) {
        let Some(e) = lib.get(id).cloned() else { return };
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
        let p = ui.painter();
        let col = track_color(e.voice);
        let sel = self.selected == Some(id);
        let hot = resp.hovered();
        p.rect_filled(
            r,
            10.0,
            if sel {
                mix(PANEL, col, 0.16)
            } else if hot {
                PANEL_HI
            } else {
                PANEL
            },
        );
        if sel {
            p.rect_stroke(r, 10.0, stroke(1.0, alpha(col, 0.8)), egui::StrokeKind::Inside);
        }
        // Play button in the track's colour
        let play_r = Rect::from_center_size(pos2(r.left() + 26.0, r.center().y), vec2(30.0, 30.0));
        let playing = self.playing.as_ref().is_some_and(|pl| pl.id == id);
        let play = ui.interact(play_r, ui.id().with(("play", id)), Sense::click());
        p.circle_filled(
            play_r.center(),
            15.0,
            if play.hovered() || playing { col } else { mix(PANEL, col, 0.45) },
        );
        p.text(
            play_r.center(),
            Align2::CENTER_CENTER,
            if playing { ph::STOP } else { ph::PLAY },
            body(14.0),
            if play.hovered() || playing { BG } else { TEXT },
        );
        if play.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.drawing = None;
            self.selected = Some(id);
            self.toggle_play(lib, id, out, now);
        }
        // Name and length
        let text_l = r.left() + 50.0;
        let wave_w = 64.0;
        let name = fit_text(p, &e.name, bold(13.0), r.width() - 50.0 - wave_w - 16.0);
        p.text(pos2(text_l, r.center().y - 9.0), Align2::LEFT_CENTER, name, bold(13.0), TEXT);
        let src = lib.source(&e).cloned();
        let len = src.as_ref().map_or(0.0, |s| {
            let (a, b) = kept(s, &e.edit);
            b - a
        });
        p.text(
            pos2(text_l, r.center().y + 10.0),
            Align2::LEFT_CENTER,
            format!("{} · {}", e.voice.name(), fmt_secs(len)),
            body(11.0),
            DIM,
        );
        let wr = Rect::from_min_size(pos2(r.right() - wave_w - 10.0, r.top() + 12.0), vec2(wave_w, r.height() - 24.0));
        if let Some(src) = &src {
            let (a, b) = kept(src, &e.edit);
            draw_wave(p, wr, src, a, b, alpha(col, 0.8));
        }
        // In this project: a small light
        if slots.contains(&id) {
            p.circle_filled(pos2(r.right() - 9.0, r.top() + 9.0), 4.0, col);
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.selected = Some(id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn editor(
        &mut self,
        ui: &mut Ui,
        r: Rect,
        lib: &mut Library,
        slots: &mut SlotIds,
        id: u64,
        playhead: Option<f64>,
        blocked: bool,
        out: &mut Vec<Out>,
        now: f64,
    ) {
        let Some(orig) = lib.get(id).cloned() else { return };
        let Some(src) = lib.source(&orig).cloned() else {
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                "The audio of this sound is missing",
                bold(14.0),
                DIM,
            );
            return;
        };
        let mut e = orig.clone();
        let len = src.seconds().max(1e-3);
        // The waveform shows the sound as it plays: the pitch squeezes (higher) or stretches
        // (lower) the whole picture – cut, bars and the rest – around the start
        let (ks, ke) = kept(&src, &e.edit);
        let ratio = 2f32.powf(-e.edit.pitch / 12.0);
        let disp = move |t: f32| ks + (t - ks) * ratio;
        let undisp = move |d: f32| ks + (d - ks) / ratio;
        // In view at most: the sound as it is and as it plays; fitted: as it is, so a higher pitch
        // leaves room at the end and a lower one runs past it
        let (d_lo, d_hi) = (disp(0.0).min(0.0), disp(len).max(len));
        if self.view_for != id {
            self.view_for = id;
            self.fit = true;
            self.drag = None;
        }
        if self.fit {
            self.view = (0.0, len);
        }
        let col = track_color(e.voice);
        let p = ui.painter().clone();
        let inner = r.shrink(18.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);

        // ── Name and delete ──
        ui.horizontal(|ui| {
            let name = ui.add(
                egui::TextEdit::singleline(&mut e.name)
                    .font(bold(22.0))
                    .desired_width(inner.width() - 60.0)
                    .frame(false),
            );
            if name.gained_focus() {
                self.checkpoint(&orig);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !e.draft && chip(ui, ph::TRASH, false, RED).on_hover_text("Delete").clicked() {
                    self.confirm_delete = Some(id);
                }
            });
        });

        // ── The track it plays on: the eight colours ──
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for v in Voice::ALL {
                let c = track_color(v);
                let on = e.voice == v;
                let g = ui.painter().layout_no_wrap(v.name().to_string(), bold(12.0), TEXT);
                let (rr, resp) = ui.allocate_exact_size(vec2(g.size().x + 26.0, 30.0), Sense::click());
                let hot = resp.hovered();
                ui.painter().rect_filled(
                    rr,
                    15.0,
                    if on {
                        c
                    } else if hot {
                        mix(PANEL, c, 0.35)
                    } else {
                        mix(PANEL, c, 0.14)
                    },
                );
                ui.painter().text(
                    rr.center(),
                    Align2::CENTER_CENTER,
                    v.name(),
                    bold(12.0),
                    if on { BG } else { mix(c, TEXT, 0.3) },
                );
                if resp
                    .on_hover_text("Plays on this track")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                    && !on
                {
                    self.checkpoint(&orig);
                    e.voice = v;
                }
            }
        });

        // ── The waveform ──
        let zoomed = !self.fit;
        let over_h = if zoomed { 22.0 } else { 0.0 };
        let wave_h = (ui.available_height() - 250.0).clamp(120.0, 380.0);
        let (area, _) = ui.allocate_exact_size(
            vec2(inner.width(), wave_h + over_h + if zoomed { 6.0 } else { 0.0 }),
            Sense::hover(),
        );
        let wave = Rect::from_min_size(area.min, vec2(area.width(), wave_h));
        let over = Rect::from_min_size(pos2(area.left(), wave.bottom() + 6.0), vec2(area.width(), over_h));
        let (v0, v1) = self.view;
        let span = (v1 - v0).max(1e-4);
        let to_x = |t: f32| wave.left() + (disp(t) - v0) / span * wave.width();
        let to_d = |x: f32| v0 + (x - wave.left()) / wave.width() * span;
        let to_t = |x: f32| undisp(to_d(x));

        p.rect_filled(wave, 14.0, SPACE);
        let clip = p.with_clip_rect(wave.shrink(1.0));
        // Outside the cut: the rest of the source, levelled like the cut so both match
        let gain = level_of(&src, &e.edit);
        draw_wave_gain(&clip, wave, &src, undisp(v0), undisp(v1), alpha(FAINT, 0.3), gain);
        let kr = Rect::from_min_max(
            pos2(to_x(ks).max(wave.left()), wave.top()),
            pos2(to_x(ke).min(wave.right()), wave.bottom()),
        );
        // Inside the cut: the sound as it plays – reversed, filtered, crushed, levelled, faded –
        // always exactly between start and end
        let shown_end = ke;
        if kr.width() > 0.0 {
            clip.rect_filled(kr, 0.0, mix(SPACE, col, 0.07));
            match self.rendered_source(lib, id) {
                Some(rs) => {
                    let sr = Rect::from_min_max(pos2(to_x(ks), wave.top()), pos2(to_x(shown_end), wave.bottom()));
                    let vis = sr.intersect(wave);
                    if vis.width() > 0.5 {
                        // Only the columns in view
                        let len_r = rs.seconds();
                        let f0 = (vis.left() - sr.left()) / sr.width().max(1e-3) * len_r;
                        let f1 = (vis.right() - sr.left()) / sr.width().max(1e-3) * len_r;
                        draw_wave(&clip.with_clip_rect(kr.union(vis)), vis, &rs, f0, f1, col);
                    }
                }
                None => draw_wave(&clip.with_clip_rect(kr), wave, &src, undisp(v0), undisp(v1), col),
            }
        }
        // Fades: grabbed along the top edge inside the cut (the wave itself shows them)
        let fi = to_x(ks + e.edit.fade_in);
        let fo = to_x(ke - e.edit.fade_out);
        let fade_band = |pos: Pos2| pos.y < wave.top() + 30.0 && pos.x > to_x(ks) + 8.0 && pos.x < to_x(ke) - 8.0;
        // Start and end: bright bars with a grip
        for t in [ks, ke] {
            let x = to_x(t);
            if x < wave.left() - 1.0 || x > wave.right() + 1.0 {
                continue;
            }
            clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(2.0, TEXT));
            clip.rect_filled(Rect::from_center_size(pos2(x, wave.center().y), vec2(8.0, 34.0)), 4.0, TEXT);
        }
        // The playhead
        if let (Some(pl), Some(pos)) = (&self.playing, playhead) {
            if pl.id == id {
                let dt = pos as f32 / SAMPLE_SR;
                // The whole sound is drawn as it plays (reversed too); a slice on the source
                let t = if pl.whole {
                    pl.from + dt
                } else if pl.reverse {
                    pl.to - dt
                } else {
                    pl.from + dt
                };
                let x = to_x(t);
                if (wave.left()..=wave.right()).contains(&x) {
                    clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.5, ACID));
                }
            }
        }
        p.text(
            wave.right_bottom() + vec2(-12.0, -10.0),
            Align2::RIGHT_BOTTOM,
            fmt_secs((ke - ks) * ratio),
            bold(12.0),
            alpha(TEXT, 0.7),
        );
        // While zoomed in: the whole sound below, with the view in it
        if zoomed {
            p.rect_filled(over, 6.0, SPACE);
            draw_wave(&p.with_clip_rect(over), over, &src, 0.0, len, alpha(col, 0.5));
            let ox = |t: f32| over.left() + t / len * over.width();
            let (s0, s1) = (undisp(v0), undisp(v1));
            let vr = Rect::from_min_max(pos2(ox(s0), over.top()), pos2(ox(s1).max(ox(s0) + 3.0), over.bottom()));
            p.rect_stroke(vr, 4.0, stroke(1.2, TEXT), egui::StrokeKind::Inside);
        }

        // ── Interaction with the waveform ──
        if !blocked {
            let resp = ui.interact(wave, ui.id().with(("wave", id)), Sense::click_and_drag());
            let near = |pos: Pos2| -> Option<Handle> {
                if fade_band(pos) {
                    // The nearer of the two fades
                    return Some(if (fi - pos.x).abs() <= (fo - pos.x).abs() {
                        Handle::FadeIn
                    } else {
                        Handle::FadeOut
                    });
                }
                if (to_x(ks) - pos.x).abs() < 8.0 {
                    return Some(Handle::Start);
                }
                if (to_x(ke) - pos.x).abs() < 8.0 {
                    return Some(Handle::End);
                }
                None
            };
            if let Some(pos) = resp.hover_pos() {
                // Say what a drag does where the mouse is
                let hint = match near(pos) {
                    Some(Handle::Start) => Some("Start – drag to cut"),
                    Some(Handle::End) => Some("End – drag to cut"),
                    Some(Handle::FadeIn) => Some("Fade in – drag"),
                    Some(Handle::FadeOut) => Some("Fade out – drag"),
                    _ => None,
                };
                if let (Some(h), None) = (hint, self.drag) {
                    let g = p.layout_no_wrap(h.to_string(), bold(11.0), TEXT);
                    let at = pos2(pos.x + 14.0, pos.y + 16.0);
                    let br = Rect::from_min_size(at, g.size() + vec2(14.0, 8.0));
                    p.rect_filled(br, 6.0, Color32::from_rgb(20, 20, 24));
                    p.galley(br.min + vec2(7.0, 4.0), g, TEXT);
                }
                if fade_band(pos) && self.drag.is_none() {
                    // A faint line where the fades end
                    for x in [fi, fo] {
                        clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.0, alpha(TEXT, 0.35)));
                    }
                }
                match near(pos) {
                    Some(_) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
                    None => ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand),
                }
                // Scroll zooms around the mouse; sideways scroll pans
                let (dx, dy, zoom) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y, i.zoom_delta()));
                let f = ((-dy * 0.004).exp() / zoom).clamp(0.5, 2.0);
                if (f - 1.0).abs() > 1e-4 {
                    let t = to_d(pos.x);
                    let ns = (span * f).clamp(0.004, d_hi - d_lo);
                    let a = t - (t - v0) * ns / span;
                    self.view = (a, a + ns);
                    self.fit = false;
                }
                if dx.abs() > 0.1 {
                    let d = -dx / wave.width() * span;
                    self.view = (self.view.0 + d, self.view.1 + d);
                }
            }
            if resp.drag_started() {
                let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
                self.drag = Some(near(origin).unwrap_or(Handle::Pan));
                if self.drag != Some(Handle::Pan) {
                    self.checkpoint(&orig);
                }
            }
            if resp.dragged() {
                let pos = resp.interact_pointer_pos().unwrap_or_default();
                let t = to_t(pos.x).clamp(0.0, len);
                let min_len = 0.005;
                match self.drag {
                    Some(Handle::Start) => e.edit.start = t.min(ke - min_len).max(0.0),
                    Some(Handle::End) => e.edit.end = if t >= len - 1e-4 { 0.0 } else { t.max(ks + min_len) },
                    Some(Handle::FadeIn) => e.edit.fade_in = (t - ks).clamp(0.0, (ke - ks) * 0.9),
                    Some(Handle::FadeOut) => e.edit.fade_out = (ke - t).clamp(0.0, (ke - ks) * 0.9),
                    Some(Handle::Pan) => {
                        let d = -resp.drag_delta().x / wave.width() * span;
                        self.view = (self.view.0 + d, self.view.1 + d);
                        self.fit = false;
                    }
                    None => {}
                }
            }
            if resp.drag_stopped() {
                self.drag = None;
            }
            if resp.double_clicked() {
                self.fit = true;
            } else if resp.clicked() {
                self.play(lib, id, None, out, now);
            }
            if zoomed {
                let oresp = ui.interact(over, ui.id().with(("over", id)), Sense::click_and_drag());
                if let Some(pos) = oresp.interact_pointer_pos().filter(|_| oresp.dragged() || oresp.clicked()) {
                    let c = disp((pos.x - over.left()) / over.width() * len);
                    self.view = (c - span / 2.0, c + span / 2.0);
                }
            }
        }
        let s = (self.view.1 - self.view.0).clamp(0.004, d_hi - d_lo);
        let a = self.view.0.clamp(d_lo, (d_hi - s).max(d_lo));
        self.view = (a, a + s);

        // ── Play, loop, save or use it ──
        ui.horizontal(|ui| {
            let playing = self.playing.as_ref().is_some_and(|pl| pl.id == id && pl.whole);
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
                .on_hover_text("Play (Space)")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.toggle_play(lib, id, out, now);
            }
            if chip(ui, &format!("{}  Loop", ph::REPEAT), self.looped, col)
                .on_hover_text("Over and over – you hear every change at once")
                .clicked()
            {
                self.looped = !self.looped;
                if playing {
                    self.play(lib, id, None, out, now);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if e.draft {
                    return; // the flow's bar has Save
                }
                let slot = slots.iter().position(|&s| s == id);
                let label = match slot {
                    Some(_) => format!("{}  Playing on {}", ph::CHECK, e.voice.name()),
                    None => format!("{}  Use on {}", ph::ARROW_RIGHT, e.voice.name()),
                };
                let g = ui.painter().layout_no_wrap(label.clone(), bold(14.0), BG);
                let (br, bresp) = ui.allocate_exact_size(g.size() + vec2(36.0, 22.0), Sense::click());
                let hot = bresp.hovered();
                ui.painter()
                    .rect_filled(br, br.height() / 2.0, if hot { Color32::WHITE } else { ACID });
                ui.painter().galley(br.center() - g.size() / 2.0, g, BG);
                if bresp
                    .on_hover_text(format!("Puts it on the {} track", e.voice.name()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    out.push(Out::Use(id));
                }
            });
        });

        // ── What you can do to it: cards for values, buttons for actions (as in the sound section) ──
        ui.add_space(2.0);
        let gap = 10.0;
        let tw = ((inner.width() - gap * 4.0) / 5.0).max(80.0);
        let th = ui.available_height().clamp(76.0, 104.0);
        let size = vec2(tw, th);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let ed = &mut e.edit;
            let changed = |r: &egui::Response, me: &mut Self| {
                if r.drag_started() || r.double_clicked() || (r.hovered() && ui_scrolled(r)) {
                    me.checkpoint(&orig);
                }
            };
            // PITCH: up higher, down lower
            let mut v = (ed.pitch + 24.0) / 48.0;
            let r = value_card(
                ui,
                size,
                Tile::Pitch,
                "PITCH",
                &format!("{:+.0}", ed.pitch),
                &mut v,
                0.5,
                |v| v * 48.0 - 24.0,
                col,
            )
            .on_hover_text("Drag up: higher · down: lower · double-click: back");
            changed(&r, self);
            if r.changed() {
                ed.pitch = (v * 48.0 - 24.0).round();
            }
            // TONE: up thinner, down darker
            let tone = tone_of(ed);
            let mut v = (tone + 1.0) / 2.0;
            let label = if tone < -0.01 {
                "dark"
            } else if tone > 0.01 {
                "thin"
            } else {
                "open"
            };
            let r = value_card(ui, size, Tile::Tone, "TONE", label, &mut v, 0.5, |v| v * 2.0 - 1.0, col)
                .on_hover_text("Drag up: thinner (less bass) · down: darker · double-click: open");
            changed(&r, self);
            if r.changed() {
                set_tone(ed, v * 2.0 - 1.0);
            }
            // CRUSH: up grittier
            let mut v = ed.crush;
            let r = value_card(
                ui,
                size,
                Tile::Crush,
                "CRUSH",
                &format!("{:.0}", ed.crush * 100.0),
                &mut v,
                0.0,
                |v| v,
                col,
            )
            .on_hover_text("Drag up: grittier, like an old sampler · double-click: clean");
            changed(&r, self);
            if r.changed() {
                ed.crush = v;
            }
            // REVERSE, TRIM, CHOP: buttons like the live effects
            if action_button(
                ui,
                size,
                Tile::Reverse,
                ph::ARROWS_LEFT_RIGHT,
                "REVERSE",
                ed.reverse,
                ed.reverse as u8 as f32,
                col,
            )
            .on_hover_text("Play it backwards")
            .clicked()
            {
                self.checkpoint(&orig);
                ed.reverse = !ed.reverse;
            }
        });
        if e != orig {
            if let Some(slot) = lib.get_mut(id) {
                *slot = e;
            }
            self.unsaved = true;
        }
    }
}

/// How much the edit makes the source louder or softer (normalize or gain)
fn level_of(src: &Source, e: &Edit) -> f32 {
    if !e.normalize {
        return 10f32.powf(e.gain / 20.0);
    }
    let (a, b) = library::span(src.data.len(), e);
    let peak = src.data[a..b].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if peak > 1e-5 {
        0.89 / peak
    } else {
        1.0
    }
}

/// The tone tile's value from the edit: -1 (dark, high cut down) … 0 (open) … 1 (thin, low cut up)
fn tone_of(e: &Edit) -> f32 {
    if e.low_cut > 25.0 {
        ((e.low_cut / 20.0).ln() / 100f32.ln()).clamp(0.0, 1.0)
    } else if e.high_cut < 19_000.0 {
        -(1.0 - (e.high_cut / 300.0).ln() / (19_000f32 / 300.0).ln()).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn set_tone(e: &mut Edit, v: f32) {
    let v = if v.abs() < 0.04 { 0.0 } else { v.clamp(-1.0, 1.0) };
    e.low_cut = if v > 0.0 { 20.0 * 100f32.powf(v) } else { 20.0 };
    e.high_cut = if v < 0.0 {
        300.0 * (19_000f32 / 300.0).powf(1.0 + v)
    } else {
        20_000.0
    };
}
