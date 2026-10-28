//! Reading a structure off the OLE clipboard on Windows, including an embedded object copied
//! in Word or PowerPoint.
//!
//! What Office offers when an embedded ChemDraw object is selected and copied (Word's inline
//! shape or PowerPoint's shape, `.Copy()` through their COM automation; see
//! `scripts/e2e/office-copy-paste.ps1` and `tests::dump_live_clipboard`):
//!
//! * Word: "Embedded Object" (listed as `TYMED_ISTORAGE`, but also given as an HGLOBAL or a
//!   stream, and "Embed Source" is answered too though not listed), "Object Descriptor", RTF,
//!   HTML, EMF, METAFILEPICT, PNG, JFIF, GIF, DIB, bitmap, "Link Source" / "Link Source
//!   Descriptor" / "ObjectLink". The storage: ChemDraw's class, `\1Ole`, `\3EPRINT` (the
//!   picture), `\1CompObj`, `\3ObjInfo` and the CDX in `CONTENTS`.
//! * PowerPoint: "Embedded Object" (`TYMED_ISTORAGE`, also given in the other media),
//!   "Object Descriptor", its own internal formats, bitmap, EMF and METAFILEPICT. The storage:
//!   ChemDraw's class, `\1Ole`, `\1CompObj`, `CONTENTS` and `\2OlePres000`.
//!
//! Neither offers "ChemDraw Interchange Format" (Office re-offers the object, not the formats its
//! server published) nor any plain text, so reading `CONTENTS` is what makes such a paste work.
//!
//! The order tried: our own "ChemBuilder Molecule" (full fidelity, left to the widget), the
//! "ChemDraw Interchange Format" CDX, then "Embed Source" / "Embedded Object".

use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::System::Com::StructuredStorage::{
    CreateILockBytesOnHGlobal, IStorage, StgOpenStorageOnILockBytes,
};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IDataObject, IStream, STATFLAG_NONAME, STATSTG, STGM_READ,
    STGM_SHARE_DENY_WRITE, STGM_SHARE_EXCLUSIVE, STGMEDIUM, STREAM_SEEK_SET, TYMED_HGLOBAL,
    TYMED_ISTORAGE, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{OleGetClipboard, ReleaseStgMedium};
use windows::core::{GUID, HSTRING, w};

use crate::ole_server::{CHEMDRAW_CLSID, CLSID, com, guid_string};

/// Why an embedded object on the clipboard could not be read. The [`Display`](std::fmt::Display)
/// text is the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteError {
    /// The object is of another class (e.g. an Excel sheet or a picture).
    WrongClass(String),
    /// The object's storage has no `CONTENTS` stream.
    NoContents,
    /// The object could not be taken from the clipboard or opened.
    Unreadable,
}

impl std::fmt::Display for PasteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasteError::WrongClass(class) => {
                f.write_str(&t!("ole.paste.wrong_class", class = class))
            }
            PasteError::NoContents => f.write_str(t!("ole.paste.no_contents")),
            PasteError::Unreadable => f.write_str(t!("ole.paste.unreadable")),
        }
    }
}

/// The clipboard's data object.
pub struct Clipboard(IDataObject);

impl Clipboard {
    /// The data object on the clipboard now.
    pub fn open() -> windows::core::Result<Self> {
        super::clipboard::ensure_ole()?;
        Ok(Self(unsafe { OleGetClipboard() }?))
    }

    /// Our own "ChemBuilder Molecule" text, for the widget's `paste_from_string`.
    pub fn app_text(&self) -> Option<String> {
        String::from_utf8(trim_nul(self.global(APP_FORMAT)?)).ok()
    }

    /// The "ChemDraw Interchange Format" CDX.
    pub fn cdx(&self) -> Option<Vec<u8>> {
        self.global(CDX_FORMAT)
    }

    /// The CDX in an embedded object ("Embed Source" / "Embedded Object"); `None` when no
    /// object is offered, an error when one is but it is not a readable ChemDraw object.
    pub fn embedded_cdx(&self) -> Option<Result<Vec<u8>, PasteError>> {
        EMBED_FORMATS.iter().find_map(|name| self.embedded(name))
    }

    /// The bytes of `name` as an HGLOBAL, if offered.
    fn global(&self, name: &str) -> Option<Vec<u8>> {
        let mut medium = self.get(name, TYMED_HGLOBAL.0 as u32)?;
        let bytes = (medium.tymed == TYMED_HGLOBAL.0 as u32)
            .then(|| unsafe { global_bytes(medium.u.hGlobal) })
            .flatten();
        unsafe { ReleaseStgMedium(&mut medium) };
        bytes
    }

