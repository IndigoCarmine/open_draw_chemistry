//! The OLE clipboard on Windows: an `IDataObject` laid out like the one ChemDraw publishes, put
//! on the clipboard with `OleSetClipboard` and flushed at once so it outlives the app.
//!
//! What Word and PowerPoint need to embed on a plain paste (found by pasting ChemDraw's own copy
//! and variations of ours through their COM automation):
//!
//! * "Embed Source" — a compound document whose root class is ChemDraw's CLSID, with
//!   `\1CompObj` and the CDX in `CONTENTS`, as ChemDraw writes it — plus "Object Descriptor".
//!   ChemDraw offers the storage as `TYMED_ISTORAGE`, and so does this object.
//! * A picture for the object's cached presentation: `CF_ENHMETAFILE` and `CF_METAFILEPICT`.
//! * Nothing Office would rather paste: with a "PNG" on the clipboard Word pastes that picture,
//!   and with any text format PowerPoint pastes a text box. ChemDraw offers neither.
//!
//! `OleFlushClipboard` then renders every format onto the Win32 clipboard (the storage as a
//! compound file in an HGLOBAL, the original media recorded in "Ole Private Data", "OwnerLink" and
//! "Native" added by OLE), which is what ChemDraw leaves behind too; the widget's paste reads
//! its formats from there as before.

use std::mem::ManuallyDrop;

use chembuider_rs::Molecule;
use chembuider_rs::molecule::{cdx, image, ole};
use windows::Win32::Foundation::{
    DATA_S_SAMEFORMATETC, DV_E_FORMATETC, DV_E_TYMED, E_INVALIDARG, E_NOTIMPL, E_POINTER,
    GlobalFree, HGLOBAL, OLE_E_ADVISENOTSUPPORTED, S_OK,
};
use windows::Win32::Graphics::Gdi::{
    CopyEnhMetaFileW, DeleteEnhMetaFile, ENHMETAHEADER, GetDC, GetEnhMetaFileBits,
    GetEnhMetaFileHeader, GetWinMetaFileBits, HENHMETAFILE, MM_ANISOTROPIC, ReleaseDC,
    SetEnhMetaFileBits, SetMetaFileBitsEx,
};
use windows::Win32::System::Com::StructuredStorage::{
    CreateILockBytesOnHGlobal, IStorage, StgOpenStorageOnILockBytes,
};
use windows::Win32::System::Com::{
    DATADIR_GET, DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl,
    IEnumFORMATETC, IEnumSTATDATA, STGM_READWRITE, STGM_SHARE_EXCLUSIVE, STGMEDIUM, STGMEDIUM_0,
    TYMED_ENHMF, TYMED_HGLOBAL, TYMED_ISTORAGE, TYMED_MFPICT,
};
use windows::Win32::System::DataExchange::{METAFILEPICT, RegisterClipboardFormatW};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{OleFlushClipboard, OleInitialize, OleSetClipboard};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;
use windows::core::{BOOL, Error, GUID, HRESULT, HSTRING, Ref, Result, implement};

const CF_DIB: u16 = 8;
const CF_ENHMETAFILE: u16 = 14;
const CF_METAFILEPICT: u16 = 3;

/// ChemDraw's class id, {41BA6D21-A02E-11CE-8FD9-0020AFD1F20C}.
const CHEMDRAW_CLSID: GUID = GUID::from_u128(0x41ba6d21_a02e_11ce_8fd9_0020afd1f20c);

