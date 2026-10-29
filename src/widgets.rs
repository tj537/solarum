//! Custom controls: knobs, glowing buttons, the bipolar filter slider.

use crate::theme::*;
use eframe::egui::{self, pos2, vec2, Align2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

/// Soft glow: stacked translucent circles
pub fn glow(p: &egui::Painter, c: Pos2, r: f32, color: Color32, strength: f32) {
    if strength <= 0.01 {
        return;
    }
    for i in 0..6 {
        let k = i as f32 / 5.0;
        p.circle_filled(c, r * (1.0 + k * 1.4), alpha(color, strength * 0.16 * (1.0 - k)));
    }
}

/// Soft glow around a rounded rectangle
pub fn glow_rect(p: &egui::Painter, r: Rect, radius: f32, color: Color32, strength: f32) {
    if strength <= 0.01 {
        return;
    }
    // One soft blurred shadow in the colour – no rings
    let c = color.gamma_multiply((strength * 0.35).min(0.6));
    p.add(egui::epaint::RectShape::filled(r.expand(3.0), radius + 3.0, c).with_blur_width(18.0));
}

/// Arc as a polyline from angle `a0` to `a1` (radians, 0 = right, clockwise on screen)
pub fn arc(p: &egui::Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = ((a1 - a0).abs() * r / 4.0).clamp(4.0, 128.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(Shape::line(pts, stroke));
}

/// A minimal slider with a few stops (a line, a dot per stop, a knob on the picked one);
/// returns the stop clicked or dragged to
pub fn stops(ui: &mut Ui, values: &[u8], picked: u8, width: f32, color: Color32) -> Option<u8> {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), Sense::click_and_drag());
    let line = rect.shrink2(vec2(10.0, 0.0));
    let n = values.len().max(2) as f32 - 1.0;
    let x_of = |i: usize| line.left() + line.width() * i as f32 / n;
    let y = rect.top() + 12.0;
    let p = ui.painter();
    p.line_segment(
        [pos2(line.left(), y), pos2(line.right(), y)],
        stroke(2.0, mix(PANEL_HI, Color32::WHITE, 0.14)),
    );
    let at = values.iter().position(|&v| v == picked);
    if let Some(i) = at {
        p.line_segment([pos2(line.left(), y), pos2(x_of(i), y)], stroke(2.5, color));
    }
    for (i, v) in values.iter().enumerate() {
        let on = at.is_some_and(|a| i <= a);
        p.circle_filled(
            pos2(x_of(i), y),
            if values.len() > 8 { 2.0 } else { 3.0 },
            if on { color } else { FAINT },
        );
        // Many stops: only the ends and the picked one are labelled
        if values.len() > 5 && i != 0 && i + 1 != values.len() && Some(i) != at {
            continue;
        }
        p.text(
            pos2(x_of(i), rect.bottom()),
            Align2::CENTER_BOTTOM,
            v.to_string(),
            bold(11.0),
            if Some(i) == at { TEXT } else { DIM },
        );
    }
    if let Some(i) = at {
        p.circle_filled(pos2(x_of(i), y), 7.0, TEXT);
    }
    let pos = resp.interact_pointer_pos().filter(|_| resp.clicked() || resp.dragged())?;
    let i = (((pos.x - line.left()) / line.width()) * n).round().clamp(0.0, n) as usize;
    (values[i] != picked).then_some(values[i])
}

/// A screen set into the concrete: a dark well and a thick bezel;
/// returns the screen's rect inside it
/// (`bezel_w`: how thick the frame is)
pub fn bezel_frame(p: &egui::Painter, outer: Rect, radius: f32, bezel_w: f32) -> Rect {
    p.rect_filled(outer.expand(2.0), radius + 2.0, Color32::from_black_alpha(110));
    let bezel = outer.shrink(2.0);
    p.rect_filled(bezel, radius, mix(PANEL, BG, 0.45));
    p.rect_stroke(bezel, radius, stroke(1.0, Color32::from_white_alpha(9)), egui::StrokeKind::Inside);
    outer.shrink(2.0 + bezel_w)
}

/// Small pill toggle (chips)
pub fn chip(ui: &mut Ui, text: &str, on: bool, color: Color32) -> Response {
    chip_sized(ui, text, on, color, None)
}

