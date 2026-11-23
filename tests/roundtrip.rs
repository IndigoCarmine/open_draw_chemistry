//! Open → save → open on files made by ChemDraw: the structures, the arrows and texts the
//! editor shows, and the objects it cannot show yet (schemes, shapes, …) must all survive, in
//! either format.

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

/// Arrows and texts as (kind or text, first point), rounded to 0.01 units, for comparing.
fn page_objects(mol: &chembuider_rs::Molecule) -> Vec<(String, [i32; 2])> {
    use chembuider_rs::molecule::PageObject;
    let r = |p: [f32; 2]| [(p[0] * 100.0).round() as i32, (p[1] * 100.0).round() as i32];
    mol.objects
        .iter()
        .map(|o| match o {
            PageObject::Arrow(a) => (format!("{:?}", a.kind), r(a.tail)),
            PageObject::Text(t) => (t.text.clone(), r(t.pos)),
            PageObject::Table(t) => ("Table".to_owned(), r(t.pos)),
            PageObject::Bracket(b) => (format!("Bracket {}", b.label), [0, 0]),
            PageObject::Curly(c) => (format!("Curly {:?}", c.kind), [0, 0]),
        })
        .collect()
}

#[test]
fn a_reaction_opens_with_its_arrow_and_texts_editable() {
    // The summary is checked in Japanese.
    let _lang = open_draw_chemistry::i18n::ScopedLanguage::new(open_draw_chemistry::i18n::Lang::Ja);
    for name in ["Reaction.cdx", "Reaction.cdxml"] {
        let loaded = io::load(&data(name)).unwrap();
        let objects = page_objects(&loaded.molecule);
        let kinds: Vec<&str> = objects.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["(I)", "(I)", "Reaction"], "{name}");
        // The reaction scheme and the two chemical properties stay unshown.
        assert_eq!(loaded.other_objects, 3, "{name}");
        assert_eq!(
            loaded.summary(),
            "矢印 1 本・テキスト 2 個を編集できます。表示されないオブジェクト 3 個は保存時に保持されます",
            "{name}"
        );
    }
}

#[test]
fn a_moved_arrow_is_saved_moved_and_a_deleted_one_is_gone() {
    use chembuider_rs::molecule::PageObject;
    for name in ["Reaction.cdx", "Reaction.cdxml"] {
        let loaded = io::load(&data(name)).unwrap();
        for format in Format::ALL {
            let mut mol = loaded.molecule.clone();
            let arrow = mol
                .objects
                .iter_mut()
                .find(|o| o.as_arrow().is_some())
                .unwrap();
            arrow.translate([0.0, 2.0]);
            let bytes = io::encode(format, &mol, Some(&loaded.original)).unwrap();
            let back = io::decode(format, &bytes).unwrap();
            assert_eq!(
                page_objects(&back.molecule),
                page_objects(&mol),
                "{name} → {format:?}"
            );

            let id = mol
                .objects
                .iter()
                .find(|o| o.as_arrow().is_some())
                .map(PageObject::id);
            mol.remove_object(id.unwrap());
            let bytes = io::encode(format, &mol, Some(&loaded.original)).unwrap();
            let back = io::decode(format, &bytes).unwrap();
            assert!(
                back.molecule.objects.iter().all(|o| o.as_arrow().is_none()),
                "{name} → {format:?}: the deleted arrow is not written"
            );
            assert_eq!(back.other_objects, loaded.other_objects);
        }
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
            assert_eq!(
                page_objects(&back.molecule),
                page_objects(&loaded.molecule),
                "{name} → {format:?}: arrows and texts, where they were"
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

/// A curly arrow drawn on a structure read from ChemDraw is saved into the document in either
/// format and comes back on the same atoms and bond, bending the same way, with the summary
/// counting it.
#[test]
fn a_curly_arrow_drawn_on_a_chemdraw_structure_is_saved_and_read_back() {
    use chembuider_rs::molecule::{CurlyArrow, CurlyEnd, CurlyKind};
    let _lang = open_draw_chemistry::i18n::ScopedLanguage::new(open_draw_chemistry::i18n::Lang::En);
    let loaded = io::load(&data("chemdraw26_benzene.cdx")).unwrap();
    let mut mol = loaded.molecule.clone();
    let bond = mol.bonds[0].clone();
    let other = mol
        .atoms
        .iter()
        .find(|a| a.id != bond.begin && a.id != bond.end)
        .unwrap()
        .id;
    let arrow = CurlyArrow {
        skew: 0.2,
        ..CurlyArrow::new(
            CurlyEnd::Bond {
                bond: bond.id,
                side: bond.end,
            },
            CurlyEnd::Atom(other),
            -110.0,
            CurlyKind::Single,
        )
    };
    mol.add_curly_arrow(arrow.clone());
    let pos = |m: &chembuider_rs::Molecule, id: u32| {
        let p = m.atom_by_id(id).unwrap().pos;
        [(p[0] * 100.0).round() as i32, (p[1] * 100.0).round() as i32]
    };
    for format in Format::ALL {
        let bytes = io::encode(format, &mol, Some(&loaded.original)).unwrap();
        let back = io::decode(format, &bytes).unwrap();
        let read: Vec<&CurlyArrow> = back.molecule.curly_arrows().collect();
        assert_eq!(read.len(), 1, "{format:?}");
        let c = read[0];
        let CurlyEnd::Bond { bond: b, side } = c.tail else {
            panic!("{format:?}: {c:?}")
        };
        let b = back.molecule.bond_by_id(b).unwrap();
        let mut ends = [pos(&back.molecule, b.begin), pos(&back.molecule, b.end)];
        let mut expected = [pos(&mol, bond.begin), pos(&mol, bond.end)];
        ends.sort();
        expected.sort();
        assert_eq!(ends, expected, "{format:?}: the same bond");
        assert_eq!(pos(&back.molecule, side), pos(&mol, bond.end), "{format:?}");
        let CurlyEnd::Atom(head) = c.head else {
            panic!("{format:?}: {c:?}")
        };
        assert_eq!(pos(&back.molecule, head), pos(&mol, other), "{format:?}");
        assert!(
            (c.angle - arrow.angle).abs() < 0.05 && (c.skew - arrow.skew).abs() < 1e-4,
            "{format:?}: {c:?}"
        );
        assert_eq!(c.kind, CurlyKind::Single);
        assert_eq!(back.other_objects, loaded.other_objects, "{format:?}");
        assert!(
            back.summary().contains("1 curly arrow"),
            "{}",
            back.summary()
        );
    }
}
