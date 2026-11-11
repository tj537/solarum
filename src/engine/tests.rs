//! The engine's tests: timing, arrangement, the auto mode, songs, recording, effects – and
//! the ones that listen to the result (`crate::listen`).

use super::*;
use crate::song::Vibe;
use std::collections::HashSet;

fn stats(buf: &[f32]) -> (f32, f32) {
    let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let rms = (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt();
    (peak, rms)
}

/// Counts the allocations of the thread that asks (to prove the audio callback makes none)
struct Counting;
thread_local! {
    static ALLOCS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get().map(|n| n + 1)));
        unsafe { std::alloc::System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(p, l) }
    }
}
#[global_allocator]
static COUNTING: Counting = Counting;

#[test]
fn the_auto_mode_renders_without_allocating() {
    // Auto mode with AUTO FX over many cycles: patterns, sounds, breakdowns, drops – all
    // on the audio thread, so not one allocation (no events: those box the song)
    for vibe in [Vibe::Hard, Vibe::Dub, Vibe::Minimal] {
        let mut song = gen::generate(vibe, &Default::default(), &mut Rng::new(3));
        song.auto = true;
        song.auto_bars = 2;
        song.bpm = 180.0;
        let mut e = Engine::new(8_000.0, song, Shared::new(), None, 5);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 512];
        ALLOCS.with(|c| c.set(Some(0)));
        for _ in 0..4000 {
            e.render(&mut buf);
        }
        let n = ALLOCS.with(|c| c.replace(None)).unwrap();
        assert_eq!(n, 0, "{vibe:?}: {n} allocations on the audio thread");
    }
}

#[test]
fn a_nan_sound_plays_silence_and_asks_for_a_new_engine() {
    use crate::sampler::{SampleBuf, CUSTOM};
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
    song.track_mut(Voice::Clap).sound = CUSTOM;
    let mut slots: crate::sampler::Slots = Default::default();
    slots[0] = Some(Arc::new(SampleBuf {
        data: vec![f32::NAN; 4000],
        pitch: 0.0,
    }));
    let shared = Shared::new();
    let mut e = Engine::new(44_100.0, song, shared.clone(), None, 5);
    e.handle(Cmd::Samples(Box::new(slots)));
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * e.step_len() as usize * 16];
    e.render(&mut buf);
    assert!(buf.iter().all(|x| x.is_finite()));
    assert!(shared.broken.load(Ordering::Relaxed));
}

#[test]
fn your_own_sounds_play_on_their_track_without_allocating() {
    use crate::sampler::{SampleBuf, CUSTOM};
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
    for t in song.tracks.iter_mut() {
        t.mute = true;
    }
    let clap = song.track_mut(Voice::Clap);
    clap.mute = false;
    clap.sound = CUSTOM + 3;
    let buf = Arc::new(SampleBuf {
        data: (0..4000).map(|i| (i as f32 * 0.07).sin() * 0.8).collect(),
        pitch: 0.0,
    });
    let mut slots: crate::sampler::Slots = Default::default();
    slots[3] = Some(buf.clone());
    let mut e = Engine::new(44_100.0, song, Shared::new(), None, 5);
    e.handle(Cmd::Samples(Box::new(slots)));
    e.handle(Cmd::Play);
    let mut out = vec![0.0; 1024];
    let mut peak = 0.0f32;
    ALLOCS.with(|c| c.set(Some(0)));
    for _ in 0..200 {
        e.render(&mut out);
        peak = out.iter().fold(peak, |m, x| m.max(x.abs()));
    }
    let n = ALLOCS.with(|c| c.replace(None)).unwrap();
    assert_eq!(n, 0, "{n} allocations on the audio thread");
    assert!(peak > 0.1 && peak <= 1.0, "{peak}");
    // The audition plays while stopped, and says where it is
    e.handle(Cmd::Stop);
    e.handle(Cmd::Audition(Some(buf), false));
    e.render(&mut out);
    assert!(e.shared.audition.load(Ordering::Relaxed) < 4000);
}

#[test]
fn every_vibe_sounds_like_a_mix_not_a_brick() {
    // Three grooves of every vibe, heard like an ear would (`listen`): loud enough, never
    // clipped, not squashed flat, no offset, lows that carry without drowning the rest,
    // highs that are there, and a stereo picture that is neither mono nor falling apart
    use crate::listen;
    for (i, vibe) in Vibe::ALL.into_iter().enumerate() {
        for seed in 0..3u64 {
            let song = gen::generate(vibe, &Default::default(), &mut Rng::new(i as u64 * 10 + seed));
            let buf = render_offline(&song, 8, 44_100, 1);
            assert!(buf.iter().all(|x| x.is_finite()));
            let h = listen::hear(&buf[..buf.len() - 44_100 * 4], 44_100.0);
            let what = format!("{vibe:?} {seed}: {}", h.line());
            assert!(h.peak <= crate::master::CEILING * song.master + 1e-4, "clips – {what}");
            assert!((-18.0..=-9.5).contains(&h.lufs), "loudness – {what}");
            assert!(h.crest_db >= 8.5, "squashed – {what}");
            assert!(h.dc < 1e-3, "offset – {what}");
            assert!((-24.0..=-5.0).contains(&h.side_db), "stereo – {what}");
            let [sub, bass, _, mid, pres, air] = h.bands;
            assert!(sub < -1.0 && bass < -2.0, "the lows drown the rest – {what}");
            assert!(sub.max(bass) > -8.0, "no low end – {what}");
            assert!(mid.max(pres) > -22.0 && air > -26.0, "dull – {what}");
        }
    }
}

#[test]
fn the_sounds_of_a_track_are_about_equally_loud() {
    // Switching a track's sound changes its colour, not the mix: every sound within
    // 3.5 dB (as the ear weighs it) of the middle one
    let base = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(11));
    for (i, v) in Voice::ALL.into_iter().enumerate() {
        let mut loud: Vec<(f32, &str)> = (0..v.sounds().len())
            .map(|sound| {
                let buf = render_offline(&levels::solo(&base, i, sound as u8), 2, 44_100, 1);
                (crate::listen::loudness(&buf[..buf.len() - 44_100 * 4], 44_100.0), v.sounds()[sound])
            })
            .collect();
        loud.sort_by(|a, b| a.0.total_cmp(&b.0));
        let middle = loud[loud.len() / 2].0;
        for (lufs, name) in loud {
            assert!(
                (lufs - middle).abs() < 3.5,
                "{v:?} {name}: {lufs:.1} LUFS, the others around {middle:.1}"
            );
        }
    }
}

