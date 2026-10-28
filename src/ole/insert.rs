//! Merging a drawing read from the clipboard (a CDX from ChemDraw or an Office object) into the
//! editor as one paste.
//!
//! The editor widget's own CDX paste keeps only atoms and bonds; this keeps the page objects
//! too (arrows, text, reaction tables, brackets), with every id they hold remapped into the
//! drawing, and the appearance (colours, shadows, ring fills). Like the widget's paste, the drawing is centred on the cursor (a CDX's absolute
//! page coordinates would otherwise land it far off-screen), selected, and undone in one step.

use std::collections::{HashMap, HashSet};

use chembuider_rs::ChemStructEditor;
use chembuider_rs::Molecule;
use chembuider_rs::molecule::page::PageObject;

/// Paste `pasted` into `editor`. Returns false (and changes nothing) when it is blank.
pub fn paste_drawing(editor: &mut ChemStructEditor, pasted: &Molecule) -> bool {
    let Some(center) = center(pasted) else {
        return false;
    };
    // The pasted atoms join the molecule a running cleanup is rewriting; stop it first.
    editor.cancel_cleanup();
    editor.push_undo();
    let target = editor.last_mouse_mol;
    let delta = [target[0] - center[0], target[1] - center[1]];
    let mol = &mut editor.molecule;

    let mut atoms = HashMap::new();
    for a in &pasted.atoms {
        let id = mol.add_atom(
            a.element.clone(),
            [a.pos[0] + delta[0], a.pos[1] + delta[1]],
            a.charge,
        );
        if let Some(atom) = mol.atom_by_id_mut(id) {
            atom.group_h = a.group_h;
            atom.color = a.color;
            atom.shadow = a.shadow;
        }
        atoms.insert(a.id, id);
    }
    for b in &pasted.bonds {
        let (Some(&begin), Some(&end)) = (atoms.get(&b.begin), atoms.get(&b.end)) else {
            continue;
        };
        let id = mol.add_bond(begin, end, b.order.clone());
        if let Some(bond) = mol.bond_by_id_mut(id) {
            bond.stereo = b.stereo.clone();
            bond.double_position = b.double_position;
            bond.color = b.color;
        }
    }
    for fill in &pasted.ring_fills {
        let ids: Option<Vec<u32>> = fill.atoms.iter().map(|a| atoms.get(a).copied()).collect();
        if let Some(ids) = ids {
            mol.set_ring_fill(ids, fill.color);
        }
    }

    // Arrows first, so the tables that name an arrow can be pointed at its new id.
    let mut objects = HashMap::new();
    let mut ordered: Vec<&PageObject> = pasted.objects.iter().collect();
    ordered.sort_by_key(|o| o.as_arrow().is_none());
    let mut new_objects = HashSet::new();
    for object in ordered {
        let mut object = object.clone();
        object.translate(delta);
        match &mut object {
            PageObject::Arrow(a) => a.source = None,
            PageObject::Text(t) => t.source = None,
            PageObject::Table(t) => {
                // The ChemDraw ids it was read with mean nothing in this drawing.
                t.source = None;
                t.arrow = t.arrow.and_then(|id| objects.get(&id).copied());
                for entry in t.reactants.iter_mut().chain(t.products.iter_mut()) {
                    entry.atom = entry.atom.and_then(|id| atoms.get(&id).copied());
                    entry.fragment = None;
                }
            }
            PageObject::Bracket(b) => {
                b.source = None;
                b.atoms = b
                    .atoms
                    .iter()
                    .filter_map(|id| atoms.get(id).copied())
                    .collect();
                if b.atoms.is_empty() {
                    continue;
                }
            }
        }
        let old = object.id();
        let id = mol.push_object(object);
        objects.insert(old, id);
        new_objects.insert(id);
    }

    editor.selected_atoms = atoms.into_values().collect();
    editor.selected_objects = new_objects;
    true
}

