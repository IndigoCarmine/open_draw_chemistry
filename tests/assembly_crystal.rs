//! The crystal step end to end: a monomer copied by a space group's operations into a unit
//! cell, repeated, written out as CIF and PDB, and (with OpenBabel) read back.

use open_draw_chemistry::assembly::build::build;
use open_draw_chemistry::assembly::export::{self, AssemblyExport};
use open_draw_chemistry::assembly::math::{self, Cell, Vec3};
use open_draw_chemistry::assembly::model::{Lattice, centroid};
use open_draw_chemistry::assembly::symop::{self, SymOp};
use open_draw_chemistry::assembly::{Built, Component, Design, Step, file};
use open_draw_chemistry::three_d::{Atom3d, Bond3d};

fn atom(element: &str, pos: Vec3) -> Atom3d {
    Atom3d {
        element: element.to_owned(),
        pos,
    }
}

fn bond(begin: usize, end: usize) -> Bond3d {
    Bond3d {
        begin,
        end,
        order: 1,
    }
}

fn monoclinic() -> Cell {
    Cell {
        a: 7.0,
        b: 8.0,
        c: 9.0,
        alpha: 90.0,
        beta: 100.0,
        gamma: 90.0,
    }
}

/// A bent three-atom molecule (no symmetry) with its centroid at fractional `at` of `cell`.
fn bent(cell: &Cell, at: Vec3) -> Component {
    let local = [[0.0, 0.0, 0.0], [1.43, 0.0, 0.0], [-0.5, 1.3, 0.2]];
    let c = centroid(&local);
    let origin = cell.to_cart(at);
    Component {
        name: "meoh".into(),
        atoms: ["C", "O", "H"]
            .iter()
            .zip(local)
            .map(|(e, p)| atom(e, math::add(math::sub(p, c), origin)))
            .collect(),
        bonds: vec![bond(0, 1), bond(0, 2)],
        orient: None,
    }
}

/// Benzene centred on the origin, in the xy plane.
fn benzene() -> Component {
    let mut atoms = Vec::new();
    for (element, r) in [("C", 1.397), ("H", 2.481)] {
        for k in 0..6 {
            let a = (60.0 * k as f64).to_radians();
            atoms.push(atom(element, [r * a.cos(), r * a.sin(), 0.0]));
        }
    }
    let mut bonds = Vec::new();
    for k in 0..6 {
        bonds.push(bond(k, (k + 1) % 6));
        bonds.push(bond(k, k + 6));
    }
    Component {
        name: "benzene".into(),
        atoms,
        bonds,
        orient: None,
    }
}

fn crystal(component: Component, cell: Cell, group: &str, reps: [u32; 3]) -> Design {
    let preset = symop::PRESETS.iter().find(|p| p.name == group).unwrap();
    Design {
        components: vec![component],
        steps: vec![Step::Lattice(Lattice {
            enabled: true,
            cell,
            reps,
            symops: preset.symops(),
        })],
        ..Default::default()
    }
}

fn molecule_centroid(built: &Built, m: usize) -> Vec3 {
    let atoms: Vec<Vec3> = built
        .molecule_atoms(m)
        .map(|a| built.atoms[a].pos)
        .collect();
    centroid(&atoms)
}

#[test]
fn p21c_makes_four_whole_molecules_in_the_cell_and_repeats_them() {
    let cell = monoclinic();
    let at = [0.15, 0.2, 0.3];
    let design = crystal(bent(&cell, at), cell, "P2₁/c", [1, 1, 1]);
    let built = build(&design, 1).unwrap();
    assert_eq!(built.molecule_count(), 4);
    let c = built.crystal.as_ref().unwrap();
    assert_eq!((c.z, c.asymmetric.len()), (4, 1));
    assert_eq!(built.cell, Some(cell));

    // The centroids are the general position's images, wrapped into the cell.
    let expected = [
        [0.15, 0.2, 0.3],
        [0.85, 0.7, 0.2], // -x, y+1/2, -z+1/2
        [0.85, 0.8, 0.7], // -x, -y, -z
        [0.15, 0.3, 0.8], // x, -y+1/2, z+1/2
    ];
    for (m, e) in expected.iter().enumerate() {
        let f = cell.to_frac(molecule_centroid(&built, m));
        assert!(math::distance(f, *e) < 1e-9, "molecule {m}: {f:?}");
    }
    // Rigid copies: bond lengths kept, and the inverted copies are mirror images.
    for m in 0..4 {
        let r = built.molecule_atoms(m);
        let d = math::distance(built.atoms[r.start].pos, built.atoms[r.start + 1].pos);
        assert!((d - 1.43).abs() < 1e-9);
    }

    // 2×2×2: the unit cell first, then its copies, a fastest; the box is the supercell.
    let design = crystal(bent(&cell, at), cell, "P2₁/c", [2, 2, 2]);
    let built = build(&design, 1).unwrap();
    assert_eq!(built.molecule_count(), 32);
    assert_eq!(built.crystal.as_ref().unwrap().z, 4);
    let box_ = built.cell.unwrap();
    assert_eq!(
        (box_.a, box_.b, box_.c, box_.beta),
        (14.0, 16.0, 18.0, 100.0)
    );
    let [va, vb, vc] = cell.vectors();
    for (m, shift) in [
        (4, va),
        (8, vb),
        (16, vc),
        (31, math::add(va, math::add(vb, vc))),
    ] {
        let image = molecule_centroid(&built, m);
        let base = molecule_centroid(&built, m % 4);
        assert!(math::distance(image, math::add(base, shift)) < 1e-9, "{m}");
    }
    // Every molecule lies in the supercell.
    for m in 0..32 {
        let f = box_.to_frac(molecule_centroid(&built, m));
        assert!(f.iter().all(|x| (0.0..1.0).contains(x)), "{m}: {f:?}");
    }
}

