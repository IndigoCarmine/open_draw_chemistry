//! The COM local server: a class factory and the embedded object (`IOleObject`,
//! `IDataObject`, `IPersistStorage`, `IRunnableObject`), for out-of-place editing.
//!
//! # Threads
//!
//! The objects live on one thread of their own, a single-threaded apartment with its own
//! message loop ([`Server::start`]). Containers call into them there; the egui app on the main
//! thread is told what to show through [`ToApp`] messages (which wake its event loop), and
//! answers with [`ToServer`] messages (which set an event the server's loop waits on). The
//! server thread makes every call back to the container (`IOleClientSite::SaveObject`, the
//! advise sinks), since those proxies belong to its apartment.
//!
//! The class is registered `REGCLS_SINGLEUSE`: each object the containers create gets its own
//! process (and window), so a process's life is one editing session.
//!
//! # Storage
//!
//! The object's storage holds `CONTENTS` (the CDX), `\x01CompObj` and the class id. The class
//! is the one the object was loaded with (ChemDraw's, for ChemDraw objects activated through
//! `TreatAs`), so the object stays what it was; a new object is stored as ChemDraw's class
//! (unless the server runs with `--ole-own-class`, for testing beside an installed ChemDraw).
//!
//! # 3D models
//!
//! The server also serves this app's 3D class ([`CLSID_3D`]; a second class factory, so one
//! process serves either): the same object, holding a [`model3d::Object3d`] instead of a CDX.
//! Its storage has the MOL2 in `CONTENTS` and the look and the picture in streams of their
//! own (see [`model3d`]); its presentation is the picture with a transparent background, as an
//! EMF (see [`super::picture`]). The app draws the picture when the user saves, and sends it
//! with the model; the server never draws a 3D model itself.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chembuider_rs::Molecule;
use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, COLORREF, DATA_S_SAMEFORMATETC, DV_E_DVASPECT, DV_E_FORMATETC,
    DV_E_TYMED, E_FAIL, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, HANDLE, HGLOBAL, HWND,
    OLE_E_NOCONNECTION, RECT, S_FALSE, S_OK, SIZE,
};
use windows::Win32::Graphics::Gdi::{
    CloseEnhMetaFile, CreateEnhMetaFileW, CreatePen, DeleteEnhMetaFile, DeleteObject,
    ENHMETAHEADER, GetDC, GetDeviceCaps, GetEnhMetaFileHeader, GetStockObject, GetWinMetaFileBits,
    HENHMETAFILE, LOGPALETTE, LOGPIXELSX, LOGPIXELSY, MM_ANISOTROPIC, NULL_BRUSH, PS_SOLID,
    Rectangle, ReleaseDC, SelectObject, SetMetaFileBitsEx,
};
use windows::Win32::Media::Audio::{CoRegisterMessageFilter, IMessageFilter, IMessageFilter_Impl};
use windows::Win32::Media::HTASK;
use windows::Win32::System::Com::StructuredStorage::{
    CreateILockBytesOnHGlobal, GetHGlobalFromILockBytes, IPersistStorage, IPersistStorage_Impl,
    IStorage, ReadClassStg, StgCreateDocfileOnILockBytes, WriteClassStg,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoDisconnectObject, CoRegisterClassObject, CoRevokeClassObject,
    CoTaskMemAlloc, CreateDataAdviseHolder, DATADIR_GET, DVASPECT, DVASPECT_CONTENT,
    DVASPECT_DOCPRINT, FORMATETC, IAdviseSink, IBindCtx, IClassFactory, IClassFactory_Impl,
    IDataAdviseHolder, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, IMoniker,
    INTERFACEINFO, IPersist_Impl, IRunnableObject, IRunnableObject_Impl, PENDINGMSG_WAITDEFPROCESS,
    REGCLS_SINGLEUSE, SERVERCALL_ISHANDLED, STGM_CREATE, STGM_READ, STGM_READWRITE,
    STGM_SHARE_EXCLUSIVE, STGM_WRITE, STGMEDIUM, STGMEDIUM_0, TYMED_ENHMF, TYMED_HGLOBAL,
    TYMED_ISTORAGE, TYMED_MFPICT,
};
use windows::Win32::System::DataExchange::{METAFILEPICT, RegisterClipboardFormatW};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_ENHMETAFILE, CF_METAFILEPICT, CreateOleAdviseHolder, IEnumOLEVERB, IOleAdviseHolder,
    IOleClientSite, IOleObject, IOleObject_Impl, OLECLOSE, OLECLOSE_NOSAVE, OLECLOSE_SAVEIFDIRTY,
    OLEGETMONIKER, OLEIVERB_DISCARDUNDOSTATE, OLEIVERB_HIDE, OLEMISC, OLEMISC_CANTLINKINSIDE,
    OLEWHICHMK, OleInitialize, OleRegEnumVerbs, OleUninitialize, USERCLASSTYPE,
    USERCLASSTYPE_APPNAME, USERCLASSTYPE_SHORT,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage,
};
use windows::core::{
    BOOL, ComObject, GUID, HRESULT, HSTRING, IUnknown, IUnknownImpl, Interface, PCWSTR, PWSTR,
    Param, Ref, Result, implement, w,
};

use super::model3d::{self, Object3d};
use super::{
    AppBridge, CHEMDRAW_CLSID, CLSID, CLSID_3D, HostNames, Kind, Native, ObjectId, ServerSide,
    ToApp, ToServer, compobj, picture as picture3d,
};
use crate::io::{self, Format};

/// How long the server waits for a first object before giving up (COM started it for nothing).
const FIRST_OBJECT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a container's close waits for the app to hand over unsaved changes.
const CLOSE_REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a call the container rejects as busy is retried.
const RETRY_BUSY_FOR_MS: u32 = 30_000;

/// What the main thread waits for before it opens the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainEvent {
    /// An object is to be shown: run the app.
    Shown,
    /// No object is left (or none came): the process can end.
    Idle,
}

/// The running server, as the main thread holds it.
pub struct Server {
    notify: super::Notifier,
    main_rx: Receiver<MainEvent>,
    thread: Option<JoinHandle<()>>,
    event: SendHandle,
}

/// The server thread's wake-up event, shared with the app's side.
#[derive(Clone, Copy)]
struct SendHandle(HANDLE);
// SAFETY: an event handle may be used from any thread.
unsafe impl Send for SendHandle {}
unsafe impl Sync for SendHandle {}

impl SendHandle {
    fn set(&self) {
        // SAFETY: the event outlives every sender (it is closed after the thread ends).
        unsafe {
            let _ = SetEvent(self.0);
        }
    }
}

impl Server {
    /// Start the server thread and register the class factory. `own_class` stores new objects
    /// under this app's class instead of ChemDraw's.
    pub fn start(own_class: bool) -> std::result::Result<(Server, AppBridge), String> {
        // SAFETY: an unnamed auto-reset event.
        let event = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|e| e.to_string())?;
        let event = SendHandle(event);
        let wake = Box::new(move || event.set());
        let (bridge, side) = super::channel(wake, true);
        let notify = bridge.notifier();
        let (main_tx, main_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("ole-server".into())
            .spawn(move || run(side, main_tx, event, own_class, ready_tx))
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok((
                Server {
                    notify,
                    main_rx,
                    thread: Some(thread),
                    event,
                },
                bridge,
            )),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("OLE サーバーのスレッドが終了しました".to_owned()), // i18n-allow: log output
        }
    }

    /// Wait until a container shows an object (`true`) or the server has nothing to do.
    pub fn wait_until_shown(&self) -> bool {
        matches!(self.main_rx.recv(), Ok(MainEvent::Shown))
    }

    /// Finish what the app sent, close the remaining sessions and stop the server thread.
    pub fn shutdown(mut self) {
        self.notify.send(ToServer::Shutdown);
        if let Some(thread) = self.thread.take() {
            // A container that hangs must not keep the process: wait a while, then leave.
            let deadline = Instant::now() + Duration::from_secs(15);
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if thread.is_finished() {
                let _ = thread.join();
                // SAFETY: nothing uses the event once the thread has ended and the app is gone.
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(self.event.0);
                }
            }
        }
    }
}

