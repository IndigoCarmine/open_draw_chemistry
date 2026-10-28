//! The drawn structure as an MDL molfile (V2000), the form OpenBabel reads it in.
//!
//! Hydrogens are left implicit: OpenBabel fills each atom's valence when it reads the file, and
//! [`super::generate`] makes them explicit. Abbreviations ("Ph", "OMe", "CO_2Et") are expanded
//! into their atoms first, on a copy, with the editor's own label machinery — an abbreviation
//! written into the file as an element would be read as a dummy atom and give a wrong model.
//!
//! A polymer drawn with repeat-unit brackets (`[CH2–CH2]n`) is built as one repeat unit with its
//! open ends closed by hydrogens (`n = 1`): the bare carbons that only stand for the chain going
//! on are left out, so OpenBabel fills the crossing bonds' valences with H. A multiple group
//! (×n) is refused: its atoms would have to be drawn n times.

use std::collections::HashMap;
use std::fmt::Write as _;

use chembuider_rs::label_chain::resolve_label;
use chembuider_rs::molecule::BracketUsage;
use chembuider_rs::molecule::bracket::open_ends;
use chembuider_rs::molecule::elements::element_of_label;
use chembuider_rs::widget::rich_text::RichText;
use chembuider_rs::{BondOrder, BondStereo, ChemStructEditor, Config, Molecule};

use super::Error;

/// Molfile ångström per editor unit. The editor draws a bond 1.5 units long, so this makes a
/// drawn bond 1.5 Å — close to a real one, which gives OpenBabel's 2D stereo perception and
/// 3D builder sensible distances to start from.
pub const ANGSTROM_PER_UNIT: f64 = 1.0;

/// A V2000 count field is three digits wide.
const MAX_COUNT: usize = 999;

/// What 3D generation builds of a drawing with brackets: for a polymer's repeat unit, the
/// drawing without the open ends (one unit, its ends closed by hydrogens); `None` when the
/// brackets change nothing (no brackets, or a mixture's). A multiple group is refused.
pub fn repeat_unit(mol: &Molecule) -> Result<Option<Molecule>, Error> {
    if mol.brackets().next().is_none() {
        return Ok(None);
    }
    if mol
        .brackets()
        .any(|b| b.usage == BracketUsage::MultipleGroup)
    {
        return Err(Error::MultipleGroup);
    }
    if !mol.brackets().any(|b| b.usage.is_polymer_unit()) {
        return Ok(None);
    }
    let mut unit = mol.clone();
    for id in open_ends(mol) {
        unit.remove_atom(id);
    }
    unit.objects.clear();
    Ok(Some(unit))
}

/// The status bar note for generating `mol` in 3D when it is a polymer drawn with
/// repeat-unit brackets: what is built instead.
pub fn polymer_note(mol: &Molecule) -> Option<&'static str> {
    match repeat_unit(mol) {
        Ok(Some(_)) => Some("繰り返し単位 1 つ分（n = 1、鎖の端は H で閉じた構造）で 3D 化します"),
        _ => None,
    }
}