/// Put a copy of `mol` on the clipboard: an embeddable ChemDraw object for Office, the pictures,
/// the CDX for ChemDraw and `app_text` (the widget's own JSON) for pasting back here.
pub fn set(mol: &Molecule, app_text: &str) -> Result<()> {
    ensure_ole()?;
    let object: IDataObject = DataObject::new(mol, app_text).into();
    // Another program may hold the clipboard open for a moment; OleSetClipboard does not wait.
    let mut tries = 0;
    loop {
        match unsafe { OleSetClipboard(&object) } {
            Ok(()) => break,
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    // Render everything now: the copy then survives this app quitting, and nothing is asked of
    // this object later from inside another program's paste.
    unsafe { OleFlushClipboard() }
}

/// OLE on this thread, once. winit has usually done it already (for drag and drop), in which
/// case this only adds a reference; it fails if the thread is a multithreaded apartment.
pub(super) fn ensure_ole() -> Result<()> {
    thread_local! {
        static INITIALIZED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if INITIALIZED.get() {
        return Ok(());
    }
    unsafe { OleInitialize(None) }?;
    INITIALIZED.set(true);
    Ok(())
}

/// One format the data object offers.
struct Entry {
    format: u16,
    data: Data,
}

enum Data {
    /// Bytes handed out in an HGLOBAL.
    Global(Vec<u8>),
    /// A compound document handed out as an `IStorage`.
    Storage(Vec<u8>),
    /// [`DataObject::emf`], as itself or converted to a Windows metafile.
    EnhMetafile,
    MetafilePict,
}

impl Data {
    fn tymed(&self) -> u32 {
        (match self {
            Data::Global(_) => TYMED_HGLOBAL,
            Data::Storage(_) => TYMED_ISTORAGE,
            Data::EnhMetafile => TYMED_ENHMF,
            Data::MetafilePict => TYMED_MFPICT,
        })
        .0 as u32
    }
}

/// An enhanced metafile this object owns.
struct Emf(HENHMETAFILE);

impl Drop for Emf {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteEnhMetaFile(Some(self.0));
        }
    }
}

#[implement(IDataObject)]
struct DataObject {
    entries: Vec<Entry>,
    emf: Option<Emf>,
}

impl DataObject {
    fn new(mol: &Molecule, app_text: &str) -> Self {
        let registered =
            |name: &str| unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) } as u16;
        let descriptor = ole::object_descriptor(mol);
        // The picture at the object's size (ChemDraw's scale), so Word and PowerPoint, which
        // size the pasted object from different places, both show it as ChemDraw would.
        let emf = image::molecule_to_emf(mol).map(|h| {
            let emf = Emf(HENHMETAFILE(h as _));
            match descriptor_extent(&descriptor).and_then(|(cx, cy)| with_frame(emf.0, cx, cy)) {
                Some(scaled) => Emf(scaled),
                None => emf,
            }
        });
        let mut entries = Vec::new();
        let mut push = |format: u16, data: Data| entries.push(Entry { format, data });

        // ChemDraw's own order: CDX, then the object (Embed Source + Object Descriptor), then
        // its pictures.
        if let Some(cdx) = cdx::molecule_to_cdx_bytes(mol) {
            push(registered("ChemDraw Interchange Format"), Data::Global(cdx));
        }
        if let Some(embed) = ole::molecule_to_ole_embed(mol) {
            push(registered("Embed Source"), Data::Storage(embed));
            push(registered("Object Descriptor"), Data::Global(descriptor));
        }
        if emf.is_some() {
            push(CF_ENHMETAFILE, Data::EnhMetafile);
            push(CF_METAFILEPICT, Data::MetafilePict);
        }
        // A bitmap for programs without metafile support. Deliberately no "PNG": Word pastes a
        // PNG picture in preference to the object. And no text format either: PowerPoint
        // pastes any text as a text box.
        if let Some(dib) = image::molecule_to_dib(mol) {
            push(CF_DIB, Data::Global(dib));
        }
        // The widget's paste reads this back first (full fidelity).
        push(
            registered("ChemBuilder Molecule"),
            Data::Global(app_text.as_bytes().to_vec()),
        );
        Self { entries, emf }
    }

    fn formats(&self) -> Vec<FORMATETC> {
        self.entries
            .iter()
            .map(|e| FORMATETC {
                cfFormat: e.format,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: e.data.tymed(),
            })
            .collect()
    }

    /// The entry `format` asks for, or the error QueryGetData/GetData report.
    fn find(&self, format: *const FORMATETC) -> Result<&Entry> {
        let format = unsafe { format.as_ref() }.ok_or(Error::from(E_POINTER))?;
        if format.dwAspect != DVASPECT_CONTENT.0 {
            return Err(DV_E_FORMATETC.into());
        }
        let entry = self
            .entries
            .iter()
            .find(|e| e.format == format.cfFormat)
            .ok_or(Error::from(DV_E_FORMATETC))?;
        if format.tymed & entry.data.tymed() == 0 {
            return Err(DV_E_TYMED.into());
        }
        Ok(entry)
    }

    fn emf(&self) -> Result<HENHMETAFILE> {
        self.emf.as_ref().map(|e| e.0).ok_or(DV_E_FORMATETC.into())
    }
}