// ─── Server thread state ────────────────────────────────────────────────────────────────────

struct ServerState {
    side: ServerSide,
    to_main: Sender<MainEvent>,
    /// The live objects. Removed by the object's `Drop`, so the pointers stay valid.
    objects: HashMap<ObjectId, *const EmbeddedObject_Impl>,
    next_id: ObjectId,
    locks: u32,
    created: bool,
    idle_sent: bool,
    own_class: bool,
}

impl ServerState {
    fn check_idle(&mut self) {
        if self.created && self.objects.is_empty() && self.locks == 0 && !self.idle_sent {
            self.idle_sent = true;
            self.side.send(ToApp::Idle);
            let _ = self.to_main.send(MainEvent::Idle);
        }
    }
}

thread_local! {
    static SERVER: RefCell<Option<ServerState>> = const { RefCell::new(None) };
}

/// Run `f` on the server state. Never call out of the apartment inside `f`: a call back in
/// would find the state borrowed.
fn with_server<R>(f: impl FnOnce(&mut ServerState) -> R) -> Option<R> {
    SERVER.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Run `f` on the live object `id`, kept alive meanwhile.
fn with_object(id: ObjectId, f: impl FnOnce(&EmbeddedObject_Impl)) {
    let Some(ptr) = with_server(|s| s.objects.get(&id).copied()).flatten() else {
        return;
    };
    // SAFETY: the pointer is removed from the map before the object is freed, and everything
    // here runs on the server thread, so it is live; the reference taken keeps it so during f.
    let this = unsafe { &*ptr };
    let _keep: IUnknown = this.to_interface();
    f(this);
}

fn run(
    side: ServerSide,
    to_main: Sender<MainEvent>,
    event: SendHandle,
    own_class: bool,
    ready: mpsc::SyncSender<std::result::Result<(), String>>,
) {
    // SAFETY: called once on this new thread, paired with OleUninitialize below.
    if let Err(e) = unsafe { OleInitialize(None) } {
        let _ = ready.send(Err(e.to_string()));
        return;
    }
    SERVER.with(|s| {
        *s.borrow_mut() = Some(ServerState {
            side,
            to_main,
            objects: HashMap::new(),
            next_id: 0,
            locks: 0,
            created: false,
            idle_sent: false,
            own_class,
        })
    });
    let filter: IMessageFilter = BusyRetry.into();
    // SAFETY: registered on this STA thread; the previous filter is not needed.
    unsafe {
        let _ = CoRegisterMessageFilter(&filter, None);
    }
    // One factory a class: COM starts the same command for both, and either may be asked for.
    let mut cookies = Vec::new();
    for kind in [Kind::Structure, Kind::Model3d] {
        let factory: IClassFactory = Factory { kind }.into();
        // SAFETY: a live class factory, registered for its class on this apartment.
        match unsafe {
            CoRegisterClassObject(
                &GUID::from_u128(kind.class()),
                &factory,
                CLSCTX_LOCAL_SERVER,
                REGCLS_SINGLEUSE,
            )
        } {
            Ok(cookie) => cookies.push(cookie),
            Err(e) => {
                let _ = ready.send(Err(e.to_string()));
                for cookie in cookies {
                    // SAFETY: revoking what was registered just above.
                    unsafe {
                        let _ = CoRevokeClassObject(cookie);
                    }
                }
                SERVER.with(|s| s.borrow_mut().take());
                // SAFETY: pairs with OleInitialize.
                unsafe { OleUninitialize() };
                return;
            }
        }
    }
    let _ = ready.send(Ok(()));

    let started = Instant::now();
    'outer: loop {
        // SAFETY: waiting on our own event; messages are pumped below.
        unsafe {
            MsgWaitForMultipleObjectsEx(Some(&[event.0]), 500, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Some(message) = with_server(|s| s.side.rx.try_recv().ok()).flatten() {
            match message {
                ToServer::Update { id, cdx } => with_object(id, |this| {
                    super::log(&format!("update {id}: {} bytes", cdx.len()));
                    this.set_contents(cdx);
                    this.notify_changed();
                }),
                ToServer::Update3d { id, object } => with_object(id, |this| {
                    super::log(&format!(
                        "update 3D {id}: {} bytes of MOL2, {} of picture",
                        object.mol2.len(),
                        object.png.len()
                    ));
                    this.set_model(*object);
                    this.notify_changed();
                }),
                ToServer::Closed { id } => with_object(id, |this| {
                    super::log(&format!("closed {id}"));
                    this.end_session();
                }),
                ToServer::Shutdown => break 'outer,
            }
        }
        with_server(|s| {
            if !s.created && started.elapsed() > FIRST_OBJECT_TIMEOUT && !s.idle_sent {
                s.idle_sent = true;
                let _ = s.to_main.send(MainEvent::Idle);
                s.side.send(ToApp::Idle);
            }
        });
    }

    // SAFETY: revoking our own registrations; then cutting off containers still holding
    // objects (they keep the cached picture), on this thread.
    for cookie in cookies {
        unsafe {
            let _ = CoRevokeClassObject(cookie);
        }
    }
    let ids = with_server(|s| s.objects.keys().copied().collect::<Vec<_>>()).unwrap_or_default();
    for id in ids {
        with_object(id, |this| {
            if this.state.borrow().open {
                this.end_session();
            }
            let me: IUnknown = this.to_interface();
            // SAFETY: disconnecting our own object.
            unsafe {
                let _ = CoDisconnectObject(&me, None);
            }
        });
    }
    // SAFETY: removing our filter from this thread.
    unsafe {
        let _ = CoRegisterMessageFilter(None, None);
    }
    SERVER.with(|s| s.borrow_mut().take());
    // SAFETY: pairs with OleInitialize.
    unsafe { OleUninitialize() };
}

// ─── Busy containers ────────────────────────────────────────────────────────────────────────

/// Retries calls a container rejects while it is busy (Word answers SERVERCALL_RETRYLATER
/// while it is itself in a call), instead of failing them at once.
#[implement(IMessageFilter, Agile = false)]
struct BusyRetry;

impl IMessageFilter_Impl for BusyRetry_Impl {
    fn HandleInComingCall(
        &self,
        _calltype: u32,
        _caller: HTASK,
        _tick: u32,
        _info: *const INTERFACEINFO,
    ) -> u32 {
        SERVERCALL_ISHANDLED.0 as u32
    }

    fn RetryRejectedCall(&self, _callee: HTASK, tick: u32, _reject: u32) -> u32 {
        if tick < RETRY_BUSY_FOR_MS {
            100
        } else {
            u32::MAX
        }
    }

    fn MessagePending(&self, _callee: HTASK, _tick: u32, _pending: u32) -> u32 {
        PENDINGMSG_WAITDEFPROCESS.0 as u32
    }
}

// ─── Class factory ──────────────────────────────────────────────────────────────────────────

/// Makes the objects of one class.
#[implement(IClassFactory, Agile = false)]
struct Factory {
    kind: Kind,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        super::log(&format!("IClassFactory::CreateInstance ({:?})", self.kind));
        if ppv.is_null() || riid.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: checked non-null; COM passes a valid out-pointer.
        unsafe { *ppv = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let (id, own_class) = with_server(|s| {
            s.next_id += 1;
            s.created = true;
            (s.next_id, s.own_class)
        })
        .ok_or(E_FAIL)?;
        let obj = ComObject::new(EmbeddedObject {
            id,
            state: RefCell::new(ObjState::new(own_class, self.kind)),
        });
        let ptr: *const EmbeddedObject_Impl = &*obj;
        with_server(|s| s.objects.insert(id, ptr));
        let unknown: IUnknown = obj.to_interface();
        // SAFETY: riid and ppv as checked above.
        unsafe { unknown.query(riid, ppv).ok() }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        super::log("IClassFactory::LockServer");
        with_server(|s| {
            if lock.as_bool() {
                s.locks += 1;
            } else {
                s.locks = s.locks.saturating_sub(1);
                s.check_idle();
            }
        });
        Ok(())
    }
}

// ─── The embedded object ────────────────────────────────────────────────────────────────────

struct ObjState {
    /// What the object holds.
    kind: Kind,
    /// The class the storage is written with.
    class: GUID,
    /// A 3D object's model, look and picture; `None` for a new, empty one.
    object3d: Option<Object3d>,
    /// The CDX as last saved by the user (or loaded); `None` for a new, empty object.
    cdx: Option<Vec<u8>>,
    /// [`ObjState::cdx`] read, for the picture.
    molecule: Option<Molecule>,
    /// The storage handed to `InitNew`/`Load`/`SaveCompleted`, until `HandsOffStorage`.
    storage: Option<IStorage>,
    /// Changed since the container last saved the object.
    dirty: bool,
    site: Option<IOleClientSite>,
    ole_advise: Option<IOleAdviseHolder>,
    data_advise: Option<IDataAdviseHolder>,
    host: HostNames,
    /// Open in the app's window.
    open: bool,
}

impl ObjState {
    fn new(own_class: bool, kind: Kind) -> Self {
        let class = match kind {
            Kind::Model3d => CLSID_3D,
            Kind::Structure if own_class => CLSID,
            Kind::Structure => CHEMDRAW_CLSID,
        };
        Self {
            kind,
            class: GUID::from_u128(class),
            object3d: None,
            cdx: None,
            molecule: None,
            storage: None,
            dirty: false,
            site: None,
            ole_advise: None,
            data_advise: None,
            host: HostNames::default(),
            open: false,
        }
    }
}

#[implement(
    IOleObject,
    IDataObject,
    IPersistStorage,
    IRunnableObject,
    Agile = false
)]
struct EmbeddedObject {
    id: ObjectId,
    state: RefCell<ObjState>,
}

