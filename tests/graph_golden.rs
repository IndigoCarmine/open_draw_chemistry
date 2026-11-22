//! The node graph's saved form and what it builds, pinned: a version 5 design using most kinds
//! of node, a custom node taken from the library and an `.odnode` file, written before the
//! node machinery moved into the `flow_nodes_rs` crate. Opening and saving them again must give
//! the same bytes, the same custom-node fingerprints and the same atoms.

use open_draw_chemistry::assembly::graph::{defs, library};
use open_draw_chemistry::assembly::{build, file};
use sha2::{Digest, Sha256};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/graph_golden")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name))
        .unwrap()
        .replace("\r\n", "\n")
}

/// The built atoms, bonds and molecules, rounded and hashed.
fn digest(design: &open_draw_chemistry::assembly::Design) -> String {
    let built = build::build(design, 1).unwrap();
    let mut text = String::new();
    for a in &built.atoms {
        text += &format!(
            "{} {:.6} {:.6} {:.6}\n",
            a.element, a.pos[0], a.pos[1], a.pos[2]
        );
    }
    for b in &built.bonds {
        text += &format!("{} {} {}\n", b.begin, b.end, b.order);
    }
    text += &format!("{:?}\n{:?}\n", built.molecule_start, built.component_of_molecule);
    text += &format!("{:?}\n", built.crystal.as_ref().map(|c| (c.z, c.reps)));
    let ev = design.evaluation();
    text += &format!("errors {} warnings {}\n", ev.errors.len(), ev.warnings.len());
    let hash = Sha256::digest(text.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("atoms {}\nmolecules {}\nbuild {hex}\n", built.atoms.len(), built.molecule_count())
}

/// `actual` against fixture `name`; with `GOLDEN_WRITE=1`, the fixture is written instead (only
/// for a deliberate change of format, never to make a refactor pass).
fn check(name: &str, actual: &str) {
    if std::env::var_os("GOLDEN_WRITE").is_some() {
        std::fs::write(fixture(name), actual).unwrap();
        return;
    }
    assert_eq!(actual, read(name), "{name}");
}

fn hashes(defs_: &[defs::NodeDef]) -> String {
    defs_
        .iter()
        .map(|d| format!("{} {}\n", d.id.0, defs::content_hash(d)))
        .collect()
}

#[test]
fn a_saved_graph_reads_and_writes_back_unchanged() {
    let text = read("design.odasm");
    let design = file::from_str(&text).unwrap();
    assert_eq!(file::to_string(&design), text, "saved bytes");
    check("design.hashes", &hashes(&design.defs));
    check("design.digest", &digest(&design));
}

#[test]
fn a_library_file_reads_and_writes_back_unchanged() {
    let text = read("ring.odnode");
    let entry = library::from_str(&text).unwrap();
    let mut all = entry.deps.clone();
    all.push(entry.def.clone());
    assert_eq!(library::to_string(&entry.def, &all), text, "saved bytes");
    check("ring.hashes", &hashes(&all));
}
