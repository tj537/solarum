//! The SOUNDS screen: your library on the left (filter by track, the project's eight slots), the
//! editor on the right – a waveform you cut by dragging its edges and fades, the track it plays
//! on as a row of colours, and picture tiles for what you can do to it: pitch, reverse, tone,
//! and crush. Plus drawing, importing and recording.

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
    /// Copy the kit's own sound of this track into the library, to edit it
    Kit(Voice),
}

/// What the pen does on the drawing board
#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Pen,
    Erase,
}

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Start,
    End,
    FadeIn,
    FadeOut,
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
    /// The whole sound in view (it follows the pitch) until you zoom or move it
    fit: bool,
    drag: Option<Handle>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    looped: bool,
    playing: Option<Playing>,
    /// A sound waiting for a slot (all eight are taken)
    replace: Option<u64>,
    confirm_delete: Option<u64>,
    record_panel: bool,
    recorder: Option<Recorder>,
    inputs: Vec<String>,
    input: Option<String>,
    message: Option<(String, f64)>,
    unsaved: bool,
    /// The drawing board for a new sound, and the last column the pen was at
    drawing: Option<library::Drawing>,
    draw_name: &'static str,
    /// The track the drawing is for (its colour, and where it goes)
    draw_voice: Voice,
    /// The template it started from (Reset goes back there)
    template: Option<library::Drawing>,
    draw_undo: Vec<library::Drawing>,
    draw_redo: Vec<library::Drawing>,
    tool: Tool,
    /// The last column the pen was at, its value, and the lane (0 = pitch, 1 = loudness)
    pen: Option<(usize, f32, u8)>,
    /// The start page: every way to make a sound; templates are painted or the kit's real ones
    start: bool,
    real: bool,
    /// Little waveforms of the templates, one per track (made once)
    thumbs: Vec<Vec<f32>>,
    /// Space this frame (taken before any button can see it)
    space: bool,
    /// The edited sound as a waveform, for the buffer it was made from
    shown: Option<(Arc<SampleBuf>, Arc<Source>)>,
    rng: crate::rng::Rng,
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
            fit: true,
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            looped: false,
            playing: None,
            replace: None,
            confirm_delete: None,
            record_panel: false,
            recorder: None,
            inputs: Vec::new(),
            input: None,
            message: None,
            unsaved: false,
            drawing: None,
            draw_name: "Drawing",
            draw_voice: Voice::Rim,
            template: None,
            draw_undo: Vec::new(),
            draw_redo: Vec::new(),
            tool: Tool::Pen,
            pen: None,
            start: false,
            real: false,
            thumbs: Vec::new(),
            space: false,
            shown: None,
            rng: crate::rng::Rng::from_clock(),
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

/// How much the edit makes the source louder or softer (normalize or gain)
fn level_of(src: &Source, e: &Edit) -> f32 {
    if !e.normalize {
        return 10f32.powf(e.gain / 20.0);
    }
    let (a, b) = library::span(src.data.len(), e);
    let peak = src.data[a..b].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if peak > 1e-5 {
        0.89 / peak
    } else {
        1.0
    }
}

/// The edit's kept part in seconds
fn kept(src: &Source, e: &Edit) -> (f32, f32) {
    let (a, b) = library::span(src.data.len(), e);
    (a as f32 / SAMPLE_SR, b as f32 / SAMPLE_SR)
}

/// A waveform of `src` from `from` to `to` seconds into `r`
fn draw_wave(p: &egui::Painter, r: Rect, src: &Source, from: f32, to: f32, color: Color32) {
    draw_wave_gain(p, r, src, from, to, color, 1.0);
}