impl Drop for EmbeddedObject {
    fn drop(&mut self) {
        let id = self.id;
        with_server(|s| {
            s.objects.remove(&id);
            s.check_idle();
        });
    }
}

impl EmbeddedObject_Impl {
    fn kind(&self) -> Kind {
        self.state.borrow().kind
    }

    /// Take `cdx` as the object's contents (the user saved in the app).
    fn set_contents(&self, cdx: Vec<u8>) {
        let mut st = self.state.borrow_mut();
        st.molecule = io::decode(Format::Cdx, &cdx).ok().map(|l| l.molecule);
        st.cdx = Some(cdx);
        st.dirty = true;
    }

    /// Take `object` as the 3D object's contents (the user saved in the app). Without a
    /// picture the one the object has is kept, with its size.
    fn set_model(&self, mut object: Object3d) {
        let mut st = self.state.borrow_mut();
        if object.png.is_empty()
            && let Some(old) = &st.object3d
        {
            object.png = old.png.clone();
            object.look.extent = old.look.extent;
        }
        st.object3d = Some(object);
        st.dirty = true;
    }

    /// Take what the app handed back on a close.
    fn set_native(&self, native: Native) {
        match native {
            Native::Cdx(cdx) => self.set_contents(cdx),
            Native::Model3d(object) => self.set_model(*object),
        }
    }

    /// Tell the container the contents changed: new picture, and save the object into its
    /// document.
    fn notify_changed(&self) {
        let (site, data_advise, ole_advise) = {
            let st = self.state.borrow();
            (
                st.site.clone(),
                st.data_advise.clone(),
                st.ole_advise.clone(),
            )
        };
        let me: IDataObject = self.to_interface();
        // SAFETY: calls to the container's own objects, from their apartment; no state is
        // borrowed while they run (they call back into this object).
        unsafe {
            if let Some(h) = &data_advise
                && let Err(e) = h.SendOnDataChange(&me, None, 0)
            {
                super::log(&format!("OnDataChange failed: {e}"));
            }
            if let Some(site) = &site
                && let Err(e) = site.SaveObject()
            {
                super::log(&format!("SaveObject failed: {e}"));
            }
            if let Some(h) = &ole_advise {
                let _ = h.SendOnSave();
            }
        }
    }

    /// The window no longer shows the object: tell the container, which then lets it go.
    fn end_session(&self) {
        let (site, ole_advise) = {
            let mut st = self.state.borrow_mut();
            st.open = false;
            (st.site.clone(), st.ole_advise.clone())
        };
        // SAFETY: as in notify_changed.
        unsafe {
            if let Some(site) = &site {
                let _ = site.OnShowWindow(false);
            }
            if let Some(h) = &ole_advise {
                let _ = h.SendOnClose();
            }
        }
    }

    /// Show the object in the app's window.
    fn show(&self) {
        let (site, message, first) = {
            let mut st = self.state.borrow_mut();
            let first = !st.open;
            st.open = true;
            let (id, host) = (self.id, st.host.clone());
            let message = match st.kind {
                Kind::Structure => ToApp::Open {
                    id,
                    cdx: st.cdx.clone(),
                    host,
                },
                Kind::Model3d => ToApp::Open3d {
                    id,
                    object: st.object3d.clone().map(Box::new),
                    host,
                },
            };
            (st.site.clone(), message, first)
        };
        with_server(|s| {
            s.side.send(message);
            if first {
                let _ = s.to_main.send(MainEvent::Shown);
            }
        });
        // SAFETY: calls to the container's site from its apartment.
        unsafe {
            if let Some(site) = &site {
                let _ = site.ShowObject();
                let _ = site.OnShowWindow(true);
            }
        }
    }

    fn cdx_or_empty(&self) -> Vec<u8> {
        let st = self.state.borrow();
        match &st.cdx {
            Some(cdx) => cdx.clone(),
            None => io::encode(Format::Cdx, &Molecule::default(), None).unwrap_or_default(),
        }
    }

    fn write_to(&self, stg: &IStorage) -> Result<()> {
        let (kind, class) = {
            let st = self.state.borrow();
            (st.kind, st.class)
        };
        match kind {
            Kind::Structure => write_object(stg, &class, &self.cdx_or_empty()),
            Kind::Model3d => write_object_3d(stg, self.state.borrow().object3d.as_ref()),
        }
    }

    /// The object's native data, as the clipboard format names it: the CDX, or the MOL2.
    fn native_bytes(&self) -> Vec<u8> {
        if self.kind() == Kind::Structure {
            return self.cdx_or_empty();
        }
        let st = self.state.borrow();
        st.object3d
            .as_ref()
            .map(|o| o.mol2.clone().into_bytes())
            .unwrap_or_default()
    }

