//! Luna, the little robot that plays the sets: how far it has come with you (stardust and
//! levels – earned by playing, breeding and keeping sounds) and how it looks (antenna,
//! colour, an extra – each unlocked at a level). No shop, no money: only progress.
//!
//! The robot is drawn here too, in its skin, wherever it shows: circling the sun on the start
//! screen, on the SOLAR OS screen, with the sounds of the day, in the settings.

use crate::theme::*;
use crate::widgets::glow;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Shape, Stroke};
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};

/// The stardust each level starts at (level 1 … 10)
pub const LEVELS: [u32; 10] = [0, 30, 80, 160, 280, 450, 700, 1000, 1400, 2000];

/// Seconds of playing for one stardust – for the first `PLAY_FULL` of a day, then `PLAY_SLOW`:
/// a set left running all night shouldn't max Luna out
pub const PLAY_SECONDS: f32 = 20.0;
pub const PLAY_FULL: u32 = 180;
pub const PLAY_SLOW: f32 = 80.0;
/// Stardust for picking a bred sound, and for keeping or saving one – at most `*_DAY` a day,
/// so clicking through the rounds doesn't either
pub const DUST_PICK: u32 = 2;
pub const DUST_KEEP: u32 = 5;
pub const PICK_DAY: u32 = 40;
pub const KEEP_DAY: u32 = 60;

/// Where stardust comes from
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Play,
    Pick,
    Keep,
}

/// Stardust by where it came from
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    #[serde(default)]
    pub play: u32,
    #[serde(default)]
    pub pick: u32,
    #[serde(default)]
    pub keep: u32,
}

impl Tally {
    fn of(&mut self, src: Source) -> &mut u32 {
        match src {
            Source::Play => &mut self.play,
            Source::Pick => &mut self.pick,
            Source::Keep => &mut self.keep,
        }
    }
}

/// Today, as days since 1970 (UTC) – the daily limits start over with it
pub fn today() -> u32 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        / 86_400) as u32
}

/// The antennas: name and the level that unlocks it
pub const ANTENNAS: [(&str, usize); 5] = [("Classic", 1), ("Double", 3), ("Star", 6), ("Halo", 7), ("Dish", 9)];
/// The colours: name, level, colour
pub const COLORS: [(&str, usize, Color32); 5] = [
    ("Lime", 1, ACID),
    ("Ice", 2, Color32::from_rgb(110, 220, 255)),
    ("Bubblegum", 5, Color32::from_rgb(255, 120, 200)),
    ("Gold", 8, Color32::from_rgb(255, 205, 80)),
    ("Plasma", 10, Color32::from_rgb(190, 140, 255)),
];
/// The extras: name and level (0 = none)
pub const EXTRAS: [(&str, usize); 4] = [("None", 1), ("Shades", 4), ("Wings", 7), ("Devil horns", 10)];

