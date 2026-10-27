// Release builds get no console window on Windows; debug builds keep it so
// `println!`/panics stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use open_draw_chemistry::{APP_NAME, OpenDrawApp};

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
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([640.0, 420.0]);

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        // The 3D viewer renders through wgpu; glow (the default) has no render state for it.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    // Files to open, as passed by the desktop (`%F`) or a drop onto the executable.
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(OpenDrawApp::new(cc, files)))),
    )
}
