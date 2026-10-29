//! The pads of the right panel: the character pad (dark × acid, a ring for space), the EQ
//! curve with three points to drag, and the filter pad (low-pass ← off → high-pass × resonance)

use crate::theme::*;
use crate::widgets::glow;
use eframe::egui::{self, pos2, vec2, Align2, Color32, Pos2, Rect, Response, Sense, Ui};
use egui_phosphor::regular as ph;

/// The low-pass and high-pass sides of the filter
pub const LOW_PASS: Color32 = Color32::from_rgb(69, 200, 255);
pub const HIGH_PASS: Color32 = Color32::from_rgb(255, 154, 60);

fn pad_frame(p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 12.0, Color32::from_black_alpha(80));
    p.rect_stroke(rect, 12.0, stroke(1.0, LINE), egui::StrokeKind::Inside);
    for k in 1..4 {
        let x = rect.left() + rect.width() * k as f32 / 4.0;
        let y = rect.top() + rect.height() * k as f32 / 4.0;
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], stroke(1.0, alpha(LINE, 0.5)));
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], stroke(1.0, alpha(LINE, 0.5)));
    }
}

fn dashed(p: &egui::Painter, a: Pos2, b: Pos2, color: Color32) {
    p.add(egui::Shape::dashed_line(&[a, b], stroke(1.0, color), 4.0, 4.0));
}

/// Dark (up) × acid (right) in one pad: drag anywhere, double-click resets. `shown_acid` is
/// the acid as the engine plays it (the auto mode sweeps it) – the puck follows it.
pub fn character(ui: &mut Ui, dark: &mut f32, acid: &mut f32, def: [f32; 2], shown_acid: Option<f32>, width: f32, height: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    if resp.double_clicked() {
        [*dark, *acid] = def;
        resp.mark_changed();
    } else if let Some(pos) = resp
        .interact_pointer_pos()
        .filter(|_| resp.is_pointer_button_down_on() || resp.dragged())
    {
        let a = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let d = ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0);
        if (a - *acid).abs() + (d - *dark).abs() > 1e-4 {
            (*acid, *dark) = (a, d);
            resp.mark_changed();
        }
    }

    let p = ui.painter_at(rect.expand(2.0));
    pad_frame(&p, rect);
    let held = resp.is_pointer_button_down_on() || resp.dragged();
    let shown = if held { *acid } else { shown_acid.unwrap_or(*acid) };
    let at = pos2(rect.left() + shown * rect.width(), rect.bottom() - *dark * rect.height());
    dashed(&p, pos2(rect.left(), at.y), pos2(rect.right(), at.y), alpha(TEXT, 0.25));
    dashed(&p, pos2(at.x, rect.top()), pos2(at.x, rect.bottom()), alpha(TEXT, 0.25));
    let pad = vec2(10.0, 8.0);
    let dark_col = track_color(crate::song::Voice::Ride);
    p.text(
        rect.left_top() + pad,
        Align2::LEFT_TOP,
        format!("DARK {}", ph::ARROW_UP),
        bold(10.5),
        dark_col,
    );
    p.text(
        rect.right_top() + vec2(-pad.x, pad.y),
        Align2::RIGHT_TOP,
        "dirty",
        body(10.5),
        FAINT,
    );
    p.text(
        rect.left_bottom() + vec2(pad.x, -pad.y),
        Align2::LEFT_BOTTOM,
        "clean",
        body(10.5),
        FAINT,
    );
    p.text(
        rect.right_bottom() - pad,
        Align2::RIGHT_BOTTOM,
        format!("ACID {}", ph::ARROW_RIGHT),
        bold(10.5),
        ACID,
    );
    let hot = resp.hovered() || resp.dragged();
    // The same puck as the filter pad's
    if hot {
        glow(&p, at, 11.0, TEXT, 0.25);
    }
    p.circle_filled(at, 11.0, mix(PANEL, BG, 0.5));
    p.circle_stroke(at, 11.0, stroke(2.0, TEXT));
    resp.on_hover_cursor(egui::CursorIcon::Crosshair)
}

