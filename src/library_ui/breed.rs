//! Breeding: two sounds of a track make a family of children to pick from (and the sound
//! of the day).

use super::*;

/// Breeding a sound: four children of your last pick, every round a little calmer
pub(super) struct Breed {
    pub(super) voice: Voice,
    /// Your pick of every round so far – the last one is yours (none yet: the track's own)
    pub(super) picks: Vec<library::Genes>,
    pub(super) kids: [library::Genes; 4],
    pub(super) bufs: [Arc<SampleBuf>; 4],
    pub(super) rng: crate::rng::Rng,
    /// The card the pointer is on – it plays once when the pointer gets there
    pub(super) hover: Option<usize>,
    /// When these four came (the cards grow in)
    pub(super) born: f64,
}

impl Breed {
    fn new(voice: Voice, rng: crate::rng::Rng, now: f64) -> Breed {
        let g = library::Genes::of(voice);
        let mut b = Breed {
            voice,
            picks: Vec::new(),
            kids: [g; 4],
            bufs: std::array::from_fn(|_| bred_buf(&g)),
            rng,
            hover: None,
            born: now,
        };
        b.litter(now);
        b
    }

    pub(super) fn parent(&self) -> library::Genes {
        self.picks.last().copied().unwrap_or(library::Genes::of(self.voice))
    }

    fn litter(&mut self, now: f64) {
        self.kids = library::litter(&self.parent(), self.picks.len(), &mut self.rng);
        self.bufs = std::array::from_fn(|k| bred_buf(&self.kids[k]));
        self.hover = None;
        self.born = now;
    }

    fn pick(&mut self, k: usize, now: f64) {
        self.picks.push(self.kids[k]);
        self.litter(now);
    }

    /// Take the last pick back: its parent has four new children
    pub(super) fn round_back(&mut self, now: f64) {
        if self.picks.pop().is_some() {
            self.litter(now);
        }
    }
}

impl LibraryUi {
    /// Your last pick becomes a sound (a draft) – the family stays, for Back
    pub(super) fn breed_next(&mut self, lib: &mut Library, out: &mut Vec<Out>) {
        let Some(b) = &self.breed else { return };
        let Some(g) = b.picks.last() else { return };
        let data = library::render_drawing(&g.drawing());
        let base = format!("Bred {}", b.voice.name().to_lowercase());
        let n = lib.entries.iter().filter(|e| e.name.starts_with(&base)).count() + 1;
        let id = lib.add(&format!("{base} {n}"), b.voice, data);
        lib.set_draft(id, true);
        self.stop(out);
        self.finish_with(id, Step::Breed);
    }

    /// Open the breeding step for a track, `rounds` picks in (for the snapshots)
    #[cfg(test)]
    pub fn show_breed(&mut self, v: Voice, rounds: usize) {
        self.open = true;
        self.start_breeding(v, crate::rng::Rng::new(4), 0.0);
        if let Some(b) = self.breed.as_mut() {
            for k in 0..rounds {
                b.pick((k * 3 + 1) % 4, 0.0);
            }
            b.born = -10.0;
        }
    }

    pub(super) fn start_breeding(&mut self, voice: Voice, rng: crate::rng::Rng, now: f64) {
        self.breed = Some(Breed::new(voice, rng, now));
        self.drawing = None;
        self.flow = Some(Step::Breed);
    }

    /// Breeding: four children of your last pick – hover to hear, click the best one; on the
    /// left the sound you have so far and its family line
    pub(super) fn breed_page(&mut self, ui: &mut Ui, r: Rect, playhead: Option<f64>, blocked: bool, out: &mut Vec<Out>, now: f64) {
        let Some(b) = self.breed.as_ref() else { return };
        let voice = b.voice;
        let col = track_color(voice);
        let round = b.picks.len();
        let parent = b.parent();
        let kids = b.kids;
        let age = (now - b.born) as f32;
        let ease = |x: f32| 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3);
        let playing = self.playing.as_ref().map(|pl| pl.id).zip(playhead);
        let p = ui.painter().clone();
        let inner = r.shrink(18.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        if blocked {
            ui.disable();
        }
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);

