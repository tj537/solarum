//! Solari's ear: how a groove sounds to someone who knows techno, and the director that
//! picks the auto mode's changes with it.
//!
//! The generator only knows how to make a change. The director makes a few of them on
//! copies of the groove (a `Song` is `Copy`, so nothing allocates on the audio thread),
//! listens to each and plays the one that sounds best right now:
//!
//! - **The low end:** bass notes on the kick fight for the same room – fewer is better.
//! - **Density:** as busy as the energy asks for – sparse when it's low, full when it's high.
//! - **Crowding:** four tracks on one step is a wall, not a groove.
//! - **Step size:** a change you can hear, but not a different track – bigger at a phrase.
//! - **The arc:** over a cycle the groove wanders away from its theme (how it was when the
//!   cycle began), furthest at about two thirds, and finds back to it before the drop –
//!   so the reprise of a track is one of the candidates.
//! - **Taste:** what you kept or undid in SOLAR OS. It learns per track and per kind.
//!
//! A little chance on top keeps it from always making the same choice.

use crate::gen::{self, Change, Recent};
use crate::rng::Rng;
use crate::song::{Song, Track, Voice, STEPS, TRACKS};
use serde::{Deserialize, Serialize};

/// How many changes the director tries before it plays one
const CANDIDATES: usize = 6;

/// What a groove sounds like, in numbers
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hearing {
    /// Hits per bar without the kick (ghosts and chance steps count for what they play)
    pub density: f32,
    /// Share of the bass notes that land on a kick
    pub clash: f32,
    /// Share of the steps where four or more tracks hit at once
    pub crowd: f32,
}

/// Listen to one bar of the groove as it plays (muted tracks are silent)
pub fn hear(song: &Song) -> Hearing {
    let on = |t: &Track, i: usize| {
        let s = t.steps[i % t.len.max(1) as usize];
        if !s.on || t.mute {
            0.0
        } else {
            let w = if s.ghost { 0.5 } else { 1.0 };
            w * s.chance.min(100) as f32 / 100.0
        }
    };
    let (kick, bass) = (song.track(Voice::Kick), song.track(Voice::Bass));
    let (mut density, mut notes, mut clash, mut crowd) = (0.0, 0.0, 0.0, 0.0);
    for i in 0..STEPS {
        let mut at_once = 0;
        for v in Voice::ALL.into_iter().filter(|&v| v != Voice::Kick) {
            let x = on(song.track(v), i);
            density += x;
            if x >= 0.75 {
                at_once += 1;
            }
        }
        if at_once >= 4 {
            crowd += 1.0;
        }
        let b = on(bass, i);
        notes += b;
        clash += b * on(kick, i).min(1.0);
    }
    Hearing {
        density,
        clash: if notes > 0.0 { clash / notes } else { 0.0 },
        crowd: crowd / STEPS as f32,
    }
}

/// How many hits a bar wants at this energy (without the kick)
pub fn density_goal(energy: f32) -> f32 {
    16.0 + 30.0 * energy.clamp(0.0, 1.0)
}

/// How far the groove should have wandered from its theme `progress` (0…1) into the groove
/// part of the cycle: away, furthest at two thirds, then back towards it before the drop.
/// A hot set wanders further.
pub fn drift_goal(progress: f32, heat: f32) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    let peak = 3.0 + 4.0 * heat.clamp(0.0, 1.0);
    let shape = if p < 0.65 { p / 0.65 } else { 1.0 - 0.6 * (p - 0.65) / 0.35 };
    peak * shape
}

/// How far the tracks are from the theme: steps that moved, other sounds (mutes don't count –
/// the energy takes tracks out and brings them back)
pub fn drift(a: &[Track; TRACKS], b: &[Track; TRACKS]) -> f32 {
    a.iter().zip(b).map(|(x, y)| track_drift(x, y)).sum()
}

fn track_drift(x: &Track, y: &Track) -> f32 {
    let moved = x
        .steps
        .iter()
        .zip(&y.steps)
        .filter(|(p, q)| p.on != q.on || (p.on && p.note != q.note))
        .count();
    moved as f32 / 4.0 + if x.sound != y.sound { 1.0 } else { 0.0 } + if x.len != y.len { 0.5 } else { 0.0 }
}

