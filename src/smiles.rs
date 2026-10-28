//! The SMILES of the selected structures (or of the whole drawing), for the 解析 panel.
//!
//! With OpenBabel (the `openbabel` feature) the drawing goes through [`molfile::to_molfile`],
//! which keeps wedges and hashes and expands abbreviations, and OpenBabel writes a canonical
//! **isomeric** SMILES: it perceives tetrahedral centres from the 2D coordinates plus wedges,
//! and double-bond geometry from the coordinates. Without OpenBabel, or if it fails, the
//! library's own [`chembuider_rs::molecule::smiles::to_smiles`] is used — canonical too, but
//! with no stereochemistry, which the panel says.

use std::collections::HashSet;

use chembuider_rs::molecule::smiles::to_smiles;
use chembuider_rs::widget::analysis::analysis_target;
use chembuider_rs::{BondOrder, Config, Molecule};

use crate::three_d::Error;
use crate::three_d::molfile::expand_labels;

/// A computed SMILES.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Smiles {
    pub text: String,
    /// Whether the string carries stereochemistry (OpenBabel's isomeric SMILES); `false` for
    /// the stereo-less fallback.
    pub isomeric: bool,
}

/// The structures the SMILES is of: those with a selected atom, or everything drawn. Page
/// objects (arrows, text, tables) play no part.
pub fn target(mol: &Molecule, selection: &HashSet<u32>) -> Molecule {
    let mut target = analysis_target(mol, selection);
    target.objects.clear();
    target.ring_fills.clear();
    target
}

/// The SMILES of `mol` (already the [`target`]), or a short Japanese reason why there is none.
/// May be slow for a big drawing with OpenBabel: call it off the UI thread.
pub fn compute(mol: &Molecule, config: &Config) -> Result<Smiles, String> {
    if mol.atoms.is_empty() {
        return Err("構造がありません".to_owned());
    }
    if mol.bonds.iter().any(|b| b.order == BondOrder::Quadruple) {
        return Err("四重結合は SMILES に対応していません".to_owned());
    }
    match openbabel_smiles(mol, config) {
        Ok(text) => Ok(Smiles {
            text,
            isomeric: true,
        }),
        // These are properties of the drawing, so the fallback would fail on them too.
        Err(Error::UnknownLabels(labels)) => Err(format!(
            "SMILES にできないラベルがあります: {}",
            labels.join("、")
        )),
        Err(_) => fallback(mol, config),
    }
}

/// The library's stereo-less SMILES, abbreviations expanded first.
fn fallback(mol: &Molecule, config: &Config) -> Result<Smiles, String> {
    let expanded = expand_labels(mol, config).map_err(|e| match e {
        Error::UnknownLabels(labels) => {
            format!("SMILES にできないラベルがあります: {}", labels.join("、"))
        }
        other => other.to_string(),
    })?;
    let text = to_smiles(&expanded);
    if text.is_empty() {
        return Err("SMILES を生成できませんでした".to_owned());
    }
    Ok(Smiles {
        text,
        isomeric: false,
    })
}

#[cfg(feature = "openbabel")]
fn openbabel_smiles(mol: &Molecule, config: &Config) -> Result<String, Error> {
    let molfile = crate::three_d::molfile::to_molfile(mol, config)?;
    let ob = openbabel::Molecule::parse(&molfile, "mol").map_err(|_| Error::Unreadable)?;
    // "can" is canonical and, since OpenBabel 3, isomeric by default. It writes
    // "<smiles>\t<title>\n"; only the first field is the SMILES.
    let out = ob.write("can").map_err(|_| Error::Unreadable)?;
    out.split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or(Error::Unreadable)
}

#[cfg(not(feature = "openbabel"))]
fn openbabel_smiles(mol: &Molecule, config: &Config) -> Result<String, Error> {
    // Still surface unknown labels the same way as the OpenBabel path.
    expand_labels(mol, config)?;
    Err(Error::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chembuider_rs::BondStereo;

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
        assert_eq!(compute(&m, &config()).unwrap().text, "CCO");
    }

    #[test]
    fn two_molecules_are_dot_joined() {
        let mut m = Molecule::default();
        ethanol(&mut m, 0.0);
        m.add_atom("N".into(), [10.0, 0.0], 0);
        let s = compute(&m, &config()).unwrap().text;
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
        assert_eq!(
            compute(&target(&m, &selected), &config()).unwrap().text,
            "N"
        );
        let all = target(&m, &HashSet::new());
        assert_eq!(all.atoms.len(), 4);
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
        assert!(compute(&q, &config()).unwrap_err().contains("四重結合"));
        assert!(compute(&Molecule::default(), &config()).is_err());
    }

    #[cfg(feature = "openbabel")]
    fn canonical(smiles: &str) -> String {
        let ob = openbabel::Molecule::parse(smiles, "smi").unwrap();
        ob.write("can")
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    }

    #[cfg(feature = "openbabel")]
    #[test]
    fn a_wedge_gives_the_drawn_configuration() {
        let r = compute(&butan_2_ol(BondStereo::WedgeUp), &config()).unwrap();
        assert!(r.isomeric);
        assert!(r.text.contains('@'), "{}", r.text);
        // (R)-butan-2-ol.
        assert_eq!(r.text, canonical("C[C@@H](O)CC"));
        let s = compute(&butan_2_ol(BondStereo::WedgeDown), &config()).unwrap();
        assert_eq!(s.text, canonical("C[C@H](O)CC"));
        assert_ne!(r.text, s.text);
        // No wedge: no stereo.
        let plain = compute(&butan_2_ol(BondStereo::None), &config()).unwrap();
        assert!(!plain.text.contains('@'), "{}", plain.text);
    }

    #[cfg(feature = "openbabel")]
    #[test]
    fn double_bond_geometry_is_kept() {
        let e = compute(&but_2_ene(true), &config()).unwrap().text;
        let z = compute(&but_2_ene(false), &config()).unwrap().text;
        assert!(e.contains('/') || e.contains('\\'), "{e}");
        assert_eq!(e, canonical("C/C=C/C"));
        assert_eq!(z, canonical("C/C=C\\C"));
    }

    #[cfg(not(feature = "openbabel"))]
    #[test]
    fn without_openbabel_the_fallback_has_no_stereo() {
        let r = compute(&butan_2_ol(BondStereo::WedgeUp), &config()).unwrap();
        assert!(!r.isomeric);
        assert!(!r.text.contains('@'));
        let e = compute(&but_2_ene(true), &config()).unwrap();
        assert!(!e.isomeric);
        assert_eq!(e.text, "CC=CC");
    }
}
