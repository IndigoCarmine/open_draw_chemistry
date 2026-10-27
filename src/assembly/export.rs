//! Writing a built assembly as PDB, XYZ, MOL2 or (for a crystal) CIF — the app's own writers,
//! so exporting works without OpenBabel and stays fast for large assemblies.
//!
//! Every molecule is a residue: PDB residue numbers count molecules (wrapping after 9999) and
//! chain IDs cycle A–Z per molecule; MOL2 has one SUBSTRUCTURE per molecule. Bonds are the
//! monomers' own, written as CONECT records and MOL2 bonds.

use std::fmt::Write as _;

use super::build::Built;
use super::math;
use super::model::{self, Design};
use super::symop;

/// A format an assembly can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyExport {
    Pdb,
    Xyz,
    Mol2,
    /// Crystallographic Information File: only for a crystal ([`AssemblyExport::needs_crystal`]).
    Cif,
}

impl AssemblyExport {
    /// The formats every assembly can be written to.
    pub const ALL: [AssemblyExport; 3] = [
        AssemblyExport::Pdb,
        AssemblyExport::Xyz,
        AssemblyExport::Mol2,
    ];

    /// The formats that need a crystal (a lattice step).
    pub const CRYSTAL: [AssemblyExport; 1] = [AssemblyExport::Cif];

    pub fn extension(self) -> &'static str {
        match self {
            AssemblyExport::Pdb => "pdb",
            AssemblyExport::Xyz => "xyz",
            AssemblyExport::Mol2 => "mol2",
            AssemblyExport::Cif => "cif",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            AssemblyExport::Pdb => "Protein Data Bank",
            AssemblyExport::Xyz => "XYZ",
            AssemblyExport::Mol2 => "Tripos MOL2",
            AssemblyExport::Cif => "Crystallographic Information File",
        }
    }

    pub fn needs_crystal(self) -> bool {
        self == AssemblyExport::Cif
    }
}

/// `built` as the text of a file in `format`. `design` names the residues; `title` goes into
/// the header. CIF without a crystal is empty (the panel does not offer it).
pub fn export(format: AssemblyExport, built: &Built, design: &Design, title: &str) -> String {
    match format {
        AssemblyExport::Pdb => pdb(built, design, title),
        AssemblyExport::Xyz => xyz(built, title),
        AssemblyExport::Mol2 => mol2(built, design, title),
        AssemblyExport::Cif => cif(built, design, title).unwrap_or_default(),
    }
}

/// A CIF value: bare when it is a plain token, otherwise in single quotes (CIF 1.1 has no
/// escapes, so a quote inside is replaced).
fn cif_value(text: &str) -> String {
    let plain = !text.is_empty()
        && text.chars().all(|c| {
            c.is_ascii_graphic() && !matches!(c, '\'' | '"' | '#' | '$' | '_' | ';' | '[' | ']')
        });
    if plain {
        text.to_owned()
    } else {
        let cleaned: String = text
            .chars()
            .map(|c| if c == '\'' || c.is_control() { ' ' } else { c })
            .collect();
        format!("'{cleaned}'")
    }
}

