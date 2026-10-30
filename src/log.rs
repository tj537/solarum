//! The log under the mood: what Solarum says (every caption), where the auto mode is and when
//! the drop comes, and now and then a line from the ship – typed in like on an old terminal

use crate::engine::{Shared, NO_BREAK};
use crate::rng::Rng;
use crate::song::Song;
use crate::theme::*;
use crate::widgets::glow;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Ui};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;

/// Characters typed per second
const TYPE_SPEED: f64 = 48.0;
const KEEP: usize = 40;

/// Tips, one now and then while the music plays (in turn with what's going on in the set)
const TIPS: [&str; 14] = [
    "Tip: click a track's name to hear its sound",
    "Tip: drag a bass or synth planet up or down for another note",
    "Tip: right-click a planet – accent, slide, how often it plays",
    "Tip: lock a track and shuffle and the auto mode leave it alone",
    "Tip: B builds up for two bars and drops on the next",
    "Tip: hold WASH (E) through the last bar before a drop",
    "Tip: hold ECHO (W) on the last clap of a phrase",
    "Tip: R records everything you hear – save it as a WAV",
    "Tip: keep a groove in a favorite – it comes in on the next bar",
    "Tip: AUTO SONG sits in the arrow next to record",
    "Tip: scroll on a sound to flip through the sounds",
    "Tip: S solos a track – click again to bring the rest back",
    "Tip: double-click a knob to reset it",
    "Tip: the spring-back filter snaps home when you let go",
];

/// What the auto mode is doing, read from `Shared`
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Where {
    /// Bar within the cycle and its length
    pub bar: u64,
    pub len: u64,
    /// Bar of the breakdown in the cycle (None: a roller without one)
    pub brk: Option<u64>,
    pub heat: f32,
    /// How low the breakdown goes
    pub depth: f32,
    /// A change every this many bars (SET)
    pub every: u64,
    /// Bars until the next change lands (None: none planned before the drop)
    pub next: Option<u64>,
    /// How far the energy slider lifts (or lowers) the set, −1…1
    pub drive: f32,
}

impl Where {
    pub fn read(shared: &Shared, bar_now: u64, every: u8) -> Self {
        let start = shared.cycle_start.load(Ordering::Relaxed);
        let brk = shared.cycle_break.load(Ordering::Relaxed);
        let next = shared.next_change.load(Ordering::Relaxed);
        Where {
            depth: shared.cycle_depth.load(Ordering::Relaxed) as f32 / 1000.0,
            every: every.max(1) as u64,
            drive: f32::from_bits(shared.drive.load(Ordering::Relaxed)),
            next: (next != u64::MAX && next >= bar_now).then(|| next - bar_now),
            bar: bar_now.saturating_sub(start),
            len: shared.cycle_len.load(Ordering::Relaxed).max(1) as u64,
            brk: (brk != NO_BREAK).then_some(brk as u64),
            heat: shared.heat.load(Ordering::Relaxed) as f32 / 1000.0,
        }
    }

    /// Bars until the drop lands
    pub fn bars_to_drop(&self) -> u64 {
        self.len.saturating_sub(self.bar)
    }

    pub fn phase(&self) -> &'static str {
        if self.bar + 2 >= self.len {
            "BUILD-UP"
        } else if self.brk.is_some_and(|b| self.bar >= b) {
            "BREAKDOWN"
        } else if self.brk.is_none() {
            "ROLLER"
        } else {
            "GROOVE"
        }
    }

    /// The bar the breakdown (or, for a roller, the build-up) starts
    fn groove_end(&self) -> u64 {
        self.brk.unwrap_or(self.len.saturating_sub(2))
    }

    fn heat_word(&self) -> &'static str {
        match self.heat {
            h if h >= 0.9 => "peak",
            h if h >= 0.7 => "hot",
            h if h >= 0.5 => "warm",
            _ => "cool",
        }
    }
}

