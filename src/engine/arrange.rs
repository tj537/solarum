//! The arrangement: the auto mode's cycles (plans, breakdowns, build-ups, drops, fills, the
//! energy curve and AUTO FX's knobs) and song mode's sections. Everything here runs on a
//! bar line or a step, from `Engine::fire`.

use super::*;

/// A build-up: two bars from `start`. On top of the groove turning into it, a gate chops
/// harder and harder under a light high-pass
pub(super) struct Build {
    pub(super) start: u64,
    pub(super) len: u64,
}

/// What a track plays at step `idx` of a build-up that started at `start` (progress `p`)
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum BuildPlay {
    /// Its own pattern
    Pattern,
    Rest,
    /// A hit at this velocity, whatever the pattern says
    Hit(f32),
    /// The pattern looping a short piece from step `at`, `up` semitones higher
    Loop {
        at: u64,
        up: i8,
    },
}

/// The build-up made from the groove: in the first bar everything plays on, the kick
/// already rolling in quarters. In the second bar the kick rolls in 8ths, then 16ths,
/// the hats run in 16ths, the open hat rests, and bass, synth and percussion loop the
/// bar's first beat, then half a beat – the synths an octave up for the last stretch.
pub(super) fn build_play(voice: Voice, p: f32, idx: u64, start: u64) -> BuildPlay {
    let second = start + STEPS as u64;
    let late = idx >= second;
    let last = p >= 0.75;
    match voice {
        Voice::Kick => {
            let every = if !late {
                4
            } else if !last {
                2
            } else {
                1
            };
            if (idx - start).is_multiple_of(every) {
                BuildPlay::Hit(0.6 + 0.4 * p)
            } else {
                BuildPlay::Rest
            }
        }
        Voice::Hat if late => BuildPlay::Hit(0.45 + 0.45 * (p - 0.5) * 2.0),
        Voice::OpenHat if late => BuildPlay::Rest,
        Voice::Bass | Voice::Acid | Voice::Rim | Voice::Ride if late => {
            let len = if last { 2 } else { 4 };
            let synth = matches!(voice, Voice::Bass | Voice::Acid);
            BuildPlay::Loop {
                at: second + (idx - second) % len,
                up: if last && synth { 12 } else { 0 },
            }
        }
        _ => BuildPlay::Pattern,
    }
}

/// Steps of silence before a drop lands (half a beat)
const DROP_GAP: u64 = 2;

/// Where a song's energy is about to drop (the slim song bar shows it) – by hand there's no
/// drop zone any more, DROP drops
pub const DROP_ZONE: f32 = 0.9;

/// How the auto mode arranges one 32-bar cycle
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Plan {
    /// Bar of the cycle where the breakdown starts (it lasts until the build-up at bar 30);
    /// 30: no breakdown, the groove rolls straight into the build-up
    pub(super) breakdown_at: u64,
    /// Tracks that leave for the breakdown
    pub(super) out: &'static [Voice],
    /// A low-pass filter opens slowly through the breakdown
    pub(super) sweep: bool,
    /// The energy at the bottom of the breakdown: what else the layers take out
    pub(super) depth: f32,
}

pub(super) const PLANS: [Plan; 5] = [
    // Classic: kick and bass out for six bars
    Plan {
        breakdown_at: 24,
        out: &[Voice::Kick, Voice::Bass],
        sweep: false,
        depth: 0.2,
    },
    // Short and filtered: only the kick leaves, for two bars – the bass rolls on
    Plan {
        breakdown_at: 28,
        out: &[Voice::Kick],
        sweep: true,
        depth: 0.45,
    },
    // Stripped: just hats, percussion and the synth line
    Plan {
        breakdown_at: 24,
        out: &[Voice::Kick, Voice::Bass, Voice::Clap, Voice::Ride],
        sweep: false,
        depth: 0.12,
    },
    // Filtered: no kick, everything else under a slowly opening filter
    Plan {
        breakdown_at: 24,
        out: &[Voice::Kick],
        sweep: true,
        depth: 0.35,
    },
    // Roller: no breakdown – the groove rolls on into a build-up and drops on itself
    Plan {
        breakdown_at: 30,
        out: &[],
        sweep: false,
        depth: 0.0,
    },
];

