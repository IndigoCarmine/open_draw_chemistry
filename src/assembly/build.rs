//! Turning a [`Design`] into atoms: every placed molecule's atoms moved into place, with the
//! monomer's bonds copied (never perceived) and each atom's molecule recorded.
//!
//! [`AssemblyState::built`] keeps the last result and rebuilds only when the state's revision
//! changes — sp_builder's "rebuild when the graph signature changes" — so the viewer and the
//! panel can ask for it every frame.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use super::math::Cell;
use super::model::{AssemblyState, Design, OrientError};
use crate::three_d::{Atom3d, Bond3d};

/// The most atoms an assembly may have. Beyond this the viewer and the exports become
/// impractical, so the build is refused rather than freezing the app.
pub const MAX_ATOMS: usize = 200_000;

/// A built assembly. Molecules are contiguous runs of atoms, in the order the design places
/// them (see [`Design::instances`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Built {
    pub atoms: Vec<Atom3d>,
    pub bonds: Vec<Bond3d>,
    /// The molecule each atom belongs to.
    pub molecule_of_atom: Vec<usize>,
    /// The first atom of each molecule, and the atom count at the end.
    pub molecule_start: Vec<usize>,
    /// The component each molecule is a copy of.
    pub component_of_molecule: Vec<usize>,
    /// The unit cell, for a crystal (A2).
    pub cell: Option<Cell>,
    /// The state's revision this was built at.
    pub revision: u64,
}

impl Built {
    pub fn molecule_count(&self) -> usize {
        self.component_of_molecule.len()
    }

    /// The atoms of molecule `m`.
    pub fn molecule_atoms(&self, m: usize) -> Range<usize> {
        self.molecule_start[m]..self.molecule_start[m + 1]
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }
}