/// The master EQ as a curve: three points (low, mid, high) to drag up or down, ±`song::EQ_DB`;
/// double-click a point to flatten it
pub fn eq_curve(ui: &mut Ui, bands: &mut [f32; 3], colors: [Color32; 3], width: f32, height: f32) -> Response {
    let (all, mut resp) = ui.allocate_exact_size(vec2(width, height + 18.0), Sense::click_and_drag());
    // One card like the others: the points above, their names along the bottom
    let rect = Rect::from_min_max(all.min + vec2(0.0, 2.0), pos2(all.right(), all.bottom() - 18.0));
    let xs = [1.0 / 6.0, 0.5, 5.0 / 6.0].map(|f| rect.left() + f * rect.width());
    let span = rect.height() / 2.0 - 8.0;
    let y_of = |v: f32| rect.center().y - (v - 0.5) * 2.0 * span;
    let nearest = |x: f32| (0..3).min_by(|&a, &b| (xs[a] - x).abs().total_cmp(&(xs[b] - x).abs())).unwrap_or(1);
    // Which point the hand has (it stays the same through a drag)
    let id = resp.id.with("band");
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.double_clicked() {
            bands[nearest(pos.x)] = 0.5;
            resp.mark_changed();
        } else if resp.is_pointer_button_down_on() || resp.dragged() {
            let k = if resp.drag_started() || resp.is_pointer_button_down_on() && !resp.dragged() {
                let k = nearest(pos.x);
                ui.data_mut(|d| d.insert_temp(id, k));
                k
            } else {
                ui.data(|d| d.get_temp(id)).unwrap_or_else(|| nearest(pos.x))
            };
            let v = (0.5 + (rect.center().y - pos.y) / span / 2.0).clamp(0.0, 1.0);
            if (v - bands[k]).abs() > 1e-4 {
                bands[k] = v;
                resp.mark_changed();
            }
        }
    }

    let p = ui.painter_at(all.expand(2.0));
    p.rect_filled(all, 10.0, Color32::from_black_alpha(80));
    p.rect_stroke(all, 10.0, stroke(1.0, LINE), egui::StrokeKind::Inside);
    dashed(
        &p,
        pos2(rect.left() + 8.0, rect.center().y),
        pos2(rect.right() - 8.0, rect.center().y),
        alpha(LINE, 0.9),
    );
    // The response: a low shelf, a bell and a high shelf added up
    let g = bands.map(|v| (v - 0.5) * 2.0);
    let curve: Vec<Pos2> = (0..=64)
        .map(|i| {
            let f = i as f32 / 64.0;
            let x = rect.left() + 4.0 + f * (rect.width() - 8.0);
            let s = |d: f32| 1.0 / (1.0 + (-d / 0.06).exp());
            let gain = g[0] * s(1.0 / 6.0 + 0.1 - f) + g[1] * (-((f - 0.5) / 0.16).powi(2)).exp() + g[2] * s(f - 5.0 / 6.0 + 0.1);
            pos2(x, rect.center().y - gain.clamp(-1.0, 1.0) * span)
        })
        .collect();
    p.add(egui::Shape::line(curve, stroke(2.0, alpha(TEXT, 0.55))));
    let hot = resp.hovered() || resp.dragged();
    let names = ["LOW", "MID", "HIGH"];
    for k in 0..3 {
        let at = pos2(xs[k], y_of(bands[k]));
        if hot {
            glow(&p, at, 7.0, colors[k], 0.5);
        }
        p.circle_filled(at, 7.0, colors[k]);
        // The name alone while flat, with its dB once it's moved
        let db = ((bands[k] - 0.5) * 2.0 * crate::song::EQ_DB).round();
        let name = if db == 0.0 {
            names[k].to_string()
        } else {
            format!("{} {db:+.0}", names[k])
        };
        p.text(
            pos2(xs[k], all.bottom() - 11.0),
            Align2::CENTER_CENTER,
            name,
            bold(10.0),
            if db != 0.0 {
                colors[k]
            } else if hot {
                TEXT
            } else {
                DIM
            },
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::ResizeVertical)
}