/// A chip with a fixed width (`None`: as wide as its text)
pub fn chip_sized(ui: &mut Ui, text: &str, on: bool, color: Color32, width: Option<f32>) -> Response {
    chip_shaped(ui, text, on, color, width, 30.0, 13.0)
}

fn chip_shaped(ui: &mut Ui, text: &str, on: bool, color: Color32, width: Option<f32>, height: f32, font: f32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), bold(font), TEXT);
    let size = vec2(width.unwrap_or(galley.size().x + 22.0), height);
    let r = height / 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let hot = resp.hovered();
    if on {
        // Opaque: a translucent colour reads washed out on the concrete
        p.rect_filled(rect, r, mix(PANEL, color, 0.2));
        p.rect_stroke(rect, r, stroke(1.2, color), egui::StrokeKind::Inside);
    } else {
        p.rect_filled(rect, r, if hot { PANEL_HI } else { Color32::TRANSPARENT });
        p.rect_stroke(rect, r, stroke(1.0, if hot { FAINT } else { LINE }), egui::StrokeKind::Inside);
    }
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        bold(font),
        if on {
            color
        } else if hot {
            TEXT
        } else {
            DIM
        },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The record button with an arrow beside it in one pill: (record, arrow)
pub fn rec_split(ui: &mut Ui, text: &str, on: bool, now: f64) -> (Response, Response) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), bold(13.0), TEXT);
    let (main_w, arrow_w) = ((galley.size().x + 22.0).max(40.0), 24.0);
    let (all, _) = ui.allocate_exact_size(vec2(main_w + arrow_w, 30.0), Sense::hover());
    let main = Rect::from_min_size(all.min, vec2(main_w, 30.0));
    let arrow = Rect::from_min_max(pos2(main.right(), all.top()), all.max);
    let rec = ui.interact(main, ui.id().with("rec"), Sense::click());
    let more = ui.interact(arrow, ui.id().with("rec_more"), Sense::click());
    let p = ui.painter();
    if on {
        let pulse = 0.5 + 0.5 * (now as f32 * 4.0).sin();
        glow_rect(p, all, 15.0, RED, 0.4 + 0.4 * pulse);
        p.rect_filled(all, 15.0, mix(PANEL, RED, 0.75 + 0.15 * pulse));
    } else {
        p.rect_filled(all, 15.0, mix(PANEL, RED, 0.14));
        for (r, hot, rounding) in [
            (
                main,
                rec.hovered(),
                egui::CornerRadius {
                    nw: 15,
                    sw: 15,
                    ne: 0,
                    se: 0,
                },
            ),
            (
                arrow,
                more.hovered(),
                egui::CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: 15,
                    se: 15,
                },
            ),
        ] {
            if hot {
                p.rect_filled(r, rounding, mix(PANEL, RED, 0.3));
            }
        }
        p.rect_stroke(all, 15.0, stroke(1.2, RED), egui::StrokeKind::Inside);
    }
    let ink = if on { TEXT } else { RED };
    p.line_segment(
        [pos2(main.right(), all.top() + 7.0), pos2(main.right(), all.bottom() - 7.0)],
        stroke(1.0, alpha(ink, 0.5)),
    );
    p.text(main.center(), Align2::CENTER_CENTER, text, bold(13.0), ink);
    p.text(
        arrow.center() - vec2(2.0, 0.0),
        Align2::CENTER_CENTER,
        egui_phosphor::regular::CARET_DOWN,
        body(12.0),
        ink,
    );
    (rec.on_hover_cursor(egui::CursorIcon::PointingHand), more)
}

/// Side-by-side choices in one pill; the picked one is filled. Returns the one clicked.
pub fn segmented(ui: &mut Ui, items: &[(String, &str)], picked: &[bool], color: Color32) -> Option<usize> {
    let widths: Vec<f32> = items
        .iter()
        .map(|(t, _)| ui.painter().layout_no_wrap(t.clone(), bold(12.5), TEXT).size().x + 26.0)
        .collect();
    let (rect, _) = ui.allocate_exact_size(vec2(widths.iter().sum(), 30.0), Sense::hover());
    ui.painter().rect_filled(rect, 15.0, mix(BG, SPACE, 0.35));
    let mut x = rect.left();
    let mut clicked = None;
    for (i, ((text, tip), w)) in items.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*w, 30.0));
        x += w;
        let resp = ui.interact(r, ui.id().with(("segment", i)), Sense::click()).on_hover_text(*tip);
        let p = ui.painter();
        let on = picked.get(i).copied().unwrap_or(false);
        if on {
            p.rect_filled(r.shrink(2.0), 13.0, color);
        } else if resp.hovered() {
            p.rect_filled(r.shrink(2.0), 13.0, PANEL_HI);
        }
        let ink = if on {
            SPACE
        } else if resp.hovered() {
            TEXT
        } else {
            DIM
        };
        p.text(r.center(), Align2::CENTER_CENTER, text, bold(12.5), ink);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            clicked = Some(i);
        }
    }
    ui.painter().rect_stroke(rect, 15.0, stroke(1.0, LINE), egui::StrokeKind::Inside);
    clicked
}

