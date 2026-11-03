//! The SMILES of the selected structures (or of the whole drawing), for the 解析 panel.
//!
//! Written by chembuider-rs's own [`to_smiles`], the same writer as the SMILES file export, so
//! no OpenBabel is involved. The string is **canonical** (it depends only on the chemical
//! graph) but carries **no stereochemistry**: wedges, hashes and double-bond geometry are not
//! read, so enantiomers and E/Z isomers give the same string. Rings come out in Kekulé form
//! (`C1=CC=CC=C1`, not `c1ccccc1`), as the editor stores them.
//!
//! Abbreviations ("Ph", "OMe", "CO_2Et") are expanded into their atoms first, with the same
//! machinery as the 3D builder ([`molfile::expand_labels`]); a label that is neither an element
//! nor a known abbreviation is refused rather than written as a wildcard.
//!
//! Brackets with a number (`[CH2–CH2–O]3`, a multiple group) are written out first, so the
//! SMILES is that of the molecule they stand for, as are the panel's formula and masses. A
//! repeat unit labelled `n` stays as drawn: the SMILES of `n = 1`, end groups included.

use std::collections::HashSet;

use chembuider_rs::molecule::bracket::expand;
use chembuider_rs::molecule::smiles::to_smiles;
use chembuider_rs::widget::analysis::analysis_target;
use chembuider_rs::{BondOrder, Config, Molecule};

use crate::three_d::Error;
use crate::three_d::molfile;

/// The structures the SMILES is of: those with a selected atom, or everything drawn, numbered
/// brackets written out (see the module docs). Page objects (arrows, text, tables, brackets
/// left) play no part.
pub fn target(mol: &Molecule, selection: &HashSet<u32>) -> Molecule {
    let mut target = expand(&analysis_target(mol, selection));
    target.objects.clear();
    target.ring_fills.clear();
    target
}

/// The canonical SMILES (no stereo, Kekulé rings) of `mol` (already the [`target`]), or a
/// short reason why there is none. May be slow for a big drawing: call it off the UI thread.
pub fn compute(mol: &Molecule, config: &Config) -> Result<String, String> {
    if mol.atoms.is_empty() {
        return Err(t!("smiles.error.empty").to_owned());
    }
    if mol.bonds.iter().any(|b| b.order == BondOrder::Quadruple) {
        return Err(t!("smiles.error.quadruple_bond").to_owned());
    }
    expanded_smiles(mol, config).map_err(|e| match e {
        Error::UnknownLabels(labels) => {
            t!(
                "smiles.error.unknown_labels",
                labels = labels.join(t!("smiles.error.label_separator"))
            )
        }
        other => other.to_string(),
    })
}

