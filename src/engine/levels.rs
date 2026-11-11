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