    /// The CDX of the embedded object offered as `name`; `None` when it is not offered.
    fn embedded(&self, name: &str) -> Option<Result<Vec<u8>, PasteError>> {
        let tymed = (TYMED_ISTORAGE.0 | TYMED_HGLOBAL.0 | TYMED_ISTREAM.0) as u32;
        let format = self.format(name, tymed);
        if unsafe { self.0.QueryGetData(&format) }.is_err() {
            return None;
        }
        let Some(mut medium) = self.get(name, tymed) else {
            return Some(Err(PasteError::Unreadable));
        };
        let storage = unsafe { storage_of(&medium) };
        let result = storage
            .ok_or(PasteError::Unreadable)
            .and_then(|s| structure_from_storage(&s));
        unsafe { ReleaseStgMedium(&mut medium) };
        Some(result)
    }

    fn format(&self, name: &str, tymed: u32) -> FORMATETC {
        FORMATETC {
            cfFormat: registered(name),
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed,
        }
    }

    fn get(&self, name: &str, tymed: u32) -> Option<STGMEDIUM> {
        unsafe { self.0.GetData(&self.format(name, tymed)) }.ok()
    }
}

/// Our own format ("ChemBuilder Molecule"): the widget's clipboard JSON.
const APP_FORMAT: &str = "ChemBuilder Molecule";
/// ChemDraw's CDX format.
const CDX_FORMAT: &str = crate::ole_server::CDX_FORMAT_NAME;
/// An OLE object to embed: "Embed Source" from OLE 2 programs (Office, ChemDraw, this app),
/// "Embedded Object" from some others.
const EMBED_FORMATS: [&str; 2] = ["Embed Source", "Embedded Object"];

fn registered(name: &str) -> u16 {
    unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) as u16 }
}

/// Text in an HGLOBAL usually ends with a NUL (and its block may be rounded up past it).
fn trim_nul(mut bytes: Vec<u8>) -> Vec<u8> {
    if let Some(end) = bytes.iter().position(|&b| b == 0) {
        bytes.truncate(end);
    }
    bytes
}

/// The structure (a CDX) in an embedded object's storage: its `CONTENTS`, if the object is a
/// ChemDraw object (ChemDraw's class or this app's own).
pub fn structure_from_storage(storage: &IStorage) -> Result<Vec<u8>, PasteError> {
    let mut stat = STATSTG::default();
    unsafe { storage.Stat(&mut stat, STATFLAG_NONAME.0 as u32) }
        .map_err(|_| PasteError::Unreadable)?;
    let class = stat.clsid;
    if class != GUID::from_u128(CHEMDRAW_CLSID) && class != GUID::from_u128(CLSID) {
        return Err(PasteError::WrongClass(guid_string(class.to_u128())));
    }
    match unsafe { com::read_stream(storage, w!("CONTENTS")) } {
        Some(cdx) if !cdx.is_empty() => Ok(cdx),
        _ => Err(PasteError::NoContents),
    }
}

/// The storage a medium holds: an `IStorage` as it is, or a compound file in an HGLOBAL or a
/// stream opened as one.
unsafe fn storage_of(medium: &STGMEDIUM) -> Option<IStorage> {
    unsafe {
        if medium.tymed == TYMED_ISTORAGE.0 as u32 {
            return medium.u.pstg.as_ref().cloned();
        }
        let bytes = if medium.tymed == TYMED_HGLOBAL.0 as u32 {
            global_bytes(medium.u.hGlobal)?
        } else if medium.tymed == TYMED_ISTREAM.0 as u32 {
            stream_bytes(medium.u.pstm.as_ref()?)?
        } else {
            return None;
        };
        storage_from_bytes(&bytes)
    }
}

/// A compound file's bytes opened read-only as an `IStorage` (over a copy in memory).
pub fn storage_from_bytes(bytes: &[u8]) -> Option<IStorage> {
    let handle = super::clipboard::global(bytes).ok()?;
    let lock_bytes = match unsafe { CreateILockBytesOnHGlobal(Some(handle), true) } {
        Ok(l) => l,
        Err(_) => {
            unsafe {
                let _ = GlobalFree(Some(handle));
            }
            return None;
        }
    };
    unsafe {
        StgOpenStorageOnILockBytes(
            &lock_bytes,
            None,
            STGM_READ | STGM_SHARE_DENY_WRITE,
            None,
            None,
        )
        .or_else(|_| {
            StgOpenStorageOnILockBytes(
                &lock_bytes,
                None,
                STGM_READ | STGM_SHARE_EXCLUSIVE,
                None,
                None,
            )
        })
        .ok()
    }
}

