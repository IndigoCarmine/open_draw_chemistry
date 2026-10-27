//! The assembly builder end to end: a benzene monomer built into a stack of rosettes, written
//! out as PDB, XYZ and MOL2, and (with OpenBabel) read back with every atom.

use std::time::Instant;

use open_draw_chemistry::assembly::export::{self, AssemblyExport};
use open_draw_chemistry::assembly::model::{Ring, Stack};
use open_draw_chemistry::assembly::{AssemblyState, Component, Design, Step, build, file};
use open_draw_chemistry::three_d::{Atom3d, Bond3d};

/// Benzene: from the test file through OpenBabel when it is built in, by hand otherwise.
fn benzene() -> Component {
    #[cfg(feature = "openbabel")]
    {
        use open_draw_chemistry::three_d::read::{Format3d, read};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/benzene.mol2");
        let text = std::fs::read_to_string(path).unwrap();
        let model = read(Format3d::Mol2, &text).unwrap().model.unwrap();
        Component::from_model("benzene", &model)
    }
    #[cfg(not(feature = "openbabel"))]
    {
        hand_built_benzene()
    }
}

#[cfg_attr(feature = "openbabel", allow(dead_code))]
fn hand_built_benzene() -> Component {
    let mut atoms = Vec::new();
    for (element, r) in [("C", 1.397), ("H", 2.481)] {
        for k in 0..6 {
            let a = (60.0 * k as f64).to_radians();
            atoms.push(Atom3d {
                element: element.to_owned(),
                pos: [r * a.cos(), r * a.sin(), 0.0],
            });
        }
    }
    let mut bonds = Vec::new();
    for k in 0..6 {
        bonds.push(Bond3d {
            begin: k,
            end: (k + 1) % 6,
            order: if k % 2 == 0 { 2 } else { 1 },
        });
        bonds.push(Bond3d {
            begin: k,
            end: k + 6,
            order: 1,
        });
    }
    Component {
        name: "benzene".to_owned(),
        atoms,
        bonds,
        orient: None,
    }
}

fn rosette_stack(rings: u32, layers: u32) -> Design {
    Design {
        components: vec![benzene()],
        base: 0,
        steps: vec![
            Step::Ring(Ring {
                enabled: true,
                n: rings,
                radius: 7.0,
                tilt_deg: 0.0,
            }),
            Step::Stack(Stack {
                enabled: true,
                n: layers,
                rise: 3.6,
                twist_deg: 15.0,
                slip: 0.5,
            }),
        ],
        manual: Vec::new(),
    }
}

#[test]
fn a_benzene_rosette_stack_exports_and_reads_back() {
    let mut state = AssemblyState::with_design(rosette_stack(6, 10));
    let built = state.built().unwrap();
    assert_eq!(built.molecule_count(), 60);
    assert_eq!(built.atoms.len(), 720);
    assert_eq!(built.bonds.len(), 720);

    let gap = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    // The monomer is centred on its origin, so the centres sit on the ring: neighbours in a
    // layer are 2r·sin(π/6) = r apart.
    let centre = |m: usize| {
        let r = built.molecule_atoms(m);
        let n = r.len() as f64;
        r.fold([0.0; 3], |c, a| {
            let p = built.atoms[a].pos;
            [c[0] + p[0] / n, c[1] + p[1] / n, c[2] + p[2] / n]
        })
    };
    assert!((gap(centre(0), centre(1)) - 7.0).abs() < 1e-6);
    // Layers are 3.6 Å apart along z.
    assert!((centre(6)[2] - centre(0)[2] - 3.6).abs() < 1e-6);

    let design = &state.design;
    for format in AssemblyExport::ALL {
        let text = export::export(format, &built, design, "benzene rosette");
        assert!(!text.is_empty());
        #[cfg(feature = "openbabel")]
        {
            let mol = openbabel::Molecule::parse(&text, format.extension())
                .unwrap_or_else(|_| panic!("OpenBabel could not read the {format:?} export"));
            assert_eq!(mol.num_atoms() as usize, 720, "{format:?}");
            if format != AssemblyExport::Xyz {
                assert_eq!(mol.num_bonds() as usize, 720, "{format:?} keeps the bonds");
            }
        }
    }

    // The app's own 3D reader opens the PDB export (as the app does on File ▸ Open).
    #[cfg(feature = "openbabel")]
    {
        use open_draw_chemistry::three_d::read::{Format3d, read};
        let pdb = export::export(AssemblyExport::Pdb, &built, design, "rosette");
        let opened = read(Format3d::Pdb, &pdb).unwrap();
        assert_eq!(opened.model.unwrap().atoms.len(), 720);
    }

    // And the design survives an .odasm round trip.
    let back = file::from_str(&file::to_string(design)).unwrap();
    assert_eq!(build::build(&back, 1).unwrap().atoms, built.atoms);
}

/// ~50 000 atoms build quickly. The bound is generous in a debug build (unoptimised code is
/// roughly ten times slower); the goal of under a second applies to release builds.
#[test]
fn fifty_thousand_atoms_build_fast() {
    let design = rosette_stack(6, 700);
    let start = Instant::now();
    let built = build::build(&design, 1).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(built.atoms.len(), 50_400);
    let limit = if cfg!(debug_assertions) { 10.0 } else { 1.0 };
    assert!(
        elapsed.as_secs_f64() < limit,
        "built {} atoms in {elapsed:?}",
        built.atoms.len()
    );
    // Writing it out is linear too.
    let start = Instant::now();
    let pdb = export::pdb(&built, &design, "big");
    assert!(pdb.lines().filter(|l| l.starts_with("HETATM")).count() == 50_400);
    assert!(start.elapsed().as_secs_f64() < limit * 2.0);
}
