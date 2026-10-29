//! The groove generator behind *Shuffle* and the auto mode.
//!
//! Techno has strong conventions – four-to-the-floor kick, claps on 2 and 4,
//! off-beat hats, a rolling bass that dodges the kick. The generator starts from
//! those, then varies density with the Energy macro and spices things up with
//! Euclidean rhythms, so every roll sounds like techno but never the same.

use crate::form::Part;
use crate::rng::Rng;
use crate::song::{Macros, Song, Step, Track, Vibe, Voice, STEPS, TRACKS};

/// `k` hits spread as evenly as possible over `n` steps, rotated by `rot`
pub fn euclid(k: usize, n: usize, rot: usize) -> [bool; STEPS] {
    let mut out = [false; STEPS];
    if n == 0 {
        return out;
    }
    let k = k.min(n);
    for i in 0..n {
        // Bresenham-style distribution
        if (i * k) % n < k {
            out[(i + rot) % n] = true;
        }
    }
    out
}

fn from_mask(mask: [bool; STEPS]) -> Track {
    let mut t = Track::default();
    for (s, on) in t.steps.iter_mut().zip(mask) {
        s.on = on;
    }
    t
}

fn from_positions(pos: &[usize]) -> Track {
    let mut t = Track::default();
    for &p in pos {
        t.steps[p % STEPS] = Step::hit();
    }
    t
}

/// Chord changes per half bar, as scale degrees: i–i, i–VI, i–iv, i–VII, i–v
const HALF_BAR_CHORDS: [[i32; 2]; 5] = [[0, 0], [0, 5], [0, 3], [0, -1], [0, 4]];

/// The song's chord progression – derived from key and scale, so bass and synth
/// agree even when only one of them is rolled again
fn progression(song: &Song) -> [i32; 2] {
    // A song-wide progression moves bar by bar already – then the bar itself stays on one chord
    if song.prog != 0 {
        return [0, 0];
    }
    let k = song.root as usize + song.scale as usize * 5;
    HALF_BAR_CHORDS[k % HALF_BAR_CHORDS.len()]
}

/// The moods (indices into `MOODS`) that suit a vibe – a groove always has one of them
pub fn moods_for(vibe: Vibe) -> &'static [usize] {
    // Dark, Hypnotic, Deep, Sad, Euphoric, Dreamy, Epic, Sunny, Mysterious
    match vibe {
        Vibe::Acid | Vibe::Industrial | Vibe::Hard => &[0, 0, 1, 1, 8],
        Vibe::Minimal | Vibe::Hypnotic | Vibe::Custom => &[1, 1, 2, 0, 8],
        Vibe::Dub => &[2, 3, 5, 1],
        Vibe::Melodic | Vibe::Rave => &[3, 4, 5, 6, 4],
        Vibe::Peak => &[1, 3, 4, 0],
        Vibe::Electro => &[1, 2, 6, 0],
    }
}

/// The groove the auto mode grows into from what's there: every track that has something
/// (or is locked) stays as it is, every empty one gets a part that fits the vibe, the key and
/// the mood – the engine brings them in one per bar (`Cmd::Grow`)
pub fn grow_target(song: &Song, rng: &mut Rng) -> Song {
    let mut target = *song;
    let vibe = song.vibe.rules();
    for v in Voice::ALL {
        let t = song.track(v);
        if t.lock || t.hits() > 0 {
            continue;
        }
        let sound = sound_for(v, vibe, rng);
        target.track_mut(v).sound = sound;
        let fresh = track_for(v, vibe, song.macros.energy.max(0.6), &target, rng);
        let tt = target.track_mut(v);
        tt.steps = fresh.steps;
        tt.len = fresh.len;
        tt.mute = false;
    }
    target
}

/// A whole new groove
pub fn generate(vibe: Vibe, m: &Macros, rng: &mut Rng) -> Song {
    generate_keeping(vibe, m, rng, &[None; TRACKS])
}

/// A whole new groove that keeps the given tracks (the locked ones) as they are
pub fn generate_keeping(vibe: Vibe, m: &Macros, rng: &mut Rng, keep: &[Option<Track>; TRACKS]) -> Song {
    if vibe == Vibe::Custom {
        let mut song = generate_keeping(vibe.rules(), m, rng, keep);
        song.vibe = vibe;
        return song;
    }
    let mut song = Song::empty();
    song.vibe = vibe;
    song.macros = *m;
    let (lo, hi) = vibe.bpm();
    song.bpm = (lo + rng.f32() * (hi - lo)).round();
    song.root = *rng.pick(&[36u8, 37, 38, 39, 40, 41, 42, 43]); // C2 … G2
    song.set_mood(*rng.pick(moods_for(vibe)));
    song.swing = match vibe {
        Vibe::Minimal => 0.15 + rng.f32() * 0.2,
        Vibe::Dub | Vibe::Electro => 0.08 + rng.f32() * 0.15,
        Vibe::Hypnotic | Vibe::Melodic => rng.f32() * 0.12,
        _ => 0.0,
    };
    // Sounds first: the synth line is written for the sound that plays it
    for v in Voice::ALL {
        song.track_mut(v).sound = sound_for(v, vibe, rng);
    }
    // Each groove its own drive: busier or sparser than the energy alone would make it
    let drive = (m.energy + rng.f32() * 0.4 - 0.2).clamp(0.0, 1.0);
    for (i, v) in Voice::ALL.into_iter().enumerate() {
        if let Some(t) = keep[i] {
            *song.track_mut(v) = t;
            continue;
        }
        let vol = song.track(v).volume;
        *song.track_mut(v) = track_for(v, vibe, drive, &song, rng);
        song.track_mut(v).volume = vol;
    }
    song
}

/// Shuffle: three new grooves of the vibe, and the one that differs most from `now`, so
/// two shuffles in a row never sound alike (locked tracks stay as they are)
pub fn shuffle(vibe: Vibe, m: &Macros, rng: &mut Rng, keep: &[Option<Track>; TRACKS], now: &Song) -> Song {
    let mut best = generate_keeping(vibe, m, rng, keep);
    let mut far = distance(&best, now);
    for _ in 0..2 {
        let s = generate_keeping(vibe, m, rng, keep);
        let d = distance(&s, now);
        if d > far {
            (best, far) = (s, d);
        }
    }
    best
}

/// How different two grooves sound: key, mood, tempo, and per track the sound, whether it
/// plays and how many of its steps moved
pub fn distance(a: &Song, b: &Song) -> f32 {
    let flag = |x: bool| if x { 1.0 } else { 0.0 };
    let mut d = 2.0 * flag(a.root != b.root) + 2.0 * flag(a.mood() != b.mood()) + (a.bpm - b.bpm).abs() / 4.0;
    for (x, y) in a.tracks.iter().zip(&b.tracks) {
        let moved = x.steps.iter().zip(&y.steps).filter(|(p, q)| p.on != q.on).count();
        d += flag(x.sound != y.sound) + flag(x.mute != y.mute) + moved as f32 / 4.0;
    }
    d
}

/// A sound that suits the vibe (index into `Voice::sounds()`)
pub fn sound_for(v: Voice, vibe: Vibe, rng: &mut Rng) -> u8 {
    *rng.pick(sounds_for(v, vibe))
}