unsafe fn global_bytes(handle: HGLOBAL) -> Option<Vec<u8>> {
    unsafe {
        let size = GlobalSize(handle);
        let ptr = GlobalLock(handle) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(handle);
        Some(bytes)
    }
}

unsafe fn stream_bytes(stream: &IStream) -> Option<Vec<u8>> {
    unsafe {
        stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            stream
                .Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read))
                .ok()
                .ok()?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        Some(out)
    }
}

/// Every format the clipboard offers, one line each (`name tymed=…`), for investigating what
/// another program put there.
pub fn describe() -> windows::core::Result<Vec<String>> {
    use windows::Win32::System::Com::DATADIR_GET;
    use windows::Win32::System::DataExchange::GetClipboardFormatNameW;
    let object = Clipboard::open()?.0;
    let formats = unsafe { object.EnumFormatEtc(DATADIR_GET.0 as u32) }?;
    let mut out = Vec::new();
    let mut one = [FORMATETC::default()];
    while unsafe { formats.Next(&mut one, None) }.is_ok() && one[0].cfFormat != 0 {
        let f = one[0];
        let mut name = [0u16; 256];
        let n = unsafe { GetClipboardFormatNameW(u32::from(f.cfFormat), &mut name) };
        let name = if n > 0 {
            String::from_utf16_lossy(&name[..n as usize])
        } else {
            format!("CF_{}", f.cfFormat)
        };
        out.push(format!(
            "{name} tymed={:#x} aspect={} lindex={}",
            f.tymed, f.dwAspect, f.lindex
        ));
        one[0] = FORMATETC::default();
    }
    Ok(out)
}

