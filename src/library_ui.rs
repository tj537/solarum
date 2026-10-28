//! The SOUNDS screen: your library on the left (filter by track, search, the project's eight
//! slots), the editor on the right – a zoomable waveform with start/end and fade handles, slice
//! markers and pads, gain, pitch, filters, crush, reverse – plus importing and recording.

use crate::library::{self, Edit, Entry, Library, Recorder, SlotIds, Source, BLOCK, MAX_SECONDS};
use crate::sampler::{SampleBuf, SAMPLE_SLOTS, SAMPLE_SR};
use crate::song::Voice;
use crate::theme::*;
use crate::widgets::{bar_slider, chip, fit_text};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Pos2, Rect, Sense, Ui, UiBuilder};
use egui_phosphor::regular as ph;
use std::path::PathBuf;
use std::sync::Arc;

/// What the screen asks the app to do
pub enum Out {
    Back,
    /// Play this (looped or once); `None` stops
    Audition(Option<Arc<SampleBuf>>, bool),
    /// Put slot `k` on its track now
    Assign(Voice, usize),
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
    /// Where in the source the buffer starts (seconds) and whether it runs backwards
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
        segs.push(egui::Shape::line_segment(
            [pos2(px, mid - hi * h - 0.5), pos2(px, mid - lo * h + 0.5)],
            stroke(1.0, color),
        ));
    }
    p.extend(segs);
}