/// What the auto mode is after with its next change
#[derive(Clone, Copy, Debug)]
pub struct Aim {
    /// The change that opens a phrase (bigger) – or a small touch in between
    pub big: bool,
    /// It may take tracks out and bring them in
    pub mutes: bool,
    /// How far into the groove part of the cycle, 0…1
    pub progress: f32,
    /// How hot the set is, 0…1
    pub heat: f32,
    /// Tracks you just worked on – left alone for now
    pub hands_off: [bool; TRACKS],
}

/// Why the director picked a change – said in the log when it's the reason
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    /// The bass got out of the kick's way
    Room,
    /// Fewer hits: the energy is low
    Air,
    /// More hits: the energy is high
    Drive,
    /// The theme came back
    Home,
}

impl Why {
    pub fn describe(self) -> &'static str {
        match self {
            Why::Room => "room for the kick",
            Why::Air => "more air",
            Why::Drive => "more drive",
            Why::Home => "the theme returns",
        }
    }
}

/// The kinds of change Solari learns your taste for (`Change::kind`)
pub const KINDS: usize = 8;
pub const KIND_NAMES: [&str; KINDS] = [
    "new patterns",
    "new notes",
    "new sounds",
    "shifts",
    "short loops",
    "tracks out",
    "tracks in",
    "reprises",
];

/// What you kept (👍) and undid (👎) in SOLAR OS, per track and per kind of change.
/// Saved with the settings, so Solari knows you in every project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Taste {
    pub voice: [i8; TRACKS],
    pub kind: [i8; KINDS],
}

impl Taste {
    const LIMIT: i8 = 6;

    /// A thumb up or down for a change
    pub fn learn(&mut self, c: Change, liked: bool) {
        let d = if liked { 1 } else { -2 };
        if let Some(v) = c.voice() {
            let x = &mut self.voice[v.index()];
            *x = (*x + d).clamp(-Self::LIMIT, Self::LIMIT);
        }
        if let Some(k) = c.kind() {
            let x = &mut self.kind[k];
            *x = (*x + d).clamp(-Self::LIMIT, Self::LIMIT);
        }
    }

    /// How much more (or less) a change appeals to you
    pub fn bonus(&self, c: Change) -> f32 {
        let v = c.voice().map_or(0, |v| self.voice[v.index()]);
        let k = c.kind().map_or(0, |k| self.kind[k]);
        0.12 * (v + k) as f32
    }

    pub fn is_empty(&self) -> bool {
        *self == Taste::default()
    }

    /// What it has learned, in words (for the terminal)
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return "No taste yet – give a change a thumb up or down".into();
        }
        let list = |like: bool| {
            let mut out: Vec<&str> = Vec::new();
            let keep = |x: i8| if like { x > 0 } else { x < 0 };
            out.extend(Voice::ALL.iter().filter(|v| keep(self.voice[v.index()])).map(|v| v.name()));
            out.extend((0..KINDS).filter(|&k| keep(self.kind[k])).map(|k| KIND_NAMES[k]));
            out.join(", ")
        };
        let (more, less) = (list(true), list(false));
        match (more.is_empty(), less.is_empty()) {
            (false, false) => format!("More: {more} · less: {less}"),
            (false, true) => format!("More: {more}"),
            _ => format!("Less: {less}"),
        }
    }
}

/// The candidate the director plays
struct Pick {
    score: f32,
    song: Song,
    recent: Recent,
    change: Change,
}

