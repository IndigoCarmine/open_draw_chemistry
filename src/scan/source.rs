//! The structure a scan evaluates at each grid point.

use crate::assembly::build;
use crate::assembly::model::Design;
use crate::three_d::geom::{Graph, Vec3};
use crate::three_d::{Atom3d, Bond3d, Model3d, edit};

use super::Param;
use super::energy;

/// What is scanned: the assembly's design, or the 3D model's coordinates.
#[derive(Debug, Clone)]
pub enum Source {
    /// The design as it was when the scan started; each point is that design with the
    /// parameters set, rebuilt.
    Assembly {
        design: Design,
        params: Vec<Param>,
        /// The elements and bonds of the design as built at the start, to tell a point whose
        /// atoms merely moved from one with other atoms.
        topology: Option<(Vec<String>, Vec<Bond3d>)>,
    },
    /// The model as it was when the scan started; each point edits its coordinates, the
    /// parameters set in turn (the smaller side of each moving), never moving `anchor` — nor,
    /// in a relaxed scan, the atoms fixed in the 3D view (see [`Source::at_from`]).
    Model {
        model: Model3d,
        graph: Graph,
        anchor: Option<usize>,
        params: Vec<Param>,
    },
}

/// The structure at one grid point.
#[derive(Debug, Clone, PartialEq)]
pub enum Point {
    /// The reference structure's atoms, moved: only the coordinates change.
    Coords(Vec<Vec3>),
    /// A structure with other atoms or bonds than the reference (a crystal whose copies on
    /// special positions changed with the cell): it has to be read afresh.
    Rebuilt {
        atoms: Vec<Atom3d>,
        bonds: Vec<Bond3d>,
    },
}

/// The structure the scan starts from, as OpenBabel reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub molfile: String,
    pub atoms: usize,
    /// Its coordinates.
    pub coords: Vec<Vec3>,
}

impl Source {
    pub fn assembly(design: &Design, params: Vec<Param>) -> Self {
        Source::Assembly {
            topology: design_topology(design),
            design: design.clone(),
            params,
        }
    }

    pub fn model(model: &Model3d, anchor: Option<usize>, params: Vec<Param>) -> Self {
        Source::Model {
            graph: edit::graph(model),
            model: model.clone(),
            anchor,
            params,
        }
    }

    pub fn params(&self) -> &[Param] {
        match self {
            Source::Assembly { params, .. } | Source::Model { params, .. } => params,
        }
    }

    /// How many atoms the structure has now (the design as built).
    pub fn atom_count(&self) -> usize {
        match self {
            Source::Assembly { design, .. } => design.atom_count(),
            Source::Model { model, .. } => model.atoms.len(),
        }
    }

    /// The reference structure: the design built as it is, or the model's own molfile.
    pub fn reference(&self) -> Result<Reference, String> {
        match self {
            Source::Assembly { design, .. } => {
                let built = build::build(design, 0).map_err(|e| e.to_string())?;
                if built.is_empty() {
                    return Err(t!("scan.error.empty_assembly").into());
                }
                Ok(Reference {
                    molfile: energy::molfile(&built.atoms, &built.bonds),
                    atoms: built.atoms.len(),
                    coords: built.atoms.iter().map(|a| a.pos).collect(),
                })
            }
            Source::Model { model, .. } => Ok(Reference {
                molfile: model.molfile.clone(),
                atoms: model.atoms.len(),
                coords: edit::coords(model),
            }),
        }
    }

    /// The structure with the parameters at `values` (one per parameter, in order).
    pub fn at(&self, values: &[f64]) -> Result<Point, String> {
        match self {
            Source::Assembly {
                design,
                params,
                topology,
            } => {
                let mut d = design.clone();
                for (p, &v) in params.iter().zip(values) {
                    p.write_design(&mut d, v).map_err(|e| e.to_string())?;
                }
                let built = build::build(&d, 0).map_err(|e| e.to_string())?;
                let same = topology.as_ref().is_some_and(|(elements, bonds)| {
                    elements.len() == built.atoms.len()
                        && elements
                            .iter()
                            .zip(&built.atoms)
                            .all(|(e, a)| *e == a.element)
                        && *bonds == built.bonds
                });
                if same {
                    Ok(Point::Coords(built.atoms.iter().map(|a| a.pos).collect()))
                } else {
                    Ok(Point::Rebuilt {
                        atoms: built.atoms,
                        bonds: built.bonds,
                    })
                }
            }
            Source::Model { model, .. } => self.at_from(&edit::coords(model), values, &[]),
        }
    }

    /// The model's atoms at `from` (another structure of them, such as a neighbouring point's
    /// relaxed one) with the parameters set to `values`, none of the `fixed` atoms moving (see
    /// [`Source::pinned`]). The assembly's points are always rebuilt from its design, so for
    /// it this is [`Source::at`].
    pub fn at_from(&self, from: &[Vec3], values: &[f64], fixed: &[usize]) -> Result<Point, String> {
        match self {
            Source::Assembly { .. } => self.at(values),
            Source::Model { graph, params, .. } => {
                let mut coords = from.to_vec();
                for (p, &v) in params.iter().zip(values) {
                    coords = p
                        .write_coords(&coords, graph, v, &self.pinned(p, fixed)?)
                        .map_err(|e| e.to_string())?;
                }
                Ok(Point::Coords(coords))
            }
        }
    }

    /// The atoms setting the model's parameter `param` keeps still: the `fixed` ones (those a
    /// relaxed scan holds where they were when it started) and the anchor — or the `fixed`
    /// ones alone when with the anchor no side is free, for the minimiser moves the anchor
    /// anyway. Fails when the `fixed` atoms are on both sides of the parameter.
    fn pinned(&self, param: &Param, fixed: &[usize]) -> Result<Vec<usize>, String> {
        let Source::Model {
            model,
            graph,
            anchor,
            ..
        } = self
        else {
            return Ok(Vec::new());
        };
        let mut pinned = fixed.to_vec();
        pinned.extend(anchor.filter(|a| !fixed.contains(a)));
        if !param.blocked_by(graph, &pinned) {
            Ok(pinned)
        } else if !param.blocked_by(graph, fixed) {
            Ok(fixed.to_vec())
        } else {
            Err(t!(
                "scan.window.relax_fixed_both_sides",
                param = param.label(None, Some(model))
            ))
        }
    }

    /// Why the parameters cannot be set at all without moving one of the `fixed` atoms, if
    /// they cannot (checked before a relaxed scan starts; which side moves does not depend on
    /// the values).
    pub fn fixed_conflict(&self, fixed: &[usize]) -> Option<String> {
        self.params()
            .iter()
            .find_map(|p| self.pinned(p, fixed).err())
    }
}

/// The elements and bonds of `design` as built.
fn design_topology(design: &Design) -> Option<(Vec<String>, Vec<Bond3d>)> {
    let built = build::build(design, 0).ok()?;
    Some((
        built.atoms.into_iter().map(|a| a.element).collect(),
        built.bonds,
    ))
}