impl IDataObject_Impl for DataObject_Impl {
    fn GetData(&self, format: *const FORMATETC) -> Result<STGMEDIUM> {
        let entry = self.find(format)?;
        let u = match &entry.data {
            Data::Global(bytes) => STGMEDIUM_0 {
                hGlobal: global(bytes)?,
            },
            Data::Storage(bytes) => STGMEDIUM_0 {
                pstg: ManuallyDrop::new(Some(open_storage(bytes)?)),
            },
            Data::EnhMetafile => {
                let copy = unsafe { CopyEnhMetaFileW(self.emf()?, None) };
                if copy.is_invalid() {
                    return Err(Error::from_thread());
                }
                STGMEDIUM_0 { hEnhMetaFile: copy }
            }
            Data::MetafilePict => STGMEDIUM_0 {
                hMetaFilePict: metafile_pict(self.emf()?)?.0,
            },
        };
        Ok(STGMEDIUM {
            tymed: entry.data.tymed(),
            u,
            pUnkForRelease: ManuallyDrop::new(None),
        })
    }

    /// Office (and OleFlushClipboard) take "Embed Source" this way: into a storage of theirs.
    fn GetDataHere(&self, format: *const FORMATETC, medium: *mut STGMEDIUM) -> Result<()> {
        let entry = self.find(format)?;
        let medium = unsafe { medium.as_ref() }.ok_or(Error::from(E_POINTER))?;
        let Data::Storage(bytes) = &entry.data else {
            return Err(DV_E_FORMATETC.into());
        };
        if medium.tymed != TYMED_ISTORAGE.0 as u32 {
            return Err(DV_E_TYMED.into());
        }
        let dest = unsafe { medium.u.pstg.as_ref() }.ok_or(Error::from(E_INVALIDARG))?;
        let source = open_storage(bytes)?;
        unsafe {
            source.CopyTo(None, None, dest)?;
            dest.SetClass(&CHEMDRAW_CLSID)
        }
    }

    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        match self.find(format) {
            Ok(_) => S_OK,
            Err(e) => e.code(),
        }
    }

    fn GetCanonicalFormatEtc(&self, _in: *const FORMATETC, out: *mut FORMATETC) -> HRESULT {
        if let Some(out) = unsafe { out.as_mut() } {
            out.ptd = std::ptr::null_mut();
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, direction: u32) -> Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        unsafe { SHCreateStdEnumFmtEtc(&self.formats()) }
    }

    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

/// A movable HGLOBAL holding `bytes`, owned by whoever receives it.
pub(super) fn global(bytes: &[u8]) -> Result<HGLOBAL> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let ptr = GlobalLock(handle) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err(Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(handle);
        Ok(handle)
    }
}

/// A compound document's bytes opened as an `IStorage` (over its own copy in memory).
fn open_storage(bytes: &[u8]) -> Result<IStorage> {
    let handle = global(bytes)?;
    let lock_bytes = match unsafe { CreateILockBytesOnHGlobal(Some(handle), true) } {
        Ok(lock_bytes) => lock_bytes,
        Err(e) => {
            unsafe {
                let _ = GlobalFree(Some(handle));
            }
            return Err(e);
        }
    };
    unsafe {
        StgOpenStorageOnILockBytes(
            &lock_bytes,
            None,
            STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
            None,
            None,
        )
    }
}

/// The object's size in an OBJECTDESCRIPTOR (`sizel`, HIMETRIC).
fn descriptor_extent(descriptor: &[u8]) -> Option<(i32, i32)> {
    let field = |at: usize| {
        Some(i32::from_le_bytes(
            descriptor.get(at..at + 4)?.try_into().ok()?,
        ))
    };
    Some((field(24)?, field(28)?)).filter(|&(cx, cy)| cx > 0 && cy > 0)
}

