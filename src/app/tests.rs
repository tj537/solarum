use super::*;
use crate::offscreen::Offscreen;

fn frame(app: &mut SolarumApp, snap: &mut Offscreen, t: f64) {
    snap.frame(t, |ctx| app.ui(ctx));
}

#[test]
fn undo_and_redo_a_shuffle() {
    let mut snap = Offscreen::new(1360.0, 860.0);
    install(&snap.ctx);
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    frame(&mut app, &mut snap, 1.0);
    let original = app.song;
    app.shuffle(Vibe::Industrial, 2.0);
    frame(&mut app, &mut snap, 2.0);
    let shuffled = app.song;
    assert_ne!(shuffled, original);

    app.undo(3.0);
    frame(&mut app, &mut snap, 3.0);
    assert_eq!(app.song.tracks, original.tracks);
    app.redo(4.0);
    frame(&mut app, &mut snap, 4.0);
    assert_eq!(app.song.tracks, shuffled.tracks);
    // Redo history survives the restore itself, and a new edit clears it
    app.undo(5.0);
    frame(&mut app, &mut snap, 5.0);
    assert_eq!(app.redo.len(), 1);
    app.song.tracks[0].mute = true;
    frame(&mut app, &mut snap, 7.0);
    assert!(app.redo.is_empty());
}

#[test]
fn quick_edits_are_one_undo_step() {
    let mut snap = Offscreen::new(1360.0, 860.0);
    install(&snap.ctx);
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    frame(&mut app, &mut snap, 1.0);
    for (i, t) in [2.0, 2.1, 2.2, 2.3].into_iter().enumerate() {
        app.song.macros.energy = 0.1 * i as f32;
        frame(&mut app, &mut snap, t);
    }
    assert_eq!(app.undo.len(), 1);
}

#[test]
fn slots_save_and_load() {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    let first = app.song;
    app.slot_clicked(0, false, 0.0); // empty → save
    assert_eq!(app.slots[0], Some(first));
    app.shuffle(Vibe::Acid, 0.0);
    assert_eq!(app.slot_now, None, "a shuffle leaves the scene alone");
    app.song.master = 0.3;
    app.slot_clicked(0, false, 0.0); // full → load
    assert_eq!(app.song.tracks, first.tracks);
    assert_eq!(app.song.master, 0.3, "volume belongs to the moment");
}

#[test]
fn the_lit_scene_keeps_changes() {
    let mut snap = Offscreen::new(1360.0, 860.0);
    install(&snap.ctx);
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    frame(&mut app, &mut snap, 1.0);
    app.slot_clicked(1, false, 1.0);
    frame(&mut app, &mut snap, 1.1);
    app.song.tracks[2].steps[5].on = !app.song.tracks[2].steps[5].on;
    frame(&mut app, &mut snap, 2.0);
    assert_eq!(app.slots[1].unwrap().tracks[2], app.song.tracks[2]);
    // Undo doesn't write into a scene
    app.undo(3.0);
    frame(&mut app, &mut snap, 3.0);
    assert_eq!(app.slot_now, None);
    assert_ne!(app.slots[1].unwrap().tracks[2], app.song.tracks[2]);
}

/// The app wired to a real engine (no sound card): the engine renders on demand
fn with_engine() -> (SolarumApp, crate::engine::Engine, Receiver<Cmd>) {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    let (tx, rx) = channel();
    let (ev_tx, events) = channel();
    app.tx = tx;
    app.events = events;
    let engine = crate::engine::Engine::new(44_100.0, app.song, app.shared.clone(), Some(ev_tx), 1);
    (app, engine, rx)
}

#[test]
fn the_first_start_is_hypnotic_and_waits_for_play() {
    let (mut app, mut e, rx) = with_engine();
    let ctx = egui::Context::default();
    let mut buf = vec![0.0; 2 * 44_100];
    e.drain(&rx);
    e.render(&mut buf);
    app.sync(&ctx, 0.0, 0.016);
    assert_eq!(app.song.vibe, Vibe::Hypnotic);
    assert!(!app.playing() && !app.song.auto, "silent until play or Space");
    assert!(buf.iter().all(|s| s.abs() < 1e-3), "not a sound before play");
}