/// What plays, as the log's status line sees it
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    Stopped,
    /// A song (song mode) – it says what it does itself
    Song,
    /// The auto mode with AUTO FX: it arranges, so there's a drop to count down to
    Auto(Where),
    /// The auto mode without AUTO FX, or no auto mode: just the bar
    Bar(u64),
}

pub struct Log {
    /// Text and when it came
    lines: VecDeque<(String, f64)>,
    last_bar: Option<u64>,
    /// When the next fact or tip comes, and which kind it is (a fact about the set, the
    /// groove, or a tip – in turn)
    next_line: f64,
    kind: u32,
    last_tip: usize,
    /// Drops so far and seconds played, for the facts
    drops: u32,
    played: f64,
    last_tick: Option<f64>,
    /// Facts about the set and tips now and then (a setting)
    pub facts: bool,
    /// By hand: where the energy was (0 break, 1 groove, 2 everything) – a crossing gets a line
    zone: Option<u8>,
    rng: Rng,
}

impl Log {
    pub fn new() -> Self {
        Log {
            lines: VecDeque::new(),
            last_bar: None,
            next_line: 30.0,
            kind: 0,
            last_tip: usize::MAX,
            drops: 0,
            played: 0.0,
            last_tick: None,
            facts: true,
            zone: None,
            rng: Rng::new(1984),
        }
    }

    pub fn push(&mut self, text: impl Into<String>, now: f64) {
        let text = text.into();
        // The same line twice in a row within a few seconds is once
        if self.lines.back().is_some_and(|(t, at)| *t == text && now - at < 5.0) {
            return;
        }
        self.lines.push_back((text, now));
        while self.lines.len() > KEEP {
            self.lines.pop_front();
        }
    }