/// Which plans suit a cycle of the set: calm cycles roll or filter, the peak strips down
fn plans_for(heat: f32) -> &'static [usize] {
    if heat >= 0.9 {
        &[0, 2, 0]
    } else if heat < 0.5 {
        &[1, 3, 4]
    } else {
        &[0, 1, 3, 4]
    }
}

/// The knobs AUTO FX plays with: eight track volumes, dark and space, the master EQ
pub(super) const KNOBS: usize = TRACKS + 5;

/// How far each knob may wander from where you put it
pub(super) const KNOB_RANGE: [f32; KNOBS] = [0.1, 0.12, 0.12, 0.14, 0.1, 0.14, 0.14, 0.12, 0.08, 0.1, 0.05, 0.04, 0.05];

pub(super) fn knobs(s: &Song) -> [f32; KNOBS] {
    let mut k = [0.0; KNOBS];
    for (v, t) in k.iter_mut().zip(&s.tracks) {
        *v = t.volume;
    }
    k[TRACKS..].copy_from_slice(&[s.macros.dark, s.macros.space, s.eq.low, s.eq.mid, s.eq.high]);
    k
}

pub(super) fn set_knobs(s: &mut Song, k: [f32; KNOBS]) {
    for (t, v) in s.tracks.iter_mut().zip(k) {
        t.volume = v;
    }
    [s.macros.dark, s.macros.space, s.eq.low, s.eq.mid, s.eq.high] = k[TRACKS..].try_into().unwrap();
}

pub(super) struct Breakdown {
    pub(super) start: u64,
    pub(super) len: u64,
    pub(super) out: &'static [Voice],
    /// Mute state of every track before the breakdown
    pub(super) before: [bool; TRACKS],
    pub(super) sweep: bool,
}

/// A one-bar fill at the end of an 8-bar phrase (auto mode, or by hand: `Cmd::Fill`)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fill {
    /// Clap roll into the next phrase
    Roll,
    /// Kick and bass drop out for the last beat
    Gap,
    /// Falling toms
    Toms,
    /// Open hats on every 8th
    OpenHats,
    /// The clap thrown into the echo
    Throw,
}

pub const FILLS: [Fill; 5] = [Fill::Roll, Fill::Gap, Fill::Toms, Fill::OpenHats, Fill::Throw];

impl Fill {
    pub fn name(self) -> &'static str {
        match self {
            Fill::Roll => "Clap roll",
            Fill::Gap => "Gap – kick and bass out for a beat",
            Fill::Toms => "Falling toms",
            Fill::OpenHats => "Open hats",
            Fill::Throw => "Clap into the echo",
        }
    }
}
/// Fills that close 16 bars, and the lighter ones for the 8 bars in between
const BIG_FILLS: [Fill; 3] = [Fill::Roll, Fill::Toms, Fill::Throw];
const SMALL_FILLS: [Fill; 2] = [Fill::Gap, Fill::OpenHats];

impl Engine {
    pub(super) fn start_build(&mut self) {
        // Two bars of build-up from the next bar on, so it always starts and lands on the one
        let next = self.last_fired.map_or(0, |s| s + 1);
        self.build = Some(Build {
            start: next.div_ceil(STEPS as u64) * STEPS as u64,
            len: 2 * STEPS as u64,
        });
        self.send_song(Some(Change::Build));
    }

    /// A build-up the arrangement starts now (auto mode, song mode)
    fn auto_build(&mut self, idx: u64) {
        self.build = Some(Build {
            start: idx,
            len: 2 * STEPS as u64,
        });
        self.send_song(Some(Change::Build));
    }

    /// 0…1 through the build-up; None before it starts
    pub(super) fn build_progress(&self, idx: f64) -> Option<f32> {
        self.build
            .as_ref()
            .filter(|b| idx >= b.start as f64)
            .map(|b| ((idx - b.start as f64) / b.len as f64).clamp(0.0, 1.0) as f32)
    }

    /// The breath before the drop: the last half beat of the build-up
    pub(super) fn in_drop_gap(&self, idx: f64) -> bool {
        self.build.as_ref().is_some_and(|b| idx >= (b.start + b.len - DROP_GAP) as f64)
    }

    /// The last bar of a phrase: a fill – a big one every 16 bars, never the same twice in a row
    fn phrase_end(&mut self, bar: u64, big: bool) {
        let last = self.last_fill;
        let fills: &[Fill] = if big { &BIG_FILLS } else { &SMALL_FILLS };
        let n = fills.iter().filter(|&&f| Some(f) != last).count() as u32;
        let f = fills
            .iter()
            .copied()
            .filter(|&f| Some(f) != last)
            .nth(self.auto_rng.below(n) as usize);
        self.fill = f.map(|f| (bar, f));
        self.last_fill = f;
    }