#[test]
fn a_set_recording_ends_in_the_save_dialog() {
    let (mut app, mut e, rx) = with_engine();
    let ctx = egui::Context::default();
    let mut buf = vec![0.0; 2 * 44_100];
    let mut tick = |app: &mut SolarumApp, e: &mut crate::engine::Engine, secs: usize| {
        for _ in 0..secs {
            e.drain(&rx);
            e.render(&mut buf);
            app.sync(&ctx, 0.0, 0.016);
        }
    };
    app.toggle_capture(0.0);
    app.toggle_play();
    tick(&mut app, &mut e, 3);
    app.toggle_play(); // stop the music, then the recording
    tick(&mut app, &mut e, 1);
    app.toggle_capture(0.0);
    tick(&mut app, &mut e, 3);
    assert!(matches!(app.export, Export::Recorded), "the save dialog is up");
    // Three seconds of music and two of tail – the second while it was stopped isn't in it
    let secs = app.recording.len() as f32 / 2.0 / app.rec_sr as f32;
    assert!((4.5..5.6).contains(&secs), "{secs} s recorded");
}

#[test]
fn a_favorite_goes_dark_once_the_groove_changes() {
    let (mut app, mut e, rx) = with_engine();
    let ctx = egui::Context::default();
    app.slot_clicked(0, true, 0.0);
    app.finish_frame(0.0);
    e.drain(&rx);
    assert_eq!(app.slot_now, Some(0));
    // Only the energy moves: still the saved groove (a build-up makes the engine send its song)
    e.handle(Cmd::Play);
    e.song.macros.energy = 0.2;
    e.handle(Cmd::Drop);
    app.sync(&ctx, 0.1, 0.016);
    assert!(app.song.macros.energy < 0.3, "the engine's song came in");
    assert_eq!(app.slot_now, Some(0), "energy is not the groove");
    // The auto mode changes a pattern: it's not the saved groove any more
    e.handle(Cmd::Stop);
    e.song.tracks[2].steps[5].on = !e.song.tracks[2].steps[5].on;
    e.handle(Cmd::Play);
    e.handle(Cmd::Drop);
    app.sync(&ctx, 0.2, 0.016);
    assert_eq!(app.slot_now, None);
}

#[test]
fn song_mode_and_auto_mode_take_turns() {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    // A song needs no saved patterns – it plays what you hear
    app.song.auto = true;
    app.start_song(1, 0.0);
    assert_eq!(app.song_form, Some(1));
    assert!(!app.song.auto && app.song.curve);
    app.toggle_auto(0.0);
    assert_eq!(app.song_form, None);
    app.start_song(0, 0.0);
    app.stop_song(0.0);
    assert_eq!(app.song_form, None);
}

#[test]
fn shuffle_rolls_only_the_tracks_that_are_on() {
    let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
    app.shuffle(Vibe::Peak, 0.0);
    app.song.tracks[1].mute = true;
    app.song.tracks[2].lock = true;
    let before = app.song;
    app.command("shuffle", 0.0);
    assert_eq!(app.song.tracks[1].steps, before.tracks[1].steps, "a muted track stays");
    assert_eq!(app.song.tracks[2].steps, before.tracks[2].steps, "a locked track stays");
    assert_eq!((app.song.vibe, app.song.root, app.song.bpm), (before.vibe, before.root, before.bpm));
    assert!((0..8).any(|i| app.song.tracks[i].steps != before.tracks[i].steps));
}