/// An effect button: icon and name on top, a picture of what it does below (`art`, drawn
/// in the ink it's given), its key in the corner. `lit` 0…1 lights it up without pressing
/// it – when Solarum uses the effect by itself.
#[allow(clippy::too_many_arguments)]
pub fn fx_button(
    ui: &Ui,
    rect: Rect,
    resp: &Response,
    icon: &str,
    name: &str,
    key: &str,
    color: Color32,
    active: bool,
    lit: f32,
    art: impl FnOnce(&egui::Painter, Rect, Color32),
) {
    let p = ui.painter();
    let hot = resp.hovered();
    let lit = if active { 0.0 } else { lit.clamp(0.0, 1.0) };
    let rect = if active { rect.shrink(1.5) } else { rect };
    if active {
        glow_rect(p, rect, 12.0, color, 0.9);
    } else if lit > 0.0 {
        glow_rect(p, rect, 12.0, color, 0.8 * lit);
    }
    // Dark like the pads; lit in its colour
    let fill = if active {
        color
    } else if hot {
        mix(mix(PAD, PANEL_HI, 0.45), color, 0.85 * lit)
    } else {
        mix(PAD, color, 0.85 * lit)
    };
    p.rect_filled(rect, 12.0, fill);
    p.rect_stroke(
        rect,
        12.0,
        stroke(
            1.0,
            if active || lit > 0.3 {
                color
            } else {
                alpha(color, if hot { 0.8 } else { 0.4 })
            },
        ),
        egui::StrokeKind::Inside,
    );
    let fg = if active {
        SPACE
    } else {
        if lit > 0.45 {
            SPACE
        } else {
            color
        }
    };
    // The name on top, the picture of what it does below
    let art_rect = Rect::from_min_max(
        pos2(rect.left() + 10.0, rect.top() + rect.height() * 0.48),
        pos2(rect.right() - 10.0, rect.bottom() - 6.0),
    );
    // (kept inside the button, however wild it gets)
    let clipped = p.with_clip_rect(rect.shrink(3.0));
    art(
        &clipped,
        art_rect,
        if active {
            alpha(SPACE, 0.75)
        } else {
            alpha(color, if hot { 0.95 } else { 0.75 })
        },
    );
    p.text(
        pos2(rect.center().x, rect.top() + rect.height() * 0.27),
        Align2::CENTER_CENTER,
        format!("{icon} {name}"),
        bold(11.5),
        fg,
    );
    p.text(
        rect.right_top() + vec2(-7.0, 5.0),
        Align2::RIGHT_TOP,
        key,
        bold(9.5),
        if active { alpha(BG, 0.6) } else { alpha(TEXT, 0.55) },
    );
}

pub struct EnergyOut {
    pub changed: bool,
    pub locked_clicked: bool,
}

/// The energy's colour, as the fader shows it: icy blue in a break, lime when everything plays
pub fn energy_color(e: f32) -> Color32 {
    mix(Color32::from_rgb(69, 200, 255), ACID, e.clamp(0.0, 1.0))
}