/// The filter as a pad: left low-pass, right high-pass, the middle off; up adds resonance.
/// Let go and it springs back to off.
/// `engine_pos` is where the engine's filter is (smoothed, or swept by a breakdown) – the
/// puck shows it while no hand is on the pad.
pub fn filter_pad(ui: &mut Ui, pos: &mut f32, res: &mut f32, engine_pos: f32, width: f32, height: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let held = resp.is_pointer_button_down_on() || resp.dragged();
    let rest_y = rect.center().y + rect.height() * 0.15;
    if let Some(at) = resp.interact_pointer_pos().filter(|_| held) {
        *pos = ((at.x - rect.center().x) / (rect.width() / 2.0 - 12.0)).clamp(-1.0, 1.0);
        *res = ((rest_y - at.y) / (rest_y - rect.top() - 12.0)).clamp(0.0, 1.0);
        resp.mark_changed();
    } else if pos.abs() > 0.0 || *res > 0.0 {
        // Spring back
        *pos *= 0.75;
        *res *= 0.75;
        if pos.abs() < 0.01 {
            *pos = 0.0;
        }
        if *res < 0.01 {
            *res = 0.0;
        }
        resp.mark_changed();
        ui.ctx().request_repaint();
    }

    let p = ui.painter_at(rect.expand(2.0));
    pad_frame(&p, rect);
    dashed(
        &p,
        pos2(rect.center().x, rect.top() + 22.0),
        pos2(rect.center().x, rect.bottom()),
        alpha(TEXT, 0.25),
    );
    let color = if (if held || pos.abs() > 0.01 { *pos } else { engine_pos }) < 0.0 {
        LOW_PASS
    } else {
        HIGH_PASS
    };
    let pad = vec2(10.0, 8.0);
    p.text(rect.center_top() + vec2(0.0, pad.y), Align2::CENTER_TOP, "OFF", bold(10.0), FAINT);
    p.text(
        rect.left_top() + pad,
        Align2::LEFT_TOP,
        format!("RES {}", ph::ARROW_UP),
        bold(10.0),
        DIM,
    );
    p.text(
        rect.left_bottom() + vec2(pad.x, -pad.y),
        Align2::LEFT_BOTTOM,
        format!("{} LOW-PASS", ph::ARROW_LEFT),
        bold(10.0),
        if *pos < -0.01 { LOW_PASS } else { DIM },
    );
    p.text(
        rect.right_bottom() - pad,
        Align2::RIGHT_BOTTOM,
        format!("HIGH-PASS {}", ph::ARROW_RIGHT),
        bold(10.0),
        if *pos > 0.01 { HIGH_PASS } else { DIM },
    );
    let shown = if held || pos.abs() > 0.01 { *pos } else { engine_pos };
    let at = pos2(
        rect.center().x + shown * (rect.width() / 2.0 - 12.0),
        rest_y - *res * (rest_y - rect.top() - 12.0),
    );
    let amount = shown.abs().max(*res);
    if shown.abs() > 0.01 {
        p.line_segment([pos2(rect.center().x, at.y), at], stroke(3.0, alpha(color, 0.6)));
    }
    glow(&p, at, 11.0, color, amount);
    p.circle_filled(at, 11.0, mix(PANEL, BG, 0.5));
    p.circle_stroke(at, 11.0, stroke(2.0, if amount > 0.01 { color } else { TEXT }));
    resp.on_hover_cursor(egui::CursorIcon::Grab)
}

/// A card's frame and its label row; returns the picture area
pub fn card(p: &egui::Painter, rect: Rect, label: &str, value: &str, color: Color32, hot: bool) -> Rect {
    p.rect_filled(rect, 10.0, Color32::from_black_alpha(80));
    p.rect_stroke(
        rect,
        10.0,
        stroke(1.0, if hot { alpha(color, 0.7) } else { LINE }),
        egui::StrokeKind::Inside,
    );
    let row = rect.bottom() - 11.0;
    p.text(
        pos2(rect.left() + 8.0, row),
        Align2::LEFT_CENTER,
        label,
        bold(10.0),
        if hot { TEXT } else { DIM },
    );
    p.text(pos2(rect.right() - 8.0, row), Align2::RIGHT_CENTER, value, bold(11.0), color);
    Rect::from_min_max(rect.min + vec2(8.0, 8.0), pos2(rect.right() - 8.0, rect.bottom() - 22.0))
}

/// What a 0…1 value sounds like, in a word: the first for 0, the others share the rest.
/// While it's dragged the number shows instead, for the fine-tuning
fn in_words(resp: &Response, value: f32, words: &[&str]) -> String {
    if resp.dragged() {
        format!("{:.0}", value * 100.0)
    } else if value < 0.005 {
        words[0].to_string()
    } else {
        let n = words.len() - 1;
        words[1 + ((value * n as f32) as usize).min(n - 1)].to_string()
    }
}

