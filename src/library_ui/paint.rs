//! Drawing helpers shared by the SOUNDS screens: waveforms, the picture tiles and value
//! cards, pills, the empty state and the pictures of the new-sound flow.

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Tile {
    Pitch,
    Reverse,
    Tone,
    Crush,
    /// Soft, bright or air – what a drawn line sounds like
    Wave(u8),
    Length,
}

pub(super) fn fmt_secs(s: f32) -> String {
    if s < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else {
        format!("{s:.2} s")
    }
}

/// A waveform of `src` from `from` to `to` seconds into `r`
pub(super) fn draw_wave(p: &egui::Painter, r: Rect, src: &Source, from: f32, to: f32, color: Color32) {
    draw_wave_gain(p, r, src, from, to, color, 1.0);
}

/// The same, louder or softer (as the edit levels the sound)
pub(super) fn draw_wave_gain(p: &egui::Painter, r: Rect, src: &Source, from: f32, to: f32, color: Color32, gain: f32) {
    let w = r.width().max(1.0) as usize;
    let span = (to - from).max(1e-6);
    let mid = r.center().y;
    let h = r.height() * 0.48 * gain;
    let mut segs = Vec::with_capacity(w);
    for x in 0..w {
        let t0 = from + span * x as f32 / w as f32;
        let t1 = from + span * (x + 1) as f32 / w as f32;
        let a = ((t0 * SAMPLE_SR) as usize).min(src.data.len());
        let b = ((t1 * SAMPLE_SR) as usize).clamp(a, src.data.len());
        if a >= src.data.len() {
            break;
        }
        let (lo, hi) = if b - a >= BLOCK * 2 {
            src.peaks[a / BLOCK..(b / BLOCK).min(src.peaks.len())]
                .iter()
                .fold((0.0f32, 0.0f32), |(l, h), &(x, y)| (l.min(x), h.max(y)))
        } else {
            src.data[a..(b + 1).min(src.data.len())]
                .iter()
                .fold((0.0f32, 0.0f32), |(l, h), &v| (l.min(v), h.max(v)))
        };
        let px = r.left() + x as f32 + 0.5;
        segs.push(Shape::line_segment(
            [pos2(px, mid - hi * h - 0.5), pos2(px, mid - lo * h + 0.5)],
            stroke(1.0, color),
        ));
    }
    p.extend(segs);
}

/// A line through `f(x)` for x in 0…1, inside `r` (y: -1 … 1)
fn curve(p: &egui::Painter, r: Rect, s: Stroke, f: impl Fn(f32) -> f32) {
    let n = 64;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let x = i as f32 / n as f32;
            pos2(r.left() + x * r.width(), r.center().y - f(x).clamp(-1.0, 1.0) * r.height() * 0.5)
        })
        .collect();
    p.add(Shape::line(pts, s));
}

