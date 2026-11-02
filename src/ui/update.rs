//! The update's dialogs: the Yes/No question at startup, the download's progress and, when the
//! restart was called off, the question of what to do with the verified download.

use crate::update::flow::{Event, UpdateFlow, View};
use crate::update::release::Plan;

/// Draw whatever the update has to show, passing the user's answers on to `flow`. What comes back
/// is for the app to carry out.
pub fn show(ctx: &egui::Context, flow: &mut UpdateFlow) -> Option<Event> {
    match flow.view() {
        View::Nothing => None,
        View::Prompt(available) => {
            let mut answer = None;
            let modal = egui::Modal::new(egui::Id::unique("update_available")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading(t!("update.prompt.title"));
                ui.label(t!(
                    "update.prompt.message",
                    version = available.version,
                    current = crate::APP_VERSION
                ));
                ui.hyperlink_to(t!("update.prompt.release_notes"), &available.release_page);
                ui.add_space(4.0);
                ui.label(match available.plan {
                    Plan::Install(_) => t!("update.prompt.install"),
                    Plan::ReleasePage(_) => t!("update.prompt.open_page"),
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(t!("update.prompt.yes")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(t!("update.prompt.no")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            if modal.should_close() && answer.is_none() {
                answer = Some(false);
            }
            answer.and_then(|yes| flow.answer(yes))
        }
        View::Downloading {
            version,
            done,
            total,
        } => {
            let mut cancel = false;
            corner_window(t!("update.download.title")).show(ctx, |ui| {
                ui.label(t!("update.download.message", version = version));
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(280.0)
                        .show_percentage(),
                );
                cancel = ui.button(t!("common.cancel")).clicked();
            });
            if cancel {
                flow.cancel();
            }
            None
        }
        View::Ready => {
            let (mut restart, mut discard) = (false, false);
            corner_window(t!("update.ready.title")).show(ctx, |ui| {
                ui.label(t!("update.ready.message"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    restart = ui.button(t!("update.ready.restart")).clicked();
                    discard = ui.button(t!("common.cancel")).clicked();
                });
            });
            if discard {
                flow.discard();
            }
            if restart { flow.restart() } else { None }
        }
    }
}

/// Stays out of the way of the drawing: the work goes on while an update downloads.
fn corner_window(title: &str) -> egui::Window<'_> {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
}