#[test]
fn molecules_on_special_positions_are_kept_once() {
    // Benzene on the inversion centre at the origin: P-1 gives one molecule, not two.
    let cell = Cell {
        a: 6.0,
        b: 7.0,
        c: 8.0,
        alpha: 80.0,
        beta: 95.0,
        gamma: 102.0,
    };
    let design = crystal(benzene(), cell, "P-1", [1, 1, 1]);
    // Counted exactly, as the build makes it: hand-placed molecules' indices follow the
    // steps' molecules, so an upper bound would misplace them.
    assert_eq!(design.molecule_count(), 1);
    let built = build(&design, 1).unwrap();
    assert_eq!(built.crystal.as_ref().unwrap().z, 1);
    // In P2₁/c the inversion centre halves the four copies.
    let design = crystal(benzene(), monoclinic(), "P2₁/c", [1, 1, 1]);
    assert_eq!(build(&design, 1).unwrap().molecule_count(), 2);
    // A molecule with no centre of symmetry at the same place is not merged: only its
    // centroid sits on the inversion centre, not its atoms.
    let design = crystal(bent(&cell, [0.0, 0.0, 0.0]), cell, "P-1", [1, 1, 1]);
    assert_eq!(build(&design, 1).unwrap().molecule_count(), 2);
    // A molecule at the cell's corner stays whole and lands at the origin, not at (1,1,1).
    let built = build(&crystal(benzene(), cell, "P-1", [1, 1, 1]), 1).unwrap();
    assert!(math::norm(molecule_centroid(&built, 0)) < 1e-9);
}

#[test]
fn benzene_in_pbca_has_its_known_density() {
    // Benzene crystallises in Pbca with Z = 4, the molecules on inversion centres. A cell of
    // the size reported near its melting point (a ≈ 7.46, b ≈ 9.67, c ≈ 7.03 Å) gives about
    // 1.02 g/cm³, against the liquid's 0.88 and the solid's ≈ 1.0–1.1.
    let cell = Cell {
        a: 7.460,
        b: 9.666,
        c: 7.034,
        ..Cell::default()
    };
    let built = build(&crystal(benzene(), cell, "Pbca", [1, 1, 1]), 1).unwrap();
    let c = built.crystal.as_ref().unwrap();
    assert_eq!(
        c.z, 4,
        "eight operations, each molecule on an inversion centre"
    );
    let mass = built.cell_mass().unwrap();
    assert!((mass - 4.0 * 78.114).abs() < 0.01, "{mass}");
    // 4 × 78.114 g/mol ÷ (N_A × 507.21 Å³).
    let density = built.density().unwrap();
    assert!((density - 1.0229).abs() < 2e-3, "{density}");
}

#[test]
fn an_impossible_crystal_is_refused_in_japanese() {
    let mut design = crystal(benzene(), monoclinic(), "P2₁2₁2₁", [1, 1, 1]);
    // An orthorhombic group in a monoclinic cell would distort the molecules.
    let err = build(&design, 1).unwrap_err().to_string();
    assert!(err.contains("格子定数と合いません"), "{err}");
    if let Step::Lattice(l) = &mut design.steps[0] {
        l.cell.beta = 90.0;
        l.cell.gamma = 200.0;
    }
    let err = build(&design, 1).unwrap_err().to_string();
    assert!(err.starts_with("結晶"), "{err}");
    if let Step::Lattice(l) = &mut design.steps[0] {
        l.cell.gamma = 90.0;
        l.symops = vec![SymOp::parse("-x,-y,-z").unwrap()];
    }
    assert!(build(&design, 1).is_err(), "no identity");
}

