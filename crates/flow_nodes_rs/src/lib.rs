//! A foundation for Blender-style node graphs.
//!
//! An application describes its kinds of node — their sockets, the type of data each carries,
//! the values of inputs no wire feeds — by implementing [`NodeKind`] for its own type (usually
//! an `enum`), and how each kind computes by implementing [`Evaluate`]. In return it gets,
//! for any such kinds:
//!
//! - [`graph`]: graphs of nodes and wires that save as plain serde data;
//! - [`ops`]: the edits a node editor makes — connect (type-checked, never looping),
//!   disconnect, delete (joining around the deleted node), duplicate, group into a custom node
//!   and ungroup, expose an input, lay out;
//! - [`def`]: custom nodes — a graph behind one node, with ports of its own, nested and used
//!   anywhere — and the fingerprint telling a copy from the library's;
//! - [`eval`]: a pull evaluator — each node reached computed once, bypassed and failing nodes
//!   passing their data through, custom nodes evaluated inside, loops and recursion reported;
//! - [`expr`] and [`field`]: number expressions, and values per element;
//! - [`library`] (feature `library`): custom nodes kept on disk;
//! - [`editor`] (feature `egui`): a node editor widget that reports edits instead of making
//!   them, so the application keeps its own undo.
//!
//! The crate knows no language: its errors are values ([`ops::ConnectError`],
//! [`eval::Problem`], …) with English `Display`s, and every label comes from the application.

pub mod def;
#[cfg(feature = "egui")]
pub mod editor;
pub mod eval;
pub mod expr;
pub mod field;
pub mod graph;
pub mod id;
#[cfg(feature = "library")]
pub mod library;
pub mod ops;
pub mod schema;
pub mod scope;

pub use def::{GroupRef, LibraryStamp, NodeDef, Port, PortOf, Status};
pub use eval::{EvalCx, Evaluate, Issue, Outcome, Problem};
pub use graph::{Graph, Link, LinkOf, Node};
pub use id::{DefId, NodeId};
pub use schema::{
    InDecl, InDeclOf, Key, NodeKind, OutDecl, OutDeclOf, Role, SocketType, ValueHint,
};
pub use scope::Scope;

/// The usual distance between neighbouring nodes, left to right, in canvas units: where new
/// group input and output nodes go, and the columns of [`ops::layout`].
pub const SPACING: f32 = 220.0;

#[cfg(test)]
mod tests;