/// What a tile does, as a little picture of its value (`v`: the tile's own range)
pub(super) fn picture(p: &egui::Painter, r: Rect, tile: Tile, v: f32, col: Color32, t: f32) {
    let s = stroke(2.0, col);
    let faint = stroke(1.5, alpha(col, 0.35));
    match tile {
        // A wave: more cycles the higher it goes
        Tile::Pitch => {
            let cycles = 2.0 * 2f32.powf(v / 12.0);
            curve(p, r, faint, |x| (x * TAU * 2.0).sin() * 0.7);
            curve(p, r, s, |x| (x * TAU * cycles + t * 2.0).sin() * 0.7);
        }
        // A hit and its tail – turned round when on
        Tile::Reverse => {
            let env = |x: f32| (-x * 5.0).exp();
            curve(p, r, s, |x| {
                let x = if v > 0.5 { 1.0 - x } else { x };
                (x * TAU * 9.0).sin() * env(x) * 0.9
            });
            let y = r.bottom() - 2.0;
            let (a, b) = if v > 0.5 { (r.right(), r.left()) } else { (r.left(), r.right()) };
            p.arrow(pos2(a, y), vec2(b - a, 0.0) * 0.35, stroke(1.5, alpha(col, 0.7)));
        }
        // A filter curve: left closes the highs (darker), right cuts the lows (thinner)
        Tile::Tone => {
            let lo = v.max(0.0);
            let hi = (-v).max(0.0);
            curve(p, r, s, move |x| {
                let low = 1.0 / (1.0 + ((lo * 0.8 - x) * 14.0).exp().powf(if lo > 0.01 { 1.0 } else { 0.0 }));
                let high = 1.0 / (1.0 + ((x - (1.0 - hi * 0.8)) * 14.0).exp().powf(if hi > 0.01 { 1.0 } else { 0.0 }));
                let g = if lo > 0.01 { low } else { 1.0 } * if hi > 0.01 { high } else { 1.0 };
                g * 1.4 - 0.7
            });
        }
        // A sine turning into stairs
        Tile::Crush => {
            let steps = 2f32.powf(5.0 - v * 4.0).max(1.0);
            let hold = 1.0 / (6.0 + (1.0 - v) * 60.0);
            curve(p, r, faint, |x| (x * TAU * 1.5).sin() * 0.8);
            curve(p, r, s, |x| {
                let x = (x / hold).floor() * hold;
                ((x * TAU * 1.5).sin() * 0.8 * steps).round() / steps
            });
        }
        // Hits with cuts between them – as many as it finds
        Tile::Wave(w) => {
            curve(p, r, s, move |x| match w {
                1 => ((x * 3.0).fract() * 2.0 - 1.0) * 0.75,
                2 => ((x * 913.0).sin() * 43_758.5).fract() * 1.4 - 0.7,
                _ => (x * TAU * 3.0).sin() * 0.75,
            });
        }
        // A line as long as the sound
        Tile::Length => {
            let y = r.center().y;
            let x1 = r.left() + r.width() * v;
            p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], faint);
            p.line_segment([pos2(r.left(), y), pos2(x1, y)], stroke(3.0, col));
            p.circle_filled(pos2(x1, y), 5.0, col);
        }
    }
}

/// A value as the sound section's cards show it (SPACE, PUMP, DRIVE): drag up or down or
/// scroll, double-click resets; the picture shows what it does. `v` is 0…1, `shown` the
/// picture's own value.
#[allow(clippy::too_many_arguments)]
pub(super) fn value_card(
    ui: &mut Ui,
    size: egui::Vec2,
    kind: Tile,
    name: &str,
    value: &str,
    v: &mut f32,
    default: f32,
    shown: impl Fn(f32) -> f32,
    col: Color32,
) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    crate::controls::drag_value(ui, &mut resp, v, default);
    let p = ui.painter_at(rect);
    let hot = resp.hovered() || resp.dragged();
    let area = crate::controls::card(&p, rect, name, value, col, hot);
    let t = if hot { ui.input(|i| i.time) as f32 } else { 0.0 };
    picture(&p, area, kind, shown(*v), col, t);
    if hot {
        ui.ctx().request_repaint();
    }
    resp.on_hover_cursor(egui::CursorIcon::ResizeVertical)
}

/// An action as the live effects show it: the name on top, a picture of what it does below
#[allow(clippy::too_many_arguments)]
pub(super) fn action_button(
    ui: &mut Ui,
    size: egui::Vec2,
    kind: Tile,
    icon: &str,
    name: &str,
    active: bool,
    shown: f32,
    col: Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    crate::widgets::fx_button(ui, rect, &resp, icon, name, "", col, active, 0.0, |p, r, ink| {
        picture(p, r, kind, shown, ink, 0.0)
    });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The mouse wheel turned over this card (a scroll changes its value)
pub(super) fn ui_scrolled(r: &egui::Response) -> bool {
    r.ctx.input(|i| i.raw_scroll_delta.y != 0.0)
}

/// Behind a dialog: the screen darker, and clicks on it go nowhere
pub(super) fn dim_screen(ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("lib_dim"))
        .order(egui::Order::Middle)
        .fixed_pos(pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let r = ctx.screen_rect();
            ui.allocate_rect(r, Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(150));
        });
}

pub(super) fn empty_state(p: &egui::Painter, r: Rect) {
    p.rect_filled(r, 14.0, mix(BG, PANEL, 0.6));
    p.text(r.center(), Align2::CENTER_CENTER, "Pick a sound on the left", bold(16.0), FAINT);
}

