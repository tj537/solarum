//! Tests only: screenshots of the whole window, rendered without a window.
//!
//!     cargo test --release render_ -- --ignored    → target/snapshots/*.png

#[cfg(test)]
mod tests {
    use crate::app::{SolarumApp, View};
    use crate::offscreen::Offscreen;
    use crate::orbit::Live;
    use crate::song::{Song, TRACKS};
    use std::path::Path;

    fn render(name: &str, w: f32, h: f32, setup: impl Fn(&mut SolarumApp)) {
        let mut snap = Offscreen::retina(w, h, 2.0);
        crate::theme::install(&snap.ctx);
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        setup(&mut app);
        let mut shapes = Vec::new();
        // A few frames so layouts and fonts settle
        for t in 0..4 {
            shapes = snap.frame(10.0 + t as f64 * 0.016, |ctx| app.ui(ctx));
        }
        snap.save(&shapes, Path::new(&format!("target/snapshots/{name}.png")));
    }

    /// The settings at the top: Luna's level and skins
    #[test]
    #[ignore]
    fn render_settings_luna() {
        let mut snap = Offscreen::retina(1360.0, 860.0, 2.0);
        crate::theme::install(&snap.ctx);
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        app.settings = true;
        app.luna = crate::luna::Luna {
            dust: 1100,
            antenna: 2,
            color: 3,
            extra: 2,
            ..Default::default()
        };
        let mut shapes = Vec::new();
        for t in 0..4 {
            shapes = snap.frame(t as f64 * 0.05, |ctx| app.ui(ctx));
        }
        snap.save(&shapes, Path::new("target/snapshots/settings_luna.png"));
    }

    /// The settings, scrolled down to the support card at the bottom
    #[test]
    #[ignore]
    fn render_settings() {
        use eframe::egui::{pos2, vec2, Event, MouseWheelUnit};
        let mut snap = Offscreen::retina(1360.0, 860.0, 2.0);
        crate::theme::install(&snap.ctx);
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        app.settings = true;
        let mid = pos2(680.0, 430.0);
        let mut shapes = Vec::new();
        for t in 0..10 {
            let events = if t < 3 {
                vec![]
            } else {
                vec![
                    Event::PointerMoved(mid),
                    Event::MouseWheel {
                        unit: MouseWheelUnit::Point,
                        delta: vec2(0.0, -400.0),
                        modifiers: Default::default(),
                    },
                ]
            };
            shapes = snap.frame_with_events(10.0 + t as f64 * 0.016, events, |ctx| app.ui(ctx));
        }
        snap.save(&shapes, Path::new("target/snapshots/settings.png"));
    }

    /// The face over time: frames at these seconds, a build-up (or none) in each
    fn render_face(name: &str, frames: &[(f64, Option<f32>)]) {
        let mut snap = Offscreen::retina(1360.0, 860.0, 2.0);
        crate::theme::install(&snap.ctx);
        let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
        app.visual = true;
        app.face = true;
        app.song.macros.energy = 0.8;
        let mut shapes = Vec::new();
        for &(t, build) in frames {
            app.fake_live = Some(Live {
                build,
                ..playing(t * 8.0, &[0, 3])
            });
            shapes = snap.frame(t, |ctx| app.ui(ctx));
        }
        snap.save(&shapes, Path::new(&format!("target/snapshots/{name}.png")));
    }

    #[test]
    #[ignore]
    fn render_face_frames() {
        render_face("face", &[(0.5, None), (0.52, None)]);
        render_face(
            "face_gape",
            &[(crate::face::gape_at(), None), (crate::face::gape_at() + 0.02, None)],
        );
        render_face("face_build", &[(10.0, Some(0.85)), (10.02, Some(0.85))]);
        render_face("face_scream", &[(10.0, Some(0.99)), (10.02, None), (10.35, None)]);
        render_face("face_laser", &[(10.0, Some(0.99)), (10.02, None), (10.9, None)]);
        render_face("face_crack", &[(10.0, Some(0.99)), (10.02, None), (11.7, None)]);
        render_face("face_burst", &[(10.0, Some(0.99)), (10.02, None), (12.15, None)]);
        render_face("face_back", &[(10.0, Some(0.99)), (10.02, None), (13.0, None)]);
    }