    #[cfg(test)]
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(|(t, _)| t.as_str())
    }

    /// A drop happened (for the facts)
    pub fn note_drop(&mut self) {
        self.drops += 1;
    }

    /// Once per frame: countdowns on new bars, and now and then a fact about the set or a tip
    pub fn tick(&mut self, state: State, now: f64, song: &Song) {
        let bar = match state {
            State::Auto(w) => Some(w.bar),
            State::Bar(b) => Some(b),
            _ => None,
        };
        if bar != self.last_bar {
            self.last_bar = bar;
            match state {
                State::Auto(w) => self.countdown(w, now),
                State::Bar(b) => self.phrase(b, now),
                _ => {}
            }
        }
        // By hand the energy decides what plays: say when it crosses into the break or to everything
        if let State::Bar(_) = state {
            let e = song.macros.energy;
            let zone = if e < 0.3 {
                0
            } else if e >= 0.85 {
                2
            } else {
                1
            };
            if self.zone.is_some_and(|z| z != zone) {
                self.push(
                    match zone {
                        0 => "Break: kick and bass are out – bring the energy back to drop them in",
                        2 => "Energy 85+: everything plays – 90 starts the drop",
                        _ => "Groove: tracks come in as the energy rises",
                    },
                    now,
                );
            }
            self.zone = Some(zone);
        } else {
            self.zone = None;
        }
        let dt = self.last_tick.map_or(0.0, |t| (now - t).clamp(0.0, 1.0));
        self.last_tick = Some(now);
        if state == State::Stopped {
            self.next_line = self.next_line.max(now + 20.0);
            return;
        }
        self.played += dt;
        if now >= self.next_line && self.facts {
            self.next_line = now + 30.0 + self.rng.f32() as f64 * 20.0;
            let line = self.next_fact(song);
            self.push(line, now);
        }
    }

    fn next_fact(&mut self, song: &Song) -> String {
        self.kind += 1;
        match self.kind % 3 {
            0 => {
                let mins = (self.played / 60.0) as u32;
                let time = if mins == 0 {
                    "Under a minute in".to_string()
                } else {
                    format!("{mins} min in")
                };
                match self.drops {
                    0 => format!("{time} · no drop yet"),
                    1 => format!("{time} · 1 drop"),
                    n => format!("{time} · {n} drops"),
                }
            }
            1 => {
                let mood = song.mood().map_or("", |k| crate::song::MOODS[k].0);
                format!("{} · {} · {mood} · {:.0} BPM", song.vibe.name(), song.root_name(), song.bpm)
            }
            _ => {
                let mut i = self.rng.below(TIPS.len() as u32) as usize;
                if i == self.last_tip {
                    i = (i + 1) % TIPS.len();
                }
                self.last_tip = i;
                TIPS[i].to_string()
            }
        }
    }

    /// By hand: when to press DROP so it lands on the next phrase, and phrase ends
    fn phrase(&mut self, bar: u64, now: f64) {
        match bar % 32 {
            // B starts two bars of build-up on the next bar: pressed in the sixth bar of a
            // phrase, the drop lands on the one of the next
            13 | 29 => self.push("Press B now – the drop lands on the next phrase", now),
            31 => self.push("Phrase end – a good moment to switch a favorite or roll a track", now),
            _ => {}
        }
    }

    fn countdown(&mut self, w: Where, now: f64) {
        if w.bar == 0 {
            self.push(format!("New cycle · {} bars · the set runs {}", w.len, w.heat_word()), now);
            if w.brk.is_none() {
                self.push("No breakdown this time – it just rolls on", now);
            }
        }
        if let Some(b) = w.brk.filter(|&b| b > w.bar) {
            let n = b - w.bar;
            if n == 8 || n == 4 {
                self.push(format!("Breakdown in {n} bars"), now);
            }
        }
        let n = w.bars_to_drop();
        if matches!(n, 16 | 8) && w.brk.is_none_or(|b| w.bar >= b || n == 16) {
            self.push(format!("Drop in {n} bars"), now);
        }
    }

    /// The log: a status line, then the lines, the newest at the bottom being typed. `top` and
    /// `bottom` keep room for what the computer draws there (its menu bar, the prompt).
    #[allow(clippy::too_many_arguments)]
    pub fn ui(&self, ui: &mut Ui, state: State, energy: Option<f32>, height: f32, top: f32, bottom: f32, now: f64) -> Rect {
        let w = ui.available_width();
        let h = height.max(60.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let p = ui.painter_at(rect);
        // A little darker than the concrete around it, with a faint picture noise
        p.rect_filled(rect, 10.0, Color32::from_black_alpha(70));
        p.rect_stroke(rect, 10.0, stroke(1.0, mix(PANEL, ACID, 0.25)), egui::StrokeKind::Inside);
        let frame = (now * 14.0) as u32;
        for i in 0..(rect.area() / 90.0) as u32 {
            let h = |k: u32| {
                let mut x = i.wrapping_mul(0x9E37_79B9) ^ frame.wrapping_mul(0x85EB_CA6B) ^ k.wrapping_mul(0xC2B2_AE35);
                x ^= x >> 15;
                x = x.wrapping_mul(0x2C1B_3C6D);
                x ^= x >> 12;
                (x & 0xFFFF) as f32 / 65535.0
            };
            let at = pos2(
                rect.left() + 2.0 + h(1) * (rect.width() - 4.0),
                rect.top() + 2.0 + h(2) * (rect.height() - 4.0),
            );
            // Exact premultiplied greys: a speck of light, never a wash
            let v = (4.0 + h(3) * 10.0) as u8;
            p.rect_filled(
                Rect::from_min_size(at, vec2(1.5, 1.5)),
                0.0,
                Color32::from_rgba_premultiplied(v, v, v, v),
            );
        }
        let mono = |size: f32| FontId::monospace(size);
        let inner = Rect::from_min_max(rect.min + vec2(10.0, 8.0 + top), rect.max - vec2(10.0, 8.0 + bottom));

        // Status line
        let status = match state {
            State::Stopped => "STOPPED".to_string(),
            State::Song => "SONG".to_string(),
            State::Auto(w) => format!("{} · {}", w.phase(), w.heat_word().to_uppercase()),
            State::Bar(b) => format!("BAR {} · PHRASE {}", b + 1, b / 8 + 1),
        };
        p.text(inner.left_top(), Align2::LEFT_TOP, &status, mono(11.0), ACID);
        let mut top = inner.top() + 18.0;
        if let State::Auto(w) = state {
            let drop = match w.bars_to_drop() {
                0 | 1 => "DROP NOW".to_string(),
                n => format!("DROP IN {n}"),
            };
            p.text(inner.right_top(), Align2::RIGHT_TOP, drop, mono(11.0), ACID);
            // Room under DROP IN for the drop's marker
            let map = Rect::from_min_size(pos2(inner.left(), top + 8.0), vec2(inner.width(), 24.0));
            set_map(&p, map, w, energy.unwrap_or(0.0), now);
            top += 42.0;
            let next = match w.next {
                Some(n) => format!("CHANGE IN {}", n.max(1)),
                None => String::new(),
            };
            // How far the energy slider moved the set
            let lift = match w.drive {
                d if d > 0.05 => format!("TURNED UP +{:.0}", d * 100.0),
                d if d < -0.05 => format!("CHILLED {:.0}", d * 100.0),
                _ => String::new(),
            };
            p.text(pos2(inner.left(), top), Align2::LEFT_TOP, lift, mono(10.0), alpha(ACID, 0.7));
            p.text(pos2(inner.right(), top), Align2::RIGHT_TOP, next, mono(10.0), alpha(ACID, 0.7));
            top += 14.0;
        }
        // By hand: the bar within the 8-bar phrase, so drops and changes land on the one
        if let State::Bar(b) = state {
            p.text(pos2(inner.left(), top), Align2::LEFT_TOP, "PHRASE", mono(10.0), alpha(ACID, 0.7));
            p.text(
                pos2(inner.right(), top),
                Align2::RIGHT_TOP,
                format!("{}/8", b % 8 + 1),
                mono(10.0),
                ACID,
            );
            let (x0, x1) = (inner.left() + 50.0, inner.right() - 24.0);
            let cw = (x1 - x0) / 8.0;
            for i in 0..8 {
                let r = Rect::from_min_size(pos2(x0 + i as f32 * cw, top + 2.0), vec2(cw - 3.0, 8.0));
                let c = if i as u64 == b % 8 {
                    ACID
                } else if (i as u64) < b % 8 {
                    alpha(ACID, 0.45)
                } else {
                    alpha(ACID, 0.12)
                };
                p.rect_filled(r, 1.0, c);
            }
            top += 16.0;
        }
        // Some room above and below the line
        top += 4.0;
        p.line_segment(
            [pos2(inner.left(), top - 4.0), pos2(inner.right(), top - 4.0)],
            stroke(1.0, alpha(ACID, 0.25)),
        );
        top += 4.0;

        // The lines, from the newest up – clipped below the status line, fading out before it
        let p = p.with_clip_rect(Rect::from_min_max(pos2(rect.left(), top), rect.max));
        let font = mono(10.5);
        let mut bottom = inner.bottom();
        for (k, (text, at)) in self.lines.iter().rev().enumerate() {
            if bottom <= top {
                break;
            }
            let newest = k == 0;
            let typed = if newest {
                let chars = ((now - at) * TYPE_SPEED).max(0.0) as usize;
                text.chars().take(chars).collect::<String>()
            } else {
                text.clone()
            };
            let fade = if newest { 1.0 } else { (0.75 - 0.08 * k as f32).max(0.25) };
            let galley = p.layout(format!("> {typed}"), font.clone(), ACID, inner.width() - 8.0);
            let gh = galley.size().y;
            let pos = pos2(inner.left(), bottom - gh);
            let edge = ((pos.y - top) / 30.0).clamp(0.0, 1.0);
            // A blinking block where the typing is
            if newest && (now * 0.8).fract() < 0.55 {
                let end = galley.rows.last().map_or(pos, |r| pos + vec2(r.rect.right() + 2.0, r.rect.top()));
                p.rect_filled(Rect::from_min_size(end, vec2(6.0, 11.0)), 0.0, alpha(ACID, 0.8));
            }
            p.galley_with_override_text_color(pos, galley, alpha(ACID, fade * edge));
            bottom -= gh + 4.0;
        }
        rect
    }
}

