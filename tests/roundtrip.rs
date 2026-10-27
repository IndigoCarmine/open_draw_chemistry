//! Open → save → open on files made by ChemDraw: the structures and the objects the editor
//! cannot show yet (arrows, captions, …) must both survive, in either format.

use std::path::{Path, PathBuf};

use open_draw_chemistry::io::{self, Format};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

const FILES: [&str; 5] = [
    "chemdraw26_benzene.cdx",
    "chemdraw26_charge_stereo.cdx",
    "Reaction.cdx",
    "Reaction.cdxml",
    "scheme_two_rows.cdxml",
];

#[test]
fn chemdraw_files_open_with_their_structures() {
    for name in FILES {
        let loaded = io::load(&data(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!loaded.molecule.atoms.is_empty(), "{name}: no atoms read");
    }
}

#[test]
fn a_reaction_keeps_its_arrow_and_text_through_a_save() {
    for name in ["Reaction.cdx", "Reaction.cdxml"] {
        let loaded = io::load(&data(name)).unwrap();
        assert!(
            loaded.other_objects > 0,
            "{name}: expected arrows/text besides the structures"
        );
    }
}

#[test]
fn saving_in_either_format_keeps_structures_and_other_objects() {
    for name in FILES {
        let loaded = io::load(&data(name)).unwrap();
        for format in Format::ALL {
            let bytes = io::encode(format, &loaded.molecule, Some(&loaded.original))
                .unwrap_or_else(|e| panic!("{name} → {format:?}: {e}"));
            let back = io::decode(format, &bytes)
                .unwrap_or_else(|e| panic!("{name} → {format:?} → back: {e}"));
            assert_eq!(
                back.molecule.atoms.len(),
                loaded.molecule.atoms.len(),
                "{name} → {format:?}: atoms"
            );
            assert_eq!(
                back.molecule.bonds.len(),
                loaded.molecule.bonds.len(),
                "{name} → {format:?}: bonds"
            );
            assert_eq!(
                back.other_objects, loaded.other_objects,
                "{name} → {format:?}: objects other than structures"
            );
        }
    }
}

#[test]
fn an_edit_is_what_gets_saved() {
    let loaded = io::load(&data("chemdraw26_benzene.cdx")).unwrap();
    let mut mol = loaded.molecule.clone();
    let any = mol.atoms[0].id;
    let n = mol.add_atom("N".to_owned(), [10.0, 10.0], 0);
    mol.add_bond(any, n, chembuider_rs::BondOrder::Single);

    let bytes = io::encode(Format::Cdx, &mol, Some(&loaded.original)).unwrap();
    let back = io::decode(Format::Cdx, &bytes).unwrap();
    assert_eq!(back.molecule.atoms.len(), loaded.molecule.atoms.len() + 1);
    assert!(back.molecule.atoms.iter().any(|a| a.element == "N"));
}
