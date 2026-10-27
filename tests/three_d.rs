//! 2D → 3D through OpenBabel: drawn and ChemDraw-made structures come out as sane 3D models,
//! and a model survives being exported and read back.
#![cfg(feature = "openbabel")]

use std::path::Path;

use chembuider_rs::{BondOrder, BondStereo, Config, Molecule};
use open_draw_chemistry::io;
use open_draw_chemistry::three_d::read::Format3d;
use open_draw_chemistry::three_d::{self, Export3d, ForceField, Model3d};

/// Bonded pairs of carbons in `model`, with their lengths.
fn carbon_carbon_bonds(model: &Model3d) -> Vec<f64> {
    model
        .bonds
        .iter()
        .filter(|b| model.atoms[b.begin].element == "C" && model.atoms[b.end].element == "C")
        .map(|b| model.distance(b.begin, b.end))
        .collect()
}

fn ethanol() -> Molecule {
    let mut mol = Molecule::default();
    let c1 = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
    let c2 = mol.add_atom("C".to_owned(), [1.3, 0.75], 0);
    let o = mol.add_atom("O".to_owned(), [2.6, 0.0], 0);
    mol.add_bond(c1, c2, BondOrder::Single);
    mol.add_bond(c2, o, BondOrder::Single);
    mol
}

#[test]
fn ethanol_gets_its_hydrogens_and_a_relaxed_geometry() {
    let model = three_d::generate(&ethanol(), &Config::default(), ForceField::Mmff94).unwrap();
    assert_eq!(model.formula, "C2H6O");
    assert_eq!(model.atoms.len(), 9);
    assert_eq!(model.hydrogen_count(), 6);
    assert!(model.energy.is_finite(), "energy {}", model.energy);
    assert_eq!(model.energy_unit, "kcal/mol");
    let cc = carbon_carbon_bonds(&model);
    assert_eq!(cc.len(), 1);
    assert!((1.45..=1.6).contains(&cc[0]), "C–C {} Å", cc[0]);
    // A real third dimension, not the flat drawing.
    assert!(model.atoms.iter().any(|a| a.pos[2].abs() > 0.3));
}

#[test]
fn chemdraw_benzene_becomes_a_flat_hexagon_with_six_hydrogens() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/chemdraw26_benzene.cdx");
    let drawing = io::load(&path).unwrap().molecule;
    let model = three_d::generate(&drawing, &Config::default(), ForceField::Mmff94).unwrap();
    assert_eq!(model.formula, "C6H6");
    assert_eq!(model.hydrogen_count(), 6);
    assert!(model.energy.is_finite());
    let cc = carbon_carbon_bonds(&model);
    assert_eq!(cc.len(), 6);
    for d in cc {
        assert!((1.36..=1.45).contains(&d), "aromatic C–C {d} Å");
    }
}

/// CHFClBr with F on a wedge: a solid or a hashed one.
fn bromochlorofluoromethane(stereo: BondStereo) -> Molecule {
    let mut mol = Molecule::default();
    let c = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
    let f = mol.add_atom("F".to_owned(), [1.5, 0.0], 0);
    let cl = mol.add_atom("Cl".to_owned(), [-0.75, 1.3], 0);
    let br = mol.add_atom("Br".to_owned(), [-0.75, -1.3], 0);
    let wedge = mol.add_bond(c, f, BondOrder::Single);
    mol.add_bond(c, cl, BondOrder::Single);
    mol.add_bond(c, br, BondOrder::Single);
    mol.bonds.iter_mut().find(|b| b.id == wedge).unwrap().stereo = stereo;
    mol
}

/// The signed volume of F, Cl and Br about the carbon: its sign is the centre's handedness.
fn handedness(model: &Model3d) -> f64 {
    let at = |i: usize| {
        let (p, c) = (model.atoms[i].pos, model.atoms[0].pos);
        [p[0] - c[0], p[1] - c[1], p[2] - c[2]]
    };
    let (f, cl, br) = (at(1), at(2), at(3));
    f[0] * (cl[1] * br[2] - cl[2] * br[1]) - f[1] * (cl[0] * br[2] - cl[2] * br[0])
        + f[2] * (cl[0] * br[1] - cl[1] * br[0])
}

#[test]
fn wedges_decide_the_enantiomer() {
    let model = |stereo| {
        three_d::generate(
            &bromochlorofluoromethane(stereo),
            &Config::default(),
            ForceField::Mmff94,
        )
        .unwrap()
    };
    let (up, down) = (model(BondStereo::WedgeUp), model(BondStereo::WedgeDown));
    assert_eq!(
        ["C", "F", "Cl", "Br"],
        [0, 1, 2, 3].map(|i| up.atoms[i].element.as_str())
    );
    // On screen (y down) F points right, Cl down-left and Br up-left. A solid wedge lifts F
    // towards the viewer, which with y flipped to point up makes the volume negative.
    assert!(handedness(&up) < -0.5, "wedge: {}", handedness(&up));
    assert!(handedness(&down) > 0.5, "hash: {}", handedness(&down));
}

#[test]
fn every_force_field_relaxes_ethanol() {
    for ff in ForceField::ALL {
        let model = three_d::generate(&ethanol(), &Config::default(), ff)
            .unwrap_or_else(|e| panic!("{ff}: {e}"));
        assert!(model.energy.is_finite(), "{ff}: energy {}", model.energy);
        assert_eq!(model.force_field, ff);
    }
}

#[test]
fn optimising_does_not_raise_the_energy() {
    let model = three_d::generate(&ethanol(), &Config::default(), ForceField::Uff).unwrap();
    let optimized = three_d::optimize(&model, ForceField::Uff).unwrap();
    assert_eq!(optimized.atoms.len(), model.atoms.len());
    assert!(
        optimized.energy <= model.energy + 1e-3,
        "{} → {}",
        model.energy,
        optimized.energy
    );
}

#[test]
fn exports_read_back_with_every_atom() {
    let model = three_d::generate(&ethanol(), &Config::default(), ForceField::Mmff94).unwrap();
    for format in Export3d::ALL {
        let text = three_d::export(&model, format).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        let back = openbabel::Molecule::parse(&text, format.extension())
            .unwrap_or_else(|e| panic!("{format:?} back: {e}"));
        assert_eq!(
            back.num_atoms() as usize,
            model.atoms.len(),
            "{format:?} atoms"
        );
        assert!(back.has_3d(), "{format:?} keeps the 3D coordinates");
    }
}

#[test]
fn mol2_export_reads_back_with_the_same_coordinates() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/chemdraw26_benzene.cdx");
    let benzene = io::load(&path).unwrap().molecule;
    for drawing in [ethanol(), benzene] {
        let model = three_d::generate(&drawing, &Config::default(), ForceField::Mmff94).unwrap();
        let text = three_d::export(&model, Export3d::Mol2).unwrap();
        assert!(text.contains("@<TRIPOS>BOND"), "{text}");
        let back = three_d::read::read(Format3d::Mol2, &text)
            .unwrap()
            .model
            .unwrap();
        assert_eq!(back.atoms.len(), model.atoms.len());
        for (a, b) in model.atoms.iter().zip(&back.atoms) {
            assert_eq!(a.element, b.element);
            for k in 0..3 {
                assert!((a.pos[k] - b.pos[k]).abs() < 1e-3, "{a:?} vs {b:?}");
            }
        }
    }
}
