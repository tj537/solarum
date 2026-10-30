//! Luna, the little robot that plays the sets: how far it has come with you (stardust and
//! levels – earned by playing, breeding and keeping sounds) and how it looks (antenna,
//! colour, an extra – each unlocked at a level). No shop, no money: only progress.
//!
//! The robot is drawn here too, in its skin, wherever it shows: circling the sun on the start
//! screen, on the SOLAR OS screen, with the sounds of the day, in the settings.

use crate::theme::*;
use crate::widgets::glow;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Shape};
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};

/// The stardust each level starts at (level 1 … 10)
pub const LEVELS: [u32; 10] = [0, 30, 80, 160, 280, 450, 700, 1000, 1400, 2000];

/// Seconds of playing for one stardust
pub const PLAY_SECONDS: f32 = 20.0;
/// Stardust for picking a bred sound, and for keeping or saving one
pub const DUST_PICK: u32 = 2;
pub const DUST_KEEP: u32 = 5;

/// The antennas: name and the level that unlocks it
pub const ANTENNAS: [(&str, usize); 4] = [("Classic", 1), ("Double", 3), ("Star", 6), ("Dish", 9)];
/// The colours: name, level, colour
pub const COLORS: [(&str, usize, Color32); 5] = [
    ("Lime", 1, ACID),
    ("Ice", 2, Color32::from_rgb(110, 220, 255)),
    ("Bubblegum", 5, Color32::from_rgb(255, 120, 200)),
    ("Gold", 8, Color32::from_rgb(255, 205, 80)),
    ("Plasma", 10, Color32::from_rgb(190, 140, 255)),
];
/// The extras: name and level (0 = none)
pub const EXTRAS: [(&str, usize); 4] = [("None", 1), ("Shades", 4), ("Headphones", 7), ("Crown", 10)];

/// New lines Luna drops into the log from a level on
const LINES: [(usize, &str); 12] = [
    (2, "Level two, fam. I can feel the bass in my circuits now."),
    (3, "Two antennas, double the vibes. Science."),
    (4, "Shades on. The future's bright, bro."),
    (4, "Can't see a thing in these. Don't care. Looking fresh."),
    (5, "Halfway to the stars, brother. Keep cooking."),
    (6, "I literally have a star on my head. Main character."),
    (7, "Headphones on. Now I hear the hi-hats breathing."),
    (7, "Real talk, you made me who I am. Respect."),
    (8, "Gold edition Luna. Limited drop. Only one exists."),
    (9, "Satellite dish online. Picking up raves from three galaxies."),
    (10, "Level ten. The crown fits. We're royalty now."),
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
}