    /// The auto mode with AUTO FX: it plays with the mix (`drift_knobs`)
    fn drifts_mix(&self) -> bool {
        self.song.auto && self.song.auto_fx && self.arrangement.is_none()
    }

    /// A track whose layer the energy falls below is muted (the mute button shows it) – by
    /// hand, in the auto mode and in songs alike; when the energy comes back, the tracks it
    /// muted come back too. Only crossings count, so a track you unmute stays on. True when
    /// a mute changed.
    pub(super) fn energy_mutes(&mut self) -> bool {
        let mut changed = false;
        let (before, e) = (self.last_layer, self.layer_energy);
        self.last_layer = e;
        let held: &[Voice] = self.breakdown.as_ref().map_or(&[], |b| b.out);
        for (i, t) in self.song.tracks.iter_mut().enumerate() {
            if held.contains(&Voice::ALL[i]) {
                continue;
            }
            let layer = LAYERS[i];
            if before >= layer && e < layer && !t.mute && !t.lock {
                t.mute = true;
                self.energy_out[i] = true;
                self.mute_pending[i] = Some(true);
                changed = true;
            } else if e >= layer && self.energy_out[i] {
                self.energy_out[i] = false;
                if t.mute {
                    t.mute = false;
                    self.mute_pending[i] = Some(false);
                    changed = true;
                }
            }
        }
        changed
    }

    /// AUTO FX plays with the mix: every knob glides towards its goal (back home when the
    /// auto mode or AUTO FX is off). True when a knob moved.
    pub(super) fn drift_knobs(&mut self) -> bool {
        if !(self.drifts_mix() && self.playing) {
            self.knob_goal = [0.0; KNOBS];
        }
        if self.knob_off == self.knob_goal {
            return false;
        }
        let mut k = knobs(&self.song);
        for i in 0..KNOBS {
            // Across its whole range in about two bars
            let rate = KNOB_RANGE[i] / 32.0;
            let off = &mut self.knob_off[i];
            *off += (self.knob_goal[i] - *off).clamp(-rate, rate);
            if (self.knob_goal[i] - *off).abs() < 1e-5 {
                *off = self.knob_goal[i];
            }
            k[i] = (self.knob_home[i] + *off).clamp(0.0, 1.0);
        }
        set_knobs(&mut self.song, k);
        true
    }

    /// New goals for a couple of knobs: a track a bit louder or softer, the room a bit
    /// bigger, the tone a bit darker – never far from where you put them
    fn new_knob_goals(&mut self, n: usize) {
        for _ in 0..n {
            let i = self.auto_rng.below(KNOBS as u32) as usize;
            let r = KNOB_RANGE[i];
            // Now and then back home
            self.knob_goal[i] = if self.auto_rng.chance(0.25) {
                0.0
            } else {
                (self.auto_rng.f32() * 2.0 - 1.0) * r
            };
        }
    }

    /// The energy may play with the pattern (ghost hats, thinner percussion): only while
    /// SOLAR OS plays (auto or song mode) and the setting allows it
    pub(super) fn touches(&self) -> bool {
        (self.song.auto || self.arrangement.is_some()) && self.shared.touches.load(Ordering::Relaxed)
    }

    /// Solarum moves the energy: the auto mode with AUTO FX, or a song
    pub(super) fn steers_energy(&self) -> bool {
        (self.song.auto && self.song.auto_fx) || self.arrangement.is_some()
    }

    fn start_breakdown(&mut self, idx: u64) {
        let plan = self.plan;
        let before = self.song.tracks.map(|t| t.mute);
        for &v in plan.out {
            self.song.track_mut(v).mute = true;
        }
        self.breakdown = Some(Breakdown {
            start: idx,
            len: (self.cycle_len - 2).saturating_sub(self.breakdown_at()).max(1) * STEPS as u64,
            out: plan.out,
            before,
            sweep: plan.sweep,
        });
        self.send_song(Some(Change::Breakdown));
    }

    /// Bring back what the breakdown took out
    fn restore_breakdown(&mut self) {
        if let Some(b) = self.breakdown.take() {
            for &v in b.out {
                self.song.track_mut(v).mute = b.before[v.index()];
            }
        }
    }