/// New lines Luna drops into the log from a level on
const LINES: [(usize, &str); 12] = [
    (2, "Level two, fam. I can feel the bass in my circuits now."),
    (3, "Two antennas, double the vibes. Science."),
    (4, "Shades on. The future's bright, bro."),
    (4, "Can't see a thing in these. Don't care. Looking fresh."),
    (5, "Halfway to the stars, brother. Keep cooking."),
    (6, "I literally have a star on my head. Main character."),
    (7, "Halo and wings. Angel of the dancefloor, fam."),
    (7, "Real talk, you made me who I am. Respect."),
    (8, "Gold edition Luna. Limited drop. Only one exists."),
    (9, "Satellite dish online. Picking up raves from three galaxies."),
    (10, "Devil horns, level ten. Angel wings, rave demon. Balance."),
    (10, "We did it, fam. From a little robot to a legend."),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Luna {
    #[serde(default)]
    pub dust: u32,
    /// The skin: an index into `ANTENNAS`, `COLORS`, `EXTRAS`
    #[serde(default)]
    pub antenna: u8,
    #[serde(default)]
    pub color: u8,
    #[serde(default)]
    pub extra: u8,
    /// Seconds played that aren't stardust yet
    #[serde(default)]
    pub(crate) played: f32,
    /// All the stardust by where it came from, and today's (for the daily limits)
    #[serde(default)]
    pub total: Tally,
    #[serde(default)]
    pub today: Tally,
    #[serde(default)]
    pub day: u32,
    /// Everything unlocked, to try the skins (`SOLARUM_LUNA_ALL=1`) – never saved
    #[serde(skip)]
    pub preview: bool,
}

impl Luna {
    /// 1 … 10
    pub fn level(&self) -> usize {
        if self.preview {
            return LEVELS.len();
        }
        LEVELS.iter().filter(|&&l| self.dust >= l).count()
    }

    /// How far into this level (0 … 1), and the stardust the next one needs (None at the top)
    pub fn progress(&self) -> (f32, Option<u32>) {
        let lv = self.level();
        match LEVELS.get(lv) {
            Some(&next) => {
                let from = LEVELS[lv - 1];
                ((self.dust - from) as f32 / (next - from) as f32, Some(next))
            }
            None => (1.0, None),
        }
    }

    /// Add stardust from `src`; the new level if that was a level up
    fn add(&mut self, src: Source, dust: u32) -> Option<usize> {
        let before = self.level();
        self.dust = self.dust.saturating_add(dust);
        *self.total.of(src) += dust;
        *self.today.of(src) += dust;
        (self.level() > before).then(|| self.level())
    }

    /// A new day: today's limits start over
    fn roll(&mut self, day: u32) {
        if self.day != day {
            self.day = day;
            self.today = Tally::default();
        }
    }

    /// The stardust a pick or a keep would bring on `day` (0 once the day's limit is reached)
    pub fn would_earn(&self, src: Source, day: u32) -> u32 {
        let used = if self.day == day { self.today } else { Tally::default() };
        match src {
            Source::Pick => DUST_PICK.min(PICK_DAY.saturating_sub(used.pick)),
            Source::Keep => DUST_KEEP.min(KEEP_DAY.saturating_sub(used.keep)),
            Source::Play => 1,
        }
    }

    /// A bred sound picked or a sound kept on `day`: the stardust it brought, and the new
    /// level if that was a level up
    pub fn earn(&mut self, src: Source, day: u32) -> (u32, Option<usize>) {
        let n = self.would_earn(src, day);
        self.roll(day);
        if n == 0 {
            return (0, None);
        }
        (n, self.add(src, n))
    }

    /// `dt` seconds of playing on `day`
    pub fn play(&mut self, dt: f32, day: u32) -> Option<usize> {
        self.roll(day);
        self.played += dt.clamp(0.0, 1.0);
        let per = if self.today.play < PLAY_FULL { PLAY_SECONDS } else { PLAY_SLOW };
        if self.played < per {
            return None;
        }
        self.played -= per;
        self.add(Source::Play, 1)
    }

    /// Today's stardust by where it came from (nothing yet if the last earning was another day)
    pub fn today_tally(&self, day: u32) -> Tally {
        if self.day == day {
            self.today
        } else {
            Tally::default()
        }
    }

    /// The same progress and skin – what's worth saving (not the seconds in between)
    pub fn same(&self, o: &Luna) -> bool {
        Luna { played: 0.0, ..*self } == Luna { played: 0.0, ..*o }
    }

    /// Where Luna lives: its own little file next to the settings, so a reset of those
    /// doesn't take it
    pub fn path() -> Option<std::path::PathBuf> {
        eframe::storage_dir("Solarum").map(|d| d.join("luna.ron"))
    }

    /// Read it. A file that can't be read is moved aside (`luna.broken.ron`) instead of
    /// being written over, so nothing is lost for good.
    pub fn load(path: &std::path::Path) -> Option<Luna> {
        let text = std::fs::read_to_string(path).ok()?;
        match ron::from_str::<Luna>(&text) {
            Ok(l) => Some(l),
            Err(_) => {
                let _ = std::fs::rename(path, path.with_extension("broken.ron"));
                None
            }
        }
    }

    /// Write it (atomically – a crash mid-write leaves the old file)
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = ron::ser::to_string_pretty(&Luna { played: 0.0, ..*self }, Default::default()).map_err(std::io::Error::other)?;
        crate::file::write_atomic(path, text.as_bytes())
    }

    /// From its file, or the copy in the settings (where it lived first) – whichever came
    /// further
    pub fn merge(file: Option<Luna>, settings: Option<Luna>) -> Luna {
        let mut l = match (file, settings) {
            (Some(f), Some(s)) if s.dust > f.dust => s,
            (Some(f), _) => f,
            (None, s) => s.unwrap_or_default(),
        };
        l.sanitize();
        l
    }

    pub fn ink(&self) -> Color32 {
        COLORS[(self.color as usize).min(COLORS.len() - 1)].2
    }

    /// Wear only what's unlocked (saved settings may say more, after a reset of the dust)
    pub fn sanitize(&mut self) {
        let lv = self.level();
        if ANTENNAS.get(self.antenna as usize).is_none_or(|a| a.1 > lv) {
            self.antenna = 0;
        }
        if COLORS.get(self.color as usize).is_none_or(|c| c.1 > lv) {
            self.color = 0;
        }
        if EXTRAS.get(self.extra as usize).is_none_or(|e| e.1 > lv) {
            self.extra = 0;
        }
    }

    /// The lines Luna can drop while it plays: the old ones and every one unlocked since
    pub fn lines(&self) -> Vec<&'static str> {
        let lv = self.level();
        crate::log::DROPS
            .iter()
            .copied()
            .chain(LINES.iter().filter(|l| l.0 <= lv).map(|l| l.1))
            .collect()
    }

    /// What a level brought, in words ("Luna got shades")
    pub fn unlocked(level: usize) -> String {
        let mut got: Vec<String> = Vec::new();
        got.extend(
            ANTENNAS
                .iter()
                .filter(|a| a.1 == level)
                .map(|a| format!("the {} antenna", a.0.to_lowercase())),
        );
        got.extend(
            COLORS
                .iter()
                .filter(|c| c.1 == level)
                .map(|c| format!("the colour {}", c.0.to_lowercase())),
        );
        got.extend(EXTRAS.iter().filter(|e| e.1 == level).map(|e| e.0.to_lowercase()));
        if got.is_empty() {
            "new lines to drop".to_owned()
        } else {
            got.join(" and ")
        }
    }

    /// Luna in its skin. `fade` dims it (the SOLAR OS screen coming on), `wave` lifts the
    /// right arm (0: it hangs, 1: a full wave); `t` is seconds since it appeared.
    pub fn draw(&self, p: &egui::Painter, c: Pos2, s: f32, t: f32, fade: f32, wave: f32) {
        let ink = self.ink().gamma_multiply(fade.clamp(0.0, 1.0));
        let line = stroke(2.0, ink);
        // Body and head
        let body = Rect::from_center_size(c + vec2(0.0, 0.62 * s), vec2(0.56 * s, 0.34 * s));
        if self.extra == 2 {
            // two wings from behind the shoulders: a curved top edge, three scallops below,
            // beating slowly
            let flap = (t * 3.0).sin() * 0.18 + 0.05;
            let quad = |a: egui::Vec2, ctrl: egui::Vec2, b: egui::Vec2, out: &mut Vec<egui::Vec2>| {
                for i in 1..=8 {
                    let u = i as f32 / 8.0;
                    out.push(a * (1.0 - u) * (1.0 - u) + ctrl * 2.0 * u * (1.0 - u) + b * u * u);
                }
            };
            for side in [-1.0f32, 1.0] {
                let base = pos2(c.x + side * 0.3 * s, body.top() + 0.04 * s);
                let tip = vec2(0.8, -0.46);
                let mut pts = vec![vec2(0.0, 0.0)];
                quad(vec2(0.0, 0.0), vec2(0.3, -0.5), tip, &mut pts);
                let stops = [tip, vec2(0.62, -0.14), vec2(0.42, -0.02), vec2(0.2, 0.06), vec2(0.0, 0.0)];
                for w in stops.windows(2) {
                    let mid = (w[0] + w[1]) * 0.5 + vec2(0.04, 0.09);
                    quad(w[0], mid, w[1], &mut pts);
                }
                // the last scallop ends where the edge began: once is enough for the outline
                pts.pop();
                let (sn, cs) = flap.sin_cos();
                let pts: Vec<Pos2> = pts
                    .iter()
                    .map(|v| {
                        let r = vec2(v.x * cs + v.y * sn, -v.x * sn + v.y * cs);
                        base + vec2(side * r.x, r.y) * s
                    })
                    .collect();
                // two feather lines inside, from the shoulder to the scallops
                let at = |v: egui::Vec2| {
                    let r = vec2(v.x * cs + v.y * sn, -v.x * sn + v.y * cs);
                    base + vec2(side * r.x, r.y) * s
                };
                for v in [vec2(0.62, -0.14), vec2(0.42, -0.02)] {
                    p.line_segment([at(vec2(0.06, -0.04)), at(v * 0.92)], stroke(1.2, ink.gamma_multiply(0.6)));
                }
                p.add(Shape::closed_line(pts, stroke(2.0, ink)));
            }
        }
        p.rect_stroke(body, 0.08 * s, line, egui::StrokeKind::Middle);
        p.circle_filled(body.center(), 0.05 * s, ink.gamma_multiply(0.5 + 0.5 * (t * 4.0).sin().abs()));
        p.line_segment([c + vec2(0.0, 0.39 * s), body.center_top()], line);
        let head = Rect::from_center_size(c, vec2(s, 0.78 * s));
        p.rect_stroke(head, 0.16 * s, line, egui::StrokeKind::Middle);
        // Ears
        for side in [-1.0, 1.0] {
            let ear = Rect::from_center_size(c + vec2(side * 0.56 * s, 0.0), vec2(0.1 * s, 0.26 * s));
            p.rect_filled(ear, 0.03 * s, ink.gamma_multiply(0.7));
        }
        self.draw_antenna(p, head, s, t, ink);
        if self.extra == 3 {
            // devil horns on the corners of the head: thick at the root, curving out and then
            // back in to a glowing point
            let top = head.top() + 0.02 * s;
            for side in [-1.0f32, 1.0] {
                let x0 = c.x + side * 0.3 * s;
                let (a, ctrl, b) = (
                    pos2(x0, top),
                    pos2(x0 + side * 0.26 * s, top - 0.12 * s),
                    pos2(x0 + side * 0.1 * s, top - 0.36 * s),
                );
                let at = |u: f32| {
                    pos2(
                        a.x * (1.0 - u) * (1.0 - u) + ctrl.x * 2.0 * u * (1.0 - u) + b.x * u * u,
                        a.y * (1.0 - u) * (1.0 - u) + ctrl.y * 2.0 * u * (1.0 - u) + b.y * u * u,
                    )
                };
                for i in 0..14 {
                    let (u0, u1) = (i as f32 / 14.0, (i + 1) as f32 / 14.0);
                    let w = (0.16 - 0.14 * u0) * s;
                    p.line_segment([at(u0), at(u1)], stroke(w, ink));
                    p.circle_filled(at(u1), w / 2.0, ink);
                }
                glow(p, b, 0.07 * s, ink, 0.6);
            }
        }
        // Eyes: blink now and then – or shades
        if self.extra == 1 {
            for side in [-1.0, 1.0] {
                let lens = Rect::from_center_size(c + vec2(side * 0.21 * s, -0.08 * s), vec2(0.3 * s, 0.2 * s));
                p.rect_filled(lens, 0.06 * s, ink);
                let shine = lens.left_top() + vec2(0.06 * s, 0.05 * s);
                p.line_segment([shine, shine + vec2(0.08 * s, 0.0)], stroke(1.5, Color32::from_white_alpha(170)));
            }
            p.line_segment([c + vec2(-0.06 * s, -0.1 * s), c + vec2(0.06 * s, -0.1 * s)], line);
        } else {
            let blink = if (t * 0.7).fract() > 0.92 { 0.15 } else { 1.0 };
            for side in [-1.0, 1.0] {
                let eye = Rect::from_center_size(c + vec2(side * 0.21 * s, -0.08 * s), vec2(0.15 * s, 0.18 * s * blink));
                p.rect_filled(eye, 0.04 * s, ink);
            }
        }
        // A smile
        let smile: Vec<Pos2> = (0..=12)
            .map(|i| {
                let a = PI * (0.15 + 0.7 * i as f32 / 12.0);
                c + vec2(0.0, 0.08 * s) + vec2(a.cos(), a.sin()) * 0.2 * s
            })
            .collect();
        p.add(Shape::line(smile, line));
        // The right arm waves, the left one hangs
        let shoulder_r = body.right_top() + vec2(0.0, 0.06 * s);
        let a = -1.0 - 0.45 * (t * 7.0).sin();
        let up = shoulder_r + vec2(a.cos().abs(), a.sin()) * 0.38 * s;
        let down = shoulder_r + vec2(0.2 * s, 0.26 * s);
        let hand_r = down + (up - down) * wave.clamp(0.0, 1.0);
        p.line_segment([shoulder_r, hand_r], line);
        p.circle_stroke(hand_r, 0.06 * s, line);
        let shoulder_l = body.left_top() + vec2(0.0, 0.06 * s);
        let hand_l = shoulder_l + vec2(-0.2 * s, 0.26 * s);
        p.line_segment([shoulder_l, hand_l], line);
        p.circle_stroke(hand_l, 0.06 * s, line);
    }

    fn draw_antenna(&self, p: &egui::Painter, head: Rect, s: f32, t: f32, ink: Color32) {
        let line = stroke(2.0, ink);
        let root = head.center_top();
        let light = |tip: Pos2, on: bool| {
            if on {
                glow(p, tip, 0.08 * s, ink, 0.8);
            }
            p.circle_filled(tip, 0.06 * s, if on { ink } else { ink.gamma_multiply(0.4) });
        };
        match self.antenna {
            // two, blinking in turn
            1 => {
                for (k, side) in [-1.0f32, 1.0].into_iter().enumerate() {
                    let tip = root + vec2(side * 0.2 * s, -0.24 * s);
                    p.line_segment([root + vec2(side * 0.08 * s, 0.0), tip], line);
                    light(tip, ((t * 2.5).fract() < 0.5) == (k == 0));
                }
            }
            // a big star that turns slowly and shines
            2 => {
                let tip = root - vec2(0.0, 0.34 * s);
                p.line_segment([root, tip], line);
                glow(p, tip, 0.14 * s, ink, 0.2 + 0.15 * (t * 3.0).sin().abs());
                let pts: Vec<Pos2> = (0..10)
                    .map(|i| {
                        let a = t * 0.8 + i as f32 * TAU / 10.0 - PI / 2.0;
                        let r = if i % 2 == 0 { 0.24 } else { 0.1 } * s;
                        tip + vec2(a.cos(), a.sin()) * r
                    })
                    .collect();
                for i in 0..10 {
                    p.add(Shape::convex_polygon(vec![tip, pts[i], pts[(i + 1) % 10]], ink, Stroke::NONE));
                }
                p.circle_filled(tip, 0.04 * s, Color32::WHITE.gamma_multiply(0.8));
            }
            // a halo instead of an antenna, floating where the light would be, bobbing a little
            3 => {
                let hc = pos2(root.x, root.y - 0.3 * s + (t * 2.0).sin() * 0.03 * s);
                let ring: Vec<Pos2> = (0..32)
                    .map(|i| {
                        let a = i as f32 / 32.0 * TAU;
                        hc + vec2(a.cos() * 0.36 * s, a.sin() * 0.09 * s)
                    })
                    .collect();
                p.add(Shape::closed_line(ring.clone(), stroke(0.12 * s, ink.gamma_multiply(0.15))));
                p.add(Shape::closed_line(ring, stroke(2.5, ink)));
            }
            // a satellite dish on a mast, tilting left and right, its feed blinking
            4 => {
                let mid = root - vec2(0.0, 0.3 * s);
                p.line_segment([root, mid + vec2(0.0, 0.04 * s)], line);
                let tilt = -0.35 + 0.3 * (t * 1.2).sin();
                let turn = |v: egui::Vec2| vec2(v.x * tilt.cos() - v.y * tilt.sin(), v.x * tilt.sin() + v.y * tilt.cos());
                let bowl: Vec<Pos2> = (0..=16)
                    .map(|i| {
                        let a = PI * i as f32 / 16.0;
                        mid + turn(vec2(a.cos() * 0.26 * s, a.sin() * 0.12 * s))
                    })
                    .collect();
                p.add(Shape::convex_polygon(bowl, ink.gamma_multiply(0.55), stroke(1.5, ink)));
                let feed = mid + turn(vec2(0.0, -0.2 * s));
                p.line_segment([mid, feed], stroke(1.5, ink));
                light(feed, (t * 1.5).fract() < 0.5);
            }
            // the classic: one, blinking
            _ => {
                let tip = root - vec2(0.0, 0.23 * s);
                p.line_segment([root, tip], line);
                light(tip, (t * 2.5).fract() < 0.6);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stardust_levels_and_skins() {
        let mut l = Luna::default();
        assert_eq!(l.level(), 1);
        assert_eq!(l.add(Source::Keep, 29), None);
        assert_eq!(l.add(Source::Keep, 1), Some(2));
        assert_eq!(l.progress(), (0.0, Some(80)));
        l.add(Source::Keep, 10_000);
        assert_eq!(l.level(), 10);
        assert_eq!(l.progress(), (1.0, None));
        // everything unlocked at the top; nothing worn that isn't unlocked
        assert_eq!(l.lines().len(), crate::log::DROPS.len() + LINES.len());
        let mut fresh = Luna {
            antenna: 3,
            color: 4,
            extra: 3,
            ..Default::default()
        };
        fresh.sanitize();
        assert_eq!((fresh.antenna, fresh.color, fresh.extra), (0, 0, 0));
        assert_eq!(Luna::unlocked(4), "shades");
        assert_eq!(Luna::unlocked(10), "the colour plasma and devil horns");
        assert!((2..=10).all(|lv| Luna::unlocked(lv) != "new lines to drop" || LINES.iter().any(|l| l.0 == lv)));
    }

    #[test]
    fn daily_limits_and_play_rate() {
        let mut l = Luna::default();
        // picks: 2 each until 40 a day, then nothing – the next day it starts over
        let got: u32 = (0..30).map(|_| l.earn(Source::Pick, 100).0).sum();
        assert_eq!(got, PICK_DAY);
        assert_eq!(l.would_earn(Source::Pick, 100), 0);
        assert_eq!(l.earn(Source::Pick, 101).0, DUST_PICK);
        assert_eq!(l.today.pick, DUST_PICK);
        assert_eq!(l.total.pick, PICK_DAY + DUST_PICK);
        // playing: full rate for the first PLAY_FULL a day, then slower
        let mut p = Luna::default();
        let frames = |secs: f32| (secs * 60.0 + 30.0) as usize;
        for _ in 0..frames(PLAY_FULL as f32 * PLAY_SECONDS) {
            p.play(1.0 / 60.0, 7);
        }
        assert_eq!(p.dust, PLAY_FULL);
        for _ in 0..frames(10.0 * PLAY_SLOW) {
            p.play(1.0 / 60.0, 7);
        }
        assert_eq!(p.dust, PLAY_FULL + 10);
        assert_eq!(p.total.play, p.dust);
    }

    #[test]
    fn saved_in_its_own_file_safely() {
        let dir = std::env::temp_dir().join(format!("solarum-luna-{}", std::process::id()));
        let path = dir.join("luna.ron");
        let mut l = Luna::default();
        l.earn(Source::Keep, 3);
        l.played = 7.0;
        l.save(&path).unwrap();
        let back = Luna::load(&path).unwrap();
        assert!(back.same(&l));
        assert_eq!(back.played, 0.0);
        // a broken file is moved aside, not lost
        std::fs::write(&path, "not ron").unwrap();
        assert!(Luna::load(&path).is_none());
        assert!(dir.join("luna.broken.ron").exists());
        // the one that came further wins (the settings' copy from before the file)
        let old = Luna {
            dust: 500,
            ..Default::default()
        };
        assert_eq!(Luna::merge(Some(back), Some(old)).dust, 500);
        assert_eq!(Luna::merge(Some(old), Some(back)).dust, 500);
        assert_eq!(Luna::merge(None, None).dust, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