    /// The picture the container shows; `opaque` lays a 3D model's over white (for a Windows
    /// metafile, which has no alpha). The caller owns the handle.
    fn presentation(&self, opaque: bool) -> HENHMETAFILE {
        let st = self.state.borrow();
        match st.kind {
            Kind::Structure => picture(st.molecule.as_ref()),
            Kind::Model3d => st
                .object3d
                .as_ref()
                .and_then(|o| picture_3d(o, opaque))
                .unwrap_or_else(|| picture(None)),
        }
    }
}

/// A 3D object's picture as an EMF: transparent, or over white when `opaque`. `None` without a
/// picture. The caller owns the handle.
pub(crate) fn picture_3d(object: &Object3d, opaque: bool) -> Option<HENHMETAFILE> {
    let pixels = object.pixels()?;
    let emf = if opaque {
        picture3d::opaque_emf(&pixels, object.look.extent)
    } else {
        picture3d::alpha_emf(&pixels, object.look.extent)
    };
    emf.ok().map(picture3d::Emf::into_raw)
}

/// Write a 3D object's storage: the 3D class, its `\x01CompObj` and its streams (an empty
/// `CONTENTS` for a new object nothing was made in yet).
pub(crate) fn write_object_3d(stg: &IStorage, object: Option<&Object3d>) -> Result<()> {
    let class = GUID::from_u128(CLSID_3D);
    // SAFETY: a valid storage; streams are created (replacing old ones) and written in full.
    unsafe {
        WriteClassStg(stg, &class)?;
        write_stream(stg, w!("\u{1}CompObj"), &compobj::for_class(CLSID_3D))?;
        match object {
            Some(object) => {
                for (name, bytes) in object.streams() {
                    write_stream(stg, &HSTRING::from(name), &bytes)?;
                }
            }
            None => write_stream(stg, &HSTRING::from(model3d::CONTENTS), &[])?,
        }
    }
    Ok(())
}

/// The 3D object in `stg`, as [`write_object_3d`] wrote it; `None` when it has no model.
pub(crate) fn read_object_3d(stg: &IStorage) -> Option<Object3d> {
    // SAFETY: reading streams of a valid storage.
    Object3d::from_streams(|name| unsafe { read_stream(stg, &HSTRING::from(name)) })
}

/// Write an object's storage: class, `\x01CompObj` and `CONTENTS`.
pub(crate) fn write_object(stg: &IStorage, class: &GUID, cdx: &[u8]) -> Result<()> {
    // SAFETY: a valid storage; streams are created (replacing old ones) and written in full.
    unsafe {
        WriteClassStg(stg, class)?;
        write_stream(
            stg,
            w!("\u{1}CompObj"),
            &compobj::for_class(class.to_u128()),
        )?;
        write_stream(stg, w!("CONTENTS"), cdx)?;
    }
    Ok(())
}

unsafe fn write_stream(stg: &IStorage, name: impl Param<PCWSTR>, data: &[u8]) -> Result<()> {
    // SAFETY: the caller passes a valid storage; data is written from a live slice.
    unsafe {
        let stream =
            stg.CreateStream(name, STGM_CREATE | STGM_WRITE | STGM_SHARE_EXCLUSIVE, 0, 0)?;
        let mut written = 0u32;
        stream
            .Write(data.as_ptr().cast(), data.len() as u32, Some(&mut written))
            .ok()?;
        if written as usize != data.len() {
            return Err(E_FAIL.into());
        }
    }
    Ok(())
}

pub(crate) unsafe fn read_stream(stg: &IStorage, name: impl Param<PCWSTR>) -> Option<Vec<u8>> {
    // SAFETY: the caller passes a valid storage; reads go into a live buffer.
    unsafe {
        let stream = stg
            .OpenStream(name, None, STGM_READ | STGM_SHARE_EXCLUSIVE, 0)
            .ok()?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            let hr = stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read));
            if hr.is_err() {
                return None;
            }
            out.extend_from_slice(&buf[..read as usize]);
            if read == 0 || hr == S_FALSE {
                break;
            }
        }
        Some(out)
    }
}

// ── Presentation ──

/// The structure's picture, in the app's drawing style so it looks as the canvas does (a
/// blank one for an empty object). The caller owns the handle.
fn picture(mol: Option<&Molecule>) -> HENHMETAFILE {
    let style = super::picture_style();
    if let Some(h) =
        mol.and_then(|m| chembuider_rs::molecule::image::molecule_to_emf_styled(m, &style))
    {
        return HENHMETAFILE(h as *mut core::ffi::c_void);
    }
    // An empty object: a light grey frame, 2.54 × 1.27 cm, so there is something to
    // double-click (and a metafile with no records cannot be converted for CF_METAFILEPICT).
    let frame = RECT {
        left: 0,
        top: 0,
        right: 2540,
        bottom: 1270,
    };
    // SAFETY: a memory metafile drawn in the screen's units and closed at once; the pen is
    // deselected and freed before closing.
    unsafe {
        let screen = GetDC(None);
        let (dx, dy) = (
            GetDeviceCaps(Some(screen), LOGPIXELSX),
            GetDeviceCaps(Some(screen), LOGPIXELSY),
        );
        ReleaseDC(None, screen);
        let dc = CreateEnhMetaFileW(None, PCWSTR::null(), Some(&frame), PCWSTR::null());
        let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00C0_C0C0));
        let old_pen = SelectObject(dc, pen.into());
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let _ = Rectangle(dc, 0, 0, dx - 1, dy / 2 - 1);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(pen.into());
        CloseEnhMetaFile(dc)
    }
}

/// A picture's size in HIMETRIC (its frame is kept in 0.01 mm).
fn picture_size(h: HENHMETAFILE) -> SIZE {
    let mut header = ENHMETAHEADER::default();
    // SAFETY: reads the header into a local of the size given.
    unsafe {
        GetEnhMetaFileHeader(
            h,
            std::mem::size_of::<ENHMETAHEADER>() as u32,
            Some(&mut header),
        );
    }
    let f = header.rclFrame;
    SIZE {
        cx: (f.right - f.left).max(1),
        cy: (f.bottom - f.top).max(1),
    }
}

/// The picture as a `METAFILEPICT` in global memory (`CF_METAFILEPICT`), for containers whose
/// cache keeps Windows metafiles.
pub(crate) fn metafile_pict(h: HENHMETAFILE) -> Result<HGLOBAL> {
    let size = picture_size(h);
    // SAFETY: converts our own metafile against the screen DC; the result is copied into a
    // new handle owned by the METAFILEPICT, which the receiver frees.
    unsafe {
        let dc = GetDC(None);
        let n = GetWinMetaFileBits(h, None, MM_ANISOTROPIC.0, dc);
        let mut bits = vec![0u8; n as usize];
        GetWinMetaFileBits(h, Some(&mut bits), MM_ANISOTROPIC.0, dc);
        ReleaseDC(None, dc);
        if n == 0 {
            return Err(E_FAIL.into());
        }
        let hmf = SetMetaFileBitsEx(&bits);
        let g = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<METAFILEPICT>())?;
        let p = GlobalLock(g).cast::<METAFILEPICT>();
        if p.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        p.write(METAFILEPICT {
            mm: MM_ANISOTROPIC.0,
            xExt: size.cx,
            yExt: size.cy,
            hMF: hmf,
        });
        let _ = GlobalUnlock(g);
        Ok(g)
    }
}

fn global_bytes(bytes: &[u8]) -> Result<HGLOBAL> {
    // SAFETY: a new movable block of the right size, filled while locked.
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(g).cast::<u8>();
        if p.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(g);
        Ok(g)
    }
}

