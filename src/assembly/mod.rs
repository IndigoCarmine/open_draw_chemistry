//! The molecular assembly builder (分子集合体ビルダー): monomers copied into rings, stacks,
//! helices and rows, built into one structure and written out.
//!
//! Pure code — no egui, no OpenBabel — so it is tested on hand-built structures:
//!
//! - [`math`]: rigid transforms and the rotations they are made from;
//! - [`model`]: the monomers, the steps, the design and its undo history;
//! - [`build`]: the design turned into atoms and bonds, cached by revision;
//! - [`export`]: PDB, XYZ and MOL2 writers;
//! - [`file`]: the `.odasm` file the design is saved in.
//!
//! The geometry is ported from IndigoCarmine's own tools (cited where it is used):
//! build_md_rs (rosettes, stacks, orientation by three atoms), gmx_harness (`align`) and
//! sp_builder (the row of copies, rebuilding only on change). Crystals (a lattice step with
//! symmetry operations) and free placement of molecules by hand follow in later phases; the
//! types leave room for them ([`model::Design::manual`], [`build::Built::cell`]).

pub mod build;
pub mod export;
pub mod file;
pub mod math;
pub mod model;

pub use build::{BuildError, Built, MAX_ATOMS};
pub use export::AssemblyExport;
pub use model::{AssemblyState, Component, Design, Instance, Step};