/// The same, louder or softer (as the edit levels the sound)
fn draw_wave_gain(p: &egui::Painter, r: Rect, src: &Source, from: f32, to: f32, color: Color32, gain: f32) {
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

// ───────────── Picture tiles ─────────────

#[derive(Clone, Copy, PartialEq)]
enum Tile {
    Pitch,
    Reverse,
    Tone,
    Crush,
    /// Soft, bright or air – what a drawn line sounds like
    Wave(u8),
    Length,
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
        self.start = false;
    }

    /// The kit's own sound of a track, rendered by the app: a new draft to cut and shape
    pub fn adopt_kit(&mut self, lib: &mut Library, voice: Voice, data: Vec<f32>, now: f64) {
        let n = lib.entries.iter().filter(|e| e.voice == voice).count() + 1;
        let id = lib.add(&format!("{} {n}", voice.name()), voice, data);
        lib.set_draft(id, true);
        self.drawing = None;
        self.select(id);
        self.say("The kit's own sound – cut it, shape it, then Save", now);
    }

    fn open_start(&mut self) {
        self.start = true;
        self.drawing = None;
        self.record_panel = false;
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
                    if paths.len() == 1 {
                        lib.set_draft(id, true);
                    }
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
            (1, None) => self.say("Cut it the way you like, then press Save", now),
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

    /// Silence – always sent, so a loop the screen lost track of stops too
    fn stop(&mut self, out: &mut Vec<Out>) {
        self.playing = None;
        out.push(Out::Audition(None, false));
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
        // Space plays – unless you're typing; a clicked button doesn't keep the keyboard
        let typing = ui.ctx().wants_keyboard_input();
        if !typing {
            if let Some(f) = ui.memory(|m| m.focused()) {
                ui.memory_mut(|m| m.surrender_focus(f));
            }
        }
        self.space = !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));

        // The audition: forget it once it's over; a looped sound follows the edit
        if let Some(pl) = &self.playing {
            let over = playhead.is_none() && now - pl.since > 0.2 && !self.looped;
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
            self.selected = lib
                .entries
                .iter()
                .find(|e| !e.draft && self.filter.is_none_or(|v| e.voice == v))
                .map(|e| e.id);
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
        // + New sound opens the start page (every way to make one, templates first)
        ui_top.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let g = ui.painter().layout_no_wrap(format!("{}  New sound", ph::PLUS), bold(14.0), BG);
            let (br, resp) = ui.allocate_exact_size(g.size() + vec2(34.0, 16.0), Sense::click());
            let on = self.start || resp.hovered();
            ui.painter()
                .rect_filled(br, br.height() / 2.0, if on { Color32::WHITE } else { ACID });
            ui.painter().galley(br.center() - g.size() / 2.0, g, BG);
            if resp
                .on_hover_text("Start from a track's sound, a blank board, your mic or a file (⌘N)")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.open_start();
            }
        });

        let body_r = Rect::from_min_max(pos2(full.left() + margin, top.bottom() + 12.0), full.max - vec2(margin, margin));
        let list_w = (body_r.width() * 0.28).clamp(260.0, 340.0);
        let list_r = Rect::from_min_size(body_r.min, vec2(list_w, body_r.height()));
        let edit_r = Rect::from_min_max(pos2(list_r.right() + 18.0, body_r.top()), body_r.max);

        self.list(ui, list_r, lib, slots, &mut out, now);
        let blocked = self.record_panel || self.replace.is_some() || self.confirm_delete.is_some();
        match self.selected {
            _ if self.drawing.is_some() => self.draw_board(ui, edit_r, lib, playhead, blocked, &mut out, now),
            _ if self.start || lib.entries.is_empty() => self.start_page(ui, edit_r, lib, blocked, &mut out, now),
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
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        // Filter by track: the track colours (only the tracks that have sounds)
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
            if chip(ui, "All", self.filter.is_none(), TEXT).clicked() {
                self.filter = None;
            }
            for v in Voice::ALL {
                let has = lib.entries.iter().any(|e| !e.draft && e.voice == v) || self.filter == Some(v);
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
            .filter(|e| !e.draft && self.filter.is_none_or(|v| e.voice == v) && (q.is_empty() || e.name.to_lowercase().contains(&q)))
            .map(|e| e.id)
            .collect();
        let list_h = ui.available_height().max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_h)
            .min_scrolled_height(list_h)
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                if shown.is_empty() {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new(if lib.entries.is_empty() {
                            "No sounds yet – make one on the right"
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
            self.drawing = None;
            self.selected = Some(id);
            self.start = false;
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
        // In this project: a small light
        if slots.contains(&id) {
            p.circle_filled(pos2(r.right() - 9.0, r.top() + 9.0), 4.0, col);
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.selected = Some(id);
            self.start = false;
            if self.drawing.take().is_some() {
                self.stop(out);
            }
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
        // The waveform shows the sound as it plays: the pitch squeezes (higher) or stretches
        // (lower) the whole picture – cut, bars and the rest – around the start
        let (ks, ke) = kept(&src, &e.edit);
        let ratio = 2f32.powf(-e.edit.pitch / 12.0);
        let disp = move |t: f32| ks + (t - ks) * ratio;
        let undisp = move |d: f32| ks + (d - ks) / ratio;
        // In view at most: the sound as it is and as it plays; fitted: as it is, so a higher pitch
        // leaves room at the end and a lower one runs past it
        let (d_lo, d_hi) = (disp(0.0).min(0.0), disp(len).max(len));
        if self.view_for != id {
            self.view_for = id;
            self.fit = true;
            self.drag = None;
        }
        if self.fit {
            self.view = (0.0, len);
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
                if !e.draft && chip(ui, ph::TRASH, false, RED).on_hover_text("Delete").clicked() {
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
        let zoomed = !self.fit;
        let over_h = if zoomed { 22.0 } else { 0.0 };
        let wave_h = (ui.available_height() - 250.0).clamp(120.0, 380.0);
        let (area, _) = ui.allocate_exact_size(
            vec2(inner.width(), wave_h + over_h + if zoomed { 6.0 } else { 0.0 }),
            Sense::hover(),
        );
        let wave = Rect::from_min_size(area.min, vec2(area.width(), wave_h));
        let over = Rect::from_min_size(pos2(area.left(), wave.bottom() + 6.0), vec2(area.width(), over_h));
        let (v0, v1) = self.view;
        let span = (v1 - v0).max(1e-4);
        let to_x = |t: f32| wave.left() + (disp(t) - v0) / span * wave.width();
        let to_d = |x: f32| v0 + (x - wave.left()) / wave.width() * span;
        let to_t = |x: f32| undisp(to_d(x));

        p.rect_filled(wave, 14.0, SPACE);
        let clip = p.with_clip_rect(wave.shrink(1.0));
        // Outside the cut: the rest of the source, levelled like the cut so both match
        let gain = level_of(&src, &e.edit);
        draw_wave_gain(&clip, wave, &src, undisp(v0), undisp(v1), alpha(FAINT, 0.3), gain);
        let kr = Rect::from_min_max(
            pos2(to_x(ks).max(wave.left()), wave.top()),
            pos2(to_x(ke).min(wave.right()), wave.bottom()),
        );
        // Inside the cut: the sound as it plays – reversed, filtered, crushed, levelled, faded –
        // always exactly between start and end
        let shown_end = ke;
        if kr.width() > 0.0 {
            clip.rect_filled(kr, 0.0, mix(SPACE, col, 0.07));
            match self.rendered_source(lib, id) {
                Some(rs) => {
                    let sr = Rect::from_min_max(pos2(to_x(ks), wave.top()), pos2(to_x(shown_end), wave.bottom()));
                    let vis = sr.intersect(wave);
                    if vis.width() > 0.5 {
                        // Only the columns in view
                        let len_r = rs.seconds();
                        let f0 = (vis.left() - sr.left()) / sr.width().max(1e-3) * len_r;
                        let f1 = (vis.right() - sr.left()) / sr.width().max(1e-3) * len_r;
                        draw_wave(&clip.with_clip_rect(kr.union(vis)), vis, &rs, f0, f1, col);
                    }
                }
                None => draw_wave(&clip.with_clip_rect(kr), wave, &src, undisp(v0), undisp(v1), col),
            }
        }
        // Fades: grabbed along the top edge inside the cut (the wave itself shows them)
        let fi = to_x(ks + e.edit.fade_in);
        let fo = to_x(ke - e.edit.fade_out);
        let fade_band = |pos: Pos2| pos.y < wave.top() + 30.0 && pos.x > to_x(ks) + 8.0 && pos.x < to_x(ke) - 8.0;
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
                // The whole sound is drawn as it plays (reversed too); a slice on the source
                let t = if pl.whole {
                    pl.from + dt
                } else if pl.reverse {
                    pl.to - dt
                } else {
                    pl.from + dt
                };
                let x = to_x(t);
                if (wave.left()..=wave.right()).contains(&x) {
                    clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.5, ACID));
                }
            }
        }
        p.text(
            wave.right_bottom() + vec2(-12.0, -10.0),
            Align2::RIGHT_BOTTOM,
            fmt_secs((ke - ks) * ratio),
            bold(12.0),
            alpha(TEXT, 0.7),
        );
        // While zoomed in: the whole sound below, with the view in it
        if zoomed {
            p.rect_filled(over, 6.0, SPACE);
            draw_wave(&p.with_clip_rect(over), over, &src, 0.0, len, alpha(col, 0.5));
            let ox = |t: f32| over.left() + t / len * over.width();
            let (s0, s1) = (undisp(v0), undisp(v1));
            let vr = Rect::from_min_max(pos2(ox(s0), over.top()), pos2(ox(s1).max(ox(s0) + 3.0), over.bottom()));
            p.rect_stroke(vr, 4.0, stroke(1.2, TEXT), egui::StrokeKind::Inside);
        }

        // ── Interaction with the waveform ──
        if !blocked {
            let resp = ui.interact(wave, ui.id().with(("wave", id)), Sense::click_and_drag());
            let near = |pos: Pos2| -> Option<Handle> {
                if fade_band(pos) {
                    // The nearer of the two fades
                    return Some(if (fi - pos.x).abs() <= (fo - pos.x).abs() {
                        Handle::FadeIn
                    } else {
                        Handle::FadeOut
                    });
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
                // Say what a drag does where the mouse is
                let hint = match near(pos) {
                    Some(Handle::Start) => Some("Start – drag to cut"),
                    Some(Handle::End) => Some("End – drag to cut"),
                    Some(Handle::FadeIn) => Some("Fade in – drag"),
                    Some(Handle::FadeOut) => Some("Fade out – drag"),
                    _ => None,
                };
                if let (Some(h), None) = (hint, self.drag) {
                    let g = p.layout_no_wrap(h.to_string(), bold(11.0), TEXT);
                    let at = pos2(pos.x + 14.0, pos.y + 16.0);
                    let br = Rect::from_min_size(at, g.size() + vec2(14.0, 8.0));
                    p.rect_filled(br, 6.0, Color32::from_rgb(20, 20, 24));
                    p.galley(br.min + vec2(7.0, 4.0), g, TEXT);
                }
                if fade_band(pos) && self.drag.is_none() {
                    // A faint line where the fades end
                    for x in [fi, fo] {
                        clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.0, alpha(TEXT, 0.35)));
                    }
                }
                match near(pos) {
                    Some(_) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
                    None => ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand),
                }
                // Scroll zooms around the mouse; sideways scroll pans
                let (dx, dy, zoom) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y, i.zoom_delta()));
                let f = ((-dy * 0.004).exp() / zoom).clamp(0.5, 2.0);
                if (f - 1.0).abs() > 1e-4 {
                    let t = to_d(pos.x);
                    let ns = (span * f).clamp(0.004, d_hi - d_lo);
                    let a = t - (t - v0) * ns / span;
                    self.view = (a, a + ns);
                    self.fit = false;
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
                    Some(Handle::Pan) => {
                        let d = -resp.drag_delta().x / wave.width() * span;
                        self.view = (self.view.0 + d, self.view.1 + d);
                        self.fit = false;
                    }
                    None => {}
                }
            }
            if resp.drag_stopped() {
                self.drag = None;
            }
            if resp.double_clicked() {
                self.fit = true;
            } else if resp.clicked() {
                self.play(lib, id, None, out, now);
            }
            if zoomed {
                let oresp = ui.interact(over, ui.id().with(("over", id)), Sense::click_and_drag());
                if let Some(pos) = oresp.interact_pointer_pos().filter(|_| oresp.dragged() || oresp.clicked()) {
                    let c = disp((pos.x - over.left()) / over.width() * len);
                    self.view = (c - span / 2.0, c + span / 2.0);
                }
            }
        }
        let s = (self.view.1 - self.view.0).clamp(0.004, d_hi - d_lo);
        let a = self.view.0.clamp(d_lo, (d_hi - s).max(d_lo));
        self.view = (a, a + s);

        // ── Play, loop, save or use it ──
        let mut save_now = false;
        let mut discard = false;
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
                if e.draft {
                    // New: Save puts it in the list on the left, Discard throws it away
                    let g = ui.painter().layout_no_wrap(format!("{}  Save", ph::FLOPPY_DISK), bold(14.0), BG);
                    let (br, bresp) = ui.allocate_exact_size(g.size() + vec2(44.0, 22.0), Sense::click());
                    let hot = bresp.hovered();
                    ui.painter()
                        .rect_filled(br, br.height() / 2.0, if hot { Color32::WHITE } else { ACID });
                    ui.painter().galley(br.center() - g.size() / 2.0, g, BG);
                    if bresp
                        .on_hover_text("Into your sounds on the left")
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        save_now = true;
                    }
                    if chip(ui, &format!("{}  Discard", ph::X), false, RED).clicked() {
                        discard = true;
                    }
                    return;
                }
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
                    .on_hover_text(format!("Puts it on the {} track", e.voice.name()))
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
        let tw = ((inner.width() - gap * 4.0) / 5.0).max(80.0);
        let th = ui.available_height().clamp(76.0, 104.0);
        let size = vec2(tw, th);
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
        });
        if save_now {
            e.draft = false;
            self.say(format!("Saved – {} is in your sounds", e.name), now);
        }
        if discard {
            self.stop(out);
            lib.remove(id);
            self.selected = None;
            return;
        }
        if e != orig {
            if let Some(slot) = lib.get_mut(id) {
                *slot = e;
            }
            self.unsaved = true;
        }
    }

    /// For the screenshots: the board with a surprise on it
    #[cfg(test)]
    pub fn draw_surprise(&mut self, seed: u64) {
        self.open = true;
        let d = library::surprise(&mut crate::rng::Rng::new(seed));
        self.start_drawing(Some(d), Voice::Rim, false);
    }

    /// Open the board with a track's template (for the snapshots)
    #[cfg(test)]
    pub fn draw_template(&mut self, v: Voice) {
        self.open = true;
        self.start_drawing(Some((library::template(v), template_name(v))), v, true);
    }

    /// Open the start page (for the snapshots)
    #[cfg(test)]
    pub fn show_start(&mut self, real: bool) {
        self.open = true;
        self.open_start();
        self.real = real;
    }

    fn start_drawing(&mut self, from: Option<(library::Drawing, &'static str)>, voice: Voice, template: bool) {
        let (d, name) = from.unwrap_or((library::Drawing::default(), "Drawing"));
        self.template = template.then(|| d.clone());
        self.drawing = Some(d);
        self.draw_name = name;
        self.draw_voice = voice;
        self.draw_undo.clear();
        self.draw_redo.clear();
        self.tool = Tool::Pen;
        self.pen = None;
        self.record_panel = false;
        self.start = false;
    }

    // ───────────── The start page ─────────────

    /// Every way to a new sound: a track to start from (its template to paint over, or the
    /// kit's real sound to cut and shape) – or a blank board, a surprise, the mic, a file
    fn start_page(&mut self, ui: &mut Ui, r: Rect, lib: &mut Library, blocked: bool, out: &mut Vec<Out>, now: f64) {
        if self.thumbs.is_empty() {
            self.thumbs = Voice::ALL
                .iter()
                .map(|&v| {
                    let x = library::render_drawing(&library::template(v));
                    let k = x.len().div_ceil(60).max(1);
                    x.chunks(k).map(|c| c.iter().fold(0.0f32, |m, s| m.max(s.abs()))).collect()
                })
                .collect();
        }
        let p = ui.painter().clone();
        p.rect_filled(r, 14.0, mix(BG, PANEL, 0.6));
        let inner = r.shrink(22.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        if blocked {
            ui.disable();
        }
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Make a sound").font(bold(26.0)).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !lib.entries.is_empty() && chip(ui, &format!("{}  Close", ph::X), false, TEXT).clicked() {
                    self.start = false;
                }
            });
        });
        ui.label(
            egui::RichText::new("Pick a track to start from, then make it yours. Or start from nothing.")
                .font(body(13.0))
                .color(DIM),
        );
        ui.add_space(10.0);
        // Paint over the template, or edit the kit's real sound
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if chip(ui, &format!("{}  Paint over it", ph::PENCIL_SIMPLE), !self.real, ACID)
                .on_hover_text("A drawing of the track's sound – paint its pitch and loudness")
                .clicked()
            {
                self.real = false;
            }
            if chip(ui, &format!("{}  Edit the real one", ph::WAVEFORM), self.real, ACID)
                .on_hover_text("The kit's own sound as it plays now – cut it, pitch it, turn it round")
                .clicked()
            {
                self.real = true;
            }
        });
        ui.add_space(10.0);

        // Eight tracks, two rows of four
        let gap = 12.0;
        let low_h = 92.0;
        let grid_h = (ui.available_height() - low_h - 44.0).max(160.0);
        let cw = (inner.width() - gap * 3.0) / 4.0;
        let ch = ((grid_h - gap) / 2.0).min(190.0);
        let t = ui.input(|i| i.time) as f32;
        let mut picked = None;
        for row in Voice::ALL.chunks(4) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &v in row {
                    let (cr, resp) = ui.allocate_exact_size(vec2(cw, ch), Sense::click());
                    let col = track_color(v);
                    let hot = resp.hovered() && !blocked;
                    let p = ui.painter_at(cr);
                    p.rect_filled(cr, 12.0, if hot { mix(PAD, col, 0.10) } else { PAD });
                    p.rect_stroke(
                        cr,
                        12.0,
                        stroke(if hot { 1.5 } else { 1.0 }, if hot { col } else { LINE }),
                        egui::StrokeKind::Inside,
                    );
                    p.circle_filled(pos2(cr.left() + 18.0, cr.top() + 20.0), 5.0, col);
                    p.text(
                        pos2(cr.left() + 30.0, cr.top() + 20.0),
                        Align2::LEFT_CENTER,
                        v.name(),
                        bold(15.0),
                        TEXT,
                    );
                    let pic = Rect::from_min_max(pos2(cr.left() + 14.0, cr.top() + 38.0), pos2(cr.right() - 14.0, cr.bottom() - 30.0));
                    if self.real {
                        thumb_wave(&p, pic, &self.thumbs[v.index()], col);
                    } else {
                        thumb_drawing(&p, pic, &library::template(v), col);
                    }
                    let hint = if self.real { "Edit it" } else { "Paint it" };
                    p.text(
                        pos2(cr.right() - 14.0, cr.bottom() - 15.0),
                        Align2::RIGHT_CENTER,
                        format!("{hint}  {}", ph::ARROW_RIGHT),
                        bold(12.0),
                        if hot { col } else { FAINT },
                    );
                    if hot {
                        // a little sweep over the picture while you look at it
                        let x = pic.left() + (t * 0.6).fract() * pic.width();
                        p.line_segment([pos2(x, pic.top()), pos2(x, pic.bottom())], stroke(1.0, alpha(TEXT, 0.35)));
                        ui.ctx().request_repaint();
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
                        picked = Some(v);
                    }
                }
            });
            ui.add_space(gap - 6.0);
        }
        if let Some(v) = picked {
            if self.real {
                out.push(Out::Kit(v));
            } else {
                self.start_drawing(Some((library::template(v), template_name(v))), v, true);
                self.play_drawing(out, now);
            }
        }

        // Or from nothing
        ui.add_space(6.0);
        ui.label(egui::RichText::new("OR START FROM").font(bold(11.0)).color(FAINT));
        let bw = (inner.width() - gap * 3.0) / 4.0;
        let mut way = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (k, icon, title, sub) in [
                (0, ph::PENCIL_SIMPLE_LINE, "Blank board", "draw any line, hear it"),
                (1, ph::SPARKLE, "Surprise me", "a random space sound"),
                (2, ph::MICROPHONE, "Record", "voice, a door, anything"),
                (3, ph::UPLOAD_SIMPLE, "Import", "WAV, AIFF, MP3, FLAC, OGG"),
            ] {
                let (br, resp) = ui.allocate_exact_size(vec2(bw, low_h - 20.0), Sense::click());
                let hot = resp.hovered() && !blocked;
                let p = ui.painter_at(br);
                p.rect_filled(br, 12.0, if hot { mix(PAD, ACID, 0.08) } else { PAD });
                p.rect_stroke(br, 12.0, stroke(1.0, if hot { ACID } else { LINE }), egui::StrokeKind::Inside);
                let c = pos2(br.left() + 34.0, br.center().y);
                p.circle_filled(c, 20.0, if hot { ACID } else { mix(PAD, ACID, 0.18) });
                p.text(c, Align2::CENTER_CENTER, icon, body(19.0), if hot { BG } else { ACID });
                p.text(pos2(c.x + 32.0, br.center().y - 9.0), Align2::LEFT_CENTER, title, bold(14.0), TEXT);
                let sub = fit_text(&p, sub, body(11.5), br.right() - c.x - 40.0);
                p.text(pos2(c.x + 32.0, br.center().y + 10.0), Align2::LEFT_CENTER, sub, body(11.5), DIM);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !blocked {
                    way = Some(k);
                }
            }
        });
        match way {
            Some(0) => self.start_drawing(None, self.filter.unwrap_or(Voice::Rim), false),
            Some(1) => {
                let d = library::surprise(&mut self.rng);
                self.start_drawing(Some(d), self.filter.unwrap_or(Voice::Rim), false);
                self.play_drawing(out, now);
            }
            Some(2) => {
                self.record_panel = true;
                self.inputs = library::inputs();
            }
            Some(3) => {
                if let Some(paths) = rfd::FileDialog::new().add_filter("Audio", &library::EXTENSIONS).pick_files() {
                    self.start = false;
                    self.import(lib, &paths, now);
                }
            }
            _ => {}
        }
    }

    /// Hear the drawing as it is now
    fn play_drawing(&mut self, out: &mut Vec<Out>, now: f64) {
        let Some(d) = &self.drawing else { return };
        if d.pitch.iter().all(Option::is_none) {
            return;
        }
        let buf = Arc::new(SampleBuf {
            data: library::render_drawing(d),
            pitch: 0.0,
        });
        out.push(Out::Audition(Some(buf.clone()), self.looped));
        self.playing = Some(Playing {
            id: 0,
            buf,
            from: 0.0,
            to: d.length,
            reverse: false,
            whole: false,
            since: now,
        });
    }

    fn draw_undo_step(&mut self, back: bool) {
        let Some(d) = self.drawing.clone() else { return };
        let (from, to) = if back {
            (&mut self.draw_undo, &mut self.draw_redo)
        } else {
            (&mut self.draw_redo, &mut self.draw_undo)
        };
        if let Some(prev) = from.pop() {
            to.push(d);
            self.drawing = Some(prev);
        }
    }

    fn draw_checkpoint(&mut self, d: &library::Drawing) {
        self.draw_undo.push(d.clone());
        if self.draw_undo.len() > 60 {
            self.draw_undo.remove(0);
        }
        self.draw_redo.clear();
    }

    /// The drawing board: paint a line (higher is higher, gaps are silence) and under it how
    /// loud it is; pick the track, how it sounds and how long it is, hear it, keep it
    #[allow(clippy::too_many_arguments)]
    fn draw_board(&mut self, ui: &mut Ui, r: Rect, lib: &mut Library, playhead: Option<f64>, blocked: bool, out: &mut Vec<Out>, now: f64) {
        let Some(mut d) = self.drawing.clone() else { return };
        let before = d.clone();
        let col = track_color(self.draw_voice);
        let p = ui.painter().clone();
        p.rect_filled(r, 14.0, mix(BG, PANEL, 0.6));
        let inner = r.shrink(18.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
        // The name, and the track it's for
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.draw_name).font(bold(22.0)).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for &v in Voice::ALL.iter().rev() {
                    if crate::widgets::chip_sized(ui, v.name(), self.draw_voice == v, track_color(v), None)
                        .on_hover_text(format!("It goes on the {} track", v.name()))
                        .clicked()
                    {
                        self.draw_voice = v;
                    }
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new("FOR").font(bold(11.0)).color(FAINT));
            });
        });

        // The board: the line on top, its loudness below
        let board_h = (ui.available_height() - 180.0).clamp(200.0, 470.0);
        let (board, resp) = ui.allocate_exact_size(vec2(inner.width(), board_h), Sense::click_and_drag());
        let lane_gap = 8.0;
        let loud_h = (board_h * 0.26).clamp(56.0, 120.0);
        let top = Rect::from_min_max(board.min, pos2(board.right(), board.bottom() - loud_h - lane_gap));
        let low = Rect::from_min_max(pos2(board.left(), board.bottom() - loud_h), board.max);
        p.rect_filled(top, 14.0, SPACE);
        p.rect_filled(low, 12.0, SPACE);
        for k in 1..7 {
            let y = top.bottom() - top.height() * k as f32 / 7.0;
            p.line_segment(
                [pos2(top.left() + 8.0, y), pos2(top.right() - 8.0, y)],
                stroke(1.0, alpha(LINE, 0.25)),
            );
        }
        p.text(top.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, "PITCH", bold(10.5), FAINT);
        p.text(low.left_top() + vec2(12.0, 8.0), Align2::LEFT_TOP, "LOUDNESS", bold(10.5), FAINT);
        p.text(
            top.right_top() + vec2(-12.0, 10.0),
            Align2::RIGHT_TOP,
            "higher is higher · gaps are silence · right button erases",
            body(11.0),
            FAINT,
        );
        let n = d.pitch.len();
        let col_x = |c: usize| board.left() + (c as f32 + 0.5) / n as f32 * board.width();
        let val_y = |v: f32| top.bottom() - 10.0 - v * (top.height() - 20.0);
        let lvl_y = |v: f32| low.bottom() - 6.0 - v * (low.height() - 26.0);
        // Loudness: bars where the line sounds, a faint outline where it's silent
        let bar_w = (board.width() / n as f32).max(1.0);
        for c in 0..n {
            let x = col_x(c);
            let y = lvl_y(d.level[c]);
            let on = d.pitch[c].is_some();
            p.line_segment(
                [pos2(x, low.bottom() - 6.0), pos2(x, y)],
                stroke(bar_w, alpha(col, if on { 0.38 } else { 0.08 })),
            );
        }
        let pts: Vec<Pos2> = (0..n).map(|c| pos2(col_x(c), lvl_y(d.level[c]))).collect();
        p.add(Shape::line(pts, stroke(1.5, alpha(col, 0.9))));
        // The line: one stroke per run of drawn columns, with a soft glow under it
        let mut runs: Vec<Vec<Pos2>> = Vec::new();
        let mut run = Vec::new();
        for (c, v) in d.pitch.iter().enumerate() {
            match v {
                Some(v) => run.push(pos2(col_x(c), val_y(*v))),
                None if !run.is_empty() => runs.push(std::mem::take(&mut run)),
                None => {}
            }
        }
        if !run.is_empty() {
            runs.push(run);
        }
        for run in runs {
            if run.len() == 1 {
                p.circle_filled(run[0], 3.0, col);
                continue;
            }
            p.add(Shape::line(run.clone(), stroke(9.0, mix(SPACE, col, 0.18))));
            p.add(Shape::line(run, stroke(3.0, col)));
        }
        if d.pitch.iter().all(Option::is_none) {
            p.text(
                top.center(),
                Align2::CENTER_CENTER,
                format!("{}  draw here", ph::PENCIL_SIMPLE),
                bold(16.0),
                FAINT,
            );
        }
        // The playhead while it plays
        if let (Some(pl), Some(pos)) = (&self.playing, playhead) {
            if pl.id == 0 {
                let x = board.left() + (pos as f32 / SAMPLE_SR / d.length).min(1.0) * board.width();
                p.line_segment([pos2(x, board.top()), pos2(x, board.bottom())], stroke(1.5, TEXT));
            }
        }
        let mut stroke_done = false;
        if !blocked {
            let erase = self.tool == Tool::Erase || ui.input(|i| i.pointer.secondary_down());
            if let Some(pos) = resp
                .interact_pointer_pos()
                .filter(|_| resp.is_pointer_button_down_on() || resp.dragged())
            {
                // the lane is the one the stroke began in
                let lane = self.pen.map_or(u8::from(pos.y > top.bottom() + lane_gap / 2.0), |(_, _, l)| l);
                if self.pen.is_none() {
                    self.draw_checkpoint(&d);
                }
                let c = (((pos.x - board.left()) / board.width() * n as f32) as isize).clamp(0, n as isize - 1) as usize;
                let v = if lane == 0 {
                    (top.bottom() - 10.0 - pos.y) / (top.height() - 20.0)
                } else {
                    (low.bottom() - 6.0 - pos.y) / (low.height() - 26.0)
                }
                .clamp(0.0, 1.0);
                let (c0, v0, _) = self.pen.unwrap_or((c, v, lane));
                let (a, b) = if c0 <= c { (c0, c) } else { (c, c0) };
                for k in a..=b {
                    let f = if b == a { 1.0 } else { (k - a) as f32 / (b - a) as f32 };
                    let f = if c0 <= c { f } else { 1.0 - f };
                    let x = v0 + (v - v0) * f;
                    if lane == 0 {
                        d.pitch[k] = if erase { None } else { Some(x) };
                    } else {
                        d.level[k] = if erase { 0.0 } else { x };
                    }
                }
                self.pen = Some((c, v, lane));
            } else if self.pen.take().is_some() {
                stroke_done = true;
            }
            resp.on_hover_cursor(if self.tool == Tool::Erase {
                egui::CursorIcon::NotAllowed
            } else {
                egui::CursorIcon::Crosshair
            });
        }

        // Play, loop; the pen and the eraser; undo, reset, clear; cancel or keep it
        let mut play = stroke_done;
        let mut replay = false;
        let mut undo = None;
        ui.horizontal(|ui| {
            let playing = self.playing.as_ref().is_some_and(|pl| pl.id == 0);
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
                .on_hover_text("Hear it (Space)")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                || self.space
            {
                if playing {
                    self.stop(out);
                } else {
                    play = true;
                }
            }
            if chip(ui, &format!("{}  Loop", ph::REPEAT), self.looped, col)
                .on_hover_text("Over and over – you hear every stroke at once")
                .clicked()
            {
                self.looped = !self.looped;
                replay = playing;
            }
            ui.add_space(10.0);
            if chip(ui, &format!("{}  Pen", ph::PENCIL_SIMPLE), self.tool == Tool::Pen, col)
                .on_hover_text("Paint the line and its loudness")
                .clicked()
            {
                self.tool = Tool::Pen;
            }
            if chip(ui, &format!("{}  Eraser", ph::ERASER), self.tool == Tool::Erase, col)
                .on_hover_text("Wipe out a part – silence (the right mouse button erases too)")
                .clicked()
            {
                self.tool = Tool::Erase;
            }
            ui.add_space(10.0);
            ui.add_enabled_ui(!self.draw_undo.is_empty(), |ui| {
                if chip(ui, ph::ARROW_COUNTER_CLOCKWISE, false, TEXT)
                    .on_hover_text("Undo (⌘Z)")
                    .clicked()
                {
                    undo = Some(true);
                }
            });
            ui.add_enabled_ui(!self.draw_redo.is_empty(), |ui| {
                if chip(ui, ph::ARROW_CLOCKWISE, false, TEXT).on_hover_text("Redo (⇧⌘Z)").clicked() {
                    undo = Some(false);
                }
            });
            if let Some(t) = self.template.clone() {
                if chip(ui, &format!("{}  Reset", ph::ARROWS_COUNTER_CLOCKWISE), false, TEXT)
                    .on_hover_text("Back to the template")
                    .clicked()
                    && d != t
                {
                    self.draw_checkpoint(&d);
                    d = t;
                    play = true;
                }
            }
            if chip(ui, &format!("{}  Clear", ph::BROOM), false, TEXT)
                .on_hover_text("Start again on an empty board")
                .clicked()
            {
                self.draw_checkpoint(&d);
                d.pitch = vec![None; n];
                d.level = vec![1.0; n];
                self.stop(out);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let drawn = d.pitch.iter().any(Option::is_some);
                let g = ui
                    .painter()
                    .layout_no_wrap(format!("{}  Make it a sound", ph::CHECK), bold(14.0), BG);
                let (br, bresp) = ui.allocate_exact_size(g.size() + vec2(36.0, 22.0), Sense::click());
                let hot = bresp.hovered() && drawn;
                ui.painter().rect_filled(
                    br,
                    br.height() / 2.0,
                    if hot {
                        Color32::WHITE
                    } else if drawn {
                        ACID
                    } else {
                        PANEL_HI
                    },
                );
                ui.painter().galley(br.center() - g.size() / 2.0, g, BG);
                if bresp.clicked() && drawn {
                    let data = library::render_drawing(&d);
                    let n = lib.entries.iter().filter(|e| e.name.starts_with(self.draw_name)).count() + 1;
                    let id = lib.add(&format!("{} {n}", self.draw_name), self.draw_voice, data);
                    lib.set_draft(id, true);
                    self.selected = Some(id);
                    self.drawing = None;
                    self.stop(out);
                    self.say("Now cut it, shape it and press Save", now);
                }
                if chip(ui, &format!("{}  Back", ph::X), false, TEXT)
                    .on_hover_text("Leave the board – back to the start page (Esc)")
                    .clicked()
                {
                    self.drawing = None;
                    self.start = true;
                    self.stop(out);
                }
            });
        });

        // How it sounds and how long: the editor's tile size
        ui.add_space(2.0);
        let gap = 10.0;
        let tw = ((inner.width() - gap * 4.0) / 5.0).max(80.0);
        let th = ui.available_height().clamp(76.0, 104.0);
        let size = vec2(tw, th);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (w, name) in [(0u8, "SOFT"), (1, "BRIGHT"), (2, "AIR")] {
                if action_button(ui, size, Tile::Wave(w), ph::WAVE_SINE, name, d.wave == w, 0.0, col).clicked() {
                    d.wave = w;
                    play = true;
                }
            }
            // LENGTH as a slider: click or drag along it
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let hot = resp.hovered() || resp.dragged();
            let p = ui.painter_at(rect);
            let area = crate::controls::card(&p, rect, "LENGTH", &fmt_secs(d.length), col, hot);
            if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
                let v = ((pos.x - area.left()) / area.width()).clamp(0.0, 1.0);
                d.length = 0.05 + v * 3.95;
            }
            if resp.double_clicked() {
                d.length = 1.0;
            }
            picture(&p, area, Tile::Length, ((d.length - 0.05) / 3.95).clamp(0.0, 1.0), col, 0.0);
            let resp = resp
                .on_hover_text("Click or drag: shorter to the left, longer to the right · double-click: 1 s")
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            if resp.drag_started() {
                self.draw_checkpoint(&before);
            }
            if resp.drag_stopped() || resp.clicked() {
                play = true;
            }
        });
        if replay {
            play = true;
        }
        if self.drawing.is_none() {
            return;
        }
        if d.wave != before.wave {
            // a wave tile is one step back
            self.draw_checkpoint(&before);
        }
        self.drawing = Some(d);
        if let Some(back) = undo {
            self.draw_undo_step(back);
            play = true;
        }
        if play {
            self.play_drawing(out, now);
        }
    }

    /// The sound as it plays, ready to draw (made again only when the edit changed)
    fn rendered_source(&mut self, lib: &mut Library, id: u64) -> Option<Arc<Source>> {
        let buf = lib.sound(id)?;
        if let Some((b, src)) = &self.shown {
            if Arc::ptr_eq(b, &buf) {
                return Some(src.clone());
            }
        }
        let src = Arc::new(Source::new(buf.data.clone()));
        self.shown = Some((buf, src.clone()));
        Some(src)
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
        if ui.ctx().wants_keyboard_input() {
            if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
                ui.memory_mut(|m| m.request_focus(egui::Id::NULL));
            }
            return;
        }
        let (space, esc, undo, redo, del, up, down) = ui.input_mut(|i| {
            let cmd = i.modifiers.command;
            (
                self.space,
                i.key_pressed(egui::Key::Escape),
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && i.modifiers.shift && i.key_pressed(egui::Key::Z),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        let new = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::N));
        if new && !self.record_panel {
            self.open_start();
        }
        if self.drawing.is_some() {
            if esc && !self.record_panel {
                self.drawing = None;
                self.start = true;
                self.stop(out);
            }
            if undo || redo {
                self.draw_undo_step(undo);
                self.play_drawing(out, now);
            }
            return;
        }
        if esc && self.start && !self.record_panel && !lib.entries.is_empty() {
            self.start = false;
            return;
        }
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
                .filter(|e| !e.draft && self.filter.is_none_or(|v| e.voice == v) && (q.is_empty() || e.name.to_lowercase().contains(&q)))
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
                                    let n = lib.entries.iter().filter(|e| e.name.starts_with("Signal")).count() + 1;
                                    let voice = self.filter.unwrap_or(Voice::Rim);
                                    let id = lib.add(&format!("Signal {n}"), voice, data);
                                    lib.set_draft(id, true);
                                    self.selected = Some(id);
                                    self.record_panel = false;
                                    self.say("Recorded – pick its track, cut it, then press Save", now);
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
                        self.selected = lib.entries.iter().find(|e| !e.draft).map(|e| e.id);
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
        if nothing { "Press + New sound" } else { "Pick a sound on the left" },
        bold(18.0),
        TEXT,
    );
    p.text(
        c + vec2(0.0, 40.0),
        Align2::CENTER_CENTER,
        if nothing {
            "draw one, record your voice, a door, anything – or drop audio files here"
        } else {
            "or drop more audio files here"
        },
        body(12.5),
        DIM,
    );
}

/// The name a sound painted from a track's template gets
fn template_name(v: Voice) -> &'static str {
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
fn thumb_drawing(p: &egui::Painter, r: Rect, d: &library::Drawing, col: Color32) {
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

/// A sound in small: its peaks mirrored round the middle
fn thumb_wave(p: &egui::Painter, r: Rect, peaks: &[f32], col: Color32) {
    let top = peaks.iter().fold(1e-4f32, |m, x| m.max(*x));
    let w = r.width() / peaks.len().max(1) as f32;
    for (i, pk) in peaks.iter().enumerate() {
        let x = r.left() + (i as f32 + 0.5) * w;
        let h = (pk / top).max(0.02) * r.height() * 0.48;
        p.line_segment(
            [pos2(x, r.center().y - h), pos2(x, r.center().y + h)],
            stroke((w * 0.6).max(1.0), col),
        );
    }
}