    pub(super) fn end_breakdown(&mut self) {
        self.restore_breakdown();
        self.send_song(None);
    }

    fn drop_now(&mut self) {
        self.build = None;
        self.restore_breakdown();
        self.shared.build.store(0, Ordering::Relaxed);
        self.song.track_mut(Voice::Kick).mute = false;
        self.song.track_mut(Voice::Bass).mute = false;
        self.boom.trigger();
        self.crash = 1.0;
        // Energy turned down before the drop? It comes back with it
        let m = &mut self.song.macros;
        m.energy = m.energy.max(0.85);
        let mut change = Change::Drop;
        if self.song.auto {
            let peaked = self.heat >= 0.9;
            self.new_cycle(self.bar_now);
            // A new chapter only every few drops – the set keeps its identity for a while –
            // and most likely as the set breathes out after its peak
            let n = self.drops_since_chapter;
            if (n >= 2 && (peaked || self.auto_rng.chance(0.35))) || n >= 4 {
                // Tracks you're working on sit the new chapter out
                let locks = self.song.tracks.map(|t| t.lock);
                for (t, &until) in self.song.tracks.iter_mut().zip(&self.hands_off) {
                    t.lock |= until > self.bar_now;
                }
                change = gen::new_chapter(&mut self.song, &mut self.auto_rng, peaked);
                for (t, l) in self.song.tracks.iter_mut().zip(locks) {
                    t.lock = l;
                }
                self.drops_since_chapter = 0;
            } else {
                self.auto_change(true, 0.0);
                self.drops_since_chapter += 1;
            }
            // What the drop brought is the new cycle's theme
            self.home = self.song.tracks;
            if self.song.auto_fx {
                self.song.macros.energy = self.curve_target(0);
            }
        }
        // The drop lands at once; from here the curve glides on
        self.energy_goal = self.song.macros.energy;
        self.layer_energy = self.song.macros.energy;
        self.send_song(Some(change));
    }

    /// Put a pattern in place, keeping the live settings (volume, EQ, energy …); a running
    /// break keeps its tracks out
    pub(super) fn load_pattern(&mut self, mut s: Song, slot: Option<usize>) {
        s.master = self.song.master;
        s.auto = self.song.auto && self.arrangement.is_none();
        s.auto_bars = self.song.auto_bars;
        s.auto_fx = self.song.auto_fx;
        s.eq = self.song.eq;
        s.macros.energy = self.song.macros.energy;
        s.sanitize();
        if let Some(b) = &mut self.breakdown {
            b.before = s.tracks.map(|t| t.mute);
            for &v in b.out {
                s.track_mut(v).mute = true;
            }
        }
        self.song = s;
        self.pattern_now = slot;
    }

    /// Song mode at a bar start: the next section when one is over (or on a jump), the
    /// energy curve, a break's build-up, fills
    fn arrange_bar(&mut self, idx: u64, bar: u64, dropped: bool) {
        let Some(a) = &self.arrangement else { return };
        let form = &FORMS[a.form % FORMS.len()];
        let full = a.patterns.map(|p| p.is_some());
        let over = self.section_start == PENDING
            || self.jump.is_some()
            || bar >= self.section_start + form.sections[self.section.min(form.sections.len() - 1)].bars as u64;
        if over {
            let next = match self.jump.take() {
                Some(j) => j,
                None if self.section_start == PENDING => self.section,
                None => self.section + 1,
            };
            if next >= form.sections.len() {
                self.end_song();
            } else {
                self.enter_section(next, idx, bar, dropped);
            }
            return;
        }
        let sec = form.sections[self.section];
        let (played, n) = (bar - self.section_start, sec.bars as u64);
        let next = form.sections.get(self.section + 1).copied();
        let into_break = next.is_some_and(|nx| nx.part == Part::Break);
        self.energy_goal = sec.part.energy(played, n);
        // The last two bars before a break already ease off, so the break doesn't cut in
        if into_break && played + 2 >= n {
            self.energy_goal = self.energy_goal.min(0.6);
        }
        // Grooves and peaks keep moving: a gentle change every 8 bars
        if matches!(sec.part, Part::Groove | Part::Peak) && played > 0 && played.is_multiple_of(8) {
            if let Some(c) = gen::vary(&mut self.song, &mut self.auto_rng) {
                self.send_song(Some(c));
            }
        }
        if sec.part == Part::Break && played + 2 == n && self.build.is_none() {
            self.auto_build(idx);
        } else if played + 1 == n && sec.part != Part::Break {
            // A fill into a break, a clap roll into another favorite
            let other = next.is_some_and(|nx| pattern_slot(nx.pattern, full) != self.pattern_now);
            if into_break {
                self.fill = Some((bar, *self.auto_rng.pick(&FILLS)));
            } else if other {
                self.fill = Some((bar, Fill::Roll));
            }
        } else if matches!(sec.part, Part::Groove | Part::Peak)
            && (played + 1).is_multiple_of(8)
            && played + 1 < n
            && self.auto_rng.chance(0.7)
        {
            self.phrase_end(bar, (played + 1).is_multiple_of(16));
        }
    }