/// The bytes of a compound file in memory that `write` fills.
pub(crate) fn storage_bytes(write: impl FnOnce(&IStorage) -> Result<()>) -> Result<Vec<u8>> {
    // SAFETY: an HGLOBAL-backed ILockBytes and a storage on it, committed and released before
    // the bytes (as many as the ILockBytes says it holds) are copied out of the locked block.
    unsafe {
        let bytes = CreateILockBytesOnHGlobal(None, true)?;
        {
            let stg = StgCreateDocfileOnILockBytes(
                &bytes,
                STGM_CREATE | STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
                0,
            )?;
            write(&stg)?;
            stg.Commit(0)?;
        }
        let mut stat = windows::Win32::System::Com::STATSTG::default();
        bytes.Stat(
            &mut stat,
            windows::Win32::System::Com::STATFLAG_NONAME.0 as u32,
        )?;
        let g = GetHGlobalFromILockBytes(&bytes)?;
        let p = GlobalLock(g).cast::<u8>();
        if p.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        let out = std::slice::from_raw_parts(p, stat.cbSize as usize).to_vec();
        let _ = GlobalUnlock(g);
        Ok(out)
    }
}

/// A new compound file in memory.
pub(crate) fn memory_storage() -> Result<IStorage> {
    // SAFETY: an HGLOBAL-backed ILockBytes freed with the storage.
    unsafe {
        let bytes = CreateILockBytesOnHGlobal(None, true)?;
        StgCreateDocfileOnILockBytes(
            &bytes,
            STGM_CREATE | STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
            0,
        )
    }
}

/// The formats the object offers: its picture both ways, its native data (the CDX, or a 3D
/// model's MOL2) and itself as an embedding.
struct Formats {
    native: u16,
    embed_source: u16,
}

impl Formats {
    fn get(kind: Kind) -> Self {
        let native = match kind {
            Kind::Structure => HSTRING::from(super::CDX_FORMAT_NAME),
            Kind::Model3d => HSTRING::from(model3d::MOL2_FORMAT_NAME),
        };
        // SAFETY: registering (or looking up) named clipboard formats.
        unsafe {
            Self {
                native: RegisterClipboardFormatW(&native) as u16,
                embed_source: RegisterClipboardFormatW(w!("Embed Source")) as u16,
            }
        }
    }

    fn list(&self) -> Vec<FORMATETC> {
        let f = |cf: u16, tymed: i32| FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: tymed as u32,
        };
        vec![
            f(CF_ENHMETAFILE.0, TYMED_ENHMF.0),
            f(CF_METAFILEPICT.0, TYMED_MFPICT.0),
            f(self.native, TYMED_HGLOBAL.0),
            f(self.embed_source, TYMED_ISTORAGE.0),
        ]
    }

    /// Check `f` against what is offered.
    fn check(&self, f: &FORMATETC) -> HRESULT {
        if f.dwAspect != DVASPECT_CONTENT.0 && f.dwAspect != DVASPECT_DOCPRINT.0 {
            return DV_E_DVASPECT;
        }
        match self.list().iter().find(|o| o.cfFormat == f.cfFormat) {
            None => DV_E_FORMATETC,
            Some(o) if o.tymed & f.tymed == 0 => DV_E_TYMED,
            Some(_) => S_OK,
        }
    }
}

fn medium(tymed: i32, u: STGMEDIUM_0) -> STGMEDIUM {
    STGMEDIUM {
        tymed: tymed as u32,
        u,
        pUnkForRelease: ManuallyDrop::new(None),
    }
}

impl IDataObject_Impl for EmbeddedObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> Result<STGMEDIUM> {
        // SAFETY: COM passes a valid FORMATETC (or null, refused).
        let f = unsafe { pformatetcin.as_ref() }.ok_or(E_INVALIDARG)?;
        super::log(&format!(
            "IDataObject::GetData cf={} tymed={} aspect={}",
            f.cfFormat, f.tymed, f.dwAspect
        ));
        let formats = Formats::get(self.kind());
        formats.check(f).ok()?;
        let cf = f.cfFormat;
        if cf == CF_ENHMETAFILE.0 {
            let h = self.presentation(false);
            Ok(medium(TYMED_ENHMF.0, STGMEDIUM_0 { hEnhMetaFile: h }))
        } else if cf == CF_METAFILEPICT.0 {
            let h = self.presentation(true);
            let pict = metafile_pict(h);
            // SAFETY: our own metafile, no longer needed once converted.
            unsafe {
                let _ = DeleteEnhMetaFile(Some(h));
            }
            Ok(medium(
                TYMED_MFPICT.0,
                STGMEDIUM_0 {
                    hMetaFilePict: pict?.0,
                },
            ))
        } else if cf == formats.native {
            let g = global_bytes(&self.native_bytes())?;
            Ok(medium(TYMED_HGLOBAL.0, STGMEDIUM_0 { hGlobal: g }))
        } else {
            let stg = memory_storage()?;
            self.write_to(&stg)?;
            // SAFETY: committing our own new storage.
            unsafe { stg.Commit(0)? };
            Ok(medium(
                TYMED_ISTORAGE.0,
                STGMEDIUM_0 {
                    pstg: ManuallyDrop::new(Some(stg)),
                },
            ))
        }
    }

    fn GetDataHere(&self, pformatetc: *const FORMATETC, pmedium: *mut STGMEDIUM) -> Result<()> {
        super::log("IDataObject::GetDataHere");
        // SAFETY: COM passes valid pointers (or null, refused).
        let (f, m) = unsafe { (pformatetc.as_ref(), pmedium.as_ref()) };
        let (f, m) = (f.ok_or(E_INVALIDARG)?, m.ok_or(E_INVALIDARG)?);
        let formats = Formats::get(self.kind());
        if f.cfFormat != formats.embed_source || m.tymed != TYMED_ISTORAGE.0 as u32 {
            return Err(DV_E_FORMATETC.into());
        }
        // SAFETY: the medium is a storage, as just checked.
        let stg = unsafe { (*m.u.pstg).clone() }.ok_or(E_INVALIDARG)?;
        self.write_to(&stg)
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        // SAFETY: as in GetData.
        match unsafe { pformatetc.as_ref() } {
            Some(f) => Formats::get(self.kind()).check(f),
            None => E_INVALIDARG,
        }
    }

    fn GetCanonicalFormatEtc(&self, _in: *const FORMATETC, out: *mut FORMATETC) -> HRESULT {
        // SAFETY: COM passes a valid out-pointer (or null, refused).
        match unsafe { out.as_mut() } {
            Some(out) => {
                out.ptd = std::ptr::null_mut();
                DATA_S_SAMEFORMATETC
            }
            None => E_INVALIDARG,
        }
    }

    fn SetData(&self, _f: *const FORMATETC, _m: *const STGMEDIUM, _release: BOOL) -> Result<()> {
        super::log("IDataObject::SetData");
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, direction: u32) -> Result<IEnumFORMATETC> {
        super::log("IDataObject::EnumFormatEtc");
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        // SAFETY: the shell copies the array.
        unsafe { SHCreateStdEnumFmtEtc(&Formats::get(self.kind()).list()) }
    }

    fn DAdvise(
        &self,
        pformatetc: *const FORMATETC,
        advf: u32,
        sink: Ref<IAdviseSink>,
    ) -> Result<u32> {
        super::log("IDataObject::DAdvise");
        let holder = {
            let mut st = self.state.borrow_mut();
            if st.data_advise.is_none() {
                // SAFETY: plain constructor.
                st.data_advise = Some(unsafe { CreateDataAdviseHolder()? });
            }
            st.data_advise.clone().expect("just set")
        };
        let me: IDataObject = self.to_interface();
        // SAFETY: the holder may call GetData right away (ADVF_PRIMEFIRST): nothing is borrowed.
        unsafe { holder.Advise(&me, pformatetc, advf, sink.as_ref()) }
    }

    fn DUnadvise(&self, connection: u32) -> Result<()> {
        super::log("IDataObject::DUnadvise");
        let holder = self.state.borrow().data_advise.clone();
        match holder {
            // SAFETY: forwarding to the holder.
            Some(h) => unsafe { h.Unadvise(connection) },
            None => Err(OLE_E_NOCONNECTION.into()),
        }
    }

    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
        super::log("IDataObject::EnumDAdvise");
        let holder = self.state.borrow().data_advise.clone();
        match holder {
            // SAFETY: forwarding to the holder.
            Some(h) => unsafe { h.EnumAdvise() },
            None => Err(E_FAIL.into()),
        }
    }
}

