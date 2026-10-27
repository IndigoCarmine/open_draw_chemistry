//! Opening 3D structure files (XYZ, MOL2, PDB, SDF): the file's own coordinates become the 3D
//! model, and the editor gets a clean 2D depiction without hydrogens on carbon.

use std::path::{Path, PathBuf};

use open_draw_chemistry::io::{self, Kind};
use open_draw_chemistry::three_d::read::{self, Format3d};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

#[test]
fn extensions_route_to_the_3d_reader() {
    for (name, format) in [
        ("a.mol2", Format3d::Mol2),
        ("a.PDB", Format3d::Pdb),
        ("a.xyz", Format3d::Xyz),
        ("a.mol", Format3d::Mol),
        ("a.sdf", Format3d::Sdf),
    ] {
        assert_eq!(
            Kind::from_path(Path::new(name)),
            Some(Kind::Structure3d(format)),
            "{name}"
        );
    }
    assert_eq!(
        Kind::from_path(Path::new("a.cdx")),
        Some(Kind::Document(io::Format::Cdx))
    );
    assert!(matches!(
        io::load_3d(Path::new("a.cdx")),
        Err(io::Error::UnknownFormat)
    ));
}

#[cfg(not(feature = "openbabel"))]
#[test]
fn without_openbabel_3d_files_say_what_is_missing() {
    let err = io::load_3d(&data("ethanol.xyz")).unwrap_err();
    assert!(matches!(
        err,
        io::Error::Structure3d(read::Error::Unavailable)
    ));
    assert!(err.to_string().contains("OpenBabel が必要"), "{err}");
}

#[cfg(feature = "openbabel")]
mod with_openbabel {
    use super::*;

    use chembuider_rs::{BondOrder, Config, Molecule};
    use open_draw_chemistry::document::Document;
    use open_draw_chemistry::three_d::Model3d;
    use open_draw_chemistry::three_d::read::Opened3d;

