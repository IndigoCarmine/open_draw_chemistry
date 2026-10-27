//! Which part of a drawing becomes a 3D model: the drawing's molecules (connected components)
//! and the sub-drawing holding some of them.
//!
//! 3D化 on the canvas turns one molecule of a drawing into a model — the molecules holding the
//! selected atoms, or the one under the pointer. The [`super::Job`] remembers that part by the
//! atom ids it started from ([`Scope`]) and tells whether it is stale by extracting the same part
//! of the current drawing again, so editing another molecule of the drawing leaves the model
//! current while growing, shrinking or changing its own molecule makes it stale.

use std::collections::{BTreeSet, HashMap};

use chembuider_rs::Molecule;

/// The part of a drawing a model is made from: the molecules holding these atoms (every atom
/// of those molecules when the model was made, sorted). Kept as atom ids rather than as the
/// molecules themselves, so the part can be found again in an edited drawing — even after
/// some of its atoms are deleted.
pub type Scope = Vec<u32>;

/// The ids of every atom bonded, directly or through others, to one of `seeds`: the molecules
/// they belong to. Seeds not in the drawing are ignored.
pub fn components(drawing: &Molecule, seeds: &[u32]) -> BTreeSet<u32> {
    let mut neighbours: HashMap<u32, Vec<u32>> = HashMap::new();
    for bond in &drawing.bonds {
        neighbours.entry(bond.begin).or_default().push(bond.end);
        neighbours.entry(bond.end).or_default().push(bond.begin);
    }
    let mut found = BTreeSet::new();
    let mut stack: Vec<u32> = seeds
        .iter()
        .copied()
        .filter(|&id| drawing.atom_by_id(id).is_some())
        .collect();
    while let Some(id) = stack.pop() {
        if found.insert(id) {
            stack.extend(
                neighbours
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .filter(|n| !found.contains(n)),
            );
        }
    }
    found
}

/// The part of `drawing` made of the atoms in `atoms` and the bonds between them; atoms keep
/// their ids. Ring shading entirely inside the part is kept.
///
/// The part is built afresh rather than cut from a clone, so it depends only on those atoms
/// and bonds: the drawing's id allocator (which counts every atom ever added, and takes part
/// in `==`) is not carried over. The part is for reading, not for adding to.
pub fn extract(drawing: &Molecule, atoms: &BTreeSet<u32>) -> Molecule {
    let mut part = Molecule::default();
    part.name = drawing.name.clone();
    part.atoms = drawing
        .atoms
        .iter()
        .filter(|a| atoms.contains(&a.id))
        .cloned()
        .collect();
    part.bonds = drawing
        .bonds
        .iter()
        .filter(|b| atoms.contains(&b.begin) && atoms.contains(&b.end))
        .cloned()
        .collect();
    part.ring_fills = drawing
        .ring_fills
        .iter()
        .filter(|f| f.atoms.iter().all(|id| atoms.contains(id)))
        .cloned()
        .collect();
    part
}

/// The molecules of `drawing` holding the atoms of `scope`, as they are now (the whole drawing
/// for no scope). Empty when none of them is left.
pub fn scoped(drawing: &Molecule, scope: Option<&[u32]>) -> Molecule {
    match scope {
        None => drawing.clone(),
        Some(seeds) => extract(drawing, &components(drawing, seeds)),
    }
}

