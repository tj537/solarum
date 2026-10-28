//! The SOUNDS screen: your library on the left (filter by track, the project's eight slots), the
//! editor on the right – a waveform you cut by dragging its edges and fades, the track it plays
//! on as a row of colours, and picture tiles for what you can do to it: pitch, reverse, tone,
//! crush, trim and chop (slices you play as pads). Plus importing and recording.

use crate::library::{self, Edit, Entry, Library, Recorder, SlotIds, Source, BLOCK, MAX_SECONDS};
use crate::sampler::{SampleBuf, SAMPLE_SLOTS, SAMPLE_SR};
use crate::song::Voice;
use crate::theme::*;
use crate::widgets::{chip, fit_text};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, UiBuilder};
use egui_phosphor::regular as ph;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::Arc;

/// What the screen asks the app to do
pub enum Out {
    Back,
    /// Play this (looped or once); `None` stops
    Audition(Option<Arc<SampleBuf>>, bool),
    /// Put slot `k` on its track now
    Assign(Voice, usize),
    /// Give this sound a slot and put it on its track
    Use(u64),
}

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Start,
    End,
    FadeIn,
    FadeOut,
    Slice(usize),
    Pan,
}

struct Playing {
    id: u64,
    buf: Arc<SampleBuf>,
    /// Where in the source the buffer starts and ends (seconds) and whether it runs backwards
    from: f32,
    to: f32,
    reverse: bool,
    /// The whole sound (follows edits while it loops) or one slice
    whole: bool,
    since: f64,
}

pub struct LibraryUi {
    pub open: bool,
    selected: Option<u64>,
    filter: Option<Voice>,
    search: String,
    /// Seconds of the source in view, and for which sound
    view: (f32, f32),
    view_for: u64,
    drag: Option<Handle>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    looped: bool,
    playing: Option<Playing>,
    /// How eagerly CHOP finds hits (dragging the tile)
    sensitivity: f32,
    /// A sound waiting for a slot (all eight are taken)
    replace: Option<u64>,
    confirm_delete: Option<u64>,
    record_panel: bool,
    recorder: Option<Recorder>,
    inputs: Vec<String>,
    input: Option<String>,
    message: Option<(String, f64)>,
    unsaved: bool,
}

impl Default for LibraryUi {
    fn default() -> Self {
        LibraryUi {
            open: false,
            selected: None,
            filter: None,
            search: String::new(),
            view: (0.0, 1.0),
            view_for: 0,
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            looped: false,
            playing: None,
            sensitivity: 0.5,
            replace: None,
            confirm_delete: None,
            record_panel: false,
            recorder: None,
            inputs: Vec::new(),
            input: None,
            message: None,
            unsaved: false,
        }
    }
}

fn fmt_secs(s: f32) -> String {
    if s < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else {
        format!("{s:.2} s")
    }
}

/// The edit's kept part in seconds
fn kept(src: &Source, e: &Edit) -> (f32, f32) {
    let (a, b) = library::span(src.data.len(), e);
    (a as f32 / SAMPLE_SR, b as f32 / SAMPLE_SR)
}

/// The edges of the slices: start, the markers in between, end
fn edges(e: &Edit, ks: f32, ke: f32, slices: &[f32]) -> Vec<f32> {
    let _ = e;
    let mut v = vec![ks];
    let mut m: Vec<f32> = slices.iter().copied().filter(|&m| m > ks && m < ke).collect();
    m.sort_by(f32::total_cmp);
    v.extend(m);
    v.push(ke);
    v
}

/// A waveform of `src` from `from` to `to` seconds into `r`
fn draw_wave(p: &egui::Painter, r: Rect, src: &Source, from: f32, to: f32, color: Color32) {
    let w = r.width().max(1.0) as usize;
    let span = (to - from).max(1e-6);
    let mid = r.center().y;
    let h = r.height() * 0.48;
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

// ───────────── Picture tiles ─────────────

#[derive(Clone, Copy, PartialEq)]
enum Tile {
    Pitch,
    Reverse,
    Tone,
    Crush,
    Trim,
    Chop,
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
fn picture(p: &egui::Painter, r: Rect, tile: Tile, v: f32, col: Color32, t: f32) {
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
        // A sound with brackets closing in on it
        Tile::Trim => {
            let inner = Rect::from_min_max(r.min + vec2(r.width() * 0.22, 0.0), r.max - vec2(r.width() * 0.22, 0.0));
            curve(p, r, faint, |x| (x * TAU * 10.0).sin() * 0.06);
            curve(p, inner, s, |x| (x * TAU * 8.0).sin() * (-x * 4.0).exp() * 0.9);
            for x in [inner.left(), inner.right()] {
                p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], stroke(2.0, TEXT));
            }
        }
        // Hits with cuts between them – as many as it finds
        Tile::Chop => {
            let n = v.round().max(1.0) as usize;
            curve(p, r, s, |x| {
                let k = (x * n as f32).fract();
                (x * TAU * 6.0 * n as f32).sin() * (-k * 5.0).exp() * 0.9
            });
            for i in 1..n {
                let x = r.left() + r.width() * i as f32 / n as f32;
                p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], stroke(1.2, ACID));
            }
        }
    }
}

