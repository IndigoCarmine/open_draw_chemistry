//! Open DrawChemistry: a chemical structure and reaction drawing application.
//!
//! The app lives in a library (rather than entirely in `main.rs`) so its state and logic stay
//! unit-testable. The structure editor itself is chembuider-rs's widget; this crate arranges
//! it into a window and handles documents and files.

pub mod app;
pub mod document;
pub mod io;
pub mod theme;
pub mod three_d;
pub mod ui;

pub use app::OpenDrawApp;

/// Display name, used for the window title and the eframe persistence key.
pub const APP_NAME: &str = "Open DrawChemistry";