/// What 3D化 turns into a model, given the canvas's selection and the atom right-clicked on:
/// the molecules holding the selected atoms, else the molecule clicked on, else the whole
/// drawing (`Some(None)`). `None` when there is nothing drawn.
///
/// A part that is the whole drawing anyway is returned as `Some(None)`, so the model tracks
/// the drawing as a whole.
pub fn target(
    drawing: &Molecule,
    selected: impl IntoIterator<Item = u32>,
    clicked: Option<u32>,
) -> Option<Option<Scope>> {
    if drawing.atoms.is_empty() {
        return None;
    }
    let mut seeds: Vec<u32> = selected
        .into_iter()
        .filter(|&id| drawing.atom_by_id(id).is_some())
        .collect();
    if seeds.is_empty() {
        seeds.extend(clicked.filter(|&id| drawing.atom_by_id(id).is_some()));
    }
    if seeds.is_empty() {
        return Some(None);
    }
    let atoms = components(drawing, &seeds);
    if atoms.len() == drawing.atoms.len() {
        return Some(None);
    }
    Some(Some(atoms.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chembuider_rs::BondOrder;

    /// Ethanol (C–C–O) and, apart from it, methylamine (C–N). Returns the drawing and the
    /// ids [c1, c2, o, c3, n].
    fn two_molecules() -> (Molecule, [u32; 5]) {
        let mut mol = Molecule::default();
        let c1 = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let c2 = mol.add_atom("C".to_owned(), [1.3, 0.0], 0);
        let o = mol.add_atom("O".to_owned(), [2.0, 1.0], 0);
        mol.add_bond(c1, c2, BondOrder::Single);
        mol.add_bond(c2, o, BondOrder::Single);
        let c3 = mol.add_atom("C".to_owned(), [10.0, 0.0], 0);
        let n = mol.add_atom("N".to_owned(), [11.3, 0.0], 0);
        mol.add_bond(c3, n, BondOrder::Single);
        (mol, [c1, c2, o, c3, n])
    }

    #[test]
    fn the_selection_picks_the_molecules_holding_it() {
        let (mol, [c1, c2, o, c3, n]) = two_molecules();
        // One selected atom brings its whole molecule.
        let scope = target(&mol, [o], None).unwrap().unwrap();
        assert_eq!(scope, vec![c1, c2, o]);
        let part = scoped(&mol, Some(&scope));
        let ids: BTreeSet<u32> = part.atoms.iter().map(|a| a.id).collect();
        assert_eq!(ids, BTreeSet::from([c1, c2, o]));
        assert_eq!(part.bonds.len(), 2);
        // The selection wins over the atom clicked on.
        assert_eq!(target(&mol, [c2], Some(n)), Some(Some(vec![c1, c2, o])));
        // Atoms of both molecules selected: that is the whole drawing.
        assert_eq!(target(&mol, [c1, c3], None), Some(None));
    }

    #[test]
    fn without_a_selection_the_molecule_clicked_on_is_taken() {
        let (mol, [_, _, _, c3, n]) = two_molecules();
        let scope = target(&mol, [], Some(n)).unwrap().unwrap();
        let part = scoped(&mol, Some(&scope));
        let ids: BTreeSet<u32> = part.atoms.iter().map(|a| a.id).collect();
        assert_eq!(ids, BTreeSet::from([c3, n]));
        assert_eq!(part.bonds.len(), 1);
        // Clicked on nothing (or an atom since deleted): the whole drawing.
        assert_eq!(target(&mol, [], None), Some(None));
        assert_eq!(target(&mol, [999], Some(998)), Some(None));
    }

    #[test]
    fn an_empty_drawing_has_nothing_to_turn_into_3d() {
        assert_eq!(target(&Molecule::default(), [], None), None);
        assert_eq!(target(&Molecule::default(), [1], Some(2)), None);
    }

    #[test]
    fn a_single_molecule_drawing_is_taken_whole() {
        let mut mol = Molecule::default();
        let a = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
        let b = mol.add_atom("O".to_owned(), [1.3, 0.0], 0);
        mol.add_bond(a, b, BondOrder::Single);
        assert_eq!(target(&mol, [a], Some(b)), Some(None));
        assert_eq!(scoped(&mol, None), mol);
    }

    /// The part found again in an edited drawing: unchanged by edits elsewhere, changed by
    /// edits to its own molecule.
    #[test]
    fn the_part_follows_its_own_molecule_only() {
        let (mut mol, [c1, c2, o, c3, _]) = two_molecules();
        let scope = vec![c1, c2, o];
        let before = scoped(&mol, Some(&scope));

        // Another molecule edited: the part is the same.
        let far = mol.add_atom("Cl".to_owned(), [12.0, 0.0], 0);
        mol.add_bond(c3, far, BondOrder::Single);
        assert_eq!(scoped(&mol, Some(&scope)), before);

        // Its own molecule grows: it differs.
        let mut grown = mol.clone();
        let extra = grown.add_atom("N".to_owned(), [3.0, 1.0], 0);
        grown.add_bond(o, extra, BondOrder::Single);
        assert_ne!(scoped(&grown, Some(&scope)), before);

        // Joined to the other molecule: it differs.
        let mut joined = mol.clone();
        joined.add_bond(c2, c3, BondOrder::Single);
        assert_ne!(scoped(&joined, Some(&scope)), before);

        // One of its atoms deleted: the rest is still found, and differs.
        let mut gone = mol.clone();
        gone.remove_atom(o);
        let rest = scoped(&gone, Some(&scope));
        assert_eq!(rest.atoms.len(), 2);
        assert_ne!(rest, before);
    }
}