impl Luna {
    /// 1 … 10
    pub fn level(&self) -> usize {
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

    /// Add stardust; the new level if that was a level up
    pub fn earn(&mut self, dust: u32) -> Option<usize> {
        let before = self.level();
        self.dust = self.dust.saturating_add(dust);
        (self.level() > before).then(|| self.level())
    }

    /// `dt` seconds of playing
    pub fn play(&mut self, dt: f32) -> Option<usize> {
        self.played += dt.clamp(0.0, 1.0);
        let whole = (self.played / PLAY_SECONDS) as u32;
        if whole == 0 {
            return None;
        }
        self.played -= whole as f32 * PLAY_SECONDS;
        self.earn(whole)
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
        p.rect_stroke(body, 0.08 * s, line, egui::StrokeKind::Middle);
        p.circle_filled(body.center(), 0.05 * s, ink.gamma_multiply(0.5 + 0.5 * (t * 4.0).sin().abs()));
        p.line_segment([c + vec2(0.0, 0.39 * s), body.center_top()], line);
        let head = Rect::from_center_size(c, vec2(s, 0.78 * s));
        p.rect_stroke(head, 0.16 * s, line, egui::StrokeKind::Middle);
        let headphones = self.extra == 2;
        // Ears (big cups with headphones on)
        for side in [-1.0, 1.0] {
            let size = if headphones {
                vec2(0.16 * s, 0.36 * s)
            } else {
                vec2(0.1 * s, 0.26 * s)
            };
            let ear = Rect::from_center_size(c + vec2(side * (0.56 + if headphones { 0.03 } else { 0.0 }) * s, 0.0), size);
            p.rect_filled(ear, 0.04 * s, ink.gamma_multiply(if headphones { 1.0 } else { 0.7 }));
        }
        if headphones {
            let band: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = PI + PI * i as f32 / 20.0;
                    c + vec2(a.cos() * 0.6 * s, a.sin() * 0.56 * s - 0.04 * s)
                })
                .collect();
            p.add(Shape::line(band, stroke(3.0, ink)));
        }
        self.draw_antenna(p, head, s, t, ink);
        if self.extra == 3 {
            // a little crown on top, around the antenna
            let base = head.top() - 0.02 * s;
            let w = 0.5 * s;
            let pts = [
                pos2(c.x - w / 2.0, base),
                pos2(c.x - w / 2.0, base - 0.18 * s),
                pos2(c.x - w / 4.0, base - 0.08 * s),
                pos2(c.x, base - 0.22 * s),
                pos2(c.x + w / 4.0, base - 0.08 * s),
                pos2(c.x + w / 2.0, base - 0.18 * s),
                pos2(c.x + w / 2.0, base),
            ];
            p.add(Shape::convex_polygon(pts[..].to_vec(), ink.gamma_multiply(0.35), stroke(1.5, ink)));
            for x in [-0.25, 0.0, 0.25] {
                let y = if x == 0.0 { 0.22 } else { 0.18 };
                p.circle_filled(pos2(c.x + x * w * 2.0, base - y * s), 0.035 * s, ink);
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
            // a star that turns slowly
            2 => {
                let tip = root - vec2(0.0, 0.26 * s);
                p.line_segment([root, tip + vec2(0.0, 0.08 * s)], line);
                glow(p, tip, 0.12 * s, ink, 0.5 + 0.3 * (t * 3.0).sin().abs());
                let pts: Vec<Pos2> = (0..10)
                    .map(|i| {
                        let a = t * 0.8 + i as f32 * TAU / 10.0 - PI / 2.0;
                        let r = if i % 2 == 0 { 0.13 } else { 0.055 } * s;
                        tip + vec2(a.cos(), a.sin()) * r
                    })
                    .collect();
                p.add(Shape::closed_line(pts, stroke(1.5, ink)));
            }
            // a satellite dish, scanning left and right
            3 => {
                let tip = root - vec2(0.0, 0.2 * s);
                p.line_segment([root, tip], line);
                let look = (t * 1.2).sin() * 0.5;
                let dish: Vec<Pos2> = (0..=12)
                    .map(|i| {
                        let a = -PI / 2.0 + look + (i as f32 / 12.0 - 0.5) * 2.2;
                        tip + vec2(a.cos(), a.sin()) * 0.16 * s + vec2(0.0, 0.1 * s)
                    })
                    .collect();
                p.add(Shape::line(dish, stroke(2.0, ink)));
                let aim = tip + vec2(look.sin(), -look.cos().abs()) * 0.16 * s;
                p.line_segment([tip, aim], stroke(1.0, ink));
                light(aim, (t * 1.5).fract() < 0.5);
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
        assert_eq!(l.earn(29), None);
        assert_eq!(l.earn(1), Some(2));
        assert_eq!(l.progress(), (0.0, Some(80)));
        // playing: one stardust every PLAY_SECONDS, a level up when it gets there
        let mut ups = Vec::new();
        for _ in 0..((50.0 * PLAY_SECONDS + 0.5) * 60.0) as usize {
            if let Some(lv) = l.play(1.0 / 60.0) {
                ups.push(lv);
            }
        }
        assert_eq!(l.dust, 80);
        assert_eq!(ups, vec![3]);
        l.earn(10_000);
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
        assert_eq!(Luna::unlocked(10), "the colour plasma and crown");
        // every level brings something
        assert!((2..=10).all(|lv| Luna::unlocked(lv) != "new lines to drop" || LINES.iter().any(|l| l.0 == lv)));
    }
}
