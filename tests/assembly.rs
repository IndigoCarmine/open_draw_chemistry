//! The assembly builder end to end: a benzene monomer built into a stack of rosettes, written
//! out as PDB, XYZ and MOL2, and (with OpenBabel) read back with every atom.

use std::time::Instant;

use open_draw_chemistry::assembly::export::{self, AssemblyExport};
use open_draw_chemistry::assembly::math::{self, Rigid};
use open_draw_chemistry::assembly::model::{Ring, Stack};
use open_draw_chemistry::assembly::{
    AssemblyState, Component, Design, Instance, Step, build, clash, file,
};
use open_draw_chemistry::three_d::geom::Clash;
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
        ..Default::default()
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

/// A small second monomer (a guest), by hand.
fn water() -> Component {
    let atom = |element: &str, pos| Atom3d {
        element: element.to_owned(),
        pos,
    };
    Component {
        name: "water".to_owned(),
        atoms: vec![
            atom("O", [0.0, 0.0, 0.0]),
            atom("H", [0.96, 0.0, 0.0]),
            atom("H", [-0.24, 0.93, 0.0]),
        ],
        bonds: vec![
            Bond3d {
                begin: 0,
                end: 1,
                order: 1,
            },
            Bond3d {
                begin: 0,
                end: 2,
                order: 1,
            },
        ],
        orient: None,
    }
}

/// Host–guest: hand-placed molecules of two components (one detached from the steps) are
/// saved and opened as the same assembly.
#[test]
fn hand_placed_molecules_of_two_components_survive_an_odasm_round_trip() {
    let mut design = rosette_stack(6, 2);
    design.components.push(water());
    design.manual.push(Instance {
        component: 1,
        transform: Rigid {
            rot: math::euler_xyz([10.0, 20.0, 30.0]),
            trans: [0.5, -0.25, 1.8],
        },
    });
    let detached = design.detach(3).unwrap();
    design.manual.push(Instance {
        component: 1,
        transform: Rigid::translation([0.0, 0.0, -3.0]),
    });
    let built = build::build(&design, 1).unwrap();
    assert_eq!(built.molecule_count(), 14);
    assert_eq!(built.component_of_molecule[detached], 0);
    assert_eq!(built.component_of_molecule[13], 1);

    let text = file::to_string(&design);
    let back = file::from_str(&text).unwrap();
    // JSON numbers may come back a last bit off, so coordinates are compared closely (the
    // components' through the rebuilt atoms below).
    let names =
        |d: &Design| -> Vec<String> { d.components.iter().map(|c| c.name.clone()).collect() };
    assert_eq!(names(&back), names(&design));
    assert_eq!(back.excluded, design.excluded);
    assert_eq!(back.manual.len(), design.manual.len());
    for (b, d) in back.manual.iter().zip(&design.manual) {
        assert_eq!(b.component, d.component);
        assert!(b.transform.approx_eq(&d.transform, 1e-12));
    }
    let rebuilt = build::build(&back, 2).unwrap();
    assert_eq!(rebuilt.component_of_molecule, built.component_of_molecule);
    assert_eq!(rebuilt.atoms.len(), built.atoms.len());
    for (a, b) in rebuilt.atoms.iter().zip(&built.atoms) {
        assert_eq!(a.element, b.element);
        assert!(math::distance(a.pos, b.pos) < 1e-9);
    }

    // A file from before free placement (no exclusions) still opens.
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(old.as_object_mut().unwrap().remove("excluded").is_some());
    let old = file::from_str(&old.to_string()).unwrap();
    assert!(old.excluded.is_empty());
    assert_eq!(old.manual.len(), design.manual.len());
}

/// The cell list finds exactly the pairs testing every pair finds, on a built assembly that is
/// packed too tightly (layers 2 Å apart, a guest inside), open and in a periodic box.
#[test]
fn the_clash_grid_agrees_with_every_pair_on_a_built_assembly() {
    let mut design = rosette_stack(6, 4);
    if let Step::Stack(s) = &mut design.steps[1] {
        s.rise = 2.0;
    }
    design.components.push(water());
    design.manual.push(Instance {
        component: 1,
        transform: Rigid::translation([6.5, 0.5, 0.2]),
    });
    let built = build::build(&design, 1).unwrap();
    let positions: Vec<[f64; 3]> = built.atoms.iter().map(|a| a.pos).collect();
    let kinds = clash::kinds(&built);
    let key = |c: &Clash| (c.a, c.b);
    let periodic = clash::Periodic::new([[30.0, 0.0, 0.0], [5.0, 28.0, 0.0], [0.0, 0.0, 8.0]]);
    for periodic in [None, periodic.as_ref()] {
        let grid = clash::find(&built, periodic);
        let brute = clash::brute_force(&positions, &kinds, &built.molecule_of_atom, periodic);
        assert!(!brute.is_empty());
        let mut a: Vec<_> = grid.pairs.iter().map(key).collect();
        let mut b: Vec<_> = brute.iter().map(key).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "periodic: {}", periodic.is_some());
        // Never inside one molecule (outside a box).
        if periodic.is_none() {
            assert!(
                grid.pairs
                    .iter()
                    .all(|c| built.molecule_of_atom[c.a] != built.molecule_of_atom[c.b])
            );
        }
    }
}

/// ~50 000 atoms are checked for clashes quickly: generous in a debug build, under a second in
/// a release build.
#[test]
fn fifty_thousand_atoms_clash_check_fast() {
    let mut design = rosette_stack(6, 700);
    // Tight layers, so there are clashes to collect as well.
    if let Step::Stack(s) = &mut design.steps[1] {
        s.rise = 2.5;
    }
    let built = build::build(&design, 1).unwrap();
    assert_eq!(built.atoms.len(), 50_400);
    let start = Instant::now();
    let found = clash::find(&built, None);
    let elapsed = start.elapsed();
    assert!(!found.pairs.is_empty() && !found.truncated);
    let limit = if cfg!(debug_assertions) { 10.0 } else { 1.0 };
    assert!(
        elapsed.as_secs_f64() < limit,
        "checked {} atoms in {elapsed:?}",
        built.atoms.len()
    );
}