    fn enter_section(&mut self, i: usize, idx: u64, bar: u64, dropped: bool) {
        let Some(a) = &self.arrangement else { return };
        let sec = FORMS[a.form % FORMS.len()].sections[i];
        let slot = pattern_slot(sec.pattern, a.patterns.map(|p| p.is_some()));
        let pattern = slot.and_then(|k| a.patterns[k]).unwrap_or(a.fallback);
        let first = self.section_start == PENDING;
        // A song records itself – unless the whole session is being recorded anyway
        if first && self.events.is_some() && !self.capturing {
            self.finish_recording();
            self.recording = true;
            self.rec = Vec::with_capacity(REC_CHUNK + 4096);
        }
        // A jump out of a break: kick and bass come back, its build-up is off
        if self.breakdown.is_some() {
            self.restore_breakdown();
        }
        if !dropped && self.build.is_some() {
            self.build = None;
            self.shared.build.store(0, Ordering::Relaxed);
        }
        self.section = i;
        self.section_start = bar;
        let same = !first && slot == self.pattern_now;
        if !same {
            self.load_pattern(pattern, slot);
        }
        match sec.part {
            Part::Break => {
                let before = self.song.tracks.map(|t| t.mute);
                let out: &'static [Voice] = &[Voice::Kick, Voice::Bass];
                for &v in out {
                    self.song.track_mut(v).mute = true;
                }
                self.breakdown = Some(Breakdown {
                    start: idx,
                    len: sec.bars as u64 * STEPS as u64,
                    out,
                    before,
                    sweep: false,
                });
            }
            Part::Peak if !dropped => self.drop_now(),
            _ => {}
        }
        // After the drop with the same favorite: a new chapter of it, so it feels fresh
        if sec.part == Part::Peak && same {
            gen::new_chapter(&mut self.song, &mut self.auto_rng, false);
        }
        self.energy_goal = sec.part.energy(0, sec.bars as u64);
        if sec.part == Part::Peak {
            self.song.macros.energy = self.energy_goal;
            self.layer_energy = self.energy_goal;
        }
        self.shared.part.store(i as u32 + 1, Ordering::Relaxed);
        self.shared.part_start.store(bar, Ordering::Relaxed);
        self.send_song(Some(Change::Part(sec.part, i, slot)));
    }

    fn end_song(&mut self) {
        self.handle(Cmd::Stop);
        self.send_song(Some(Change::SongEnd));
    }

    /// Auto mode's energy: lands high on the drop and settles, climbs a step every phrase
    /// towards the breakdown, falls to the plan's depth and creeps up through it, then
    /// climbs through the build-up. The heat of the cycle sets how high it all goes.
    pub(super) fn curve_target(&self, b: u64) -> f32 {
        curve(b, self.cycle_len, self.breakdown_at(), self.plan.depth, self.heat_now(), self.drive)
    }

    /// The set's heat with your drive on top (chill cools it, turnup heats it)
    fn heat_now(&self) -> f32 {
        (self.heat + 0.3 * self.drive).clamp(0.0, 1.0)
    }

    /// Bar of the auto cycle where the breakdown starts – as far before its end as the plan says
    pub(super) fn breakdown_at(&self) -> u64 {
        self.cycle_len - (32 - self.plan.breakdown_at)
    }

    /// The plan has a breakdown (a roller doesn't)
    pub(super) fn has_breakdown(&self) -> bool {
        self.breakdown_at() < self.cycle_len - 2
    }