/// Drag up or down (or scroll) to change a 0…1 value, double-click resets
pub fn drag_value(ui: &Ui, resp: &mut Response, value: &mut f32, default: f32) {
    let mut v = *value;
    if resp.double_clicked() {
        v = default;
    } else {
        if resp.dragged() {
            v -= resp.drag_delta().y / 120.0;
        }
        if resp.hovered() {
            v += ui.input(|i| i.raw_scroll_delta.y) / 400.0;
        }
    }
    let v = v.clamp(0.0, 1.0);
    if (v - *value).abs() > 1e-5 {
        *value = v;
        resp.mark_changed();
    }
}

/// SPACE as the room it makes: echoes rippling out, wider and longer the more
pub fn space_card(ui: &mut Ui, value: &mut f32, default: f32, color: Color32, width: f32, height: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    drag_value(ui, &mut resp, value, default);
    let p = ui.painter_at(rect);
    let hot = resp.hovered() || resp.dragged();
    let area = card(
        &p,
        rect,
        "SPACE",
        &in_words(&resp, *value, &["dry", "room", "hall", "cave"]),
        color,
        hot,
    );
    let pic = area;
    let c = pos2(pic.left() + 6.0, pic.center().y);
    p.circle_filled(c, 3.5, color);
    // Rings like echoes: more of them, further out and slower to fade with more space
    let reach = pic.width() - 8.0;
    let n = 1 + (*value * 4.0).round() as usize;
    for k in 0..n {
        let f = (k + 1) as f32 / 5.0;
        let r = 6.0 + f * reach * (0.35 + 0.65 * *value);
        let fade = (1.0 - k as f32 / 5.0) * (0.35 + 0.5 * *value);
        // Only the right half: sound going out into the room
        let pts: Vec<Pos2> = (0..=24)
            .map(|i| {
                let a = -1.2 + 2.4 * i as f32 / 24.0;
                c + vec2(a.cos(), a.sin()) * r
            })
            .filter(|q| pic.expand(1.0).contains(*q))
            .collect();
        if pts.len() > 1 {
            p.add(egui::Shape::line(pts, stroke(1.5, alpha(color, fade))));
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::ResizeVertical)
}

/// PUMP as what it does: the mix ducking under four kicks and breathing back
pub fn pump_card(ui: &mut Ui, value: &mut f32, default: f32, color: Color32, width: f32, height: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    drag_value(ui, &mut resp, value, default);
    let p = ui.painter_at(rect);
    let hot = resp.hovered() || resp.dragged();
    let area = card(
        &p,
        rect,
        "PUMP",
        &in_words(&resp, *value, &["off", "soft", "breathing", "pumping"]),
        color,
        hot,
    );
    let depth = *value * 0.9;
    let pts: Vec<Pos2> = (0..=80)
        .map(|i| {
            let f = i as f32 / 80.0;
            let phase = (f * 4.0).fract();
            let env = 1.0 - depth * (-phase * 5.0).exp();
            pos2(area.left() + f * area.width(), area.top() + (1.0 - env) * area.height())
        })
        .collect();
    // A soft fill where the mix dips, column by column (premultiplied: `alpha` would glare) –
    // with no pump nothing dips and the card lies still
    for pt in pts.iter().step_by(2) {
        p.line_segment([pos2(pt.x, area.top()), *pt], stroke(1.5, color.gamma_multiply(0.22)));
    }
    let ink = if *value < 0.005 { alpha(color, 0.35) } else { color };
    p.add(egui::Shape::line(pts, stroke(2.0, ink)));
    for k in 0..4 {
        let x = area.left() + k as f32 / 4.0 * area.width();
        p.circle_filled(pos2(x, area.bottom()), 2.0, alpha(TEXT, 0.5));
    }
    resp.on_hover_cursor(egui::CursorIcon::ResizeVertical)
}

/// DRIVE as what it does: a wave pushed into saturation, the clean one behind it
pub fn drive_card(ui: &mut Ui, value: &mut f32, default: f32, color: Color32, width: f32, height: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    drag_value(ui, &mut resp, value, default);
    let p = ui.painter_at(rect);
    let hot = resp.hovered() || resp.dragged();
    let area = card(
        &p,
        rect,
        "DRIVE",
        &in_words(&resp, *value, &["clean", "warm", "hot", "fuzz"]),
        color,
        hot,
    );
    let k = 1.0 + *value * 9.0;
    let wave = |shape: &dyn Fn(f32) -> f32| -> Vec<Pos2> {
        (0..=80)
            .map(|i| {
                let f = i as f32 / 80.0;
                let s = (f * std::f32::consts::TAU * 1.5).sin();
                pos2(area.left() + f * area.width(), area.center().y - shape(s) * area.height() / 2.0)
            })
            .collect()
    };
    p.add(egui::Shape::line(wave(&|s| s), stroke(1.0, alpha(TEXT, 0.18))));
    p.add(egui::Shape::line(wave(&|s| (k * s).tanh() / k.tanh()), stroke(2.0, color)));
    resp.on_hover_cursor(egui::CursorIcon::ResizeVertical)
}

/// KEY as a slider (C … B): the key the whole song is in – bass and synth move with it, the
/// notes of the planets stay the same steps of the scale; drag or click along it (or scroll),
/// double-click resets
pub fn pitch_card(ui: &mut Ui, semis: &mut u8, default: u8, label: &str, color: Color32, width: f32, height: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    let hot = resp.hovered() || resp.dragged();
    let area = card(&p, rect, "KEY", label, color, hot);
    let sw = area.width() / 12.0;
    let step_at = |x: f32| (((x - area.left()) / sw).floor() as i32).clamp(0, 11) as u8;
    let mut picked = *semis;
    if resp.double_clicked() {
        picked = default;
    } else if let Some(at) = resp
        .interact_pointer_pos()
        .filter(|_| resp.is_pointer_button_down_on() || resp.dragged())
    {
        picked = step_at(at.x);
    } else if resp.hovered() {
        let id = resp.id.with("scroll");
        let mut acc: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0) + ui.input(|i| i.raw_scroll_delta.y);
        while acc.abs() >= 30.0 {
            picked = (picked as i32 + acc.signum() as i32).clamp(0, 11) as u8;
            acc -= 30.0 * acc.signum();
        }
        ui.data_mut(|d| d.insert_temp(id, acc));
    }
    if picked != *semis {
        *semis = picked;
        resp.mark_changed();
    }
    // A slider: a line with a tick per key, the sharps shorter, a knob on the one picked
    let y = area.center().y + 2.0;
    let x_of = |k: u8| area.left() + (k as f32 + 0.5) * sw;
    p.line_segment([pos2(x_of(0), y), pos2(x_of(11), y)], stroke(2.0, Color32::from_white_alpha(30)));
    p.line_segment([pos2(x_of(0), y), pos2(x_of(*semis), y)], stroke(2.5, color));
    for k in 0..12u8 {
        let sharp = matches!(k, 1 | 3 | 6 | 8 | 10);
        let h = if sharp { 3.0 } else { 6.0 };
        p.line_segment(
            [pos2(x_of(k), y - h), pos2(x_of(k), y + h)],
            stroke(1.0, Color32::from_white_alpha(if k <= *semis { 90 } else { 40 })),
        );
    }
    let knob = pos2(x_of(*semis), y);
    if hot {
        glow(&p, knob, 6.0, color, 0.6);
    }
    p.circle_filled(knob, 7.0, TEXT);
    p.circle_filled(knob, 3.0, color);
    resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
}

