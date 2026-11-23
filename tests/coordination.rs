//! Coordination complexes: a ligand's lone pair given to a metal, drawn as a dative bond
//! (an arrow from the donor to the metal), from 2D to 3D.

use chembuider_rs::{BondOrder, BondStereo, Config, Molecule};
use open_draw_chemistry::three_d::{self, ForceField, Model3d};

/// 1,10-Phenanthroline in a frame of its own: its nitrogens on the u axis either side of the
/// origin, the rings above them (v > 0). Kekulé bonds.
const PHEN_ATOMS: [(&str, f32, f32); 14] = [
    ("C", -1.5000, 2.5981),
    ("C", -0.7500, 1.2990),
    ("C", 0.7500, 1.2990),
    ("C", 1.5000, 2.5981),
    ("C", 0.7500, 3.8971),
    ("C", -0.7500, 3.8971),
    ("N", 1.5000, 0.0000),
    ("C", 3.0000, 0.0000),
    ("C", 3.7500, 1.2990),
    ("C", 3.0000, 2.5981),
    ("C", -3.7500, 1.2990),
    ("C", -3.0000, 0.0000),
    ("N", -1.5000, 0.0000),
    ("C", -3.0000, 2.5981),
];
const PHEN_BONDS: [(usize, usize, u8); 16] = [
    (0, 1, 2),
    (0, 5, 1),
    (0, 13, 1),
    (1, 2, 1),
    (1, 12, 1),
    (2, 3, 2),
    (2, 6, 1),
    (3, 4, 1),
    (3, 9, 1),
    (4, 5, 2),
    (6, 7, 2),
    (7, 8, 1),
    (8, 9, 2),
    (10, 11, 1),
    (10, 13, 2),
    (11, 12, 2),
];

/// [Fe(phen)₃]²⁺ as drawn: the iron in the middle, three phenanthrolines around it, each
/// nitrogen bound to the iron by `stereo` (a dative arrow from the nitrogen, or a plain line).
fn fe_phen3(stereo: BondStereo) -> Molecule {
    let mut mol = Molecule::default();
    let fe = mol.add_atom("Fe".to_owned(), [0.0, 0.0], 2);
    for k in 0..3 {
        let theta = (90.0 + 120.0 * k as f32).to_radians();
        let (c, s) = (theta.cos(), theta.sin());
        let origin = [1.5 * c, 1.5 * s];
        let ids: Vec<u32> = PHEN_ATOMS
            .iter()
            .map(|&(element, u, v)| {
                let pos = [origin[0] - u * s + v * c, origin[1] + u * c + v * s];
                mol.add_atom(element.to_owned(), pos, 0)
            })
            .collect();
        for (a, b, order) in PHEN_BONDS {
            let order = if order == 2 {
                BondOrder::Double
            } else {
                BondOrder::Single
            };
            mol.add_bond(ids[a], ids[b], order);
        }
        for (i, &(element, _, _)) in PHEN_ATOMS.iter().enumerate() {
            if element == "N" {
                let bond = mol.add_bond(ids[i], fe, BondOrder::Single);
                mol.bonds.iter_mut().find(|b| b.id == bond).unwrap().stereo = stereo.clone();
            }
        }
    }
    mol
}