/// The sounds that suit the vibe on a track (some twice: more likely)
fn sounds_for(v: Voice, vibe: Vibe) -> &'static [u8] {
    use Vibe::*;
    let vibe = vibe.rules();
    match v {
        // 909, 808, Hard, Deep, Punch, Rumble, Distort
        Voice::Kick => match vibe {
            Dub => &[3, 1, 3, 5],
            Minimal => &[4, 0, 3],
            Melodic => &[0, 3, 4],
            Hypnotic | Custom => &[0, 3, 0],
            Electro => &[1],
            Peak | Acid => &[0, 4, 0],
            Rave => &[2, 0],
            Industrial => &[2, 0, 2, 6],
            Hard => &[2, 6],
        },
        // Clap, Snare, Big clap, Snap, Noise, Lo-fi
        Voice::Clap => match vibe {
            Dub | Minimal => &[3, 0, 5],
            Melodic => &[2, 0],
            Hypnotic | Custom => &[0, 3],
            Electro => &[1],
            Peak => &[0, 2],
            Acid => &[0, 1],
            Rave => &[1, 2],
            Industrial => &[1, 0, 4],
            Hard => &[0, 1, 2, 4],
        },
        // Rim, Cowbell, Clave, Tom, Blip, Conga, Zap
        Voice::Rim => match vibe {
            Dub => &[0, 4],
            Minimal => &[0, 2, 4],
            Melodic => &[2, 0, 3, 5],
            Hypnotic | Custom => &[0, 3],
            Electro => &[1, 2, 6],
            Peak => &[0, 3],
            Acid => &[0, 1],
            Rave => &[1, 0],
            Industrial | Hard => &[3, 0, 6],
        },
        // 909, 808, Shaker, Tick, Metal, Crisp
        Voice::Hat => match vibe {
            Dub => &[2, 1],
            Minimal => &[3, 0, 2, 5],
            Melodic => &[2, 0],
            Electro => &[1],
            Hard | Industrial => &[0, 4],
            _ => &[0, 1, 0],
        },
        // 909, 808, Sizzle, Long, Metal
        Voice::OpenHat => match vibe {
            Electro => &[1],
            Dub => &[1, 2],
            Melodic | Minimal => &[0, 2, 3],
            Industrial | Hard => &[0, 4],
            _ => &[0, 0, 1],
        },
        // Ride, Bell, Dark, Crash, Shimmer
        Voice::Ride => match vibe {
            Minimal => &[1, 0, 4],
            Dub => &[2],
            Electro => &[1],
            Melodic => &[0, 4],
            _ => &[0, 2, 0],
        },
        // Rolling, Sub, Reese, Pulse, Donk, FM, Growl
        Voice::Bass => match vibe {
            Dub => &[1],
            Minimal => &[1, 3, 0, 5],
            Melodic => &[1, 2],
            Hypnotic | Custom => &[0, 2, 0],
            Electro => &[3, 1, 5],
            Peak => &[0, 3],
            Acid => &[0, 1],
            Rave => &[2, 4],
            Industrial => &[0, 2, 6],
            Hard => &[4, 0, 6],
        },
        // 303 saw, 303 square, Stab, Pluck, Hoover, Pad, Organ, Supersaw
        Voice::Acid => match vibe {
            Dub => &[2, 5],
            Minimal => &[3, 1, 2],
            Melodic => &[3, 3, 2, 5, 7],
            Hypnotic | Custom => &[0, 3, 2],
            Electro => &[1, 3],
            Peak => &[2, 0, 7],
            Acid => &[0, 1, 0],
            Rave => &[4, 2, 6],
            Industrial => &[0, 4],
            Hard => &[4, 2],
        },
    }
}

/// A fresh pattern for one track that fits the vibe (keeps the track's sound)
pub fn track_for(v: Voice, vibe: Vibe, energy: f32, song: &Song, rng: &mut Rng) -> Track {
    let vibe = vibe.rules();
    let e = energy.clamp(0.0, 1.0);
    let old = *song.track(v);
    let sound = old.sound;
    let mut t = match v {
        Voice::Kick => kick(vibe, rng),
        Voice::Clap => clap(vibe, e, rng),
        Voice::Rim => rim(vibe, e, song, rng),
        Voice::Hat => hat(vibe, e, rng),
        Voice::OpenHat => open_hat(vibe, e, rng),
        Voice::Ride => ride(vibe, e, rng),
        Voice::Bass => bass(vibe, e, song, rng),
        Voice::Acid => synth_line(vibe, e, song, rng),
    };
    t.volume = old.volume;
    t.sound = sound;
    t.tune = old.tune;
    t.decay = old.decay;
    t.lock = old.lock;
    t.mute = match v {
        // The synth line plays in the vibes made for it – elsewhere it often waits muted for the auto mode
        Voice::Acid => match vibe {
            Vibe::Acid | Vibe::Dub | Vibe::Melodic => false,
            Vibe::Rave => rng.chance(0.25),
            _ => !rng.chance(0.3),
        },
        Voice::Ride => matches!(vibe, Vibe::Minimal | Vibe::Dub) && rng.chance(0.6),
        Voice::Rim => matches!(vibe, Vibe::Hard) && rng.chance(0.4),
        _ => false,
    };
    t
}

fn kick(vibe: Vibe, rng: &mut Rng) -> Track {
    if !vibe.four_on_the_floor() {
        // Electro: broken kick patterns, always on the one
        let mut t = from_positions(rng.pick(&[
            &[0usize, 6, 10][..],
            &[0, 3, 8, 11],
            &[0, 7, 10],
            &[0, 6, 9, 12],
            &[0, 10],
            &[0, 3, 6, 10, 13],
        ]));
        t.steps[0].accent = true;
        if rng.chance(0.4) {
            t.steps[*rng.pick(&[13usize, 14, 15])] = Step::hit();
        }
        return t;
    }
    let mut t = from_positions(&[0, 4, 8, 12]);
    for i in [0, 4, 8, 12] {
        t.steps[i].accent = true;
    }
    // Ghost kicks: zero to three soft extra hits, so every roll of the dice sounds different
    let ghosts: &[usize] = match vibe {
        Vibe::Industrial | Vibe::Hard => &[2, 3, 6, 10, 11, 14, 15],
        Vibe::Minimal | Vibe::Hypnotic | Vibe::Dub => &[3, 7, 10, 11, 15],
        _ => &[7, 10, 14, 15],
    };
    let n = match vibe {
        Vibe::Industrial | Vibe::Hard => rng.range(1, 3),
        Vibe::Rave | Vibe::Peak => rng.range(0, 1),
        _ => rng.range(0, 2),
    };
    // They always play (soft, unaccented) – a kick that only sometimes comes sounds like a stumble
    for _ in 0..n {
        t.steps[*rng.pick(ghosts)] = Step::hit();
    }
    // Sometimes the last beat of the bar rolls
    if matches!(vibe, Vibe::Hard | Vibe::Rave) && rng.chance(0.3) {
        t.steps[14] = Step::hit();
        t.steps[15] = Step::hit();
    }
    t
}