/// A value as the sound section's cards show it (SPACE, PUMP, DRIVE): drag up or down or
/// scroll, double-click resets; the picture shows what it does. `v` is 0…1, `shown` the
/// picture's own value.
#[allow(clippy::too_many_arguments)]
fn value_card(
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
fn action_button(
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
fn ui_scrolled(r: &egui::Response) -> bool {
    r.ctx.input(|i| i.raw_scroll_delta.y != 0.0)
}

/// The tone tile's value from the edit: -1 (dark, high cut down) … 0 (open) … 1 (thin, low cut up)
fn tone_of(e: &Edit) -> f32 {
    if e.low_cut > 25.0 {
        ((e.low_cut / 20.0).ln() / 100f32.ln()).clamp(0.0, 1.0)
    } else if e.high_cut < 19_000.0 {
        -(1.0 - (e.high_cut / 300.0).ln() / (19_000f32 / 300.0).ln()).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn set_tone(e: &mut Edit, v: f32) {
    let v = if v.abs() < 0.04 { 0.0 } else { v.clamp(-1.0, 1.0) };
    e.low_cut = if v > 0.0 { 20.0 * 100f32.powf(v) } else { 20.0 };
    e.high_cut = if v < 0.0 {
        300.0 * (19_000f32 / 300.0).powf(1.0 + v)
    } else {
        20_000.0
    };
}

impl LibraryUi {
    /// Open the screen, filtered to a track (from its sound menu) or showing everything
    pub fn show_for(&mut self, voice: Option<Voice>) {
        self.open = true;
        self.filter = voice;
    }

    pub fn select(&mut self, id: u64) {
        self.selected = Some(id);
    }

    /// All eight slots are playing: open the screen and ask which one this sound replaces
    pub fn ask_replace(&mut self, id: u64) {
        self.open = true;
        self.selected = Some(id);
        self.replace = Some(id);
    }

    fn say(&mut self, text: impl Into<String>, now: f64) {
        self.message = Some((text.into(), now));
    }

    fn checkpoint(&mut self, e: &Entry) {
        if self.undo.last() == Some(e) {
            return;
        }
        self.undo.push(e.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Bring audio files into the library
    pub fn import(&mut self, lib: &mut Library, paths: &[PathBuf], now: f64) {
        let mut added = 0;
        let mut last_err = None;
        for path in paths {
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            match library::decode_file(path) {
                Ok(data) => {
                    let voice = self.filter.unwrap_or_else(|| library::guess_voice(&name));
                    let long = data.len() as f32 >= MAX_SECONDS * SAMPLE_SR - 1.0;
                    let id = lib.add(&name, voice, data);
                    self.select(id);
                    added += 1;
                    if long {
                        last_err = Some(format!("{name}: kept the first {MAX_SECONDS:.0} seconds"));
                    }
                }
                Err(e) => last_err = Some(format!("{name}: {e}")),
            }
        }
        match (added, last_err) {
            (_, Some(e)) => self.say(e, now),
            (1, None) => self.say("Imported – trimmed and levelled", now),
            (n, None) if n > 1 => self.say(format!("Imported {n} sounds"), now),
            _ => {}
        }
    }

    fn play(&mut self, lib: &mut Library, id: u64, region: Option<(f32, f32)>, out: &mut Vec<Out>, now: f64) {
        let Some(e) = lib.get(id).cloned() else { return };
        let Some(src) = lib.source(&e).cloned() else { return };
        let (buf, from, to, whole) = match region {
            Some((a, b)) => {
                let edit = Edit {
                    start: a,
                    end: b,
                    fade_in: 0.0,
                    fade_out: 0.004,
                    ..e.edit
                };
                let buf = Arc::new(SampleBuf {
                    data: library::render(&src.data, &edit),
                    pitch: e.edit.pitch,
                });
                (buf, a, b, false)
            }
            None => {
                let Some(buf) = lib.sound(id) else { return };
                let (a, b) = kept(&src, &e.edit);
                (buf, a, b, true)
            }
        };
        out.push(Out::Audition(Some(buf.clone()), self.looped && whole));
        self.playing = Some(Playing {
            id,
            buf,
            from,
            to,
            reverse: e.edit.reverse,
            whole,
            since: now,
        });
    }

    fn stop(&mut self, out: &mut Vec<Out>) {
        if self.playing.take().is_some() {
            out.push(Out::Audition(None, false));
        }
    }

    fn toggle_play(&mut self, lib: &mut Library, id: u64, out: &mut Vec<Out>, now: f64) {
        if self.playing.as_ref().is_some_and(|pl| pl.id == id && pl.whole) {
            self.stop(out);
        } else {
            self.play(lib, id, None, out, now);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, lib: &mut Library, slots: &mut SlotIds, playhead: Option<f64>, now: f64) -> Vec<Out> {
        let mut out = Vec::new();
        let full = ui.max_rect();
        let p = ui.painter().clone();

        // The audition: forget it once it's over; a looped sound follows the edit
        if let Some(pl) = &self.playing {
            let over = playhead.is_none() && now - pl.since > 0.2 && !(self.looped && pl.whole);
            if over {
                self.playing = None;
            } else if pl.whole && self.looped {
                let id = pl.id;
                if let Some(buf) = lib.sound(id) {
                    if !Arc::ptr_eq(&buf, &pl.buf) {
                        self.play(lib, id, None, &mut out, now);
                    }
                }
            }
        }
        if self.playing.is_some() || self.recorder.is_some() {
            ui.ctx().request_repaint();
        }
        if self.selected.is_some_and(|id| lib.get(id).is_none()) {
            self.selected = None;
        }
        if self.selected.is_none() {
            self.selected = lib.entries.iter().find(|e| self.filter.is_none_or(|v| e.voice == v)).map(|e| e.id);
        }

        // ── Top bar ──
        let margin = 22.0;
        let top = Rect::from_min_size(full.min + vec2(margin, 14.0), vec2(full.width() - margin * 2.0, 52.0));
        let mut ui_top = ui.new_child(
            UiBuilder::new()
                .max_rect(top)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if chip(&mut ui_top, &format!("{}  Back", ph::ARROW_LEFT), false, TEXT).clicked() {
            out.push(Out::Back);
        }
        ui_top.add_space(14.0);
        ui_top.label(egui::RichText::new("SOUNDS").font(display(34.0)).color(TEXT));
        ui_top.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if chip(ui, &format!("{}  Record", ph::MICROPHONE), self.record_panel, RED)
                .on_hover_text("Record from your microphone")
                .clicked()
            {
                self.record_panel = !self.record_panel;
                if self.record_panel {
                    self.inputs = library::inputs();
                }
            }
            if chip(ui, &format!("{}  Import", ph::UPLOAD_SIMPLE), false, ACID)
                .on_hover_text("WAV, AIFF, MP3, FLAC or OGG – or drop files onto the window")
                .clicked()
            {
                if let Some(paths) = rfd::FileDialog::new().add_filter("Audio", &library::EXTENSIONS).pick_files() {
                    self.import(lib, &paths, now);
                }
            }
        });

        let body_r = Rect::from_min_max(pos2(full.left() + margin, top.bottom() + 12.0), full.max - vec2(margin, margin));
        let list_w = (body_r.width() * 0.28).clamp(260.0, 340.0);
        let list_r = Rect::from_min_size(body_r.min, vec2(list_w, body_r.height()));
        let edit_r = Rect::from_min_max(pos2(list_r.right() + 18.0, body_r.top()), body_r.max);

        self.list(ui, list_r, lib, slots, &mut out, now);
        let blocked = self.record_panel || self.replace.is_some() || self.confirm_delete.is_some();
        match self.selected {
            Some(id) => self.editor(ui, edit_r, lib, slots, id, playhead, blocked, &mut out, now),
            None => empty_state(&p, edit_r, lib.entries.is_empty()),
        }

        self.keys(ui, lib, &mut out, now);
        self.record_window(ui, lib, now);
        self.replace_window(ui, lib, slots, &mut out);
        self.delete_window(ui, lib, slots, &mut out);

        // A message for a few seconds at the bottom
        if let Some((text, at)) = &self.message {
            let age = (now - at) as f32;
            if age < 4.0 {
                let a = (4.0 - age).min(1.0);
                let g = p.layout_no_wrap(text.clone(), bold(13.0), alpha(TEXT, a));
                let r = Rect::from_center_size(pos2(full.center().x, full.bottom() - 40.0), g.size() + vec2(32.0, 18.0));
                p.rect_filled(r, 10.0, alpha(Color32::from_rgb(20, 20, 24), a));
                p.rect_stroke(r, 10.0, stroke(1.0, alpha(LINE, a)), egui::StrokeKind::Inside);
                p.galley(r.center() - g.size() / 2.0, g, TEXT);
                ui.ctx().request_repaint();
            } else {
                self.message = None;
            }
        }
        // The index is written once the hands are off
        if self.unsaved && !ui.input(|i| i.pointer.any_down()) {
            lib.save();
            self.unsaved = false;
        }
        out
    }

    // ───────────── The list ─────────────

    fn list(&mut self, ui: &mut Ui, r: Rect, lib: &mut Library, slots: &mut SlotIds, out: &mut Vec<Out>, now: f64) {
        let p = ui.painter().clone();
        p.rect_filled(r, 14.0, mix(BG, PANEL, 0.6));
        let inner = r.shrink(12.0);
        let slots_h = 64.0;
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        // Filter by track: the track colours (only the tracks that have sounds)
        ui.horizontal_wrapped(|ui| {
            if chip(ui, "All", self.filter.is_none(), TEXT).clicked() {
                self.filter = None;
            }
            for v in Voice::ALL {
                let has = lib.entries.iter().any(|e| e.voice == v) || self.filter == Some(v);
                if has && chip(ui, v.name(), self.filter == Some(v), track_color(v)).clicked() {
                    self.filter = if self.filter == Some(v) { None } else { Some(v) };
                }
            }
        });
        if lib.entries.len() > 10 {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(format!("{}  Search", ph::MAGNIFYING_GLASS))
                    .desired_width(f32::INFINITY),
            );
        }
        let q = self.search.to_lowercase();
        let shown: Vec<u64> = lib
            .entries
            .iter()
            .filter(|e| self.filter.is_none_or(|v| e.voice == v) && (q.is_empty() || e.name.to_lowercase().contains(&q)))
            .map(|e| e.id)
            .collect();
        let list_h = (ui.available_height() - slots_h - 8.0).max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_h)
            .min_scrolled_height(list_h)
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                if shown.is_empty() {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new(if lib.entries.is_empty() {
                            "No sounds yet – import or record one"
                        } else {
                            "No sounds on this track yet"
                        })
                        .font(body(13.0))
                        .color(FAINT),
                    );
                }
                for id in &shown {
                    self.row(ui, lib, slots, *id, out, now);
                }
            });

        // The project's eight slots as a row of lights
        let sr = Rect::from_min_max(pos2(inner.left(), inner.bottom() - slots_h), inner.max);
        let used = slots.iter().filter(|&&s| s != 0).count();
        p.text(
            sr.left_top(),
            Align2::LEFT_TOP,
            format!("IN THIS PROJECT  {used} / {SAMPLE_SLOTS}"),
            bold(10.5),
            FAINT,
        );
        let d = ((sr.width() - 7.0 * 6.0) / 8.0).min(34.0);
        for (k, slot) in slots.iter_mut().enumerate() {
            let c = pos2(sr.left() + d / 2.0 + k as f32 * (d + 6.0), sr.top() + 22.0 + d / 2.0);
            let rr = Rect::from_center_size(c, vec2(d, d));
            let resp = ui.interact(rr, ui.id().with(("slot", k)), Sense::click());
            match lib.get(*slot) {
                Some(e) => {
                    let col = track_color(e.voice);
                    let sel = self.selected == Some(e.id);
                    p.circle_filled(c, d / 2.0, if sel { col } else { mix(PANEL, col, 0.55) });
                    p.text(c, Align2::CENTER_CENTER, format!("{}", k + 1), bold(11.0), BG);
                    let id = e.id;
                    let resp = resp
                        .on_hover_text(format!("{} · {} (right-click: remove)", e.name, e.voice.name()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        self.selected = Some(id);
                        self.play(lib, id, None, out, now);
                    }
                    resp.context_menu(|ui| {
                        if ui.button(format!("{}  Remove from the project", ph::X)).clicked() {
                            *slot = 0;
                            ui.close_menu();
                        }
                    });
                }
                None => {
                    p.circle_stroke(c, d / 2.0 - 1.0, stroke(1.0, alpha(LINE, 0.8)));
                }
            }
        }
    }

    fn row(&mut self, ui: &mut Ui, lib: &mut Library, slots: &SlotIds, id: u64, out: &mut Vec<Out>, now: f64) {
        let Some(e) = lib.get(id).cloned() else { return };
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
        let p = ui.painter();
        let col = track_color(e.voice);
        let sel = self.selected == Some(id);
        let hot = resp.hovered();
        p.rect_filled(
            r,
            10.0,
            if sel {
                mix(PANEL, col, 0.16)
            } else if hot {
                PANEL_HI
            } else {
                PANEL
            },
        );
        if sel {
            p.rect_stroke(r, 10.0, stroke(1.0, alpha(col, 0.8)), egui::StrokeKind::Inside);
        }
        // Play button in the track's colour
        let play_r = Rect::from_center_size(pos2(r.left() + 26.0, r.center().y), vec2(30.0, 30.0));
        let playing = self.playing.as_ref().is_some_and(|pl| pl.id == id);
        let play = ui.interact(play_r, ui.id().with(("play", id)), Sense::click());
        p.circle_filled(
            play_r.center(),
            15.0,
            if play.hovered() || playing { col } else { mix(PANEL, col, 0.45) },
        );
        p.text(
            play_r.center(),
            Align2::CENTER_CENTER,
            if playing { ph::STOP } else { ph::PLAY },
            body(14.0),
            if play.hovered() || playing { BG } else { TEXT },
        );
        if play.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.selected = Some(id);
            self.toggle_play(lib, id, out, now);
        }
        // Name and length
        let text_l = r.left() + 50.0;
        let wave_w = 64.0;
        let name = fit_text(p, &e.name, bold(13.0), r.width() - 50.0 - wave_w - 16.0);
        p.text(pos2(text_l, r.center().y - 9.0), Align2::LEFT_CENTER, name, bold(13.0), TEXT);
        let src = lib.source(&e).cloned();
        let len = src.as_ref().map_or(0.0, |s| {
            let (a, b) = kept(s, &e.edit);
            b - a
        });
        p.text(
            pos2(text_l, r.center().y + 10.0),
            Align2::LEFT_CENTER,
            format!("{} · {}", e.voice.name(), fmt_secs(len)),
            body(11.0),
            DIM,
        );
        let wr = Rect::from_min_size(pos2(r.right() - wave_w - 10.0, r.top() + 12.0), vec2(wave_w, r.height() - 24.0));
        if let Some(src) = &src {
            let (a, b) = kept(src, &e.edit);
            draw_wave(p, wr, src, a, b, alpha(col, 0.8));
        }
        if let Some(k) = slots.iter().position(|&s| s == id) {
            let c = pos2(r.right() - 10.0, r.top() + 10.0);
            p.circle_filled(c, 8.0, col);
            p.text(c, Align2::CENTER_CENTER, format!("{}", k + 1), bold(9.5), BG);
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.selected = Some(id);
        }
    }

    // ───────────── The editor ─────────────

    #[allow(clippy::too_many_arguments)]
    fn editor(
        &mut self,
        ui: &mut Ui,
        r: Rect,
        lib: &mut Library,
        slots: &mut SlotIds,
        id: u64,
        playhead: Option<f64>,
        blocked: bool,
        out: &mut Vec<Out>,
        now: f64,
    ) {
        let Some(orig) = lib.get(id).cloned() else { return };
        let Some(src) = lib.source(&orig).cloned() else {
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                "The audio of this sound is missing",
                bold(14.0),
                DIM,
            );
            return;
        };
        let mut e = orig.clone();
        let len = src.seconds().max(1e-3);
        if self.view_for != id {
            self.view_for = id;
            self.view = (0.0, len);
            self.drag = None;
        }
        let col = track_color(e.voice);
        let p = ui.painter().clone();
        p.rect_filled(r, 14.0, mix(BG, PANEL, 0.6));
        let inner = r.shrink(18.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);

        // ── Name and delete ──
        ui.horizontal(|ui| {
            let name = ui.add(
                egui::TextEdit::singleline(&mut e.name)
                    .font(bold(22.0))
                    .desired_width(inner.width() - 60.0)
                    .frame(false),
            );
            if name.gained_focus() {
                self.checkpoint(&orig);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chip(ui, ph::TRASH, false, RED).on_hover_text("Delete").clicked() {
                    self.confirm_delete = Some(id);
                }
            });
        });

        // ── The track it plays on: the eight colours ──
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for v in Voice::ALL {
                let c = track_color(v);
                let on = e.voice == v;
                let g = ui.painter().layout_no_wrap(v.name().to_string(), bold(12.0), TEXT);
                let (rr, resp) = ui.allocate_exact_size(vec2(g.size().x + 26.0, 30.0), Sense::click());
                let hot = resp.hovered();
                ui.painter().rect_filled(
                    rr,
                    15.0,
                    if on {
                        c
                    } else if hot {
                        mix(PANEL, c, 0.35)
                    } else {
                        mix(PANEL, c, 0.14)
                    },
                );
                ui.painter().text(
                    rr.center(),
                    Align2::CENTER_CENTER,
                    v.name(),
                    bold(12.0),
                    if on { BG } else { mix(c, TEXT, 0.3) },
                );
                if resp
                    .on_hover_text("Plays on this track")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                    && !on
                {
                    self.checkpoint(&orig);
                    e.voice = v;
                }
            }
        });

        // ── The waveform ──
        let chopped = !e.slices.is_empty();
        let lane_h = if chopped { 18.0 } else { 0.0 };
        let zoomed = self.view.1 - self.view.0 < len * 0.98;
        let over_h = if zoomed { 22.0 } else { 0.0 };
        let wave_h = (ui.available_height() - 250.0 - if chopped { 50.0 } else { 0.0 }).clamp(120.0, 380.0);
        let (area, _) = ui.allocate_exact_size(
            vec2(inner.width(), lane_h + wave_h + over_h + if zoomed { 6.0 } else { 0.0 }),
            Sense::hover(),
        );
        let lane = Rect::from_min_size(area.min, vec2(area.width(), lane_h));
        let wave = Rect::from_min_size(pos2(area.left(), lane.bottom()), vec2(area.width(), wave_h));
        let over = Rect::from_min_size(pos2(area.left(), wave.bottom() + 6.0), vec2(area.width(), over_h));
        let (v0, v1) = self.view;
        let span = (v1 - v0).max(1e-4);
        let to_x = |t: f32| wave.left() + (t - v0) / span * wave.width();
        let to_t = |x: f32| v0 + (x - wave.left()) / wave.width() * span;
        let (ks, ke) = kept(&src, &e.edit);

        p.rect_filled(wave, 14.0, SPACE);
        let clip = p.with_clip_rect(wave.shrink(1.0));
        draw_wave(&clip, wave, &src, v0, v1, alpha(FAINT, 0.3));
        let kr = Rect::from_min_max(
            pos2(to_x(ks).max(wave.left()), wave.top()),
            pos2(to_x(ke).min(wave.right()), wave.bottom()),
        );
        if kr.width() > 0.0 {
            clip.rect_filled(kr, 0.0, mix(SPACE, col, 0.07));
            draw_wave(&clip.with_clip_rect(kr), wave, &src, v0, v1, col);
        }
        // Fades as lines from the corners, with a dot to grab
        let fi = to_x(ks + e.edit.fade_in);
        let fo = to_x(ke - e.edit.fade_out);
        let fy = wave.top() + 14.0;
        clip.line_segment([pos2(to_x(ks), wave.bottom()), pos2(fi, fy)], stroke(1.2, alpha(TEXT, 0.45)));
        clip.line_segment([pos2(fo, fy), pos2(to_x(ke), wave.bottom())], stroke(1.2, alpha(TEXT, 0.45)));
        for x in [fi, fo] {
            clip.circle_filled(pos2(x, fy), 5.0, TEXT);
        }
        // Slice lines and their flags
        let marks: Vec<f32> = {
            let mut m = e.slices.clone();
            m.sort_by(f32::total_cmp);
            m
        };
        for (i, &m) in marks.iter().enumerate() {
            let x = to_x(m);
            if x < wave.left() || x > wave.right() {
                continue;
            }
            clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.0, alpha(ACID, 0.7)));
            p.add(Shape::convex_polygon(
                vec![
                    pos2(x - 6.0, lane.top() + 2.0),
                    pos2(x + 6.0, lane.top() + 2.0),
                    pos2(x, lane.bottom()),
                ],
                ACID,
                Stroke::NONE,
            ));
            p.text(
                pos2(x + 8.0, lane.center().y),
                Align2::LEFT_CENTER,
                format!("{}", i + 2),
                bold(9.5),
                ACID,
            );
        }
        // Start and end: bright bars with a grip
        for t in [ks, ke] {
            let x = to_x(t);
            if x < wave.left() - 1.0 || x > wave.right() + 1.0 {
                continue;
            }
            clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(2.0, TEXT));
            clip.rect_filled(Rect::from_center_size(pos2(x, wave.center().y), vec2(8.0, 34.0)), 4.0, TEXT);
        }
        // The playhead
        if let (Some(pl), Some(pos)) = (&self.playing, playhead) {
            if pl.id == id {
                let dt = pos as f32 / SAMPLE_SR;
                let t = if pl.reverse { pl.to - dt } else { pl.from + dt };
                let x = to_x(t);
                if (wave.left()..=wave.right()).contains(&x) {
                    clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.5, ACID));
                }
            }
        }
        p.text(
            wave.right_bottom() + vec2(-12.0, -10.0),
            Align2::RIGHT_BOTTOM,
            fmt_secs(ke - ks),
            bold(12.0),
            alpha(TEXT, 0.7),
        );
        // While zoomed in: the whole sound below, with the view in it
        if zoomed {
            p.rect_filled(over, 6.0, SPACE);
            draw_wave(&p.with_clip_rect(over), over, &src, 0.0, len, alpha(col, 0.5));
            let ox = |t: f32| over.left() + t / len * over.width();
            let vr = Rect::from_min_max(pos2(ox(v0), over.top()), pos2(ox(v1).max(ox(v0) + 3.0), over.bottom()));
            p.rect_stroke(vr, 4.0, stroke(1.2, TEXT), egui::StrokeKind::Inside);
        }

        // ── Interaction with the waveform ──
        if !blocked {
            let resp = ui.interact(lane.union(wave), ui.id().with(("wave", id)), Sense::click_and_drag());
            let marks_now = marks.clone();
            let near = |pos: Pos2| -> Option<Handle> {
                if lane.contains(pos) {
                    return marks_now.iter().position(|&m| (to_x(m) - pos.x).abs() < 7.0).map(Handle::Slice);
                }
                if (pos.y - fy).abs() < 9.0 {
                    if (fi - pos.x).abs() < 8.0 {
                        return Some(Handle::FadeIn);
                    }
                    if (fo - pos.x).abs() < 8.0 {
                        return Some(Handle::FadeOut);
                    }
                }
                if (to_x(ks) - pos.x).abs() < 8.0 {
                    return Some(Handle::Start);
                }
                if (to_x(ke) - pos.x).abs() < 8.0 {
                    return Some(Handle::End);
                }
                None
            };
            if let Some(pos) = resp.hover_pos() {
                match near(pos) {
                    Some(_) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
                    None if lane.contains(pos) => ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair),
                    None => ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand),
                }
                // Scroll zooms around the mouse; sideways scroll pans
                let (dx, dy, zoom) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y, i.zoom_delta()));
                let f = ((-dy * 0.004).exp() / zoom).clamp(0.5, 2.0);
                if (f - 1.0).abs() > 1e-4 {
                    let t = to_t(pos.x);
                    let ns = (span * f).clamp(0.004, len);
                    let a = t - (t - v0) * ns / span;
                    self.view = (a, a + ns);
                }
                if dx.abs() > 0.1 {
                    let d = -dx / wave.width() * span;
                    self.view = (self.view.0 + d, self.view.1 + d);
                }
            }
            if resp.drag_started() {
                let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
                self.drag = Some(near(origin).unwrap_or(Handle::Pan));
                if self.drag != Some(Handle::Pan) {
                    self.checkpoint(&orig);
                }
            }
            if resp.dragged() {
                let pos = resp.interact_pointer_pos().unwrap_or_default();
                let t = to_t(pos.x).clamp(0.0, len);
                let min_len = 0.005;
                match self.drag {
                    Some(Handle::Start) => e.edit.start = t.min(ke - min_len).max(0.0),
                    Some(Handle::End) => e.edit.end = if t >= len - 1e-4 { 0.0 } else { t.max(ks + min_len) },
                    Some(Handle::FadeIn) => e.edit.fade_in = (t - ks).clamp(0.0, (ke - ks) * 0.9),
                    Some(Handle::FadeOut) => e.edit.fade_out = (ke - t).clamp(0.0, (ke - ks) * 0.9),
                    Some(Handle::Slice(i)) => {
                        let mut m = marks.clone();
                        if let Some(x) = m.get_mut(i) {
                            *x = t.clamp(ks + 0.002, ke - 0.002);
                        }
                        e.slices = m;
                    }
                    Some(Handle::Pan) => {
                        let d = -resp.drag_delta().x / wave.width() * span;
                        self.view = (self.view.0 + d, self.view.1 + d);
                    }
                    None => {}
                }
            }
            if resp.drag_stopped() {
                self.drag = None;
            }
            if resp.double_clicked() {
                self.view = (0.0, len);
            } else if resp.clicked() {
                let pos = resp.interact_pointer_pos().unwrap_or_default();
                let t = to_t(pos.x);
                if lane.contains(pos) {
                    if t > ks + 0.002 && t < ke - 0.002 && near(pos).is_none() {
                        self.checkpoint(&orig);
                        e.slices.push(t);
                    }
                } else if chopped && t >= ks && t <= ke {
                    // Click a slice to hear it
                    if let Some(w) = edges(&e.edit, ks, ke, &marks).windows(2).find(|w| t >= w[0] && t < w[1]) {
                        self.play(lib, id, Some((w[0], w[1])), out, now);
                    }
                } else {
                    self.play(lib, id, None, out, now);
                }
            }
            if resp.secondary_clicked() {
                if let Some(Handle::Slice(i)) = resp.interact_pointer_pos().and_then(near) {
                    self.checkpoint(&orig);
                    let mut m = marks.clone();
                    m.remove(i);
                    e.slices = m;
                }
            }
            if zoomed {
                let oresp = ui.interact(over, ui.id().with(("over", id)), Sense::click_and_drag());
                if let Some(pos) = oresp.interact_pointer_pos().filter(|_| oresp.dragged() || oresp.clicked()) {
                    let c = (pos.x - over.left()) / over.width() * len;
                    self.view = (c - span / 2.0, c + span / 2.0);
                }
            }
        }
        let s = (self.view.1 - self.view.0).clamp(0.004, len);
        let a = self.view.0.clamp(0.0, (len - s).max(0.0));
        self.view = (a, a + s);

        // ── Play, loop, use it ──
        ui.horizontal(|ui| {
            let playing = self.playing.as_ref().is_some_and(|pl| pl.id == id && pl.whole);
            let (pr, presp) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::click());
            ui.painter().circle_filled(
                pr.center(),
                23.0,
                if presp.hovered() || playing { col } else { mix(PANEL, col, 0.6) },
            );
            ui.painter().text(
                pr.center(),
                Align2::CENTER_CENTER,
                if playing { ph::STOP } else { ph::PLAY },
                body(20.0),
                BG,
            );
            if presp
                .on_hover_text("Play (Space)")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.toggle_play(lib, id, out, now);
            }
            if chip(ui, &format!("{}  Loop", ph::REPEAT), self.looped, col)
                .on_hover_text("Over and over – you hear every change at once")
                .clicked()
            {
                self.looped = !self.looped;
                if playing {
                    self.play(lib, id, None, out, now);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let slot = slots.iter().position(|&s| s == id);
                let label = match slot {
                    Some(_) => format!("{}  Playing on {}", ph::CHECK, e.voice.name()),
                    None => format!("{}  Use on {}", ph::ARROW_RIGHT, e.voice.name()),
                };
                let g = ui.painter().layout_no_wrap(label.clone(), bold(14.0), BG);
                let (br, bresp) = ui.allocate_exact_size(g.size() + vec2(36.0, 22.0), Sense::click());
                let hot = bresp.hovered();
                ui.painter()
                    .rect_filled(br, br.height() / 2.0, if hot { Color32::WHITE } else { ACID });
                ui.painter().galley(br.center() - g.size() / 2.0, g, BG);
                if bresp
                    .on_hover_text(format!(
                        "Puts it on the {} track – a project holds {SAMPLE_SLOTS} of your sounds",
                        e.voice.name()
                    ))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    out.push(Out::Use(id));
                }
            });
        });

        // ── What you can do to it: cards for values, buttons for actions (as in the sound section) ──
        ui.add_space(2.0);
        let gap = 10.0;
        let tw = ((inner.width() - gap * 5.0) / 6.0).max(80.0);
        let th = (ui.available_height() - if chopped { 60.0 } else { 0.0 }).clamp(76.0, 104.0);
        let size = vec2(tw, th);
        let mut hits = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let ed = &mut e.edit;
            let changed = |r: &egui::Response, me: &mut Self| {
                if r.drag_started() || r.double_clicked() || (r.hovered() && ui_scrolled(r)) {
                    me.checkpoint(&orig);
                }
            };
            // PITCH: up higher, down lower
            let mut v = (ed.pitch + 24.0) / 48.0;
            let r = value_card(
                ui,
                size,
                Tile::Pitch,
                "PITCH",
                &format!("{:+.0}", ed.pitch),
                &mut v,
                0.5,
                |v| v * 48.0 - 24.0,
                col,
            )
            .on_hover_text("Drag up: higher · down: lower · double-click: back");
            changed(&r, self);
            if r.changed() {
                ed.pitch = (v * 48.0 - 24.0).round();
            }
            // TONE: up thinner, down darker
            let tone = tone_of(ed);
            let mut v = (tone + 1.0) / 2.0;
            let label = if tone < -0.01 {
                "dark"
            } else if tone > 0.01 {
                "thin"
            } else {
                "open"
            };
            let r = value_card(ui, size, Tile::Tone, "TONE", label, &mut v, 0.5, |v| v * 2.0 - 1.0, col)
                .on_hover_text("Drag up: thinner (less bass) · down: darker · double-click: open");
            changed(&r, self);
            if r.changed() {
                set_tone(ed, v * 2.0 - 1.0);
            }
            // CRUSH: up grittier
            let mut v = ed.crush;
            let r = value_card(
                ui,
                size,
                Tile::Crush,
                "CRUSH",
                &format!("{:.0}", ed.crush * 100.0),
                &mut v,
                0.0,
                |v| v,
                col,
            )
            .on_hover_text("Drag up: grittier, like an old sampler · double-click: clean");
            changed(&r, self);
            if r.changed() {
                ed.crush = v;
            }
            // REVERSE, TRIM, CHOP: buttons like the live effects
            if action_button(
                ui,
                size,
                Tile::Reverse,
                ph::ARROWS_LEFT_RIGHT,
                "REVERSE",
                ed.reverse,
                ed.reverse as u8 as f32,
                col,
            )
            .on_hover_text("Play it backwards")
            .clicked()
            {
                self.checkpoint(&orig);
                ed.reverse = !ed.reverse;
            }
            if action_button(ui, size, Tile::Trim, ph::SCISSORS, "TRIM", false, 0.0, col)
                .on_hover_text("Cut the silence before and after")
                .clicked()
            {
                self.checkpoint(&orig);
                let (a, b) = library::trim_points(&src.data);
                ed.start = a;
                ed.end = b;
            }
            let count = e.slices.len() + 1;
            if action_button(
                ui,
                size,
                Tile::Chop,
                ph::GRID_FOUR,
                "CHOP",
                chopped,
                if chopped { count as f32 } else { 3.0 },
                col,
            )
            .on_hover_text(if chopped {
                "Put it back together"
            } else {
                "Cut it at every hit – play the pieces as pads"
            })
            .clicked()
            {
                self.checkpoint(&orig);
                hits = Some(if chopped { None } else { Some(self.sensitivity) });
            }
        });
        match hits {
            Some(Some(sens)) => {
                e.slices = library::transients(&src.data, ks, ke, sens);
                if e.slices.is_empty() {
                    self.say("No clear hits in this sound", now);
                }
            }
            Some(None) => e.slices.clear(),
            None => {}
        }

        // Slice pads: play them like a sampler, keep them as sounds
        let mut refind = false;
        if !e.slices.is_empty() {
            let edges = edges(&e.edit, ks, ke, &e.slices);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (i, w) in edges.windows(2).enumerate().take(16) {
                    let (pr, resp) = ui.allocate_exact_size(vec2(42.0, 42.0), Sense::click());
                    let hot = resp.hovered();
                    let on = self
                        .playing
                        .as_ref()
                        .is_some_and(|pl| pl.id == id && !pl.whole && (pl.from - w[0]).abs() < 1e-4);
                    ui.painter().rect_filled(
                        pr,
                        10.0,
                        mix(
                            PANEL,
                            col,
                            if on {
                                0.9
                            } else if hot {
                                0.45
                            } else {
                                0.22
                            },
                        ),
                    );
                    ui.painter().text(
                        pr.center(),
                        Align2::CENTER_CENTER,
                        format!("{}", i + 1),
                        bold(13.0),
                        if on { BG } else { TEXT },
                    );
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.play(lib, id, Some((w[0], w[1])), out, now);
                    }
                }
                ui.add_space(8.0);
                if chip(ui, ph::MINUS, false, TEXT).on_hover_text("Fewer pieces").clicked() {
                    self.sensitivity = (self.sensitivity - 0.12).max(0.0);
                    refind = true;
                }
                if chip(ui, ph::PLUS, false, TEXT).on_hover_text("More pieces").clicked() {
                    self.sensitivity = (self.sensitivity + 0.12).min(1.0);
                    refind = true;
                }
                ui.add_space(8.0);
                if chip(
                    ui,
                    &format!("{}  Keep {} pieces as sounds", ph::SCISSORS, edges.len() - 1),
                    false,
                    ACID,
                )
                .on_hover_text("Every piece becomes a sound of its own")
                .clicked()
                {
                    for (i, w) in edges.windows(2).enumerate().rev() {
                        let edit = Edit {
                            start: w[0],
                            end: w[1],
                            fade_in: 0.0,
                            fade_out: 0.005,
                            ..e.edit
                        };
                        lib.add_cut(&e, format!("{} {}", e.name, i + 1), edit);
                    }
                    self.say(format!("{} new sounds below {}", edges.len() - 1, e.name), now);
                }
            });
        }

        if refind {
            self.checkpoint(&orig);
            let found = library::transients(&src.data, ks, ke, self.sensitivity);
            if !found.is_empty() {
                e.slices = found;
            }
        }
        if e != orig {
            if let Some(slot) = lib.get_mut(id) {
                *slot = e;
            }
            self.unsaved = true;
        }
    }

    fn undo_step(&mut self, lib: &mut Library) {
        if let Some(prev) = self.undo.pop() {
            if let Some(cur) = lib.get_mut(prev.id) {
                self.redo.push(std::mem::replace(cur, prev.clone()));
                self.selected = Some(prev.id);
                self.unsaved = true;
            }
        }
    }

    fn redo_step(&mut self, lib: &mut Library) {
        if let Some(next) = self.redo.pop() {
            if let Some(cur) = lib.get_mut(next.id) {
                self.undo.push(std::mem::replace(cur, next.clone()));
                self.selected = Some(next.id);
                self.unsaved = true;
            }
        }
    }

    fn keys(&mut self, ui: &mut Ui, lib: &mut Library, out: &mut Vec<Out>, now: f64) {
        if ui.memory(|m| m.focused().is_some()) {
            if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
                ui.memory_mut(|m| m.request_focus(egui::Id::NULL));
            }
            return;
        }
        let (space, esc, undo, redo, del, up, down) = ui.input_mut(|i| {
            let cmd = i.modifiers.command;
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Space),
                i.key_pressed(egui::Key::Escape),
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && i.modifiers.shift && i.key_pressed(egui::Key::Z),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if esc {
            if self.record_panel || self.replace.is_some() || self.confirm_delete.is_some() {
                self.record_panel = false;
                self.replace = None;
                self.confirm_delete = None;
            } else {
                out.push(Out::Back);
            }
        }
        if undo {
            self.undo_step(lib);
        }
        if redo {
            self.redo_step(lib);
        }
        let Some(id) = self.selected else { return };
        if space {
            self.toggle_play(lib, id, out, now);
        }
        if del {
            self.confirm_delete = Some(id);
        }
        if up || down {
            let q = self.search.to_lowercase();
            let shown: Vec<u64> = lib
                .entries
                .iter()
                .filter(|e| self.filter.is_none_or(|v| e.voice == v) && (q.is_empty() || e.name.to_lowercase().contains(&q)))
                .map(|e| e.id)
                .collect();
            if let Some(i) = shown.iter().position(|&s| s == id) {
                let j = if up { i.saturating_sub(1) } else { (i + 1).min(shown.len() - 1) };
                self.selected = Some(shown[j]);
            }
        }
    }

    // ───────────── Dialogs ─────────────

    fn record_window(&mut self, ui: &mut Ui, lib: &mut Library, now: f64) {
        if !self.record_panel {
            return;
        }
        let ctx = ui.ctx().clone();
        dim_screen(&ctx);
        egui::Window::new("record")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(520.0, 280.0))
            .show(&ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("RECORD").font(display(26.0)).color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(ph::X).clicked() {
                            self.recorder = None;
                            self.record_panel = false;
                        }
                    });
                });
                ui.add_enabled_ui(self.recorder.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Input").font(bold(12.0)).color(DIM));
                        egui::ComboBox::from_id_salt("rec_input")
                            .selected_text(self.input.clone().unwrap_or_else(|| "Default input".into()))
                            .width(300.0)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.input.is_none(), "Default input").clicked() {
                                    self.input = None;
                                }
                                for name in &self.inputs {
                                    if ui.selectable_label(self.input.as_ref() == Some(name), name).clicked() {
                                        self.input = Some(name.clone());
                                    }
                                }
                            });
                    });
                });
                // Level and what came in so far
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 90.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, 8.0, SPACE);
                if let Some(rec) = &self.recorder {
                    let peaks = rec.snapshot(r.width() as usize);
                    let w = r.width() / (MAX_SECONDS * 20.0);
                    let secs = rec.seconds();
                    let n = peaks.len().max(1) as f32;
                    for (i, &v) in peaks.iter().enumerate() {
                        let x = r.left() + (i as f32 / n) * secs * 20.0 * w;
                        let h = v.min(1.0) * r.height() * 0.45;
                        p.line_segment([pos2(x, r.center().y - h), pos2(x, r.center().y + h)], stroke(1.0, RED));
                    }
                    let lvl = rec.level().min(1.0);
                    let meter = Rect::from_min_size(pos2(r.left(), r.bottom() + 4.0), vec2(r.width() * lvl, 4.0));
                    ui.painter().rect_filled(meter, 2.0, if lvl > 0.95 { RED } else { ACID });
                } else {
                    p.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        format!("Up to {MAX_SECONDS:.0} seconds · silence is trimmed afterwards"),
                        body(12.0),
                        FAINT,
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let rec_on = self.recorder.is_some();
                    let label = if rec_on {
                        format!("{}  Stop", ph::STOP)
                    } else {
                        format!("{}  Start recording", ph::RECORD)
                    };
                    let stop_now = self.recorder.as_ref().is_some_and(|r| r.seconds() >= MAX_SECONDS - 0.05);
                    if chip(ui, &label, rec_on, RED).clicked() || stop_now {
                        match self.recorder.take() {
                            Some(rec) => {
                                let data = rec.finish();
                                if data.len() < 400 {
                                    self.say("That was too short", now);
                                } else {
                                    let n = lib.entries.iter().filter(|e| e.name.starts_with("Recording")).count() + 1;
                                    let voice = self.filter.unwrap_or(Voice::Rim);
                                    let id = lib.add(&format!("Recording {n}"), voice, data);
                                    self.selected = Some(id);
                                    self.record_panel = false;
                                    self.say("Recorded – trimmed and levelled. Pick its track at the top.", now);
                                }
                            }
                            None => match Recorder::start(self.input.as_deref()) {
                                Ok(r) => self.recorder = Some(r),
                                Err(e) => self.say(
                                    format!("Can't record: {e}. On a Mac, allow Solarum in Privacy & Security › Microphone."),
                                    now,
                                ),
                            },
                        }
                    }
                    if let Some(rec) = &self.recorder {
                        ui.label(
                            egui::RichText::new(format!("{:.1} s / {MAX_SECONDS:.0} s", rec.seconds()))
                                .font(bold(13.0))
                                .color(TEXT),
                        );
                    }
                });
            });
    }

    fn replace_window(&mut self, ui: &mut Ui, lib: &Library, slots: &mut SlotIds, out: &mut Vec<Out>) {
        let Some(new) = self.replace else { return };
        let ctx = ui.ctx().clone();
        dim_screen(&ctx);
        let name = lib.get(new).map(|e| e.name.clone()).unwrap_or_default();
        egui::Window::new("replace")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(440.0, 0.0))
            .show(&ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("All {SAMPLE_SLOTS} slots are taken"))
                        .font(bold(17.0))
                        .color(TEXT),
                );
                ui.label(
                    egui::RichText::new(format!("Which one should {name} replace? Tracks playing it switch over."))
                        .font(body(12.5))
                        .color(DIM),
                );
                ui.add_space(6.0);
                for (k, slot) in slots.iter_mut().enumerate() {
                    if let Some(e) = lib.get(*slot) {
                        let t = egui::RichText::new(format!("{}   {}  ·  {}", k + 1, e.name, e.voice.name()))
                            .font(bold(13.0))
                            .color(track_color(e.voice));
                        if ui.add_sized(vec2(ui.available_width(), 30.0), egui::Button::new(t)).clicked() {
                            *slot = new;
                            if let Some(v) = lib.get(new).map(|e| e.voice) {
                                out.push(Out::Assign(v, k));
                            }
                            self.replace = None;
                        }
                    }
                }
                ui.add_space(4.0);
                if ui.button("Cancel").clicked() {
                    self.replace = None;
                }
            });
    }

    fn delete_window(&mut self, ui: &mut Ui, lib: &mut Library, slots: &mut SlotIds, out: &mut Vec<Out>) {
        let Some(id) = self.confirm_delete else { return };
        let Some(e) = lib.get(id).cloned() else {
            self.confirm_delete = None;
            return;
        };
        let ctx = ui.ctx().clone();
        dim_screen(&ctx);
        egui::Window::new("delete")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(380.0, 0.0))
            .show(&ctx, |ui| {
                ui.label(egui::RichText::new(format!("Delete {}?", e.name)).font(bold(17.0)).color(TEXT));
                let used = slots.contains(&id);
                ui.label(
                    egui::RichText::new(if used {
                        "It leaves this project too. Saved projects keep their own copy."
                    } else {
                        "Saved projects that use it keep their own copy."
                    })
                    .font(body(12.5))
                    .color(DIM),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(egui::RichText::new(format!("{}  Delete", ph::TRASH)).color(RED))
                        .clicked()
                    {
                        if self.playing.as_ref().is_some_and(|pl| pl.id == id) {
                            self.stop(out);
                        }
                        for s in slots.iter_mut().filter(|s| **s == id) {
                            *s = 0;
                        }
                        lib.remove(id);
                        self.undo.retain(|u| u.id != id);
                        self.redo.retain(|u| u.id != id);
                        self.selected = lib.entries.first().map(|e| e.id);
                        self.confirm_delete = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = None;
                    }
                });
            });
    }
}

