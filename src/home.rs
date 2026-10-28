//! The start screen: what Solarum shows when it opens, and behind the logo – a new project,
//! the last session, opening a file, letting Solari play, the recent projects as cards with
//! a little picture of each groove, settings and help

use crate::file::SolarumFile;
use crate::song::{Song, Vibe, Voice, STEPS, TRACKS};
use crate::theme::*;
use crate::widgets::{fit_text, glow};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Rect, Sense, Ui};
use egui_phosphor::regular as ph;
use std::path::PathBuf;

/// A project from the recent list, read once when the start screen opens
pub struct Recent {
    pub path: PathBuf,
    pub name: String,
    /// When it was last saved, in words ("today", "3 days ago")
    pub when: String,
    /// None: the file is gone or can't be read
    pub song: Option<Song>,
}

/// What a click on the start screen asks for
pub enum Action {
    New,
    Continue,
    Open,
    /// Let Solari play – in this vibe, or one it picks
    Auto(Option<Vibe>),
    Recent(usize),
    Forget(usize),
    Settings,
    Help,
    /// The SOUNDS screen: your library
    Sounds,
}

/// Read the recent projects (their grooves for the pictures, and when they were saved)
pub fn load_recent(paths: &[PathBuf]) -> Vec<Recent> {
    paths
        .iter()
        .map(|path| {
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let when = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| match d.as_secs() / 86_400 {
                    0 => "today".to_string(),
                    1 => "yesterday".to_string(),
                    n => format!("{n} days ago"),
                })
                .unwrap_or_else(|| "missing".to_string());
            let song = SolarumFile::load(path).ok().map(|f| f.song);
            Recent {
                path: path.clone(),
                name,
                when,
                song,
            }
        })
        .collect()
}

/// A little solar system of a groove: its tracks as tilted orbits, its steps as dots on them,
/// turning by `turn` (radians; 0 for a still picture)
pub fn preview(p: &egui::Painter, r: Rect, song: &Song, turn: f32) {
    let c = r.center();
    let rx = (r.width() * 0.47).min(r.height() * 1.25);
    for (t, track) in song.tracks.iter().enumerate() {
        let f = 0.3 + 0.7 * (t as f32 + 0.5) / TRACKS as f32;
        let (ox, oy) = (rx * f, rx * f * 0.34);
        let color = track_color(Voice::ALL[t]);
        let ring: Vec<_> = (0..=64)
            .map(|i| {
                let a = i as f32 / 64.0 * std::f32::consts::TAU;
                c + vec2(a.cos() * ox, a.sin() * oy)
            })
            .collect();
        p.add(egui::Shape::line(
            ring,
            stroke(1.0, color.gamma_multiply(if track.mute { 0.08 } else { 0.22 })),
        ));
        // Inner orbits turn a little faster, like real ones
        let spin = turn * (1.6 - f);
        for (s, st) in track.steps.iter().enumerate().filter(|(_, st)| st.on) {
            let a = -std::f32::consts::FRAC_PI_2 + s as f32 / STEPS as f32 * std::f32::consts::TAU + spin;
            let at = c + vec2(a.cos() * ox, a.sin() * oy);
            let near = 0.75 + 0.25 * a.sin();
            let rad = (if st.accent { 3.2 } else { 2.3 }) * (rx / 160.0).clamp(0.6, 1.8) * near;
            p.circle_filled(at, rad, color.gamma_multiply(if track.mute { 0.3 } else { near }));
        }
    }
    glow(p, c, rx * 0.06, ACID, 0.3);
    p.circle_filled(c, rx * 0.045, mix(ACID, Color32::WHITE, 0.15));
}