/// The point the paste is centred on: the atoms' centroid, or for a drawing of page objects
/// only, theirs. `None` for a blank drawing.
fn center(mol: &Molecule) -> Option<[f32; 2]> {
    let points: Vec<[f32; 2]> = if mol.atoms.is_empty() {
        mol.objects.iter().flat_map(|o| o.key_points()).collect()
    } else {
        mol.atoms.iter().map(|a| a.pos).collect()
    };
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f32;
    Some([
        points.iter().map(|p| p[0]).sum::<f32>() / n,
        points.iter().map(|p| p[1]).sum::<f32>() / n,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use chembuider_rs::BondOrder;
    use chembuider_rs::molecule::page::{ArrowKind, ReactionTable, TableEntry};

    /// Ethanol far out on a ChemDraw page, an arrow, a text and a table naming both.
    fn scheme() -> Molecule {
        let mut mol = Molecule::default();
        let c1 = mol.add_atom("C".into(), [100.0, 100.0], 0);
        let c2 = mol.add_atom("C".into(), [101.5, 100.0], 0);
        let o = mol.add_atom("O".into(), [103.0, 100.0], 0);
        mol.add_bond(c1, c2, BondOrder::Single);
        mol.add_bond(c2, o, BondOrder::Single);
        let arrow = mol.add_arrow([105.0, 100.0], [110.0, 100.0], ArrowKind::Reaction);
        mol.add_text([105.0, 99.0], "H_2O");
        mol.push_object(PageObject::Table(ReactionTable {
            id: 0,
            pos: [100.0, 104.0],
            arrow: Some(arrow),
            reactants: vec![TableEntry {
                atom: Some(c1),
                fragment: Some(77),
                ..Default::default()
            }],
            products: Vec::new(),
            source: Some(99),
        }));
        mol
    }

    #[test]
    fn pastes_atoms_and_page_objects_at_the_cursor_selected() {
        let mut editor = ChemStructEditor::default();
        let existing = editor.molecule.add_atom("N".into(), [0.0, 0.0], 0);
        editor.last_mouse_mol = [2.0, 3.0];
        assert!(paste_drawing(&mut editor, &scheme()));

        let mol = &editor.molecule;
        assert_eq!(mol.atoms.len(), 4);
        assert_eq!(mol.bonds.len(), 2);
        assert_eq!(mol.objects.len(), 3);
        // Centred on the cursor.
        let pasted: Vec<_> = mol.atoms.iter().filter(|a| a.id != existing).collect();
        let cx = pasted.iter().map(|a| a.pos[0]).sum::<f32>() / 3.0;
        let cy = pasted.iter().map(|a| a.pos[1]).sum::<f32>() / 3.0;
        assert!((cx - 2.0).abs() < 1e-4 && (cy - 3.0).abs() < 1e-4);
        // Selected: the pasted atoms and objects, not what was there.
        assert_eq!(editor.selected_atoms.len(), 3);
        assert!(!editor.selected_atoms.contains(&existing));
        assert_eq!(editor.selected_objects.len(), 3);
        // The table follows the new ids.
        let table = mol.objects.iter().find_map(|o| o.as_table()).unwrap();
        let arrow = mol.objects.iter().find_map(|o| o.as_arrow()).unwrap();
        assert_eq!(table.arrow, Some(arrow.id));
        assert_eq!(table.source, None);
        let atom = table.reactants[0].atom.unwrap();
        assert!(editor.selected_atoms.contains(&atom));
        assert_eq!(table.reactants[0].fragment, None);
        // One undo step takes it all back.
        assert!(editor.undo());
        assert_eq!(editor.molecule.atoms.len(), 1);
        assert!(editor.molecule.objects.is_empty());
    }

    /// The appearance comes along: the colours of the bonds, atom labels and texts, the
    /// structure's shadow and its ring fills, on the new ids.
    #[test]
    fn pastes_colours_shadows_and_ring_fills() {
        use chembuider_rs::molecule::Shadow;
        let mut pasted = Molecule::default();
        let ring: Vec<u32> = (0..3)
            .map(|k| {
                let angle = k as f32 * std::f32::consts::TAU / 3.0;
                pasted.add_atom("C".into(), [angle.cos(), angle.sin()], 0)
            })
            .collect();
        for k in 0..3 {
            pasted.add_bond(ring[k], ring[(k + 1) % 3], BondOrder::Single);
        }
        let shadow = Shadow {
            color: [10, 20, 30, 120],
            offset: [0.2, 0.3],
            blur: 0.25,
        };
        for a in &mut pasted.atoms {
            a.shadow = Some(shadow);
        }
        pasted.atoms[1].color = Some([0, 0, 255, 255]);
        pasted.bonds[2].color = Some([255, 0, 0, 255]);
        pasted.set_ring_fill(ring.clone(), [0, 200, 0, 100]);
        let text = pasted.add_text([3.0, 0.0], "THF");
        if let Some(PageObject::Text(t)) = pasted.objects.iter_mut().find(|o| o.id() == text) {
            t.color = Some([0, 128, 0, 255]);
        }

        let mut editor = ChemStructEditor::default();
        // Something already there, so the pasted ids differ from the originals.
        let a = editor.molecule.add_atom("N".into(), [9.0, 9.0], 0);
        let b = editor.molecule.add_atom("O".into(), [10.0, 9.0], 0);
        editor.molecule.add_bond(a, b, BondOrder::Single);
        assert!(paste_drawing(&mut editor, &pasted));

        let mol = &editor.molecule;
        let new: Vec<&chembuider_rs::molecule::Atom> = mol
            .atoms
            .iter()
            .filter(|x| x.id != a && x.id != b)
            .collect();
        assert_eq!(new.len(), 3);
        assert!(new.iter().all(|x| x.shadow == Some(shadow)));
        assert_eq!(
            new.iter().filter_map(|x| x.color).collect::<Vec<_>>(),
            [[0, 0, 255, 255]]
        );
        assert_eq!(mol.shadow_of(new[0].id), Some(shadow));
        assert_eq!(mol.shadow_of(a), None);
        let coloured: Vec<_> = mol.bonds.iter().filter_map(|x| x.color).collect();
        assert_eq!(coloured, [[255, 0, 0, 255]]);
        assert_eq!(mol.ring_fills.len(), 1);
        assert_eq!(mol.ring_fills[0].color, [0, 200, 0, 100]);
        assert!(
            mol.ring_fills[0]
                .atoms
                .iter()
                .all(|id| new.iter().any(|x| x.id == *id))
        );
        let text = mol.objects.iter().find_map(|o| o.as_text()).unwrap();
        assert_eq!(text.color, Some([0, 128, 0, 255]));
    }

    #[test]
    fn blank_drawing_pastes_nothing() {
        let mut editor = ChemStructEditor::default();
        assert!(!paste_drawing(&mut editor, &Molecule::default()));
        assert!(editor.undo_stack.is_empty());
    }
}