/// The picture on an effect button (in `Fx::ALL` order), moving with `t`: stutter repeats a
/// slice, echo fades out in repeats, wash smears waves into each other, crush steps a wave,
/// gate chops it, tape stop slows it down
/// `boost` (0…1) while the effect is held: everything bigger and bolder.
pub fn fx_art(p: &egui::Painter, r: Rect, kind: usize, ink: Color32, t: f32, boost: f32) {
    let line = |pts: Vec<Pos2>, w: f32, c: Color32| {
        p.add(egui::Shape::line(pts, stroke(w + boost, c)));
    };
    let (cy, amp) = (r.center().y, r.height() / 2.0 * (1.0 + 0.3 * boost));
    let wave = |f: &dyn Fn(f32) -> f32, n: usize| -> Vec<Pos2> {
        (0..=n)
            .map(|i| {
                let x = i as f32 / n as f32;
                pos2(r.left() + x * r.width(), cy - f(x).clamp(-1.0, 1.0) * amp)
            })
            .collect()
    };
    match kind {
        // Stutter: the same little slice, over and over
        0 => {
            let n = 6;
            let w = r.width() / n as f32;
            let hit = (t * 3.0).fract();
            for k in 0..n {
                let x0 = r.left() + k as f32 * w;
                let lit = if (k as f32 / n as f32 - hit).abs() < 0.17 { 1.0 } else { 0.6 };
                let pts: Vec<Pos2> = (0..=12)
                    .map(|i| {
                        let f = i as f32 / 12.0;
                        let y = (f * 18.0).sin() * (1.0 - f).powi(2);
                        pos2(x0 + 2.0 + f * (w - 4.0), cy - y * amp)
                    })
                    .collect();
                line(pts, 2.0, ink.gamma_multiply(lit));
            }
        }
        // Echo: a hit and its repeats, each softer, travelling
        1 => {
            let shift = (t * 0.8).fract();
            for k in 0..7 {
                let f = (k as f32 + shift) / 7.0;
                let x = r.left() + f * r.width();
                let h = amp * 0.62f32.powf(k as f32 + shift) * 2.0;
                let c = ink.gamma_multiply((1.0 - f * 0.7).max(0.2));
                p.line_segment([pos2(x, cy - h / 2.0), pos2(x, cy + h / 2.0)], stroke(3.0, c));
            }
        }
        // Wash: three waves smearing into each other
        2 => {
            for k in 0..3 {
                let ph = t * (0.7 + k as f32 * 0.3) + k as f32 * 2.1;
                let a = 0.55 - k as f32 * 0.12;
                let pts = wave(&|x| a * (x * 9.0 + ph).sin() + 0.25 * (x * 4.0 - ph * 0.7).sin(), 48);
                line(pts, 2.5 - k as f32 * 0.5, ink.gamma_multiply(1.0 - k as f32 * 0.25));
            }
        }
        // Crush: a wave in stairs
        3 => {
            let steps = 14;
            let mut pts = Vec::new();
            for i in 0..steps {
                let x0 = i as f32 / steps as f32;
                let x1 = (i + 1) as f32 / steps as f32;
                let y = ((x0 * 12.0 + t * 2.0).sin() * 3.0).round() / 3.0;
                let yy = cy - y * amp * 0.9;
                pts.push(pos2(r.left() + x0 * r.width(), yy));
                pts.push(pos2(r.left() + x1 * r.width(), yy));
            }
            line(pts, 2.5, ink);
        }
        // Gate: the sound chopped into sixteenths
        4 => {
            let n = 12;
            let w = r.width() / n as f32;
            let run = (t * 2.0) as usize;
            for k in 0..n {
                let open = (k + run) % 3 != 2;
                if open {
                    let h = amp * (1.4 + 0.5 * ((k as f32 * 1.7 + t).sin()));
                    let b = Rect::from_center_size(pos2(r.left() + (k as f32 + 0.5) * w, cy), vec2(w * 0.62, h.min(r.height())));
                    p.rect_filled(b, 2.0, ink);
                }
            }
        }
        // Tape stop: a wave that slows down and sinks
        _ => {
            let ph = t * 3.0;
            let pts = wave(&|x| (1.0 - 0.55 * x) * ((1.0 - x).powf(0.6) * 26.0 + ph).sin() - 0.25 * x * x, 64);
            line(pts, 2.5, ink);
        }
    }
}