/// The energy fader: drag to strip the groove down (break) or build it up; the
/// full end lets everything play (no drop zone – DROP drops).
/// `steered`: who moves it instead of you ("AUTO", "SONG") – then it's a slim bar that only
/// shows where the set is.
pub fn energy_fader(ui: &mut Ui, value: &mut f32, width: f32, height: f32, steered: Option<&str>) -> EnergyOut {
    if let Some(who) = steered {
        return energy_shown(ui, *value, width, height, who);
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let mut out = EnergyOut {
        changed: false,
        locked_clicked: false,
    };
    let p = ui.painter();
    let top = rect.top() + 8.0;
    p.text(pos2(rect.left(), top), Align2::LEFT_CENTER, "ENERGY", bold(10.5), DIM);
    p.text(
        pos2(rect.right(), top),
        Align2::RIGHT_CENTER,
        format!("{:.0}", *value * 100.0),
        bold(12.0),
        TEXT,
    );
    let track = Rect::from_min_size(pos2(rect.left() + 6.0, top + 14.0), vec2(width - 12.0, 10.0));
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.is_pointer_button_down_on() {
            let v = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            if (v - *value).abs() > 1e-4 {
                *value = v;
                out.changed = true;
            }
        }
    }
    let p = ui.painter();
    let cool = Color32::from_rgb(69, 200, 255);
    let hot_col = mix(cool, ACID, *value);
    p.rect_filled(track, 5.0, PANEL_HI);
    // The break zone
    let break_x = track.left() + track.width() * crate::engine::BREAK_ZONE;
    p.rect_filled(Rect::from_min_max(track.min, pos2(break_x, track.bottom())), 5.0, alpha(cool, 0.22));
    let fill = Rect::from_min_max(track.min, pos2(track.left() + track.width() * *value, track.bottom()));
    p.rect_filled(fill, 5.0, alpha(hot_col, 0.85));
    let knob = pos2(fill.right(), track.center().y);
    glow(p, knob, 7.0, hot_col, 0.6);
    p.circle_filled(knob, 8.0, TEXT);
    let y = track.bottom() + 1.0;
    if height > 52.0 {
        p.text(
            pos2(track.right(), y + 9.0),
            Align2::RIGHT_CENTER,
            "full",
            bold(9.5),
            alpha(ACID, 0.8),
        );
        p.text(
            pos2(track.left(), y + 9.0),
            Align2::LEFT_CENTER,
            "break",
            bold(9.5),
            alpha(cool, 0.8),
        );
        p.text(
            pos2(track.left() + track.width() * 0.6, y + 9.0),
            Align2::CENTER_CENTER,
            "groove",
            bold(9.5),
            FAINT,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal).on_hover_text(
        "Energy is the arrangement – it changes what plays, not how loud. ← → on the keyboard.\n\
         • Break (blue, under 30): no kick, no bass, the sound closes and opens into the reverb.\n\
         • Groove: kick, bass, open hat and ride come in one by one.\n\
         • Drop (red, 90 and up): the build-up starts on the next bar, the drop two bars later.\n\
         So pull it down, bring it up slowly, and you play a whole track with one fader.\n\
         In the auto mode with AUTO FX and in AUTO SONG, Solarum moves it.",
    );
    out
}

/// The energy while Solarum steers it: a slim bar with the break and drop zones, no knob
fn energy_shown(ui: &mut Ui, value: f32, width: f32, height: f32, who: &str) -> EnergyOut {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let p = ui.painter();
    let top = rect.top() + 8.0;
    p.text(pos2(rect.left(), top), Align2::LEFT_CENTER, "ENERGY", bold(10.5), DIM);
    p.text(
        pos2(rect.right(), top),
        Align2::RIGHT_CENTER,
        format!("{who} · {:.0}", value * 100.0),
        bold(10.5),
        ACID,
    );
    let cool = Color32::from_rgb(69, 200, 255);
    let track = Rect::from_min_size(pos2(rect.left() + 6.0, top + 12.0), vec2(width - 12.0, 4.0));
    let at = |e: f32| track.left() + track.width() * e;
    p.rect_filled(track, 2.0, PANEL_HI);
    p.rect_filled(
        Rect::from_min_max(track.min, pos2(at(crate::engine::BREAK_ZONE), track.bottom())),
        2.0,
        alpha(cool, 0.22),
    );
    p.rect_filled(
        Rect::from_min_max(pos2(at(crate::engine::DROP_ZONE), track.top()), track.max),
        2.0,
        alpha(RED, 0.35),
    );
    let fill = Rect::from_min_max(track.min, pos2(at(value), track.bottom()));
    p.rect_filled(fill, 2.0, alpha(mix(cool, RED, value), 0.8));
    let clicked = resp
        .on_hover_text(
            "Solarum steers the energy now – breaks and drops by itself.\nSwitch AUTO FX off (or stop AUTO SONG) to take it over.",
        )
        .clicked();
    EnergyOut {
        changed: false,
        locked_clicked: clicked,
    }
}

/// A soft round spot that fades smoothly to nothing (no rings) – the clouds in the concrete.
/// `centre` is premultiplied (see `shade`).
pub fn soft_spot(p: &egui::Painter, c: Pos2, r: f32, centre: Color32) {
    let mut mesh = egui::Mesh::default();
    let n = 48u32;
    let [cr, cg, cb, ca] = centre.to_array();
    let mid = Color32::from_rgba_premultiplied(cr / 2, cg / 2, cb / 2, ca / 2);
    mesh.colored_vertex(c, centre);
    // A middle ring keeps the falloff soft instead of a cone
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let dir = vec2(a.cos(), a.sin());
        mesh.colored_vertex(c + dir * r * 0.45, mid);
        mesh.colored_vertex(c + dir * r, Color32::TRANSPARENT);
    }
    for k in 0..n {
        let (i0, o0) = (1 + 2 * k, 2 + 2 * k);
        let (i1, o1) = (1 + 2 * ((k + 1) % n), 2 + 2 * ((k + 1) % n));
        mesh.add_triangle(0, i0, i1);
        mesh.add_triangle(i0, o0, o1);
        mesh.add_triangle(i0, o1, i1);
    }
    p.add(Shape::mesh(mesh));
}