/// The CIF text of a crystal: the cell, the symmetry operations, and the asymmetric unit (the
/// molecules the lattice step was applied to) as fractional coordinates, each molecule moved
/// into the cell by its centroid. Atom labels are the element and a running number per
/// element (`C1`, `C2`, … `H1`, …), unique in the file. The space group's name is written when
/// the operations are one of the presets ([`symop::matching_preset`]); otherwise only the
/// operations, which is all a reader needs. `None` without a crystal.
///
/// Molecules placed by hand or by steps after the lattice are not part of the crystal and are
/// not written; bonds are left to the reader to perceive, as CIF readers do.
pub fn cif(built: &Built, design: &Design, title: &str) -> Option<String> {
    let crystal = built.crystal.as_ref()?;
    let cell = &crystal.cell;
    let mut out = String::new();
    let block: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let block = if block.trim_matches('_').is_empty() {
        "assembly".to_owned()
    } else {
        block
    };
    let _ = writeln!(out, "data_{block}");
    out.push('\n');
    let _ = writeln!(
        out,
        "_audit_creation_method {}",
        cif_value("Open DrawChemistry molecular assembly")
    );
    let _ = writeln!(out, "_chemical_name_common {}", cif_value(title));
    if let Some(preset) = symop::matching_preset(&crystal.symops) {
        let _ = writeln!(
            out,
            "_symmetry_space_group_name_H-M {}",
            cif_value(preset.hm)
        );
        let _ = writeln!(out, "_space_group_name_H-M_alt {}", cif_value(preset.hm));
    }
    out.push('\n');
    for (name, value) in [
        ("_cell_length_a", cell.a),
        ("_cell_length_b", cell.b),
        ("_cell_length_c", cell.c),
        ("_cell_angle_alpha", cell.alpha),
        ("_cell_angle_beta", cell.beta),
        ("_cell_angle_gamma", cell.gamma),
    ] {
        let _ = writeln!(out, "{name:<20} {value:.4}");
    }
    let _ = writeln!(out, "{:<20} {:.2}", "_cell_volume", cell.volume());
    // Z counts formula units; with a one-molecule asymmetric unit that is the molecules.
    if crystal.asymmetric.len() == 1 {
        let _ = writeln!(out, "{:<20} {}", "_cell_formula_units_Z", crystal.z);
    }
    out.push('\n');
    let _ = writeln!(out, "loop_");
    let _ = writeln!(out, "_space_group_symop_id");
    let _ = writeln!(out, "_space_group_symop_operation_xyz");
    for (i, op) in crystal.symops.iter().enumerate() {
        let _ = writeln!(out, "{} '{op}'", i + 1);
    }
    out.push('\n');
    let _ = writeln!(out, "loop_");
    for field in [
        "_atom_site_label",
        "_atom_site_type_symbol",
        "_atom_site_fract_x",
        "_atom_site_fract_y",
        "_atom_site_fract_z",
        "_atom_site_occupancy",
    ] {
        let _ = writeln!(out, "{field}");
    }
    let shapes = design.shapes();
    let mut numbers: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for inst in &crystal.asymmetric {
        let Some(Some((positions, elements))) = shapes.get(inst.component) else {
            continue;
        };
        let placed: Vec<_> = positions.iter().map(|&p| inst.transform.apply(p)).collect();
        // The molecule moved into the cell by its centroid, as the lattice step does.
        let shift = cell
            .to_frac(model::centroid(&placed))
            .map(|f| -(f + 1e-6).floor());
        for (p, element) in placed.iter().zip(elements) {
            let [x, y, z] = math::add(cell.to_frac(*p), shift);
            let n = numbers.entry(element.clone()).or_default();
            *n += 1;
            let _ = writeln!(
                out,
                "{:<8} {:<3} {x:>10.6} {y:>10.6} {z:>10.6} 1",
                format!("{element}{n}"),
                element
            );
        }
    }
    Some(out)
}

/// PDB's serial numbers have five columns; beyond them CONECT records cannot name atoms.
const MAX_PDB_SERIAL: usize = 99_999;

/// The residue name of molecule `m`.
fn residue_name(built: &Built, design: &Design, m: usize) -> String {
    design
        .components
        .get(built.component_of_molecule[m])
        .map_or_else(|| "MOL".to_owned(), |c| c.residue_name())
}

/// The chain ID of molecule `m`: A–Z in turn.
fn chain_id(m: usize) -> char {
    (b'A' + (m % 26) as u8) as char
}

/// The residue number of molecule `m`: 1–9999, wrapping.
fn residue_number(m: usize) -> usize {
    m % 9999 + 1
}

/// An atom name: the element and its number within the molecule, aligned as PDB wants (a
/// one-letter element starts in column 14).
fn pdb_atom_name(element: &str, number: usize) -> String {
    let element = element.to_ascii_uppercase();
    let mut name = format!("{element}{number}");
    if name.len() > 4 {
        name = element.chars().take(4).collect();
    }
    if element.len() == 1 && name.len() < 4 {
        format!(" {name:<3}")
    } else {
        format!("{name:<4}")
    }
}

