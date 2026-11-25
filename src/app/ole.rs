//! The app's side of the OLE server (see [`crate::ole_server`]): embedded objects open as
//! tabs; saving such a tab writes into the container's document, closing it ends the editing
//! session, and a COM-launched process quits once no embedded object is left.
//!
//! An embedded 3D model opens in the 3D view, shown as it was (style, camera). Writing it back
//! draws its picture off screen from the look the view keeps up to date (see
//! [`OpenDrawApp::track_embedded_look`]), so it can be written whatever view is shown.

use std::time::Instant;

use super::OpenDrawApp;
use crate::document::{Document, Embedded};
use crate::io::{self, Format};
use crate::ole_server::model3d::Object3d;
use crate::ole_server::script::Step;
use crate::ole_server::{AppBridge, HostNames, Native, ObjectId, ToApp, ToServer};
use crate::ui::ViewMode;
use crate::ui::viewport3d::{Picture, Viewport3d};

impl OpenDrawApp {
    /// Serve embedded objects through `bridge` (the process is the OLE server).
    pub fn with_ole(mut self, bridge: AppBridge, ctx: &egui::Context) -> Self {
        bridge.attach(ctx);
        // The window opens for the embedded object, not for an empty drawing.
        self.docs.retain(|d| !d.is_pristine());
        self.active = 0;
        self.ole = Some(bridge);
        self
    }

    fn embedded_index(&self, id: ObjectId) -> Option<usize> {
        self.docs
            .iter()
            .position(|d| d.embedded.as_ref().is_some_and(|e| e.id == id))
    }

    /// Handle what the server sent; called every frame, also while the window is hidden.
    pub(super) fn poll_ole(&mut self, ctx: &egui::Context) {
        let Some(bridge) = &self.ole else { return };
        let mut messages = Vec::new();
        while let Some(m) = bridge.try_recv() {
            messages.push(m);
        }
        for message in messages {
            crate::ole_server::log(&format!("app: {}", message.summary()));
            match message {
                ToApp::Open { id, cdx, host } => self.open_embedded(ctx, id, cdx, &host),
                ToApp::Open3d { id, object, host } => {
                    self.open_embedded_3d(ctx, id, object.map(|o| *o), &host)
                }
                ToApp::Close { id, reply } => {
                    if let Some(i) = self.embedded_index(id) {
                        if let Some(reply) = reply {
                            let native = if self.docs[i].is_dirty() {
                                self.commit_embedded(i).ok()
                            } else {
                                None
                            };
                            let _ = reply.send(native);
                        }
                        self.remove_tab(i);
                    }
                }
                ToApp::Idle => {
                    if let Some(bridge) = &mut self.ole {
                        bridge.idle = true;
                    }
                }
            }
        }
        self.run_ole_script();
        self.send_live_updates(ctx);
        self.quit_when_served(ctx);
    }

    /// Where the container shows edits at once (see [`AppBridge::live_updates`]), write them
    /// back a moment after they are made, between gestures, as if they were saved.
    fn send_live_updates(&mut self, ctx: &egui::Context) {
        let Some(bridge) = &mut self.ole else { return };
        let Some(delay) = bridge.live_updates else {
            return;
        };
        let edited: Vec<usize> = (0..self.docs.len())
            .filter(|&i| self.docs[i].embedded.is_some() && self.docs[i].is_dirty())
            .collect();
        if edited.is_empty() {
            bridge.edited_at = None;
            return;
        }
        let waited = bridge.edited_at.get_or_insert_with(Instant::now).elapsed();
        let wait = delay.saturating_sub(waited);
        if !wait.is_zero() || ctx.input(|i| i.pointer.any_down()) {
            ctx.request_repaint_after(wait.max(delay / 4));
            return;
        }
        bridge.edited_at = None;
        for i in edited {
            self.save_embedded(i);
        }
    }

