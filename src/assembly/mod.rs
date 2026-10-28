//! The molecular assembly builder (分子集合体ビルダー): monomers copied into rings, stacks,
//! helices and rows, built into one structure and written out.
//!
//! Pure code — no egui, no OpenBabel — so it is tested on hand-built structures:
//!
//! - [`math`]: rigid transforms, the rotations they are made from, and the unit cell;
//! - [`symop`]: crystallographic symmetry operations (`-x,y+1/2,-z+1/2`) and space groups;
//! - [`model`]: the monomers, the steps, the design and its undo history;
//! - [`build`]: the design turned into atoms and bonds, cached by revision;
//! - [`place`]: molecules placed by hand — their pose, mouse moves, a free spot for a new one;
//! - [`clash`]: atoms of different molecules too close, by a cell list (periodic or not);
//! - [`measure`]: atoms picked across molecules for a distance, an angle or a dihedral;
//! - [`export`]: PDB, XYZ, MOL2 and CIF writers;
//! - [`file`]: the `.odasm` file the design is saved in.
//!
//! The geometry is ported from IndigoCarmine's own tools (cited where it is used):
//! build_md_rs (rosettes, stacks, orientation by three atoms), gmx_harness (`align`) and
//! sp_builder (the row of copies, rebuilding only on change). A crystal is a lattice step
//! ([`model::Lattice`]): the cell constants and a space group's operations copy everything
//! built before it.

pub mod build;
pub mod clash;
pub mod export;
pub mod file;
pub mod math;
pub mod measure;
pub mod model;
pub mod place;
pub mod symop;

pub use build::{BuildError, Built, MAX_ATOMS};
pub use export::AssemblyExport;
pub use model::{AssemblyState, Component, Design, Instance, Step};