#[test]
fn steps_fire_on_time() {
    let mut song = Song::empty();
    song.bpm = 120.0;
    let shared = Shared::new();
    let mut e = Engine::new(48_000.0, song, shared.clone(), None, 1);
    e.handle(Cmd::Play);
    // 120 bpm → one 16th = 0.125 s = 6000 samples
    let mut buf = vec![0.0; 47_000 * 2];
    e.render(&mut buf);
    assert_eq!(shared.step.load(Ordering::Relaxed), 7);
    e.render(&mut buf[..2 * 2000]);
    assert_eq!(shared.step.load(Ordering::Relaxed), 8);
}

#[test]
fn by_hand_the_pattern_plays_exactly_as_written() {
    // One hat and one rim, energy high and low: no ghost hats, nothing thinned out
    for energy in [0.95, 0.35] {
        let mut song = Song::empty();
        song.bpm = 200.0;
        song.macros.energy = energy;
        song.track_mut(Voice::Hat).steps[2] = crate::song::Step::hit();
        song.track_mut(Voice::Rim).steps[6] = crate::song::Step::hit();
        let shared = Shared::new();
        let mut e = Engine::new(22_050.0, song, shared.clone(), None, 3);
        e.handle(Cmd::Play);
        let bars = 8;
        let mut buf = vec![0.0; 2 * (e.step_len() * (STEPS * bars) as f64) as usize];
        e.render(&mut buf);
        let hits = |v: Voice| shared.hits[v.index()].load(Ordering::Relaxed) as usize;
        assert_eq!(hits(Voice::Hat), bars, "energy {energy}");
        assert_eq!(hits(Voice::Rim), bars, "energy {energy}");
    }
}

#[test]
fn a_fill_by_hand_lands_in_the_second_half_of_a_bar() {
    let mut song = Song::empty();
    song.bpm = 200.0;
    let shared = Shared::new();
    let mut e = Engine::new(22_050.0, song, shared.clone(), None, 3);
    e.handle(Cmd::Fill(Fill::Roll));
    assert_eq!(e.fill, None, "not while stopped");
    e.handle(Cmd::Play);
    let step = e.step_len() as usize;
    let mut buf = vec![0.0; 2 * step * 3];
    e.render(&mut buf);
    e.handle(Cmd::Fill(Fill::Roll));
    assert_eq!(e.fill, Some((0, Fill::Roll)), "early in the bar: this one");
    let mut buf = vec![0.0; 2 * step * 7];
    e.render(&mut buf);
    e.handle(Cmd::Fill(Fill::Toms));
    assert_eq!(e.fill, Some((1, Fill::Toms)), "late in the bar: the next one");
    // The roll played: claps although the clap track is empty
    assert!(shared.hits[Voice::Clap.index()].load(Ordering::Relaxed) > 0);
}

#[test]
fn auto_mode_arranges_breakdown_build_and_drop() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    song.auto = true;
    song.auto_bars = 4;
    song.bpm = 200.0; // fast, to keep the test short
    let mut e = Engine::new(22_050.0, song, Shared::new(), Some(tx), 9);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 33];
    e.render(&mut buf);
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert!(changes.contains(&Change::Breakdown), "{changes:?}");
    assert!(changes.contains(&Change::Build));
    assert!(matches!(changes.last(), Some(Change::Drop | Change::NewChapter)), "{changes:?}");
    // Changes every 4 bars before the breakdown
    assert!(changes.len() >= 5 + 3, "{changes:?}");
    assert!(!e.song.track(Voice::Kick).mute);
}

#[test]
fn breakdown_ends_when_auto_mode_stops() {
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    song.auto = true;
    song.bpm = 200.0;
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 9);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 25 + 200];
    e.render(&mut buf); // into bar 24: breakdown
    assert!(e.song.track(Voice::Kick).mute);
    let mut off = e.song;
    off.auto = false;
    e.handle(Cmd::Song(Box::new(off)));
    assert!(!e.song.track(Voice::Kick).mute && !e.song.track(Voice::Bass).mute);

    // Same when the music stops
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 9);
    e.handle(Cmd::Play);
    e.render(&mut buf);
    e.handle(Cmd::Stop);
    assert!(!e.song.track(Voice::Kick).mute);
}

#[test]
fn manual_drop_builds_two_bars_from_the_next_bar() {
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = Shared::new();
    let mut e = Engine::new(22_050.0, Song::default(), shared.clone(), Some(tx), 2);
    e.handle(Cmd::Play);
    let step = e.step_len() as usize;
    let mut buf = vec![0.0; 2 * step * 5];
    e.render(&mut buf); // into bar 0
    e.handle(Cmd::Drop);
    assert!(e.build.is_some());
    // Waiting for bar 1: the beat plays on, the button already shows it
    let kicks = |s: &Shared| s.hits[Voice::Kick.index()].load(Ordering::Relaxed);
    let before = kicks(&shared);
    let mut buf = vec![0.0; 2 * step * 10];
    e.render(&mut buf);
    assert!(kicks(&shared) > before, "kick until the bar is over");
    assert!(shared.build_progress().is_some());
    assert_eq!(e.build_progress(e.phase), None);
    // Build-up through bars 1 and 2, the drop on bar 3
    let mut buf = vec![0.0; 2 * step * 32];
    e.render(&mut buf);
    assert!(e.build.is_some(), "still building on step 47");
    let mut buf = vec![0.0; 2 * step * 2];
    e.render(&mut buf);
    assert!(e.build.is_none(), "drop on step 48");
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert_eq!(changes, vec![Change::Build, Change::Drop]);
}

#[test]
fn build_up_turns_the_groove_into_a_roll_and_breathes_before_the_drop() {
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(4));
    song.macros.energy = 0.9;
    for v in Voice::ALL {
        song.track_mut(v).mute = false;
    }
    let shared = Shared::new();
    let mut e = Engine::new(22_050.0, song, shared.clone(), None, 1);
    e.handle(Cmd::Play);
    let step = e.step_len() as usize;
    let hits = |v: Voice| shared.hits[v.index()].load(Ordering::Relaxed);
    // Windows end in the middle of a step, away from the step boundaries
    let mut buf = vec![0.0; 2 * (step * 15 + step / 2)];
    e.render(&mut buf);
    e.handle(Cmd::Drop);
    let count = |e: &mut Engine, steps: usize, v: Voice| {
        let before = hits(v);
        let mut buf = vec![0.0; 2 * step * steps];
        e.render(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        hits(v) - before
    };
    // Kick roll: quarters in the first bar, 8ths, then 16ths – up to the gap
    assert_eq!(count(&mut e, 16, Voice::Kick), 4);
    assert_eq!(count(&mut e, 8, Voice::Kick), 4);
    assert_eq!(count(&mut e, 6, Voice::Kick), 6);
    // The gap: nothing new starts in the last half beat
    let before = Voice::ALL.map(hits);
    let mut buf = vec![0.0; 2 * step * 2];
    e.render(&mut buf);
    assert_eq!(Voice::ALL.map(hits), before, "silence before the drop");
    assert!(count(&mut e, 4, Voice::Kick) > 0 && e.build.is_none(), "the drop");
}