impl IPersist_Impl for EmbeddedObject_Impl {
    fn GetClassID(&self) -> Result<GUID> {
        super::log("IPersist::GetClassID");
        Ok(self.state.borrow().class)
    }
}

impl IPersistStorage_Impl for EmbeddedObject_Impl {
    fn IsDirty(&self) -> HRESULT {
        super::log("IPersistStorage::IsDirty");
        if self.state.borrow().dirty {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn InitNew(&self, pstg: Ref<IStorage>) -> Result<()> {
        super::log("IPersistStorage::InitNew");
        let stg = pstg.ok()?.clone();
        let mut st = self.state.borrow_mut();
        st.storage = Some(stg);
        st.dirty = true;
        Ok(())
    }

    fn Load(&self, pstg: Ref<IStorage>) -> Result<()> {
        super::log("IPersistStorage::Load");
        let stg = pstg.ok()?.clone();
        // SAFETY: reading the container's storage.
        let class = unsafe { ReadClassStg(&stg).ok() }.filter(|c| *c != GUID::zeroed());
        if class.map_or(self.kind() == Kind::Model3d, |c| c.to_u128() == CLSID_3D) {
            let object = read_object_3d(&stg);
            let mut st = self.state.borrow_mut();
            st.kind = Kind::Model3d;
            st.class = GUID::from_u128(CLSID_3D);
            st.object3d = object;
            st.storage = Some(stg);
            st.dirty = false;
            return Ok(());
        }
        // SAFETY: reading the container's storage.
        let cdx = unsafe { read_stream(&stg, w!("CONTENTS")) };
        let mut st = self.state.borrow_mut();
        st.kind = Kind::Structure;
        if let Some(class) = class {
            st.class = class;
        }
        st.molecule = cdx
            .as_deref()
            .and_then(|c| io::decode(Format::Cdx, c).ok())
            .map(|l| l.molecule);
        st.cdx = cdx;
        st.storage = Some(stg);
        st.dirty = false;
        Ok(())
    }

    fn Save(&self, pstgsave: Ref<IStorage>, _same_as_load: BOOL) -> Result<()> {
        super::log("IPersistStorage::Save");
        self.write_to(pstgsave.ok()?)
    }

    fn SaveCompleted(&self, pstgnew: Ref<IStorage>) -> Result<()> {
        super::log("IPersistStorage::SaveCompleted");
        let mut st = self.state.borrow_mut();
        if let Some(stg) = pstgnew.as_ref() {
            st.storage = Some(stg.clone());
        }
        st.dirty = false;
        Ok(())
    }

    fn HandsOffStorage(&self) -> Result<()> {
        super::log("IPersistStorage::HandsOffStorage");
        self.state.borrow_mut().storage = None;
        Ok(())
    }
}

fn co_string(s: &str) -> Result<PWSTR> {
    let wide: Vec<u16> = s.encode_utf16().chain([0]).collect();
    // SAFETY: a CoTaskMem block of the right size, which the caller frees.
    unsafe {
        let p = CoTaskMemAlloc(wide.len() * 2).cast::<u16>();
        if p.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        Ok(PWSTR(p))
    }
}

fn pcwstr_string(p: &PCWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: a NUL-terminated string from the container.
        unsafe { p.to_string() }.unwrap_or_default()
    }
}

impl IOleObject_Impl for EmbeddedObject_Impl {
    fn SetClientSite(&self, site: Ref<IOleClientSite>) -> Result<()> {
        super::log("IOleObject::SetClientSite");
        self.state.borrow_mut().site = site.cloned();
        Ok(())
    }

    fn GetClientSite(&self) -> Result<IOleClientSite> {
        super::log("IOleObject::GetClientSite");
        self.state
            .borrow()
            .site
            .clone()
            .ok_or_else(|| E_FAIL.into())
    }

    fn SetHostNames(&self, app: &PCWSTR, obj: &PCWSTR) -> Result<()> {
        super::log("IOleObject::SetHostNames");
        self.state.borrow_mut().host = HostNames {
            app: pcwstr_string(app),
            document: pcwstr_string(obj),
        };
        Ok(())
    }

    fn Close(&self, option: &OLECLOSE) -> Result<()> {
        super::log("IOleObject::Close");
        let (open, dirty, site) = {
            let st = self.state.borrow();
            (st.open, st.dirty, st.site.clone())
        };
        let id = self.id;
        if open {
            let reply = if *option == OLECLOSE_NOSAVE {
                with_server(|s| s.side.send(ToApp::Close { id, reply: None }));
                None
            } else {
                let (tx, rx) = mpsc::channel();
                with_server(|s| {
                    s.side.send(ToApp::Close {
                        id,
                        reply: Some(tx),
                    })
                });
                rx.recv_timeout(CLOSE_REPLY_TIMEOUT).ok().flatten()
            };
            if let Some(native) = reply {
                self.set_native(native);
                self.notify_changed();
            }
            self.end_session();
        } else {
            if dirty
                && *option == OLECLOSE_SAVEIFDIRTY
                && let Some(site) = &site
            {
                // SAFETY: a call to the container's site from its apartment.
                unsafe {
                    let _ = site.SaveObject();
                }
            }
            let holder = self.state.borrow().ole_advise.clone();
            if let Some(h) = holder {
                // SAFETY: as above.
                unsafe {
                    let _ = h.SendOnClose();
                }
            }
        }
        Ok(())
    }

    fn SetMoniker(&self, _which: &OLEWHICHMK, _mk: Ref<IMoniker>) -> Result<()> {
        super::log("IOleObject::SetMoniker");
        Ok(())
    }

    fn GetMoniker(&self, assign: &OLEGETMONIKER, which: &OLEWHICHMK) -> Result<IMoniker> {
        super::log("IOleObject::GetMoniker");
        let site = self.state.borrow().site.clone();
        match site {
            // SAFETY: forwarding to the container's site.
            Some(site) => unsafe { site.GetMoniker(*assign, *which) },
            None => Err(E_FAIL.into()),
        }
    }

    fn InitFromData(&self, _data: Ref<IDataObject>, _creation: BOOL, _r: u32) -> Result<()> {
        super::log("IOleObject::InitFromData");
        Err(E_NOTIMPL.into())
    }

    fn GetClipboardData(&self, _r: u32) -> Result<IDataObject> {
        super::log("IOleObject::GetClipboardData");
        Err(E_NOTIMPL.into())
    }