fn clap(vibe: Vibe, e: f32, rng: &mut Rng) -> Track {
    let mut t = match vibe {
        Vibe::Minimal | Vibe::Dub => from_positions(if rng.chance(0.5) { &[12] } else { &[4, 12] }),
        Vibe::Hypnotic => {
            if rng.chance(0.35) {
                Track::default()
            } else {
                from_positions(&[4, 12])
            }
        }
        Vibe::Electro => from_positions(rng.pick(&[&[4usize, 12][..], &[4, 12, 15], &[4, 11, 12]])),
        _ => from_positions(&[4, 12]),
    };
    if e > 0.55 && rng.chance(e - 0.3) {
        t.steps[*rng.pick(&[7usize, 9, 14, 15])] = Step::hit();
    }
    if matches!(vibe, Vibe::Industrial | Vibe::Hard) && rng.chance(0.5) {
        t.steps[13] = Step::hit();
    }
    t
}

fn rim(vibe: Vibe, e: f32, song: &Song, rng: &mut Rng) -> Track {
    let k = match vibe {
        Vibe::Minimal => rng.range(3, 6),
        Vibe::Industrial | Vibe::Electro => rng.range(4, 7),
        Vibe::Dub => rng.range(1, 3),
        _ => rng.range(2, 5),
    } as usize
        + (e * 2.0) as usize;
    let mut t = from_mask(euclid(k, STEPS, rng.below(16) as usize));
    // Rims shouldn't sit on the kick or the clap
    let clap = song.track(Voice::Clap);
    for i in 0..STEPS {
        if i % 4 == 0 || (clap.len == 16 && clap.steps[i].on) {
            t.steps[i].on = false;
        }
    }
    if t.hits() == 0 {
        if let Some(i) = pick_if(rng, &[3usize, 6, 10, 13, 2, 11], |i| !clap.steps[i].on) {
            t.steps[i] = Step::hit();
        }
    }
    for s in t.steps.iter_mut().filter(|s| s.on) {
        s.accent = rng.chance(0.3);
    }
    t
}

/// Every step of the bar (no `Vec`: the auto mode calls this on the audio thread)
const ALL_STEPS: [usize; STEPS] = {
    let mut a = [0; STEPS];
    let mut i = 0;
    while i < STEPS {
        a[i] = i;
        i += 1;
    }
    a
};

fn hat(vibe: Vibe, e: f32, rng: &mut Rng) -> Track {
    let density = e + rng.f32() * 0.3 - 0.15;
    let sixteenths = density > 0.55 || matches!(vibe, Vibe::Industrial | Vibe::Peak | Vibe::Hard | Vibe::Rave);
    let mut t = if sixteenths {
        // 16ths, accents on the off-beats, some gaps when the energy is low
        let mut t = from_positions(&ALL_STEPS);
        let gaps = rng.chance(0.3);
        for (i, s) in t.steps.iter_mut().enumerate() {
            s.accent = i % 4 == 2;
            if i % 2 == 1 && rng.chance(0.3 * (1.0 - e)) {
                s.on = false;
            }
            if gaps && i % 4 == 0 {
                s.on = false;
            }
        }
        t
    } else {
        match rng.below(3) {
            0 => from_positions(&[2, 6, 10, 14]),
            1 => from_positions(&[2, 3, 6, 10, 11, 14]),
            _ => {
                let mut t = from_positions(&[1, 2, 3, 5, 6, 7, 9, 10, 11, 13, 14, 15]);
                for i in [2, 6, 10, 14] {
                    t.steps[i].accent = true;
                }
                t
            }
        }
    };
    if matches!(vibe, Vibe::Minimal) && rng.chance(0.5) {
        t = from_mask(euclid(rng.range(5, 9) as usize, STEPS, 2));
    }
    if matches!(vibe, Vibe::Dub | Vibe::Melodic) && rng.chance(0.5) {
        // Shaker groove: soft 16ths, louder off-beats
        t = from_positions(&ALL_STEPS);
        for (i, s) in t.steps.iter_mut().enumerate() {
            s.accent = i % 4 == 2;
        }
    }
    t
}

fn open_hat(vibe: Vibe, e: f32, rng: &mut Rng) -> Track {
    let mut t = from_positions(&[2, 6, 10, 14]);
    if matches!(vibe, Vibe::Minimal | Vibe::Dub) || (e < 0.35 && rng.chance(0.5)) {
        t = from_positions(&[*rng.pick(&[6usize, 14])]);
    } else if matches!(vibe, Vibe::Electro) {
        t = from_positions(rng.pick(&[&[2usize, 10][..], &[6, 14], &[14]]));
    } else if rng.chance(0.2) {
        t = from_mask(euclid(3, 8, 2));
        t.len = 8;
        t.expand();
    }
    if e > 0.75 && rng.chance(0.3) {
        t.steps[15] = Step::hit();
    }
    t
}

fn ride(vibe: Vibe, e: f32, rng: &mut Rng) -> Track {
    match vibe {
        Vibe::Peak | Vibe::Industrial | Vibe::Hard | Vibe::Rave => {
            let mut t = if e > 0.5 {
                from_positions(&[0, 2, 4, 6, 8, 10, 12, 14])
            } else {
                from_positions(&[2, 6, 10, 14])
            };
            for i in [2, 6, 10, 14] {
                t.steps[i].accent = true;
            }
            t
        }
        _ => from_mask(euclid(rng.range(3, 5) as usize, STEPS, rng.below(4) as usize * 2)),
    }
}

fn bass(vibe: Vibe, e: f32, song: &Song, rng: &mut Rng) -> Track {
    let scale = song.scale;
    // Rolling bass lives between the kicks
    let template: &[usize] = match vibe {
        Vibe::Hypnotic | Vibe::Custom | Vibe::Industrial => {
            if rng.chance(0.6) {
                &[1, 2, 3, 5, 6, 7, 9, 10, 11, 13, 14, 15]
            } else {
                &[2, 3, 6, 7, 10, 11, 14, 15]
            }
        }
        Vibe::Peak => rng.pick(&[
            &[2usize, 6, 10, 14][..],
            &[2, 3, 6, 7, 10, 11, 14, 15][..],
            &[1, 2, 5, 6, 9, 10, 13, 14],
        ]),
        Vibe::Minimal => rng.pick(&[&[3usize, 6, 11, 14][..], &[2, 7, 10, 15][..], &[3, 7, 10, 14]]),
        Vibe::Acid | Vibe::Hard => rng.pick(&[&[2usize, 6, 10, 14][..], &[2, 3, 6, 7, 10, 11, 14, 15]]),
        Vibe::Rave => rng.pick(&[&[2usize, 6, 10, 14][..], &[2, 5, 6, 10, 13, 14]]),
        Vibe::Dub => rng.pick(&[&[2usize, 10][..], &[3, 11], &[2, 7, 10]]),
        Vibe::Melodic => rng.pick(&[&[2usize, 6, 10, 14][..], &[2, 3, 6, 10, 11, 14]]),
        Vibe::Electro => rng.pick(&[&[2usize, 3, 7, 11, 14][..], &[1, 4, 7, 9, 12, 14], &[3, 4, 7, 11, 12, 15]]),
    };
    let mut t = from_positions(template);
    // The bass plays the root of the chord the synths play over it, so they always agree
    let prog = progression(song);
    // Folded back below the root when a chord takes it too high
    let fold = |n: i8| if n > 7 { n - 12 } else { n };
    for (i, s) in t.steps.iter_mut().enumerate() {
        if !s.on {
            continue;
        }
        let home = fold(scale.shift(0, prog[i / 8]));
        s.note = home;
        // Mostly the root; a few octave jumps and fifths for movement
        if rng.chance(0.12 + e * 0.1) {
            s.note = home + 12;
        } else if rng.chance(0.08) {
            s.note = fold(scale.shift(0, prog[i / 8] + 4));
        }
        s.accent = i % 4 == 2;
    }
    if matches!(vibe, Vibe::Minimal) && rng.chance(0.5) {
        // Answer phrase: the first half an octave up on the chord's sixth below
        for (i, s) in t.steps.iter_mut().enumerate().take(8).filter(|(_, s)| s.on) {
            s.note = fold(scale.shift(0, prog[i / 8] - 2)) + 12;
        }
    }
    // Never on top of a kick: they would fight for the low end
    let kick = song.track(Voice::Kick);
    for (s, k) in t.steps.iter_mut().zip(kick.steps) {
        if k.on && kick.len == 16 {
            s.on = false;
        }
    }
    t
}