/// The PDB text: a CRYST1 record for a cell, HETATM records, CONECT records for the bonds
/// (when every atom has a serial), END.
pub fn pdb(built: &Built, design: &Design, title: &str) -> String {
    let mut out = String::with_capacity(built.atoms.len() * 81 + 256);
    let _ = writeln!(
        out,
        "COMPND    {}",
        title.chars().take(70).collect::<String>()
    );
    let _ = writeln!(out, "REMARK   1 Open DrawChemistry molecular assembly");
    if let Some(c) = &built.cell {
        let _ = writeln!(
            out,
            "CRYST1{:9.3}{:9.3}{:9.3}{:7.2}{:7.2}{:7.2} P 1           1",
            c.a, c.b, c.c, c.alpha, c.beta, c.gamma
        );
    }
    for m in 0..built.molecule_count() {
        let resname = residue_name(built, design, m);
        let (chain, resseq) = (chain_id(m), residue_number(m));
        for (k, a) in built.molecule_atoms(m).enumerate() {
            let atom = &built.atoms[a];
            // Serials wrap after 99999 (CONECT is then left out).
            let serial = a % MAX_PDB_SERIAL + 1;
            let [x, y, z] = atom.pos;
            // Columns: serial 7–11, name 13–16, altLoc 17 (blank), residue 18–20, chain 22,
            // residue number 23–26, iCode 27 (blank), x y z 31–54, occupancy, B, element 77–78.
            let _ = writeln!(
                out,
                "HETATM{serial:>5} {name} {resname:>3} {chain}{resseq:>4}    {x:>8.3}{y:>8.3}{z:>8.3}{occ:>6.2}{temp:>6.2}          {element:>2}",
                name = pdb_atom_name(&atom.element, k + 1),
                occ = 1.0,
                temp = 0.0,
                element = atom.element.to_ascii_uppercase(),
            );
        }
    }
    if built.atoms.len() <= MAX_PDB_SERIAL {
        let mut partners = vec![Vec::new(); built.atoms.len()];
        for b in &built.bonds {
            partners[b.begin].push(b.end);
            partners[b.end].push(b.begin);
        }
        for (a, list) in partners.iter().enumerate() {
            for chunk in list.chunks(4) {
                let _ = write!(out, "CONECT{:>5}", a + 1);
                for p in chunk {
                    let _ = write!(out, "{:>5}", p + 1);
                }
                out.push('\n');
            }
        }
    }
    out.push_str("END\n");
    out
}

/// The XYZ text: the atom count, a title line, and one line per atom.
pub fn xyz(built: &Built, title: &str) -> String {
    let mut out = String::with_capacity(built.atoms.len() * 48 + 64);
    let _ = writeln!(out, "{}", built.atoms.len());
    let _ = writeln!(out, "{}", title.replace(['\n', '\r'], " "));
    for atom in &built.atoms {
        let [x, y, z] = atom.pos;
        let _ = writeln!(out, "{:<2} {x:>14.6} {y:>14.6} {z:>14.6}", atom.element);
    }
    out
}

/// A SYBYL atom type from the element and its bonds (MOL2 readers want one; the
/// hybridisation is read off the bond orders).
fn sybyl_type(element: &str, orders: &[u8]) -> String {
    let aromatic = orders.contains(&5);
    let max = orders
        .iter()
        .copied()
        .filter(|&o| o != 5)
        .max()
        .unwrap_or(1);
    match element {
        "C" | "N" if aromatic => format!("{element}.ar"),
        "C" if orders.iter().filter(|&&o| o == 2).count() >= 2 => "C.1".to_owned(),
        "C" => match max {
            3 => "C.1".to_owned(),
            2 => "C.2".to_owned(),
            _ => "C.3".to_owned(),
        },
        "N" => match max {
            3 => "N.1".to_owned(),
            2 => "N.2".to_owned(),
            _ => "N.3".to_owned(),
        },
        "O" if max == 2 => "O.2".to_owned(),
        "O" => "O.3".to_owned(),
        "S" if max == 2 => "S.2".to_owned(),
        "S" => "S.3".to_owned(),
        "P" => "P.3".to_owned(),
        other => other.to_owned(),
    }
}