/// "Let Solari play" as a little SOLAR OS screen: green on black, a monospace prompt, faint
/// scanlines, Solari's face and a blinking cursor
fn solari_button(ui: &mut Ui, r: Rect, t: f32) -> egui::Response {
    let resp = ui.interact(r, ui.id().with("auto"), Sense::click());
    let hot = resp.hovered();
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, 12.0, Color32::from_rgb(8, 11, 6));
    let mut y = r.top() + 2.0;
    while y < r.bottom() {
        p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], stroke(1.0, ACID.gamma_multiply(0.035)));
        y += 3.0;
    }
    p.rect_stroke(
        r,
        12.0,
        stroke(if hot { 1.6 } else { 1.0 }, ACID.gamma_multiply(if hot { 0.9 } else { 0.5 })),
        egui::StrokeKind::Inside,
    );
    crate::log::robot(&p, pos2(r.left() + 28.0, r.center().y - 4.0), 20.0, if hot { t } else { 0.0 }, ACID);
    let mono = |s: f32| egui::FontId::monospace(s);
    p.text(
        pos2(r.left() + 50.0, r.center().y - 9.0),
        Align2::LEFT_CENTER,
        "> Let Solari play",
        mono(13.5),
        ACID,
    );
    let galley = p.layout_no_wrap(
        "  Pick a vibe – Solari builds a set".to_string(),
        mono(10.5),
        ACID.gamma_multiply(0.6),
    );
    let w = galley.size().x;
    p.galley(pos2(r.left() + 50.0, r.center().y + 3.0), galley, ACID);
    if (t * 0.8).fract() < 0.55 {
        p.rect_filled(
            Rect::from_min_size(pos2(r.left() + 52.0 + w, r.center().y + 4.0), vec2(6.0, 11.0)),
            0.0,
            ACID.gamma_multiply(0.8),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A short line about a groove: vibe, mood, tempo
fn about(song: &Song) -> String {
    let mood = song.mood().map_or(String::new(), |k| format!(" · {}", crate::song::MOODS[k].0));
    format!("{}{mood} · {:.0} BPM", song.vibe.name(), song.bpm)
}

/// The start screen; returns what was clicked. `demo` is shown when there's nothing from last time.
pub fn home(ui: &mut Ui, sounds: usize, recent: &[Recent], last: Option<(&Song, &str)>, update: Option<&str>) -> Option<Action> {
    let mut action = None;
    let full = ui.max_rect();
    let p = ui.painter().clone();
    let t = ui.input(|i| i.time) as f32;
    ui.ctx().request_repaint();
    let margin = (full.width() * 0.07).clamp(32.0, 110.0);
    let content = full.shrink2(vec2(margin, 0.0));
    // The top: the name, the buttons, and on the right the logo as a light source
    let hero_h = (full.height() * 0.42).clamp(300.0, 400.0);
    let hero = Rect::from_min_size(
        pos2(content.left(), full.top() + (full.height() * 0.07).max(28.0)),
        vec2(content.width(), hero_h),
    );
    let side = Rect::from_min_max(pos2(hero.left() + 420.0f32.min(hero.width() * 0.5), hero.top() - 20.0), hero.max);
    let size = (side.width().min(side.height()) * 0.72).min(300.0);
    let sun = side.center();

    // Space, its stars flying out from the sun
    p.rect_filled(full, 0.0, SPACE);
    crate::widgets::warp_field(&p, full, sun, t);
    // The sun from the planets view, big: lime, its surface turning; a click warms it to red
    // (and back)
    let rs = size * 0.36;
    let sun_resp = ui.interact(
        Rect::from_center_size(sun, vec2(rs * 2.0, rs * 2.0)),
        ui.id().with("home_sun"),
        Sense::click(),
    );
    let lit_id = ui.id().with("home_sun_red");
    let red = ui.data(|d| d.get_temp::<bool>(lit_id)).unwrap_or(false);
    if sun_resp.clicked() {
        ui.data_mut(|d| d.insert_temp(lit_id, !red));
    }
    let heat = ui.ctx().animate_bool_with_time(lit_id, red, 1.5);
    let warm = mix(ACID, RED, heat);
    let pulse = 0.5 + 0.5 * (t * 2.2).sin();
    let shine = 0.16 + 0.05 * pulse + if sun_resp.hovered() { 0.08 } else { 0.0 };
    crate::planets::sun_ball(&p, sun, rs, warm, 0.0, shine, t);
    sun_resp.on_hover_cursor(egui::CursorIcon::PointingHand);

    // The left: the name, then the buttons
    let mut y = hero.top();
    p.text(pos2(hero.left() - 6.0, y), Align2::LEFT_TOP, "SOLARUM", display(132.0), TEXT);
    y += 172.0;
    let bh = 54.0;
    let gap = 12.0;
    // New project and Open side by side
    let new_r = Rect::from_min_size(pos2(hero.left(), y), vec2(220.0, bh));
    let open_r = Rect::from_min_size(pos2(new_r.right() + gap, y), vec2(130.0, bh));
    let sounds_r = Rect::from_min_size(pos2(open_r.right() + gap, y), vec2(150.0, bh));
    let row_right = sounds_r.right();
    let button = |ui: &mut Ui, r: Rect, id: &str, icon: &str, title: &str, primary: bool| {
        let resp = ui.interact(r, ui.id().with(id), Sense::click());
        let hot = resp.hovered();
        let (fill, ink) = if primary {
            (if hot { Color32::WHITE } else { mix(TEXT, Color32::WHITE, 0.3) }, BG)
        } else {
            (
                if hot {
                    Color32::from_rgba_premultiplied(16, 16, 18, 16)
                } else {
                    Color32::from_rgba_premultiplied(8, 8, 9, 8)
                },
                TEXT,
            )
        };
        p.rect_filled(r, 12.0, fill);
        if !primary {
            p.rect_stroke(
                r,
                12.0,
                stroke(1.0, if hot { alpha(TEXT, 0.35) } else { LINE }),
                egui::StrokeKind::Inside,
            );
        }
        p.text(pos2(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, icon, body(20.0), ink);
        p.text(pos2(r.left() + 48.0, r.center().y), Align2::LEFT_CENTER, title, bold(15.0), ink);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    };
    if button(ui, new_r, "new", ph::PLUS, "New project", true)
        .on_hover_text("An empty sky – build it by hand")
        .clicked()
    {
        action = Some(Action::New);
    }
    if button(ui, open_r, "open", ph::FOLDER_OPEN, "Open…", false)
        .on_hover_text("A .solarum file")
        .clicked()
    {
        action = Some(Action::Open);
    }
    let tip = match sounds {
        0 => "Your own sounds: record, import and cut them".to_string(),
        1 => "Your library: 1 sound".to_string(),
        n => format!("Your library: {n} sounds"),
    };
    if button(ui, sounds_r, "sounds", ph::WAVEFORM, "Sounds", false)
        .on_hover_text(tip)
        .clicked()
    {
        action = Some(Action::Sounds);
    }
    y += bh + gap;
    // Back to the project that's open (or was last time) – its own button, with what it is
    if let Some((song, name)) = last {
        let r = Rect::from_min_max(pos2(hero.left(), y), pos2(row_right, y + bh));
        let resp = ui.interact(r, ui.id().with("back"), Sense::click());
        let hot = resp.hovered();
        p.rect_filled(
            r,
            12.0,
            if hot {
                Color32::from_rgba_premultiplied(16, 16, 18, 16)
            } else {
                Color32::from_rgba_premultiplied(8, 8, 9, 8)
            },
        );
        p.rect_stroke(
            r,
            12.0,
            stroke(1.0, if hot { alpha(TEXT, 0.35) } else { LINE }),
            egui::StrokeKind::Inside,
        );
        p.text(
            pos2(r.left() + 18.0, r.center().y),
            Align2::LEFT_CENTER,
            ph::ARROW_LEFT,
            body(20.0),
            TEXT,
        );
        p.text(
            pos2(r.left() + 48.0, r.center().y - 9.0),
            Align2::LEFT_CENTER,
            format!("Back to {name}"),
            bold(15.0),
            TEXT,
        );
        p.text(
            pos2(r.left() + 48.0, r.center().y + 10.0),
            Align2::LEFT_CENTER,
            about(song),
            body(11.5),
            DIM,
        );
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            action = Some(Action::Continue);
        }
        y += bh + gap;
    }
    // Solari's button: a little SOLAR OS screen; a click offers a few vibes to start from
    let solari_r = Rect::from_min_max(pos2(hero.left(), y), pos2(row_right, y + 62.0));
    let solari = solari_button(ui, solari_r, t);
    let popup = ui.make_persistent_id("solari_vibes");
    if solari.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    egui::popup_below_widget(ui, popup, &solari, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(solari_r.width() - 16.0);
        ui.label(
            egui::RichText::new("What should Solari play?")
                .font(egui::FontId::monospace(12.0))
                .color(ACID),
        );
        ui.add_space(4.0);
        let examples = [
            (Some(Vibe::Hypnotic), "Hypnotic", "deep, rolling, one long trip"),
            (Some(Vibe::Acid), "Acid", "a squelching 303 line"),
            (Some(Vibe::Peak), "Peak time", "big room, hands up"),
            (Some(Vibe::Dub), "Dub", "echoes, chords, space"),
            (Some(Vibe::Melodic), "Melodic", "warm chords that move"),
            (Some(Vibe::Hard), "Hard", "fast, loud, relentless"),
            (None, "Surprise me", "Solari picks"),
        ];
        for (vibe, name, line) in examples {
            let mut job = egui::text::LayoutJob::default();
            let fmt = |font, color| egui::TextFormat {
                font_id: font,
                color,
                ..Default::default()
            };
            job.append(&format!("> {name}"), 0.0, fmt(egui::FontId::monospace(13.0), ACID));
            job.append(
                &format!("   {line}"),
                0.0,
                fmt(egui::FontId::monospace(11.0), ACID.gamma_multiply(0.55)),
            );
            if ui.selectable_label(false, job).clicked() {
                action = Some(Action::Auto(vibe));
            }
        }
    });

    // Right under the buttons and the light – the more room below, the more projects show
    let ry = (solari_r.bottom() + 56.0).max(sun.y + size * 0.5 + 24.0);
    p.text(
        pos2(content.left(), ry),
        Align2::LEFT_TOP,
        "RECENT PROJECTS",
        bold(11.0),
        alpha(TEXT, 0.5),
    );
    let grid_top = ry + 26.0;
    let footer_h = 46.0;
    if recent.is_empty() {
        p.text(
            pos2(content.left(), grid_top + 4.0),
            Align2::LEFT_TOP,
            "Projects you save or open show up here – File → Save, or ⌘/Ctrl + S.",
            body(13.0),
            DIM,
        );
    } else {
        let gap = 16.0;
        // As many as fit: more columns on a wide window, more rows on a tall one
        let cols = ((content.width() + gap) / (200.0 + gap)).floor().clamp(2.0, 8.0) as usize;
        let cw = (content.width() - gap * (cols - 1) as f32) / cols as f32;
        let ch = (cw * 0.55 + 52.0).min(180.0);
        let rows = (((full.bottom() - footer_h - grid_top + gap) / (ch + gap)).floor() as usize).max(1);
        for (i, rec) in recent.iter().enumerate().take(cols * rows) {
            let r = Rect::from_min_size(
                pos2(
                    content.left() + (i % cols) as f32 * (cw + gap),
                    grid_top + (i / cols) as f32 * (ch + gap),
                ),
                vec2(cw, ch),
            );
            let resp = ui.interact(r, ui.id().with(("recent", i)), Sense::click());
            let ok = rec.song.is_some();
            let hot = ok && resp.hovered();
            // A solid card, in front of the sun: the picture on top, the name and a line below
            p.rect_filled(r, 14.0, if hot { mix(SPACE, PANEL, 0.55) } else { mix(SPACE, PANEL, 0.35) });
            p.rect_stroke(
                r,
                14.0,
                stroke(1.0, if hot { alpha(TEXT, 0.35) } else { LINE }),
                egui::StrokeKind::Inside,
            );
            let pic = Rect::from_min_max(r.min + vec2(6.0, 6.0), pos2(r.right() - 6.0, r.bottom() - 52.0));
            p.rect_filled(pic, 10.0, SPACE);
            match &rec.song {
                Some(song) => preview(&p.with_clip_rect(pic), pic.shrink(8.0), song, if hot { t * 0.3 } else { 0.0 }),
                None => {
                    p.text(
                        pic.center(),
                        Align2::CENTER_CENTER,
                        format!("{}  File not found", ph::WARNING_CIRCLE),
                        body(12.0),
                        FAINT,
                    );
                }
            }
            let name = fit_text(&p, &rec.name, bold(13.5), cw - 24.0);
            p.text(
                pos2(r.left() + 12.0, r.bottom() - 34.0),
                Align2::LEFT_CENTER,
                name,
                bold(13.5),
                if ok { TEXT } else { FAINT },
            );
            let line = match &rec.song {
                Some(song) => format!("{} · {}", rec.when, about(song)),
                None => "Removed or moved – right-click to forget it".to_string(),
            };
            let line = fit_text(&p, &line, body(11.0), cw - 24.0);
            p.text(pos2(r.left() + 12.0, r.bottom() - 15.0), Align2::LEFT_CENTER, line, body(11.0), DIM);
            let resp = resp.on_hover_text(rec.path.display().to_string()).on_hover_cursor(if ok {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::Default
            });
            if resp.clicked() && ok {
                action = Some(Action::Recent(i));
            }
            resp.context_menu(|ui| {
                if ui.button(format!("{}  Remove from the list", ph::X)).clicked() {
                    action = Some(Action::Forget(i));
                    ui.close_menu();
                }
            });
        }
    }

    // The footer: settings and help on the left, the version (and an update) on the right
    let fy = full.bottom() - footer_h / 2.0;
    let mut x = content.left() - 10.0;
    for (icon, label, act) in [
        (ph::GEAR_SIX, "Settings", Action::Settings),
        (ph::QUESTION, "How it works", Action::Help),
    ] {
        let text = format!("{icon}  {label}");
        let galley = p.layout_no_wrap(text.clone(), bold(12.5), TEXT);
        let r = Rect::from_center_size(pos2(x + galley.size().x / 2.0 + 10.0, fy), galley.size() + vec2(20.0, 12.0));
        let resp = ui.interact(r, ui.id().with(("foot", label)), Sense::click());
        let hot = resp.hovered();
        if hot {
            p.rect_filled(r, 8.0, Color32::from_rgba_premultiplied(14, 14, 16, 14));
        }
        p.text(r.center(), Align2::CENTER_CENTER, text, bold(12.5), if hot { TEXT } else { DIM });
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            action = Some(act);
        }
        x = r.right() + 4.0;
    }
    let version = match update {
        Some(u) => u.to_string(),
        None => concat!("Solarum ", env!("CARGO_PKG_VERSION")).to_string(),
    };
    p.text(
        pos2(content.right(), fy),
        Align2::RIGHT_CENTER,
        version,
        body(12.0),
        if update.is_some() { ACID } else { FAINT },
    );
    action
}