/// [`to_smiles`] of `mol` with its abbreviations expanded (a multiple group that could not be
/// written out is refused, as for 3D).
fn expanded_smiles(mol: &Molecule, config: &Config) -> Result<String, Error> {
    let mol = molfile::expand_labels(&molfile::built_structure(mol)?, config)?;
    Ok(to_smiles(&mol))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chembuider_rs::BondStereo;
    use chembuider_rs::molecule::{BracketUsage, PageObject};

    fn config() -> Config {
        Config::default()
    }

    /// Ethanol, C–C–O.
    fn ethanol(mol: &mut Molecule, dx: f32) -> u32 {
        let a = mol.add_atom("C".into(), [dx, 0.0], 0);
        let b = mol.add_atom("C".into(), [dx + 1.3, 0.75], 0);
        let c = mol.add_atom("O".into(), [dx + 2.6, 0.0], 0);
        mol.add_bond(a, b, BondOrder::Single);
        mol.add_bond(b, c, BondOrder::Single);
        a
    }

    /// Butan-2-ol with C2's bond to O wedged (`stereo`). In molfile coordinates (y up) O is
    /// above C2, the ethyl lower right and the methyl lower left: a solid wedge makes it (R).
    fn butan_2_ol(stereo: BondStereo) -> Molecule {
        let mut m = Molecule::default();
        let c2 = m.add_atom("C".into(), [0.0, 0.0], 0);
        // Editor y points down: negative y is up on screen.
        let o = m.add_atom("O".into(), [0.0, -1.5], 0);
        let c1 = m.add_atom("C".into(), [-1.3, 0.75], 0);
        let c3 = m.add_atom("C".into(), [1.3, 0.75], 0);
        let c4 = m.add_atom("C".into(), [2.6, 0.0], 0);
        let wedge = m.add_bond(c2, o, BondOrder::Single);
        m.bonds.iter_mut().find(|b| b.id == wedge).unwrap().stereo = stereo;
        m.add_bond(c2, c1, BondOrder::Single);
        m.add_bond(c2, c3, BondOrder::Single);
        m.add_bond(c3, c4, BondOrder::Single);
        m
    }

    fn but_2_ene(trans: bool) -> Molecule {
        let mut m = Molecule::default();
        let c1 = m.add_atom("C".into(), [-0.65, -1.1], 0);
        let c2 = m.add_atom("C".into(), [0.0, 0.0], 0);
        let c3 = m.add_atom("C".into(), [1.3, 0.0], 0);
        let y4 = if trans { 1.1 } else { -1.1 };
        let c4 = m.add_atom("C".into(), [1.95, y4], 0);
        m.add_bond(c1, c2, BondOrder::Single);
        m.add_bond(c2, c3, BondOrder::Double);
        m.add_bond(c3, c4, BondOrder::Single);
        m
    }

    #[test]
    fn ethanol_is_cco() {
        let mut m = Molecule::default();
        ethanol(&mut m, 0.0);
        assert_eq!(compute(&m, &config()).unwrap(), "CCO");
    }

    #[test]
    fn two_molecules_are_dot_joined() {
        let mut m = Molecule::default();
        ethanol(&mut m, 0.0);
        m.add_atom("N".into(), [10.0, 0.0], 0);
        let s = compute(&m, &config()).unwrap();
        let mut parts: Vec<&str> = s.split('.').collect();
        parts.sort_unstable();
        assert_eq!(parts, ["CCO", "N"]);
    }

    #[test]
    fn the_selection_scopes_the_smiles() {
        let mut m = Molecule::default();
        ethanol(&mut m, 0.0);
        let n = m.add_atom("N".into(), [10.0, 0.0], 0);
        let selected: HashSet<u32> = [n].into_iter().collect();
        assert_eq!(compute(&target(&m, &selected), &config()).unwrap(), "N");
        let all = target(&m, &HashSet::new());
        assert_eq!(all.atoms.len(), 4);
    }

    /// Me–[CH2–CH2–O]–Me with the repeat unit labelled `label`.
    fn glyme(label: &str) -> Molecule {
        let mut m = Molecule::default();
        let ids: Vec<u32> = ["C", "C", "C", "O", "C"]
            .iter()
            .enumerate()
            .map(|(k, el)| {
                let y = if k % 2 == 0 { 0.0 } else { -0.75 };
                m.add_atom((*el).into(), [k as f32 * 1.3, y], 0)
            })
            .collect();
        for w in ids.windows(2) {
            m.add_bond(w[0], w[1], BondOrder::Single);
        }
        let id = m
            .add_bracket(&[ids[1], ids[2], ids[3]].into(), BracketUsage::Sru)
            .unwrap();
        if let Some(PageObject::Bracket(b)) = m.object_by_id_mut(id) {
            b.label = label.into();
        }
        m
    }

    #[test]
    fn a_numbered_repeat_unit_is_written_out() {
        let all = HashSet::new();
        let s = compute(&target(&glyme("3"), &all), &config()).unwrap();
        assert_eq!(s, "CCCOCCOCCOC", "CH3(CH2CH2O)3CH3");
        let s = compute(&target(&glyme("n"), &all), &config()).unwrap();
        assert_eq!(s, "CCCOC", "n = 1, the end groups kept");
    }

    #[test]
    fn unknown_labels_and_quadruple_bonds_give_a_reason() {
        let mut m = Molecule::default();
        let a = m.add_atom("Xyzzy".into(), [0.0, 0.0], 0);
        let b = m.add_atom("C".into(), [1.3, 0.0], 0);
        m.add_bond(a, b, BondOrder::Single);
        let err = compute(&m, &config()).unwrap_err();
        assert!(err.contains("Xyzzy"), "{err}");

        let mut q = Molecule::default();
        let a = q.add_atom("Mo".into(), [0.0, 0.0], 0);
        let b = q.add_atom("Mo".into(), [1.3, 0.0], 0);
        q.add_bond(a, b, BondOrder::Quadruple);
        assert!(compute(&q, &config()).unwrap_err().contains("Quadruple"));
        assert!(compute(&Molecule::default(), &config()).is_err());
    }

    /// A hexagon of alternating single and double bonds, as the editor stores benzene.
    fn benzene(mol: &mut Molecule, dx: f32) -> Vec<u32> {
        let ids: Vec<u32> = (0..6)
            .map(|k| {
                let angle = k as f32 * std::f32::consts::FRAC_PI_3;
                mol.add_atom("C".into(), [dx + 1.5 * angle.cos(), 1.5 * angle.sin()], 0)
            })
            .collect();
        for k in 0..6 {
            let order = if k % 2 == 0 {
                BondOrder::Double
            } else {
                BondOrder::Single
            };
            mol.add_bond(ids[k], ids[(k + 1) % 6], order);
        }
        ids
    }

    #[test]
    fn benzene_is_written_kekule() {
        let mut m = Molecule::default();
        benzene(&mut m, 0.0);
        assert_eq!(compute(&m, &config()).unwrap(), "C1=CC=CC=C1");
    }

    #[test]
    fn abbreviations_are_expanded() {
        // Methanol drawn as C–OMe is dimethyl ether, not a wildcard.
        let mut m = Molecule::default();
        let c = m.add_atom("C".into(), [0.0, 0.0], 0);
        let ome = m.add_atom("OMe".into(), [1.5, 0.0], 0);
        m.add_bond(c, ome, BondOrder::Single);
        assert_eq!(compute(&m, &config()).unwrap(), "COC");

        // Toluene drawn as Me–Ph equals toluene drawn in full.
        let mut drawn = Molecule::default();
        let me = drawn.add_atom("C".into(), [0.0, 0.0], 0);
        let ph = drawn.add_atom("Ph".into(), [1.5, 0.0], 0);
        drawn.add_bond(me, ph, BondOrder::Single);
        let mut full = Molecule::default();
        let ring = benzene(&mut full, 3.0);
        let me = full.add_atom("C".into(), [6.0, 0.0], 0);
        full.add_bond(ring[0], me, BondOrder::Single);
        let s = compute(&drawn, &config()).unwrap();
        assert!(!s.contains('*'), "{s}");
        assert_eq!(s, compute(&full, &config()).unwrap());
    }

    #[test]
    fn the_selection_scopes_an_expanded_abbreviation() {
        let mut m = Molecule::default();
        ethanol(&mut m, 0.0);
        let c = m.add_atom("C".into(), [10.0, 0.0], 0);
        let ph = m.add_atom("Ph".into(), [11.5, 0.0], 0);
        m.add_bond(c, ph, BondOrder::Single);
        let selected: HashSet<u32> = [ph].into_iter().collect();
        let mut toluene = Molecule::default();
        let ring = benzene(&mut toluene, 0.0);
        let me = toluene.add_atom("C".into(), [3.0, 0.0], 0);
        toluene.add_bond(ring[0], me, BondOrder::Single);
        assert_eq!(
            compute(&target(&m, &selected), &config()).unwrap(),
            compute(&toluene, &config()).unwrap()
        );
    }

    #[test]
    fn stereo_marks_are_not_written() {
        let plain = compute(&butan_2_ol(BondStereo::None), &config()).unwrap();
        assert!(!plain.contains('@'), "{plain}");
        for stereo in [BondStereo::WedgeUp, BondStereo::WedgeDown] {
            assert_eq!(compute(&butan_2_ol(stereo), &config()).unwrap(), plain);
        }
        let e = compute(&but_2_ene(true), &config()).unwrap();
        assert!(!e.contains('/') && !e.contains('\\'), "{e}");
        assert_eq!(e, compute(&but_2_ene(false), &config()).unwrap());
    }
}
