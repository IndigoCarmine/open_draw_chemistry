//! The executable as AcaudaOffice's editor, over a real local socket (see `src/acauda.rs`).

use std::path::Path;
use std::time::Duration;

use acauda_embed::{
    HandlerManifest, HandlerMessage, HostMessage, HostSession, Mode, OleContent, find_handler,
};
use open_draw_chemistry::acauda::{APP_ID, CONTENTS};
use open_draw_chemistry::io::{self, Format};

const EXE: &str = env!("CARGO_BIN_EXE_open_draw_chemistry");
const WAIT: Duration = Duration::from_secs(60);

fn request(request: &HostMessage) -> HandlerMessage {
    let no_args: [&str; 0] = [];
    let mut session = HostSession::spawn(EXE, &no_args, Mode::Headless, WAIT).unwrap();
    let answer = session.request(request).unwrap();
    let status = session.process().unwrap().wait_timeout(WAIT);
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
    answer
}

fn benzene() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/chemdraw26_benzene.cdx");
    std::fs::read(path).unwrap()
}

fn atoms(content: &OleContent) -> usize {
    let cdx = content.stream(CONTENTS).unwrap();
    io::decode(Format::Cdx, cdx).unwrap().molecule.atoms.len()
}

#[test]
fn the_sample_manifest_finds_every_chemdraw_object() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("installer/acauda/open-draw-chemistry.toml");
    let manifest = HandlerManifest::load(&path).unwrap();
    assert_eq!(manifest.id, APP_ID);
    let handlers = [manifest];
    for prog_id in [
        "ChemDraw.Document.6.0",
        "ChemDraw_x64.Document.6.0",
        "OpenDrawChemistry.Document.1",
    ] {
        assert!(find_handler(&handlers, prog_id).is_some(), "{prog_id}");
    }
    assert!(find_handler(&handlers, "Excel.Sheet.12").is_none());
    assert!(handlers[0].new_object);
    assert!(handlers[0].can_import("cdxml") && handlers[0].can_export("CDX"));
}

#[test]
fn a_new_object_comes_from_the_executable() {
    let HandlerMessage::Created {
        content,
        preview,
        size_emu,
    } = request(&HostMessage::New)
    else {
        panic!("not created")
    };
    assert_eq!(content.prog_id, "ChemDraw_x64.Document.6.0");
    assert_eq!(atoms(&content), 0);
    assert!(!preview.is_empty());
    assert!(size_emu.is_some());
}

#[test]
fn a_file_goes_in_is_drawn_and_comes_out() {
    let cdx = benzene();
    let HandlerMessage::Created { content, .. } = request(&HostMessage::Import {
        extension: "cdx".into(),
        data: cdx.clone(),
    }) else {
        panic!("not imported")
    };
    assert_eq!(content.stream(CONTENTS), Some(&cdx[..]));

    let HandlerMessage::Rendered { preview, size_emu } = request(&HostMessage::Render {
        content: content.clone(),
    }) else {
        panic!("not drawn")
    };
    assert!(!preview.is_empty());
    assert!(size_emu.is_some_and(|(w, h)| w > 0 && h > 0));

    let HandlerMessage::Exported { data } = request(&HostMessage::Export {
        content,
        extension: "cdxml".into(),
    }) else {
        panic!("not exported")
    };
    assert!(String::from_utf8_lossy(&data).contains("<CDXML"));
}

#[test]
fn what_it_cannot_do_is_said_not_failed() {
    let answer = request(&HostMessage::Import {
        extension: "cdx".into(),
        data: b"junk".to_vec(),
    });
    assert!(matches!(answer, HandlerMessage::Error { .. }), "{answer:?}");
}

/// The whole round: the object opens in a window, the script edits and saves it, and closing
/// its tab ends the process. Run on a desktop:
///   cargo test --test acauda_embed -- --ignored
#[test]
#[ignore = "opens a window"]
fn an_object_is_edited_in_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.txt");
    std::fs::write(&script, "add_atom N 2 0\nsave\nclose\n").unwrap();
    let args = ["--ole-test-script".to_owned(), script.display().to_string()];
    let mut session = HostSession::spawn(EXE, &args, Mode::Interactive, WAIT).unwrap();

    let HandlerMessage::Created { content, .. } = request(&HostMessage::Import {
        extension: "cdx".into(),
        data: benzene(),
    }) else {
        panic!("not imported")
    };
    let before = atoms(&content);
    session
        .send(&HostMessage::Open {
            content,
            title: "doc.pptx".into(),
            locale: "en".into(),
        })
        .unwrap();

    let HandlerMessage::Update {
        content,
        preview,
        size_emu,
    } = session.recv().unwrap()
    else {
        panic!("no update")
    };
    assert_eq!(atoms(&content), before + 1);
    assert_eq!(content.prog_id, "ChemDraw_x64.Document.6.0");
    assert!(!preview.is_empty());
    assert!(size_emu.is_some());
    assert_eq!(session.recv().unwrap(), HandlerMessage::Closed);
    let status = session.process().unwrap().wait_timeout(WAIT);
    assert!(status.is_some_and(|s| s.success()), "{status:?}");
}
