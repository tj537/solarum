//! Colours, fonts and the egui style: concrete around, deep space in the middle,
//! neon track colours.

use crate::song::Voice;
use eframe::egui::{self, Color32, FontFamily, FontId, Stroke};

/// Concrete: the walls around everything
pub const BG: Color32 = Color32::from_rgb(57, 57, 59);
/// Recessed into the concrete (cards, fields)
pub const PANEL: Color32 = Color32::from_rgb(44, 44, 47);
pub const PANEL_HI: Color32 = Color32::from_rgb(70, 70, 74);
pub const LINE: Color32 = Color32::from_rgb(84, 84, 90);
pub const TEXT: Color32 = Color32::from_rgb(240, 240, 243);
pub const DIM: Color32 = Color32::from_rgb(180, 180, 187);
pub const FAINT: Color32 = Color32::from_rgb(150, 150, 158);
/// The dark inside of a pad, card or button (what black at 80/255 makes of the concrete)
pub const PAD: Color32 = Color32::from_rgb(39, 39, 41);
/// Deep space: the window in the middle and the visualizer
pub const SPACE: Color32 = Color32::from_rgb(5, 6, 13);
pub const RED: Color32 = Color32::from_rgb(255, 59, 70);
pub const ACID: Color32 = Color32::from_rgb(200, 255, 61);

pub fn track_color(v: Voice) -> Color32 {
    match v {
        Voice::Kick => Color32::from_rgb(255, 77, 94),
        Voice::Clap => Color32::from_rgb(255, 154, 60),
        Voice::Rim => Color32::from_rgb(255, 216, 74),
        Voice::Hat => Color32::from_rgb(89, 240, 184),
        Voice::OpenHat => Color32::from_rgb(69, 200, 255),
        Voice::Ride => Color32::from_rgb(140, 125, 255),
        Voice::Bass => Color32::from_rgb(211, 107, 255),
        Voice::Acid => ACID,
    }
}

/// `c` with alpha 0…1 (egui colours are premultiplied)
pub fn alpha(c: Color32, a: f32) -> Color32 {
    let a = a.clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let add = |fonts: &mut egui::FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), egui::FontData::from_static(bytes).into());
    };
    add(&mut fonts, "grotesk", include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"));
    add(&mut fonts, "grotesk-bold", include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf"));
    add(&mut fonts, "anton", include_bytes!("../assets/fonts/Anton-Regular.ttf"));
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    let fallback: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    let prop = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
    prop.insert(0, "grotesk".into());
    for (family, first) in [("bold", "grotesk-bold"), ("display", "anton")] {
        let mut list = vec![first.to_string()];
        list.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(egui::TextStyle::Body, body(14.0));
    style.text_styles.insert(egui::TextStyle::Button, body(14.0));
    style.text_styles.insert(egui::TextStyle::Small, body(11.5));
    style.text_styles.insert(egui::TextStyle::Heading, bold(20.0));
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.slider_rail_height = 4.0;
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = PANEL_HI;
    v.window_stroke = stroke(1.0, LINE);
    v.window_corner_radius = 14.0.into();
    v.menu_corner_radius = 10.0.into();
    v.override_text_color = Some(TEXT);
    // Opaque: `alpha` blends in linear light and turns the lime far too bright for the
    // ACID text of a picked entry on it
    v.selection.bg_fill = mix(PANEL, ACID, 0.22);
    v.selection.stroke = stroke(1.0, ACID);
    v.hyperlink_color = ACID;
    for (w, fill) in [
        (&mut v.widgets.noninteractive, PANEL),
        (&mut v.widgets.inactive, PANEL_HI),
        (&mut v.widgets.hovered, Color32::from_rgb(80, 80, 86)),
        (&mut v.widgets.active, Color32::from_rgb(92, 92, 100)),
        (&mut v.widgets.open, PANEL_HI),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = 8.0.into();
        w.bg_stroke = stroke(1.0, LINE);
        w.fg_stroke = stroke(1.5, TEXT);
    }
    v.widgets.noninteractive.fg_stroke = stroke(1.0, DIM);
    ctx.set_style(style);
}

pub fn stroke(width: f32, color: Color32) -> Stroke {
    Stroke::new(width, color)
}