/// The burst when an effect is pressed: its outline flies out of the button and fades,
/// with a flash inside (`k` 0…1 through the burst)
pub fn fx_burst(p: &egui::Painter, r: Rect, color: Color32, k: f32) {
    let e = 1.0 - (1.0 - k).powi(3);
    for (i, spread) in [(0, 22.0), (1, 12.0)] {
        let a = (1.0 - k) * if i == 0 { 0.9 } else { 0.5 };
        p.rect_stroke(
            r.expand(2.0 + e * spread),
            12.0 + e * spread * 0.5,
            stroke(2.5 - i as f32, color.gamma_multiply(a)),
            egui::StrokeKind::Outside,
        );
    }
    if k < 0.3 {
        p.rect_filled(r, 12.0, Color32::from_white_alpha((60.0 * (1.0 - k / 0.3)) as u8));
    }
}

/// A visualizer's button in the menu: a little picture of what it shows and its name
pub fn visualizer_card(p: &egui::Painter, r: Rect, face: bool, hot: bool, t: f32) {
    // Everything stays inside the card, the glows too
    let p = &p.with_clip_rect(r);
    let color = if face { RED } else { ACID };
    p.rect_filled(r, 12.0, SPACE);
    p.rect_stroke(
        r,
        12.0,
        stroke(if hot { 2.0 } else { 1.0 }, if hot { color } else { LINE }),
        egui::StrokeKind::Inside,
    );
    let pic = Rect::from_min_max(r.min + vec2(10.0, 10.0), pos2(r.right() - 10.0, r.bottom() - 34.0));
    let c = pic.center();
    // A few stars
    for i in 0..18u32 {
        let h = |k: u32| ((i.wrapping_mul(2_654_435_761) ^ k.wrapping_mul(40_503)) % 1000) as f32 / 1000.0;
        p.circle_filled(
            pos2(pic.left() + h(1) * pic.width(), pic.top() + h(2) * pic.height()),
            0.6 + h(3) * 0.8,
            Color32::from_white_alpha(40),
        );
    }
    let move_t = if hot { t } else { 0.0 };
    if face {
        // A low-poly head: outline, eye holes, a slit for a mouth, a few facets
        let s = pic.height() * 0.46;
        let nod = (move_t * 4.0).sin() * 2.0;
        let pt = |x: f32, y: f32| c + vec2(x * s, y * s + nod);
        let outline = [
            pt(-0.55, -0.7),
            pt(0.0, -0.95),
            pt(0.55, -0.7),
            pt(0.7, -0.1),
            pt(0.5, 0.55),
            pt(0.0, 0.85),
            pt(-0.5, 0.55),
            pt(-0.7, -0.1),
        ];
        glow(p, c, s * 0.9, RED, 0.35);
        p.add(egui::Shape::convex_polygon(
            outline.to_vec(),
            mix(SPACE, RED, 0.25),
            stroke(1.5, RED),
        ));
        for (a, b) in [(0, 4), (2, 6), (1, 5), (7, 3)] {
            p.line_segment([outline[a], outline[b]], stroke(0.8, RED.gamma_multiply(0.45)));
        }
        for x in [-0.28, 0.28] {
            let eye = vec![pt(x - 0.16, -0.2), pt(x + 0.16, -0.24), pt(x + 0.12, -0.05), pt(x - 0.12, -0.04)];
            p.add(egui::Shape::convex_polygon(eye, SPACE, stroke(1.0, RED)));
        }
        p.line_segment([pt(-0.25, 0.35), pt(0.25, 0.35)], stroke(3.0, SPACE));
    } else {
        // LIGHTS: a ring of spectrum bars, a shockwave, the core
        let rr = pic.height() * 0.28;
        let colors = [
            ACID,
            Color32::from_rgb(69, 200, 255),
            Color32::from_rgb(255, 154, 60),
            Color32::from_rgb(140, 125, 255),
        ];
        for i in 0..40 {
            let a = i as f32 / 40.0 * std::f32::consts::TAU;
            let len = 6.0 + 14.0 * (0.5 + 0.5 * (i as f32 * 1.7 + move_t * 6.0).sin()).powi(2);
            let dir = vec2(a.cos(), a.sin());
            p.line_segment([c + dir * rr, c + dir * (rr + len)], stroke(2.0, colors[i % 4]));
        }
        let wave = rr + 26.0 + (move_t * 1.5).fract() * 20.0;
        p.circle_stroke(c, wave, stroke(1.5, ACID.gamma_multiply(0.35)));
        glow(p, c, rr * 0.8, ACID, 0.5);
        p.circle_filled(c, rr * 0.45, ACID);
    }
    p.text(
        pos2(r.left() + 12.0, r.bottom() - 12.0),
        Align2::LEFT_BOTTOM,
        if face { "FACE" } else { "LIGHTS" },
        bold(14.0),
        if hot { color } else { TEXT },
    );
    p.text(
        pos2(r.right() - 12.0, r.bottom() - 13.0),
        Align2::RIGHT_BOTTOM,
        if face { "O" } else { "P" },
        bold(10.0),
        DIM,
    );
}