/// The map of the cycle: the energy the auto mode plans for every bar as a line with a soft
/// area under it (the played part bright), dashed lines where the breakdown and the build-up
/// start, the drop at the end, dots where the changes land and the bar playing now
fn set_map(p: &egui::Painter, r: Rect, w: Where, energy: f32, now: f64) {
    let n = w.len.max(1);
    let x = |b: f32| r.left() + r.width() * b / n as f32;
    let groove = w.groove_end();
    let build = n.saturating_sub(2);
    let at = w.brk.unwrap_or(build);
    let y = |e: f32| r.bottom() - (r.height() - 2.0) * e.clamp(0.04, 1.0);
    let plan = |b: u64| crate::engine::curve(b, n, at, w.depth, w.heat, w.drive);
    // Phrases: faint lines every 8 bars
    for k in (8..n).step_by(8) {
        let kx = x(k as f32);
        p.line_segment([pos2(kx, r.top()), pos2(kx, r.bottom())], stroke(1.0, alpha(ACID, 0.07)));
    }
    // The area under the plan, bar by bar
    // (bars at the same level and on the same side of now are one piece – no seams)
    let mut b = 0;
    while b < n {
        let (level, past) = (plan(b), b <= w.bar);
        let mut end = b + 1;
        while end < n && (plan(end) - level).abs() < 1e-4 && (end <= w.bar) == past {
            end += 1;
        }
        let col = Rect::from_min_max(pos2(x(b as f32), y(level)), pos2(x(end as f32), r.bottom()));
        p.rect_filled(col, 0.0, alpha(ACID, if past { 0.28 } else { 0.08 }));
        b = end;
    }
    // The line through it: played bright, ahead dimmer
    let point = |b: u64| pos2(x(b as f32 + 0.5), y(plan(b)));
    let split = w.bar.min(n - 1);
    let past: Vec<Pos2> = (0..=split).map(point).collect();
    let ahead: Vec<Pos2> = (split..n).map(point).collect();
    p.add(egui::Shape::line(ahead, stroke(1.5, alpha(ACID, 0.5))));
    p.add(egui::Shape::line(past, stroke(1.5, ACID)));
    // Where the breakdown and the build-up start, and their names when there's room
    let mark = |b: u64, label: &str| {
        let bx = x(b as f32);
        p.extend(egui::Shape::dashed_line(
            &[pos2(bx, r.top()), pos2(bx, r.bottom())],
            stroke(1.0, alpha(ACID, 0.45)),
            2.0,
            2.0,
        ));
        let room = x(if b == build { n as f32 } else { build as f32 }) - bx;
        if room > 30.0 {
            p.text(
                pos2(bx + 3.0, r.top()),
                Align2::LEFT_TOP,
                label,
                FontId::monospace(8.0),
                alpha(ACID, 0.6),
            );
        }
    };
    if w.brk.is_some() {
        mark(at, "BREAK");
    }
    mark(build, "BUILD");
    // Now: a bright line and where the energy really is
    let px = x(w.bar as f32 + 0.5);
    p.line_segment([pos2(px, r.top() - 2.0), pos2(px, r.bottom() + 1.0)], stroke(1.5, ACID));
    let dot = pos2(px, r.bottom() - (r.height() - 2.0) * energy.clamp(0.0, 1.0));
    glow(p, dot, 3.0, ACID, 0.4 + 0.2 * (now * 3.0).sin().abs() as f32);
    p.circle_filled(dot, 2.2, Color32::WHITE);
    // The drop at the end
    let tip = pos2(r.right() - 3.5, r.top() - 1.0);
    p.add(egui::Shape::convex_polygon(
        vec![tip + vec2(-3.5, -5.0), tip + vec2(3.5, -5.0), tip],
        ACID,
        egui::Stroke::NONE,
    ));
    // Where the changes land: small dots under the map, the next one big
    let y = r.bottom() + 5.0;
    let next = w.next.map(|k| w.bar + k);
    let mut b = 0;
    while b < groove {
        let c = pos2(x(b as f32 + 0.5), y);
        if Some(b) == next {
            p.circle_filled(c, 2.2, ACID);
        } else {
            p.circle_filled(c, 1.0, alpha(ACID, if b <= w.bar { 0.25 } else { 0.5 }));
        }
        b += w.every;
    }
}

