//! The OLE server: editing ChemDraw objects embedded in Word/PowerPoint documents.
//!
//! On Windows the executable is also a COM local server ([`com`], registered by
//! [`register`]). A container (Word, PowerPoint, …) starts it with `-Embedding` when the user
//! double-clicks an embedded object; the object's CDX opens here in a tab marked as embedded,
//! and saving or closing that tab writes the structure back into the container's document.
//!
//! The objects stay ChemDraw objects: the storage keeps ChemDraw's class id and a
//! ChemDraw-compatible `\x01CompObj` stream, and the presentation (the picture the container
//! shows) is refreshed on every save. On a PC without ChemDraw, `--register-ole` points
//! ChemDraw's class at this server (`TreatAs`); where ChemDraw is installed, nothing of
//! ChemDraw's is touched and only this app's own class ([`CLSID`]) is registered.
//!
//! The COM objects live on their own single-threaded apartment thread ([`com`]); the egui app
//! runs on the main thread. They talk through the channels in [`AppBridge`] / [`ServerSide`]
//! (this module, platform independent so the app's side is unit-testable anywhere).

#[cfg(windows)]
pub mod com;
pub mod compobj;
#[cfg(windows)]
pub mod register;
pub mod script;

use std::io::Write as _;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

/// The server's log file (`--ole-log <file>`): a COM-launched process has no console, so this
/// is where to see what a container asked for.
static LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

/// Log to `path` from now on (appending).
pub fn open_log(path: &std::path::Path) {
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = LOG.set(Mutex::new(file));
    }
}

/// Note `message` in the log (and on stderr).
pub fn log(message: &str) {
    eprintln!("OLE: {message}");
    if let Some(file) = LOG.get() {
        let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() % 100_000_000);
        let _ = writeln!(
            file,
            "{ms:>9} [{}] {message}",
            std::thread::current().name().unwrap_or("?")
        );
    }
}

/// This app's own OLE class: `{E14F99CE-3139-403D-8F25-1E80D9DA8AE2}`. Used for testing on a PC
/// with ChemDraw, and as the `TreatAs` target of ChemDraw's class on a PC without it.
pub const CLSID: u128 = 0xE14F99CE_3139_403D_8F25_1E80D9DA8AE2;
/// ChemDraw's OLE class, `{41BA6D21-A02E-11CE-8FD9-0020AFD1F20C}`.
pub const CHEMDRAW_CLSID: u128 = 0x41BA6D21_A02E_11CE_8FD9_0020AFD1F20C;
/// The versioned ProgID of [`CLSID`].
pub const PROG_ID: &str = "OpenDrawChemistry.Document.1";
/// The version-independent ProgID of [`CLSID`].
pub const PROG_ID_CURRENT: &str = "OpenDrawChemistry.Document";
/// ChemDraw's ProgIDs (32- and 64-bit ChemDraw); either registered means ChemDraw is installed.
pub const CHEMDRAW_PROG_IDS: [&str; 2] = ["ChemDraw.Document.6.0", "ChemDraw_x64.Document.6.0"];
/// The user type of [`CLSID`] (what Insert Object lists).
pub const USER_TYPE: &str = "Open DrawChemistry 構造式";
/// The short user type (the object's verbs menu).
pub const USER_TYPE_SHORT: &str = "構造式";
/// The clipboard format the CONTENTS stream holds, as ChemDraw names it.
pub const CDX_FORMAT_NAME: &str = "ChemDraw Interchange Format";

/// A GUID constant in registry form, `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
pub fn guid_string(guid: u128) -> String {
    let h = format!("{guid:032X}");
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Whether the command line is COM starting this executable as a local server.
pub fn is_embedding_launch<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> bool {
    args.iter().any(|a| {
        let a = a.as_ref().to_string_lossy();
        a.eq_ignore_ascii_case("-Embedding") || a.eq_ignore_ascii_case("/Embedding")
    })
}

/// The id of an embedded object within this server process.
pub type ObjectId = u64;

/// The container's names for an object, from `IOleObject::SetHostNames`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostNames {
    /// The containing application ("Microsoft Word").
    pub app: String,
    /// The containing document ("文書 1").
    pub document: String,
}

impl HostNames {
    /// The application name for a tab title: "Microsoft Word" → "Word".
    pub fn short_app(&self) -> String {
        let app = self.app.trim();
        let app = app.strip_prefix("Microsoft ").unwrap_or(app);
        if app.is_empty() {
            "Office".to_owned()
        } else {
            app.to_owned()
        }
    }
}

/// From the COM server to the app.
#[derive(Debug)]
pub enum ToApp {
    /// Show the object for editing (a tab is opened, or brought to the front when it is open).
    Open {
        id: ObjectId,
        /// The object's CONTENTS (CDX); `None` for a newly inserted object.
        cdx: Option<Vec<u8>>,
        host: HostNames,
    },
    /// The container closes the object: remove its tab. When `reply` is set the container
    /// wants unsaved changes: answer with the CDX when there are any, `None` otherwise.
    Close {
        id: ObjectId,
        reply: Option<Sender<Option<Vec<u8>>>>,
    },
    /// No embedded object is left in the server: a COM-launched process can end.
    Idle,
}

