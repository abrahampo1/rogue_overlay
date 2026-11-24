//! Rogue Overlay — mana skins, skill cooldowns and ore timer overlay for Rogue Lineage,
//! with a control panel (skin catalog, cooldowns, ore timer, settings).
//!
//! The overlay runs on its own thread with a Win32 message loop; the panel (egui) on the
//! main thread. They talk through `shared::Shared` (config + commands + live status).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod config;
mod overlay;
mod panel;
mod render;
mod shared;
mod skins;
mod win;

use eframe::egui;

fn main() {
    win::enable_dpi_awareness();
    let cfg = config::load().unwrap_or_else(|e| {
        win::message_box(&format!("{e}\n\nUsing the default settings."));
        config::Config::default()
    });
    skins::ensure_examples();

    let shared = shared::new(cfg);
    let overlay = overlay::spawn(shared.clone());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rogue Overlay")
            .with_inner_size([760.0, 640.0])
            .with_min_inner_size([560.0, 420.0]),
        ..Default::default()
    };
    let panel_shared = shared.clone();
    if let Err(e) = eframe::run_native("Rogue Overlay", options, Box::new(move |_cc| Ok(Box::new(panel::Panel::new(panel_shared))))) {
        win::message_box(&format!("Could not open the panel: {e}"));
    }

    // Closing the panel also closes the overlay. Wait for it (it uninstalls the hooks) but never
    // more than 2 s, then exit no matter what so closing can never hang.
    if let Ok(mut s) = shared.lock() {
        s.panel_closed = true;
        s.cmds.push(shared::Cmd::Quit);
        let _ = config::save(&s.cfg);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = overlay.join();
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    std::process::exit(0);
}