/// The greetings Luna, the computer's mascot, types when it boots
pub const HELLOS: [&str; 4] = [
    "Yo bro, Luna here. I got this.",
    "What's good, brother? Let me cook.",
    "Ayo, Luna online. We outside tonight.",
    "Sit back, fam. I'm driving.",
];

/// What Luna drops into the log now and then while it plays
pub const DROPS: [&str; 16] = [
    "This groove is bussin, no cap.",
    "Lowkey this bass hits different, bro.",
    "Brother, the kick is cooking right now.",
    "Fam, are you even dancing?",
    "On god, this one's a vibe.",
    "Big up the hi-hats. They work hard.",
    "Next drop gonna be crazy, trust.",
    "Bro said techno. I heard techno.",
    "Real ones stay till the last track.",
    "Deadass, I could loop this all night.",
    "That synth? Straight from the mothership.",
    "Hydrate, bro. The rave needs you.",
    "Say less. The bassline speaks.",
    "Ayo, who let the acid out?",
    "My guy, the neighbours are nodding too.",
    "Main character energy on this one, fr.",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn at(bar: u64, brk: Option<u64>) -> State {
        State::Auto(Where {
            bar,
            len: 32,
            brk,
            heat: 0.6,
            depth: 0.2,
            every: 4,
            next: None,
            drive: 0.0,
        })
    }

    #[test]
    fn the_log_counts_down_to_breakdown_and_drop() {
        let mut log = Log::new();
        for bar in 0..32 {
            log.tick(at(bar, Some(24)), bar as f64, &Song::default());
        }
        let lines: Vec<&str> = log.lines().collect();
        assert!(lines[0].starts_with("New cycle · 32 bars"), "{lines:?}");
        for want in ["Drop in 16 bars", "Breakdown in 8 bars", "Breakdown in 4 bars", "Drop in 8 bars"] {
            assert!(lines.contains(&want), "{want}: {lines:?}");
        }
        // Each said once, in time order
        assert_eq!(lines.iter().filter(|l| l.starts_with("Drop in")).count(), 2);
    }

    #[test]
    fn by_hand_the_log_times_the_drop_and_names_the_energy() {
        let mut song = Song::default();
        let mut log = Log::new();
        for bar in 0..32 {
            log.tick(State::Bar(bar), bar as f64, &song);
        }
        assert_eq!(log.lines().filter(|l| l.starts_with("Press B now")).count(), 2);
        song.macros.energy = 0.2;
        log.tick(State::Bar(32), 40.0, &song);
        assert!(log.lines().last().is_some_and(|l| l.starts_with("Break")));
    }

    #[test]
    fn a_roller_says_so_and_facts_and_tips_come_in_turn() {
        let song = Song::default();
        let mut log = Log::new();
        log.tick(at(0, None), 0.0, &song);
        assert!(log.lines().any(|l| l.contains("No breakdown")));
        // Stopped: nothing comes; playing: a line every half minute or so, in turn
        log.tick(State::Stopped, 100.0, &song);
        let n = log.lines().count();
        log.tick(State::Bar(3), 119.0, &song);
        assert_eq!(log.lines().count(), n);
        log.note_drop();
        for t in 0..300 {
            log.tick(State::Bar(3), 120.0 + t as f64, &song);
        }
        let lines: Vec<&str> = log.lines().skip(n).collect();
        assert!((6..=11).contains(&lines.len()), "{lines:?}");
        assert!(lines.iter().any(|l| l.ends_with("1 drop")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("BPM")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("Tip:")), "{lines:?}");
    }
}