/// `mol` as a V2000 molfile, abbreviations expanded (see the module docs).
///
/// Coordinates are scaled by [`ANGSTROM_PER_UNIT`] with y flipped (the editor's y points down,
/// a molfile's up), so the drawing keeps its handedness and wedges keep their meaning. Fails
/// on an empty drawing, on labels that are no element and no known abbreviation, and on
/// quadruple bonds, which a molfile has no order for.
pub fn to_molfile(mol: &Molecule, config: &Config) -> Result<String, Error> {
    if mol.atoms.is_empty() {
        return Err(Error::Empty);
    }
    let unit = repeat_unit(mol)?;
    let mol = expand_labels(unit.as_ref().unwrap_or(mol), config)?;
    if mol.atoms.len() > MAX_COUNT || mol.bonds.len() > MAX_COUNT {
        return Err(Error::TooLarge);
    }
    if mol.bonds.iter().any(|b| b.order == BondOrder::Quadruple) {
        return Err(Error::QuadrupleBond);
    }

    // Molfile atoms are numbered from 1 in file order.
    let index: HashMap<u32, usize> = mol
        .atoms
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id, i + 1))
        .collect();
    let stereo_flags: Vec<u8> = mol.bonds.iter().map(|b| stereo_flag(&b.stereo)).collect();
    // The chiral flag says the wedges mean the drawn enantiomer, not either one.
    let chiral = u8::from(stereo_flags.iter().any(|&s| s == 1 || s == 6));

    let mut out = String::new();
    // Header: name, program/dimension line, comment.
    let _ = writeln!(out, "{}", mol.name.lines().next().unwrap_or(""));
    let _ = writeln!(out, "  OpenDraw          2D");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:>3}{:>3}  0  0{:>3}  0  0  0  0  0999 V2000",
        mol.atoms.len(),
        mol.bonds.len(),
        chiral
    );
    for atom in &mol.atoms {
        let x = f64::from(atom.pos[0]) * ANGSTROM_PER_UNIT;
        let y = -f64::from(atom.pos[1]) * ANGSTROM_PER_UNIT;
        // Charges go in `M  CHG` below, which supersedes the atom block's field.
        let _ = writeln!(
            out,
            "{x:>10.4}{y:>10.4}{:>10.4} {:<3} 0  0  0  0  0  0  0  0  0  0  0  0",
            0.0, atom.element
        );
    }
    for (bond, stereo) in mol.bonds.iter().zip(&stereo_flags) {
        let order = match bond.order {
            BondOrder::Single => 1,
            BondOrder::Double => 2,
            BondOrder::Triple => 3,
            BondOrder::Quadruple => unreachable!("rejected above"),
        };
        let _ = writeln!(
            out,
            "{:>3}{:>3}{:>3}{:>3}  0  0  0",
            index[&bond.begin], index[&bond.end], order, stereo
        );
    }
    let charged: Vec<(usize, i8)> = mol
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, a)| a.charge != 0)
        .map(|(i, a)| (i + 1, a.charge))
        .collect();
    // At most eight entries per line.
    for chunk in charged.chunks(8) {
        let _ = write!(out, "M  CHG{:>3}", chunk.len());
        for (atom, charge) in chunk {
            let _ = write!(out, " {atom:>3} {charge:>3}");
        }
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "M  END");
    Ok(out)
}

/// The molfile bond stereo flag of a drawn bond: 1 for a solid wedge, 6 for a hashed one and
/// 4 for a wavy (either) bond. Every wedge points from its narrow begin atom, as a molfile's do.
fn stereo_flag(stereo: &BondStereo) -> u8 {
    match stereo {
        BondStereo::WedgeUp => 1,
        BondStereo::WedgeDown | BondStereo::Hashed => 6,
        BondStereo::Wavy => 4,
        _ => 0,
    }
}

/// A copy of `mol` with every abbreviation expanded into its atoms, as the editor's "expand
/// label" would; [`Error::UnknownLabels`] lists the labels that are neither elements nor
/// abbreviations.
pub fn expand_labels(mol: &Molecule, config: &Config) -> Result<Molecule, Error> {
    let labels: Vec<(u32, String)> = mol
        .atoms
        .iter()
        .filter(|a| element_of_label(&a.element).is_none())
        .map(|a| (a.id, a.element.clone()))
        .collect();
    if labels.is_empty() {
        return Ok(mol.clone());
    }

    // The placement logic lives on the editor widget, so a scratch editor holds the copy.
    let mut editor = ChemStructEditor::default();
    editor.config = config.clone();
    editor.molecule = mol.clone();
    for (id, label) in labels {
        let Some(frag) = resolve_label(config, &label).or_else(|| {
            // A label from ChemDraw carries its formatting ("CO_2Et"): try the plain text.
            let plain = plain_text(&label);
            (plain != label)
                .then(|| resolve_label(config, &plain))
                .flatten()
        }) else {
            continue;
        };
        let root = &frag.atoms[frag.attach_idx];
        if let Some(atom) = editor.molecule.atom_by_id_mut(id) {
            atom.element = root.element.clone();
            atom.charge = root.charge;
        }
        let (angle, flip) = editor.best_fragment_placement(id, &frag);
        editor.molecule.insert_fragment(&frag, id, angle, flip);
    }

    // Whatever is still no element was not expanded (or expanded into another label).
    let mut unknown: Vec<String> = editor
        .molecule
        .atoms
        .iter()
        .filter(|a| element_of_label(&a.element).is_none())
        .map(|a| a.element.clone())
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        unknown.dedup();
        return Err(Error::UnknownLabels(unknown));
    }
    Ok(editor.molecule)
}