#[test]
fn build_up_loops_the_riff_and_lifts_it() {
    let start = 32;
    assert_eq!(build_play(Voice::Acid, 0.2, 36, start), BuildPlay::Pattern);
    assert_eq!(build_play(Voice::Acid, 0.6, 54, start), BuildPlay::Loop { at: 50, up: 0 });
    assert_eq!(build_play(Voice::Bass, 0.8, 59, start), BuildPlay::Loop { at: 49, up: 12 });
    assert_eq!(build_play(Voice::Rim, 0.8, 59, start), BuildPlay::Loop { at: 49, up: 0 });
    assert_eq!(build_play(Voice::OpenHat, 0.6, 50, start), BuildPlay::Rest);
    assert!(matches!(build_play(Voice::Hat, 0.6, 51, start), BuildPlay::Hit(_)));
}

#[test]
fn stopped_the_energy_still_shows_what_it_mutes() {
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    song.macros.energy = 1.0;
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    let mut s = e.song;
    s.macros.energy = 0.25;
    e.handle(Cmd::Song(Box::new(s)));
    assert!(
        e.song.track(Voice::Kick).mute && e.song.track(Voice::Bass).mute,
        "break: kick and bass out"
    );
    assert!(!e.song.track(Voice::Hat).mute);
    let mut s = e.song;
    s.macros.energy = 1.0;
    e.handle(Cmd::Song(Box::new(s)));
    assert!(!e.song.track(Voice::Kick).mute, "and back");
}

#[test]
fn energy_mutes_survive_songs_that_crossed_them() {
    // The UI keeps sending songs while the fader moves; some still carry the mutes from
    // before the engine changed them – they mustn't leave tracks muted at 100
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    song.macros.energy = 1.0;
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    let ui = e.song; // what the UI has: nothing muted
    for v in [0.6, 0.3, 0.1] {
        let mut s = ui;
        s.macros.energy = v;
        e.handle(Cmd::Song(Box::new(s)));
    }
    assert!(e.song.track(Voice::Kick).mute, "down: the kick is out");
    // Back up, still with the UI's old (unmuted) tracks, then once with the engine's
    let mut s = ui;
    s.macros.energy = 1.0;
    e.handle(Cmd::Song(Box::new(s)));
    let mut s = e.song;
    s.macros.energy = 1.0;
    e.handle(Cmd::Song(Box::new(s)));
    assert!(
        e.song.tracks.iter().all(|t| !t.mute) || e.song.tracks.iter().zip(&song.tracks).all(|(a, b)| a.mute == b.mute),
        "at 100 everything is back: {:?}",
        e.song.tracks.map(|t| t.mute)
    );
}

#[test]
fn a_new_groove_drops_the_energys_old_mutes() {
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    song.macros.energy = 1.0;
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    let mut s = e.song;
    s.macros.energy = 0.1;
    e.handle(Cmd::Song(Box::new(s)));
    assert!(e.song.track(Voice::Kick).mute);
    // A new project: other steps, nothing muted, energy full – nothing may stay muted
    let mut fresh = Song::empty();
    fresh.macros.energy = 1.0;
    fresh.track_mut(Voice::Kick).steps[0].on = true;
    e.handle(Cmd::Song(Box::new(fresh)));
    assert!(e.song.tracks.iter().all(|t| !t.mute), "{:?}", e.song.tracks.map(|t| t.mute));
}

#[test]
fn energy_by_hand_never_drops_by_itself() {
    // No drop zone any more: the energy all the way up is just everything playing
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * e.step_len() as usize];
    e.render(&mut buf);
    for v in [0.6, 0.95, 1.0] {
        let mut s = e.song;
        s.macros.energy = v;
        e.handle(Cmd::Song(Box::new(s)));
        e.render(&mut buf);
    }
    assert!(e.build.is_none());
    e.handle(Cmd::Drop);
    assert!(e.build.is_some(), "DROP drops");
}

#[test]
fn recording_writes_into_the_pattern() {
    let mut song = Song::empty();
    song.bpm = 120.0;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut e = Engine::new(48_000.0, song, Shared::new(), Some(tx), 1);
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * 6000 * 3 + 2 * 1000];
    e.render(&mut buf); // a little into step 3
    e.handle(Cmd::Hit {
        voice: Voice::Clap,
        vel: 1.0,
        record: true,
    });
    assert!(e.song.track(Voice::Clap).steps[3].on);
    assert!(rx.try_recv().is_ok());
}

#[test]
fn a_recorded_hit_is_heard() {
    let mut song = Song::empty();
    song.macros.energy = 0.6;
    let shared = Shared::new();
    let mut e = Engine::new(22_050.0, song, shared.clone(), None, 1);
    e.handle(Cmd::Play);
    let step = e.step_len() as usize;
    let mut buf = vec![0.0; 2 * step * 3 + 100];
    e.render(&mut buf);
    // The ride only plays from energy 70 – recording it lets it in
    e.handle(Cmd::Hit {
        voice: Voice::Ride,
        vel: 0.9,
        record: true,
    });
    assert!(e.song.macros.energy >= LAYERS[Voice::Ride.index()]);
    let before = shared.hits[Voice::Ride.index()].load(Ordering::Relaxed);
    let mut buf = vec![0.0; 2 * step * 16];
    e.render(&mut buf);
    assert_eq!(
        shared.hits[Voice::Ride.index()].load(Ordering::Relaxed) - before,
        1,
        "the loop plays it"
    );
}