/// The MOL2 text: one MOLECULE with every atom and bond, and a SUBSTRUCTURE per molecule.
pub fn mol2(built: &Built, design: &Design, title: &str) -> String {
    let mut out = String::with_capacity(built.atoms.len() * 80 + built.bonds.len() * 30 + 256);
    let molecules = built.molecule_count();
    let _ = writeln!(out, "@<TRIPOS>MOLECULE");
    let _ = writeln!(out, "{}", title.replace(['\n', '\r'], " "));
    let _ = writeln!(
        out,
        "{:>6} {:>6} {:>6} 0 0",
        built.atoms.len(),
        built.bonds.len(),
        molecules
    );
    let _ = writeln!(out, "SMALL");
    let _ = writeln!(out, "NO_CHARGES");
    out.push('\n');

    let mut orders = vec![Vec::new(); built.atoms.len()];
    for b in &built.bonds {
        orders[b.begin].push(b.order);
        orders[b.end].push(b.order);
    }
    let _ = writeln!(out, "@<TRIPOS>ATOM");
    let subst_names: Vec<String> = (0..molecules)
        .map(|m| format!("{}{}", residue_name(built, design, m), m + 1))
        .collect();
    for (m, subst_name) in subst_names.iter().enumerate() {
        for (k, a) in built.molecule_atoms(m).enumerate() {
            let atom = &built.atoms[a];
            let [x, y, z] = atom.pos;
            let _ = writeln!(
                out,
                "{:>7} {:<8} {x:>10.4} {y:>10.4} {z:>10.4} {:<6} {:>5} {:<8} {:>7.4}",
                a + 1,
                format!("{}{}", atom.element, k + 1),
                sybyl_type(&atom.element, &orders[a]),
                m + 1,
                subst_name,
                0.0,
            );
        }
    }
    let _ = writeln!(out, "@<TRIPOS>BOND");
    for (i, b) in built.bonds.iter().enumerate() {
        let kind = match b.order {
            5 => "ar".to_owned(),
            o => o.to_string(),
        };
        let _ = writeln!(
            out,
            "{:>6} {:>6} {:>6} {kind}",
            i + 1,
            b.begin + 1,
            b.end + 1
        );
    }
    let _ = writeln!(out, "@<TRIPOS>SUBSTRUCTURE");
    for (m, name) in subst_names.iter().enumerate() {
        let _ = writeln!(
            out,
            "{:>6} {:<8} {:>7} RESIDUE           1 {}     {:<3}  0 ROOT",
            m + 1,
            name,
            built.molecule_start[m] + 1,
            chain_id(m),
            residue_name(built, design, m),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::build::build;
    use crate::assembly::math::Cell;
    use crate::assembly::model::{Component, Step};
    use crate::three_d::{Atom3d, Bond3d};

    fn small() -> (Built, Design) {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        let design = Design {
            components: vec![Component {
                name: "co2!".into(),
                atoms: vec![
                    atom("C", [0.0, 0.0, 0.0]),
                    atom("O", [1.16, 0.0, 0.0]),
                    atom("O", [-1.16, 0.0, 0.0]),
                    atom("Cl", [0.0, -12.3456, 100.5]),
                ],
                bonds: vec![
                    Bond3d {
                        begin: 0,
                        end: 1,
                        order: 2,
                    },
                    Bond3d {
                        begin: 0,
                        end: 2,
                        order: 2,
                    },
                ],
                orient: None,
            }],
            steps: vec![Step::new_array()],
            ..Default::default()
        };
        (build(&design, 1).unwrap(), design)
    }

    #[test]
    fn pdb_records_are_fixed_width() {
        let (mut built, design) = small();
        let text = pdb(&built, &design, "test");
        let atoms: Vec<&str> = text.lines().filter(|l| l.starts_with("HETATM")).collect();
        assert_eq!(atoms.len(), 16);
        for line in &atoms {
            assert_eq!(line.len(), 78, "{line:?}");
        }
        // Molecule 2 (chain B, residue 2), its third atom (O3).
        let l = atoms[6];
        assert_eq!(&l[6..11], "    7");
        assert_eq!(&l[12..16], " O3 ");
        assert_eq!(&l[17..20], "CO2");
        assert_eq!(&l[21..22], "B");
        assert_eq!(&l[22..26], "   2");
        assert_eq!(&l[30..38], "   3.840", "x of 5 − 1.16");
        assert_eq!(&l[54..60], "  1.00");
        assert_eq!(&l[76..78], " O");
        // A two-letter element starts in column 13; wide coordinates still fit.
        let cl = atoms[3];
        assert_eq!(&cl[12..16], "CL4 ");
        assert_eq!(&cl[38..46], " -12.346");
        assert_eq!(&cl[46..54], " 100.500");
        assert_eq!(&cl[76..78], "CL");
        // Bonds both ways, and no CRYST1 without a cell.
        assert!(text.contains("CONECT    1    2    3\n"), "{text}");
        assert!(text.contains("CONECT    2    1\n"));
        assert!(!text.contains("CRYST1"));
        assert!(text.ends_with("END\n"));

        built.cell = Some(Cell {
            a: 10.0,
            b: 12.5,
            c: 7.25,
            alpha: 90.0,
            beta: 101.5,
            gamma: 90.0,
        });
        let text = pdb(&built, &design, "test");
        let cryst = text.lines().find(|l| l.starts_with("CRYST1")).unwrap();
        assert_eq!(&cryst[6..15], "   10.000");
        assert_eq!(&cryst[15..24], "   12.500");
        assert_eq!(&cryst[24..33], "    7.250");
        assert_eq!(&cryst[40..47], " 101.50");
    }

    #[test]
    fn residue_numbers_wrap_and_chains_cycle() {
        assert_eq!(residue_number(0), 1);
        assert_eq!(residue_number(9998), 9999);
        assert_eq!(residue_number(9999), 1);
        assert_eq!(chain_id(0), 'A');
        assert_eq!(chain_id(25), 'Z');
        assert_eq!(chain_id(26), 'A');
        assert_eq!(pdb_atom_name("C", 12), " C12");
        assert_eq!(pdb_atom_name("C", 123), "C123");
        assert_eq!(pdb_atom_name("Br", 1), "BR1 ");
        assert_eq!(pdb_atom_name("H", 12345), " H  ");
    }

    #[test]
    fn xyz_and_mol2_have_their_shape() {
        let (built, design) = small();
        let text = xyz(&built, "two\nlines");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "16");
        assert_eq!(lines[1], "two lines");
        assert_eq!(lines.len(), 18);
        assert!(lines[2].starts_with("C "));
        assert_eq!(lines[2].split_whitespace().count(), 4);

        let text = mol2(&built, &design, "asm");
        let section = |name: &str| -> Vec<&str> {
            text.split(&format!("@<TRIPOS>{name}\n"))
                .nth(1)
                .unwrap()
                .lines()
                .take_while(|l| !l.starts_with("@<TRIPOS>"))
                .filter(|l| !l.trim().is_empty())
                .collect()
        };
        let counts: Vec<usize> = section("MOLECULE")[1]
            .split_whitespace()
            .map(|t| t.parse().unwrap())
            .collect();
        assert_eq!(&counts[..3], &[16, 8, 4]);
        let atoms = section("ATOM");
        assert_eq!(atoms.len(), 16);
        let fields: Vec<&str> = atoms[4].split_whitespace().collect();
        assert_eq!(fields.len(), 9);
        assert_eq!(fields[5], "C.1", "a carbon with two double bonds");
        assert_eq!(fields[6], "2");
        assert_eq!(fields[7], "CO22");
        assert_eq!(section("BOND").len(), 8);
        assert_eq!(
            section("BOND")[2].split_whitespace().collect::<Vec<_>>(),
            ["3", "5", "6", "2"]
        );
        let subst = section("SUBSTRUCTURE");
        assert_eq!(subst.len(), 4);
        assert_eq!(
            subst[1].split_whitespace().take(3).collect::<Vec<_>>(),
            ["2", "CO22", "5"]
        );
    }
}