/// A storage's class and elements, one line each, for investigating.
pub fn describe_storage(storage: &IStorage, indent: usize) -> Vec<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    let mut out = Vec::new();
    let mut stat = STATSTG::default();
    if unsafe { storage.Stat(&mut stat, STATFLAG_NONAME.0 as u32) }.is_ok() {
        out.push(format!(
            "{:indent$}class {}",
            "",
            guid_string(stat.clsid.to_u128())
        ));
    }
    let Ok(elements) = (unsafe { storage.EnumElements(None, None, None) }) else {
        return out;
    };
    let mut one = [STATSTG::default()];
    while unsafe { elements.Next(&mut one, None) }.is_ok() && !one[0].pwcsName.is_null() {
        let name = unsafe { one[0].pwcsName.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(one[0].pwcsName.0 as _)) };
        let kind = one[0].r#type;
        out.push(format!(
            "{:indent$}{:?} type={kind} size={}",
            "", name, one[0].cbSize
        ));
        if kind == 1 {
            // STGTY_STORAGE
            if let Ok(child) = unsafe {
                storage.OpenStorage(
                    &HSTRING::from(name.as_str()),
                    None,
                    STGM_READ | STGM_SHARE_EXCLUSIVE,
                    std::ptr::null(),
                    0,
                )
            } {
                out.extend(describe_storage(&child, indent + 2));
            }
        }
        one[0] = STATSTG::default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ole_server::com::{memory_storage, write_object};
    use windows::Win32::System::Ole::OleInitialize;

    /// A CDX of ethanol, as ChemDraw would put in CONTENTS.
    fn ethanol_cdx() -> Vec<u8> {
        let mut mol = chembuider_rs::Molecule::default();
        let a = mol.add_atom("C".into(), [0.0, 0.0], 0);
        let b = mol.add_atom("C".into(), [1.5, 0.0], 0);
        let c = mol.add_atom("O".into(), [3.0, 0.0], 0);
        mol.add_bond(a, b, chembuider_rs::BondOrder::Single);
        mol.add_bond(b, c, chembuider_rs::BondOrder::Single);
        chembuider_rs::molecule::cdx::molecule_to_cdx_bytes(&mol).unwrap()
    }

    fn object(class: u128, contents: Option<&[u8]>) -> IStorage {
        unsafe { OleInitialize(None) }.unwrap();
        let storage = memory_storage().unwrap();
        match contents {
            Some(cdx) => write_object(&storage, &GUID::from_u128(class), cdx).unwrap(),
            None => unsafe {
                windows::Win32::System::Com::StructuredStorage::WriteClassStg(
                    &storage,
                    &GUID::from_u128(class),
                )
                .unwrap()
            },
        }
        storage
    }

    #[test]
    fn reads_contents_of_a_chemdraw_object_and_our_own() {
        let cdx = ethanol_cdx();
        for class in [CHEMDRAW_CLSID, CLSID] {
            let read = structure_from_storage(&object(class, Some(&cdx))).unwrap();
            assert_eq!(read, cdx);
            let loaded = crate::io::decode(crate::io::Format::Cdx, &read).unwrap();
            assert_eq!(loaded.molecule.atoms.len(), 3);
            assert_eq!(loaded.molecule.bonds.len(), 2);
        }
    }

    #[test]
    fn another_class_is_refused_by_name() {
        // Excel.Sheet.12
        let excel = 0x00020830_0000_0000_C000_000000000046;
        let err = structure_from_storage(&object(excel, Some(&ethanol_cdx()))).unwrap_err();
        assert_eq!(
            err,
            PasteError::WrongClass("{00020830-0000-0000-C000-000000000046}".into())
        );
        assert!(
            err.to_string().contains("not a ChemDraw structure"),
            "{err}"
        );
    }

    #[test]
    fn missing_contents_is_reported() {
        let err = structure_from_storage(&object(CHEMDRAW_CLSID, None)).unwrap_err();
        assert_eq!(err, PasteError::NoContents);
        assert!(err.to_string().contains("CONTENTS"));
    }

    #[test]
    fn a_compound_file_in_memory_reads_back() {
        // What "Embed Source" holds once flushed to the Win32 clipboard: the compound file's
        // bytes in an HGLOBAL.
        let cdx = ethanol_cdx();
        let embed = chembuider_rs::molecule::ole::molecule_to_ole_embed(
            &crate::io::decode(crate::io::Format::Cdx, &cdx)
                .unwrap()
                .molecule,
        )
        .unwrap();
        unsafe { OleInitialize(None) }.unwrap();
        let storage = storage_from_bytes(&embed).unwrap();
        let read = structure_from_storage(&storage).unwrap();
        let loaded = crate::io::decode(crate::io::Format::Cdx, &read).unwrap();
        assert_eq!(loaded.molecule.atoms.len(), 3);
        assert!(storage_from_bytes(b"not a compound file").is_none());
    }

    /// Prints every format on the clipboard, and the layout of an embedded object's storage.
    ///   cargo test --lib ole::paste::tests::dump_live_clipboard -- --ignored --nocapture
    #[test]
    #[ignore = "reads the system clipboard"]
    fn dump_live_clipboard() {
        for line in describe().unwrap() {
            println!("format: {line}");
        }
        let clipboard = Clipboard::open().unwrap();
        for name in EMBED_FORMATS {
            for tymed in [TYMED_ISTORAGE, TYMED_HGLOBAL, TYMED_ISTREAM] {
                let Some(mut medium) = clipboard.get(name, tymed.0 as u32) else {
                    println!("{name} as tymed {}: not given", tymed.0);
                    continue;
                };
                println!("{name} as tymed {}: given", tymed.0);
                if let Some(storage) = unsafe { storage_of(&medium) } {
                    for line in describe_storage(&storage, 2) {
                        println!("{line}");
                    }
                }
                unsafe { ReleaseStgMedium(&mut medium) };
            }
        }
        println!(
            "app text: {:?} bytes",
            clipboard.app_text().map(|t| t.len())
        );
        println!("cdx: {:?} bytes", clipboard.cdx().map(|c| c.len()));
        match clipboard.embedded_cdx() {
            None => println!("embedded: none"),
            Some(Err(e)) => println!("embedded: {e}"),
            Some(Ok(c)) => match crate::io::decode(crate::io::Format::Cdx, &c) {
                Ok(l) => println!(
                    "embedded: cdx {} bytes, {} atoms, {} bonds, {} objects",
                    c.len(),
                    l.molecule.atoms.len(),
                    l.molecule.bonds.len(),
                    l.molecule.objects.len()
                ),
                Err(e) => println!("embedded: cdx {} bytes unreadable: {e}", c.len()),
            },
        }
    }
}