/// A copy of `emf` whose picture is `cx` × `cy` HIMETRIC: the frame and the reference
/// device's physical size are scaled together in the header, so the drawing (kept in device
/// units) still fills the frame. (Replaying it into a new metafile instead gave a picture
/// PowerPoint refused to paste.)
fn with_frame(emf: HENHMETAFILE, cx: i32, cy: i32) -> Option<HENHMETAFILE> {
    // ENHMETAHEADER offsets: rclFrame 24..40, szlMillimeters 80..88, nSize 4..8 and, when the
    // header is that long, szlMicrometers 100..108.
    const FRAME: usize = 24;
    const MILLIMETERS: usize = 80;
    const MICROMETERS: usize = 100;
    let size = unsafe { GetEnhMetaFileBits(emf, None) } as usize;
    let mut bits = vec![0u8; size];
    if size < MILLIMETERS + 8
        || unsafe { GetEnhMetaFileBits(emf, Some(&mut bits)) } as usize != size
    {
        return None;
    }
    let read = |bits: &[u8], at: usize| i32::from_le_bytes(bits[at..at + 4].try_into().unwrap());
    let header_size = read(&bits, 4) as usize;
    let (left, top) = (read(&bits, FRAME), read(&bits, FRAME + 4));
    let (width, height) = (read(&bits, FRAME + 8) - left, read(&bits, FRAME + 12) - top);
    if width <= 0 || height <= 0 {
        return None;
    }
    let scale = |bits: &mut [u8], at: usize, to: i32, from: i32| {
        let value = (i64::from(read(bits, at)) * i64::from(to) / i64::from(from)) as i32;
        bits[at..at + 4].copy_from_slice(&value.to_le_bytes());
    };
    // Every HIMETRIC quantity scales by the same factor, so the frame keeps covering the same
    // device units.
    scale(&mut bits, FRAME, cx, width);
    scale(&mut bits, FRAME + 4, cy, height);
    let (left, top) = (read(&bits, FRAME), read(&bits, FRAME + 4));
    bits[FRAME + 8..FRAME + 12].copy_from_slice(&(left + cx).to_le_bytes());
    bits[FRAME + 12..FRAME + 16].copy_from_slice(&(top + cy).to_le_bytes());
    scale(&mut bits, MILLIMETERS, cx, width);
    scale(&mut bits, MILLIMETERS + 4, cy, height);
    if header_size >= MICROMETERS + 8 {
        scale(&mut bits, MICROMETERS, cx, width);
        scale(&mut bits, MICROMETERS + 4, cy, height);
    }
    let scaled = unsafe { SetEnhMetaFileBits(&bits) };
    (!scaled.is_invalid()).then_some(scaled)
}

