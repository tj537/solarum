// On Windows, don't open a console window next to the app (release builds only)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod controls;
mod ear;
mod engine;
mod face;
mod file;
mod form;
mod gen;
mod home;
mod library;
mod library_ui;
mod log;
mod midi;
#[cfg(test)]
mod offscreen;
mod orbit;
mod pads;
mod planets;
// Local-only extras (not in git), built with --cfg reel
#[cfg(all(test, reel))]
mod reel {
    include!("../marketing/reel.rs");
}
mod rng;
mod sampler;
#[cfg(test)]
mod snapshot;
mod song;
mod synth;
mod theme;
mod update;
mod visual;
mod wav;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([1100.0, 700.0])
        .with_title("Solarum");
    // Icon for the window, taskbar and Dock (when started without the .app bundle)
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    move_old_storage();
    eframe::run_native("Solarum", options, Box::new(|cc| Ok(Box::new(app::SolarumApp::new(cc)))))
}

/// Saved grooves and settings used to live under the old name, "Bunker": move them over once
fn move_old_storage() {
    let (Some(old), Some(new)) = (eframe::storage_dir("Bunker"), eframe::storage_dir("Solarum")) else {
        return;
    };
    if !old.exists() || new.exists() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::rename(&old, &new).is_ok() {
        // On Windows the data sits one folder deeper (…\Bunker\data); drop the empty folder left behind
        if let Some(parent) = old.parent().filter(|p| p.ends_with("Bunker")) {
            let _ = std::fs::remove_dir(parent);
        }
    }
}