/// A small labelled slider: the label, its value, and the bar (`v` 0…1)
fn knob(ui: &mut Ui, label: &str, value: String, v: &mut f32, width: f32, color: Color32, enabled: bool) -> egui::Response {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).font(bold(10.5)).color(FAINT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(value).font(body(11.0)).color(if enabled { DIM } else { FAINT }));
            });
        });
        ui.add_enabled_ui(enabled, |ui| bar_slider(ui, v, width, color)).inner
    })
    .inner
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

    fn say(&mut self, text: impl Into<String>, now: f64) {
        self.message = Some((text.into(), now));
    }

    fn checkpoint(&mut self, e: &Entry) {
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
            (1, None) => self.say("Imported – trimmed and levelled. Cut it on the right.", now),
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

        // ── Top bar ──
        let margin = 22.0;
        let top = Rect::from_min_size(full.min + vec2(margin, 14.0), vec2(full.width() - margin * 2.0, 52.0));
        let mut ui_top = ui.new_child(
            UiBuilder::new()
                .max_rect(top)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if chip(&mut ui_top, &format!("{}  Back", ph::ARROW_LEFT), false, TEXT)
            .on_hover_text("Esc")
            .clicked()
        {
            out.push(Out::Back);
        }
        ui_top.add_space(14.0);
        ui_top.label(egui::RichText::new("SOUNDS").font(display(34.0)).color(TEXT));
        ui_top.add_space(10.0);
        ui_top.label(
            egui::RichText::new(format!(
                "{} in your library · record, import, cut – up to {SAMPLE_SLOTS} in a project",
                lib.entries.len()
            ))
            .font(body(12.5))
            .color(DIM),
        );
        ui_top.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if chip(ui, &format!("{}  Record", ph::MICROPHONE), self.record_panel, RED)
                .on_hover_text("Record from your microphone or audio interface")
                .clicked()
            {
                self.record_panel = !self.record_panel;
                if self.record_panel {
                    self.inputs = library::inputs();
                }
            }
            if chip(ui, &format!("{}  Import…", ph::UPLOAD_SIMPLE), false, ACID)
                .on_hover_text("WAV, AIFF, MP3, FLAC or OGG – or drop files onto the window")
                .clicked()
            {
                if let Some(paths) = rfd::FileDialog::new().add_filter("Audio", &library::EXTENSIONS).pick_files() {
                    self.import(lib, &paths, now);
                }
            }
        });

        let body_r = Rect::from_min_max(pos2(full.left() + margin, top.bottom() + 12.0), full.max - vec2(margin, margin));
        let list_w = (body_r.width() * 0.3).clamp(280.0, 380.0);
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
        self.replace_window(ui, lib, slots);
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
        let slots_h = 150.0;
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        // Filter by track
        ui.horizontal_wrapped(|ui| {
            if chip(ui, "All", self.filter.is_none(), TEXT).clicked() {
                self.filter = None;
            }
            for v in Voice::ALL {
                let n = lib.entries.iter().filter(|e| e.voice == v).count();
                let label = if n > 0 { format!("{} {n}", v.name()) } else { v.name().to_string() };
                if chip(ui, &label, self.filter == Some(v), track_color(v)).clicked() {
                    self.filter = if self.filter == Some(v) { None } else { Some(v) };
                }
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search", ph::MAGNIFYING_GLASS))
                .desired_width(f32::INFINITY),
        );
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
                            "No sounds yet.\nImport a file or record one."
                        } else {
                            "Nothing here – try another track or search."
                        })
                        .font(body(13.0))
                        .color(FAINT),
                    );
                }
                for id in &shown {
                    self.row(ui, lib, slots, *id, out, now);
                }
            });

        // The project's eight slots
        let sr = Rect::from_min_max(pos2(inner.left(), inner.bottom() - slots_h), inner.max);
        let used = slots.iter().filter(|&&s| s != 0).count();
        p.text(
            sr.left_top(),
            Align2::LEFT_TOP,
            format!("IN THIS PROJECT  {used}/{SAMPLE_SLOTS}"),
            bold(10.5),
            FAINT,
        );
        let cols = 4;
        let gap = 6.0;
        let cw = (sr.width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let ch = (sr.height() - 22.0 - gap) / 2.0;
        for (k, slot) in slots.iter_mut().enumerate() {
            let c = Rect::from_min_size(
                pos2(
                    sr.left() + (k % cols) as f32 * (cw + gap),
                    sr.top() + 20.0 + (k / cols) as f32 * (ch + gap),
                ),
                vec2(cw, ch),
            );
            let resp = ui.interact(c, ui.id().with(("slot", k)), Sense::click());
            let e = lib.get(*slot);
            let hot = resp.hovered();
            match e {
                Some(e) => {
                    let col = track_color(e.voice);
                    let sel = self.selected == Some(e.id);
                    p.rect_filled(c, 8.0, mix(PANEL, col, if sel { 0.3 } else { 0.14 }));
                    p.rect_stroke(
                        c,
                        8.0,
                        stroke(1.0, if sel || hot { col } else { alpha(col, 0.5) }),
                        egui::StrokeKind::Inside,
                    );
                    p.text(
                        c.left_top() + vec2(7.0, 5.0),
                        Align2::LEFT_TOP,
                        format!("{}", k + 1),
                        bold(10.0),
                        col,
                    );
                    let name = fit_text(&p, &e.name, bold(11.0), cw - 12.0);
                    p.text(c.left_bottom() + vec2(7.0, -6.0), Align2::LEFT_BOTTOM, name, bold(11.0), TEXT);
                    let id = e.id;
                    let resp = resp
                        .on_hover_text(format!("{} on the {} track – click to edit", e.name, e.voice.name()))
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
                    p.rect_stroke(c, 8.0, stroke(1.0, alpha(LINE, 0.6)), egui::StrokeKind::Inside);
                    p.text(
                        c.center(),
                        Align2::CENTER_CENTER,
                        format!("{}", k + 1),
                        bold(11.0),
                        alpha(FAINT, 0.6),
                    );
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
        p.rect_filled(Rect::from_min_size(r.min + vec2(0.0, 10.0), vec2(3.0, r.height() - 20.0)), 1.5, col);
        // Play button
        let play_r = Rect::from_center_size(pos2(r.left() + 26.0, r.center().y), vec2(30.0, 30.0));
        let playing = self.playing.as_ref().is_some_and(|pl| pl.id == id);
        let play = ui.interact(play_r, ui.id().with(("play", id)), Sense::click());
        p.circle_filled(
            play_r.center(),
            14.0,
            if play.hovered() {
                mix(PANEL, col, 0.5)
            } else {
                mix(PANEL, col, 0.25)
            },
        );
        p.text(
            play_r.center(),
            Align2::CENTER_CENTER,
            if playing { ph::STOP } else { ph::PLAY },
            body(14.0),
            TEXT,
        );
        if play.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            if playing {
                self.stop(out);
            } else {
                self.play(lib, id, None, out, now);
            }
        }
        // Name, track and length
        let text_l = r.left() + 48.0;
        let wave_w = 70.0;
        let name = fit_text(p, &e.name, bold(13.0), r.width() - 48.0 - wave_w - 16.0);
        p.text(pos2(text_l, r.center().y - 9.0), Align2::LEFT_CENTER, name, bold(13.0), TEXT);
        let src = lib.source(&e).cloned();
        let len = src.as_ref().map_or(0.0, |s| {
            let (a, b) = kept(s, &e.edit);
            b - a
        });
        let slot = slots.iter().position(|&s| s == id);
        let mut info = format!("{} · {}", e.voice.name(), fmt_secs(len));
        if !e.slices.is_empty() {
            info += &format!(" · {} slices", e.slices.len() + 1);
        }
        p.text(pos2(text_l, r.center().y + 10.0), Align2::LEFT_CENTER, info, body(11.0), DIM);
        // A little picture of it
        let wr = Rect::from_min_size(pos2(r.right() - wave_w - 10.0, r.top() + 12.0), vec2(wave_w, r.height() - 24.0));
        if let Some(src) = &src {
            let (a, b) = kept(src, &e.edit);
            draw_wave(p, wr, src, a, b, alpha(col, 0.8));
        }
        if let Some(k) = slot {
            let b = Rect::from_center_size(pos2(r.right() - 10.0, r.top() + 10.0), vec2(16.0, 16.0));
            p.circle_filled(b.center(), 8.0, col);
            p.text(b.center(), Align2::CENTER_CENTER, format!("{}", k + 1), bold(9.5), BG);
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.selected = Some(id);
        }
        if resp.double_clicked() {
            self.play(lib, id, None, out, now);
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
        let inner = r.shrink(16.0);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

        // ── Name, track, project ──
        ui.horizontal(|ui| {
            let name = ui.add(
                egui::TextEdit::singleline(&mut e.name)
                    .font(bold(20.0))
                    .desired_width((inner.width() * 0.32).max(160.0))
                    .frame(false),
            );
            if name.gained_focus() {
                self.checkpoint(&orig);
            }
            ui.add_space(6.0);
            egui::ComboBox::from_id_salt(("voice", id))
                .selected_text(egui::RichText::new(e.voice.name()).color(col).font(bold(13.0)))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for v in Voice::ALL {
                        let t = egui::RichText::new(v.name()).color(track_color(v)).font(bold(13.0));
                        if ui.selectable_label(e.voice == v, t).clicked() && e.voice != v {
                            self.checkpoint(&orig);
                            e.voice = v;
                        }
                    }
                })
                .response
                .on_hover_text("The track this sound plays on – it only shows up there");
            let slot = slots.iter().position(|&s| s == id);
            let label = match slot {
                Some(k) => format!("{}  In the project · slot {}", ph::CHECK, k + 1),
                None => format!("{}  Add to the project", ph::PLUS),
            };
            if chip(ui, &label, slot.is_some(), ACID)
                .on_hover_text(format!("A project uses up to {SAMPLE_SLOTS} of your sounds"))
                .clicked()
            {
                match slot {
                    Some(k) => slots[k] = 0,
                    None => match slots.iter().position(|&s| s == 0) {
                        Some(k) => {
                            slots[k] = id;
                            self.say(
                                format!("In slot {} – pick it in the {} track's sound menu", k + 1, e.voice.name()),
                                now,
                            );
                        }
                        None => self.replace = Some(id),
                    },
                }
            }
            if let Some(k) = slot {
                if chip(ui, &format!("{}  Play on {}", ph::ARROW_RIGHT, e.voice.name()), false, col)
                    .on_hover_text("Make it the track's sound now")
                    .clicked()
                {
                    out.push(Out::Assign(e.voice, k));
                    self.say(format!("The {} track plays {} now", e.voice.name(), e.name), now);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chip(ui, ph::TRASH, false, RED).on_hover_text("Delete this sound").clicked() {
                    self.confirm_delete = Some(id);
                }
                if chip(ui, ph::COPY, false, TEXT)
                    .on_hover_text("Duplicate: a copy with the same cut")
                    .clicked()
                {
                    let new = lib.add_cut(&e, format!("{} copy", e.name), e.edit);
                    self.selected = Some(new);
                }
            });
        });

        // ── The waveform ──
        let lane_h = 18.0;
        let over_h = 30.0;
        let wave_h = (ui.available_height() - 250.0).clamp(140.0, 420.0);
        let (area, _) = ui.allocate_exact_size(vec2(inner.width(), lane_h + wave_h + over_h + 26.0), Sense::hover());
        let lane = Rect::from_min_size(area.min, vec2(area.width(), lane_h));
        let wave = Rect::from_min_size(pos2(area.left(), lane.bottom()), vec2(area.width(), wave_h));
        let ruler = Rect::from_min_size(pos2(area.left(), wave.bottom()), vec2(area.width(), 18.0));
        let over = Rect::from_min_size(pos2(area.left(), ruler.bottom() + 8.0), vec2(area.width(), over_h));
        let (v0, v1) = self.view;
        let span = (v1 - v0).max(1e-4);
        let to_x = |t: f32| wave.left() + (t - v0) / span * wave.width();
        let to_t = |x: f32| v0 + (x - wave.left()) / wave.width() * span;
        let (ks, ke) = kept(&src, &e.edit);

        p.rect_filled(wave, 10.0, SPACE);
        let clip = p.with_clip_rect(wave);
        // Outside the cut: faint; inside: the track's colour
        draw_wave(&clip, wave, &src, v0, v1, alpha(FAINT, 0.35));
        let kr = Rect::from_min_max(
            pos2(to_x(ks).max(wave.left()), wave.top()),
            pos2(to_x(ke).min(wave.right()), wave.bottom()),
        );
        if kr.width() > 0.0 {
            clip.rect_filled(kr, 0.0, mix(SPACE, col, 0.07));
            draw_wave(&clip.with_clip_rect(kr), wave, &src, v0, v1, col);
        }
        clip.line_segment(
            [pos2(wave.left(), wave.center().y), pos2(wave.right(), wave.center().y)],
            stroke(1.0, alpha(LINE, 0.4)),
        );
        // Fades as lines from the corners
        let fi = to_x(ks + e.edit.fade_in);
        let fo = to_x(ke - e.edit.fade_out);
        clip.line_segment([pos2(to_x(ks), wave.bottom()), pos2(fi, wave.top())], stroke(1.2, alpha(TEXT, 0.5)));
        clip.line_segment([pos2(fo, wave.top()), pos2(to_x(ke), wave.bottom())], stroke(1.2, alpha(TEXT, 0.5)));
        // Slices
        let mut marks: Vec<f32> = e.slices.clone();
        marks.sort_by(f32::total_cmp);
        p.rect_filled(lane, 4.0, alpha(PANEL_HI, 0.5));
        for (i, &m) in marks.iter().enumerate() {
            let x = to_x(m);
            if x < wave.left() || x > wave.right() {
                continue;
            }
            clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.0, alpha(ACID, 0.7)));
            p.add(egui::Shape::convex_polygon(
                vec![
                    pos2(x - 6.0, lane.top() + 2.0),
                    pos2(x + 6.0, lane.top() + 2.0),
                    pos2(x, lane.bottom()),
                ],
                ACID,
                egui::Stroke::NONE,
            ));
            p.text(
                pos2(x + 8.0, lane.center().y),
                Align2::LEFT_CENTER,
                format!("{}", i + 2),
                bold(9.5),
                ACID,
            );
        }
        // Start and end handles
        for (t, left) in [(ks, true), (ke, false)] {
            let x = to_x(t);
            if x < wave.left() - 1.0 || x > wave.right() + 1.0 {
                continue;
            }
            clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(2.0, TEXT));
            let tab = if left {
                Rect::from_min_size(pos2(x, wave.top()), vec2(10.0, 22.0))
            } else {
                Rect::from_min_size(pos2(x - 10.0, wave.top()), vec2(10.0, 22.0))
            };
            clip.rect_filled(tab, 2.0, TEXT);
        }
        // Fade handles: small squares on the top edge
        for x in [fi, fo] {
            clip.rect_filled(
                Rect::from_center_size(pos2(x, wave.top() + 30.0), vec2(9.0, 9.0)),
                2.0,
                alpha(TEXT, 0.8),
            );
        }
        // The playhead
        if let (Some(pl), Some(pos)) = (&self.playing, playhead) {
            if pl.id == id {
                // (the playhead runs in the sound's own samples; pitch only changes how fast)
                let dt = pos as f32 / SAMPLE_SR;
                let t = if pl.reverse { pl.to - dt } else { pl.from + dt };
                let x = to_x(t);
                if (wave.left()..=wave.right()).contains(&x) {
                    clip.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], stroke(1.5, ACID));
                }
            }
        }
        // Time ruler
        let steps = [0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0];
        let step = steps.into_iter().find(|s| span / s <= 8.0).unwrap_or(10.0);
        let mut t = (v0 / step).ceil() * step;
        while t <= v1 {
            let x = to_x(t);
            p.line_segment([pos2(x, ruler.top()), pos2(x, ruler.top() + 4.0)], stroke(1.0, FAINT));
            let label = if step < 0.1 {
                format!("{:.0} ms", t * 1000.0)
            } else {
                format!("{t:.1} s")
            };
            if x < ruler.right() - 40.0 {
                p.text(pos2(x + 3.0, ruler.top() + 2.0), Align2::LEFT_TOP, label, body(10.0), FAINT);
            }
            t += step;
        }
        // Overview of the whole source, with the view in it
        p.rect_filled(over, 6.0, SPACE);
        draw_wave(&p.with_clip_rect(over), over, &src, 0.0, len, alpha(col, 0.55));
        let ox = |t: f32| over.left() + t / len * over.width();
        let vr = Rect::from_min_max(pos2(ox(v0), over.top()), pos2(ox(v1).max(ox(v0) + 3.0), over.bottom()));
        p.rect_stroke(vr, 4.0, stroke(1.2, TEXT), egui::StrokeKind::Inside);
        p.rect_filled(vr, 4.0, alpha(TEXT, 0.06));

        // ── Interaction ──
        if !blocked {
            let resp = ui.interact(lane.union(wave), ui.id().with(("wave", id)), Sense::click_and_drag());
            let hover = resp.hover_pos();
            let marks_now = marks.clone();
            let near = |pos: Pos2| -> Option<Handle> {
                if lane.contains(pos) {
                    return marks_now.iter().position(|&m| (to_x(m) - pos.x).abs() < 7.0).map(Handle::Slice);
                }
                if (pos.y - (wave.top() + 30.0)).abs() < 8.0 {
                    if (fi - pos.x).abs() < 7.0 {
                        return Some(Handle::FadeIn);
                    }
                    if (fo - pos.x).abs() < 7.0 {
                        return Some(Handle::FadeOut);
                    }
                }
                if (to_x(ks) - pos.x).abs() < 7.0 {
                    return Some(Handle::Start);
                }
                if (to_x(ke) - pos.x).abs() < 7.0 {
                    return Some(Handle::End);
                }
                None
            };
            if let Some(pos) = hover {
                match near(pos) {
                    Some(_) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
                    None if lane.contains(pos) => ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair),
                    None => ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand),
                }
                // Scroll: zoom around the mouse; sideways scroll pans
                let (dx, dy, zoom) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y, i.zoom_delta()));
                let mut f = (-dy * 0.004).exp() / zoom;
                if (f - 1.0).abs() > 1e-4 {
                    let t = to_t(pos.x);
                    f = f.clamp(0.5, 2.0);
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
                        if let Some(m) = marks.get_mut(i) {
                            *m = t.clamp(ks + 0.002, ke - 0.002);
                        }
                        e.slices = marks.clone();
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
                    // A click in the lane adds a slice
                    if t > ks + 0.002 && t < ke - 0.002 && near(pos).is_none() {
                        self.checkpoint(&orig);
                        marks.push(t);
                        e.slices = marks.clone();
                    }
                } else if !marks.is_empty() && t >= ks && t <= ke {
                    // Click a slice to hear it
                    let mut edges = vec![ks];
                    edges.extend(marks.iter().copied());
                    edges.push(ke);
                    if let Some(w) = edges.windows(2).find(|w| t >= w[0] && t < w[1]) {
                        self.play(lib, id, Some((w[0], w[1])), out, now);
                    }
                } else {
                    self.play(lib, id, None, out, now);
                }
            }
            if resp.secondary_clicked() {
                if let Some(Handle::Slice(i)) = resp.interact_pointer_pos().and_then(near) {
                    self.checkpoint(&orig);
                    marks.remove(i);
                    e.slices = marks.clone();
                }
            }
            // The overview: click or drag to move the view there
            let oresp = ui.interact(over, ui.id().with(("over", id)), Sense::click_and_drag());
            if let Some(pos) = oresp.interact_pointer_pos().filter(|_| oresp.dragged() || oresp.clicked()) {
                let c = (pos.x - over.left()) / over.width() * len;
                self.view = (c - span / 2.0, c + span / 2.0);
            }
            oresp.on_hover_cursor(egui::CursorIcon::Grab);
        }
        // Keep the view inside the sound
        let s = (self.view.1 - self.view.0).clamp(0.004, len);
        let a = self.view.0.clamp(0.0, (len - s).max(0.0));
        self.view = (a, a + s);

        // ── Transport and quick tools ──
        ui.horizontal(|ui| {
            let playing = self.playing.as_ref().is_some_and(|pl| pl.id == id);
            if chip(
                ui,
                &format!(
                    "{}  {}",
                    if playing { ph::STOP } else { ph::PLAY },
                    if playing { "Stop" } else { "Play" }
                ),
                playing,
                ACID,
            )
            .on_hover_text("Space")
            .clicked()
            {
                if playing {
                    self.stop(out);
                } else {
                    self.play(lib, id, None, out, now);
                }
            }
            if chip(ui, &format!("{}  Loop", ph::REPEAT), self.looped, ACID)
                .on_hover_text("Play it over and over – you hear every change at once")
                .clicked()
            {
                self.looped = !self.looped;
                if playing {
                    self.play(lib, id, None, out, now);
                }
            }
            ui.add_space(8.0);
            if chip(ui, &format!("{}  Fit", ph::CORNERS_OUT), false, TEXT)
                .on_hover_text("Show the whole sound (double-click the waveform)")
                .clicked()
            {
                self.view = (0.0, len);
            }
            if chip(ui, &format!("{}  Zoom to cut", ph::MAGNIFYING_GLASS_PLUS), false, TEXT).clicked() {
                let pad = (ke - ks) * 0.05;
                self.view = (ks - pad, ke + pad);
            }
            if chip(ui, &format!("{}  Auto-trim", ph::SCISSORS), false, TEXT)
                .on_hover_text("Cut the silence before and after")
                .clicked()
            {
                self.checkpoint(&orig);
                let (a, b) = library::trim_points(&src.data);
                e.edit.start = a;
                e.edit.end = b;
            }
            if chip(ui, &format!("{}  Reverse", ph::ARROWS_LEFT_RIGHT), e.edit.reverse, col).clicked() {
                self.checkpoint(&orig);
                e.edit.reverse = !e.edit.reverse;
            }
            if chip(ui, &format!("{}  Normalize", ph::ARROWS_OUT_LINE_VERTICAL), e.edit.normalize, col)
                .on_hover_text("As loud as it goes without clipping")
                .clicked()
            {
                self.checkpoint(&orig);
                e.edit.normalize = !e.edit.normalize;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.redo.is_empty(), egui::Button::new(ph::ARROW_CLOCKWISE))
                    .on_hover_text("Redo (Shift+Cmd+Z)")
                    .clicked()
                {
                    self.redo_step(lib);
                }
                if ui
                    .add_enabled(!self.undo.is_empty(), egui::Button::new(ph::ARROW_COUNTER_CLOCKWISE))
                    .on_hover_text("Undo (Cmd+Z)")
                    .clicked()
                {
                    self.undo_step(lib);
                }
                ui.label(
                    egui::RichText::new(format!("{} of {}", fmt_secs(ke - ks), fmt_secs(len)))
                        .font(body(11.5))
                        .color(DIM),
                );
            });
        });

        // ── Shape ──
        ui.add_space(4.0);
        let n = 7.0;
        let kw = ((inner.width() - 12.0 * (n - 1.0)) / n).clamp(70.0, 150.0);
        let mut started = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let ed = &mut e.edit;
            let k = |ui: &mut Ui, label: &str, text: String, v: &mut f32, enabled: bool| -> (bool, bool) {
                let r = knob(ui, label, text, v, kw, col, enabled);
                (r.changed(), r.drag_started() || r.clicked())
            };
            let mut g = (ed.gain + 24.0) / 36.0;
            let (c, s) = k(
                ui,
                "GAIN",
                if ed.normalize {
                    "auto".into()
                } else {
                    format!("{:+.1} dB", ed.gain)
                },
                &mut g,
                !ed.normalize,
            );
            if c {
                ed.gain = g * 36.0 - 24.0;
            }
            started |= s;
            let mut pt = (ed.pitch + 24.0) / 48.0;
            let (c, s) = k(ui, "PITCH", format!("{:+.0} st", ed.pitch), &mut pt, true);
            if c {
                ed.pitch = (pt * 48.0 - 24.0).round();
            }
            started |= s;
            let mut f = (ed.fade_in / 0.5).sqrt();
            let (c, s) = k(ui, "FADE IN", fmt_secs(ed.fade_in), &mut f, true);
            if c {
                ed.fade_in = f * f * 0.5;
            }
            started |= s;
            let mut f = (ed.fade_out / 2.0).sqrt();
            let (c, s) = k(ui, "FADE OUT", fmt_secs(ed.fade_out), &mut f, true);
            if c {
                ed.fade_out = f * f * 2.0;
            }
            started |= s;
            let mut lc = (ed.low_cut / 20.0).ln() / 100f32.ln();
            let (c, s) = k(
                ui,
                "LOW CUT",
                if ed.low_cut <= 25.0 {
                    "off".into()
                } else {
                    format!("{:.0} Hz", ed.low_cut)
                },
                &mut lc,
                true,
            );
            if c {
                ed.low_cut = 20.0 * 100f32.powf(lc);
            }
            started |= s;
            let mut hc = (ed.high_cut / 500.0).ln() / 40f32.ln();
            let (c, s) = k(
                ui,
                "HIGH CUT",
                if ed.high_cut >= 19_000.0 {
                    "off".into()
                } else {
                    format!("{:.1} kHz", ed.high_cut / 1000.0)
                },
                &mut hc,
                true,
            );
            if c {
                ed.high_cut = 500.0 * 40f32.powf(hc);
            }
            started |= s;
            let mut cr = ed.crush;
            let (c, s) = k(
                ui,
                "CRUSH",
                if ed.crush < 0.01 {
                    "off".into()
                } else {
                    format!("{:.0} %", ed.crush * 100.0)
                },
                &mut cr,
                true,
            );
            if c {
                ed.crush = cr;
            }
            started |= s;
        });
        if started {
            self.checkpoint(&orig);
        }

        // ── Slices ──
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("SLICES").font(bold(11.0)).color(FAINT));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Sensitivity").font(body(11.0)).color(DIM));
            bar_slider(ui, &mut self.sensitivity, 110.0, ACID).on_hover_text("Higher finds softer hits");
            if chip(ui, &format!("{}  Find hits", ph::MAGIC_WAND), false, ACID)
                .on_hover_text("Put a slice on every hit between start and end")
                .clicked()
            {
                self.checkpoint(&orig);
                e.slices = library::transients(&src.data, ks, ke, self.sensitivity);
                let n = e.slices.len() + 1;
                self.say(
                    if n > 1 {
                        format!("{n} slices – click one to hear it")
                    } else {
                        "No hits found – try a higher sensitivity".into()
                    },
                    now,
                );
            }
            if !e.slices.is_empty() {
                if chip(ui, &format!("{}  Clear", ph::X), false, TEXT).clicked() {
                    self.checkpoint(&orig);
                    e.slices.clear();
                }
                let n = e.slices.len() + 1;
                if chip(ui, &format!("{}  Save {n} slices as sounds", ph::SCISSORS), false, ACID)
                    .on_hover_text("Every slice becomes a sound of its own, on the same track")
                    .clicked()
                {
                    let mut edges = vec![ks];
                    edges.extend(marks.iter().copied().filter(|&m| m > ks && m < ke));
                    edges.push(ke);
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
            } else {
                ui.label(
                    egui::RichText::new("or click the strip above the waveform to add one · right-click removes")
                        .font(body(11.0))
                        .color(FAINT),
                );
            }
        });
        // Slice pads: click to play, like a sampler
        if !marks.is_empty() {
            let mut edges = vec![ks];
            edges.extend(marks.iter().copied().filter(|&m| m > ks && m < ke));
            edges.push(ke);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (i, w) in edges.windows(2).enumerate() {
                    let (pr, resp) = ui.allocate_exact_size(vec2(54.0, 40.0), Sense::click());
                    let hot = resp.hovered();
                    let on = self
                        .playing
                        .as_ref()
                        .is_some_and(|pl| pl.id == id && !pl.whole && (pl.from - w[0]).abs() < 1e-4);
                    ui.painter().rect_filled(
                        pr,
                        8.0,
                        mix(
                            PANEL,
                            col,
                            if on {
                                0.55
                            } else if hot {
                                0.3
                            } else {
                                0.15
                            },
                        ),
                    );
                    ui.painter().text(
                        pr.left_top() + vec2(6.0, 4.0),
                        Align2::LEFT_TOP,
                        format!("{}", i + 1),
                        bold(11.0),
                        TEXT,
                    );
                    ui.painter().text(
                        pr.right_bottom() - vec2(6.0, 4.0),
                        Align2::RIGHT_BOTTOM,
                        fmt_secs(w[1] - w[0]),
                        body(9.5),
                        DIM,
                    );
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.play(lib, id, Some((w[0], w[1])), out, now);
                    }
                }
            });
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
            if self.playing.as_ref().is_some_and(|pl| pl.id == id) {
                self.stop(out);
            } else {
                self.play(lib, id, None, out, now);
            }
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

    fn replace_window(&mut self, ui: &mut Ui, lib: &Library, slots: &mut SlotIds) {
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