#[test]
fn auto_mode_varies_and_always_brings_the_beat_back() {
    // Several cycles: different lengths, breakdowns, build-ups, fills, chapters – and
    // outside a breakdown the kick always plays
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(8));
    song.auto = true;
    song.auto_bars = 2;
    song.bpm = 180.0;
    let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    let (mut kinds, mut lens) = (HashSet::new(), HashSet::new());
    for b in 0..(32 * 10) {
        e.render(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        if e.breakdown.is_none() && e.build.is_none() && b > 0 {
            assert!(!e.song.track(Voice::Kick).mute, "kick plays (bar {b})");
        }
        kinds.insert(e.plan.breakdown_at * 100 + e.plan.out.len() as u64 * 10 + e.plan.sweep as u64);
        lens.insert(e.cycle_len);
    }
    assert!(kinds.len() >= 2, "different breakdowns");
    assert!(lens.len() >= 2, "different cycle lengths: {lens:?}");
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert!(changes.iter().any(|c| matches!(c, Change::NewSound(..))), "{changes:?}");
    assert!(changes.iter().filter(|c| matches!(c, Change::Drop | Change::NewChapter)).count() >= 5);
}

#[test]
fn a_track_you_edit_while_the_auto_mode_plays_stays_yours_for_two_phrases() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Acid, &Default::default(), &mut Rng::new(8));
    song.auto = true;
    song.auto_bars = 1;
    song.bpm = 180.0;
    let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    for _ in 0..4 {
        e.render(&mut buf);
    }
    let mut mine = e.song;
    let s = &mut mine.track_mut(Voice::Bass).steps[3];
    s.on = !s.on;
    e.handle(Cmd::Song(Box::new(mine)));
    let _ = rx.try_iter().count();
    for _ in 0..15 {
        e.render(&mut buf);
        assert_eq!(e.song.track(Voice::Bass).steps, mine.track(Voice::Bass).steps);
    }
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert!(changes.iter().filter(|c| c.voice().is_some()).count() >= 8, "{changes:?}");
    assert!(changes.iter().all(|c| c.voice() != Some(Voice::Bass)), "{changes:?}");
    // What you made is the theme now: the auto mode comes back to it, not to the old line
    assert_eq!(e.home[Voice::Bass.index()].steps, mine.track(Voice::Bass).steps);
}

#[test]
fn turnup_heats_the_set_and_chill_cools_it() {
    let sh = Shared::new();
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
    let mut e = Engine::new(11_025.0, song, sh.clone(), None, 1);
    e.handle(Cmd::Play);
    let (heat, mid) = (sh.heat.load(Ordering::Relaxed), e.curve_target(12));
    e.handle(Cmd::Drive(1.0));
    assert!(sh.heat.load(Ordering::Relaxed) > heat && e.curve_target(12) > mid);
    e.handle(Cmd::Drive(-1.0));
    assert!(sh.heat.load(Ordering::Relaxed) < heat && e.curve_target(12) < mid);
}

#[test]
fn the_director_explains_some_of_its_moves() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
    song.auto = true;
    song.auto_bars = 2;
    song.bpm = 180.0;
    let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 6);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    for _ in 0..(32 * 6) {
        e.render(&mut buf);
    }
    let whys: Vec<_> = rx
        .try_iter()
        .filter_map(|ev| if let Event::Song(_, _, _, w) = ev { w } else { None })
        .collect();
    assert!(whys.len() >= 3, "{whys:?}");
    assert!(whys.contains(&crate::ear::Why::Home), "the theme comes back now and then: {whys:?}");
}

#[test]
fn without_auto_fx_the_auto_mode_never_drops_by_itself() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(8));
    song.auto = true;
    song.auto_fx = false;
    song.bpm = 180.0;
    let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 70];
    e.render(&mut buf);
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert!(
        !changes
            .iter()
            .any(|c| matches!(c, Change::Breakdown | Change::Build | Change::Drop)),
        "{changes:?}"
    );
    assert!(changes
        .iter()
        .any(|c| matches!(c, Change::NewPattern(_) | Change::NewSound(..) | Change::Shifted(_))));
    assert!(
        !changes.iter().any(|c| matches!(c, Change::Muted(_) | Change::Unmuted(_))),
        "the bar decides what plays"
    );
    assert_eq!(e.song.macros.energy, song.macros.energy, "the energy is yours");
}

#[test]
fn at_85_everything_plays_and_the_engine_counts_edits() {
    assert!(LAYERS.iter().all(|&l| l < 0.85), "{LAYERS:?}");
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2));
    song.macros.energy = 0.85;
    for t in song.tracks.iter_mut() {
        t.mute = false;
    }
    let shared = Shared::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut e = Engine::new(22_050.0, song, shared.clone(), Some(tx), 1);
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * (e.step_len() * STEPS as f64 * 2.0) as usize];
    e.render(&mut buf);
    for (i, t) in song.tracks.iter().enumerate() {
        if t.hits() > 0 {
            assert!(shared.hits[i].load(Ordering::Relaxed) > 0, "{:?} plays at 85", Voice::ALL[i]);
        }
    }
    // Every song the engine sends says how many edits it had taken in
    e.handle(Cmd::Song(Box::new(song)));
    e.handle(Cmd::Drop);
    let acks: Vec<u64> = rx
        .try_iter()
        .filter_map(|ev| if let Event::Song(_, _, n, _) = ev { Some(n) } else { None })
        .collect();
    assert_eq!(acks.last(), Some(&1));
}

#[test]
fn a_drop_by_hand_pushes_the_next_auto_drop_back() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(8));
    song.auto = true;
    song.bpm = 180.0;
    let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 4);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 20 + 200];
    e.render(&mut buf); // into bar 20, just before the auto breakdown
    e.handle(Cmd::Drop); // build-up in bars 21–22, drop on 23
    let mut buf = vec![0.0; 2 * bar * 3];
    e.render(&mut buf);
    assert_eq!(e.cycle_start, 23, "the drop starts a new cycle");
    let _ = rx.try_iter().count();
    let mut buf = vec![0.0; 2 * bar * 20];
    e.render(&mut buf);
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert!(!changes.contains(&Change::Breakdown), "no breakdown right after: {changes:?}");
    assert!(!changes.contains(&Change::Drop));
}

#[test]
fn the_set_climbs_to_a_peak_and_breathes_out() {
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
    let mut e = Engine::new(11_025.0, song, Shared::new(), None, 7);
    let (mut peaks, mut releases) = (0, 0);
    for c in 0..60 {
        let (heat, plan) = (e.heat, e.plan);
        e.new_cycle(c * 32);
        assert_ne!(e.plan, plan, "never the same breakdown twice in a row");
        if heat >= 0.9 {
            assert!(e.heat < 0.5, "after the peak the set breathes out: {}", e.heat);
            releases += 1;
        } else {
            assert!(e.heat > heat, "it climbs: {heat} → {}", e.heat);
        }
        if e.heat >= 0.9 {
            peaks += 1;
            assert_eq!(e.cycle_len, 32, "the peak comes quick");
            assert!(e.has_breakdown() && e.plan.depth <= 0.2, "the peak strips down: {:?}", e.plan);
        }
    }
    assert!(peaks >= 10 && releases >= 10, "{peaks} peaks, {releases} releases");
}

