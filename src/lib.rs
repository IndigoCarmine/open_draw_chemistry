//! Open DrawChemistry: a chemical structure and reaction drawing application.
//!
//! The app lives in a library (rather than entirely in `main.rs`) so its state and logic stay
//! unit-testable. The structure editor itself is chembuider-rs's widget; this crate arranges
//! it into a window and handles documents and files.

// First, so its `t!` and `tn!` macros are in scope in every module after it.
#[macro_use]
pub mod i18n;

pub mod app;
pub mod assembly;
pub mod document;
pub mod fragments;
pub mod io;
pub mod iupac;
pub mod ole;
pub mod ole_server;
pub mod scan;
pub mod smiles;
pub mod theme;
pub mod three_d;
pub mod ui;

pub use app::OpenDrawApp;

/// Display name, used for the window title and the eframe persistence key.
pub const APP_NAME: &str = "Open DrawChemistry";

/// Version given by the release tag; a local build adds -dev to Cargo.toml's version.
pub const APP_VERSION: &str = env!("APP_VERSION");

#[cfg(test)]
mod version_tests {
    #[test]
    fn app_version_has_three_numeric_components() {
        assert!(!super::APP_VERSION.is_empty());
        let version = super::APP_VERSION.split('-').next().unwrap();
        let components: Vec<_> = version.split('.').collect();
        assert_eq!(components.len(), 3);
        assert!(components.iter().all(|component| {
            !component.is_empty() && component.chars().all(|c| c.is_ascii_digit())
        }));
    }
}