    /// A file with `text` in a fresh temporary directory.
    fn temp_file(name: &str, text: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("odc-open3d-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    fn open(name: &str) -> Opened3d {
        io::load_3d(&data(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn model(opened: &Opened3d) -> &Model3d {
        opened.model.as_ref().expect("a 3D file has a model")
    }

    /// The coordinates in columns `cols` of the lines `select` picks out of fixture `name`.
    fn fixture_coords(
        name: &str,
        select: impl Fn(&str) -> bool,
        cols: [usize; 3],
    ) -> Vec<[f64; 3]> {
        std::fs::read_to_string(data(name))
            .unwrap()
            .lines()
            .filter(|l| select(l))
            .map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                cols.map(|c| f[c].parse().unwrap())
            })
            .collect()
    }

    fn assert_coords_preserved(model: &Model3d, expected: &[[f64; 3]]) {
        assert_eq!(model.atoms.len(), expected.len());
        for (atom, want) in model.atoms.iter().zip(expected) {
            for k in 0..3 {
                assert!(
                    (atom.pos[k] - want[k]).abs() < 1e-3,
                    "{} at {:?}, file says {want:?}",
                    atom.element,
                    atom.pos
                );
            }
        }
    }

    /// No hydrogen at all (all of them sit on C, N or O in these fixtures), and every bond
    /// close to the editor's 1.5 units.
    fn assert_clean_drawing(drawing: &Molecule) {
        assert!(
            drawing.atoms.iter().all(|a| a.element != "H"),
            "hydrogens left in 2D: {:?}",
            drawing.atoms
        );
        for bond in &drawing.bonds {
            let a = drawing.atoms.iter().find(|a| a.id == bond.begin).unwrap();
            let b = drawing.atoms.iter().find(|a| a.id == bond.end).unwrap();
            let len = ((a.pos[0] - b.pos[0]).powi(2) + (a.pos[1] - b.pos[1]).powi(2)).sqrt();
            assert!((1.0..=2.0).contains(&len), "2D bond length {len}");
        }
    }

    fn orders(drawing: &Molecule, order: BondOrder) -> usize {
        drawing.bonds.iter().filter(|b| b.order == order).count()
    }

    #[test]
    fn xyz_gets_its_bonds_perceived_and_keeps_its_coordinates() {
        let opened = open("ethanol.xyz");
        let model = model(&opened);
        assert_eq!(model.atoms.len(), 9);
        assert_eq!(model.hydrogen_count(), 6, "explicit hydrogens kept in 3D");
        assert_eq!(model.bonds.len(), 8, "bonds perceived from distances");
        assert_eq!(model.formula, "C2H6O");
        let expected = fixture_coords("ethanol.xyz", |l| l.len() > 30, [1, 2, 3]);
        assert_coords_preserved(model, &expected);

        let drawing = &opened.drawing;
        assert_eq!(drawing.atoms.len(), 3);
        assert_eq!(drawing.bonds.len(), 2);
        assert_clean_drawing(drawing);
        assert_eq!(opened.records, 1);
        assert!(!opened.projected);
    }

    #[test]
    fn aromatic_mol2_is_drawn_kekule() {
        let opened = open("benzene.mol2");
        let model = model(&opened);
        assert_eq!(model.atoms.len(), 12);
        assert_eq!(model.bonds.len(), 12);
        let expected = fixture_coords("benzene.mol2", |l| l.contains(" BNZ "), [2, 3, 4]);
        assert_coords_preserved(model, &expected);

        let drawing = &opened.drawing;
        assert_eq!(drawing.atoms.len(), 6);
        assert_eq!(drawing.bonds.len(), 6);
        assert_clean_drawing(drawing);
        assert_eq!(orders(drawing, BondOrder::Double), 3);
        assert_eq!(orders(drawing, BondOrder::Single), 3);
        // Alternating: every carbon has exactly one double bond.
        for atom in &drawing.atoms {
            let doubles = drawing
                .bonds
                .iter()
                .filter(|b| {
                    b.order == BondOrder::Double && (b.begin == atom.id || b.end == atom.id)
                })
                .count();
            assert_eq!(doubles, 1);
        }
    }

    #[test]
    fn pdb_without_conect_gets_bonds_and_orders() {
        let opened = open("acetic_acid.pdb");
        let model = model(&opened);
        assert_eq!(model.atoms.len(), 8);
        assert_eq!(model.bonds.len(), 7);
        let expected = fixture_coords("acetic_acid.pdb", |l| l.starts_with("HETATM"), [6, 7, 8]);
        assert_coords_preserved(model, &expected);

        let drawing = &opened.drawing;
        assert_eq!(
            drawing.atoms.len(),
            4,
            "C, C, O, O; the hydroxyl H is implicit"
        );
        assert_eq!(drawing.bonds.len(), 3);
        assert_eq!(orders(drawing, BondOrder::Double), 1, "C=O");
        assert_clean_drawing(drawing);
    }

    #[test]
    fn sdf_reads_the_first_record_with_its_charge() {
        let opened = open("methylammonium_water.sdf");
        assert_eq!(opened.records, 2);
        let model = model(&opened);
        assert_eq!(model.atoms.len(), 8, "methylammonium only, not the water");
        assert_eq!(model.hydrogen_count(), 6);

        let drawing = &opened.drawing;
        assert_eq!(drawing.atoms.len(), 2);
        let n = drawing.atoms.iter().find(|a| a.element == "N").unwrap();
        assert_eq!(n.charge, 1);
        assert_clean_drawing(drawing);
    }

    #[test]
    fn a_2d_molfile_keeps_its_layout_and_has_no_model() {
        let text = "\
ethanol 2D
  handmade10282612002D

  3  2  0  0  0  0  0  0  0  0999 V2000
    0.0000    0.0000    0.0000 C   0  0  0  0  0  0  0  0  0  0  0  0
    0.8660    0.5000    0.0000 C   0  0  0  0  0  0  0  0  0  0  0  0
    1.7320    0.0000    0.0000 O   0  0  0  0  0  0  0  0  0  0  0  0
  1  2  1  0  0  0  0
  2  3  1  0  0  0  0
M  END
";
        let opened = read::read(Format3d::Mol, text).unwrap();
        assert!(opened.model.is_none());
        let drawing = &opened.drawing;
        assert_eq!(drawing.atoms.len(), 3);
        assert_clean_drawing(drawing);
        // The file's own zigzag, flipped to y down: the middle carbon is the highest point.
        assert!(drawing.atoms[1].pos[1] < drawing.atoms[0].pos[1]);
    }

    #[test]
    fn bad_files_are_reported() {
        let (dir, empty) = temp_file("empty.xyz", "  \n");
        assert!(matches!(
            io::load_3d(&empty),
            Err(io::Error::Structure3d(read::Error::Empty))
        ));
        std::fs::remove_dir_all(dir).ok();

        let (dir, garbage) = temp_file("garbage.mol2", "this is not a mol2 file\n");
        let err = io::load_3d(&garbage).unwrap_err();
        assert!(
            matches!(
                err,
                io::Error::Structure3d(read::Error::Unreadable(_) | read::Error::Empty)
            ),
            "{err:?}"
        );
        std::fs::remove_dir_all(dir).ok();

        let mut big = format!("{}\nmany\n", read::MAX_ATOMS + 1);
        for i in 0..=read::MAX_ATOMS {
            big += &format!("C {}.0 0.0 0.0\n", i * 2);
        }
        let (dir, big) = temp_file("big.xyz", &big);
        let err = io::load_3d(&big).unwrap_err();
        assert!(
            matches!(err, io::Error::Structure3d(read::Error::TooManyAtoms(_))),
            "{err:?}"
        );
        assert!(err.to_string().contains("原子が多すぎます"));
        std::fs::remove_dir_all(dir).ok();

        assert!(matches!(
            io::load_3d(Path::new("does/not/exist.pdb")),
            Err(io::Error::Io(_))
        ));
    }

    #[test]
    fn an_opened_3d_file_is_a_document_with_a_current_model_saved_elsewhere() {
        let path = data("ethanol.xyz");
        let opened = io::load_3d(&path).unwrap();
        let mut doc = Document::from_structure_3d(&Config::default(), &path, Format3d::Xyz, opened);
        assert_eq!(doc.title(), "ethanol.xyz");
        assert!(!doc.is_dirty());
        let model = doc.three_d.model().expect("the file's model");
        assert_eq!(model.atoms.len(), 9);
        assert!(!doc.three_d.is_stale(&doc.editor.molecule));
        // Saving never writes CDX over the XYZ file: it asks for a new name.
        assert!(!doc.save().unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().lines().next(),
            Some("9")
        );
    }
}
