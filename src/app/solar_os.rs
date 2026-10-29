//! SOLAR OS: Luna's terminal (commands, verdicts, the ENERGY drive, auto effects, growing), the auto settings and the computer screen with its help.

use super::*;

impl SolarumApp {
    /// Luna says something in the log
    pub(super) fn bunk(&mut self, text: impl AsRef<str>, now: f64) {
        self.log.push(text.as_ref(), now);
    }

    /// A command typed into the terminal – Luna runs the auto mode, so it answers
    pub(super) fn command(&mut self, line: &str, now: f64) {
        let line = line.trim().to_lowercase();
        if line.is_empty() {
            return;
        }
        self.log.push(format!("$ {line}"), now);
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        let arg = rest.join(" ");
        let pick = |i: u32, n: usize| (now as u32).wrapping_mul(2_654_435_761).wrapping_add(i) as usize % n;
        let find_vibe = |name: &str| {
            Vibe::ALL
                .into_iter()
                .find(|v| !name.is_empty() && v.name().to_lowercase().starts_with(name))
        };
        match first {
            "help" | "?" | "commands" | "manual" => {
                self.help_open = true;
                self.bunk("My manual's open, bro – click a command to run it.", now);
            }
            "shuffle" | "roll" | "again" | "next" | "new" => {
                let n = self.dice_all();
                if n == 0 {
                    self.bunk("Every track is off or locked, bro. Nothing to roll.", now);
                    return;
                }
                const QUIPS: [&str; 4] = [
                    "Rolled it, bro. Smells like fresh techno.",
                    "New groove, same orbit. Let's get it.",
                    "Shook the dice, brother. Four to the floor, no cap.",
                    "There you go, fam. Tell me if it slaps.",
                ];
                self.bunk(QUIPS[pick(1, QUIPS.len())], now);
            }
            "seed" => {
                // A word of your own, or one Luna picks so you can come back to it
                let word = if arg.is_empty() {
                    const WORDS: [&str; 8] = ["nebula", "pulsar", "comet", "orbit", "quasar", "aurora", "meteor", "eclipse"];
                    format!("{}-{}", WORDS[pick(3, WORDS.len())], pick(4, 90) + 10)
                } else {
                    arg.clone()
                };
                self.seeded(&word, now);
                self.bunk(
                    format!(
                        "Seed \"{word}\" in {}. Same seed, same vibe – same beat, any time, bro.",
                        self.song.vibe.name()
                    ),
                    now,
                );
            }
            "like" | "keep" | "yes" | "love" | "nice" => self.verdict(true, now),
            "nope" | "no" | "undo" | "dislike" | "meh" => self.verdict(false, now),
            "taste" => self.bunk(self.taste.summary(), now),
            "chill" | "calm" | "easy" | "softer" | "less" | "relax" => self.push_drive(-1.0, now),
            "turnup" | "harder" | "more" | "push" | "hype" | "louder" | "faster" => self.push_drive(1.0, now),
            "turn" if arg == "up" => self.push_drive(1.0, now),
            "turn" if arg == "down" => self.push_drive(-1.0, now),
            "forget" => {
                self.taste = Default::default();
                self.send(Cmd::Taste(self.taste));
                self.bunk("Clean slate, bro. Show me what you like.", now);
            }
            "vibes" | "list" => {
                let names: Vec<&str> = Vibe::ALL.iter().map(|v| v.name()).collect();
                self.bunk(names.join(" · "), now);
            }
            "vibe" | "play" | "style" => match find_vibe(&arg) {
                Some(v) => {
                    self.shuffle(v, now);
                    self.bunk(format!("{} it is, bro. {}", v.name(), v.blurb()), now);
                }
                None => self.bunk("Which one, bro? Type vibes for the list.", now),
            },
            "drop" | "boom" => {
                if self.playing() {
                    self.drop_now(now);
                    self.bunk("Hold on to something, brother. Here it comes.", now);
                } else {
                    self.bunk("Press play first, fam – I can't drop silence.", now);
                }
            }
            "fx" => {
                let on = match arg.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => !self.song.auto_fx,
                };
                self.set_auto_fx(on, now);
            }
            "every" | "change" => match arg.trim_end_matches(" bars").parse::<u8>() {
                Ok(n) if (1..=16).contains(&n) => {
                    self.song.auto_bars = n;
                    self.bunk(
                        if n == 1 {
                            "Something new every bar. Busy night, bro.".to_string()
                        } else {
                            format!("Say less. Something new every {n} bars.")
                        },
                        now,
                    );
                }
                _ => self.bunk("Every how many bars, bro? 1 to 16.", now),
            },
            "mood" => match MOODS.iter().position(|m| !arg.is_empty() && m.0.to_lowercase().starts_with(&arg)) {
                Some(k) => {
                    self.song.set_mood(k);
                    self.song.mood_hold = true;
                    gen::refit_notes(&mut self.song, &mut self.rng);
                    self.bunk(format!("{}. {}", MOODS[k].0, MOODS[k].1), now);
                }
                None if arg == "auto" => {
                    self.song.mood_hold = false;
                    self.bunk("Bet. I'll pick the mood with every shuffle.", now);
                }
                None => {
                    let names: Vec<&str> = MOODS.iter().map(|m| m.0).collect();
                    self.bunk(format!("Moods: {} – or auto", names.join(" · ")), now);
                }
            },
            "bpm" | "tempo" => match arg.parse::<f32>() {
                Ok(b) => {
                    self.song.bpm = b.clamp(*crate::song::BPM.start(), *crate::song::BPM.end());
                    self.bunk(format!("{:.0} BPM.", self.song.bpm), now);
                }
                Err(_) => self.bunk(format!("We're at {:.0} BPM, bro. Say bpm 135 to change it.", self.song.bpm), now),
            },
            "status" | "where" => {
                let mood = self.song.mood().map_or("", |k| MOODS[k].0);
                let text = format!(
                    "{} · {} · {mood} · {:.0} BPM · energy {:.0}",
                    self.song.vibe.name(),
                    self.song.root_name(),
                    self.song.bpm,
                    self.song.macros.energy * 100.0
                );
                self.bunk(text, now);
            }
            "joke" => {
                const JOKES: [&str; 4] = [
                    "Why did the kick quit, bro? It kept getting beat.",
                    "How many techno heads to change a bulb? One. And again. And again. And again.",
                    "Asked the hi-hat to chill. It said: tss. Disrespectful.",
                    "My favourite chord? Same as yesterday, brother. It's techno.",
                ];
                self.bunk(JOKES[pick(2, JOKES.len())], now);
            }
            "hi" | "hello" | "hey" | "luna" | "bunk" | "who" => {
                self.bunk("Yo bro! I'm Luna, I run the set down here. Type help if you're lost.", now)
            }
            "off" | "exit" | "bye" | "quit" | "shutdown" => {
                self.bunk("Peace out, brother. Lights out.", now);
                self.power(now);
            }
            _ => match find_vibe(first) {
                // A vibe's name alone works too
                Some(v) => {
                    self.shuffle(v, now);
                    self.bunk(format!("{} it is, bro. {}", v.name(), v.blurb()), now);
                }
                None => self.bunk("Didn't catch that, bro. I only speak techno – try help.", now),
            },
        }
    }

    /// A thumb for the auto mode's last move: up keeps it and Luna does more of that kind
    /// on that track; down puts the track back as it was (the engine then leaves it alone
    /// for two phrases) and Luna does less of it
    fn verdict(&mut self, liked: bool, now: f64) {
        let Some((c, before, _)) = self.last_auto.take() else {
            self.bunk("Nothing to rate yet, bro – wait for my next move.", now);
            return;
        };
        self.taste.learn(c, liked);
        self.send(Cmd::Taste(self.taste));
        let what = c.describe().to_lowercase();
        if liked {
            self.bunk(format!("Noted: {what}. More of that, bro."), now);
            return;
        }
        if let Some(v) = c.voice() {
            let mute_move = matches!(c, gen::Change::Muted(_) | gen::Change::Unmuted(_));
            let t = self.song.track_mut(v);
            *t = crate::song::Track {
                volume: t.volume,
                lock: t.lock,
                mute: if mute_move { before.mute } else { t.mute },
                ..before
            };
        }
        self.bunk(format!("Undone: {what}. Less of that – got it."), now);
    }

    /// chill (−1) or turnup (1): the energy slider a step down or up – with AUTO FX the
    /// whole set lifts or sinks (and the tempo follows a little), by hand the energy glides
    fn push_drive(&mut self, dir: f32, now: f64) {
        if self.song_form.is_some() {
            self.bunk("The song steers the energy right now, bro.", now);
            return;
        }
        if self.energy_locked() {
            let before = self.drive;
            self.set_drive(self.drive + dir / 3.0);
            if self.drive == before {
                self.bunk(
                    if dir > 0.0 {
                        "That's as hard as I go, fam. Any more and the speakers file a complaint."
                    } else {
                        "Any chiller and we're asleep, bro."
                    },
                    now,
                );
            } else {
                self.say_drive(before, now);
            }
            return;
        }
        let e = self.glide_energy.map_or(self.song.macros.energy, |g| g.0);
        // Chill never sinks into the break zone – that's what the energy bar is for
        let to = (e + 0.12 * dir).clamp(0.35, 1.0);
        self.glide_energy = Some((to, self.song.macros.energy));
        self.bunk(
            if dir > 0.0 {
                format!("More energy, bro – up to {:.0}.", to * 100.0)
            } else {
                format!("Easing off, bro – down to {:.0}.", to * 100.0)
            },
            now,
        );
    }

    /// With AUTO FX: how far the energy slider lifts (or lowers) the whole set, −1…1. The
    /// engine heats the set and aims at a busier groove; the tempo glides 3 BPM across the range.
    fn set_drive(&mut self, d: f32) {
        let d = d.clamp(-1.0, 1.0);
        if (d - self.drive).abs() < 1e-4 {
            return;
        }
        let bpm = self.glide_bpm.map_or(self.song.bpm, |g| g.0);
        let goal = (bpm + 3.0 * (d - self.drive)).clamp(*crate::song::BPM.start(), *crate::song::BPM.end());
        self.drive = d;
        self.send(Cmd::Drive(d));
        self.glide_bpm = Some((goal, self.song.bpm));
    }

    /// What Luna says to a new drive (`before`: where it came from)
    fn say_drive(&mut self, before: f32, now: f64) {
        let d = self.drive;
        if (d - before).abs() < 0.02 {
            return;
        }
        let up = d > before;
        let words = match d {
            d if d > 0.9 => "Full send, fam. Maximum pressure.",
            d if d > 0.5 => "Turning it up – more drive, more hits. Feel that?",
            d if d > 0.1 => "A bit more push, bro.",
            d if d > -0.1 && up => "Waking it back up, bro. Middle of the road.",
            d if d > -0.1 => "Taking the edge off, bro. Middle of the road.",
            d if d > -0.5 => "Easing off a little, bro.",
            d if d > -0.9 => "Chill mode – less pressure, more air.",
            _ => "Deep chill, brother. Floating.",
        };
        let bpm = self.glide_bpm.map_or(self.song.bpm, |g| g.0);
        self.bunk(format!("{words} → {bpm:.0} BPM"), now);
    }

    /// AUTO FX on or off (the gear, or `fx` in the terminal)
    fn set_auto_fx(&mut self, on: bool, now: f64) {
        if self.song.auto_fx == on {
            return;
        }
        self.song.auto_fx = on;
        if !on && self.song.auto {
            // The bar decides now: everything Solarum took out comes back
            for t in self.song.tracks.iter_mut().filter(|t| !t.lock) {
                t.mute = false;
            }
        }
        self.say(
            if on {
                "Auto FX on – Luna plays breaks and drops by itself"
            } else {
                "Auto FX off – the energy bar is yours: low = break, 85 and up = everything, DROP drops"
            },
            now,
        );
    }

    /// The computer's power switch: the auto mode on or off (or a running song stopped)
    pub(super) fn power(&mut self, now: f64) {
        self.power_at = now;
        if self.song_form.is_some() {
            self.stop_song(now);
        } else {
            self.toggle_auto(now);
            self.last_auto = None;
            self.help_open = false;
            if self.song.auto {
                let hello = crate::log::HELLOS[(now as usize) % crate::log::HELLOS.len()];
                self.bunk(hello, now);
                self.start_growing(now);
                // Luna takes over: the music starts right away
                if !self.playing() {
                    self.toggle_play();
                }
            }
        }
    }

    /// The auto mode comes on: Luna keeps what's there and grows the rest – every empty track
    /// comes in on its own bar (an empty sky builds up from the kick)
    fn start_growing(&mut self, now: f64) {
        let empty = self.song.tracks.iter().all(|t| t.hits() == 0);
        if self.song.vibe == Vibe::Custom {
            let vibe = Vibe::ALL[(now as usize).wrapping_mul(7) % Vibe::ALL.len()];
            self.song.vibe = vibe;
            if empty {
                let (lo, hi) = vibe.bpm();
                self.song.bpm = ((lo + hi) / 2.0).round();
            }
        }
        // Neutral is only the start: the auto mode picks a mood of the vibe and changes it later
        if self.song.mood() == Some(crate::song::NEUTRAL) {
            let moods = gen::moods_for(self.song.vibe);
            self.song.set_mood(moods[(now as usize).wrapping_mul(13) % moods.len()]);
            self.song.mood_hold = false;
            gen::refit_notes(&mut self.song, &mut self.rng);
        }
        let target = gen::grow_target(&self.song, &mut self.rng);
        if target.tracks == self.song.tracks {
            self.bunk("This beat is lowkey fire, bro. I'll keep it moving.", now);
            return;
        }
        self.send(Cmd::Grow(Box::new(target)));
        let vibe = self.song.vibe.name().to_lowercase();
        self.bunk(
            if empty {
                format!("Empty sky, fam. I'll cook something {vibe}, one track per bar.")
            } else {
                "Your beat stays, brother. I'll fill in the rest, one track per bar.".to_string()
            },
            now,
        );
    }

    /// The auto mode's settings, from SET in SOLAR OS: how often it changes
    /// something, and AUTO FX
    fn auto_settings(&mut self, ui: &mut Ui, now: f64) {
        // SOLAR OS green, like the VIBE menu
        ui.set_min_width(260.0);
        let mono = egui::FontId::monospace;
        let bars = self.song.auto_bars;
        let title = if bars == 1 {
            "Change something every bar".to_string()
        } else {
            format!("Change something every {bars} bars")
        };
        ui.label(egui::RichText::new(title).font(mono(12.0)).color(ACID));
        ui.add_space(2.0);
        let all: Vec<u8> = (1..=16).collect();
        if let Some(b) = widgets::stops(ui, &all, bars, 250.0, ACID) {
            self.song.auto_bars = b;
        }
        ui.add_space(4.0);
        let fx = self.song.auto_fx;
        let mut job = egui::text::LayoutJob::default();
        let fmt = |font, color| egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        };
        job.append(&format!("> AUTO FX {}", if fx { "on" } else { "off" }), 0.0, fmt(mono(13.0), ACID));
        job.append(
            "   breaks, build-ups and drops by itself",
            0.0,
            fmt(mono(11.0), ACID.gamma_multiply(0.55)),
        );
        let toggled = ui.selectable_label(fx, job);
        if toggled.clicked() {
            self.song.auto_fx = !fx;
        }
        if toggled
            .on_hover_text(
                "On: Solarum arranges by itself – the energy glides, breakdowns, build-ups with effects \
                 (low-pass, gate, echo) and drops. The energy shows in the log.\n\
                 Off: the energy bar is yours – low = break, 85 and up = everything, DROP drops. The auto mode \
                 still changes patterns, sounds and notes, but never takes a track out.",
            )
            .clicked()
        {
            if fx && self.song.auto {
                // The bar decides now: everything Solarum took out comes back
                for t in self.song.tracks.iter_mut().filter(|t| !t.lock) {
                    t.mute = false;
                }
            }
            self.say(
                if fx {
                    "Auto FX off – the energy bar is yours: low = break, 85 and up = everything, DROP drops"
                } else {
                    "Auto FX on – Luna plays breaks and drops by itself"
                },
                now,
            );
        }
    }

    /// The auto mode is the computer: SOLAR OS, the terminal under the mood. Off, its screen
    /// is dark with a power button; switched on it boots, Luna says hello, and it's a little
    /// operating system: a title bar (SET for the settings, OFF), a menu (the vibe, SHUFFLE),
    /// the log, and a prompt Luna answers.
    pub(super) fn computer(&mut self, ui: &mut Ui, energy: Option<f32>, now: f64) {
        let on = self.song.auto || self.song_form.is_some();
        let w = ui.available_width();
        let mut switch = false;
        // Set into the concrete: a dark well, a thick bezel, the screen inside
        let h_all = ui.available_height().max(if on { 160.0 } else { 80.0 });
        let (outer, _) = ui.allocate_exact_size(vec2(w, h_all), Sense::hover());
        widgets::bezel_frame(ui.painter(), outer, 13.0, 4.0);
        let screen = outer.shrink(6.0);
        if on && self.help_open {
            self.os_help(ui.ctx(), outer, now);
        }
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(screen));
        let ui = &mut inner;
        let w = screen.width();
        if on {
            // The last move with its thumbs sits right above the prompt
            let card = self.song.auto && self.last_auto.is_some();
            let (chrome, prompt) = (66.0, if card { 48.0 } else { 24.0 });
            let h = screen.height();
            let rect = self.log.ui(ui, self.log_state(), energy, h, chrome, prompt, now);
            switch |= self.os_chrome(ui, rect, now);
            if card {
                self.os_card(ui, rect, now);
            }
            self.os_prompt(ui, rect, now);
            // Logged in: Luna, the mascot, waves and says hello, then the log takes over
            let since = (now - self.power_at) as f32;
            if (0.0..2.0).contains(&since) {
                let p = ui.painter_at(rect);
                let fade = (1.0 - (since - 1.6).max(0.0) / 0.4).clamp(0.0, 1.0);
                p.rect_filled(rect, 10.0, mix(BG, Color32::BLACK, 0.55).gamma_multiply(fade));
                let size = (rect.height() * 0.32).min(64.0);
                let c = rect.center() - vec2(0.0, size * 0.45);
                crate::log::robot(&p, c, size, since, ACID.gamma_multiply(fade));
                let hello = crate::log::HELLOS[(self.power_at as usize) % crate::log::HELLOS.len()];
                let typed: String = hello.chars().take(((since - 0.3).max(0.0) * 50.0) as usize).collect();
                p.text(
                    pos2(rect.center().x, c.y + size * 1.05),
                    Align2::CENTER_TOP,
                    typed,
                    egui::FontId::monospace(11.0),
                    ACID.gamma_multiply(fade),
                );
                ui.ctx().request_repaint();
            }
            // Booting: a line of light opens into the picture
            let k = ((now - self.power_at) / 0.3) as f32;
            if k < 1.0 {
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 10.0, Color32::from_black_alpha((255.0 * (1.0 - k)) as u8));
                let h = (rect.height() * k * k).max(2.0);
                let band = Rect::from_center_size(rect.center(), vec2(rect.width() * (0.3 + 0.7 * k.sqrt()), h));
                p.rect_filled(band, 0.0, ACID.gamma_multiply(0.5 * (1.0 - k)));
                ui.ctx().request_repaint();
            }
        } else {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, screen.height()), Sense::click());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 10.0, Color32::from_black_alpha(130));
            p.rect_stroke(rect, 10.0, stroke(1.0, mix(PANEL, ACID, 0.18)), egui::StrokeKind::Inside);
            // The glass: a faint diagonal sheen
            let sheen = vec![
                rect.left_top() + vec2(rect.width() * 0.62, 0.0),
                rect.left_top() + vec2(rect.width() * 0.74, 0.0),
                rect.left_top() + vec2(rect.width() * 0.34, rect.height()),
                rect.left_top() + vec2(rect.width() * 0.22, rect.height()),
            ];
            p.add(egui::Shape::convex_polygon(sheen, Color32::from_white_alpha(1), egui::Stroke::NONE));
            // Switching off: the picture collapses into a line, then a dot
            let k = ((now - self.power_at) / 0.4) as f32;
            if k < 1.0 {
                let w = rect.width() * (1.0 - k).powi(2);
                p.rect_filled(
                    Rect::from_center_size(rect.center(), vec2(w.max(3.0), 2.0)),
                    1.0,
                    ACID.gamma_multiply(1.0 - k),
                );
                ui.ctx().request_repaint();
            } else {
                let hot = resp.hovered();
                let c = rect.center() - vec2(0.0, 12.0);
                if hot {
                    glow(&p, c, 22.0, ACID, 0.12);
                }
                p.circle_filled(c, 26.0, mix(PANEL, BG, 0.6));
                p.circle_stroke(c, 26.0, stroke(2.0, alpha(ACID, if hot { 0.75 } else { 0.55 })));
                p.text(
                    c,
                    Align2::CENTER_CENTER,
                    ph::POWER,
                    body(24.0),
                    alpha(ACID, if hot { 0.85 } else { 0.75 }),
                );
                p.text(
                    c + vec2(0.0, 40.0),
                    Align2::CENTER_CENTER,
                    "SOLAR OS",
                    bold(10.5),
                    if hot { TEXT } else { DIM },
                );
                p.text(
                    c + vec2(0.0, 55.0),
                    Align2::CENTER_CENTER,
                    "Switch on and Luna plays a set",
                    body(10.5),
                    FAINT,
                );
            }
            switch |= resp
                .on_hover_text("Switch on the auto mode: Luna plays a whole set by itself")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked();
        }
        if switch {
            self.power(now);
        }
    }

    /// SOLAR OS's window chrome: the title bar (SET, OFF) and the menu (VIBE, SHUFFLE).
    /// True when OFF was clicked.
    fn os_chrome(&mut self, ui: &mut Ui, rect: Rect, now: f64) -> bool {
        let p = ui.painter().clone();
        let mono = |s: f32| egui::FontId::monospace(s);
        let key = |ui: &mut Ui, r: Rect, id: &str, text: &str, strong: bool| {
            let resp = ui.interact(r, ui.id().with(id), Sense::click());
            let hot = resp.hovered();
            let fill = if hot {
                0.3
            } else if strong {
                0.14
            } else {
                0.0
            };
            if fill > 0.0 {
                p.rect_filled(r, 3.0, ACID.gamma_multiply(fill));
            }
            p.text(r.center(), Align2::CENTER_CENTER, text, mono(10.0), ACID);
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        };
        // Title bar
        let bar = Rect::from_min_max(rect.min + vec2(1.0, 1.0), pos2(rect.right() - 1.0, rect.top() + 20.0));
        p.rect_filled(
            bar,
            egui::CornerRadius {
                nw: 9,
                ne: 9,
                sw: 0,
                se: 0,
            },
            ACID.gamma_multiply(0.1),
        );
        let led = pos2(bar.left() + 10.0, bar.center().y);
        glow(&p, led, 3.5, ACID, 0.5);
        p.circle_filled(led, 3.0, ACID);
        let title = if self.song_form.is_some() { "SOLAR OS · SONG" } else { "SOLAR OS" };
        p.text(led + vec2(9.0, 0.0), Align2::LEFT_CENTER, title, mono(10.5), ACID);
        let off_r = Rect::from_min_max(
            pos2(bar.right() - 36.0, bar.top() + 2.0),
            pos2(bar.right() - 4.0, bar.bottom() - 2.0),
        );
        let set_r = Rect::from_min_max(pos2(off_r.left() - 36.0, off_r.top()), pos2(off_r.left() - 4.0, off_r.bottom()));
        let off = key(ui, off_r, "os_off", "OFF", false).on_hover_text("Switch the auto mode off (or type off)");
        let set = key(ui, set_r, "os_set", "SET", false).on_hover_text("Settings: how often Luna changes something, AUTO FX");
        let popup = ui.make_persistent_id("auto_bars");
        if set.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup_below_widget(ui, popup, &set, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            self.auto_settings(ui, now)
        });
        // Menu
        let row = Rect::from_min_max(
            pos2(rect.left() + 8.0, bar.bottom() + 4.0),
            pos2(rect.right() - 8.0, bar.bottom() + 20.0),
        );
        let vibe_r = Rect::from_min_size(row.min, vec2(row.width() * 0.62 - 3.0, row.height()));
        let shuf_r = Rect::from_min_max(pos2(vibe_r.right() + 6.0, row.top()), row.max);
        let vibe = key(ui, vibe_r, "os_vibe", "PICK A GENRE", true)
            .on_hover_text(format!("Now: {} – pick one and Luna rolls a groove in it", self.song.vibe.name()));
        // A little arrow: it opens a list
        let c = pos2(vibe_r.right() - 10.0, vibe_r.center().y);
        p.add(egui::Shape::convex_polygon(
            vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)],
            ACID,
            egui::Stroke::NONE,
        ));
        let shuf = key(ui, shuf_r, "os_shuffle", "SHUFFLE", true).on_hover_text("Rolls the dice on every track that's on");
        let popup = ui.make_persistent_id("term_vibes");
        if vibe.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup_below_widget(ui, popup, &vibe, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            self.vibe_list(ui, now)
        });
        if shuf.clicked() {
            self.command("shuffle", now);
        }
        let energy = self.song.macros.energy;
        self.os_energy(
            ui,
            Rect::from_min_max(pos2(row.left(), row.bottom() + 6.0), pos2(row.right(), row.bottom() + 22.0)),
            energy,
            now,
        );
        off.clicked()
    }

    /// The auto mode's last move, above the prompt: what it changed, and a thumb up (keep it,
    /// more of that) or down (undo it, less of that) – Luna learns your taste from them
    fn os_card(&mut self, ui: &mut Ui, rect: Rect, now: f64) {
        let Some((c, _, at)) = self.last_auto else { return };
        let row = Rect::from_min_max(
            pos2(rect.left() + 8.0, rect.bottom() - 48.0),
            pos2(rect.right() - 8.0, rect.bottom() - 28.0),
        );
        let p = ui.painter().clone();
        // It lights up when it comes, then settles
        let fresh = (1.0 - (now - at) as f32 / 1.5).clamp(0.0, 1.0);
        p.rect_filled(row, 4.0, ACID.gamma_multiply(0.08 + 0.12 * fresh));
        let key_w = 24.0;
        let down_r = Rect::from_min_size(pos2(row.right() - key_w, row.top()), vec2(key_w, row.height()));
        let up_r = down_r.translate(vec2(-key_w - 2.0, 0.0));
        let text_w = up_r.left() - row.left() - 10.0;
        let label = fit_text(&p, &c.describe().to_uppercase(), egui::FontId::monospace(10.0), text_w);
        p.text(
            pos2(row.left() + 6.0, row.center().y),
            Align2::LEFT_CENTER,
            label,
            egui::FontId::monospace(10.0),
            ACID,
        );
        let thumb = |r: Rect, id: &str, icon: &str, tip: &str| {
            let resp = ui.interact(r, ui.id().with(id), Sense::click());
            let hot = resp.hovered();
            if hot {
                p.rect_filled(r.shrink(1.0), 3.0, ACID.gamma_multiply(0.3));
            }
            p.text(r.center(), Align2::CENTER_CENTER, icon, body(13.0), if hot { TEXT } else { ACID });
            resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
        };
        let up = thumb(up_r, "os_like", ph::THUMBS_UP, "Keep it – Luna does more of that (or type like)");
        let down = thumb(
            down_r,
            "os_nope",
            ph::THUMBS_DOWN,
            "Undo it – the track goes back and Luna does less of that (or type nope)",
        );
        if up {
            self.verdict(true, now);
        } else if down {
            self.verdict(false, now);
        }
    }

    /// The energy slider under the menu – the one lever for how much pressure there is. The
    /// lit cells are the energy playing now. By hand (AUTO FX off) it sets the energy. With
    /// AUTO FX Luna steers the energy through the cycle, and the slider lifts or lowers all
    /// of it (`set_drive`): drag right and the set runs hotter, busier and a touch faster,
    /// left calmer, airier and a touch slower. The notch under it shows the lift.
    fn os_energy(&mut self, ui: &mut Ui, row: Rect, energy: f32, now: f64) {
        let p = ui.painter().clone();
        let mono = egui::FontId::monospace;
        p.text(row.left_center(), Align2::LEFT_CENTER, "ENERGY", mono(10.0), alpha(ACID, 0.7));
        let num = p.text(
            row.right_center(),
            Align2::RIGHT_CENTER,
            format!("{:>3.0}", energy * 100.0),
            mono(10.0),
            ACID,
        );
        let track = Rect::from_min_max(pos2(row.left() + 50.0, row.top()), pos2(num.left() - 8.0, row.bottom()));
        // A few pixels more to grab than to see: the row is thin
        let resp = ui.interact(track.expand2(vec2(0.0, 4.0)), ui.id().with("os_energy"), Sense::click_and_drag());
        let steer = self.energy_locked();
        let song = self.song_form.is_some();
        let v_at = |x: f32| ((x - track.left()) / track.width()).clamp(0.0, 1.0);
        if let Some(pos) = resp.interact_pointer_pos().filter(|_| !song) {
            if steer {
                // The notch is the handle and sits right under the pointer, like any slider: the
                // middle is no lift, the ends are full CHILL and TURN UP. It snaps to the middle
                // so the way back to neutral is easy. (The cells keep showing the energy playing.)
                let id = ui.id().with("os_energy_grab");
                let before = match ui.data(|d| d.get_temp::<f32>(id)) {
                    Some(b) => b,
                    None => {
                        ui.data_mut(|d| d.insert_temp(id, self.drive));
                        self.drive
                    }
                };
                self.set_drive(drive_at(pos.x, track));
                if resp.drag_stopped() || resp.clicked() {
                    ui.data_mut(|d| d.remove_temp::<f32>(id));
                    self.say_drive(before, now);
                }
            } else {
                self.glide_energy = None;
                self.set_energy(v_at(pos.x), now);
            }
        }
        if resp.clicked() && song {
            self.say(self.energy_hint(), now);
        }
        // The cells: the energy playing now, like an old level meter
        let n = 24;
        let cw = track.width() / n as f32;
        let hot = resp.hovered() || resp.dragged();
        let cells = Rect::from_min_max(track.left_top() + vec2(0.0, 2.0), track.right_bottom() - vec2(0.0, 5.0));
        for i in 0..n {
            let on = energy >= (i as f32 + 0.5) / n as f32;
            let r = Rect::from_min_size(pos2(cells.left() + i as f32 * cw, cells.top()), vec2(cw - 2.0, cells.height()));
            let a = if on {
                1.0
            } else if hot {
                0.22
            } else {
                0.12
            };
            p.rect_filled(r, 1.0, alpha(ACID, a));
        }
        // With AUTO FX: a hairline under it, the middle marked, and a notch where the lift is
        if steer {
            let y = track.bottom() - 1.5;
            p.line_segment([pos2(track.left(), y), pos2(track.right(), y)], stroke(1.0, alpha(ACID, 0.2)));
            let mid = track.center().x;
            p.line_segment([pos2(mid, y - 2.0), pos2(mid, y + 1.0)], stroke(1.0, alpha(ACID, 0.35)));
            let at = pos2(mid + self.drive * track.width() * 0.5, y);
            // Bigger while you're on it, so you see what you're holding
            let s = if hot { 1.6 } else { 1.0 };
            p.add(egui::Shape::convex_polygon(
                vec![at + vec2(-3.0, 2.0) * s, at + vec2(3.0, 2.0) * s, at + vec2(0.0, -2.5) * s],
                ACID,
                egui::Stroke::NONE,
            ));
        }
        let tip = if song {
            "The song steers the energy"
        } else if steer {
            "Energy: Luna steers it through the cycle. The notch lifts or lowers the whole set – drag it \
             right for hotter, busier and a touch faster, left for calmer, airier and a touch slower, the \
             middle for neither (or type turnup / chill)."
        } else {
            "Energy: what plays – low is a break, from 85 up everything plays (or type turnup / chill)"
        };
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
    }

    /// Luna's manual (type help): every command by topic with what it does, in a window
    /// next to SOLAR OS. A click runs a command – or, when it takes a word (vibe acid,
    /// bpm 135 …), puts it into the prompt to finish. Esc, ✕ or a click outside close it.
    fn os_help(&mut self, ctx: &egui::Context, os: Rect, now: f64) {
        // (what you type, what it does, what a click does: run it or start it in the prompt)
        enum Do {
            Run(&'static str),
            Type(&'static str),
        }
        type Row = (&'static str, &'static str, Do);
        const TOPICS: [(&str, &[Row]); 5] = [
            (
                "GROOVE",
                &[
                    ("shuffle", "new patterns on every track that's on", Do::Run("shuffle")),
                    ("acid", "a groove in another genre", Do::Type("vibe ")),
                    ("vibes", "all ten genres", Do::Run("vibes")),
                    ("seed tom", "a beat from a word: same word, same beat", Do::Type("seed ")),
                ],
            ),
            (
                "THE SET",
                &[
                    ("drop", "a build-up now, the drop on the bar after", Do::Run("drop")),
                    ("fx on · off", "I play breakdowns and drops myself", Do::Run("fx")),
                    ("turnup", "more energy – with AUTO FX hotter and faster", Do::Run("turnup")),
                    ("chill", "less energy – with AUTO FX calmer and slower", Do::Run("chill")),
                    ("every 8", "how often I change something: 1–16 bars", Do::Type("every ")),
                ],
            ),
            (
                "SOUND",
                &[
                    ("mood dark", "other chords and scale – or mood auto", Do::Type("mood ")),
                    ("bpm 135", "the tempo", Do::Type("bpm ")),
                ],
            ),
            (
                "YOUR TASTE",
                &[
                    ("like", "keep my last move – I'll do more of that", Do::Run("like")),
                    ("nope", "undo my last move – I'll do less of that", Do::Run("nope")),
                    ("taste", "what I've learned about you", Do::Run("taste")),
                    ("forget", "forget it all and start over", Do::Run("forget")),
                ],
            ),
            (
                "ME",
                &[
                    ("status", "what's playing right now", Do::Run("status")),
                    ("joke", "you'll see", Do::Run("joke")),
                    ("off", "switch me off", Do::Run("off")),
                ],
            ),
        ];
        let mono = egui::FontId::monospace;
        let mut act: Option<&Do> = None;
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        let area = egui::Area::new(egui::Id::new("luna_help"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(pos2(os.right() + 10.0, os.bottom()))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(13, 15, 11))
                    .stroke(stroke(1.0, alpha(ACID, 0.45)))
                    .shadow(ui.style().visuals.popup_shadow)
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(360.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("LUNA · MANUAL").font(mono(12.0)).color(ACID));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let x =
                                    ui.add(egui::Label::new(egui::RichText::new(ph::X).font(body(13.0)).color(DIM)).sense(Sense::click()));
                                close |= x.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                            });
                        });
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new("Type it at LUNA> or click it here")
                                .font(body(11.0))
                                .color(FAINT),
                        );
                        for (topic, rows) in TOPICS.iter() {
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new(*topic).font(mono(10.0)).color(alpha(ACID, 0.55)));
                            ui.add_space(3.0);
                            for (cmd, what, how) in rows.iter() {
                                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
                                let hot = resp.hovered();
                                let p = ui.painter();
                                if hot {
                                    p.rect_filled(r.expand2(vec2(6.0, 0.0)), 4.0, ACID.gamma_multiply(0.14));
                                }
                                p.text(r.left_center(), Align2::LEFT_CENTER, *cmd, mono(11.5), ACID);
                                let text = fit_text(p, what, body(11.0), r.width() - 100.0);
                                p.text(
                                    r.left_center() + vec2(100.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    text,
                                    body(11.0),
                                    if hot { TEXT } else { DIM },
                                );
                                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    act = Some(how);
                                }
                            }
                        }
                    });
            });
        if area.response.clicked_elsewhere() {
            close = true;
        }
        match act {
            Some(Do::Run(c)) => self.command(c, now),
            Some(Do::Type(c)) => {
                self.cmd = c.to_string();
                // Into the prompt, the cursor at the end, ready for the word
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("luna_prompt")));
                close = true;
            }
            None => {}
        }
        if close {
            self.help_open = false;
        }
    }

    /// SOLAR OS's prompt, at the bottom of the screen: type to Luna
    fn os_prompt(&mut self, ui: &mut Ui, rect: Rect, now: f64) {
        let mono = egui::FontId::monospace(11.0);
        let line = Rect::from_min_max(
            pos2(rect.left() + 10.0, rect.bottom() - 24.0),
            pos2(rect.right() - 10.0, rect.bottom() - 6.0),
        );
        let p = ui.painter();
        p.line_segment(
            [line.left_top() - vec2(0.0, 2.0), line.right_top() - vec2(0.0, 2.0)],
            stroke(1.0, ACID.gamma_multiply(0.25)),
        );
        p.text(line.left_center(), Align2::LEFT_CENTER, "LUNA>", mono.clone(), ACID);
        let field = Rect::from_min_max(pos2(line.left() + 38.0, line.top()), line.max);
        let mut sent = None;
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(field), |ui| {
            // A calm cursor
            let cursor = &mut ui.style_mut().visuals.text_cursor;
            cursor.on_duration = 0.9;
            cursor.off_duration = 0.7;
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.cmd)
                    .id(egui::Id::new("luna_prompt"))
                    .frame(false)
                    .font(mono)
                    .text_color(ACID)
                    .hint_text(egui::RichText::new("type help").color(ACID.gamma_multiply(0.35)))
                    .desired_width(field.width()),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                sent = Some(std::mem::take(&mut self.cmd));
                resp.request_focus();
            }
            self.typing = resp.has_focus();
        });
        if let Some(line) = sent {
            self.command(&line, now);
        }
    }
}

/// The drive the ENERGY slider's notch stands for at `x`: −1 at the left end, 0 in the middle
/// (with a few pixels' snap, so neutral is easy to find again), 1 at the right end
pub(super) fn drive_at(x: f32, track: Rect) -> f32 {
    let dx = x - track.center().x;
    if dx.abs() < 5.0 {
        0.0
    } else {
        (dx / (track.width() * 0.5)).clamp(-1.0, 1.0)
    }
}