/// The synth track – the pattern depends on its sound
/// How a synth sound's line is written: 0 = 303, 1 = stabs (stab, organ), 2 = arp (pluck, supersaw), 3 = hoover, 4 = pad
pub fn synth_family(sound: u8) -> u8 {
    match sound {
        2 | 6 => 1,
        3 | 7 => 2,
        4 => 3,
        5 => 4,
        _ => 0,
    }
}

fn synth_line(vibe: Vibe, e: f32, song: &Song, rng: &mut Rng) -> Track {
    match synth_family(song.track(Voice::Acid).sound) {
        1 => stabs(vibe, song, rng),
        2 => arp(e, song, rng),
        3 => hoover(song, rng),
        4 => pads(song, rng),
        _ => acid(vibe, e, song, rng),
    }
}

/// Pad chords: one per half bar or per bar, held until the next
fn pads(song: &Song, rng: &mut Rng) -> Track {
    let pos: &[usize] = rng.pick(&[&[0usize, 8][..], &[0], &[0, 6, 8, 14]]);
    let mut t = from_positions(pos);
    let prog = progression(song);
    for (i, s) in t.steps.iter_mut().enumerate().filter(|(_, s)| s.on) {
        s.note = song.scale.shift(0, prog[i / 8]);
    }
    t
}

/// Chord stabs on the off-beats, one chord per half bar
fn stabs(vibe: Vibe, song: &Song, rng: &mut Rng) -> Track {
    let pos: &[usize] = if matches!(vibe, Vibe::Dub) {
        rng.pick(&[&[3usize, 10][..], &[2, 10], &[3, 6, 11], &[2, 7, 10, 13]])
    } else {
        rng.pick(&[&[2usize, 6, 10, 14][..], &[3, 6, 11, 14], &[2, 5, 10, 13], &[0, 3, 6, 10, 12]])
    };
    let mut t = from_positions(pos);
    let prog = progression(song);
    for (i, s) in t.steps.iter_mut().enumerate().filter(|(_, s)| s.on) {
        s.note = song.scale.shift(0, prog[i / 8]) + 12;
        s.accent = rng.chance(0.3);
    }
    t
}

/// A plucked arpeggio walking up and down the chord
fn arp(e: f32, song: &Song, rng: &mut Rng) -> Track {
    let mut t = Track::default();
    let prog = progression(song);
    let shape: &[i32] = rng.pick(&[&[0, 2, 4, 7][..], &[0, 4, 2, 4], &[0, 2, 4, 2], &[0, 7, 4, 2], &[4, 2, 0, 2]]);
    let eighths = e < 0.45 || rng.chance(0.3);
    let lift = if rng.chance(0.4) { 12 } else { 0 };
    for (i, s) in t.steps.iter_mut().enumerate() {
        if eighths && i % 2 == 1 {
            continue;
        }
        s.on = !(i % 4 == 0 && rng.chance(0.25));
        let k = if eighths { i / 2 } else { i };
        s.note = song.scale.shift(0, prog[i / 8] + shape[k % shape.len()]) + 12 + lift;
        s.accent = i % 4 == 2 || rng.chance(0.15);
    }
    t.steps[0].on = true;
    t
}

/// A rave hoover riff: few long notes with slides
fn hoover(song: &Song, rng: &mut Rng) -> Track {
    let pos: &[usize] = rng.pick(&[&[0usize, 3, 6, 10][..], &[0, 6, 8, 14], &[2, 6, 10, 14], &[0, 3, 8, 11]]);
    let mut t = from_positions(pos);
    let degrees = [0, 0, 2, -1, 4, 3];
    let prog = progression(song);
    for (i, s) in t.steps.iter_mut().enumerate().filter(|(_, s)| s.on) {
        s.note = song.scale.shift(0, prog[i / 8] + *rng.pick(&degrees)) + 12;
        s.slide = rng.chance(0.3);
        s.accent = rng.chance(0.4);
    }
    t
}

/// A 303-style line: notes from the scale, accents, slides and octave jumps. On every
/// beat it lands on a note of the chord (root, third or fifth), in between it wanders.
pub fn acid(vibe: Vibe, e: f32, song: &Song, rng: &mut Rng) -> Track {
    let mut t = Track::default();
    let scale = song.scale;
    let prog = progression(song);
    let density = 0.45 + e * 0.3 + if matches!(vibe, Vibe::Acid) { 0.1 } else { 0.0 };
    let mut degree = 0i32;
    for (i, s) in t.steps.iter_mut().enumerate() {
        s.on = i == 0 || rng.chance(density);
        if !s.on {
            continue;
        }
        degree = if i % 4 == 0 {
            *rng.pick(&[0, 0, 2, 4])
        } else {
            (degree + rng.range(-2, 2)).clamp(-2, 6)
        };
        s.note = scale.shift(0, prog[i / 8] + degree) + if rng.chance(0.22) { 12 } else { 0 } + 12;
        s.accent = rng.chance(0.28);
        s.slide = i > 0 && rng.chance(0.22);
    }
    t
}

/// What the auto mode just did – shown in the UI
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    NewPattern(Voice),
    Muted(Voice),
    Unmuted(Voice),
    Shifted(Voice),
    NewSound(Voice, u8),
    NewNotes(Voice),
    Length(Voice, u8),
    /// A track that wandered goes back to how it was when the cycle began (the theme returns)
    Reprise(Voice),
    Breakdown,
    Build,
    Drop,
    /// After a drop: new bassline and synth, maybe new sounds or a new key
    NewChapter,
    /// Another pattern `0…3` came in (picked during the bar, it waited for the next one)
    Scene(usize),
    /// Song mode moved on to section `usize` of its form, playing pattern `Option<usize>`
    Part(Part, usize, Option<usize>),
    /// Song mode played its last bar
    SongEnd,
}

impl Change {
    /// The track a change went to
    pub fn voice(self) -> Option<Voice> {
        match self {
            Change::NewPattern(v)
            | Change::Muted(v)
            | Change::Unmuted(v)
            | Change::Shifted(v)
            | Change::NewSound(v, _)
            | Change::NewNotes(v)
            | Change::Length(v, _)
            | Change::Reprise(v) => Some(v),
            _ => None,
        }
    }

