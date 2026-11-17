//! Expanding fragment labels (`x` on an atom labelled `Ph`, `OMe`, …) in a document as the app
//! shows it: with the fragment list the app hands the editor, through the document's own
//! `show`, driven by the key events a keyboard sends.

use std::path::PathBuf;

use chembuider_rs::Config;
use chembuider_rs::config::FragmentDef;
use open_draw_chemistry::document::Document;
use open_draw_chemistry::fragments::FragmentStore;

const SCREEN: egui::Rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

/// The config the app gives every editor: the library's shortcuts, and the built-in fragments
/// with the user's (those in `dir`) laid over them.
fn app_config(dir: &PathBuf) -> Config {
    Config {
        fragments: FragmentStore::at(dir).fragments(),
        ..Config::embedded()
    }
}

/// A directory with no user fragments in it.
fn empty_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("odc-expand-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// One frame of the document's canvas with `events`.
fn frame(ctx: &egui::Context, doc: &mut Document, events: Vec<egui::Event>) {
    let raw = egui::RawInput {
        screen_rect: Some(SCREEN),
        events,
        focused: true,
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            doc.show(ui);
        });
    });
    output.textures_delta.clear();
}

/// The events of pressing and releasing the key that types `c` (`None` for a key that types
/// nothing, named by `key`).
fn stroke(key: egui::Key, text: Option<char>) -> Vec<egui::Event> {
    let modifiers = egui::Modifiers {
        shift: text.is_some_and(|c| c.is_ascii_uppercase()),
        ..Default::default()
    };
    let press = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    };
    let mut events = vec![press(true)];
    events.extend(text.map(|c| egui::Event::Text(c.to_string())));
    events.push(press(false));
    events
}

/// The keystrokes that label an atom `label` and expand it: backtick, the label, Enter, `x`.
fn label_and_expand(label: &str) -> Vec<Vec<egui::Event>> {
    let mut strokes = vec![stroke(egui::Key::Backtick, Some('`'))];
    for c in label.chars() {
        let key = egui::Key::from_name(&c.to_ascii_uppercase().to_string())
            .unwrap_or_else(|| panic!("no key types {c:?}"));
        strokes.push(stroke(key, Some(c)));
    }
    strokes.push(stroke(egui::Key::Enter, None));
    strokes.push(stroke(egui::Key::X, Some('x')));
    strokes
}

/// A document holding one carbon, which the keyboard is aimed at.
fn one_carbon(config: &Config) -> Document {
    let mut doc = Document::new(config, 1);
    let atom = doc.editor.molecule.add_atom("C".to_owned(), [0.0, 0.0], 0);
    doc.editor.hotspot_atom = Some(atom);
    doc
}

/// What is wrong with `doc` as the expansion of `def` on a lone atom, if anything.
fn check_expanded(doc: &Document, def: &FragmentDef) -> Result<(), String> {
    let mol = &doc.editor.molecule;
    let elements = |mut e: Vec<String>| {
        e.sort_unstable();
        e
    };
    let got = elements(mol.atoms.iter().map(|a| a.element.clone()).collect());
    let want = elements(def.atoms.iter().map(|a| a.element.clone()).collect());
    if doc.editor.label_edit.is_some() {
        return Err("the label editor is still open".to_owned());
    }
    if got != want || mol.bonds.len() != def.bonds.len() {
        return Err(format!(
            "atoms {got:?} with {} bonds, want {want:?} with {} bonds",
            mol.bonds.len(),
            def.bonds.len()
        ));
    }
    Ok(())
}

/// Every fragment the app lists expands when `x` is pressed on an atom labelled with its name.
#[test]
fn every_listed_fragment_expands_on_x() {
    let config = app_config(&empty_dir("listed"));
    assert!(
        config.fragments.len() >= Config::embedded().fragments.len(),
        "the built-in fragments are all listed"
    );
    let mut failures = Vec::new();
    for def in &config.fragments {
        let ctx = egui::Context::default();
        let mut doc = one_carbon(&config);
        let atom = doc.editor.hotspot_atom.unwrap();
        doc.editor.molecule.atom_by_id_mut(atom).unwrap().element = def.name.clone();
        frame(&ctx, &mut doc, Vec::new());
        frame(&ctx, &mut doc, stroke(egui::Key::X, Some('x')));
        if let Err(e) = check_expanded(&doc, def) {
            failures.push(format!("  {}: {e}", def.name));
        }
    }
    assert!(
        failures.is_empty(),
        "fragments that did not expand:\n{}",
        failures.join("\n")
    );
}