#[test]
fn a_seed_always_rolls_the_same_beat() {
    let mut a = SolarumApp::headless(Song::default(), View::Planets, false);
    let mut b = SolarumApp::headless(Song::default(), View::Planets, false);
    b.shuffle(Vibe::Acid, 0.0);
    b.song.vibe = a.song.vibe;
    a.command("seed tom", 0.0);
    b.command("seed Tom", 1.0);
    assert_eq!(a.song.tracks.map(|t| (t.steps, t.sound)), b.song.tracks.map(|t| (t.steps, t.sound)));
    assert_eq!(a.song.bpm, b.song.bpm);
    a.command("seed anna", 0.0);
    assert_ne!(a.song.tracks.map(|t| t.steps), b.song.tracks.map(|t| t.steps));
}

#[test]
fn new_synth_sound_gets_a_fitting_line() {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    let i = Voice::Acid.index();
    app.song.tracks[i].sound = 0;
    let before = app.song.tracks[i].steps;
    app.set_sound(i, 1, 0.0); // 303 saw → square: same line
    assert_eq!(app.song.tracks[i].steps, before);
    app.set_sound(i, 2, 0.0); // → stab: a new line
    assert_eq!(app.song.tracks[i].sound, 2);
    assert!(app.song.tracks[i].hits() <= 6, "stabs are sparse");
}

#[test]
fn bunk_runs_the_auto_mode_from_the_terminal() {
    let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
    // An empty sky: switching the computer on picks a vibe, folds the sound away and lets
    // the engine grow a groove (one track per bar – see the engine's tests)
    app.clear(0.0);
    app.sound_open = true;
    app.power(1.0);
    assert!(app.song.auto && !app.sound_open);
    assert_ne!(app.song.vibe, Vibe::Custom);
    assert!(app.log.lines().any(|l| l.contains("one track per bar")));
    app.command("vibe acid", 2.0);
    assert_eq!(app.song.vibe, Vibe::Acid);
    app.command("Hard", 3.0);
    assert_eq!(app.song.vibe, Vibe::Hard);
    app.command("every 4", 4.0);
    assert_eq!(app.song.auto_bars, 4);
    app.command("mood sun", 5.0);
    assert_eq!(app.song.mood(), Some(7));
    assert!(app.song.mood_hold);
    app.command("fx off", 6.0);
    assert!(!app.song.auto_fx);
    app.command("bpm 140", 7.0);
    assert_eq!(app.song.bpm, 140.0);
    app.command("what is this", 8.0);
    assert!(app.log.lines().last().is_some_and(|l| l.contains("try help")));
    app.command("off", 9.0);
    assert!(!app.song.auto && app.sound_open);
}

#[test]
fn favorites_belong_to_their_project() {
    let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
    app.new_project();
    assert!(!app.dirty(), "an empty new project has nothing to save");
    app.slot_clicked(0, true, 0.0);
    app.names[0] = "Intro".into();
    assert!(app.dirty());
    // Leaving it asks first; "don't save" goes on to a new, empty project
    app.home_action(crate::home::Action::New, 1.0);
    assert!(app.pending.is_some() && app.slots[0].is_some(), "it waits for the answer");
    let action = app.pending.take().unwrap();
    app.mark_saved();
    app.home_action(action, 1.0);
    assert!(app.slots.iter().all(|s| s.is_none()) && app.names.iter().all(|n| n.is_empty()));
    assert!(app.project.is_none());
    // The start screen keeps what was there for Continue
    app.slot_clicked(2, true, 2.0);
    app.open_home();
    app.new_project();
    app.home_action(crate::home::Action::Continue, 3.0);
    assert!(app.slots[2].is_some(), "Continue brings the favorites back");
}

#[test]
fn saved_and_opened_projects_lead_the_recent_list() {
    let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
    for name in ["a", "b", "a", "c"] {
        app.remember(std::path::Path::new(&format!("/tmp/{name}.solarum")));
    }
    let names: Vec<String> = app
        .recent
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["c", "a", "b"], "newest first, no doubles");
    assert_eq!(
        app.project.as_deref(),
        Some(std::path::Path::new("/tmp/c.solarum")),
        "Save writes there now"
    );
    for i in 0..20 {
        app.remember(std::path::Path::new(&format!("/tmp/{i}.solarum")));
    }
    assert_eq!(app.recent.len(), 12);
}