    /// A new auto cycle from `bar` on: the set's heat moves on (up a step, or down after
    /// the peak), and with it another kind of breakdown – never the same twice in a row –
    /// and the cycle's length: calm cycles groove longer, the peak comes quick
    pub(super) fn new_cycle(&mut self, bar: u64) {
        self.cycle_start = bar;
        // How the groove sounds now is the theme of this cycle
        self.home = self.song.tracks;
        let r = &mut self.auto_rng;
        self.heat = if self.heat >= 0.9 {
            0.3 + 0.2 * r.f32()
        } else {
            (self.heat + 0.15 + 0.2 * r.f32()).min(1.0)
        };
        let hot = (self.heat + 0.3 * self.drive).clamp(0.0, 1.0);
        let last = self.plan;
        let options = plans_for(hot);
        let n = options.iter().filter(|&&i| PLANS[i] != last).count() as u32;
        let i = options.iter().copied().filter(|&i| PLANS[i] != last).nth(r.below(n) as usize);
        self.plan = PLANS[i.unwrap_or(0)];
        let calm = matches!(self.song.vibe.rules(), Vibe::Hypnotic | Vibe::Minimal | Vibe::Dub | Vibe::Melodic);
        let lens: &[u64] = match (calm, hot) {
            (_, h) if h >= 0.9 => &[32],
            (true, h) if h < 0.5 => &[48, 64],
            (true, _) => &[32, 48, 64, 48],
            (false, h) if h < 0.5 => &[32, 48],
            (false, _) => &[32, 32, 48],
        };
        self.cycle_len = *r.pick(lens);
        self.publish_cycle();
    }

    /// Tell the UI where the auto cycle is going (for the log)
    pub(super) fn publish_cycle(&self) {
        let sh = &self.shared;
        sh.cycle_start.store(self.cycle_start, Ordering::Relaxed);
        sh.cycle_len.store(self.cycle_len as u32, Ordering::Relaxed);
        let brk = if self.has_breakdown() {
            self.breakdown_at() as u32
        } else {
            NO_BREAK
        };
        sh.cycle_break.store(brk, Ordering::Relaxed);
        sh.heat.store((self.heat_now() * 1000.0) as u32, Ordering::Relaxed);
        sh.cycle_depth.store((self.plan.depth * 1000.0) as u32, Ordering::Relaxed);
    }

    /// The auto mode's next change, picked by ear (`crate::ear`): `progress` is how far into
    /// the groove part of the cycle it lands
    fn auto_change(&mut self, big: bool, progress: f32) -> Option<Change> {
        let aim = crate::ear::Aim {
            big,
            mutes: self.song.auto_fx,
            progress,
            heat: self.heat_now(),
            hands_off: self.hands_off.map(|until| until > self.bar_now),
            push: self.drive,
        };
        let (change, why) = crate::ear::choose(&mut self.song, &mut self.auto_rng, &aim, &mut self.recent, &self.home, &self.taste)?;
        self.why = why;
        Some(change)
    }

    /// Tell the UI on which bar the next change lands
    fn publish_next_change(&self, bar: u64) {
        let every = self.song.auto_bars.max(1) as u64;
        let groove = if self.song.auto_fx { self.breakdown_at() } else { self.cycle_len };
        let b = bar.saturating_sub(self.cycle_start);
        let next = (b + 1).next_multiple_of(every);
        let at = if next < groove { self.cycle_start + next } else { u64::MAX };
        self.shared.next_change.store(at, Ordering::Relaxed);
    }