    fn DoVerb(
        &self,
        verb: i32,
        _msg: *const MSG,
        active_site: Ref<IOleClientSite>,
        _lindex: i32,
        _parent: HWND,
        _rect: *const RECT,
    ) -> Result<()> {
        super::log(&format!("IOleObject::DoVerb {verb}"));
        if verb == OLEIVERB_HIDE.0 || verb == OLEIVERB_DISCARDUNDOSTATE.0 {
            return Ok(());
        }
        {
            let mut st = self.state.borrow_mut();
            if st.site.is_none() {
                st.site = active_site.cloned();
            }
        }
        // Every other verb (edit, open, show, and the in-place ones, which are not supported)
        // opens the object in the app's window.
        self.show();
        Ok(())
    }

    fn EnumVerbs(&self) -> Result<IEnumOLEVERB> {
        // Our class's verbs, whatever class the object is stored as.
        // SAFETY: reads the registry.
        unsafe { OleRegEnumVerbs(&GUID::from_u128(self.kind().class())) }
    }

    fn Update(&self) -> Result<()> {
        super::log("IOleObject::Update");
        Ok(())
    }

    fn IsUpToDate(&self) -> Result<()> {
        super::log("IOleObject::IsUpToDate");
        Ok(())
    }

    fn GetUserClassID(&self) -> Result<GUID> {
        super::log("IOleObject::GetUserClassID");
        Ok(self.state.borrow().class)
    }

    fn GetUserType(&self, form: &USERCLASSTYPE) -> Result<PWSTR> {
        super::log("IOleObject::GetUserType");
        let kind = self.kind();
        co_string(if *form == USERCLASSTYPE_SHORT {
            kind.user_type_short()
        } else if *form == USERCLASSTYPE_APPNAME {
            crate::APP_NAME
        } else {
            kind.user_type()
        })
    }

    fn SetExtent(&self, _aspect: DVASPECT, _size: *const SIZE) -> Result<()> {
        super::log("IOleObject::SetExtent");
        // The drawing keeps its own size; a resized object stretches its picture.
        Ok(())
    }

    fn GetExtent(&self, aspect: DVASPECT) -> Result<SIZE> {
        if aspect != DVASPECT_CONTENT {
            return Err(E_INVALIDARG.into());
        }
        let h = self.presentation(false);
        let size = picture_size(h);
        // SAFETY: our own metafile.
        unsafe {
            let _ = DeleteEnhMetaFile(Some(h));
        }
        Ok(size)
    }

    fn Advise(&self, sink: Ref<IAdviseSink>) -> Result<u32> {
        super::log("IOleObject::Advise");
        let holder = {
            let mut st = self.state.borrow_mut();
            if st.ole_advise.is_none() {
                // SAFETY: plain constructor.
                st.ole_advise = Some(unsafe { CreateOleAdviseHolder()? });
            }
            st.ole_advise.clone().expect("just set")
        };
        // SAFETY: forwarding to the holder.
        unsafe { holder.Advise(sink.as_ref()) }
    }

    fn Unadvise(&self, connection: u32) -> Result<()> {
        super::log("IOleObject::Unadvise");
        let holder = self.state.borrow().ole_advise.clone();
        match holder {
            // SAFETY: forwarding to the holder.
            Some(h) => unsafe { h.Unadvise(connection) },
            None => Err(OLE_E_NOCONNECTION.into()),
        }
    }

    fn EnumAdvise(&self) -> Result<IEnumSTATDATA> {
        super::log("IOleObject::EnumAdvise");
        let holder = self.state.borrow().ole_advise.clone();
        match holder {
            // SAFETY: forwarding to the holder.
            Some(h) => unsafe { h.EnumAdvise() },
            None => Err(E_FAIL.into()),
        }
    }

    fn GetMiscStatus(&self, _aspect: DVASPECT) -> Result<OLEMISC> {
        Ok(OLEMISC_CANTLINKINSIDE)
    }

    fn SetColorScheme(&self, _palette: *const LOGPALETTE) -> Result<()> {
        super::log("IOleObject::SetColorScheme");
        Err(E_NOTIMPL.into())
    }
}

impl IRunnableObject_Impl for EmbeddedObject_Impl {
    fn GetRunningClass(&self) -> Result<GUID> {
        super::log("IRunnableObject::GetRunningClass");
        Ok(self.state.borrow().class)
    }

    fn Run(&self, _bc: Ref<IBindCtx>) -> Result<()> {
        super::log("IRunnableObject::Run");
        Ok(())
    }

    fn IsRunning(&self) -> BOOL {
        true.into()
    }

    fn LockRunning(&self, _lock: BOOL, _last_unlock_closes: BOOL) -> Result<()> {
        super::log("IRunnableObject::LockRunning");
        Ok(())
    }