#[test]
fn shuffle_keeps_a_mood_picked_by_hand() {
    let mut app = SolarumApp::headless(Song::default(), View::Planets, false);
    app.song.set_mood(7);
    app.song.mood_hold = true;
    for _ in 0..5 {
        app.shuffle(Vibe::Hard, 0.0);
        assert_eq!(app.song.mood(), Some(7));
    }
    app.song.mood_hold = false;
    app.shuffle(Vibe::Hard, 0.0);
    assert!(app.song.mood().is_some_and(|k| gen::moods_for(Vibe::Hard).contains(&k)));
}

#[test]
fn shuffle_keeps_locked_tracks() {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    app.song.tracks[6].lock = true;
    app.song.tracks[6].tune = 3;
    let bass = app.song.tracks[6];
    for vibe in Vibe::ALL {
        app.shuffle(vibe, 0.0);
        assert_eq!(app.song.tracks[6], bass);
    }
    app.roll_track(6);
    assert_eq!(app.song.tracks[6], bass, "the dice leaves it alone too");
}

#[test]
fn energy_by_hand_unless_auto_fx_steers_it() {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    app.song.auto = true;
    app.song.auto_fx = true;
    let before = app.song.macros.energy;
    app.set_energy(0.2, 0.0);
    assert_eq!(app.song.macros.energy, before, "AUTO FX steers it");
    app.song.auto_fx = false;
    app.set_energy(0.2, 0.0);
    assert_eq!(app.song.macros.energy, 0.2);
    app.set_energy(3.0, 0.0);
    assert_eq!(app.song.macros.energy, 1.0);
}

#[test]
fn solarum_file_loads_everything() {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    let mut f = SolarumFile {
        song: gen::generate(Vibe::Electro, &Default::default(), &mut Rng::new(3)),
        ..Default::default()
    };
    f.scenes[1] = Some(gen::generate(Vibe::Dub, &Default::default(), &mut Rng::new(4)));
    f.names[1] = "Sunday".into();
    app.song.master = 0.4;
    app.load_file(SolarumFile::from_text(&f.to_text()).unwrap(), "test.solarum", 0.0);
    assert_eq!(app.song.tracks, f.song.tracks);
    assert_eq!(app.song.master, 0.4, "volume belongs to the moment");
    assert_eq!(app.slots.to_vec(), f.scenes);
    assert_eq!(app.names[1], "Sunday");
}

#[test]
fn solo_and_back() {
    let mut app = SolarumApp::headless(Song::default(), View::Orbit, false);
    let before = app.song.tracks.map(|t| t.mute);
    app.toggle_solo(2, 0.0);
    for (i, t) in app.song.tracks.iter().enumerate() {
        assert_eq!(t.mute, i != 2);
    }
    app.toggle_solo(5, 0.0); // switch solo to another track
    assert!(!app.song.tracks[5].mute && app.song.tracks[2].mute);
    app.toggle_solo(5, 0.0);
    assert_eq!(app.song.tracks.map(|t| t.mute), before);
}

#[test]
fn the_energy_notch_sits_under_the_pointer() {
    use super::solar_os::drive_at;
    let track = Rect::from_min_max(pos2(100.0, 0.0), pos2(300.0, 16.0));
    // The notch is drawn at middle + drive × half the width: the mapping is its inverse
    for drive in [-1.0, -0.5, 0.3, 1.0] {
        let x = track.center().x + drive * track.width() * 0.5;
        assert!((drive_at(x, track) - drive).abs() < 1e-5, "{drive}");
    }
    assert_eq!(drive_at(203.0, track), 0.0, "snaps to the middle");
    assert_eq!((drive_at(0.0, track), drive_at(999.0, track)), (-1.0, 1.0), "past the ends");
}