/// A light or dark shade of exactly `k` levels for the concrete. (`alpha` blends white in
/// linear light, so a nearly clear white comes out far brighter than its number – too much
/// for a texture this subtle.)
fn shade(light: bool, k: u8) -> Color32 {
    if light {
        Color32::from_rgba_premultiplied(k, k, k, k)
    } else {
        // Darkens the concrete by about k levels
        let a = ((k as f32 * 255.0 / BG.r() as f32) as u32).min(255) as u8;
        Color32::from_rgba_premultiplied(0, 0, 0, a)
    }
}

/// A stable pseudo-random number 0…1
pub fn hash(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

/// Solarum concrete: grey, cloudy where it dried unevenly, a fine grain and small air pores
pub fn concrete(p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 0.0, BG);
    let at = |i: u32| pos2(rect.left() + hash(i) * rect.width(), rect.top() + hash(i + 50_000) * rect.height());
    let area = rect.area();
    // Big soft clouds, a few levels lighter or darker
    for i in 0..(area / 40_000.0) as u32 {
        let light = hash(i + 7) > 0.5;
        soft_spot(
            p,
            at(i + 1_000),
            90.0 + hash(i + 3) * 220.0,
            shade(light, 6 + (hash(i + 5) * 5.0) as u8),
        );
    }
    // Mottling
    for i in 0..(area / 3_000.0) as u32 {
        let light = hash(i + 11) > 0.5;
        p.circle_filled(at(i + 20_000), 3.0 + hash(i + 13) * 10.0, shade(light, 2));
    }
    // Fine grain
    for i in 0..(area / 260.0) as u32 {
        let light = hash(i + 90_000) > 0.5;
        p.circle_filled(at(i + 200_000), 0.5 + hash(i + 70_000) * 0.6, shade(light, 5));
    }
    // Air pores: small dark holes with a faint light lower rim
    for i in 0..(area / 6_000.0) as u32 {
        let c = at(i + 400_000);
        let r = 0.7 + hash(i + 17) * hash(i + 19) * 2.0;
        p.circle_filled(c + vec2(0.0, r * 0.45), r, shade(true, 4));
        p.circle_filled(c, r, shade(false, 16));
    }
}

/// A star field drifting past in three layers (parallax): far stars small, dim and slow,
/// near ones bigger, brighter and faster, all twinkling a little. `speed` scales the drift
/// (it's integrated per frame, so changing it never makes the stars jump); `strength` 0…1.
pub fn star_field(ui: &Ui, p: &egui::Painter, rect: Rect, id: &str, speed: f32, strength: f32) {
    let (t, dt) = ui.input(|i| (i.time as f32, i.stable_dt.min(0.1)));
    let key = egui::Id::new(("stars", id));
    let travel = ui.data_mut(|d| {
        let o = d.get_temp_mut_or(key, 0.0f32);
        *o += dt * speed * 16.0;
        *o
    });
    let cool = Color32::from_rgb(140, 200, 255);
    let (w, h) = (rect.width().max(1.0), rect.height().max(1.0));
    for (li, &(depth, n)) in [(0.3f32, 80u32), (0.6, 45), (1.0, 20)].iter().enumerate() {
        for k in 0..n {
            let i = li as u32 * 1000 + k;
            // Drifting left and a little up, wrapping around
            let x = rect.left() + (hash(i) * w - travel * depth).rem_euclid(w);
            let y = rect.top() + (hash(i + 7_777) * h - travel * depth * 0.25).rem_euclid(h);
            let twinkle = 0.65 + 0.35 * (t * (0.7 + hash(i + 33) * 1.8) + hash(i + 99) * std::f32::consts::TAU).sin();
            let col = if k % 7 == 0 { cool } else { TEXT };
            let a = (0.1 + 0.45 * depth) * twinkle * strength;
            p.circle_filled(pos2(x, y), 0.5 + depth * 1.1, alpha(col, a));
        }
    }
}

