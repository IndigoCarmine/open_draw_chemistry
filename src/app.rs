//! The application state and its per-frame update logic.
//!
//! Replace the demo widgets in [`TemplateApp::update`] with your own UI; the
//! surrounding scaffolding (persistence, panels, about dialog) is what the
//! template is really providing.

use serde::{Deserialize, Serialize};

/// Persisted application state.
///
/// `#[serde(default)]` matters for a real app: it lets you add fields later
/// without invalidating the state files already on your users' machines.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateApp {
    label: String,
    value: f32,
    counter: i64,

    /// Transient UI state — recomputed each launch rather than persisted.
    #[serde(skip)]
    show_about: bool,
}

impl Default for TemplateApp {
    fn default() -> Self {
        Self {
            label: "Hello, egui!".to_owned(),
            value: 2.7,
            counter: 0,
            show_about: false,
        }
    }
}

impl TemplateApp {
    /// Builds the app, restoring persisted state when eframe has some.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Slightly roomier than the default; tweak to taste.
        cc.egui_ctx.set_pixels_per_point(1.1);

        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }
        Self::default()
    }

    /// Increment the counter. Split out from the UI so it can be tested.
    fn increment(&mut self) {
        self.counter = self.counter.saturating_add(1);
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl eframe::App for TemplateApp {
    /// Called by eframe on a timer and at shutdown (needs the `persistence` feature).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// egui 0.35 hands the app a root [`egui::Ui`] rather than a `Context`;
    /// panels are nested inside it.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Cloning a Context is cheap (it is an Arc) and frees up `ui` for the
        // panels below, since `egui::Window` still wants a `&Context`.
        let ctx = ui.ctx().clone();

        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Reset state").clicked() {
                        self.reset();
                        ui.close();
                    }
                    ui.separator();
                    // Quitting from a menu is desktop-only; on web there is no window to close.
                    if !cfg!(target_arch = "wasm32") && ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.show_about = true;
                        ui.close();
                    }
                });

                ui.add_space(16.0);
                egui::widgets::global_theme_preference_buttons(ui);
            });
        });

        egui::Panel::left("side_panel").show(ui, |ui| {
            ui.heading("Controls");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Label:");
                ui.text_edit_singleline(&mut self.label);
            });

            ui.add(egui::Slider::new(&mut self.value, 0.0..=10.0).text("value"));

            if ui.button("Increment").clicked() {
                self.increment();
            }
            ui.label(format!("Counter: {}", self.counter));

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label("powered by ");
                    ui.hyperlink_to("egui", "https://github.com/emilk/egui");
                });
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(crate::APP_NAME);
            ui.separator();
            ui.label(&self.label);
            ui.label(format!("value = {:.2}", self.value));
            ui.separator();
            ui.label("Edit src/app.rs to start building your app.");
        });

        if self.show_about {
            let mut open = self.show_about;
            egui::Window::new("About")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.heading(crate::APP_NAME);
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.separator();
                    ui.label(env!("CARGO_PKG_DESCRIPTION"));
                    ui.hyperlink(env!("CARGO_PKG_REPOSITORY"));
                });
            self.show_about = open;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_advances_the_counter() {
        let mut app = TemplateApp::default();
        assert_eq!(app.counter, 0);
        app.increment();
        app.increment();
        assert_eq!(app.counter, 2);
    }

    #[test]
    fn increment_saturates_instead_of_overflowing() {
        let mut app = TemplateApp {
            counter: i64::MAX,
            ..Default::default()
        };
        app.increment();
        assert_eq!(app.counter, i64::MAX);
    }

    #[test]
    fn reset_restores_defaults() {
        let mut app = TemplateApp {
            label: "changed".to_owned(),
            ..Default::default()
        };
        app.increment();
        app.reset();
        assert_eq!(app.counter, 0);
        assert_eq!(app.label, TemplateApp::default().label);
    }

    /// Guards the persistence contract: state must survive a serialize round-trip,
    /// which is what eframe does between launches.
    #[test]
    fn state_survives_a_serde_round_trip() {
        let app = TemplateApp {
            label: "persisted".to_owned(),
            value: 4.5,
            counter: 7,
            show_about: true,
        };

        let json = serde_json::to_string(&app).expect("serialize");
        let restored: TemplateApp = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.label, "persisted");
        assert_eq!(restored.counter, 7);
        // `show_about` is #[serde(skip)], so it comes back at its default.
        assert!(!restored.show_about);
    }
}