/// The angles (degrees) at atom `centre` of `model` between each pair of `ligands`.
fn angles_at(model: &Model3d, centre: usize, ligands: &[usize]) -> Vec<f64> {
    let p = |i: usize| model.atoms[i].pos;
    let c = p(centre);
    let v = |i: usize| {
        let q = p(i);
        [q[0] - c[0], q[1] - c[1], q[2] - c[2]]
    };
    let mut out = Vec::new();
    for (k, &a) in ligands.iter().enumerate() {
        for &b in &ligands[k + 1..] {
            let (u, w) = (v(a), v(b));
            let dot = u[0] * w[0] + u[1] * w[1] + u[2] * w[2];
            let len = |x: [f64; 3]| (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt();
            out.push((dot / (len(u) * len(w))).acos().to_degrees());
        }
    }
    out
}

/// The atoms of `element` in `model`.
fn atoms_of(model: &Model3d, element: &str) -> Vec<usize> {
    (0..model.atoms.len())
        .filter(|&i| model.atoms[i].element == element)
        .collect()
}

/// The arrows go into the molfile as coordination bonds (type 9) from the nitrogen: the
/// 3D builder then leaves the nitrogens their valence.
#[test]
fn dative_bonds_are_written_as_coordination_bonds() {
    let molfile =
        three_d::molfile::to_molfile(&fe_phen3(BondStereo::Dative), &Config::default()).unwrap();
    let bond_lines: Vec<&str> = molfile
        .lines()
        .filter(|l| l.len() >= 9 && l[6..9].trim() == "9")
        .collect();
    assert_eq!(
        bond_lines.len(),
        6,
        "six N→Fe bonds:
{molfile}"
    );
    for line in bond_lines {
        let (begin, end): (usize, usize) = (
            line[0..3].trim().parse().unwrap(),
            line[3..6].trim().parse().unwrap(),
        );
        assert_eq!(end, 1, "to the iron, the first atom");
        assert_ne!(begin, 1, "from a nitrogen");
    }
}

/// [Fe(phen)₃]²⁺ drawn with arrows becomes an octahedral complex, under the default force
/// field too: MMFF94 has no iron, so it is built with UFF and says so.
#[test]
fn iron_tris_phenanthroline_is_built_octahedral() {
    let model = three_d::generate(
        &fe_phen3(BondStereo::Dative),
        &Config::default(),
        ForceField::default(),
    )
    .unwrap();
    assert_eq!(model.force_field, ForceField::Uff);
    assert_eq!(model.formula, "C36H24FeN6");
    assert_eq!(
        model.hydrogen_count(),
        24,
        "eight on each phenanthroline, none on N"
    );
    assert!(model.energy.is_finite());
    let fe = atoms_of(&model, "Fe")[0];
    let n = atoms_of(&model, "N");
    for &i in &n {
        let d = model.distance(fe, i);
        assert!((1.85..=2.25).contains(&d), "Fe–N {d:.2} Å");
    }
    // Twelve cis angles near 90° (the bite angle of a phenanthroline ~80°), three trans
    // near 180°.
    let mut angles = angles_at(&model, fe, &n);
    angles.sort_by(f64::total_cmp);
    for a in &angles[..12] {
        assert!((70.0..=105.0).contains(a), "cis N–Fe–N {a:.0}°: {angles:?}");
    }
    for a in &angles[12..] {
        assert!(*a > 160.0, "trans N–Fe–N {a:.0}°: {angles:?}");
    }
}

/// [Co(NH₃)₆]³⁺: each ammine keeps its three hydrogens (the arrow takes none of its valence),
/// and the six around the cobalt make an octahedron.
#[test]
fn ammine_ligands_keep_their_hydrogens() {
    let mut mol = Molecule::default();
    let co = mol.add_atom("Co".to_owned(), [0.0, 0.0], 3);
    for k in 0..6 {
        let a = (60.0 * k as f32).to_radians();
        let n = mol.add_atom("N".to_owned(), [2.0 * a.cos(), 2.0 * a.sin()], 0);
        let bond = mol.add_bond(n, co, BondOrder::Single);
        mol.bonds.iter_mut().find(|b| b.id == bond).unwrap().stereo = BondStereo::Dative;
    }
    let model = three_d::generate(&mol, &Config::default(), ForceField::Uff).unwrap();
    assert_eq!(model.formula, "CoH18N6", "three hydrogens on each nitrogen");

    // Arrows drawn the wrong way round, from the cobalt, mean the same complex.
    let mut reversed = mol.clone();
    for b in &mut reversed.bonds {
        std::mem::swap(&mut b.begin, &mut b.end);
    }
    let back = three_d::generate(&reversed, &Config::default(), ForceField::Uff).unwrap();
    assert_eq!(back.formula, "CoH18N6", "arrows from the cobalt");
    let co = atoms_of(&model, "Co")[0];
    let n = atoms_of(&model, "N");
    for &i in &n {
        let d = model.distance(co, i);
        assert!((1.8..=2.3).contains(&d), "Co–N {d:.2} Å");
    }
    let mut angles = angles_at(&model, co, &n);
    angles.sort_by(f64::total_cmp);
    for a in &angles[..12] {
        assert!((75.0..=105.0).contains(a), "cis N–Co–N {a:.0}°: {angles:?}");
    }
    for a in &angles[12..] {
        assert!(*a > 160.0, "trans N–Co–N {a:.0}°: {angles:?}");
    }
}

/// However the iron is drawn — neutral or charged, its arrows from the nitrogens or the wrong
/// way round, or plain lines — it stays bound to its six nitrogens. (UFF has iron types only
/// for an octahedral or a tetrahedral iron; drawn neutral or with its arrows pointing out, the
/// iron was perceived as neither, left out of every term, and flew off.)
#[test]
fn however_the_iron_is_drawn_it_stays_on_its_ligands() {
    for charge in [0i8, 2, 3] {
        for reversed in [false, true] {
            for stereo in [BondStereo::Dative, BondStereo::None] {
                let mut mol = fe_phen3(stereo.clone());
                mol.atoms[0].charge = charge;
                if reversed {
                    let fe = mol.atoms[0].id;
                    for b in mol.bonds.iter_mut().filter(|b| b.end == fe) {
                        std::mem::swap(&mut b.begin, &mut b.end);
                    }
                }
                let what = format!("Fe{charge:+}, reversed {reversed}, {stereo:?}");
                let model = three_d::generate(&mol, &Config::default(), ForceField::Uff)
                    .unwrap_or_else(|e| panic!("{what}: {e:?}"));
                assert_eq!(model.formula, "C36H24FeN6", "{what}");
                let fe = atoms_of(&model, "Fe")[0];
                for n in atoms_of(&model, "N") {
                    let d = model.distance(fe, n);
                    assert!((1.85..=2.25).contains(&d), "{what}: Fe–N {d:.2} Å");
                }
            }
        }
    }
}
