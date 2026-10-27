// Release builds get no console window on Windows; debug builds keep it so
// `println!`/panics stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use egui_template::{APP_NAME, TemplateApp};

/// The window/taskbar icon, embedded at compile time — the source tree is not
/// present on an end user's machine, so it cannot be loaded from disk.
const ICON_BYTES: &[u8] = include_bytes!("../resources/icon.png");

fn load_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory_with_format(ICON_BYTES, image::ImageFormat::Png).ok()?;
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();

    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([960.0, 640.0])
        .with_min_inner_size([480.0, 320.0]);

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(TemplateApp::new(cc)))),
    )
}