#[test]
fn the_energy_climbs_in_phrases_and_the_heat_sets_how_high() {
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
    let mut e = Engine::new(11_025.0, song, Shared::new(), None, 7);
    e.cycle_len = 48;
    e.plan = PLANS[0];
    e.heat = 0.5;
    // One level per phrase, a step up from phrase to phrase
    for b in 8..e.breakdown_at() {
        let phrase = (b - 8) / 8 * 8 + 8;
        assert_eq!(e.curve_target(b), e.curve_target(phrase), "bar {b}");
    }
    assert!(e.curve_target(16) > e.curve_target(8));
    // In the breakdown it creeps up, and the build-up climbs from there
    let at = e.breakdown_at();
    assert!(e.curve_target(at) < BREAK_ZONE && e.curve_target(at + 5) > e.curve_target(at));
    assert!(e.curve_target(e.cycle_len - 1) > e.curve_target(e.cycle_len - 2));
    let warm = e.curve_target(at - 1);
    e.heat = 1.0;
    assert!(e.curve_target(at - 1) > warm + 0.05, "a hot cycle runs higher");
    assert!(e.curve_target(at - 1) < 1.0 && e.curve_target(0) < 1.0);
}

#[test]
fn a_roller_cycle_builds_and_drops_without_a_breakdown() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    song.auto = true;
    song.bpm = 200.0;
    let mut e = Engine::new(22_050.0, song, Shared::new(), Some(tx), 9);
    e.handle(Cmd::Play);
    e.plan = PLANS[4];
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 33];
    e.render(&mut buf);
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert!(!changes.contains(&Change::Breakdown), "{changes:?}");
    assert!(changes.contains(&Change::Build));
    assert!(
        changes.iter().any(|c| matches!(c, Change::Drop | Change::NewChapter)),
        "{changes:?}"
    );
}

#[test]
fn live_effects_stay_clean() {
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * 22_050];
    e.render(&mut buf);
    for fx in Fx::ALL {
        e.handle(Cmd::Fx(fx, true));
        e.render(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0), "{fx:?}");
        if fx == Fx::Tape {
            // Fully stopped after a second
            let tail = &buf[buf.len() - 2000..];
            assert!(tail.iter().all(|x| x.abs() < 1e-3), "tape stop ends in silence");
        }
        e.handle(Cmd::Fx(fx, false));
        e.render(&mut buf);
    }
    let (peak, rms) = stats(&buf);
    assert!(peak > 0.1 && rms > 0.02, "back to normal after the effects");
}

fn patterns(bpm: f32) -> [Song; 3] {
    [Vibe::Dub, Vibe::Peak, Vibe::Hard].map(|v| {
        let mut s = gen::generate(v, &Default::default(), &mut Rng::new(v as u64 + 1));
        s.bpm = bpm;
        s
    })
}

#[test]
fn a_song_plays_its_parts_with_the_patterns() {
    let (tx, rx) = std::sync::mpsc::channel();
    let [a, b, c] = patterns(180.0);
    let mut e = Engine::new(11_025.0, a, Shared::new(), Some(tx), 1);
    e.handle(Cmd::Arrange(Some(Box::new(Arrangement {
        patterns: [Some(a), Some(b), Some(c), None],
        form: 0,
        fallback: a,
    }))));
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    let mut kick_in_break = None;
    for _ in 0..(FORMS[0].bars() + 2) {
        e.render(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        if e.breakdown.is_some() {
            kick_in_break = Some(e.song.track(Voice::Kick).mute);
        }
    }
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    let parts: Vec<Part> = changes
        .iter()
        .filter_map(|c| if let Change::Part(p, ..) = c { Some(*p) } else { None })
        .collect();
    assert_eq!(
        parts,
        vec![
            Part::Intro,
            Part::Groove,
            Part::Break,
            Part::Peak,
            Part::Break,
            Part::Peak,
            Part::Outro
        ]
    );
    // Favorite 1 until the first drop, favorite 2 after it, favorite 1 for the last drop
    let slots: Vec<Option<usize>> = changes
        .iter()
        .filter_map(|c| if let Change::Part(_, _, slot) = c { Some(*slot) } else { None })
        .collect();
    assert_eq!(slots, vec![Some(0), Some(0), Some(0), Some(1), Some(1), Some(0), Some(0)]);
    assert_eq!(changes.iter().filter(|c| **c == Change::Drop).count(), 2, "two drops");
    // The grooves keep moving
    assert!(changes.iter().any(|c| matches!(
        c,
        Change::NewPattern(_) | Change::NewSound(..) | Change::NewNotes(_) | Change::Shifted(_) | Change::Length(..)
    )));
    assert_eq!(kick_in_break, Some(true), "no kick in the break");
    let build = changes.iter().position(|c| *c == Change::Build).expect("a build-up");
    let drop = changes.iter().position(|c| *c == Change::Drop).expect("a drop");
    assert!(build < drop);
    assert_eq!(changes.last(), Some(&Change::SongEnd));
    assert!(!e.playing && e.arrangement.is_none(), "the song ends");
}

#[test]
fn a_song_is_recorded_with_its_tail() {
    let (tx, rx) = std::sync::mpsc::channel();
    let [a, b, _] = patterns(180.0);
    let sr = 11_025.0;
    let mut e = Engine::new(sr, a, Shared::new(), Some(tx), 1);
    e.handle(Cmd::Arrange(Some(Box::new(Arrangement {
        patterns: [Some(a), Some(b), None, None],
        form: 0,
        fallback: a,
    }))));
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    for _ in 0..(FORMS[0].bars() + 3) {
        e.render(&mut buf);
    }
    let (mut samples, mut done) = (0, false);
    for ev in rx.try_iter() {
        match ev {
            Event::Audio(piece) => {
                assert!(!done, "nothing after the end");
                samples += piece.len();
            }
            Event::Recorded => done = true,
            Event::Song(..) => {}
        }
    }
    assert!(done, "the recording is complete");
    let frames = samples / 2;
    let song = FORMS[0].bars() as usize * bar;
    let tail = (sr * 2.0) as usize;
    assert!(
        frames >= song + tail - bar && frames <= song + tail + bar,
        "{frames} vs {}",
        song + tail
    );
}

#[test]
fn a_session_is_recorded_across_stops_until_switched_off() {
    let (tx, rx) = std::sync::mpsc::channel();
    let sr = 11_025.0;
    let mut e = Engine::new(sr, Song::default(), Shared::new(), Some(tx), 1);
    e.handle(Cmd::Capture(true));
    e.handle(Cmd::Play);
    let second = vec![0.0; 2 * sr as usize];
    let mut buf = second.clone();
    e.render(&mut buf);
    e.handle(Cmd::Stop); // a pause in the jam – the recording waits, it doesn't end
    e.render(&mut buf);
    e.handle(Cmd::Play);
    e.render(&mut buf);
    e.handle(Cmd::Capture(false));
    for _ in 0..3 {
        e.render(&mut buf); // the tail, then it's done
    }
    let (mut samples, mut done) = (0, 0);
    for ev in rx.try_iter() {
        match ev {
            Event::Audio(p) => samples += p.len(),
            Event::Recorded => done += 1,
            Event::Song(..) => {}
        }
    }
    assert_eq!(done, 1);
    let secs = samples as f32 / 2.0 / sr;
    assert!((3.9..=4.1).contains(&secs), "2 s played (the pause left out) + 2 s tail: {secs}");
}

#[test]
fn a_jump_goes_to_a_part_on_the_next_bar() {
    let (tx, rx) = std::sync::mpsc::channel();
    let [a, b, _] = patterns(180.0);
    let mut e = Engine::new(11_025.0, a, Shared::new(), Some(tx), 1);
    e.handle(Cmd::Arrange(Some(Box::new(Arrangement {
        patterns: [Some(a), Some(b), None, None],
        form: 1,
        fallback: a,
    }))));
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 2 + 100];
    e.render(&mut buf);
    e.handle(Cmd::Jump(3)); // the first peak
    let mut buf = vec![0.0; 2 * bar];
    e.render(&mut buf);
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert!(changes.contains(&Change::Drop), "a peak starts with a drop: {changes:?}");
    // The first peak plays favorite 2
    assert!(changes.contains(&Change::Part(Part::Peak, 3, Some(1))), "{changes:?}");
    assert_eq!(e.section, 3);
    assert!(e.song.macros.energy > 0.9);
}