    /// What kind of change it was (`crate::ear::KINDS`), for the taste Luna learns
    pub fn kind(self) -> Option<usize> {
        Some(match self {
            Change::NewPattern(_) => 0,
            Change::NewNotes(_) => 1,
            Change::NewSound(..) => 2,
            Change::Shifted(_) => 3,
            Change::Length(..) => 4,
            Change::Muted(_) => 5,
            Change::Unmuted(_) => 6,
            Change::Reprise(_) => 7,
            _ => return None,
        })
    }

    pub fn describe(self) -> String {
        match self {
            Change::NewPattern(v) => format!("New {} pattern", v.name().to_lowercase()),
            Change::Muted(v) => format!("{} out", v.name()),
            Change::Unmuted(v) => format!("{} in", v.name()),
            Change::Shifted(v) => format!("{} shifted", v.name()),
            Change::NewSound(v, s) => format!("{} sound: {}", v.name(), v.sounds()[s as usize % v.sounds().len()]),
            Change::NewNotes(v) => format!("New {} notes", v.name().to_lowercase()),
            Change::Length(v, n) => format!("{} loops every {n} steps", v.name()),
            Change::Reprise(v) => format!("{} back to the theme", v.name()),
            Change::Breakdown => "Breakdown".into(),
            Change::Build => "Build-up".into(),
            Change::Drop => "Drop!".into(),
            Change::NewChapter => "Drop! New chapter".into(),
            Change::Scene(i) => format!("Favorite {}", i + 1),
            Change::Part(Part::Peak, ..) => "Drop! Peak".into(),
            Change::Part(p, _, slot) => match slot {
                Some(i) => format!("{} · favorite {}", p.name(), i + 1),
                None => p.name().into(),
            },
            Change::SongEnd => "The end".into(),
        }
    }
}

/// What the auto mode changed lately, so the next change goes somewhere else
#[derive(Clone, Copy, Default, Debug)]
pub struct Recent {
    /// The last two tracks changed
    voices: [Option<Voice>; 2],
    /// The last big change went to bass or synth
    melodic: bool,
}

impl Recent {
    pub(crate) fn note(&mut self, c: Change) {
        if let Some(v) = c.voice() {
            self.voices = [Some(v), self.voices[0]];
        }
    }

    pub(crate) fn fresh(&self, v: Voice) -> bool {
        !self.voices.contains(&Some(v))
    }
}

/// The drums and percussion the auto mode varies (the kick stays the kick)
const RHYTHM: [Voice; 5] = [Voice::Clap, Voice::Rim, Voice::Hat, Voice::OpenHat, Voice::Ride];
const MELODIC: [Voice; 2] = [Voice::Bass, Voice::Acid];
const EVOLVING: [Voice; 7] = [
    Voice::Clap,
    Voice::Rim,
    Voice::Hat,
    Voice::OpenHat,
    Voice::Ride,
    Voice::Bass,
    Voice::Acid,
];

/// A random item of `list` that passes `ok` – without collecting them (the audio thread calls this)
fn pick_if<T: Copy>(rng: &mut Rng, list: &[T], ok: impl Fn(T) -> bool) -> Option<T> {
    let n = list.iter().filter(|&&x| ok(x)).count();
    if n == 0 {
        return None;
    }
    let k = rng.below(n as u32) as usize;
    list.iter().copied().filter(|&x| ok(x)).nth(k)
}

/// One step of the auto mode: change one thing so the groove keeps moving but stays
/// recognizable. `big` is the change at the start of a phrase – a new pattern, new notes,
/// another sound, a track in or out – alternating between the drums and bass/synth; in
/// between come small touches – a shifted rim or ride, a half-bar loop. It never pokes the
/// tracks it changed last, never shifts the clap off the backbeat, and leaves locked tracks
/// alone (`None` when everything is locked). With `mutes` false it never takes a track out
/// or brings one in (the energy bar decides). Breakdowns and drops are arranged by the engine.
pub fn evolve(song: &mut Song, rng: &mut Rng, mutes: bool, big: bool, recent: &mut Recent) -> Option<Change> {
    let change = if big { None } else { touch(song, rng, recent) };
    let change = change.or_else(|| big_change(song, rng, mutes, recent));
    if let Some(c) = change {
        recent.note(c);
    }
    change
}

/// A small change between phrases: a shifted rim, ride or 303 line, or a half-bar loop
fn touch(song: &mut Song, rng: &mut Rng, recent: &Recent) -> Option<Change> {
    let free = |v: Voice| !song.track(v).lock && !song.track(v).mute && recent.fresh(v) && song.track(v).hits() > 0;
    if rng.chance(0.6) {
        // A 303 line shifted is a classic; stabs, plucks and hoovers sit on their chords
        let acid = song.track(Voice::Acid).sound < 2;
        let v = pick_if(rng, &[Voice::Rim, Voice::Ride, Voice::Acid], |v| {
            free(v) && (v != Voice::Acid || acid)
        })?;
        return Some(rotate(song, v, rng));
    }
    let v = pick_if(rng, &[Voice::Rim, Voice::Ride, Voice::OpenHat], |v| {
        let t = song.track(v);
        free(v) && t.len == 16 && t.steps[..8] != t.steps[8..]
    })?;
    let t = song.track_mut(v);
    t.steps.copy_within(..8, 8);
    if t.hits() == 0 {
        t.steps[2] = Step::hit();
        t.steps[10] = Step::hit();
    }
    Some(Change::Length(v, 8))
}

/// The change at the start of a phrase
fn big_change(song: &mut Song, rng: &mut Rng, mutes: bool, recent: &mut Recent) -> Option<Change> {
    let free = |v: Voice| !song.track(v).lock;
    // Mostly the other family than last time: drums, then bass or synth, then drums …
    let melodic = if recent.melodic { rng.chance(0.25) } else { rng.chance(0.6) };
    let family: &[Voice] = if melodic { &MELODIC } else { &RHYTHM };
    let v = pick_if(rng, family, |v| free(v) && recent.fresh(v))
        .or_else(|| pick_if(rng, &EVOLVING, |v| free(v) && recent.fresh(v)))
        .or_else(|| pick_if(rng, &EVOLVING, free))?;
    recent.melodic = MELODIC.contains(&v);
    let m = song.macros;
    let roll = rng.f32();
    let muted = song.track(v).mute;
    // A muted track coming back is a strong move – the synth more often than the rest
    let bring_in = mutes && muted && roll < if v == Voice::Acid { 0.6 } else { 0.4 };
    if bring_in {
        song.track_mut(v).mute = false;
        return Some(Change::Unmuted(v));
    }
    if mutes && !muted && v != Voice::Bass && roll < 0.15 {
        // Take one out – or, with only four left, bring another one in
        let playing = EVOLVING.iter().filter(|&&x| !song.track(x).mute).count();
        if playing > 4 {
            song.track_mut(v).mute = true;
            return Some(Change::Muted(v));
        }
        if let Some(w) = pick_if(rng, &EVOLVING, |x| free(x) && recent.fresh(x) && song.track(x).mute) {
            song.track_mut(w).mute = false;
            return Some(Change::Unmuted(w));
        }
    }
    if MELODIC.contains(&v) && roll < 0.6 {
        // Same rhythm, new notes
        let fresh = track_for(v, song.vibe, m.energy, song, rng);
        let t = song.track_mut(v);
        let mut notes = fresh.steps.iter().filter(|s| s.on).map(|s| s.note).cycle();
        for s in t.steps.iter_mut().filter(|s| s.on) {
            s.note = notes.next().unwrap_or(s.note);
        }
        return Some(Change::NewNotes(v));
    }
    if roll > 0.75 {
        // Another sound that suits the vibe (the synth gets a line made for its new sound)
        let old = song.track(v).sound;
        if let Some(new) = pick_if(rng, sounds_for(v, song.vibe), |s| s != old) {
            song.track_mut(v).sound = new;
            if v == Voice::Acid && !(new < 2 && old < 2) {
                let fresh = track_for(v, song.vibe, m.energy, song, rng);
                let t = song.track_mut(v);
                t.steps = fresh.steps;
                t.len = fresh.len;
            }
            return Some(Change::NewSound(v, new));
        }
    }
    let fresh = track_for(v, song.vibe, m.energy, song, rng);
    let t = song.track_mut(v);
    *t = Track {
        mute: t.mute,
        volume: t.volume,
        ..fresh
    };
    Some(Change::NewPattern(v))
}