/// A label's text without its rich-text markup (sub/superscripts, bold, escapes).
fn plain_text(label: &str) -> String {
    RichText::parse(label)
        .spans
        .into_iter()
        .map(|s| s.text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The atom and bond lines of a molfile, split into fields.
    fn blocks(molfile: &str) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
        let lines: Vec<&str> = molfile.lines().collect();
        let counts = lines[3];
        let atoms: usize = counts[0..3].trim().parse().unwrap();
        let bonds: usize = counts[3..6].trim().parse().unwrap();
        let split = |l: &&str| l.split_whitespace().map(str::to_owned).collect();
        (
            lines[4..4 + atoms].iter().map(split).collect(),
            lines[4 + atoms..4 + atoms + bonds]
                .iter()
                .map(split)
                .collect(),
        )
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

    /// Polyethylene's repeat unit as drawn: C–[C–C]n–C, the outer carbons bare.
    fn polyethylene() -> Molecule {
        let mut mol = Molecule::default();
        let ids: Vec<u32> = (0..4)
            .map(|k| mol.add_atom("C".to_owned(), [k as f32 * 1.3, 0.0], 0))
            .collect();
        for w in ids.windows(2) {
            mol.add_bond(w[0], w[1], BondOrder::Single);
        }
        mol.add_bracket(&[ids[1], ids[2]].into(), BracketUsage::Sru);
        mol
    }

    #[test]
    fn a_repeat_unit_is_built_with_its_ends_closed_by_hydrogen() {
        let mol = polyethylene();
        let (atoms, bonds) = blocks(&to_molfile(&mol, &Config::default()).unwrap());
        assert_eq!(
            (atoms.len(), bonds.len()),
            (2, 1),
            "ethane's carbons: one unit, n = 1"
        );
        assert!(polymer_note(&mol).is_some());
        assert!(polymer_note(&ethanol()).is_none());
    }

    #[test]
    fn a_multiple_group_is_refused() {
        let mut mol = ethanol();
        let first = mol.atoms[0].id;
        mol.add_bracket(&[first].into(), BracketUsage::MultipleGroup);
        assert!(matches!(
            to_molfile(&mol, &Config::default()),
            Err(Error::MultipleGroup)
        ));
    }

    #[test]
    fn counts_elements_and_bonds_are_written() {
        let text = to_molfile(&ethanol(), &Config::default()).unwrap();
        assert!(text.lines().nth(3).unwrap().ends_with("V2000"));
        assert!(text.trim_end().ends_with("M  END"));
        let (atoms, bonds) = blocks(&text);
        assert_eq!(atoms.len(), 3);
        assert_eq!(bonds.len(), 2);
        assert_eq!(atoms[2][3], "O");
        assert_eq!(bonds[1][..4], ["2", "3", "1", "0"]);
    }

    #[test]
    fn y_is_flipped_and_the_scale_kept() {
        let text = to_molfile(&ethanol(), &Config::default()).unwrap();
        let (atoms, _) = blocks(&text);
        let coord = |i: usize, j: usize| atoms[i][j].parse::<f64>().unwrap();
        assert_eq!(coord(1, 0), 1.3);
        assert_eq!(coord(1, 1), -0.75, "the editor's y points down");
        assert_eq!(coord(1, 2), 0.0);
    }

    #[test]
    fn charges_go_into_m_chg() {
        let mut mol = Molecule::default();
        let n = mol.add_atom("N".to_owned(), [0.0, 0.0], 1);
        let o = mol.add_atom("O".to_owned(), [1.5, 0.0], -1);
        mol.add_bond(n, o, BondOrder::Single);
        let text = to_molfile(&mol, &Config::default()).unwrap();
        let chg = text.lines().find(|l| l.starts_with("M  CHG")).unwrap();
        let fields: Vec<&str> = chg.split_whitespace().collect();
        assert_eq!(fields[2..], ["2", "1", "1", "2", "-1"]);
    }

    #[test]
    fn wedges_become_bond_stereo_flags() {
        let mut mol = ethanol();
        let ids: Vec<u32> = mol.atoms.iter().map(|a| a.id).collect();
        let f = mol.add_atom("F".to_owned(), [1.3, 2.25], 0);
        let cl = mol.add_atom("Cl".to_owned(), [1.3, -0.75], 0);
        let wedge = mol.add_bond(ids[1], f, BondOrder::Single);
        let hash = mol.add_bond(ids[1], cl, BondOrder::Single);
        for b in &mut mol.bonds {
            if b.id == wedge {
                b.stereo = BondStereo::WedgeUp;
            } else if b.id == hash {
                b.stereo = BondStereo::WedgeDown;
            }
        }
        let text = to_molfile(&mol, &Config::default()).unwrap();
        let (_, bonds) = blocks(&text);
        assert_eq!(bonds[2][..4], ["2", "4", "1", "1"]);
        assert_eq!(bonds[3][..4], ["2", "5", "1", "6"]);
        assert_eq!(&text.lines().nth(3).unwrap()[12..15], "  1", "chiral flag");
    }

    #[test]
    fn abbreviations_are_expanded() {
        let mut mol = Molecule::default();
        let c = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let ome = mol.add_atom("OMe".to_owned(), [1.5, 0.0], 0);
        mol.add_bond(c, ome, BondOrder::Single);
        let text = to_molfile(&mol, &Config::default()).unwrap();
        let (atoms, bonds) = blocks(&text);
        let elements: Vec<&str> = atoms.iter().map(|a| a[3].as_str()).collect();
        assert_eq!(elements, ["C", "O", "C"], "C–O–CH3");
        assert_eq!(bonds.len(), 2);
    }

    #[test]
    fn formatted_abbreviations_are_expanded() {
        let mut mol = Molecule::default();
        let c = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let ester = mol.add_atom("CO_2Me".to_owned(), [1.5, 0.0], 0);
        mol.add_bond(c, ester, BondOrder::Single);
        let expanded = expand_labels(&mol, &Config::default()).unwrap();
        let mut elements: Vec<&str> = expanded.atoms.iter().map(|a| a.element.as_str()).collect();
        elements.sort_unstable();
        assert_eq!(elements, ["C", "C", "C", "O", "O"]);
    }

    #[test]
    fn unknown_labels_are_refused() {
        let mut mol = ethanol();
        mol.add_atom("Xq".to_owned(), [5.0, 0.0], 0);
        mol.add_atom("R1".to_owned(), [6.0, 0.0], 0);
        match to_molfile(&mol, &Config::default()) {
            Err(Error::UnknownLabels(labels)) => assert_eq!(labels, ["R1", "Xq"]),
            other => panic!("expected the labels to be refused, got {other:?}"),
        }
    }

    #[test]
    fn empty_drawings_and_quadruple_bonds_are_refused() {
        assert_eq!(
            to_molfile(&Molecule::default(), &Config::default()),
            Err(Error::Empty)
        );
        let mut mol = Molecule::default();
        let a = mol.add_atom("Mo".to_owned(), [0.0, 0.0], 0);
        let b = mol.add_atom("Mo".to_owned(), [1.5, 0.0], 0);
        mol.add_bond(a, b, BondOrder::Quadruple);
        assert_eq!(
            to_molfile(&mol, &Config::default()),
            Err(Error::QuadrupleBond)
        );
    }
}