        // The round, and the track the family is for
        let mut new_voice = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("Round {}", round + 1)).font(bold(26.0)).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for &v in Voice::ALL.iter().rev() {
                    if crate::widgets::chip_sized(ui, v.name(), voice == v, track_color(v), None)
                        .on_hover_text(format!("Breed for the {} track – a new family", v.name()))
                        .clicked()
                        && v != voice
                    {
                        new_voice = Some(v);
                    }
                }
            });
        });
        let sub = if round == 0 {
            "Hover a sound to hear it – click the one you like best. It gets four children."
        } else {
            "Your pick had four children. Click the best one again – or Keep it when you love it."
        };
        ui.label(egui::RichText::new(sub).font(body(13.0)).color(DIM));
        ui.add_space(10.0);
        let area = Rect::from_min_max(pos2(inner.left(), ui.cursor().top()), inner.max);
        let gap = 14.0;
        let left_w = (area.width() * 0.27).clamp(240.0, 330.0);
        let yours = Rect::from_min_size(area.min, vec2(left_w, area.height()));
        let grid = Rect::from_min_max(pos2(yours.right() + gap + 4.0, area.top()), area.max);

        // ── The four children: they roll in, one a moment after the other ──
        if age < 0.4 {
            ui.ctx().request_repaint();
        }
        let cw = (grid.width() - gap) / 2.0;
        let ch = (grid.height() - gap) / 2.0;
        let mut hovered = None;
        let mut picked = None;
        for (k, kid) in kids.iter().enumerate() {
            let slot = Rect::from_min_size(
                grid.min + vec2((k % 2) as f32 * (cw + gap), (k / 2) as f32 * (ch + gap)),
                vec2(cw, ch),
            );
            let resp = ui.interact(slot, ui.id().with(("bred", k)), Sense::click());
            let hot = resp.hovered() && !blocked;
            if hot {
                hovered = Some(k);
                ui.ctx().request_repaint();
            }
            let a = ease((age - k as f32 * 0.06) * 5.0);
            let cr = Rect::from_center_size(slot.center(), slot.size() * (0.94 + 0.06 * a));
            p.rect_filled(cr, 12.0, alpha(if hot { mix(PAD, col, 0.10) } else { PAD }, a));
            p.rect_stroke(
                cr,
                12.0,
                stroke(if hot { 1.5 } else { 1.0 }, alpha(if hot { col } else { LINE }, a)),
                egui::StrokeKind::Inside,
            );
            // its number (the key that plays it), how it differs from your pick, how long it is
            let num = pos2(cr.left() + 26.0, cr.top() + 26.0);
            p.circle_filled(num, 12.0, alpha(if hot { col } else { PANEL_HI }, a));
            p.text(
                num,
                Align2::CENTER_CENTER,
                format!("{}", k + 1),
                bold(12.0),
                alpha(if hot { BG } else { TEXT }, a),
            );
            p.text(
                pos2(num.x + 22.0, num.y),
                Align2::LEFT_CENTER,
                kid.differs(&parent),
                bold(16.0),
                alpha(TEXT, a),
            );
            p.text(
                pos2(cr.right() - 18.0, num.y),
                Align2::RIGHT_CENTER,
                fmt_secs(kid.length),
                body(12.0),
                alpha(FAINT, a),
            );
            let win = Rect::from_min_max(pos2(cr.left() + 12.0, cr.top() + 50.0), pos2(cr.right() - 12.0, cr.bottom() - 42.0));
            p.rect_filled(win, 9.0, alpha(SPACE, a));
            let pic = win.shrink2(vec2(16.0, 12.0));
            thumb_drawing(&p, pic, &kid.drawing(), alpha(col, a));
            if let Some((_, pos)) = playing.filter(|&(id, _)| id == KID_ID + k as u64) {
                let x = pic.left() + (pos as f32 / SAMPLE_SR / kid.length).min(1.0) * pic.width();
                p.line_segment([pos2(x, win.top() + 6.0), pos2(x, win.bottom() - 6.0)], stroke(1.5, TEXT));
            }
            if round == 0 && !hot {
                p.text(
                    pos2(cr.left() + 16.0, cr.bottom() - 21.0),
                    Align2::LEFT_CENTER,
                    format!("{}  Hover to hear", ph::SPEAKER_HIGH),
                    body(12.0),
                    alpha(FAINT, a),
                );
            }
            p.text(
                pos2(cr.right() - 18.0, cr.bottom() - 21.0),
                Align2::RIGHT_CENTER,
                format!("Pick it  {}", ph::ARROW_RIGHT),
                bold(13.0),
                alpha(if hot { col } else { FAINT }, a),
            );
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
                picked = Some(k);
            }
        }

        // ── Yours: the last pick, and its family line ──
        p.rect_filled(yours, 12.0, PAD);
        p.rect_stroke(yours, 12.0, stroke(1.0, LINE), egui::StrokeKind::Inside);
        p.text(yours.left_top() + vec2(16.0, 22.0), Align2::LEFT_CENTER, "YOURS", bold(11.0), FAINT);
        let win = Rect::from_min_size(
            yours.min + vec2(12.0, 40.0),
            vec2(yours.width() - 24.0, (yours.height() * 0.34).clamp(100.0, 200.0)),
        );
        let win_resp = ui.interact(win, ui.id().with("bred-yours"), Sense::click());
        let win_hot = win_resp.hovered() && round > 0 && !blocked;
        p.rect_filled(win, 9.0, SPACE);
        if win_hot {
            p.rect_stroke(win, 9.0, stroke(1.5, col), egui::StrokeKind::Inside);
        }
        let pic = win.shrink2(vec2(14.0, 12.0));
        let caption = if round == 0 {
            thumb_drawing(&p, pic, &parent.drawing(), alpha(col, 0.25));
            p.text(win.center(), Align2::CENTER_CENTER, "Nothing picked yet", bold(14.0), DIM);
            format!("It starts from the {} sound", voice.name().to_lowercase())
        } else {
            thumb_drawing(&p, pic, &parent.drawing(), col);
            if let Some((_, pos)) = playing.filter(|&(id, _)| id == KID_ID + 4) {
                let x = pic.left() + (pos as f32 / SAMPLE_SR / parent.length).min(1.0) * pic.width();
                p.line_segment([pos2(x, win.top() + 6.0), pos2(x, win.bottom() - 6.0)], stroke(1.5, TEXT));
            }
            format!("Picked in round {round}  ·  {}", fmt_secs(parent.length))
        };
        p.text(
            pos2(win.left() + 4.0, win.bottom() + 18.0),
            Align2::LEFT_CENTER,
            caption,
            body(12.0),
            DIM,
        );
        if round > 0 {
            p.text(
                pos2(win.right() - 4.0, win.bottom() + 18.0),
                Align2::RIGHT_CENTER,
                format!("{}  Space", ph::PLAY),
                body(12.0),
                if win_hot { col } else { FAINT },
            );
        }

        // the family line: a dot for every pick, a ring for the four you're hearing now
        let fy = win.bottom() + 58.0;
        p.text(pos2(yours.left() + 16.0, fy), Align2::LEFT_CENTER, "FAMILY", bold(11.0), FAINT);
        let dots_y = fy + 28.0;
        let step = 24.0;
        let fit = ((yours.width() - 40.0) / step) as usize;
        let shown = (round + 1).min(fit.max(2));
        let first = round + 1 - shown;
        let x0 = yours.left() + 22.0;
        for i in 0..shown {
            let c = pos2(x0 + i as f32 * step, dots_y);
            if i + 1 < shown {
                p.line_segment([c, c + vec2(step, 0.0)], stroke(2.0, alpha(col, 0.45)));
            }
            if first + i < round {
                p.circle_filled(c, 6.0, col);
            } else {
                let pulse = 7.0 + (now as f32 * 3.0).sin() * 1.0;
                p.circle_stroke(c, pulse, stroke(2.0, col));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        let gens = match round {
            0 => "The first four – pick one".to_owned(),
            1 => "1 pick so far".to_owned(),
            n => format!("{n} picks so far"),
        };
        p.text(pos2(yours.left() + 16.0, dots_y + 26.0), Align2::LEFT_CENTER, gens, body(12.0), DIM);

        // what you can do with it
        let tools = Rect::from_min_max(pos2(yours.left() + 12.0, yours.bottom() - 128.0), yours.max - vec2(12.0, 12.0));
        let mut paint = false;
        let mut back = false;
        let mut fresh = false;
        let mut tu = ui.new_child(UiBuilder::new().max_rect(tools).layout(egui::Layout::bottom_up(egui::Align::Min)));
        tu.spacing_mut().item_spacing.y = 8.0;
        if chip(&mut tu, &format!("{}  New family", ph::SHUFFLE), false, TEXT)
            .on_hover_text("Start over with four new ones")
            .clicked()
        {
            fresh = true;
        }
        tu.add_enabled_ui(round > 0, |ui| {
            if chip(ui, &format!("{}  One round back", ph::ARROW_COUNTER_CLOCKWISE), false, TEXT)
                .on_hover_text("Take the last pick back (⌘Z)")
                .clicked()
            {
                back = true;
            }
            if chip(ui, &format!("{}  Paint over it", ph::PENCIL_SIMPLE), false, TEXT)
                .on_hover_text("Open it on the drawing board and change it by hand")
                .clicked()
            {
                paint = true;
            }
        });

        // ── What happened ──
        if let Some(v) = new_voice {
            self.stop(out);
            self.start_breeding(v, crate::rng::Rng::from_clock(), now);
            return;
        }
        if paint {
            self.stop(out);
            self.start_drawing(Some((parent.drawing(), template_name(voice))), voice, true);
            return;
        }
        let Some(b) = self.breed.as_mut() else { return };
        let mut hear = None;
        if fresh {
            *b = Breed::new(voice, crate::rng::Rng::from_clock(), now);
        } else if back {
            b.round_back(now);
        } else if let Some(k) = picked {
            b.pick(k, now);
            out.push(Out::Dust(crate::luna::Source::Pick));
            // the pointer is still there: the new child under it waits for the next hover
            b.hover = Some(k);
        } else if hovered != b.hover {
            b.hover = hovered;
            if let Some(k) = hovered {
                hear = Some((KID_ID + k as u64, b.bufs[k].clone()));
            }
        }
        if win_resp.clicked() && round > 0 && !blocked {
            hear = Some((KID_ID + 4, bred_buf(&parent)));
        }
        if fresh || back || picked.is_some() {
            self.stop(out);
        }
        if let Some((id, buf)) = hear {
            self.audition(id, buf, out, now);
        }
    }
}

pub(super) fn bred_buf(g: &library::Genes) -> Arc<SampleBuf> {
    Arc::new(SampleBuf {
        data: library::render_drawing(&g.drawing()),
        pitch: 0.0,
    })
}