/// New notes on the same rhythm for bass and synth (not when locked), e.g. after the
/// mood changed, so they follow the new scale and chords
pub fn refit_notes(song: &mut Song, rng: &mut Rng) {
    for v in [Voice::Bass, Voice::Acid] {
        if !song.track(v).lock {
            new_notes(song, v, rng);
        }
    }
}

/// New notes on the same rhythm for one track, fitting the vibe and the chords
pub fn new_notes(song: &mut Song, v: Voice, rng: &mut Rng) {
    let fresh = track_for(v, song.vibe, song.macros.energy, song, rng);
    let t = song.track_mut(v);
    let mut fresh_notes = fresh.steps.iter().filter(|s| s.on).map(|s| s.note).cycle();
    for s in t.steps.iter_mut().filter(|s| s.on) {
        s.note = fresh_notes.next().unwrap_or(s.note);
    }
}

/// What the energy does to the hats in the auto mode, written into the pattern: soft ghost
/// notes on about half the free off-beat 16ths. False when there was no room.
pub fn add_ghosts(song: &mut Song, v: Voice, rng: &mut Rng) -> bool {
    let t = song.track_mut(v);
    let mut added = false;
    for (i, s) in t.steps.iter_mut().enumerate() {
        if !s.on && i % 4 != 0 && rng.chance(0.5) {
            *s = Step {
                on: true,
                ghost: true,
                ..Step::default()
            };
            added = true;
        }
    }
    added
}

/// Every ghost note of a track out again
pub fn remove_ghosts(song: &mut Song, v: Voice) {
    for s in song.track_mut(v).steps.iter_mut().filter(|s| s.ghost) {
        *s = Step::default();
    }
}

/// What low energy does to the percussion in the auto mode, written into the pattern: the
/// steps without an accent play only half the time (100: back to always)
pub fn set_chance(song: &mut Song, v: Voice, chance: u8) {
    for s in song.track_mut(v).steps.iter_mut().filter(|s| s.on && !s.accent) {
        s.chance = chance;
    }
}

/// Bass and synth notes moved to the closest note of the scale (rhythm and line stay)
pub fn snap_notes(song: &mut Song) {
    let scale = song.scale;
    for v in [Voice::Bass, Voice::Acid] {
        for s in song.track_mut(v).steps.iter_mut() {
            s.note = scale.shift(s.note, 0);
        }
    }
}

/// A gentle change for a song: new notes for bass or synth, a new pattern for one of the
/// hats or percussion, or another sound for it – never a track out, never a shifted rhythm
pub fn vary(song: &mut Song, rng: &mut Rng) -> Option<Change> {
    let free = |v: Voice| !song.track(v).lock && !song.track(v).mute;
    let roll = rng.f32();
    if roll < 0.4 {
        if let Some(v) = pick_if(rng, &MELODIC, free) {
            let fresh = track_for(v, song.vibe, song.macros.energy, song, rng);
            let t = song.track_mut(v);
            let mut notes = fresh.steps.iter().filter(|s| s.on).map(|s| s.note).cycle();
            for s in t.steps.iter_mut().filter(|s| s.on) {
                s.note = notes.next().unwrap_or(s.note);
            }
            return Some(Change::NewNotes(v));
        }
    }
    let v = pick_if(rng, &[Voice::Hat, Voice::Rim, Voice::Ride, Voice::OpenHat], free)?;
    if roll < 0.75 {
        let fresh = track_for(v, song.vibe, song.macros.energy, song, rng);
        let t = song.track_mut(v);
        t.steps = fresh.steps;
        t.len = fresh.len;
        Some(Change::NewPattern(v))
    } else {
        let old = song.track(v).sound;
        let new = pick_if(rng, sounds_for(v, song.vibe), |s| s != old)?;
        song.track_mut(v).sound = new;
        Some(Change::NewSound(v, new))
    }
}

/// Rotate a pattern by a step or two – same notes, new feel
fn rotate(song: &mut Song, v: Voice, rng: &mut Rng) -> Change {
    let t = song.track_mut(v);
    let len = t.len as usize;
    t.steps[..len].rotate_right(*rng.pick(&[1usize, 2, 3]));
    Change::Shifted(v)
}