#[test]
fn a_pattern_picked_while_playing_comes_in_on_the_next_bar() {
    let (tx, rx) = std::sync::mpsc::channel();
    let [a, b, _] = patterns(130.0);
    let mut e = Engine::new(22_050.0, a, Shared::new(), Some(tx), 1);
    e.handle(Cmd::Play);
    let step = e.step_len() as usize;
    let mut buf = vec![0.0; 2 * step * 5];
    e.render(&mut buf);
    e.handle(Cmd::Queue(Box::new(b), 1));
    assert_eq!(e.song.tracks.map(|t| t.steps), a.tracks.map(|t| t.steps), "not yet");
    assert_eq!(e.fill, None, "by hand no clap roll leads into it");
    let mut buf = vec![0.0; 2 * step * 12];
    e.render(&mut buf);
    assert_eq!(e.song.tracks.map(|t| t.steps), b.tracks.map(|t| t.steps));
    let changes: Vec<Change> = rx.try_iter().filter_map(Event::change).collect();
    assert_eq!(changes, vec![Change::Scene(1)]);
    // With SOLAR OS on, a clap roll leads into the next one
    e.song.auto = true;
    e.handle(Cmd::Queue(Box::new(a), 0));
    assert!(matches!(e.fill, Some((_, Fill::Roll))), "a clap roll leads into it");
}

#[test]
fn energy_strips_the_groove_and_the_drop_brings_it_back() {
    let mut song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(4));
    song.macros.energy = 0.0;
    let shared = Shared::new();
    let mut e = Engine::new(22_050.0, song, shared.clone(), None, 1);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    e.render(&mut buf);
    let hits = |v: Voice| shared.hits[v.index()].load(Ordering::Relaxed);
    assert_eq!(hits(Voice::Kick), 0, "no kick at zero energy");
    assert!(hits(Voice::Hat) > 0, "hats stay");
    e.handle(Cmd::Drop);
    let mut buf = vec![0.0; 2 * bar * 2 + 400];
    e.render(&mut buf);
    assert!(e.song.macros.energy >= 0.85, "the drop brings the energy back");
    let before = hits(Voice::Kick);
    let mut buf = vec![0.0; 2 * bar];
    e.render(&mut buf);
    assert!(hits(Voice::Kick) >= before + 4);
}

#[test]
fn auto_fx_moves_the_energy_until_you_switch_it_off() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(4));
    song.auto = true;
    song.bpm = 180.0;
    let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 1);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 27 + 100];
    e.render(&mut buf); // into the breakdown – the energy glides down over two bars
    assert!(e.song.macros.energy < 0.3, "low in the breakdown: {}", e.song.macros.energy);
    // It glided: no step bigger than the glide between two updates sent to the UI
    let sent: Vec<f32> = rx
        .try_iter()
        .filter_map(|ev| if let Event::Song(s, ..) = ev { Some(s.macros.energy) } else { None })
        .collect();
    assert!(sent.iter().any(|&x| x > 0.85), "high after the start: {sent:?}");
    for w in sent.windows(2) {
        assert!((w[1] - w[0]).abs() <= 4.0 * GLIDE + 1e-3, "a jump: {} → {}", w[0], w[1]);
    }
    // The UI can't override it while the curve is on …
    let mut ui_song = e.song;
    ui_song.macros.energy = 0.9;
    e.handle(Cmd::Song(Box::new(ui_song)));
    assert!(e.song.macros.energy < 0.3);
    // … but with AUTO FX off the energy is the UI's (and no breakdown holds tracks out)
    ui_song.auto_fx = false;
    e.handle(Cmd::Song(Box::new(ui_song)));
    assert_eq!(e.song.macros.energy, 0.9);
    assert!(e.breakdown.is_none());
}

