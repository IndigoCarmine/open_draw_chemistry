//! Library half of the template.
//!
//! Keeping the app in a library (rather than entirely in `main.rs`) means the
//! state and update logic stay unit-testable — see the tests at the bottom of
//! [`app`].

pub mod app;

pub use app::TemplateApp;

/// Display name, used for the window title and the eframe persistence key.
pub const APP_NAME: &str = "egui Template";