    /// The planets view alone, a bass or synth planet held and dragged up by `dy`
    fn render_note_drag(name: &str, track: usize, step: usize, dy: f32) {
        use crate::planets::{planets, System};
        use eframe::egui::{self, pos2, vec2, Event, PointerButton, Rect};
        let (w, h) = (900.0, 640.0);
        let mut snap = Offscreen::retina(w, h, 2.0);
        crate::theme::install(&snap.ctx);
        let mut song = crate::gen::generate(crate::song::Vibe::Acid, &Default::default(), &mut crate::rng::Rng::new(4));
        song.tracks[track].steps[step].on = true;
        song.tracks[track].mute = false;
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
        let sys = System::new(rect);
        let voice = crate::song::Voice::ALL[track];
        let (start, _) = sys.place(track, step, sys.lift(voice, song.tracks[track].steps[step].note));
        let live = playing(20.3, &[0]);
        let mut paint = crate::orbit::Paint::default();
        let press = Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        };
        let mut frames = vec![vec![Event::PointerMoved(start)], vec![press]];
        for i in 1..=8 {
            frames.push(vec![Event::PointerMoved(start + vec2(0.0, dy * i as f32 / 8.0))]);
        }
        frames.push(vec![]);
        let mut shapes = Vec::new();
        for (i, events) in frames.into_iter().enumerate() {
            shapes = snap.frame_with_events(10.0 + i as f64 * 0.016, events, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(crate::theme::BG))
                    .show(ctx, |ui| {
                        planets(ui, rect, &mut song, &live, &mut paint);
                    });
            });
        }
        snap.save(&shapes, Path::new(&format!("target/snapshots/{name}.png")));
    }

    #[test]
    #[ignore]
    fn render_note_drags() {
        render_note_drag("planets_drag_up", 7, 9, -45.0);
        render_note_drag("planets_drag_down", 6, 12, 30.0);
        render_note_drag("planets_drag_right", 7, 4, -80.0);
    }

    fn playing(pos: f64, recent: &[usize]) -> Live {
        let mut hit_age = [2.0; TRACKS];
        for &t in recent {
            hit_age[t] = 0.03;
        }
        Live {
            playing: true,
            pos,
            hit_age,
            build: None,
            bar: (pos / 16.0) as u64,
            part: None,
            bands: [0.22, 0.12, 0.05, 0.03, 0.035, 0.02, 0.015, 0.008],
            level: 0.7,
        }
    }

    #[test]
    #[ignore]
    fn render_window() {
        render("orbit_stopped", 1360.0, 860.0, |_| {});
        // Luna on the main screen, bottom right, every skin unlocked – and in a small window
        render("luna_desk", 1360.0, 860.0, |app| {
            app.luna_desk = true;
            app.luna = crate::luna::Luna {
                dust: 520,
                antenna: 2,
                color: 2,
                extra: 1,
                ..Default::default()
            };
        });
        render("luna_desk_small", 1100.0, 700.0, |app| app.luna_desk = true);
        render("orbit_playing", 1360.0, 860.0, |app| {
            app.song.auto = true;
            app.slots[0] = Some(app.song);
            app.slots[1] = Some(crate::gen::generate(
                crate::song::Vibe::Acid,
                &Default::default(),
                &mut crate::rng::Rng::new(4),
            ));
            app.slot_now = Some(0);
            app.fake_live = Some(playing(36.3, &[0, 3, 6]));
            // SOLAR OS mid-cycle: the map, the next change, the last move with its thumbs
            use std::sync::atomic::Ordering::Relaxed;
            app.song.auto_fx = true;
            let sh = &app.shared;
            sh.step.store(13 * 16 + 5, Relaxed);
            sh.cycle_len.store(32, Relaxed);
            sh.cycle_break.store(24, Relaxed);
            sh.heat.store(650, Relaxed);
            sh.cycle_depth.store(200, Relaxed);
            sh.next_change.store(16, Relaxed);
            let v = crate::song::Voice::Bass;
            app.last_auto = Some((crate::gen::Change::NewNotes(v), *app.song.track(v), -10.0));
            app.log.push("New bass notes – room for the kick", -10.0);
            app.drive = 0.4;
            app.shared.drive.store(0.4f32.to_bits(), Relaxed);
            app.song.macros.energy = 0.82;
        });
        render("luna_manual", 1360.0, 860.0, |app| {
            app.song.auto = true;
            app.help_open = true;
            app.fake_live = Some(playing(36.3, &[0, 3, 6]));
        });
        render("orbit_chain_update", 1360.0, 860.0, |app| {
            for (i, vibe) in [crate::song::Vibe::Dub, crate::song::Vibe::Rave, crate::song::Vibe::Melodic]
                .into_iter()
                .enumerate()
            {
                app.slots[i] = Some(crate::gen::generate(
                    vibe,
                    &Default::default(),
                    &mut crate::rng::Rng::new(i as u64 + 30),
                ));
            }
            app.song = app.slots[1].unwrap();
            app.slot_now = Some(1);
            app.queued_slot = Some(2);
            app.song_form = Some(1);
            app.capture = true;
            app.new_version = Some(crate::update::Release { version: "0.3.0".into() });
            app.fake_live = Some(Live {
                part: Some((4, 9.4)),
                ..playing(20.3, &[0, 7])
            });
        });
        render("orbit_build", 1360.0, 860.0, |app| {
            app.fake_live = Some(Live {
                build: Some(0.6),
                ..playing(50.5, &[1])
            });
        });
        render("orbit_small", 1100.0, 700.0, |app| {
            app.slots[0] = Some(app.song);
            app.slot_now = Some(0);
            app.fake_live = Some(playing(4.2, &[0]));
        });
        render("break_small", 1100.0, 700.0, |app| {
            app.song.macros.energy = 0.15;
            app.fake_live = Some(playing(4.2, &[3]));
        });
        render("planets", 1360.0, 860.0, |app| {
            app.view = View::Planets;
            app.sound_open = true;
            app.song.tracks[6].steps[3].slide = true;
            app.fake_live = Some(playing(36.3, &[0, 3, 6]));
        });
        render("planets_build", 1360.0, 860.0, |app| {
            app.view = View::Planets;
            app.fake_live = Some(Live {
                build: Some(0.6),
                ..playing(50.5, &[1, 3])
            });
        });
        render("planets_small", 1100.0, 700.0, |app| {
            app.view = View::Planets;
        });
        render("pads", 1360.0, 860.0, |app| {
            app.view = View::Pads;
            app.fake_live = Some(playing(9.4, &[2, 7]));
        });
        render("pads_small", 1100.0, 700.0, |app| {
            app.view = View::Pads;
        });
        render("help_small", 1100.0, 700.0, |app| {
            app.help = true;
        });
        render("visualizer", 1360.0, 860.0, |app| {
            app.visual = true;
            app.song.macros.energy = 0.8;
            app.fake_live = Some(Live {
                ..playing(66.2, &[0, 1, 3, 6])
            });
        });
        // The SOUNDS screen: a few sounds, a loop reversed and pitched, two of them in the project
        render("sounds", 1360.0, 860.0, |app| {
            use crate::song::Voice;
            let sr = crate::sampler::SAMPLE_SR;
            let mut rng = crate::rng::Rng::new(5);
            let kick: Vec<f32> = (0..(sr * 0.5) as usize)
                .map(|i| {
                    let t = i as f32 / sr;
                    (std::f32::consts::TAU * (50.0 * t + 90.0 * (1.0 - (-t / 0.03).exp()) * 0.03)).sin() * (-t / 0.18).exp()
                })
                .collect();
            let lib = &mut app.library;
            let k = lib.add("Kitchen kick", Voice::Kick, kick);
            let hits = |rng: &mut crate::rng::Rng, len: f32, every: f32| -> Vec<f32> {
                (0..(sr * len) as usize)
                    .map(|i| {
                        let t = (i as f32 / sr) % every;
                        let n = rng.noise();
                        n * (-t / 0.05).exp() * 0.8 + n * 0.01
                    })
                    .collect()
            };
            lib.add("Door knock", Voice::Rim, hits(&mut rng, 1.2, 0.3));
            lib.add("Shaker take 2", Voice::Hat, hits(&mut rng, 0.8, 0.1));
            let lp = lib.add("Pot and pan loop", Voice::Clap, hits(&mut rng, 2.0, 0.25));
            let e = lib.get_mut(lp).unwrap();
            e.edit.low_cut = 180.0;
            e.edit.pitch = 5.0;
            e.edit.reverse = true;
            app.sample_slots[0] = k;
            app.sample_slots[1] = lp;
            app.lib_ui.show_for(None);
            app.lib_ui.select(lp);
        });
        render("sounds_empty", 1100.0, 700.0, |app| app.lib_ui.show_for(None));
        render("sounds_start", 1360.0, 860.0, |app| app.lib_ui.show_start());
        render("sounds_record", 1360.0, 860.0, |app| app.lib_ui.show_record());
        render("sounds_breed", 1360.0, 860.0, |app| {
            app.lib_ui.show_breed(crate::song::Voice::Kick, 0)
        });
        render("sounds_breed_round", 1360.0, 860.0, |app| {
            app.lib_ui.show_breed(crate::song::Voice::Acid, 3)
        });
        render("sounds_template", 1360.0, 860.0, |app| {
            app.lib_ui.draw_template(crate::song::Voice::Kick)
        });
        // Just recorded: not in the list yet, Save puts it there
        render("sounds_draft", 1100.0, 700.0, |app| {
            let noise: Vec<f32> = (0..30_000)
                .map(|i| (i as f32 * 0.02).sin() * (-(i as f32) / 9000.0).exp())
                .collect();
            let id = app.library.add("Signal 1", crate::song::Voice::Rim, noise);
            app.library.set_draft(id, true);
            app.lib_ui.show_for(None);
            app.lib_ui.finish_with(id, crate::library_ui::Step::Record);
        });
        // A take right after recording, in the editor: swells and tapers like a pine cone (the website picture)
        render("sounds_recorded", 1360.0, 860.0, |app| {
            let sr = crate::sampler::SAMPLE_SR;
            let mut rng = crate::rng::Rng::new(11);
            let len = sr * 1.4;
            let take: Vec<f32> = (0..len as usize)
                .map(|i| {
                    let t = i as f32 / sr;
                    let x = i as f32 / len;
                    let env = (std::f32::consts::PI * x.powf(0.7)).sin().powf(1.4);
                    let scales = 1.0 - 0.22 * (std::f32::consts::TAU * 11.0 * t).sin().abs();
                    let tone = (std::f32::consts::TAU * 190.0 * t).sin() * 0.7 + rng.noise() * 0.3;
                    tone * env * scales * 0.9
                })
                .collect();
            let id = app.library.add("Glass tap", crate::song::Voice::Rim, take);
            app.library.set_draft(id, true);
            app.lib_ui.show_for(None);
            app.lib_ui.finish_with(id, crate::library_ui::Step::Record);
        });
        // Luna says hello as the computer comes on (render time is ~10 s)
        render("home", 1360.0, 860.0, |app| {
            app.home = true;
            // Luna a few levels in: double antenna, shades
            app.luna = crate::luna::Luna {
                dust: 330,
                antenna: 1,
                extra: 1,
                ..Default::default()
            };
            app.last_session = Some(crate::app::Session {
                song: crate::gen::generate(crate::song::Vibe::Acid, &Default::default(), &mut crate::rng::Rng::new(9)),
                slots: [None; 12],
                names: std::array::from_fn(|_| String::new()),
                project: Some("/Users/me/Music/friday-set.solarum".into()),
                samples: [0; crate::sampler::SAMPLE_SLOTS],
            });
            let mut rng = crate::rng::Rng::new(4);
            for (i, (name, when, vibe)) in [
                ("friday-set", "today", crate::song::Vibe::Peak),
                ("acid-morning", "yesterday", crate::song::Vibe::Acid),
                ("dub-sketch", "3 days ago", crate::song::Vibe::Dub),
                ("hard-one", "12 days ago", crate::song::Vibe::Hard),
                ("lost-file", "missing", crate::song::Vibe::Minimal),
            ]
            .into_iter()
            .enumerate()
            {
                let song = (i < 4).then(|| crate::gen::generate(vibe, &Default::default(), &mut rng));
                app.recent_cache.push(crate::home::Recent {
                    path: format!("/Users/me/Music/{name}.solarum").into(),
                    name: name.into(),
                    when: when.into(),
                    song,
                });
            }
        });
        render("bunk", 1100.0, 700.0, |app| {
            app.song.auto = true;
            app.power_at = 8.2;
            app.fake_live = Some(playing(2.0, &[0]));
        });
        render("knobs_small", 1100.0, 700.0, |app| {
            app.sound_open = true;
            app.song.auto = true;
            app.song.tracks[0].lock = true;
            app.song.tracks[6].lock = true;
            app.song.prog = 3;
            app.fake_live = Some(playing(12.5, &[0]));
        });
        // The README picture: docs/images/solarum.jpg (a 2000 px wide JPEG of this)
        render("readme", 1360.0, 860.0, |app| {
            use crate::song::{Vibe, Voice};
            let mut rng = crate::rng::Rng::new(77);
            for (i, vibe) in [Vibe::Dub, Vibe::Acid, Vibe::Rave].into_iter().enumerate() {
                let mut s = crate::gen::generate(vibe, &Default::default(), &mut rng);
                s.track_mut(Voice::Acid).mute = false;
                app.slots[i] = Some(s);
            }
            app.song = app.slots[1].unwrap();
            app.song.auto = true;
            app.song.auto_bars = 4;
            app.song.prog = 3;
            app.song.macros.pump = 0.4;
            app.song.tracks[0].lock = true;
            app.song.tracks[3].tune = 2;
            app.slot_now = Some(1);
            app.fx_key[2] = true;
            app.view = View::Planets;
            app.sound_open = true;
            app.song.mood_hold = true;
            app.song.macros.energy = 0.78;
            // The auto mode in the middle of a cycle, with what the log said on the way
            use std::sync::atomic::Ordering;
            app.shared.step.store(37 * 16 + 3, Ordering::Relaxed);
            app.shared.cycle_start.store(20, Ordering::Relaxed);
            app.shared.cycle_len.store(32, Ordering::Relaxed);
            app.shared.cycle_break.store(24, Ordering::Relaxed);
            app.shared.heat.store(700, Ordering::Relaxed);
            app.log = crate::log::Log::new();
            let s = &app.song;
            let mood = s.mood().map_or("", |k| crate::song::MOODS[k].0);
            let groove = format!("{} · {} · {mood} · {:.0} BPM", s.vibe.name(), s.root_name(), s.bpm);
            for line in [
                "New cycle · 32 bars · the set runs hot",
                "Drop in 16 bars",
                "New bassline",
                &groove,
                "Breakdown in 8 bars",
                "Hat shifted",
                "Tip: hold WASH (E) through the last bar before a drop",
            ] {
                app.log.push(line, -100.0);
            }
            app.fake_live = Some(Live {
                ..playing(37.3, &[0, 3, 6, 7])
            });
        });
    }

    /// The app icon from the logo: `target/snapshots/icon.png`, 1024 px with the macOS margin
    /// (the source of assets/icon.png, .icns and .ico)
    #[test]
    #[ignore]
    fn render_icon() {
        let mut snap = Offscreen::new(1024.0, 1024.0);
        let shapes = snap.frame(0.0, |ctx| {
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::new())
                .show(ctx, |ui| {
                    let r = eframe::egui::Rect::from_center_size(eframe::egui::pos2(512.0, 512.0), eframe::egui::vec2(824.0, 824.0));
                    crate::widgets::logo(ui.painter(), r, true, 0.0);
                });
        });
        snap.save(&shapes, Path::new("target/snapshots/icon.png"));
    }
}