#[test]
fn auto_fx_mutes_what_the_energy_takes_out_and_moves_the_mix() {
    let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(4));
    song.auto = true;
    song.bpm = 180.0;
    let home = knobs(&song);
    let mut e = Engine::new(11_025.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 27 + 100];
    e.render(&mut buf); // into the breakdown: the energy is low
                        // What the low energy takes out is muted – the mute button shows it
    for (i, t) in e.song.tracks.iter().enumerate() {
        if LAYERS[i] > e.layer_energy && !t.lock {
            assert!(t.mute, "{:?} muted at {}", Voice::ALL[i], e.layer_energy);
        }
    }
    let out = e.energy_out;
    assert!(out.iter().any(|&o| o), "the energy muted something");
    // The mix moved a little, never far from where it was put
    let k = knobs(&e.song);
    assert!(k.iter().zip(&home).any(|(a, b)| (a - b).abs() > 0.01), "the knobs moved");
    for i in 0..KNOBS {
        assert!((k[i] - home[i]).abs() <= KNOB_RANGE[i] + 1e-4, "knob {i}: {} vs {}", k[i], home[i]);
    }
    // Auto mode off and the energy up again by hand: what it muted comes back, the knobs
    // glide home
    e.send_song(None);
    let mut s = e.song;
    s.auto = false;
    s.macros.energy = 0.9;
    e.handle(Cmd::Song(Box::new(s)));
    let mut buf = vec![0.0; 2 * bar * 4];
    e.render(&mut buf);
    for (i, &o) in out.iter().enumerate() {
        assert!(!o || !e.song.tracks[i].mute, "{:?} back", Voice::ALL[i]);
    }
    let k = knobs(&e.song);
    for i in 0..KNOBS {
        assert!((k[i] - home[i]).abs() < 1e-4, "knob {i} home: {} vs {}", k[i], home[i]);
    }
}

#[test]
fn a_knob_you_turn_stays_where_you_put_it() {
    let mut song = gen::generate(Vibe::Hypnotic, &Default::default(), &mut Rng::new(4));
    song.auto = true;
    let mut e = Engine::new(11_025.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar * 12];
    e.render(&mut buf);
    e.send_song(None);
    let mut s = e.song;
    s.macros.space = 0.9;
    e.handle(Cmd::Song(Box::new(s)));
    assert_eq!(e.song.macros.space, 0.9);
    assert_eq!(e.knob_home[TRACKS + 1], 0.9);
}

#[test]
fn the_auto_mode_grows_an_empty_sky_one_track_per_bar() {
    let mut song = Song::empty();
    song.auto = true;
    let target = gen::grow_target(&gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(2)), &mut Rng::new(3));
    let mut e = Engine::new(11_025.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Grow(Box::new(target)));
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    // Just short of the second bar
    let mut buf = vec![0.0; 2 * (bar - 64)];
    let filled = |e: &Engine| e.song.tracks.iter().filter(|t| t.hits() > 0).count();
    e.render(&mut buf);
    assert_eq!(filled(&e), 1, "the kick first");
    assert!(e.song.track(Voice::Kick).hits() > 0);
    let mut buf = vec![0.0; 2 * bar];
    e.render(&mut buf);
    assert_eq!(filled(&e), 2);
    for _ in 0..8 {
        e.render(&mut buf);
    }
    assert_eq!(filled(&e), target.tracks.iter().filter(|t| t.hits() > 0).count());
    assert!(e.grow.is_none(), "grown – the auto mode goes on as usual");
}

#[test]
fn an_empty_pattern_is_silent_unless_the_metronome_ticks() {
    let mut song = Song::empty();
    song.macros.energy = 1.0;
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * 22_050];
    e.render(&mut buf);
    assert!(buf.iter().all(|x| x.abs() < 1e-3), "no ghost hats on an empty hat");
    e.handle(Cmd::Metronome(true));
    e.render(&mut buf);
    assert!(buf.iter().any(|x| x.abs() > 0.05), "the metronome clicks");
}

#[test]
fn a_set_recording_waits_for_play_and_pauses_with_it() {
    let (tx, rx) = std::sync::mpsc::channel();
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(5));
    let mut e = Engine::new(11_025.0, song, Shared::new(), Some(tx), 1);
    let mut buf = vec![0.0; 2 * 11_025];
    e.handle(Cmd::Capture(true));
    e.render(&mut buf);
    assert!(e.rec.is_empty(), "armed, nothing recorded while stopped");
    e.handle(Cmd::Play);
    e.render(&mut buf);
    let after_play = e.rec.len();
    assert!(after_play > 0, "it records once playing");
    e.handle(Cmd::Stop);
    e.handle(Cmd::Capture(true));
    e.render(&mut buf);
    e.render(&mut buf);
    assert!(
        e.rec.len() <= after_play + buf.len(),
        "paused: no more than a moment after the stop"
    );
    drop(rx);
}

#[test]
fn a_resonant_filter_stays_clean() {
    let song = gen::generate(Vibe::Acid, &Default::default(), &mut Rng::new(3));
    for x in [-0.7, 0.5] {
        let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
        e.handle(Cmd::Play);
        e.handle(Cmd::Filter(x));
        e.handle(Cmd::FilterRes(1.0));
        let mut buf = vec![0.0; 2 * 22_050 * 3];
        e.render(&mut buf);
        assert!(e.res > 0.99);
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(buf.iter().all(|v| v.is_finite()) && peak <= 1.0, "filter {x}: peak {peak}");
    }
}

#[test]
fn master_section_and_shaping_stay_clean() {
    let mut song = gen::generate(Vibe::Hard, &Default::default(), &mut Rng::new(9));
    song.macros.pump = 1.0;
    song.eq = crate::song::MasterEq {
        low: 1.0,
        mid: 1.0,
        high: 1.0,
        drive: 1.0,
    };
    song.prog = 3;
    for t in song.tracks.iter_mut() {
        t.tune = 12;
        t.decay = 1.0;
    }
    let buf = render_offline(&song, 4, 22_050, 1);
    let (peak, rms) = stats(&buf);
    assert!(buf.iter().all(|x| x.is_finite()) && peak <= 1.0 && rms > 0.05, "{peak} {rms}");
    for t in song.tracks.iter_mut() {
        t.tune = -12;
        t.decay = 0.0;
    }
    song.eq = crate::song::MasterEq {
        low: 0.0,
        mid: 0.0,
        high: 0.0,
        drive: 0.0,
    };
    let buf = render_offline(&song, 4, 22_050, 1);
    let (peak, _) = stats(&buf);
    assert!(buf.iter().all(|x| x.is_finite()) && peak <= 1.0);
}

#[test]
fn spectrum_follows_the_music() {
    let shared = Shared::new();
    let mut e = Engine::new(22_050.0, Song::default(), shared.clone(), None, 1);
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * 2048];
    e.render(&mut buf);
    let bands = shared.bands();
    assert!(bands.iter().all(|b| b.is_finite()));
    assert!(bands[0] > 0.01, "the kick shows in the lowest band: {bands:?}");
}

