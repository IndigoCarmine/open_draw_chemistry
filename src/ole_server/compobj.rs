//! The `\x01CompObj` stream of an OLE object's storage ([MS-OLEDS] 2.3.8): which class wrote
//! it, its user type, the clipboard format of its native data and its ProgID. Containers and
//! conversion tools read it to name the object; ChemDraw writes it with its own class.

use super::{CDX_FORMAT_NAME, CHEMDRAW_CLSID};

/// The ProgID ChemDraw writes into its objects (64-bit ChemDraw, as Office stores it).
pub const CHEMDRAW_PROG_ID: &str = "ChemDraw_x64.Document.6.0";
/// ChemDraw's user type, as in its objects.
pub const CHEMDRAW_USER_TYPE: &str = "CS ChemDraw 64-bit Drawing";
/// This app's class's user type in the stream (ANSI, so not the Japanese one).
pub const OWN_USER_TYPE: &str = "Open DrawChemistry Structure";

/// A GUID's bytes as COM lays them out (`Data1`–`Data3` little-endian, then `Data4`).
pub fn guid_bytes(guid: u128) -> [u8; 16] {
    let b = guid.to_be_bytes();
    [
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15],
    ]
}

/// The stream for an object of class `clsid`. `user_type`, `format` and `prog_id` must be
/// ASCII (they are written as ANSI strings); the Unicode copies are left empty, as ChemDraw does.
pub fn compobj(clsid: u128, user_type: &str, format: &str, prog_id: &str) -> Vec<u8> {
    fn ansi(out: &mut Vec<u8>, s: &str) {
        debug_assert!(s.is_ascii());
        out.extend_from_slice(&(s.len() as u32 + 1).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0xFFFE_0001u32.to_le_bytes()); // Reserved1
    out.extend_from_slice(&0x0000_0A03u32.to_le_bytes()); // Version
    out.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // Reserved2 …
    out.extend_from_slice(&guid_bytes(clsid)); // … and the class
    ansi(&mut out, user_type);
    ansi(&mut out, format);
    ansi(&mut out, prog_id);
    out.extend_from_slice(&0x71B2_39F4u32.to_le_bytes()); // UnicodeMarker
    out.extend_from_slice(&[0; 12]); // empty Unicode user type, format and ProgID
    out
}

/// The stream ChemDraw writes into its objects.
pub fn chemdraw() -> Vec<u8> {
    compobj(
        CHEMDRAW_CLSID,
        CHEMDRAW_USER_TYPE,
        CDX_FORMAT_NAME,
        CHEMDRAW_PROG_ID,
    )
}

/// The stream for an object stored under `clsid`: ChemDraw's own for ChemDraw's class, this
/// app's names for any other.
pub fn for_class(clsid: u128) -> Vec<u8> {
    if clsid == CHEMDRAW_CLSID {
        chemdraw()
    } else {
        compobj(clsid, OWN_USER_TYPE, CDX_FORMAT_NAME, super::PROG_ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ChemDraw's stream as captured from its clipboard (the bytes chembuider-rs's
    /// `molecule_to_ole_embed` writes).
    const CAPTURED: &str = "0100feff030a0000ffffffff216dba412ea0ce118fd90020afd1f20c1b0000004353204368656d447261772036342d6269742044726177696e67001c0000004368656d4472617720496e7465726368616e676520466f726d6174001a0000004368656d447261775f7836342e446f63756d656e742e362e3000f439b271000000000000000000000000";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn chemdraws_stream_is_reproduced_byte_for_byte() {
        assert_eq!(hex(&chemdraw()), CAPTURED);
        assert_eq!(for_class(CHEMDRAW_CLSID), chemdraw());
    }

    #[test]
    fn guid_bytes_follow_com_layout() {
        assert_eq!(
            guid_bytes(CHEMDRAW_CLSID),
            [
                0x21, 0x6d, 0xba, 0x41, 0x2e, 0xa0, 0xce, 0x11, 0x8f, 0xd9, 0x00, 0x20, 0xaf, 0xd1,
                0xf2, 0x0c
            ]
        );
    }

    #[test]
    fn other_classes_get_this_apps_names() {
        let s = for_class(super::super::CLSID);
        assert_eq!(&s[12..28], &guid_bytes(super::super::CLSID));
        let text = String::from_utf8_lossy(&s);
        assert!(text.contains(OWN_USER_TYPE));
        assert!(text.contains(super::super::PROG_ID));
    }
}