impl ToApp {
    /// One line for the log (without the CDX bytes).
    pub fn summary(&self) -> String {
        match self {
            ToApp::Open { id, cdx, host } => format!(
                "open {id} ({} bytes) from {:?} / {:?}",
                cdx.as_ref().map_or(0, Vec::len),
                host.app,
                host.document
            ),
            ToApp::Close { id, reply } => format!("close {id} (save: {})", reply.is_some()),
            ToApp::Idle => "idle".to_owned(),
        }
    }
}

/// From the app to the COM server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToServer {
    /// The user saved: write this CDX into the object and tell the container.
    Update { id: ObjectId, cdx: Vec<u8> },
    /// The object's tab was closed: the editing session is over.
    Closed { id: ObjectId },
    /// The app is exiting.
    Shutdown,
}

/// Sends to the COM server and wakes it.
#[derive(Clone)]
pub struct Notifier {
    tx: Sender<ToServer>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Notifier {
    pub fn send(&self, message: ToServer) {
        // A server that has gone has nothing left to update.
        if self.tx.send(message).is_ok() {
            (self.wake)();
        }
    }
}

/// The app's end of the channels.
pub struct AppBridge {
    rx: Receiver<ToApp>,
    notify: Notifier,
    repaint: Arc<Mutex<Option<egui::Context>>>,
    /// The process was started by COM (`-Embedding`): it ends once no embedded object is left.
    pub launched_by_com: bool,
    /// A scripted edit to apply to the first embedded document (testing; see [`script`]).
    pub script: Option<script::Script>,
    /// An embedded document has been opened in this session.
    pub seen_object: bool,
    /// The server has no object left.
    pub idle: bool,
}

impl AppBridge {
    /// Let the server wake the app's event loop when it sends something.
    pub fn attach(&self, ctx: &egui::Context) {
        *self.repaint.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx.clone());
    }

    pub fn send(&self, message: ToServer) {
        self.notify.send(message);
    }

    /// A sender to the server that outlives the bridge (for shutting it down).
    pub fn notifier(&self) -> Notifier {
        self.notify.clone()
    }

    pub fn try_recv(&self) -> Option<ToApp> {
        self.rx.try_recv().ok()
    }
}

/// The COM server's end of the channels.
pub struct ServerSide {
    tx: Sender<ToApp>,
    pub rx: Receiver<ToServer>,
    repaint: Arc<Mutex<Option<egui::Context>>>,
}

impl ServerSide {
    pub fn send(&self, message: ToApp) {
        if self.tx.send(message).is_ok()
            && let Some(ctx) = &*self.repaint.lock().unwrap_or_else(|e| e.into_inner())
        {
            ctx.request_repaint();
        }
    }
}

/// A connected pair; `wake` is called after each message to the server.
pub fn channel(
    wake: Box<dyn Fn() + Send + Sync>,
    launched_by_com: bool,
) -> (AppBridge, ServerSide) {
    let (to_app, app_rx) = mpsc::channel();
    let (to_server, server_rx) = mpsc::channel();
    let repaint = Arc::new(Mutex::new(None));
    (
        AppBridge {
            rx: app_rx,
            notify: Notifier {
                tx: to_server,
                wake: Arc::from(wake),
            },
            repaint: Arc::clone(&repaint),
            launched_by_com,
            script: None,
            seen_object: false,
            idle: false,
        },
        ServerSide {
            tx: to_app,
            rx: server_rx,
            repaint,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_print_in_registry_form() {
        assert_eq!(
            guid_string(CHEMDRAW_CLSID),
            "{41BA6D21-A02E-11CE-8FD9-0020AFD1F20C}"
        );
        assert_eq!(guid_string(CLSID), "{E14F99CE-3139-403D-8F25-1E80D9DA8AE2}");
    }

    #[test]
    fn embedding_switch_in_either_form() {
        assert!(is_embedding_launch(&["-Embedding"]));
        assert!(is_embedding_launch(&["/embedding"]));
        assert!(!is_embedding_launch(&["a.cdx"]));
    }

    #[test]
    fn host_app_is_shortened() {
        let host = HostNames {
            app: "Microsoft Word".into(),
            document: "文書 1".into(),
        };
        assert_eq!(host.short_app(), "Word");
        assert_eq!(HostNames::default().short_app(), "Office");
    }

    #[test]
    fn messages_wake_the_other_side() {
        let woken = Arc::new(Mutex::new(0));
        let w = Arc::clone(&woken);
        let (app, server) = channel(Box::new(move || *w.lock().unwrap() += 1), true);
        app.send(ToServer::Closed { id: 3 });
        assert_eq!(*woken.lock().unwrap(), 1);
        assert_eq!(server.rx.try_recv().unwrap(), ToServer::Closed { id: 3 });
        server.send(ToApp::Idle);
        assert!(matches!(app.try_recv(), Some(ToApp::Idle)));
    }
}