#[test]
fn a_crystal_exports_as_cif_and_pdb() {
    let cell = monoclinic();
    let design = crystal(bent(&cell, [0.15, 0.2, 0.3]), cell, "P2₁/c", [2, 2, 2]);
    let built = build(&design, 1).unwrap();

    let cif = export::export(AssemblyExport::Cif, &built, &design, "test crystal");
    assert!(cif.starts_with("data_test_crystal\n"), "{cif}");
    assert!(cif.contains("_symmetry_space_group_name_H-M 'P 1 21/c 1'"));
    assert!(cif.contains("_cell_length_a       7.0000"));
    assert!(cif.contains("_cell_angle_beta     100.0000"));
    assert!(cif.contains("_cell_formula_units_Z 4"));
    let ops: Vec<&str> = cif
        .lines()
        .skip_while(|l| *l != "_space_group_symop_operation_xyz")
        .skip(1)
        .take_while(|l| !l.is_empty())
        .collect();
    assert_eq!(
        ops,
        [
            "1 'x,y,z'",
            "2 '-x,y+1/2,-z+1/2'",
            "3 '-x,-y,-z'",
            "4 'x,-y+1/2,z+1/2'"
        ]
    );
    // The asymmetric unit only: three atoms, labelled uniquely, at fractional coordinates.
    let sites: Vec<Vec<&str>> = cif
        .lines()
        .skip_while(|l| *l != "_atom_site_occupancy")
        .skip(1)
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(sites.len(), 3);
    assert_eq!(
        sites.iter().map(|s| s[0]).collect::<Vec<_>>(),
        ["C1", "O1", "H1"]
    );
    let frac: Vec<Vec3> = sites
        .iter()
        .map(|s| [s[2], s[3], s[4]].map(|v| v.parse().unwrap()))
        .collect();
    let f = centroid(&frac);
    assert!(math::distance(f, [0.15, 0.2, 0.3]) < 1e-5, "{f:?}");

    // CIF 1.1 is ASCII: a Japanese title leaves its ASCII part as the block name.
    let named = export::cif(&built, &design, "無題 1_assembly").unwrap();
    assert!(named.starts_with("data_1_assembly\n"), "{named}");
    assert!(named.is_ascii());
    assert!(
        export::cif(&built, &design, "結晶")
            .unwrap()
            .starts_with("data_assembly\n")
    );

    // No crystal, no CIF.
    let plain = Design {
        steps: vec![Step::new_ring()],
        ..design.clone()
    };
    let plain_built = build(&plain, 1).unwrap();
    assert!(export::cif(&plain_built, &plain, "x").is_none());

    // PDB: every atom, with the supercell as CRYST1.
    let pdb = export::export(AssemblyExport::Pdb, &built, &design, "test crystal");
    let cryst = pdb.lines().find(|l| l.starts_with("CRYST1")).unwrap();
    assert_eq!(&cryst[6..33], "   14.000   16.000   18.000");
    assert_eq!(pdb.lines().filter(|l| l.starts_with("HETATM")).count(), 96);

    #[cfg(feature = "openbabel")]
    {
        // OpenBabel reads the CIF's asymmetric unit as it is (it does not fill the cell
        // unless asked to), so it sees the three atoms.
        let mol = openbabel::Molecule::parse(&cif, "cif").expect("OpenBabel reads the CIF");
        assert_eq!(mol.num_atoms(), 3);
        let mol = openbabel::Molecule::parse(&pdb, "pdb").expect("OpenBabel reads the PDB");
        assert_eq!(mol.num_atoms(), 96);
    }
}

#[test]
fn a_saved_crystal_reopens_to_the_same_build() {
    let cell = monoclinic();
    let design = crystal(bent(&cell, [0.4, 0.1, 0.6]), cell, "C2/c", [1, 2, 1]);
    let text = file::to_string(&design);
    let back = file::from_str(&text).unwrap();
    assert_eq!(back, design);
    let built = build(&back, 1).unwrap();
    assert_eq!(built, build(&design, 1).unwrap());
    assert_eq!(
        built.crystal.as_ref().unwrap().z,
        8,
        "C2/c, general position"
    );
    assert_eq!(built.molecule_count(), 16);
}