    /// Bar starts: drops, a waiting pattern, song mode, and in auto mode the arrangement
    pub(super) fn new_bar(&mut self, idx: u64) {
        let bar = idx / STEPS as u64;
        self.bar_now = bar;
        let mut dropped = false;
        if let Some(b) = &self.build {
            if idx >= b.start + b.len {
                self.drop_now();
                dropped = true;
            }
        }
        // A pattern picked during the bar comes in now
        if let Some((s, slot)) = self.queued.take() {
            self.load_pattern(*s, Some(slot));
            self.send_song(Some(Change::Scene(slot)));
        }
        if self.arrangement.is_some() {
            self.arrange_bar(idx, bar, dropped);
            return;
        }
        if dropped || !self.song.auto {
            return;
        }
        // Growing: one empty track comes in per bar, nothing else changes meanwhile
        if let Some(target) = self.grow.take() {
            const ORDER: [Voice; TRACKS] = [
                Voice::Kick,
                Voice::Hat,
                Voice::Clap,
                Voice::Bass,
                Voice::Rim,
                Voice::OpenHat,
                Voice::Acid,
                Voice::Ride,
            ];
            let next = ORDER.into_iter().find(|&v| {
                let t = self.song.track(v);
                !t.lock && t.hits() == 0 && target.track(v).hits() > 0
            });
            if let Some(v) = next {
                let src = *target.track(v);
                let t = self.song.track_mut(v);
                (t.steps, t.len, t.sound, t.mute) = (src.steps, src.len, src.sound, false);
                self.grow = Some(target);
                self.send_song(Some(Change::Unmuted(v)));
                return;
            }
        }
        if bar == 0 {
            return;
        }
        // A cycle of 32–64 bars: groove (a change every few bars, a fill every 8), breakdown,
        // 2 bars build-up, drop. Every drop – also one by hand – starts the next cycle.
        // Everything counts from the cycle's start, so changes land on phrases.
        if bar < self.cycle_start || bar - self.cycle_start >= self.cycle_len {
            self.new_cycle(bar);
        }
        let b = bar - self.cycle_start;
        let at = self.breakdown_at();
        // With AUTO FX on, Solarum arranges: energy, breakdown, build-up, drop. Without it the
        // energy bar is yours (low = break, 90 = drop) and the auto mode only changes patterns,
        // sounds and notes – it never takes a track out.
        let fx = self.song.auto_fx;
        if fx {
            self.energy_goal = self.curve_target(b);
            if bar.is_multiple_of(2) {
                self.new_knob_goals(if b < 8 { 1 } else { 2 });
            }
            if b == at && self.has_breakdown() && self.build.is_none() {
                self.start_breakdown(idx);
            } else if b == self.cycle_len - 2 && self.build.is_none() {
                self.auto_build(idx);
            }
        }
        // Without AUTO FX there's no breakdown: the groove changes all through the cycle
        let groove = if fx { at } else { self.cycle_len };
        let every = self.song.auto_bars.max(1) as u64;
        if b < groove && b.is_multiple_of(every) {
            // The big change opens a phrase; in between come small touches
            let big = b.is_multiple_of(8) || every >= 8;
            if let Some(change) = self.auto_change(big, b as f32 / groove.max(1) as f32) {
                self.send_song(Some(change));
            }
        }
        self.publish_next_change(bar);
        let next = b + 1;
        if fx && next == at && self.has_breakdown() {
            // The clap thrown into the echo leads into the breakdown
            self.fill = Some((bar, Fill::Throw));
            self.last_fill = Some(Fill::Throw);
        } else if next < groove && next.is_multiple_of(8) {
            let big = next.is_multiple_of(16);
            if self.auto_rng.chance(if big { 0.9 } else { 0.7 }) {
                self.phrase_end(bar, big);
            }
        }
    }
}

/// The auto mode's energy curve over a cycle of `len` bars with its breakdown at bar `at`
/// (`len - 2` or later: a roller without one) going down to `depth`, at the set's `heat` –
/// the engine steers by it, SOLAR OS draws it. `drive` (−1…1, the energy slider) lifts or
/// lowers all of it.
pub fn curve(b: u64, len: u64, at: u64, depth: f32, h: f32, drive: f32) -> f32 {
    (plan_curve(b, len, at, depth, h) + 0.25 * drive).clamp(0.05, 1.0)
}

fn plan_curve(b: u64, len: u64, at: u64, depth: f32, h: f32) -> f32 {
    let build = len.saturating_sub(2);
    let has_breakdown = at < build;
    let drop = 0.9 + 0.08 * h;
    let settle = 0.6 + 0.15 * h;
    let top = settle + 0.08 + 0.1 * h;
    if b < 8 {
        drop - (drop - settle) * b as f32 / 8.0
    } else if b < at {
        // Terraces: a new level on every phrase, the top in the last one
        let phrases = (at - 8).div_ceil(8).max(1);
        let k = ((b - 8) / 8 + 1).min(phrases);
        settle + (top - settle) * k as f32 / phrases as f32
    } else if b < build {
        let p = (b - at) as f32 / (build - at) as f32;
        depth + 0.15 * p * p
    } else {
        let from = if has_breakdown { (depth + 0.3).max(0.5) } else { top - 0.15 };
        from + (b - build) as f32 * 0.2
    }
}