/// Behind a dialog: the screen darker, and clicks on it go nowhere
fn dim_screen(ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("lib_dim"))
        .order(egui::Order::Middle)
        .fixed_pos(pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let r = ctx.screen_rect();
            ui.allocate_rect(r, Sense::click());
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(150));
        });
}

fn empty_state(p: &egui::Painter, r: Rect, nothing: bool) {
    p.rect_filled(r, 14.0, mix(BG, PANEL, 0.6));
    let c = r.center();
    let dash = r.shrink(30.0);
    let steps = 60;
    for i in 0..steps {
        if i % 2 == 0 {
            continue;
        }
        let f0 = i as f32 / steps as f32;
        let f1 = (i + 1) as f32 / steps as f32;
        for (a, b) in [
            (dash.left_top(), dash.right_top()),
            (dash.right_top(), dash.right_bottom()),
            (dash.right_bottom(), dash.left_bottom()),
            (dash.left_bottom(), dash.left_top()),
        ] {
            p.line_segment([a + (b - a) * f0, a + (b - a) * f1], stroke(1.0, LINE));
        }
    }
    p.text(c - vec2(0.0, 40.0), Align2::CENTER_CENTER, ph::WAVEFORM, body(54.0), FAINT);
    p.text(
        c + vec2(0.0, 14.0),
        Align2::CENTER_CENTER,
        if nothing {
            "Drop audio files here"
        } else {
            "Pick a sound on the left"
        },
        bold(18.0),
        TEXT,
    );
    p.text(
        c + vec2(0.0, 40.0),
        Align2::CENTER_CENTER,
        if nothing {
            "or Import… / Record at the top – kicks, claps, your voice, a door, anything"
        } else {
            "or drop more audio files here"
        },
        body(12.5),
        DIM,
    );
}