    fn SetContainedObject(&self, _contained: BOOL) -> Result<()> {
        super::log("IRunnableObject::SetContainedObject");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CompObj chembuider-rs writes into its ChemDraw embeddings (the clipboard's "Embed
    /// Source") is the one written here.
    #[test]
    fn compobj_matches_chembuiders_embedding() {
        let mut mol = Molecule::default();
        mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let bytes = chembuider_rs::molecule::ole::molecule_to_ole_embed(&mol).unwrap();
        // SAFETY: an in-memory compound file opened read-only.
        unsafe {
            let g = global_bytes(&bytes).unwrap();
            let lb = CreateILockBytesOnHGlobal(Some(g), true).unwrap();
            let stg = windows::Win32::System::Com::StructuredStorage::StgOpenStorageOnILockBytes(
                &lb,
                None,
                STGM_READ | STGM_SHARE_EXCLUSIVE,
                None,
                None,
            )
            .unwrap();
            assert_eq!(
                read_stream(&stg, w!("\u{1}CompObj")).unwrap(),
                compobj::chemdraw()
            );
            assert_eq!(ReadClassStg(&stg).unwrap(), GUID::from_u128(CHEMDRAW_CLSID));
        }
    }

    #[test]
    fn an_object_storage_round_trips_class_and_contents() {
        let mut mol = Molecule::default();
        mol.add_atom("N".to_owned(), [0.0, 0.0], 0);
        let cdx = io::encode(Format::Cdx, &mol, None).unwrap();
        let stg = memory_storage().unwrap();
        let class = GUID::from_u128(CHEMDRAW_CLSID);
        write_object(&stg, &class, &cdx).unwrap();
        // SAFETY: reading back our own storage.
        unsafe {
            assert_eq!(ReadClassStg(&stg).unwrap(), class);
            assert_eq!(read_stream(&stg, w!("CONTENTS")).unwrap(), cdx);
        }
    }

    /// A 3D object with a picture: ethanol in spheres, a half-transparent dot drawn.
    fn object_3d() -> Object3d {
        let model = model3d::tests::ethanol();
        let look = model3d::Look3d {
            style: crate::ui::viewport3d::Style3d::Spheres,
            ..Default::default()
        };
        let mut object = Object3d::new(&model, look, None).unwrap();
        let mut rgba = vec![0u8; 8 * 6 * 4];
        rgba[(3 * 8 + 3) * 4..(3 * 8 + 3) * 4 + 4].copy_from_slice(&[90, 30, 10, 160]);
        object.set_picture(&crate::ui::viewport3d::Picture {
            rgba,
            width: 8,
            height: 6,
            px_per_point: 2.0,
        });
        object
    }

    /// The record types of an EMF.
    fn emf_records(h: HENHMETAFILE) -> Vec<u32> {
        // SAFETY: reading our own metafile into a buffer of its size.
        let bits = unsafe {
            let size = windows::Win32::Graphics::Gdi::GetEnhMetaFileBits(h, None) as usize;
            let mut bits = vec![0u8; size];
            windows::Win32::Graphics::Gdi::GetEnhMetaFileBits(h, Some(&mut bits));
            bits
        };
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= bits.len() {
            out.push(u32::from_le_bytes(bits[at..at + 4].try_into().unwrap()));
            let len = u32::from_le_bytes(bits[at + 4..at + 8].try_into().unwrap()) as usize;
            if len == 0 {
                break;
            }
            at += len;
        }
        out
    }

    const ALPHABLEND: u32 = windows::Win32::Graphics::Gdi::EMR_ALPHABLEND.0;

    #[test]
    fn a_3d_object_storage_round_trips_its_model_look_and_picture() {
        let object = object_3d();
        let stg = memory_storage().unwrap();
        write_object_3d(&stg, Some(&object)).unwrap();
        // SAFETY: reading back our own storage.
        unsafe {
            assert_eq!(ReadClassStg(&stg).unwrap(), GUID::from_u128(CLSID_3D));
            assert_eq!(
                read_stream(&stg, w!("\u{1}CompObj")).unwrap(),
                compobj::for_class(CLSID_3D)
            );
            assert_eq!(
                read_stream(&stg, w!("CONTENTS")).unwrap(),
                object.mol2.as_bytes()
            );
        }
        assert_eq!(read_object_3d(&stg).unwrap(), object);

        // A new object writes an empty CONTENTS: no model to read.
        let empty = memory_storage().unwrap();
        write_object_3d(&empty, None).unwrap();
        assert_eq!(read_object_3d(&empty), None);
    }

    #[test]
    fn a_3d_objects_presentation_is_transparent_and_sized_as_its_look() {
        let object = object_3d();
        let h = picture_3d(&object, false).unwrap();
        let records = emf_records(h);
        assert!(records.contains(&ALPHABLEND), "{records:?}");
        let size = picture_size(h);
        assert_eq!([size.cx, size.cy], object.look.extent);
        // The Windows metafile is made from the opaque one.
        let opaque = picture_3d(&object, true).unwrap();
        assert!(!emf_records(opaque).contains(&ALPHABLEND));
        let g = metafile_pict(opaque).unwrap();
        // SAFETY: freeing what was made here.
        unsafe {
            let _ = DeleteEnhMetaFile(Some(h));
            let _ = DeleteEnhMetaFile(Some(opaque));
            let p = GlobalLock(g).cast::<METAFILEPICT>();
            assert_eq!(((*p).xExt, (*p).yExt), (size.cx, size.cy));
            let _ = windows::Win32::Graphics::Gdi::DeleteMetaFile((*p).hMF);
            let _ = GlobalUnlock(g);
            let _ = windows::Win32::Foundation::GlobalFree(Some(g));
        }
        let mut no_picture = object.clone();
        no_picture.png.clear();
        assert!(picture_3d(&no_picture, false).is_none());
    }

    /// The object a container loads from a 3D storage serves the model, its MOL2 and its
    /// transparent picture, and saves them back; whichever class factory made it.
    #[test]
    fn an_embedded_3d_object_loads_serves_and_saves() {
        // SAFETY: OLE on this test thread (the storages and data objects need it).
        unsafe { OleInitialize(None) }.unwrap();
        let object = object_3d();
        let stg = memory_storage().unwrap();
        write_object_3d(&stg, Some(&object)).unwrap();
        let format = |cf: u16, tymed: i32| FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: tymed as u32,
        };
        for factory in [Kind::Model3d, Kind::Structure] {
            let unknown: IUnknown = EmbeddedObject {
                id: 1,
                state: RefCell::new(ObjState::new(false, factory)),
            }
            .into();
            let persist: IPersistStorage = unknown.cast().unwrap();
            let data: IDataObject = unknown.cast().unwrap();
            // SAFETY: COM calls on our own object, on this thread.
            unsafe {
                persist.Load(&stg).unwrap();
                assert_eq!(persist.GetClassID().unwrap(), GUID::from_u128(CLSID_3D));
                let mut medium = data
                    .GetData(&format(CF_ENHMETAFILE.0, TYMED_ENHMF.0))
                    .unwrap();
                assert!(emf_records(medium.u.hEnhMetaFile).contains(&ALPHABLEND));
                windows::Win32::System::Ole::ReleaseStgMedium(&mut medium);

                let mol2 = RegisterClipboardFormatW(&HSTRING::from(model3d::MOL2_FORMAT_NAME));
                let mut medium = data.GetData(&format(mol2 as u16, TYMED_HGLOBAL.0)).unwrap();
                let g = medium.u.hGlobal;
                let p = GlobalLock(g).cast::<u8>();
                let bytes = std::slice::from_raw_parts(p, object.mol2.len());
                assert_eq!(bytes, object.mol2.as_bytes());
                let _ = GlobalUnlock(g);
                windows::Win32::System::Ole::ReleaseStgMedium(&mut medium);

                let cdx = RegisterClipboardFormatW(w!("ChemDraw Interchange Format"));
                assert_eq!(
                    data.QueryGetData(&format(cdx as u16, TYMED_HGLOBAL.0)),
                    DV_E_FORMATETC
                );
                let saved = memory_storage().unwrap();
                persist.Save(&saved, true).unwrap();
                assert_eq!(read_object_3d(&saved).unwrap(), object);
            }
        }
    }

    #[test]
    fn a_3d_update_without_a_picture_keeps_the_objects_own() {
        let object = object_3d();
        let stg = memory_storage().unwrap();
        write_object_3d(&stg, Some(&object)).unwrap();
        let com = ComObject::new(EmbeddedObject {
            id: 2,
            state: RefCell::new(ObjState::new(false, Kind::Model3d)),
        });
        let persist: IPersistStorage = com.to_interface();
        // SAFETY: a COM call on our own object, on this thread.
        unsafe { persist.Load(&stg).unwrap() };
        let mut edited = object.clone();
        edited.png.clear();
        edited.look.extent = [1, 1];
        edited.look.style = crate::ui::viewport3d::Style3d::Wireframe;
        com.set_model(edited);
        let st = com.state.borrow();
        let now = st.object3d.as_ref().unwrap();
        assert!(st.dirty);
        assert_eq!(now.png, object.png);
        assert_eq!(now.look.extent, object.look.extent);
        assert_eq!(now.look.style, crate::ui::viewport3d::Style3d::Wireframe);
    }

    #[test]
    fn pictures_have_a_size_and_convert_to_windows_metafiles() {
        let mut mol = Molecule::default();
        let a = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let b = mol.add_atom("O".to_owned(), [1.5, 0.0], 0);
        mol.add_bond(a, b, chembuider_rs::BondOrder::Double);
        for m in [Some(&mol), None] {
            let h = picture(m);
            let size = picture_size(h);
            assert!(size.cx > 0 && size.cy > 0);
            let g = metafile_pict(h).unwrap();
            // SAFETY: freeing what was made here.
            unsafe {
                let _ = DeleteEnhMetaFile(Some(h));
                let p = GlobalLock(g).cast::<METAFILEPICT>();
                assert_eq!((*p).xExt, size.cx);
                assert!(!(*p).hMF.is_invalid());
                let _ = windows::Win32::Graphics::Gdi::DeleteMetaFile((*p).hMF);
                let _ = GlobalUnlock(g);
                let _ = windows::Win32::Foundation::GlobalFree(Some(g));
            }
        }
    }
}
