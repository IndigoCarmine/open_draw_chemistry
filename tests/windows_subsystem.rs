//! The executable is a Windows GUI program in every build: a console program that COM starts as
//! the OLE server (`-Embedding`) gets a black console window of its own next to the editor.
#![cfg(windows)]

const EXE: &str = env!("CARGO_BIN_EXE_open_draw_chemistry");

/// `IMAGE_SUBSYSTEM_WINDOWS_GUI`.
const WINDOWS_GUI: u16 = 2;

/// The `Subsystem` field of a PE image's optional header.
fn subsystem(image: &[u8]) -> u16 {
    let at = |offset: usize, len: usize| &image[offset..offset + len];
    assert_eq!(at(0, 2), b"MZ");
    let pe = u32::from_le_bytes(at(0x3c, 4).try_into().unwrap()) as usize;
    assert_eq!(at(pe, 4), b"PE\0\0");
    // The optional header follows the 4-byte signature and the 20-byte file header; its
    // Subsystem field is at offset 68 in both PE32 and PE32+.
    u16::from_le_bytes(at(pe + 24 + 68, 2).try_into().unwrap())
}

#[test]
fn the_executable_opens_no_console_window() {
    let image = std::fs::read(EXE).unwrap();
    assert_eq!(subsystem(&image), WINDOWS_GUI);
}