/// `emf` as a CF_METAFILEPICT: a Windows metafile in an HGLOBAL'd METAFILEPICT, sized in
/// HIMETRIC from the EMF's frame.
fn metafile_pict(emf: HENHMETAFILE) -> Result<HGLOBAL> {
    unsafe {
        let mut header = ENHMETAHEADER::default();
        if GetEnhMetaFileHeader(emf, size_of::<ENHMETAHEADER>() as u32, Some(&mut header)) == 0 {
            return Err(Error::from_thread());
        }
        let dc = GetDC(None);
        let size = GetWinMetaFileBits(emf, None, MM_ANISOTROPIC.0, dc);
        let mut bits = vec![0u8; size as usize];
        let written = GetWinMetaFileBits(emf, Some(&mut bits), MM_ANISOTROPIC.0, dc);
        ReleaseDC(None, dc);
        if size == 0 || written != size {
            return Err(Error::from_thread());
        }
        let metafile = SetMetaFileBitsEx(&bits);
        if metafile.is_invalid() {
            return Err(Error::from_thread());
        }
        let frame = header.rclFrame; // already in .01 mm
        let pict = METAFILEPICT {
            mm: MM_ANISOTROPIC.0,
            xExt: frame.right - frame.left,
            yExt: frame.bottom - frame.top,
            hMF: metafile,
        };
        let bytes =
            std::slice::from_raw_parts((&raw const pict).cast::<u8>(), size_of::<METAFILEPICT>());
        global(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chembuider_rs::BondOrder;
    use windows::Win32::System::Com::STATSTG;
    use windows::Win32::System::Ole::ReleaseStgMedium;

    /// Benzene, drawn as the editor would (1.5-unit bonds).
    fn benzene() -> Molecule {
        let mut mol = Molecule::default();
        let ids: Vec<u32> = (0..6)
            .map(|i| {
                let a = std::f32::consts::FRAC_PI_3 * i as f32;
                mol.add_atom("C".to_string(), [1.5 * a.cos(), 1.5 * a.sin()], 0)
            })
            .collect();
        for i in 0..6 {
            let order = if i % 2 == 0 {
                BondOrder::Double
            } else {
                BondOrder::Single
            };
            mol.add_bond(ids[i], ids[(i + 1) % 6], order);
        }
        mol
    }

    fn format_of(object: &IDataObject, name: &str) -> FORMATETC {
        let format = unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) } as u16;
        let all: Vec<FORMATETC> = unsafe {
            let e = object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
            let mut out = Vec::new();
            let mut one = [FORMATETC::default()];
            while e.Next(&mut one, None) == S_OK {
                out.push(one[0]);
            }
            out
        };
        *all.iter().find(|f| f.cfFormat == format).unwrap()
    }

    #[test]
    fn offers_embed_source_as_a_chemdraw_storage() {
        unsafe { OleInitialize(None) }.unwrap();
        let object: IDataObject = DataObject::new(&benzene(), "chembuilder-mol:{}").into();
        let format = format_of(&object, "Embed Source");
        assert_eq!(format.tymed, TYMED_ISTORAGE.0 as u32);
        unsafe {
            assert_eq!(object.QueryGetData(&format), S_OK);
            let mut medium = object.GetData(&format).unwrap();
            let storage = medium.u.pstg.as_ref().unwrap();
            let mut stat = STATSTG::default();
            storage.Stat(&mut stat, 1).unwrap(); // STATFLAG_NONAME
            assert_eq!(stat.clsid, CHEMDRAW_CLSID);
            ReleaseStgMedium(&mut medium);
        }
    }

    #[test]
    fn offers_the_formats_paste_back_and_chemdraw_read() {
        unsafe { OleInitialize(None) }.unwrap();
        let object: IDataObject = DataObject::new(&benzene(), "chembuilder-mol:{}").into();
        for name in [
            "ChemBuilder Molecule",
            "ChemDraw Interchange Format",
            "Object Descriptor",
        ] {
            let format = format_of(&object, name);
            assert_eq!(format.tymed, TYMED_HGLOBAL.0 as u32, "{name}");
            unsafe {
                let mut medium = object.GetData(&format).unwrap();
                assert!(!medium.u.hGlobal.is_invalid(), "{name}");
                ReleaseStgMedium(&mut medium);
            }
        }
        // Asking for a medium a format is not offered in is refused.
        let mut wrong = format_of(&object, "ChemBuilder Molecule");
        wrong.tymed = TYMED_ISTORAGE.0 as u32;
        assert_eq!(unsafe { object.QueryGetData(&wrong) }, DV_E_TYMED);
    }

    #[test]
    fn offers_both_metafile_pictures() {
        unsafe { OleInitialize(None) }.unwrap();
        let object: IDataObject = DataObject::new(&benzene(), "chembuilder-mol:{}").into();
        for (format, tymed) in [
            (CF_ENHMETAFILE, TYMED_ENHMF),
            (CF_METAFILEPICT, TYMED_MFPICT),
        ] {
            let request = FORMATETC {
                cfFormat: format,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: tymed.0 as u32,
            };
            unsafe {
                let mut medium = object.GetData(&request).unwrap();
                assert_eq!(medium.tymed, tymed.0 as u32);
                ReleaseStgMedium(&mut medium);
            }
        }
    }

    /// Puts a copy of benzene on the real clipboard for `scripts/e2e/office-paste.ps1`.
    ///   cargo test --lib ole::clipboard::tests::put_benzene_on_clipboard -- --ignored
    #[test]
    #[ignore = "overwrites the system clipboard"]
    fn put_benzene_on_clipboard() {
        let mol = benzene();
        let mut editor = chembuider_rs::ChemStructEditor::default();
        editor.molecule = mol.clone();
        let text = editor.copy_to_string().unwrap();
        set(&mol, &text).unwrap();

        // The widget's paste reads our own format back from the flushed clipboard.
        let mut target = chembuider_rs::ChemStructEditor::default();
        assert!(target.paste_clipboard());
        assert_eq!(target.molecule.atoms.len(), 6);
        assert_eq!(target.molecule.bonds.len(), 6);
    }

    /// Pastes whatever structure is on the clipboard into an empty editor, as Ctrl+V does (e.g.
    /// after copying in ChemDraw), and prints how many atoms arrived.
    ///   cargo test --lib ole::clipboard::tests::paste_live_clipboard -- --ignored --nocapture
    #[test]
    #[ignore = "reads the system clipboard"]
    fn paste_live_clipboard() {
        let mut editor = chembuider_rs::ChemStructEditor::default();
        assert!(editor.paste_clipboard(), "no structure on the clipboard");
        eprintln!(
            "pasted {} atoms, {} bonds",
            editor.molecule.atoms.len(),
            editor.molecule.bonds.len()
        );
    }
}
