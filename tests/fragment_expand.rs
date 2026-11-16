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