/// The one big button of a screen (lime, white on hover; grey and dead when `on` is false)
pub(super) fn pill(ui: &mut Ui, text: &str, on: bool) -> egui::Response {
    let g = ui.painter().layout_no_wrap(text.to_owned(), bold(14.0), BG);
    let (br, resp) = ui.allocate_exact_size(g.size() + vec2(36.0, 18.0), if on { Sense::click() } else { Sense::hover() });
    let fill = match (on, resp.hovered()) {
        (false, _) => PANEL_HI,
        (true, true) => Color32::WHITE,
        (true, false) => ACID,
    };
    ui.painter().rect_filled(br, br.height() / 2.0, fill);
    ui.painter().galley(br.center() - g.size() / 2.0, g, BG);
    if on {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// The name a sound painted from a track's template gets
pub(super) fn template_name(v: Voice) -> &'static str {
    match v {
        Voice::Kick => "My kick",
        Voice::Clap => "My clap",
        Voice::Rim => "My rim",
        Voice::Hat => "My hat",
        Voice::OpenHat => "My open hat",
        Voice::Ride => "My ride",
        Voice::Bass => "My bass",
        Voice::Acid => "My acid",
    }
}

/// A template in small: its line, and its loudness as bars underneath
pub(super) fn thumb_drawing(p: &egui::Painter, r: Rect, d: &library::Drawing, col: Color32) {
    let n = d.pitch.len();
    let step = 4;
    let w = r.width() / n as f32 * step as f32;
    for c in (0..n).step_by(step) {
        let x = r.left() + c as f32 / n as f32 * r.width();
        let h = d.level[c] * r.height() * 0.35;
        p.line_segment([pos2(x, r.bottom()), pos2(x, r.bottom() - h)], stroke(w * 0.6, alpha(col, 0.28)));
    }
    let line = r.with_max_y(r.bottom() - r.height() * 0.2);
    let pts: Vec<Pos2> = d
        .pitch
        .iter()
        .enumerate()
        .filter_map(|(c, v)| v.map(|v| pos2(r.left() + c as f32 / n as f32 * r.width(), line.bottom() - v * line.height())))
        .collect();
    p.add(Shape::line(pts.clone(), stroke(7.0, alpha(col, 0.15))));
    p.add(Shape::line(pts, stroke(2.5, col)));
}

/// Breeding in small: one sound, four children – on hover a spark runs to each in turn
pub(super) fn family_picture(p: &egui::Painter, r: Rect, hot: bool, t: f32) {
    let root = pos2(r.left() + 14.0, r.center().y);
    let lit = if hot { (t * 1.4) as usize % 4 } else { 1 };
    let kw = 74.0;
    for k in 0..4 {
        let y = r.top() + r.height() * (k as f32 + 0.5) / 4.0;
        let kr = Rect::from_center_size(pos2(r.right() - kw / 2.0, y), vec2(kw, r.height() / 4.0 - 5.0));
        let col = if k == lit { ACID } else { track_color(Voice::ALL[k * 2 + 1]) };
        let from = root + vec2(10.0, 0.0);
        let to = pos2(kr.left(), y);
        let mid = pos2((from.x + to.x) / 2.0, y);
        let pts: Vec<Pos2> = (0..=16)
            .map(|i| {
                // a soft S from the parent to the child
                let u = i as f32 / 16.0;
                let s = u * u * (3.0 - 2.0 * u);
                pos2(from.x + (to.x - from.x) * u, from.y + (mid.y - from.y) * s)
            })
            .collect();
        p.add(Shape::line(
            pts.clone(),
            stroke(1.5, if k == lit { alpha(ACID, 0.7) } else { alpha(LINE, 0.8) }),
        ));
        if hot && k == lit {
            let i = ((t * 1.4).fract() * 16.0) as usize;
            p.circle_filled(pts[i.min(16)], 3.0, ACID);
        }
        p.rect_filled(kr, 5.0, if k == lit { alpha(ACID, 0.12) } else { PAD });
        p.rect_stroke(
            kr,
            5.0,
            stroke(1.0, if k == lit { ACID } else { alpha(LINE, 0.8) }),
            egui::StrokeKind::Inside,
        );
        let w = kr.shrink2(vec2(7.0, 4.0));
        let pts: Vec<Pos2> = (0..=24)
            .map(|i| {
                let u = i as f32 / 24.0;
                let y = (u * TAU * (1.5 + k as f32 * 1.2)).sin() * (1.0 - u).powf(0.8 + k as f32 * 0.5);
                pos2(w.left() + u * w.width(), w.center().y - y * w.height() / 2.0)
            })
            .collect();
        p.add(Shape::line(pts, stroke(1.5, col)));
    }
    let pulse = if hot { 1.0 + (t * 5.0).sin() * 0.12 } else { 1.0 };
    p.circle_filled(root, 13.0 * pulse, alpha(ACID, 0.15));
    p.circle_filled(root, 8.0, ACID);
}

/// What a way to start does, as a little picture that comes alive on hover: a line being
/// drawn, a level meter, a file landing as a waveform
pub(super) fn way_picture(p: &egui::Painter, r: Rect, k: usize, col: Color32, hot: bool, t: f32) {
    match k {
        0 => {
            let f = if hot { (t * 0.45).fract() } else { 0.72 };
            let n = 48;
            let pts: Vec<Pos2> = (0..=((n as f32 * f) as usize))
                .map(|i| {
                    let x = i as f32 / n as f32;
                    let y = 0.5 - (x * TAU * 1.3).sin() * 0.28 * (1.0 - x * 0.4) - x * 0.1;
                    pos2(r.left() + x * r.width(), r.top() + y * r.height())
                })
                .collect();
            for i in 1..4 {
                let y = r.top() + r.height() * i as f32 / 4.0;
                p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], stroke(1.0, alpha(LINE, 0.25)));
            }
            if let Some(&tip) = pts.last() {
                p.add(Shape::line(pts.clone(), stroke(7.0, alpha(col, 0.15))));
                p.add(Shape::line(pts, stroke(2.5, col)));
                p.text(tip + vec2(7.0, -9.0), Align2::CENTER_CENTER, ph::PENCIL_SIMPLE, body(16.0), TEXT);
            }
        }
        1 => {
            let bars = 11;
            let w = r.width() / bars as f32;
            for i in 0..bars {
                let x = r.left() + (i as f32 + 0.5) * w;
                let calm = 0.25 + 0.6 * (-((i as f32 - 5.0) / 3.0).powi(2)).exp();
                let h = if hot {
                    (0.2 + 0.8 * ((t * 7.0 + i as f32 * 1.7).sin() * 0.5 + 0.5) * calm.max(0.4)).min(1.0)
                } else {
                    calm
                };
                let h = h * r.height() * 0.5;
                p.line_segment(
                    [pos2(x, r.center().y - h), pos2(x, r.center().y + h)],
                    stroke(w * 0.55, if hot { col } else { alpha(col, 0.55) }),
                );
            }
            let on = !hot || (t * 2.0).fract() < 0.6;
            p.circle_filled(r.left_top() + vec2(4.0, 2.0), 4.0, if on { RED } else { alpha(RED, 0.3) });
        }
        _ => {
            // hits decaying along the bottom half, a file icon dropping into them
            let base = r.top() + r.height() * 0.68;
            let n = 40;
            let w = r.width() / n as f32;
            for i in 0..n {
                let x = r.left() + (i as f32 + 0.5) * w;
                let ph = (i % 10) as f32 / 10.0;
                let h = (-ph * 4.0).exp() * r.height() * 0.3 + 1.0;
                p.line_segment([pos2(x, base - h), pos2(x, base + h)], stroke((w * 0.6).max(1.0), alpha(col, 0.8)));
            }
            let drop = if hot { (t * 0.9).fract() } else { 0.0 };
            let y = r.top() + 10.0 + drop * r.height() * 0.25;
            p.text(
                pos2(r.center().x, y),
                Align2::CENTER_CENTER,
                ph::FILE_AUDIO,
                body(22.0),
                alpha(TEXT, 1.0 - drop * 0.7),
            );
        }
    }
}