#[test]
fn every_sound_plays_cleanly() {
    let base = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(6));
    for v in Voice::ALL {
        for s in 0..v.sounds().len() {
            let mut song = base;
            song.track_mut(v).sound = s as u8;
            song.track_mut(v).mute = false;
            let buf = render_offline(&song, 2, 22_050, 1);
            assert!(buf.iter().all(|x| x.is_finite()));
            let (peak, _) = stats(&buf);
            assert!(peak <= 1.0, "{v:?} {s}: {peak}");
        }
    }
}

#[test]
fn wash_drowns_without_booming_and_rings_out() {
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    let db = |b: &[f32]| 20.0 * stats(b).1.log10();
    e.render(&mut buf);
    e.render(&mut buf);
    let dry = db(&buf);
    e.handle(Cmd::Fx(Fx::Wash, true));
    e.render(&mut buf);
    e.render(&mut buf);
    let wet = db(&buf);
    assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!(wet < dry + 2.5, "the wash doesn't pile up: {dry:.1} → {wet:.1} dB");
    // Let go and stop: the room keeps ringing for a while
    e.handle(Cmd::Fx(Fx::Wash, false));
    e.handle(Cmd::Stop);
    e.render(&mut buf);
    e.render(&mut buf);
    assert!(db(&buf) > -45.0, "the tail rings out: {:.1} dB", db(&buf));
}

#[test]
fn tape_stop_spins_back_up_onto_the_live_beat() {
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
    let sr = 22_050.0;
    let mut live = Engine::new(sr, song, Shared::new(), None, 1);
    let mut e = Engine::new(sr, song, Shared::new(), None, 1);
    live.handle(Cmd::Play);
    e.handle(Cmd::Play);
    let mut a = vec![0.0; 2 * 1024];
    let mut b = a.clone();
    let blocks = |secs: f32| (secs * sr / 1024.0) as usize;
    for _ in 0..blocks(0.5) {
        live.render(&mut a);
        e.render(&mut b);
    }
    e.handle(Cmd::Fx(Fx::Tape, true));
    for _ in 0..blocks(1.5) {
        live.render(&mut a);
        e.render(&mut b);
    }
    assert!(stats(&b).1 < 1e-4, "it stands still");
    e.handle(Cmd::Fx(Fx::Tape, false));
    let mut out = Vec::new();
    for _ in 0..blocks(0.6) {
        live.render(&mut a);
        e.render(&mut b);
        out.extend_from_slice(&b);
    }
    assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!(stats(&out[..out.len() / 2]).1 > 0.01, "it spins back up");
    assert_eq!(a, b, "and is back on the live beat");
}

#[test]
fn crush_keeps_the_level() {
    let song = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(3));
    let mut e = Engine::new(22_050.0, song, Shared::new(), None, 1);
    e.handle(Cmd::Play);
    let bar = (e.step_len() * STEPS as f64) as usize;
    let mut buf = vec![0.0; 2 * bar];
    let db = |b: &[f32]| 20.0 * stats(b).1.log10();
    e.render(&mut buf);
    e.render(&mut buf);
    let dry = db(&buf);
    e.handle(Cmd::Fx(Fx::Crush, true));
    e.render(&mut buf);
    e.render(&mut buf);
    let wet = db(&buf);
    assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!((wet - dry).abs() < 2.0, "{dry:.1} → {wet:.1} dB");
}

#[test]
fn stutter_repeats_two_steps() {
    let mut song = Song::empty();
    song.bpm = 120.0;
    song.track_mut(Voice::Rim).steps[5].on = true;
    let shared = Shared::new();
    let mut e = Engine::new(48_000.0, song, shared.clone(), None, 1);
    e.handle(Cmd::Play);
    let mut buf = vec![0.0; 2 * 6000 * 4 + 20];
    e.render(&mut buf); // steps 0…4 fired
    e.handle(Cmd::Fx(Fx::Stutter, true)); // loops steps 5 and 6
    let before = shared.hits[Voice::Rim.index()].load(Ordering::Relaxed);
    let mut buf = vec![0.0; 2 * 6000 * 8];
    e.render(&mut buf);
    let rims = shared.hits[Voice::Rim.index()].load(Ordering::Relaxed) - before;
    assert_eq!(rims, 4, "step 5 repeats every other step");
}

#[test]
fn lunas_sets_keep_air_and_play_riffs() {
    // 96 bars of the auto mode in a calm, a middle and a hard vibe: never the wall of
    // everything at once, and a 303 line stays a riff (a handful of notes within an octave)
    for (vibe, seed) in [(Vibe::Minimal, 1), (Vibe::Acid, 2), (Vibe::Peak, 3), (Vibe::Hard, 4)] {
        let mut song = gen::generate(vibe, &Default::default(), &mut Rng::new(seed));
        song.auto = true;
        let mut e = Engine::new(8_000.0, song, Shared::new(), None, seed);
        e.handle(Cmd::Play);
        let mut buf = vec![0.0; 2 * (e.step_len() * STEPS as f64) as usize];
        let (mut crowd, mut tracks) = (0.0, 0);
        for bar in 0..96 {
            e.render(&mut buf);
            let s = e.song;
            let on = |v: Voice| !s.track(v).mute && s.track(v).hits() > 0;
            let extras = gen::EXTRAS.into_iter().filter(|&v| on(v)).count();
            // (one over for a moment: the energy brings its own tracks back)
            assert!(extras <= gen::extras_cap(vibe) + 1, "{vibe:?} bar {bar}: {extras} extras");
            tracks += Voice::ALL.into_iter().filter(|&v| on(v)).count();
            crowd += crate::ear::hear(&s).crowd;
            let synth = s.track(Voice::Acid);
            if synth.sound < 2 && synth.hits() > 0 {
                let mut notes: Vec<i8> = synth.steps.iter().filter(|st| st.on).map(|st| st.note).collect();
                notes.sort();
                notes.dedup();
                assert!(
                    notes.len() <= 5 && notes[notes.len() - 1] - notes[0] <= 12,
                    "{vibe:?} bar {bar}: {notes:?}"
                );
            }
        }
        assert!(
            crowd / 96.0 < 0.1,
            "{vibe:?}: {:.0} % of the steps are a wall",
            crowd / 96.0 * 100.0
        );
        assert!(
            tracks as f32 / 96.0 < 6.8,
            "{vibe:?}: {:.1} tracks on average",
            tracks as f32 / 96.0
        );
    }
}
