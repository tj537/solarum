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

mod breed;
mod drawing;
mod editor;
mod flow;
mod paint;

use breed::*;
use drawing::*;
use editor::*;
use paint::*;

/// What the screen asks the app to do
pub enum Out {
    Back,
    /// Play this (looped or once); `None` stops
    Audition(Option<Arc<SampleBuf>>, bool),
    /// Put slot `k` on its track now
    Assign(Voice, usize),
    /// Give this sound a slot and put it on its track
    Use(u64),
    /// Luna earns stardust (a bred sound picked, a sound kept or saved)
    Dust(crate::luna::Source),
}

/// Where you are while making a new sound: it takes the whole screen, one step at a time
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Step {
    /// How to start: a blank board, the mic, a file or a track's template
    Start,
    Draw,
    Record,
    /// Four children of your last pick: click the best one, again and again
    Breed,
    /// The new sound (a draft) in the editor: cut, shape, Save
    Finish(u64),
}

/// Audition ids for the four children (library ids come from the clock, never this high)
const KID_ID: u64 = u64::MAX - 8;
// (DAY_ID + 0 … 2, one for each)

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
    /// Breeding a sound (the Breed step, kept for Back from Finish)
    breed: Option<Breed>,
    /// Luna's three sounds of the day (rendered once a day) and the one on show
    day: Vec<(library::DaySound, Arc<SampleBuf>)>,
    day_at: usize,
    /// Luna's skin (the app keeps it, and the stardust)
    pub luna: crate::luna::Luna,
    /// Making a new sound (`None`: the library), and the step Back goes to from Finish
    flow: Option<Step>,
    back_to: Step,
    /// Space this frame (taken before any button can see it)
    space: bool,
    /// The edited sound as a waveform, for the buffer it was made from
    shown: Option<(Arc<SampleBuf>, Arc<Source>)>,
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
            breed: None,
            day: Vec::new(),
            day_at: 0,
            luna: Default::default(),
            flow: None,
            back_to: Step::Start,
            space: false,
            shown: None,
        }
    }
}

/// The edit's kept part in seconds
fn kept(src: &Source, e: &Edit) -> (f32, f32) {
    let (a, b) = library::span(src.data.len(), e);
    (a as f32 / SAMPLE_SR, b as f32 / SAMPLE_SR)
}

// ───────────── Picture tiles ─────────────

impl LibraryUi {
    /// Open the screen, filtered to a track (from its sound menu) or showing everything
    pub fn show_for(&mut self, voice: Option<Voice>) {
        self.open = true;
        self.filter = voice;
    }

    pub fn select(&mut self, id: u64) {
        self.selected = Some(id);
    }

    /// A new sound is made: on to the last step, where it's cut, shaped and saved
    pub fn finish_with(&mut self, id: u64, back: Step) {
        self.open = true;
        self.flow = Some(Step::Finish(id));
        self.back_to = back;
    }

    /// Hear one of the bred sounds
    fn audition(&mut self, id: u64, buf: Arc<SampleBuf>, out: &mut Vec<Out>, now: f64) {
        out.push(Out::Audition(Some(buf.clone()), false));
        let to = buf.data.len() as f32 / SAMPLE_SR;
        self.playing = Some(Playing {
            id,
            buf,
            from: 0.0,
            to,
            reverse: false,
            whole: false,
            since: now,
        });
    }

    fn save_draft(&mut self, lib: &mut Library, id: u64, now: f64) {
        if let Some(e) = lib.get_mut(id) {
            e.draft = false;
            let name = e.name.clone();
            self.say(format!("Saved – {name} is in your sounds{}", self.dust_note()), now);
        }
        self.unsaved = true;
        self.flow = None;
        self.drawing = None;
        self.filter = None;
        self.selected = Some(id);
    }

    /// All eight slots are playing: open the screen and ask which one this sound replaces
    pub fn ask_replace(&mut self, id: u64) {
        self.open = true;
        self.selected = Some(id);
        self.replace = Some(id);
    }

    /// "  ·  +5 ✦" – the stardust keeping a sound brings Luna today (nothing past the day's limit)
    fn dust_note(&self) -> String {
        match self.luna.would_earn(crate::luna::Source::Keep, crate::luna::today()) {
            0 => String::new(),
            n => format!("  ·  +{n} {}", ph::SPARKLE),
        }
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
                        self.finish_with(id, Step::Start);
                    } else {
                        self.select(id);
                    }
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