/// A new chapter after a drop: one or two bigger changes – a new bassline, a new synth
/// line, another sound, other chords or (rarely) another key – not everything at once.
/// A `bold` chapter (after the peak of the set) always changes two things.
pub fn new_chapter(song: &mut Song, rng: &mut Rng, bold: bool) -> Change {
    let vibe = song.vibe;
    // What can change; a key change is the rarest
    let kinds = [0u8, 1, 1, 2, 2, 3, 3, 4];
    let first = *rng.pick(&kinds);
    let second = (bold || rng.chance(0.4)).then(|| pick_if(rng, &kinds, |k| k != first)).flatten();
    for kind in std::iter::once(first).chain(second) {
        match kind {
            0 => {
                // Up a fourth or down a whole tone – stays in the bass range
                let shifted = song.root as i32 + *rng.pick(&[5, -2, 3, -5]);
                song.root = if (36..=47).contains(&shifted) {
                    shifted as u8
                } else {
                    (shifted - 12).clamp(36, 47) as u8
                };
            }
            1 | 2 => {
                let v = if kind == 1 { Voice::Bass } else { Voice::Acid };
                if song.track(v).lock {
                    continue;
                }
                let fresh = track_for(v, vibe, song.macros.energy, song, rng);
                let t = song.track_mut(v);
                // The synth sometimes comes in with its new line
                let mute = t.mute && !(v == Voice::Acid && rng.chance(0.4));
                *t = Track {
                    mute,
                    volume: t.volume,
                    sound: t.sound,
                    tune: t.tune,
                    decay: t.decay,
                    ..fresh
                };
            }
            3 => {
                let v = *rng.pick(&[Voice::Hat, Voice::Clap, Voice::Bass, Voice::Acid]);
                if !song.track(v).lock {
                    song.track_mut(v).sound = sound_for(v, vibe, rng);
                    if v == Voice::Acid {
                        // A line written for the new sound
                        let fresh = track_for(v, vibe, song.macros.energy, song, rng);
                        let t = song.track_mut(v);
                        t.steps = fresh.steps;
                        t.len = fresh.len;
                    }
                }
            }
            _ => {
                // Another mood of the vibe – unless one was picked by hand
                let now = song.mood();
                if song.mood_hold {
                    continue;
                }
                let Some(k) = pick_if(rng, moods_for(vibe), |k| Some(k) != now) else {
                    continue;
                };
                song.set_mood(k);
                refit_notes(song, rng);
            }
        }
    }
    Change::NewChapter
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One auto mode change, big or small
    fn step(s: &mut Song, rng: &mut Rng) -> Option<Change> {
        let big = rng.chance(0.5);
        evolve(s, rng, true, big, &mut Recent::default())
    }

    #[test]
    fn euclid_spreads_hits() {
        let e = euclid(4, 16, 0);
        assert_eq!(e.iter().filter(|&&b| b).count(), 4);
        assert!(e[0] && e[4] && e[8] && e[12]);
        assert_eq!(euclid(3, 8, 0).iter().filter(|&&b| b).count(), 3);
        assert_eq!(euclid(20, 16, 0).iter().filter(|&&b| b).count(), 16);
        assert_eq!(euclid(5, 16, 3).iter().filter(|&&b| b).count(), 5);
    }

    #[test]
    fn every_vibe_makes_a_techno_groove() {
        let mut rng = Rng::new(42);
        for vibe in Vibe::ALL {
            for _ in 0..50 {
                let s = generate(vibe, &Macros::default(), &mut rng);
                let kick = s.track(Voice::Kick);
                // Four to the floor (electro: at least on the one)
                if vibe.four_on_the_floor() {
                    for i in [0, 4, 8, 12] {
                        assert!(kick.steps[i].on, "{vibe:?}");
                    }
                }
                assert!(kick.steps[0].on);
                let (lo, hi) = vibe.bpm();
                assert!(s.bpm >= lo && s.bpm <= hi);
                // The bass never doubles the kick
                for i in 0..STEPS {
                    assert!(!(kick.steps[i].on && s.track(Voice::Bass).steps[i].on), "{vibe:?} step {i}");
                }
                for (t, v) in s.tracks.iter().zip(Voice::ALL) {
                    assert!((t.sound as usize) < v.sounds().len());
                }
                for t in &s.tracks {
                    assert!((1..=16).contains(&t.len));
                }
                // Synth notes stay in the scale
                for v in [Voice::Bass, Voice::Acid] {
                    for st in s.track(v).steps.iter().filter(|st| st.on) {
                        assert!(s.scale.intervals().contains(&st.note.rem_euclid(12)), "{v:?} {}", st.note);
                    }
                }
            }
        }
    }

    #[test]
    fn every_groove_has_a_mood_of_its_vibe() {
        let mut rng = Rng::new(5);
        for vibe in Vibe::ALL {
            for _ in 0..20 {
                let mut s = generate(vibe, &Macros::default(), &mut rng);
                assert!(s.mood().is_some_and(|k| moods_for(vibe).contains(&k)), "{vibe:?}");
                for _ in 0..6 {
                    new_chapter(&mut s, &mut rng, false);
                    assert!(s.mood().is_some(), "{vibe:?} after a new chapter");
                }
                // A mood picked by hand stays
                s.mood_hold = true;
                let k = s.mood();
                for _ in 0..10 {
                    new_chapter(&mut s, &mut rng, false);
                }
                assert_eq!(s.mood(), k);
            }
        }
    }

    #[test]
    fn the_tracks_fit_together() {
        let mut rng = Rng::new(77);
        for vibe in Vibe::ALL {
            for _ in 0..40 {
                let s = generate(vibe, &Macros::default(), &mut rng);
                let prog = progression(&s);
                // The bass plays the chord's root or fifth (in some octave) under every half bar
                for (i, st) in s.track(Voice::Bass).steps.iter().enumerate().filter(|(_, st)| st.on) {
                    let root = s.scale.shift(0, prog[i / 8]).rem_euclid(12);
                    let fifth = s.scale.shift(0, prog[i / 8] + 4).rem_euclid(12);
                    let sixth = s.scale.shift(0, prog[i / 8] - 2).rem_euclid(12);
                    let n = st.note.rem_euclid(12);
                    assert!(n == root || n == fifth || n == sixth, "{vibe:?} bass step {i}: {}", st.note);
                }
                // A 303 line lands on a chord note on every beat
                let synth = s.track(Voice::Acid);
                if synth.sound < 2 {
                    for i in (0..STEPS).step_by(4).filter(|&i| synth.steps[i].on) {
                        let chord = [0, 2, 4].map(|d| s.scale.shift(0, prog[i / 8] + d).rem_euclid(12));
                        assert!(chord.contains(&synth.steps[i].note.rem_euclid(12)), "{vibe:?} synth step {i}");
                    }
                    assert_eq!(synth.len, 16);
                }
                // Rims stay off the kick and the clap
                for i in 0..STEPS {
                    let rim = s.track(Voice::Rim).steps[i].on;
                    assert!(!(rim && (i % 4 == 0 || s.track(Voice::Clap).steps[i].on)), "{vibe:?} rim step {i}");
                }
                // Loops are a full bar or half a bar – always in step with the beat
                for t in &s.tracks {
                    assert_eq!(t.len, 16, "{vibe:?}");
                }
                // Every generated step always plays – "sometimes" is only for you to set (shift-click)
                for t in &s.tracks {
                    assert!(t.steps.iter().all(|st| st.chance == 100), "{vibe:?}");
                }
            }
        }
    }

    #[test]
    fn every_mood_is_its_own_and_its_notes_fit() {
        use crate::song::MOODS;
        let mut rng = Rng::new(9);
        for (i, a) in MOODS.iter().enumerate() {
            for b in &MOODS[i + 1..] {
                assert!((a.2, a.3) != (b.2, b.3), "{} and {} are the same", a.0, b.0);
            }
            let mut s = generate(Vibe::Hypnotic, &Macros::default(), &mut rng);
            s.scale = a.2;
            s.prog = a.3;
            refit_notes(&mut s, &mut rng);
            assert_eq!(s.mood(), Some(i));
            for v in [Voice::Bass, Voice::Acid] {
                for st in s.track(v).steps.iter().filter(|st| st.on) {
                    assert!(s.scale.intervals().contains(&st.note.rem_euclid(12)), "{} {v:?} {}", a.0, st.note);
                }
            }
        }
    }

    #[test]
    fn song_variations_are_gentle() {
        let mut rng = Rng::new(17);
        let mut s = generate(Vibe::Hypnotic, &Macros::default(), &mut rng);
        let before = s;
        for _ in 0..300 {
            let c = vary(&mut s, &mut rng);
            assert!(
                matches!(c, Some(Change::NewNotes(_) | Change::NewPattern(_) | Change::NewSound(..)) | None),
                "{c:?}"
            );
            // Nothing goes out or comes in, the kick, clap and bass rhythm stay
            assert_eq!(s.tracks.map(|t| t.mute), before.tracks.map(|t| t.mute));
            for v in [Voice::Kick, Voice::Clap] {
                assert_eq!(s.track(v), before.track(v));
            }
            assert_eq!(
                s.track(Voice::Bass).steps.map(|st| st.on),
                before.track(Voice::Bass).steps.map(|st| st.on)
            );
        }
    }

    #[test]
    fn ghost_notes_and_thinning_by_hand() {
        let mut song = Song::empty();
        let hat = Voice::Hat;
        song.track_mut(hat).steps[0] = Step::hit();
        song.track_mut(hat).steps[2] = Step {
            accent: true,
            ..Step::hit()
        };
        assert!(add_ghosts(&mut song, hat, &mut Rng::new(1)));
        let t = song.track(hat);
        assert!(t.steps.iter().enumerate().all(|(i, s)| !s.ghost || (i % 4 != 0 && s.on)));
        assert!(t.steps[0].on && !t.steps[0].ghost && t.steps[2].accent);
        set_chance(&mut song, hat, 50);
        assert!(song.track(hat).steps.iter().filter(|s| s.on).all(|s| (s.chance == 50) != s.accent));
        remove_ghosts(&mut song, hat);
        assert_eq!(song.track(hat).hits(), 2);
    }

    #[test]
    fn auto_mode_picks_sounds_of_the_vibe() {
        let mut rng = Rng::new(5);
        for vibe in Vibe::ALL {
            let mut s = generate(vibe, &Macros::default(), &mut rng);
            for _ in 0..300 {
                if let Some(Change::NewSound(v, n)) = step(&mut s, &mut rng) {
                    assert!(sounds_for(v, vibe).contains(&n), "{vibe:?} {v:?} {n}");
                }
                assert_eq!(s.track(Voice::Acid).len, 16);
            }
        }
    }

    #[test]
    fn evolve_keeps_the_groove_alive() {
        let mut rng = Rng::new(3);
        let mut s = generate(Vibe::Peak, &Macros::default(), &mut rng);
        let before = s;
        let mut changes = 0;
        for _ in 0..200 {
            assert!(step(&mut s, &mut rng).is_some());
            changes += 1;
            // The kick is never touched by the auto mode
            assert_eq!(s.track(Voice::Kick), before.track(Voice::Kick));
            for t in &s.tracks {
                assert!((1..=16).contains(&t.len));
            }
            let playing = s.tracks[1..].iter().filter(|t| !t.mute).count();
            assert!(playing >= 4, "only {playing} tracks left");
        }
        assert_eq!(changes, 200);
        assert_ne!(s, before);
        s.sanitize();
    }

    #[test]
    fn vibes_sound_different() {
        // Each vibe has its own set of sounds – the generator must not pick the same kit everywhere
        let mut rng = Rng::new(8);
        let kits: Vec<[u8; 8]> = Vibe::ALL
            .iter()
            .map(|&v| generate(v, &Macros::default(), &mut rng).tracks.map(|t| t.sound))
            .collect();
        let distinct = kits.iter().filter(|k| kits.iter().filter(|o| o == k).count() == 1).count();
        assert!(distinct >= 7, "{kits:?}");
    }

    #[test]
    fn kick_dice_changes_something() {
        // The dice next to the kick used to roll the same four hits again and again
        let mut rng = Rng::new(12);
        let song = generate(Vibe::Hypnotic, &Macros::default(), &mut rng);
        let rolls: Vec<_> = (0..10)
            .map(|_| track_for(Voice::Kick, Vibe::Hypnotic, 0.6, &song, &mut rng).steps)
            .collect();
        let different = rolls.iter().filter(|r| **r != rolls[0]).count();
        assert!(different >= 4);
    }

    #[test]
    fn evolve_moves_around_and_touches_lightly() {
        let mut rng = Rng::new(12);
        let mut s = generate(Vibe::Peak, &Macros::default(), &mut rng);
        let mut recent = Recent::default();
        let mut last: Vec<Voice> = vec![];
        let mut families = 0;
        for i in 0..400 {
            let big = i % 4 == 0;
            let c = evolve(&mut s, &mut rng, true, big, &mut recent).unwrap();
            let v = c.voice().unwrap();
            assert!(!last.iter().rev().take(2).any(|&w| w == v), "{v:?} again after {last:?}");
            if matches!(c, Change::Shifted(_) | Change::Length(..)) {
                // Small touches only – the clap stays on the backbeat
                assert!(matches!(v, Voice::Rim | Voice::Ride | Voice::OpenHat | Voice::Acid), "{c:?}");
            }
            if big {
                families += usize::from(MELODIC.contains(&v));
            }
            last.push(v);
        }
        // Bass and synth get roughly every third big change
        assert!((25..=60).contains(&families), "{families}");
    }

    #[test]
    fn shuffle_never_sounds_like_the_groove_before() {
        let mut rng = Rng::new(21);
        let (mut shuffled, mut plain) = (0.0, 0.0);
        for vibe in Vibe::ALL {
            let now = generate(vibe, &Macros::default(), &mut rng);
            for _ in 0..10 {
                let s = shuffle(vibe, &Macros::default(), &mut rng, &[None; TRACKS], &now);
                assert!(distance(&s, &now) > 4.0, "{vibe:?}");
                shuffled += distance(&s, &now);
                plain += distance(&generate(vibe, &Macros::default(), &mut rng), &now);
            }
        }
        assert!(shuffled > plain * 1.1, "{shuffled} vs {plain}");
    }

    #[test]
    fn shuffles_of_one_vibe_differ_in_drive() {
        // Same vibe, same energy – still busier and sparser grooves
        let mut rng = Rng::new(2);
        let hits: Vec<usize> = (0..30)
            .map(|_| {
                let s = generate(Vibe::Hypnotic, &Macros::default(), &mut rng);
                s.tracks.iter().filter(|t| !t.mute).map(|t| t.hits()).sum()
            })
            .collect();
        let (lo, hi) = (hits.iter().min().unwrap(), hits.iter().max().unwrap());
        assert!(hi - lo >= 10, "{hits:?}");
    }

    #[test]
    fn locked_tracks_stay_as_they_are() {
        let mut rng = Rng::new(31);
        let mut s = generate(Vibe::Acid, &Macros::default(), &mut rng);
        for v in [Voice::Bass, Voice::Hat, Voice::Clap] {
            s.track_mut(v).lock = true;
        }
        let before = s;
        for _ in 0..300 {
            step(&mut s, &mut rng);
            new_chapter(&mut s, &mut rng, false);
        }
        for v in [Voice::Bass, Voice::Hat, Voice::Clap] {
            assert_eq!(s.track(v), before.track(v), "{v:?}");
        }
        // Everything locked: the auto mode has nothing to do
        for t in s.tracks.iter_mut() {
            t.lock = true;
        }
        assert_eq!(step(&mut s, &mut rng), None);
        // Shuffle keeps locked tracks
        let mut keep = [None; TRACKS];
        keep[Voice::Bass.index()] = Some(*before.track(Voice::Bass));
        let fresh = generate_keeping(Vibe::Dub, &Macros::default(), &mut rng, &keep);
        assert_eq!(fresh.track(Voice::Bass), before.track(Voice::Bass));
    }

    #[test]
    fn new_chapter_stays_in_range() {
        let mut rng = Rng::new(21);
        let mut s = generate(Vibe::Rave, &Macros::default(), &mut rng);
        for _ in 0..100 {
            new_chapter(&mut s, &mut rng, false);
            assert!((36..=47).contains(&s.root));
            let copy = s;
            s.sanitize();
            assert_eq!(s, copy);
        }
    }
}
