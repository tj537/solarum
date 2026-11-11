//! Reports for balancing the mix by numbers (ignored tests that print) and the solo song
//! they and the loudness tests share.

use super::*;
use crate::song::Vibe;

/// Prints what every vibe's groove measures like (`listen::Heard`), three seeds each:
/// `cargo test --release print_mix -- --ignored --nocapture`
#[test]
#[ignore]
fn print_mix() {
    for (i, vibe) in Vibe::ALL.into_iter().enumerate() {
        for seed in 0..3u64 {
            let song = gen::generate(vibe, &Default::default(), &mut Rng::new(i as u64 * 10 + seed));
            let buf = render_offline(&song, 8, 44_100, 1);
            let body = &buf[..buf.len() - 44_100 * 4];
            println!("{:10} {seed}  {}", vibe.name(), crate::listen::hear(body, 44_100.0).line());
        }
    }
}

/// `base` with only track `i` playing, on `sound`, dry and at full energy – one sound alone
pub fn solo(base: &Song, i: usize, sound: u8) -> Song {
    let mut s = *base;
    for (j, t) in s.tracks.iter_mut().enumerate() {
        t.mute = j != i;
    }
    s.tracks[i].sound = sound;
    if !s.tracks[i].steps.iter().any(|st| st.on) {
        // the groove left this track empty: off-beats, so every sound gets measured
        for st in s.tracks[i].steps.iter_mut().skip(2).step_by(4) {
            st.on = true;
        }
    }
    s.macros.space = 0.0;
    s.macros.dark = 0.0;
    s.macros.energy = 1.0; // the ride only plays from 70
    s
}

/// Prints the loudness of each track – and each of its sounds – on its own, for balancing the mix:
/// `cargo test --release print_levels -- --ignored --nocapture`
#[test]
#[ignore]
fn print_levels() {
    let base = gen::generate(Vibe::Peak, &Default::default(), &mut Rng::new(11));
    for (i, v) in Voice::ALL.into_iter().enumerate() {
        for sound in 0..v.sounds().len() {
            let s = solo(&base, i, sound as u8);
            let buf = render_offline(&s, 4, 44_100, 1);
            let body = &buf[..buf.len() - 44_100 * 4]; // without the tail
            let h = crate::listen::hear(body, 44_100.0);
            let bright = crate::listen::centroid(&crate::listen::spectrum(&crate::listen::mono(body), 4096), 44_100.0);
            println!(
                "{:10} {:13} {:6.1} LUFS  rms {:6.1} dB  peak {:6.1} dB  centre {:6.0} Hz",
                v.name(),
                v.sounds()[sound],
                h.lufs,
                h.rms_db,
                crate::listen::db(h.peak),
                bright
            );
        }
    }
}

/// What Luna's sets are like, per vibe: how many tracks play, how busy the bar is, how the
/// bass and synth lines move, which sounds come up – over 192 bars of the auto mode each:
/// `cargo test --release print_auto -- --ignored --nocapture`
#[test]
#[ignore]
fn print_auto() {
    for (i, vibe) in Vibe::ALL.into_iter().enumerate() {
        let (mut tracks, mut density, mut crowd, mut bars) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let (mut dmax, mut full) = (0.0f32, 0.0f32);
        let mut synth_notes = 0.0f32;
        let mut distinct = 0.0f32;
        let mut leap = 0.0f32;
        let mut span = 0.0f32;
        let mut sounds = [[0u32; 8]; TRACKS];
        for seed in 0..3u64 {
            let mut song = gen::generate(vibe, &Default::default(), &mut Rng::new(i as u64 * 10 + seed));
            song.auto = true;
            let mut e = Engine::new(8_000.0, song, Shared::new(), None, seed + 1);
            e.handle(Cmd::Play);
            let bar = (e.step_len() * STEPS as f64) as usize;
            let mut buf = vec![0.0f32; bar * 2];
            for _ in 0..192 {
                e.render(&mut buf);
                let s = e.song;
                let playing = s.tracks.iter().filter(|t| !t.mute && t.hits() > 0).count();
                let h = crate::ear::hear(&s);
                tracks += playing as f32;
                density += h.density;
                dmax = dmax.max(h.density);
                crowd += h.crowd;
                if playing >= 8 {
                    full += 1.0;
                }
                bars += 1.0;
                for (k, t) in s.tracks.iter().enumerate() {
                    if !t.mute && t.hits() > 0 {
                        sounds[k][t.sound as usize % 8] += 1;
                    }
                }
                let a = s.track(Voice::Acid);
                if !a.mute && a.hits() > 0 {
                    let notes: Vec<i8> = a.steps.iter().filter(|st| st.on).map(|st| st.note).collect();
                    let mut set = notes.clone();
                    set.sort();
                    set.dedup();
                    synth_notes += 1.0;
                    distinct += set.len() as f32;
                    span += (set[set.len() - 1] - set[0]) as f32;
                    leap += notes.windows(2).map(|w| (w[1] - w[0]).abs()).max().unwrap_or(0) as f32;
                }
            }
        }
        let n = synth_notes.max(1.0);
        println!(
            "{:10} tracks {:.1} (all 8: {:2.0}%)  hits/bar {:4.1} (max {:2.0})  crowd {:4.1}%  synth: {:.1} pitches, span {:4.1}, leap {:4.1}",
            vibe.name(),
            tracks / bars,
            100.0 * full / bars,
            density / bars,
            dmax,
            100.0 * crowd / bars,
            distinct / n,
            span / n,
            leap / n
        );
        for (k, v) in Voice::ALL.into_iter().enumerate() {
            let total: u32 = sounds[k].iter().sum();
            let list: Vec<String> = v
                .sounds()
                .iter()
                .enumerate()
                .filter(|(j, _)| sounds[k][*j] > 0)
                .map(|(j, name)| format!("{name} {}%", 100 * sounds[k][j] / total.max(1)))
                .collect();
            println!("    {:9} {}", v.name(), list.join(", "));
        }
    }
}
