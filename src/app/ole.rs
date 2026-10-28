//! The app's side of the OLE server (see [`crate::ole_server`]): embedded objects open as
//! tabs; saving such a tab writes into the container's document, closing it ends the editing
//! session, and a COM-launched process quits once no embedded object is left.

use super::OpenDrawApp;
use crate::document::{Document, Embedded};
use crate::io::{self, Format};
use crate::ole_server::script::Step;
use crate::ole_server::{AppBridge, HostNames, ObjectId, ToApp, ToServer};

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
                ToApp::Close { id, reply } => {
                    if let Some(i) = self.embedded_index(id) {
                        if let Some(reply) = reply {
                            let doc = &mut self.docs[i];
                            let cdx = if doc.is_dirty() {
                                doc.commit_embedded().ok()
                            } else {
                                None
                            };
                            let _ = reply.send(cdx);
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
        self.quit_when_served(ctx);
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

    /// Write the embedded document at `index` back into its container. Returns whether it was.
    pub(super) fn save_embedded(&mut self, index: usize) -> bool {
        let doc = &mut self.docs[index];
        let Some(id) = doc.embedded.as_ref().map(|e| e.id) else {
            return false;
        };
        match doc.commit_embedded() {
            Ok(cdx) => {
                let container = doc.embedded.as_ref().map(|e| e.container.clone());
                if let Some(bridge) = &self.ole {
                    bridge.send(ToServer::Update { id, cdx });
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
        let cdx = rx.try_recv().unwrap().expect("the edit");
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