/// The auto mode's next change: try a few, listen, play the best. `home` is the theme
/// (the tracks as the cycle began). `None` when nothing can change (all locked).
pub fn choose(
    song: &mut Song,
    rng: &mut Rng,
    aim: &Aim,
    recent: &mut Recent,
    home: &[Track; TRACKS],
    taste: &Taste,
) -> Option<(Change, Option<Why>)> {
    // Tracks you're working on count as locked while the candidates are made
    let mut base = *song;
    for (t, &off) in base.tracks.iter_mut().zip(&aim.hands_off) {
        t.lock |= off;
    }
    let before = hear(&base);
    let goal = density_goal(base.macros.energy);
    let step_goal = if aim.big { 1.5 } else { 0.6 };
    let drift_to = drift_goal(aim.progress, aim.heat);
    let score = |c: &Song, change: Change, luck: f32| {
        let h = hear(c);
        let d = gen::distance(c, &base);
        let mut s = -2.5 * h.clash - 1.5 * h.crowd - 1.2 * (h.density - goal).abs() / 12.0;
        s -= 0.8 * ((d - step_goal).abs() / step_goal).min(2.0);
        if d < 0.01 {
            // Nothing you'd hear
            s -= 3.0;
        }
        s -= 0.25 * (drift(&c.tracks, home) - drift_to).abs();
        s + taste.bonus(change) + 0.35 * luck
    };
    let mut best: Option<Pick> = None;
    let mut offer = |p: Pick| {
        if best.as_ref().is_none_or(|b| p.score > b.score) {
            best = Some(p);
        }
    };
    for _ in 0..CANDIDATES {
        let mut r = Rng::new(((rng.next_u32() as u64) << 32) | rng.next_u32() as u64);
        let (mut c, mut rec) = (base, *recent);
        if let Some(change) = gen::evolve(&mut c, &mut r, aim.mutes, aim.big, &mut rec) {
            let score = score(&c, change, r.f32());
            offer(Pick {
                score,
                song: c,
                recent: rec,
                change,
            });
        }
    }
    // The reprise: the track that wandered furthest goes back to the theme
    if aim.big {
        let far = Voice::ALL
            .into_iter()
            .filter(|&v| !base.track(v).lock && recent.fresh(v))
            .map(|v| (v, track_drift(base.track(v), &home[v.index()])))
            .filter(|&(v, d)| d >= 1.0 && home[v.index()].hits() > 0)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((v, _)) = far {
            let (mut c, mut rec) = (base, *recent);
            let h = home[v.index()];
            let t = c.track_mut(v);
            (t.steps, t.len, t.sound, t.tune, t.decay) = (h.steps, h.len, h.sound, h.tune, h.decay);
            let change = Change::Reprise(v);
            rec.note(change);
            let score = score(&c, change, rng.f32());
            offer(Pick {
                score,
                song: c,
                recent: rec,
                change,
            });
        }
    }
    let pick = best?;
    let after = hear(&pick.song);
    let why = if matches!(pick.change, Change::Reprise(_)) {
        Some(Why::Home)
    } else if before.clash - after.clash > 0.15 {
        Some(Why::Room)
    } else if (before.density - goal).abs() - (after.density - goal).abs() > 2.5 {
        Some(if after.density > before.density { Why::Drive } else { Why::Air })
    } else {
        None
    };
    // Your locks stay yours – and the hands-off tracks aren't locked for real
    let locks = song.tracks.map(|t| t.lock);
    *song = pick.song;
    for (t, l) in song.tracks.iter_mut().zip(locks) {
        t.lock = l;
    }
    *recent = pick.recent;
    Some((pick.change, why))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{Macros, Vibe};

    fn aim(big: bool, progress: f32) -> Aim {
        Aim {
            big,
            mutes: true,
            progress,
            heat: 0.6,
            hands_off: [false; TRACKS],
        }
    }

    #[test]
    fn the_ear_hears_the_bass_on_the_kick() {
        let mut s = Song::default();
        for t in s.tracks.iter_mut() {
            *t = Track::default();
        }
        for i in [0, 4, 8, 12] {
            s.track_mut(Voice::Kick).steps[i].on = true;
        }
        let b = s.track_mut(Voice::Bass);
        b.steps[0].on = true;
        b.steps[2].on = true;
        assert!((hear(&s).clash - 0.5).abs() < 1e-6);
        s.track_mut(Voice::Kick).mute = true;
        assert_eq!(hear(&s).clash, 0.0);
    }

    #[test]
    fn the_density_goal_fits_what_the_generator_makes() {
        // Grooves at medium energy land near the goal – so the ear steers, it doesn't fight
        let mut rng = Rng::new(3);
        let mut off = 0.0;
        let n = 60;
        for i in 0..n {
            let vibe = Vibe::ALL[i % Vibe::ALL.len()];
            let s = gen::generate(vibe, &Macros::default(), &mut rng);
            off += (hear(&s).density - density_goal(s.macros.energy)) / n as f32;
        }
        assert!(off.abs() < 6.0, "generated grooves are {off:.1} hits off the goal");
    }

    #[test]
    fn the_arc_wanders_off_and_comes_home_before_the_drop() {
        let (start, top, end) = (drift_goal(0.0, 0.5), drift_goal(0.65, 0.5), drift_goal(1.0, 0.5));
        assert!(start < 0.1 && top > end && end > start);
        assert!(drift_goal(0.65, 1.0) > top, "a hot set wanders further");
    }

    #[test]
    fn the_director_changes_something_you_hear_and_keeps_your_locks() {
        let mut rng = Rng::new(9);
        let mut s = gen::generate(Vibe::Hypnotic, &Macros::default(), &mut rng);
        s.track_mut(Voice::Hat).lock = true;
        let home = s.tracks;
        let mut recent = Recent::default();
        for k in 0..40 {
            let before = s;
            let (c, _) = choose(&mut s, &mut rng, &aim(k % 2 == 0, 0.5), &mut recent, &home, &Taste::default()).unwrap();
            assert_ne!(c.voice(), Some(Voice::Hat));
            assert!(s.track(Voice::Hat).lock);
            assert_eq!(s.track(Voice::Hat).steps, before.track(Voice::Hat).steps);
        }
    }

    #[test]
    fn hands_off_tracks_stay_as_you_left_them_and_stay_unlocked() {
        let mut rng = Rng::new(4);
        let mut s = gen::generate(Vibe::Acid, &Macros::default(), &mut rng);
        let home = s.tracks;
        let mut off = [false; TRACKS];
        off[Voice::Bass.index()] = true;
        off[Voice::Acid.index()] = true;
        let a = Aim {
            hands_off: off,
            ..aim(true, 0.3)
        };
        let mut recent = Recent::default();
        let (bass, acid) = (*s.track(Voice::Bass), *s.track(Voice::Acid));
        for _ in 0..30 {
            choose(&mut s, &mut rng, &a, &mut recent, &home, &Taste::default());
        }
        assert_eq!(s.track(Voice::Bass).steps, bass.steps);
        assert_eq!(s.track(Voice::Acid).steps, acid.steps);
        assert!(!s.track(Voice::Bass).lock && !s.track(Voice::Acid).lock);
    }

    #[test]
    fn late_in_the_cycle_the_theme_comes_back() {
        // A groove that wandered far: near the drop the director brings tracks home
        let mut rng = Rng::new(21);
        let mut s = gen::generate(Vibe::Peak, &Macros::default(), &mut rng);
        let home = s.tracks;
        let mut recent = Recent::default();
        for _ in 0..12 {
            choose(&mut s, &mut rng, &aim(true, 0.6), &mut recent, &home, &Taste::default());
        }
        let far = drift(&s.tracks, &home);
        let mut reprises = 0;
        for _ in 0..8 {
            if let Some((Change::Reprise(_), why)) = choose(&mut s, &mut rng, &aim(true, 1.0), &mut recent, &home, &Taste::default()) {
                assert_eq!(why, Some(Why::Home));
                reprises += 1;
            }
        }
        assert!(reprises >= 2, "only {reprises} reprises");
        assert!(drift(&s.tracks, &home) < far, "the groove found back towards its theme");
    }

    #[test]
    fn taste_steers_the_choice() {
        // Undo every sound swap and every bass change: Solari stops making them
        let mut taste = Taste::default();
        for _ in 0..4 {
            taste.learn(Change::NewSound(Voice::Hat, 0), false);
            taste.learn(Change::NewNotes(Voice::Bass), false);
        }
        assert!(taste.summary().starts_with("Less:"));
        let count = |taste: &Taste| {
            let mut rng = Rng::new(5);
            let mut s = gen::generate(Vibe::Hard, &Macros::default(), &mut rng);
            let home = s.tracks;
            let mut recent = Recent::default();
            (0..60)
                .filter_map(|_| choose(&mut s, &mut rng, &aim(true, 0.4), &mut recent, &home, taste))
                .filter(|(c, _)| matches!(c, Change::NewSound(..)) || c.voice() == Some(Voice::Bass))
                .count()
        };
        let (free, taught) = (count(&Taste::default()), count(&taste));
        assert!(taught * 2 < free, "{taught} vs {free}");
    }
}
