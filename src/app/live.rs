//! The live panel: live effects, drops and recording the set (capture).

use super::*;

impl SolarumApp {
    /// Record the session: everything you hear until it's switched off, then save it as a WAV
    pub(super) fn toggle_capture(&mut self, now: f64) {
        if self.capture {
            self.capture = false;
            self.send(Cmd::Capture(false));
        } else {
            self.recording.clear();
            self.rec_live = true;
            self.rec_sr = self.audio.as_ref().map_or(44_100, |a| a.sample_rate);
            self.capture = true;
            self.send(Cmd::Capture(true));
            if !self.playing() {
                self.say("Recording armed – it starts when you press play", now);
            }
        }
    }

    pub(super) fn drop_now(&mut self, now: f64) {
        if self.playing() {
            self.send(Cmd::Drop);
        } else {
            self.say("Start the beat first – then build up to the drop", now);
        }
    }

    // ───────────── Panels ─────────────

    pub(super) fn live_panel(&mut self, ui: &mut Ui, live: &Live, now: f64) {
        let w = ui.available_width();
        let tall = ui.available_height() > 740.0;
        // Room to breathe between the controls – less of it in a short window
        let air = if tall { 8.0 } else { 3.0 };
        // One gap between every control, across and down
        ui.spacing_mut().item_spacing.y = GAP;
        // Nine knobs, always there: the sound, then pitch and the master's drive, then the EQ
        let caret = if self.sound_open { ph::CARET_DOWN } else { ph::CARET_RIGHT };
        let fold = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("SOUND  {caret}"))
                        .font(bold(11.0))
                        .color(FAINT)
                        .extra_letter_spacing(1.6),
                )
                .sense(Sense::click()),
            )
            .on_hover_text(if self.sound_open { "Fold the knobs away" } else { "Show the knobs" })
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if fold.clicked() {
            self.sound_open = !self.sound_open;
        }
        ui.add_space(6.0);
        // What the controls below need, so the character pad gets the room that's left
        let in_log = self.song.auto && self.song.auto_fx && self.song_form.is_none();
        let gap = ui.spacing().item_spacing.y;
        let energy_h = if in_log {
            0.0
        } else if self.energy_locked() {
            30.0
        } else if tall {
            58.0
        } else {
            50.0
        };
        // With the sound open everything below gets a little more compact
        let (filter_h, bh, drop_h) = match (self.sound_open, tall) {
            (false, true) => (116.0, 58.0, 96.0),
            (false, false) => (88.0, 46.0, 72.0),
            (true, true) => (68.0, 40.0, 56.0),
            (true, false) => (64.0, 36.0, 48.0),
        };
        // LIVE (folded or not) below the sound; what's left over lets LIVE's controls grow a
        // little, each up to a sensible size
        let (card_h, eq_h) = if tall { (50.0, 40.0) } else { (46.0, 36.0) };
        let live_h = if self.live_open {
            energy_h + filter_h + 3.0 * bh + drop_h + 6.0 * gap + 14.0
        } else {
            0.0
        };
        let sound_need = if self.sound_open {
            120.0 + 2.0 * card_h + eq_h + 18.0 + 4.0 * gap
        } else {
            0.0
        };
        // Luna's screen at the bottom (a setting) takes its share first
        let luna_h = match (self.luna_desk, tall) {
            (false, _) => 0.0,
            (true, true) => 140.0,
            (true, false) => 104.0,
        };
        let luna_need = if self.luna_desk { luna_h + gap } else { 0.0 };
        let extra = (ui.available_height() - 30.0 - live_h - sound_need - 2.0 * gap - luna_need).max(0.0);
        let filter_h = filter_h + (extra * 0.3).min(150.0 - filter_h).max(0.0);
        let bh = bh + (extra * 0.45 / 3.0).min(70.0 - bh).max(0.0);
        let drop_h = drop_h + (extra * 0.25).min(120.0 - drop_h).max(0.0);
        let below = 30.0 + energy_h + filter_h + 3.0 * bh + drop_h + 6.0 * gap + 14.0 + 2.0 * gap;
        if self.sound_open {
            let m = self.song.macros;
            let dm = crate::song::Macros::default();
            let xy_h =
                (ui.available_height() - below - 2.0 * card_h - (eq_h + 18.0) - 4.0 * gap).clamp(if tall { 80.0 } else { 64.0 }, 120.0);
            // The auto mode and songs sweep the acid: the puck shows what plays
            let sweeping = (self.song.auto || self.song_form.is_some()) && self.playing();
            let shown_acid = sweeping.then(|| f32::from_bits(self.shared.acid_now.load(Ordering::Relaxed)));
            // Character: dark × acid
            let [mut dark, mut acid] = [m.dark, m.acid];
            controls::character(ui, &mut dark, &mut acid, [dm.dark, dm.acid], shown_acid, w, xy_h)
                .on_hover_text("Drag: up = darker (longer kick, rumble), right = more acid (the synth filter) · double-click resets");
            // Turning up ACID while the synth sits muted: bring it in, or the pad seems broken
            if acid > m.acid + 0.001 && self.song.track(Voice::Acid).mute && self.solo.is_none() {
                self.song.track_mut(Voice::Acid).mute = false;
                self.say(format!("Synth in: {}", self.song.sound_name(Voice::Acid)), now);
            }
            let mm = &mut self.song.macros;
            [mm.dark, mm.acid] = [dark, acid];
            let tr = self.shared.transpose.load(Ordering::Relaxed);
            let pitch_text = if tr != 0 {
                format!("{}{tr:+}", self.song.note_name())
            } else {
                self.song.note_name().to_string()
            };
            // Two by two: space and pump as faders with what they do, pitch on a keyboard,
            // drive as a wave – all dragged up or down
            let cw = (w - GAP) / 2.0;
            let mut space = m.space;
            let mut pump = m.pump;
            let mut semis = self.song.root - 36;
            let mut drive = self.song.eq.drive;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                controls::space_card(ui, &mut space, dm.space, track_color(Voice::OpenHat), cw, card_h)
                    .on_hover_text("Space: reverb and echo on everything\nDrag up or down · double-click resets");
                controls::pump_card(ui, &mut pump, dm.pump, track_color(Voice::Clap), cw, card_h)
                    .on_hover_text("Pump: everything ducks under the kick and breathes with it\nDrag up or down · double-click resets");
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                controls::pitch_card(ui, &mut semis, 5, &pitch_text, track_color(Voice::Bass), cw, card_h).on_hover_text(
                    "Key: what key the whole song is in – bass and synth move with it, drums stay.\nThe planets' own notes are steps of the scale, so the melody stays the same.\nDrag along it · double-click resets",
                );
                controls::drive_card(ui, &mut drive, crate::song::MasterEq::default().drive, RED, cw, card_h)
                    .on_hover_text("Drive: saturation on the whole mix – warmer and louder\nDrag up or down · double-click resets");
            });
            self.song.macros.space = space;
            self.song.macros.pump = pump;
            self.song.root = 36 + semis.min(11);
            self.song.eq.drive = drive;
            // The master EQ: drag the points
            let e = &mut self.song.eq;
            let mut bands = [e.low, e.mid, e.high];
            let colors = [track_color(Voice::Bass), track_color(Voice::Rim), track_color(Voice::Hat)];
            controls::eq_curve(ui, &mut bands, colors, w, eq_h).on_hover_text(
                "Master EQ: drag a point up or down (±5 dB) – low below ~110 Hz, mid around 1 kHz, high above ~7 kHz. Double-click: flat",
            );
            [e.low, e.mid, e.high] = bands;
        }

        ui.add_space(air * 0.5);
        let caret = if self.live_open { ph::CARET_DOWN } else { ph::CARET_RIGHT };
        let fold = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("LIVE  {caret}"))
                        .font(bold(11.0))
                        .color(FAINT)
                        .extra_letter_spacing(1.6),
                )
                .sense(Sense::click()),
            )
            .on_hover_text(if self.live_open {
                "Fold the live controls away – the keys still work"
            } else {
                "Show the live controls"
            })
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if fold.clicked() {
            self.live_open = !self.live_open;
        }
        if self.live_open {
            self.live_controls(ui, live, now, w, tall, energy_h, filter_h, bh, drop_h);
        }
        if self.luna_desk {
            let rest = ui.available_height() - gap;
            if rest >= 70.0 {
                // it takes what's left, down to the bottom like SOLAR OS on the left
                self.luna_screen(ui, w, rest.min(260.0), now);
            }
        }
    }

    /// Luna at the bottom of the panel, in a screen set into the concrete like SOLAR OS: it
    /// dances to the beat while the music plays, with its level and what comes next. A click
    /// opens the settings, where it's dressed up.
    fn luna_screen(&mut self, ui: &mut Ui, w: f32, h: f32, now: f64) {
        let (outer, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let screen = widgets::bezel_frame(ui.painter(), outer, 13.0, 4.0);
        let p = ui.painter_at(screen);
        let hot = resp.hovered();
        let luna = self.luna;
        let ink = luna.ink();
        let t = now as f32;
        p.rect_filled(screen, 10.0, mix(BG, Color32::BLACK, 0.55));
        if hot {
            p.rect_stroke(screen, 10.0, stroke(1.0, alpha(ink, 0.6)), egui::StrokeKind::Inside);
        }
        // to the beat while it plays, a slow sway when it doesn't
        let playing = self.playing();
        let beat = if playing {
            (now * self.song.bpm as f64 / 60.0 * std::f64::consts::PI).sin().abs() as f32
        } else {
            ((t * 0.8).sin() * 0.5 + 0.5) * 0.3
        };
        let s = (screen.height() * 0.3).clamp(22.0, 40.0);
        let lc = pos2(
            screen.left() + s * 1.5 + (t * 1.1).sin() * 2.0,
            screen.center().y - s * 0.3 - beat * s * 0.12,
        );
        let wave = if hot {
            (t * 6.0).sin() * 0.5 + 0.5
        } else {
            ((t % 6.0 - 5.0).max(0.0) * std::f32::consts::TAU).sin().abs()
        };
        crate::face::halo(&p, lc, s * 1.2, ink, if playing { 0.2 + 0.2 * beat } else { 0.18 });
        luna.draw(&p, lc, s, t, 1.0, wave);
        let shadow = Rect::from_center_size(pos2(lc.x, lc.y + s * 0.88 + beat * s * 0.12), vec2(s * (0.9 - 0.2 * beat), 4.0));
        p.rect_filled(shadow, 2.0, alpha(ink, 0.18));

        let x = screen.left() + s * 3.1;
        let tw = screen.right() - 12.0 - x;
        let lv = luna.level();
        let (into, next) = luna.progress();
        let top = screen.center().y - 40.0;
        p.text(pos2(x, top), Align2::LEFT_CENTER, "LUNA", bold(10.0), alpha(ink, 0.9));
        p.text(pos2(x, top + 20.0), Align2::LEFT_CENTER, format!("Level {lv}"), bold(18.0), TEXT);
        let bar = Rect::from_min_size(pos2(x, top + 36.0), vec2(tw, 6.0));
        p.rect_filled(bar, 3.0, PANEL_HI);
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * into, bar.height())), 3.0, ink);
        let (dust, coming) = match next {
            Some(n) => (
                format!("{} {} / {n}", ph::SPARKLE, luna.dust),
                format!("Next: {}", crate::luna::Luna::unlocked(lv + 1)),
            ),
            None => (format!("{} {}", ph::SPARKLE, luna.dust), "Everything unlocked".to_owned()),
        };
        p.text(pos2(x, top + 56.0), Align2::LEFT_CENTER, dust, bold(11.0), ink);
        let coming = widgets::fit_text(&p, &coming, body(11.0), tw);
        p.text(pos2(x, top + 74.0), Align2::LEFT_CENTER, coming, body(11.0), DIM);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Luna – click to dress it up")
            .clicked()
        {
            self.settings = true;
        }
    }

    /// LIVE: energy, the filter pad, the effects and DROP
    #[allow(clippy::too_many_arguments)]
    fn live_controls(
        &mut self,
        ui: &mut Ui,
        live: &Live,
        now: f64,
        w: f32,
        tall: bool,
        energy_h: f32,
        filter_h: f32,
        bh: f32,
        drop_h: f32,
    ) {
        let in_log = self.song.auto && self.song.auto_fx && self.song_form.is_none();
        let _ = energy_h;
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        // Energy: the big lever of the arrangement – while the auto mode steers it, it shows in the log
        let mut e = self.song.macros.energy;
        // A song or the auto mode with AUTO FX steer the energy: then it's a slim display
        let locked = self.energy_locked();
        let h = if locked {
            30.0
        } else if tall {
            58.0
        } else {
            50.0
        };
        let label = if self.song_form.is_some() { "SONG" } else { "AUTO" };
        if !in_log {
            let out = widgets::energy_fader(ui, &mut e, w, h, locked.then_some(label));
            if out.changed {
                self.set_energy(e, now);
            }
            if out.locked_clicked {
                self.say(self.energy_hint(), now);
            }
        }
        // What the engine's filter does (smoothed, or a breakdown sweeping it), for the pad
        let engine_filter = if self.playing() {
            f32::from_bits(self.shared.filter_now.load(Ordering::Relaxed))
        } else {
            0.0
        };
        let before = (self.filter, self.filter_res);
        controls::filter_pad(ui, &mut self.filter, &mut self.filter_res, engine_filter, w, filter_h)
            .on_hover_text("Filter: drag left for low-pass, right for high-pass, up for resonance – let go and it springs back");
        if self.filter != before.0 {
            self.send(Cmd::Filter(self.filter));
        }
        if self.filter_res != before.1 {
            self.send(Cmd::FilterRes(self.filter_res));
        }
        // Effects: hold to use
        let gap = GAP;
        // Two side by side, three rows
        let bw = (w - gap) / 2.0;
        for row in 0..3 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for col in 0..2 {
                    let i = row * 2 + col;
                    let fx = Fx::ALL[i];
                    let (r, resp) = ui.allocate_exact_size(vec2(bw, bh), Sense::click_and_drag());
                    self.fx_mouse[i] = resp.is_pointer_button_down_on();
                    let active = self.fx_mouse[i] || self.fx_key[i];
                    let lit = auto_fx(fx, live);
                    // The picture stands still until the mouse is on it; it runs faster while
                    // the effect is on, and a press sends a burst out of the button
                    if active && !self.fx_was_on[i] {
                        self.fx_pressed_at[i] = now;
                    }
                    self.fx_was_on[i] = active;
                    let speed = if active {
                        4.0
                    } else if lit > 0.1 {
                        3.0
                    } else if resp.hovered() {
                        1.0
                    } else {
                        0.0
                    };
                    if speed > 0.0 {
                        self.fx_phase[i] += dt * speed;
                        ui.ctx().request_repaint();
                    }
                    let t = self.fx_phase[i];
                    let boost = if active { 1.0 } else { 0.0 };
                    widgets::fx_button(
                        ui,
                        r,
                        &resp,
                        fx_icon(fx),
                        fx.name(),
                        fx.key(),
                        fx_color(fx),
                        active,
                        lit,
                        |p, rr, ink| controls::fx_art(p, rr, i, ink, t, boost),
                    );
                    let since = (now - self.fx_pressed_at[i]) as f32;
                    if since < 0.5 {
                        controls::fx_burst(ui.painter(), r, fx_color(fx), since / 0.5);
                        ui.ctx().request_repaint();
                    }
                    resp.on_hover_text(format!("{} – hold ({})", fx.blurb(), fx.key()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            });
        }
        // The drop, right under the effects
        let (r, resp) = ui.allocate_exact_size(vec2(w, drop_h), Sense::click());
        let building = live.build;
        let p = ui.painter();
        let hot = resp.hovered();
        let pulse = building.map_or(0.0, |b| ((now * (4.0 + b as f64 * 12.0)).sin() * 0.5 + 0.5) as f32);
        widgets::glow_rect(
            p,
            r,
            14.0,
            RED,
            if building.is_some() {
                0.6 + pulse * 0.8
            } else if hot {
                0.5
            } else {
                0.2
            },
        );
        p.rect_filled(
            r,
            14.0,
            if building.is_some() {
                mix(mix(PANEL, RED, 0.3), RED, pulse * 0.5)
            } else if hot {
                mix(PAD, RED, 0.3)
            } else {
                mix(PAD, RED, 0.18)
            },
        );
        if let Some(b) = building {
            let fill = Rect::from_min_size(r.min, vec2(r.width() * b, r.height()));
            p.rect_filled(fill, 14.0, mix(PANEL, RED, 0.6));
        }
        // Warning stripes, like on a hangar door – they run while it builds up
        {
            let clip = ui.painter_at(r.shrink(2.0));
            let run = building.map_or(0.0, |_| (now * 60.0 % 24.0) as f32);
            let mut x = r.left() - r.height() + run - 24.0;
            while x < r.right() {
                clip.line_segment(
                    [pos2(x, r.bottom()), pos2(x + r.height(), r.top())],
                    stroke(8.0, RED.gamma_multiply(0.09)),
                );
                x += 24.0;
            }
        }
        let p = ui.painter();
        p.rect_stroke(r, 14.0, stroke(1.5, RED), egui::StrokeKind::Inside);
        let text = if building.is_some() {
            format!("{}  BUILDING…", ph::LIGHTNING)
        } else {
            format!("{}  DROP", ph::LIGHTNING)
        };
        p.text(r.center(), Align2::CENTER_CENTER, text, bold((drop_h * 0.3).min(24.0)), TEXT);
        p.text(
            r.right_top() + vec2(-9.0, 7.0),
            Align2::RIGHT_TOP,
            "B",
            bold(9.5),
            alpha(TEXT, 0.35),
        );
        let resp = resp
            .on_hover_text("Two bars of build-up from the next bar, then the drop (B)\nRight-click: a fill")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.drop_now(now);
        }
        resp.context_menu(|ui| {
            ui.label(egui::RichText::new("Fill – in this bar's second half, or the next one's").color(DIM));
            for f in crate::engine::FILLS {
                if ui.button(f.name()).clicked() {
                    self.send(Cmd::Fill(f));
                    self.say(
                        if self.playing() {
                            format!("Fill: {}", f.name())
                        } else {
                            "Fills play while the music runs".into()
                        },
                        now,
                    );
                    ui.close_menu();
                }
            }
        });
    }
}