    fn open_embedded(
        &mut self,
        ctx: &egui::Context,
        id: ObjectId,
        cdx: Option<Vec<u8>>,
        host: &HostNames,
    ) {
        if let Some(bridge) = &mut self.ole {
            bridge.seen_object = true;
        }
        if let Some(i) = self.embedded_index(id) {
            self.active = i;
        } else {
            let loaded = match cdx.as_deref().map(|c| io::decode(Format::Cdx, c)) {
                Some(Ok(loaded)) => Some(loaded),
                Some(Err(e)) => {
                    self.status = t!("ole.embed.load_failed", error = e);
                    None
                }
                None => None,
            };
            let embedded = Embedded {
                id,
                container: host.short_app(),
                document: host.document.clone(),
                model3d: None,
            };
            let doc = Document::embedded(&self.config, loaded, embedded);
            if self.active_doc().is_some_and(Document::is_pristine) {
                self.docs[self.active] = doc;
            } else {
                self.docs.push(doc);
                self.active = self.docs.len() - 1;
            }
            self.status = t!("ole.embed.editing", app = host.short_app());
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Open the embedded 3D model `id` (or bring its tab to the front): its model in the 3D
    /// view, in the style and from the camera it was shown with.
    fn open_embedded_3d(
        &mut self,
        ctx: &egui::Context,
        id: ObjectId,
        object: Option<Object3d>,
        host: &HostNames,
    ) {
        if let Some(bridge) = &mut self.ole {
            bridge.seen_object = true;
        }
        if let Some(i) = self.embedded_index(id) {
            self.active = i;
        } else {
            let embedded = Embedded {
                id,
                container: host.short_app(),
                document: host.document.clone(),
                model3d: None,
            };
            let (doc, error) = Document::embedded_3d(&self.config, object.as_ref(), embedded);
            if let Some(object) = &object {
                self.view3d.style = object.look.style;
            }
            if self.active_doc().is_some_and(Document::is_pristine) {
                self.docs[self.active] = doc;
            } else {
                self.docs.push(doc);
                self.active = self.docs.len() - 1;
            }
            self.status = match error {
                Some(e) => t!("ole.embed.load_failed", error = e),
                None => t!("ole.embed.editing_3d", app = host.short_app()),
            };
        }
        self.view = ViewMode::ThreeD;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// The picture of the embedded 3D model at `index`, drawn off screen as its look says;
    /// `None` without a GPU or a model (the object then keeps the picture it has).
    fn embedded_3d_picture(&self, index: usize) -> Option<Picture> {
        let render_state = self.render_state.as_ref()?;
        let doc = self.docs.get(index)?;
        let model = doc.three_d.model()?;
        let look = &doc.embedded.as_ref()?.model3d.as_ref()?.look;
        let [w, h] = look.view_points;
        match Viewport3d::render_picture(
            render_state,
            model,
            look.style,
            look.camera.as_ref(),
            egui::vec2(w, h),
            look.picture_px_per_point(),
        ) {
            Ok(picture) => Some(picture),
            Err(e) => {
                crate::ole_server::log(&format!("app: no picture of the 3D model: {e}"));
                None
            }
        }
    }

    /// The embedded document at `index` as it is to be written back (a 2D structure's CDX, or
    /// a 3D model with its look and picture), marked saved.
    fn commit_embedded(&mut self, index: usize) -> Result<Native, String> {
        let is_3d = self.docs[index]
            .embedded
            .as_ref()
            .is_some_and(|e| e.model3d.is_some());
        if is_3d {
            let picture = self.embedded_3d_picture(index);
            self.docs[index]
                .commit_embedded_3d(picture.as_ref())
                .map(|object| Native::Model3d(Box::new(object)))
        } else {
            self.docs[index]
                .commit_embedded()
                .map(Native::Cdx)
                .map_err(|e| e.to_string())
        }
    }

    /// Write the embedded document at `index` back into its container. Returns whether it was.
    pub(super) fn save_embedded(&mut self, index: usize) -> bool {
        let Some(id) = self.docs[index].embedded.as_ref().map(|e| e.id) else {
            return false;
        };
        match self.commit_embedded(index) {
            Ok(native) => {
                let container = self.docs[index]
                    .embedded
                    .as_ref()
                    .map(|e| e.container.clone());
                if let Some(bridge) = &self.ole {
                    bridge.send(match native {
                        Native::Cdx(cdx) => ToServer::Update { id, cdx },
                        Native::Model3d(object) => ToServer::Update3d { id, object },
                    });
                }
                self.status = t!("ole.embed.committed", app = container.unwrap_or_default());
                true
            }
            Err(e) => {
                self.status = t!("app.status.save_failed", error = e);
                false
            }
        }
    }

    /// Before the 3D view is drawn: an embedded 3D model in the active tab is shown from its own
    /// camera when the viewer loads it, any other model framed.
    pub(super) fn prefer_embedded_camera(&mut self) {
        let camera = self
            .docs
            .get(self.active)
            .and_then(|d| d.embedded.as_ref())
            .and_then(|e| e.model3d.as_ref())
            .and_then(|e| e.look.camera);
        self.view3d.prefer_camera(camera);
    }

    /// After the 3D view is drawn: the active embedded 3D model's look follows the view (its
    /// style, camera and size), so writing it back draws what was on screen.
    pub(super) fn track_embedded_look(&mut self) {
        if !self.view.is_3d() {
            return;
        }
        let style = self.view3d.style;
        let Some((camera, points)) = self.view3d.camera() else {
            return;
        };
        let Some(doc) = self.docs.get_mut(self.active) else {
            return;
        };
        if doc.three_d.shown_model().is_none() {
            return;
        }
        if let Some(embedded) = doc.embedded_3d_mut() {
            embedded.look.style = style;
            embedded.look.camera = Some(camera);
            embedded.look.view_points = [points.x, points.y];
        }
    }

    /// Close the embedded document at `index`: unsaved changes go into the container first.
    pub(super) fn close_embedded(&mut self, index: usize) {
        let Some(id) = self.docs[index].embedded.as_ref().map(|e| e.id) else {
            return;
        };
        if self.docs[index].is_dirty() && !self.save_embedded(index) {
            return;
        }
        if let Some(bridge) = &self.ole {
            bridge.send(ToServer::Closed { id });
        }
        self.remove_tab(index);
    }

    /// Close every embedded document (the window is closing).
    pub(super) fn close_all_embedded(&mut self) {
        while let Some(i) = self.docs.iter().position(|d| d.embedded.is_some()) {
            let before = self.docs.len();
            self.close_embedded(i);
            if self.docs.len() == before {
                // Not saved (an encoding error): drop it rather than loop.
                self.remove_tab(i);
            }
        }
    }

    /// A COM-launched process ends once its embedded objects are closed, unless the user has
    /// started other work in the window.
    fn quit_when_served(&mut self, ctx: &egui::Context) {
        let Some(bridge) = &self.ole else { return };
        if !bridge.launched_by_com || !(bridge.seen_object || bridge.idle) {
            return;
        }
        let only_pristine = self
            .docs
            .iter()
            .all(|d| d.embedded.is_none() && d.is_pristine());
        if only_pristine && !self.quit_confirmed {
            crate::ole_server::log("app: no embedded object left; quitting");
            self.quit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Apply the test script (`--ole-test-script`) to the first embedded document.
    fn run_ole_script(&mut self) {
        let Some(i) = self.docs.iter().position(|d| d.embedded.is_some()) else {
            return;
        };
        let Some(script) = self.ole.as_mut().and_then(|b| b.script.take()) else {
            return;
        };
        let rest = script.apply(&mut self.docs[i].editor.molecule);
        for step in rest {
            match step {
                Step::Save => {
                    self.save_embedded(i);
                }
                Step::Close => {
                    self.close_embedded(i);
                    return;
                }
                Step::AddAtom { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ole_server::{self, ServerSide, script::Script};

    fn served_app() -> (OpenDrawApp, ServerSide) {
        let (bridge, side) = ole_server::channel(Box::new(|| {}), true);
        let mut app = OpenDrawApp::default();
        app.new_document();
        let ctx = egui::Context::default();
        (app.with_ole(bridge, &ctx), side)
    }

    fn ethanol_object() -> Object3d {
        let model = crate::ole_server::model3d::tests::ethanol();
        let look = crate::ole_server::model3d::Look3d {
            style: crate::ui::viewport3d::Style3d::Wireframe,
            ..Default::default()
        };
        Object3d::new(&model, look, None).unwrap()
    }

    #[test]
    fn an_embedded_3d_model_opens_in_the_3d_view_as_it_was_shown() {
        let (mut app, side) = served_app();
        side.send(ToApp::Open3d {
            id: 11,
            object: Some(Box::new(ethanol_object())),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        assert_eq!(app.docs.len(), 1);
        assert_eq!(app.view, ViewMode::ThreeD);
        assert_eq!(app.view3d.style, crate::ui::viewport3d::Style3d::Wireframe);
        let doc = &app.docs[0];
        assert_eq!(doc.three_d.model().unwrap().atoms.len(), 9);
        assert_eq!(doc.title(), "Embedded in Word — 文書 1");
        assert!(!doc.is_dirty());
    }

    #[test]
    fn saving_an_embedded_3d_model_sends_its_mol2_and_look() {
        let (mut app, side) = served_app();
        side.send(ToApp::Open3d {
            id: 12,
            object: Some(Box::new(ethanol_object())),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        app.view3d.style = crate::ui::viewport3d::Style3d::Spheres;
        app.docs[0].embedded_3d_mut().unwrap().look.style = app.view3d.style;
        assert!(app.docs[0].is_dirty(), "a new style is an edit");
        assert!(app.save_doc(0));
        let Ok(ToServer::Update3d { id: 12, object }) = side.rx.try_recv() else {
            panic!("no 3D update");
        };
        assert_eq!(object.look.style, crate::ui::viewport3d::Style3d::Spheres);
        let back = object.open().unwrap().model.unwrap();
        assert_eq!(back.atoms.len(), 9);
        // No GPU in the tests: no picture, so the object keeps its own.
        assert!(object.png.is_empty());
        assert!(!app.docs[0].is_dirty());

        // The container closing asks for unsaved changes: there are none now.
        let (tx, rx) = std::sync::mpsc::channel();
        side.send(ToApp::Close {
            id: 12,
            reply: Some(tx),
        });
        app.poll_ole(&ctx);
        assert_eq!(rx.try_recv().unwrap(), None);
        assert!(app.docs.is_empty());
    }

    #[test]
    fn closing_an_edited_3d_tab_writes_the_model_first() {
        let (mut app, side) = served_app();
        side.send(ToApp::Open3d {
            id: 13,
            object: Some(Box::new(ethanol_object())),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        let model = app.docs[0].three_d.model_mut().unwrap();
        let mut coords = crate::three_d::edit::coords(model);
        coords[2][1] += 0.25;
        crate::three_d::edit::set_coords(model, &coords);
        app.close_tab(0);
        let Ok(ToServer::Update3d { id: 13, object }) = side.rx.try_recv() else {
            panic!("no 3D update");
        };
        let back = object.open().unwrap().model.unwrap();
        assert!((back.atoms[2].pos[1] - coords[2][1]).abs() < 1e-3);
        assert_eq!(side.rx.try_recv(), Ok(ToServer::Closed { id: 13 }));
        assert!(app.docs.is_empty());
    }

    #[test]
    fn a_new_3d_object_opens_empty_in_the_3d_view() {
        let (mut app, side) = served_app();
        side.send(ToApp::Open3d {
            id: 14,
            object: None,
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        assert_eq!(app.view, ViewMode::ThreeD);
        assert!(app.docs[0].three_d.model().is_none());
        assert!(app.docs[0].embedded.as_ref().unwrap().model3d.is_some());
    }

    fn water_cdx() -> Vec<u8> {
        let mut mol = chembuider_rs::Molecule::default();
        mol.add_atom("O".to_owned(), [0.0, 0.0], 0);
        io::encode(Format::Cdx, &mol, None).unwrap()
    }

    fn word() -> HostNames {
        HostNames {
            app: "Microsoft Word".into(),
            document: "文書 1".into(),
        }
    }

    #[test]
    fn an_embedded_object_opens_as_a_marked_tab_in_place_of_the_empty_one() {
        let (mut app, side) = served_app();
        assert!(app.docs.is_empty(), "no empty drawing in a server window");
        side.send(ToApp::Open {
            id: 1,
            cdx: Some(water_cdx()),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        assert_eq!(app.docs.len(), 1);
        assert_eq!(app.docs[0].title(), "Embedded in Word — 文書 1");
        assert_eq!(app.docs[0].editor.molecule.atoms.len(), 1);

        // Opening it again only brings it to the front.
        side.send(ToApp::Open {
            id: 1,
            cdx: None,
            host: word(),
        });
        app.poll_ole(&ctx);
        assert_eq!(app.docs.len(), 1);
    }

    #[test]
    fn saving_sends_the_merged_cdx_and_closing_ends_the_session() {
        let (mut app, side) = served_app();
        side.send(ToApp::Open {
            id: 7,
            cdx: Some(water_cdx()),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        app.docs[0]
            .editor
            .molecule
            .add_atom("N".to_owned(), [2.0, 0.0], 0);
        assert!(app.save_doc(0), "saving an embedded document needs no file");
        let Ok(ToServer::Update { id: 7, cdx }) = side.rx.try_recv() else {
            panic!("no update");
        };
        assert_eq!(
            io::decode(Format::Cdx, &cdx).unwrap().molecule.atoms.len(),
            2
        );
        assert!(!app.docs[0].is_dirty());

        // Closing an edited tab writes the edit first, then ends the session.
        app.docs[0]
            .editor
            .molecule
            .add_atom("S".to_owned(), [4.0, 0.0], 0);
        app.close_tab(0);
        assert!(matches!(
            side.rx.try_recv(),
            Ok(ToServer::Update { id: 7, .. })
        ));
        assert_eq!(side.rx.try_recv(), Ok(ToServer::Closed { id: 7 }));
        assert!(app.docs.is_empty());
        assert!(app.pending_close.is_none(), "no unsaved-changes question");
    }

    #[test]
    fn the_container_closing_takes_unsaved_changes_with_it() {
        let (mut app, side) = served_app();
        side.send(ToApp::Open {
            id: 2,
            cdx: Some(water_cdx()),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        app.docs[0]
            .editor
            .molecule
            .add_atom("N".to_owned(), [2.0, 0.0], 0);
        let (tx, rx) = std::sync::mpsc::channel();
        side.send(ToApp::Close {
            id: 2,
            reply: Some(tx),
        });
        app.poll_ole(&ctx);
        let Some(Native::Cdx(cdx)) = rx.try_recv().unwrap() else {
            panic!("no CDX");
        };
        assert_eq!(
            io::decode(Format::Cdx, &cdx).unwrap().molecule.atoms.len(),
            2
        );
        assert!(app.docs.is_empty());
        assert!(
            app.quit_confirmed,
            "a COM-launched server quits with its last object"
        );
    }

    #[test]
    fn live_updates_write_edits_back_without_a_save() {
        let (mut app, side) = served_app();
        app.ole.as_mut().unwrap().live_updates = Some(std::time::Duration::ZERO);
        side.send(ToApp::Open {
            id: 4,
            cdx: Some(water_cdx()),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        assert!(side.rx.try_recv().is_err(), "nothing was edited");
        app.docs[0]
            .editor
            .molecule
            .add_atom("N".to_owned(), [2.0, 0.0], 0);
        app.poll_ole(&ctx);
        let Ok(ToServer::Update { id: 4, .. }) = side.rx.try_recv() else {
            panic!("no update")
        };
        assert!(!app.docs[0].is_dirty());
        app.poll_ole(&ctx);
        assert!(side.rx.try_recv().is_err(), "sent once");
    }

    #[test]
    fn live_updates_wait_a_moment_and_are_off_under_com() {
        let (mut app, side) = served_app();
        side.send(ToApp::Open {
            id: 5,
            cdx: Some(water_cdx()),
            host: word(),
        });
        let ctx = egui::Context::default();
        app.poll_ole(&ctx);
        app.docs[0]
            .editor
            .molecule
            .add_atom("N".to_owned(), [2.0, 0.0], 0);
        app.poll_ole(&ctx);
        assert!(side.rx.try_recv().is_err(), "COM writes on a save");
        app.ole.as_mut().unwrap().live_updates = Some(std::time::Duration::from_secs(60));
        app.poll_ole(&ctx);
        assert!(side.rx.try_recv().is_err(), "too soon");
        assert!(app.docs[0].is_dirty());
    }

    #[test]
    fn a_script_edits_saves_and_closes() {
        let (mut app, side) = served_app();
        app.ole.as_mut().unwrap().script =
            Some(Script::parse("add_atom N 2 0\nsave\nclose").unwrap());
        side.send(ToApp::Open {
            id: 3,
            cdx: Some(water_cdx()),
            host: word(),
        });
        app.poll_ole(&egui::Context::default());
        let Ok(ToServer::Update { cdx, .. }) = side.rx.try_recv() else {
            panic!("no update");
        };
        assert_eq!(
            io::decode(Format::Cdx, &cdx).unwrap().molecule.atoms.len(),
            2
        );
        assert_eq!(side.rx.try_recv(), Ok(ToServer::Closed { id: 3 }));
        assert!(app.quit_confirmed);
    }
}