/// Thin horizontal value bar (track volume, swing, master)
pub fn bar_slider(ui: &mut Ui, value: &mut f32, width: f32, color: Color32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 14.0), Sense::click_and_drag());
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            *value = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            resp.mark_changed();
        }
    }
    let p = ui.painter();
    let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    p.rect_filled(bar, 2.0, PANEL_HI);
    let fill = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * *value, bar.bottom()));
    p.rect_filled(fill, 2.0, alpha(color, if resp.hovered() || resp.dragged() { 1.0 } else { 0.7 }));
    if resp.hovered() || resp.dragged() {
        p.circle_filled(pos2(fill.right(), bar.center().y), 5.0, TEXT);
    }
    resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
}

/// `text` cut with "…" so it fits into `max_w`
pub fn fit_text(p: &egui::Painter, text: &str, font: egui::FontId, max_w: f32) -> String {
    let width = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), TEXT).size().x;
    if width(text) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let t: String = chars.iter().collect::<String>() + "…";
        if width(&t) <= max_w {
            return t;
        }
    }
    String::new()
}

/// The Solarum logo: the lime sun with its rings of light, on black. `tile`: on its rounded
/// square (the app icon), else bare. `time` turns the patches on its surface (0: still).
#[cfg_attr(not(test), allow(dead_code))] // (only the icon test draws it now)
pub fn logo(p: &egui::Painter, r: Rect, tile: bool, time: f32) {
    let s = r.width().min(r.height()) / 100.0;
    let c = r.center();
    if tile {
        let sq = Rect::from_center_size(c, vec2(100.0 * s, 100.0 * s));
        p.rect_filled(sq, 22.0 * s, Color32::from_rgb(4, 4, 6));
        p.rect_stroke(
            sq,
            22.0 * s,
            stroke((1.0 * s).max(0.5), Color32::from_rgb(36, 36, 42)),
            egui::StrokeKind::Inside,
        );
    }
    // Its rings of light, fading out well inside the edge, then the sun itself
    let rs = 22.0 * s;
    for i in (0..5).rev() {
        let k = i as f32 / 4.0;
        // Solid shades of lime into black – crisp rings, like the planets view's glow
        p.circle_filled(
            c,
            rs * (1.12 + k * 0.85),
            mix(Color32::from_rgb(4, 4, 6), ACID, 0.1 + 0.42 * (1.0 - k).powi(2)),
        );
    }
    crate::planets::sun_ball(p, c, rs, ACID, 0.0, 0.0, time);
}

/// Stars flying out of the middle towards you – the start screen's warp
pub fn warp_field(p: &egui::Painter, rect: Rect, center: egui::Pos2, t: f32) {
    let reach = (rect.width().max(rect.height())) * 0.75;
    for i in 0..200u32 {
        let a = hash(i) * std::f32::consts::TAU;
        // Each star's distance cycles: slow in the middle, fast and bright at the edge
        let z = (hash(i + 5_000) + t * (0.05 + 0.06 * hash(i + 9_000))).fract();
        let d = z * z * reach;
        let dir = vec2(a.cos(), a.sin());
        let at = center + dir * (d + 6.0);
        if !rect.expand(4.0).contains(at) {
            continue;
        }
        let v = (z * 1.3).min(1.0);
        let level = (30.0 + 150.0 * v) as u8;
        // Exact premultiplied white: a crisp light, never a grey smudge
        let col = Color32::from_rgba_premultiplied(level, level, level, level);
        let tail = 0.5 + z * z * z * 9.0;
        p.line_segment([at - dir * tail, at], stroke(0.7 + z * 0.9, col));
    }
}