/// Typing a fragment's name as a label and pressing `x` expands it, however the keystrokes fall
/// into frames: one a frame, two a frame, or all at once (a quick typist, or a slow frame).
#[test]
fn a_typed_fragment_label_expands_on_x() {
    let config = app_config(&empty_dir("typed"));
    let mut failures = Vec::new();
    for def in &config.fragments {
        // The keys of these names are not all letters and digits of one key each.
        if !def.name.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let strokes = label_and_expand(&def.name);
        for per_frame in [1, 2, strokes.len()] {
            let ctx = egui::Context::default();
            let mut doc = one_carbon(&config);
            frame(&ctx, &mut doc, Vec::new());
            for batch in strokes.chunks(per_frame) {
                frame(&ctx, &mut doc, batch.concat());
            }
            // Keys the editor handed on to the next frame.
            frame(&ctx, &mut doc, Vec::new());
            frame(&ctx, &mut doc, Vec::new());
            if let Err(e) = check_expanded(&doc, def) {
                failures.push(format!(
                    "  {} ({per_frame} keystrokes a frame): {e}",
                    def.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "labels that did not expand:\n{}",
        failures.join("\n")
    );
}

/// Finishing a label with a click on the empty canvas instead of Enter commits it, and `x` then
/// expands it.
#[test]
fn a_label_finished_by_clicking_away_expands_on_x() {
    let config = app_config(&empty_dir("click"));
    let def = config
        .fragments
        .iter()
        .find(|d| d.name == "Ph")
        .expect("Ph is built in");
    let ctx = egui::Context::default();
    let mut doc = one_carbon(&config);
    frame(&ctx, &mut doc, Vec::new());
    let mut strokes = label_and_expand("Ph");
    let x = strokes.pop().unwrap();
    strokes.pop(); // no Enter
    for events in strokes {
        frame(&ctx, &mut doc, events);
    }
    let away = egui::pos2(120.0, 500.0);
    let button = |pressed| egui::Event::PointerButton {
        pos: away,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut doc, vec![egui::Event::PointerMoved(away)]);
    frame(&ctx, &mut doc, vec![button(true)]);
    frame(&ctx, &mut doc, vec![button(false)]);
    frame(&ctx, &mut doc, vec![egui::Event::PointerGone]);
    frame(&ctx, &mut doc, x);
    frame(&ctx, &mut doc, Vec::new());
    check_expanded(&doc, def).unwrap();
}

/// A carbon bonded to one other carbon, the keyboard aimed at the far one, which is labelled
/// `label`.
fn labelled_chain_end(config: &Config, label: &str) -> Document {
    let mut doc = Document::new(config, 1);
    let mol = &mut doc.editor.molecule;
    let first = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
    let end = mol.add_atom(label.to_owned(), [1.5, 0.0], 0);
    mol.add_bond(first, end, chembuider_rs::BondOrder::Single);
    doc.editor.hotspot_atom = Some(end);
    doc
}

/// `doc` after `x` is pressed on its labelled atom.
fn expanded(mut doc: Document) -> Document {
    let ctx = egui::Context::default();
    frame(&ctx, &mut doc, Vec::new());
    frame(&ctx, &mut doc, stroke(egui::Key::X, Some('x')));
    frame(&ctx, &mut doc, Vec::new());
    doc
}

/// The elements of the atoms and the orders of the bonds, each sorted.
fn composition(doc: &Document) -> (Vec<String>, Vec<i16>) {
    let mol = &doc.editor.molecule;
    let mut elements: Vec<String> = mol.atoms.iter().map(|a| a.element.clone()).collect();
    let mut orders: Vec<i16> = mol.bonds.iter().map(|b| b.order.valence()).collect();
    elements.sort_unstable();
    orders.sort_unstable();
    (elements, orders)
}

/// Condensed formulas expand with the bonds their valences demand: `CN` is a nitrile, `CHO` an
/// aldehyde and `COOH` a carboxylic acid, not chains of single bonds; the abbreviations that
/// are not fragments of their own (`OTf`, `NHBoc`, …) are read from the names they are made of.
#[test]
fn condensed_labels_expand_to_the_right_structure() {
    let config = app_config(&empty_dir("condensed"));
    for (label, elements, orders) in [
        ("CN", "C C N", vec![1, 3]),
        ("CHO", "C C O", vec![1, 2]),
        ("COOH", "C C O O", vec![1, 1, 2]),
        ("CO2H", "C C O O", vec![1, 1, 2]),
        ("CH3", "C C", vec![1]),
        ("CCl3", "C C Cl Cl Cl", vec![1, 1, 1, 1]),
        ("CO2Et", "C C C C O O", vec![1, 1, 1, 1, 2]),
        ("NMe2", "C C C N", vec![1, 1, 1]),
        ("SiMe3", "C C C C Si", vec![1, 1, 1, 1]),
        ("OMs", "C C O O O S", vec![1, 1, 1, 2, 2]),
        ("OAc", "C C C O O", vec![1, 1, 1, 2]),
        ("HOOC", "C C O O", vec![1, 1, 2]),
        ("NC", "C C N", vec![1, 3]),
        ("t-Bu", "C C C C C", vec![1, 1, 1, 1]),
        ("Ms", "C C O O S", vec![1, 1, 2, 2]),
        ("Bn", "C C C C C C C C", vec![1, 1, 1, 1, 1, 2, 2, 2]),
        ("ph", "C C C C C C C", vec![1, 1, 1, 1, 2, 2, 2]),
    ] {
        let doc = expanded(labelled_chain_end(&config, label));
        let want: Vec<String> = elements.split(' ').map(str::to_owned).collect();
        assert_eq!(composition(&doc), (want, orders), "{label}");
        assert_eq!(doc.editor.unexpanded_label, None, "{label}");
    }
}

/// The fragments of the app's own database (`assets/fragments/`) are in the list the editor
/// gets, and a label of each expands on a lone atom into its whole structure.
#[test]
fn the_app_fragments_are_listed_and_expand() {
    let config = app_config(&empty_dir("app"));
    let bundled = open_draw_chemistry::fragments::app_fragments();
    for name in ["THF", "DMF", "DCM", "Tol"] {
        assert!(bundled.iter().any(|d| d.name == name), "{name} is bundled");
    }
    for def in &bundled {
        assert!(
            config.fragments.iter().any(|d| d.name == def.name),
            "{} is listed",
            def.name
        );
        let mut doc = one_carbon(&config);
        let atom = doc.editor.hotspot_atom.unwrap();
        doc.editor.molecule.atom_by_id_mut(atom).unwrap().element = def.name.clone();
        let doc = expanded(doc);
        check_expanded(&doc, def).unwrap_or_else(|e| panic!("{}: {e}", def.name));
    }
}

/// The SMILES of `doc`'s molecule.
fn smiles(doc: &Document) -> String {
    chembuider_rs::molecule::smiles::to_smiles(&doc.editor.molecule)
}

/// Whether `doc`'s molecule is the one `spec` spells: bonds such as `C0-O1` or `C0=O1` between
/// atoms named element + index.
fn is_molecule(doc: &Document, spec: &str) -> bool {
    let mut want = chembuider_rs::Molecule::default();
    let mut ids = std::collections::BTreeMap::new();
    for bond in spec.split_whitespace() {
        let at = bond.find(['-', '=']).expect("a bond");
        let order = match &bond[at..=at] {
            "=" => chembuider_rs::BondOrder::Double,
            _ => chembuider_rs::BondOrder::Single,
        };
        let [a, b] = [&bond[..at], &bond[at + 1..]].map(|name| {
            *ids.entry(name.to_owned()).or_insert_with(|| {
                let element = name.trim_end_matches(|c: char| c.is_ascii_digit());
                want.add_atom(element.to_owned(), [0.0, 0.0], 0)
            })
        });
        want.add_bond(a, b, order);
    }
    chembuider_rs::molecule::canonical::is_isomorphic(&doc.editor.molecule, &want)
}

/// A solvent label standing alone is the whole neutral molecule; `Tol` is the *p*-tolyl group,
/// joined through the ring carbon opposite its methyl.
#[test]
fn the_solvent_and_tolyl_labels_expand_to_the_right_structure() {
    let config = app_config(&empty_dir("solvents"));
    for (label, want) in [
        ("THF", "O0-C1 C1-C2 C2-C3 C3-C4 C4-O0"),
        ("DMF", "O0=C1 C1-N2 N2-C3 N2-C4"),
        ("DCM", "Cl0-C1 C1-Cl2"),
        ("Tol", "C1=C2 C2-C3 C3=C4 C4-C5 C5=C6 C6-C1 C4-C7"),
    ] {
        let mut doc = one_carbon(&config);
        let atom = doc.editor.hotspot_atom.unwrap();
        doc.editor.molecule.atom_by_id_mut(atom).unwrap().element = label.to_owned();
        let doc = expanded(doc);
        assert!(is_molecule(&doc, want), "{label} alone: {}", smiles(&doc));
        assert!(
            doc.editor.molecule.atoms.iter().all(|a| a.charge == 0),
            "{label}: no charges"
        );
    }

    // On the end of a bond: p-xylene's skeleton from a methyl carbon and `Tol`.
    let doc = expanded(labelled_chain_end(&config, "Tol"));
    assert!(
        is_molecule(&doc, "C0-C1 C1=C2 C2-C3 C3=C4 C4-C5 C5=C6 C6-C1 C4-C7"),
        "{}",
        smiles(&doc)
    );

    // The solvents join through O (THF, DMF) or Cl (DCM), which have no valence left for a
    // bond: on the end of one the label is kept, rather than drawn as an oxonium or with a
    // divalent chlorine.
    for label in ["THF", "DMF", "DCM"] {
        let before = labelled_chain_end(&config, label);
        let mol = before.editor.molecule.clone();
        let doc = expanded(before);
        assert_eq!(doc.editor.molecule, mol, "{label} on a bond is kept");
    }
}

/// A user's file named like an app fragment is the one a label expands into.
#[test]
fn a_user_fragment_overrides_the_app_one_in_the_editor() {
    let dir = empty_dir("user-over-app");
    let mine = FragmentDef {
        name: "THF".to_owned(),
        atoms: vec![chembuider_rs::config::FragAtomDef {
            element: "S".to_owned(),
            pos: [0.0, 0.0],
            charge: 0,
        }],
        bonds: Vec::new(),
        attach_idx: 0,
    };
    FragmentStore::at(&dir).save(&mine).unwrap();
    let config = app_config(&dir);
    let doc = expanded(labelled_chain_end(&config, "THF"));
    assert_eq!(composition(&doc).0, ["C", "S"]);
    std::fs::remove_dir_all(&dir).ok();
}

/// A label that stands for no structure is left as it is, and the editor says which label it
/// was, for the status bar: nothing is expanded on a guess.
#[test]
fn a_label_with_no_structure_is_left_alone_and_reported() {
    let config = app_config(&empty_dir("unread"));
    for label in ["Qq", "CO2", "AcCl", "PPh3", "BOC", "CO"] {
        let before = labelled_chain_end(&config, label);
        let mol = before.editor.molecule.clone();
        let doc = expanded(before);
        assert_eq!(doc.editor.molecule, mol, "{label}: nothing changes");
        assert_eq!(doc.editor.unexpanded_label.as_deref(), Some(label));
        assert!(doc.editor.undo_stack.is_empty(), "{label}: nothing to undo");
    }
    // A plain element is not a label to expand, so there is nothing to report.
    let doc = expanded(labelled_chain_end(&config, "N"));
    assert_eq!(doc.editor.unexpanded_label, None);
}