/// Why an assembly could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// More atoms than [`MAX_ATOMS`] (the count it would have).
    TooLarge(usize),
    /// A component's orientation atoms are unusable.
    Orient {
        component: String,
        error: OrientError,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::TooLarge(n) => write!(
                f,
                "原子が多すぎます（{n} 個。組み上げられるのは {MAX_ATOMS} 個までです）。繰り返し数を減らしてください"
            ),
            BuildError::Orient { component, error } => {
                write!(f, "単量体「{component}」: {error}")
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// Build `design`, tagging the result with `revision`.
pub fn build(design: &Design, revision: u64) -> Result<Built, BuildError> {
    let atom_count = design.atom_count();
    if atom_count > MAX_ATOMS {
        return Err(BuildError::TooLarge(atom_count));
    }
    // Each component's oriented coordinates, once.
    let oriented = design
        .components
        .iter()
        .map(|c| {
            c.oriented_positions().map_err(|error| BuildError::Orient {
                component: c.name.clone(),
                error,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let instances = design.instances();
    let bond_count: usize = instances
        .iter()
        .filter_map(|i| design.components.get(i.component))
        .map(|c| c.bonds.len())
        .sum();
    let mut built = Built {
        atoms: Vec::with_capacity(atom_count),
        bonds: Vec::with_capacity(bond_count),
        molecule_of_atom: Vec::with_capacity(atom_count),
        molecule_start: Vec::with_capacity(instances.len() + 1),
        component_of_molecule: Vec::with_capacity(instances.len()),
        cell: None,
        revision,
    };
    for instance in &instances {
        let Some(component) = design.components.get(instance.component) else {
            continue;
        };
        let m = built.component_of_molecule.len();
        let first = built.atoms.len();
        built.molecule_start.push(first);
        built.component_of_molecule.push(instance.component);
        for (atom, &pos) in component.atoms.iter().zip(&oriented[instance.component]) {
            built.atoms.push(Atom3d {
                element: atom.element.clone(),
                pos: instance.transform.apply(pos),
            });
            built.molecule_of_atom.push(m);
        }
        let n = component.atoms.len();
        built.bonds.extend(
            component
                .bonds
                .iter()
                .filter(|b| b.begin < n && b.end < n)
                .map(|b| Bond3d {
                    begin: first + b.begin,
                    end: first + b.end,
                    order: b.order,
                }),
        );
    }
    built.molecule_start.push(built.atoms.len());
    Ok(built)
}

/// The last build and the revision it was made at.
pub(super) type Cached = (u64, Result<Arc<Built>, BuildError>);

impl AssemblyState {
    /// The assembly as it is now, rebuilt only when the design has changed since last asked.
    pub fn built(&mut self) -> Result<Arc<Built>, BuildError> {
        let revision = self.revision();
        match &self.cache {
            Some((at, result)) if *at == revision => result.clone(),
            _ => {
                let result = build(&self.design, revision).map(Arc::new);
                if let (Ok(built), Some(m)) = (&result, self.selected)
                    && m >= built.molecule_count()
                {
                    self.selected = None;
                }
                self.cache = Some((revision, result.clone()));
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::math::{self, Rigid};
    use crate::assembly::model::{Component, Instance, Step};

    fn methanol_like() -> Component {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        Component {
            name: "meoh".into(),
            atoms: vec![
                atom("C", [0.0, 0.0, 0.0]),
                atom("O", [1.4, 0.0, 0.0]),
                atom("H", [1.8, 0.9, 0.0]),
            ],
            bonds: vec![
                Bond3d {
                    begin: 0,
                    end: 1,
                    order: 1,
                },
                Bond3d {
                    begin: 1,
                    end: 2,
                    order: 1,
                },
            ],
            orient: None,
        }
    }

    #[test]
    fn every_copy_gets_its_atoms_and_its_own_bonds() {
        let design = Design {
            components: vec![methanol_like()],
            base: 0,
            steps: vec![Step::new_ring(), Step::new_stack()],
            manual: vec![Instance {
                component: 0,
                transform: Rigid::translation([0.0, 0.0, -10.0]),
            }],
        };
        let built = build(&design, 7).unwrap();
        assert_eq!(built.revision, 7);
        assert_eq!(built.molecule_count(), 61);
        assert_eq!(built.atoms.len(), 183);
        assert_eq!(built.bonds.len(), 122);
        assert_eq!(built.molecule_start.len(), 62);
        for (m, range) in (0..built.molecule_count()).map(|m| (m, built.molecule_atoms(m))) {
            assert_eq!(range.len(), 3);
            assert!(range.clone().all(|a| built.molecule_of_atom[a] == m));
        }
        // Bonds stay inside their molecule and keep their length.
        for b in &built.bonds {
            assert_eq!(
                built.molecule_of_atom[b.begin],
                built.molecule_of_atom[b.end]
            );
        }
        let d = math::distance(built.atoms[30].pos, built.atoms[31].pos);
        assert!((d - 1.4).abs() < 1e-9);
        // The hand-placed molecule is last.
        assert!((built.atoms[180].pos[2] + 10.0).abs() < 1e-9);
    }

    #[test]
    fn too_many_atoms_and_bad_orientations_are_refused_in_japanese() {
        let mut design = Design {
            components: vec![methanol_like()],
            steps: vec![Step::new_stack(); 6],
            ..Default::default()
        };
        // 10⁶ molecules.
        let err = build(&design, 1).unwrap_err();
        assert_eq!(err, BuildError::TooLarge(3_000_000));
        assert!(err.to_string().contains("多すぎます"));
        design.steps.truncate(1);
        design.components[0].orient = Some([0, 1, 1]);
        let err = build(&design, 1).unwrap_err();
        assert!(err.to_string().contains("meoh"), "{err}");
    }

    #[test]
    fn the_build_is_cached_by_revision() {
        let mut state = AssemblyState::with_design(Design {
            components: vec![methanol_like()],
            steps: vec![Step::new_ring()],
            ..Default::default()
        });
        let a = state.built().unwrap();
        let b = state.built().unwrap();
        assert!(Arc::ptr_eq(&a, &b), "unchanged: the same build");
        state.selected = Some(5);
        state.edit(|d| d.steps.clear());
        let c = state.built().unwrap();
        assert_eq!(c.molecule_count(), 1);
        assert_eq!(state.selected, None, "the selection went with its molecule");
    }
}