        let margin = 22.0;
        let top = Rect::from_min_size(full.min + vec2(margin, 14.0), vec2(full.width() - margin * 2.0, 52.0));
        let body_r = Rect::from_min_max(pos2(full.left() + margin, top.bottom() + 12.0), full.max - vec2(margin, margin));
        let blocked = self.replace.is_some() || self.confirm_delete.is_some();
        if let Some(step) = self.flow {
            // ── Making a new sound: the whole screen, one step at a time ──
            self.flow_bar(ui, top, lib, step, &mut out, now);
            match self.flow {
                Some(Step::Start) => self.start_page(ui, body_r, lib, playhead, blocked, &mut out, now),
                Some(Step::Draw) => self.draw_board(ui, body_r, playhead, blocked, &mut out, now),
                Some(Step::Record) => self.record_page(ui, body_r, lib, now),
                Some(Step::Breed) => self.breed_page(ui, body_r, playhead, blocked, &mut out, now),
                Some(Step::Finish(id)) if lib.get(id).is_some() => {
                    self.editor(ui, body_r, lib, slots, id, playhead, blocked, &mut out, now)
                }
                _ => self.flow = None,
            }
        } else {
            // ── The library: your sounds on the left, the one you picked on the right ──
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
                if pill(ui, &format!("{}  New sound", ph::PLUS), true)
                    .on_hover_text("Draw one, record one, import one or paint over a track's sound (⌘N)")
                    .clicked()
                {
                    self.open_start();
                }
            });
            if lib.entries.iter().any(|e| !e.draft) {
                let list_w = (body_r.width() * 0.28).clamp(260.0, 340.0);
                let list_r = Rect::from_min_size(body_r.min, vec2(list_w, body_r.height()));
                let edit_r = Rect::from_min_max(pos2(list_r.right() + 18.0, body_r.top()), body_r.max);
                self.list(ui, list_r, lib, slots, &mut out, now);
                match self.selected.filter(|&id| lib.get(id).is_some_and(|e| !e.draft)) {
                    Some(id) => self.editor(ui, edit_r, lib, slots, id, playhead, blocked, &mut out, now),
                    None => empty_state(&p, edit_r),
                }
            } else {
                // Nothing yet: nothing to show – the + up there is the way in
                p.text(body_r.center(), Align2::CENTER_CENTER, "No sounds yet", bold(16.0), FAINT);
            }
        }

        self.keys(ui, lib, &mut out, now);
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

    // ───────────── The editor ─────────────

    // ───────────── The start page ─────────────

    // ───────────── Luna's sounds of the day ─────────────

    // ───────────── Breeding ─────────────

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
        if let Some(step) = self.flow {
            // Esc goes one step back (from the start: out); the board and the editor keep their keys
            if esc {
                self.flow_back(lib, out);
                return;
            }
            match step {
                Step::Breed => {
                    let digit = ui.input(|i| {
                        [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4]
                            .iter()
                            .position(|&k| i.key_pressed(k))
                    });
                    if let Some(b) = self.breed.as_mut() {
                        if undo {
                            b.round_back(now);
                        }
                        let hear = match digit {
                            Some(k) => Some((KID_ID + k as u64, b.bufs[k].clone())),
                            None if space && !b.picks.is_empty() => Some((KID_ID + 4, bred_buf(&b.parent()))),
                            None => None,
                        };
                        if let Some((id, buf)) = hear {
                            self.audition(id, buf, out, now);
                        }
                    }
                }
                Step::Draw if undo || redo => {
                    self.draw_undo_step(undo);
                    self.play_drawing(out, now);
                }
                Step::Finish(id) => {
                    if undo {
                        self.undo_step(lib);
                    }
                    if redo {
                        self.redo_step(lib);
                    }
                    if space {
                        self.toggle_play(lib, id, out, now);
                    }
                }
                _ => {}
            }
            return;
        }
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::N)) {
            self.open_start();
            return;
        }
        if esc {
            if self.replace.is_some() || self.confirm_delete.is_some() {
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
