//! Labels that are written formulas — salts (`K2CO3`, `KHF2`) and complexes written with ligand
//! abbreviations (`PdCl2(amphos)2`): Expand draws nothing for them, but they have a formula and
//! a molecular weight, so the reaction table computes with them.
//!
//! Every test installs the same groups (the app's, with no user fragments), so they can run in
//! parallel although the groups are process-wide.

use chembuider_rs::Config;
use chembuider_rs::molecule::PageObject;
use chembuider_rs::molecule::page::ArrowKind;
use chembuider_rs::molecule::reaction_table::TableState;
use chembuider_rs::reaction::model::Mass;
use chembuider_rs::reaction::{LabelFormulaError, MolecularFormula};
use open_draw_chemistry::document::Document;
use open_draw_chemistry::fragments::{self, FragmentStore};

const SCREEN: egui::Rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));

/// The app's config with no user fragments, its formula groups installed.
fn app_config() -> Config {
    let dir = std::env::temp_dir().join("odc-formula-labels-no-such-dir");
    let config = Config {
        fragments: FragmentStore::at(dir).fragments(),
        ..Config::embedded()
    };
    fragments::install_formula_groups(&config.fragments);
    config
}

/// The formula of a lone atom labelled `label`.
fn formula(label: &str) -> MolecularFormula {
    let mut mol = chembuider_rs::Molecule::default();
    mol.add_atom(label.to_owned(), [0.0, 0.0], 0);
    MolecularFormula::of(&mol)
}

fn weight(label: &str) -> String {
    match formula(label).weight() {
        Some(w) => format!("{w:.2}"),
        None => "none".to_owned(),
    }
}

/// `label` on a lone atom after `x` is pressed on it: the atoms then drawn, and the label the
/// editor reports as not expanded.
fn expand(config: &Config, label: &str) -> (usize, Option<String>) {
    let mut doc = Document::new(config, 1);
    let atom = doc
        .editor
        .molecule
        .add_atom(label.to_owned(), [0.0, 0.0], 0);
    doc.editor.hotspot_atom = Some(atom);
    let ctx = egui::Context::default();
    let mut frame = |events: Vec<egui::Event>| {
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
    };
    let key = |pressed| egui::Event::Key {
        key: egui::Key::X,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Default::default(),
    };
    frame(Vec::new());
    frame(vec![
        key(true),
        egui::Event::Text("x".to_owned()),
        key(false),
    ]);
    frame(Vec::new());
    (
        doc.editor.molecule.atoms.len(),
        doc.editor.unexpanded_label.clone(),
    )
}

/// Salts and simple inorganic formulas: the composition (Hill order) and the weight. The
/// weights are computed by hand from the standard atomic weights.
#[test]
fn inorganic_formula_labels_have_their_composition_and_weight() {
    app_config();
    for (label, plain, mw) in [
        ("K2CO3", "CK2O3", "138.20"),
        ("KHF2", "F2HK", "78.10"),
        ("NaH", "HNa", "24.00"),
        ("NaOH", "HNaO", "40.00"),
        ("KOH", "HKO", "56.11"),
        ("LiAlH4", "AlH4Li", "37.95"),
        ("NaBH4", "BH4Na", "37.83"),
        ("Cs2CO3", "CCs2O3", "325.83"),
        ("K3PO4", "K3O4P", "212.26"),
        ("Na2SO4", "Na2O4S", "142.04"),
        ("MgSO4", "MgO4S", "120.36"),
        ("NaHCO3", "CHNaO3", "84.01"),
        ("NH4Cl", "ClH4N", "53.49"),
        ("CuI", "CuI", "190.45"),
        ("H2O", "H2O", "18.02"),
        ("HCl", "ClH", "36.46"),
        ("K_2CO_3", "CK2O3", "138.20"),
        ("CuSO4\u{b7}5H2O", "CuH10O9S", "249.68"),
    ] {
        assert_eq!(formula(label).plain(), plain, "{label}");
        assert_eq!(weight(label), mw, "{label}");
    }
}

/// Complexes written with ligand and group abbreviations, in any order, with nested
/// parentheses, square brackets and an adduct dot.
#[test]
fn complex_labels_have_their_composition_and_weight() {
    app_config();
    for (label, plain, mw) in [
        ("Pd(OAc)2", "C4H6O4Pd", "224.51"),
        ("PdCl2(amphos)2", "C32H56Cl2N2P2Pd", "708.08"),
        ("Pd(dppf)Cl2", "C34H28Cl2FeP2Pd", "731.71"),
        ("[(TMEDA)Zn(OH)(OTf)]3", "C21H51F9N6O12S3Zn3", "1042.97"),
        ("Pd(PPh3)4", "C72H60P4Pd", "1155.59"),
        ("Pd2(dba)3", "C51H42O3Pd2", "915.73"),
        ("Ni(cod)2", "C16H24Ni", "275.06"),
        ("[Ir(cod)Cl]2", "C16H24Cl2Ir2", "671.71"),
        ("RuCl2(PPh3)3", "C54H45Cl2P3Ru", "958.85"),
        ("Cu(OTf)2", "C2CuF6O6S2", "361.67"),
        ("Pd(dppf)Cl2\u{b7}CH2Cl2", "C35H30Cl4FeP2Pd", "816.64"),
        ("Pd(PCy3)2", "C36H66P2Pd", "667.29"),
        ("Pd(P(tBu)3)2", "C24H54P2Pd", "511.06"),
        ("Cp2ZrCl2", "C10H10Cl2Zr", "292.31"),
        ("[Cp*RhCl2]2", "C20H30Cl4Rh2", "618.08"),
        ("B2pin2", "C12H24B2O4", "253.94"),
        ("ZnCl2(THF)2", "C8H16Cl2O2Zn", "280.49"),
        ("Pd(MeCN)2Cl2", "C4H6Cl2N2Pd", "259.43"),
        ("AcCl", "C2H3ClO", "78.50"),
        // Written in another case.
        ("PdCl2(AmPhos)2", "C32H56Cl2N2P2Pd", "708.08"),
        ("Ni(COD)2", "C16H24Ni", "275.06"),
        ("Pd(DPPF)Cl2", "C34H28Cl2FeP2Pd", "731.71"),
        ("ZnCl2(thf)2", "C8H16Cl2O2Zn", "280.49"),
    ] {
        assert_eq!(formula(label).plain(), plain, "{label}");
        assert_eq!(weight(label), mw, "{label}");
    }
}

/// An abbreviation is never taken for element symbols or the other way round.
#[test]
fn abbreviations_and_elements_are_told_apart() {
    app_config();
    // Cobalt stays cobalt next to `cod`, and carbonyl stays C and O.
    assert_eq!(formula("CoCl2").plain(), "Cl2Co");
    assert_eq!(formula("Co(cod)2").plain(), "C16H24Co");
    assert_eq!(formula("Co2(CO)8").plain(), "C8Co2O8");
    assert_eq!(formula("Fe(CO)5").plain(), "C5FeO5");
    // A fragment's name in another case is not read into a formula of elements.
    assert_eq!(formula("BN").plain(), "BN");
    assert_eq!(formula("PH3").plain(), "H3P");
    // `iPr` (isopropyl) and `IPr` (the carbene) differ only in case.
    assert_eq!(formula("Ti(OiPr)4").plain(), "C12H28O4Ti");
    assert_eq!(formula("Pd(IPr)Cl2").plain(), "C27H36Cl2N2Pd");
}

/// A group that is not known leaves the label without a weight — never a partial one — and
/// is named: `TEMDA` is not `TMEDA`.
#[test]
fn an_unknown_group_gives_no_weight_and_is_named() {
    app_config();
    let label = "[(TEMDA)Zn(OH)(OTf)]3";
    assert_eq!(weight(label), "none");
    assert_eq!(formula(label).unknown_elements(), ["TEMDA"]);
    assert_eq!(
        MolecularFormula::of_label(label),
        Err(LabelFormulaError::UnknownGroup("TEMDA".to_owned()))
    );
    let _lang = open_draw_chemistry::i18n::ScopedLanguage::new(open_draw_chemistry::i18n::Lang::En);
    let message = fragments::not_expanded_message(label);
    assert!(message.contains("Unknown group “TEMDA”"), "{message}");
    assert_eq!(weight("Pd(xyzphos)Cl2"), "none");
    assert_eq!(weight("Qq"), "none");
}

/// Expand draws no structure for a salt or a complex: the label stays, and the status bar says
/// it is a formula, with its composition and weight. Labels that expand still do.
#[test]
fn expand_keeps_formula_labels_and_says_so() {
    let config = app_config();
    let _lang = open_draw_chemistry::i18n::ScopedLanguage::new(open_draw_chemistry::i18n::Lang::En);
    for (label, says) in [
        ("K2CO3", "is a formula (CK2O3, 138.20 g/mol)"),
        ("KHF2", "is a formula (F2HK, 78.10 g/mol)"),
        ("Cs2CO3", "is a formula (CCs2O3, 325.83 g/mol)"),
        ("H2O", "is a formula (H2O, 18.02 g/mol)"),
        ("AcCl", "is a formula (C2H3ClO, 78.50 g/mol)"),
        (
            "PdCl2(amphos)2",
            "is a formula (C32H56Cl2N2P2Pd, 708.08 g/mol)",
        ),
        (
            "Pd(dppf)Cl2",
            "is a formula (C34H28Cl2FeP2Pd, 731.71 g/mol)",
        ),
        (
            "[(TMEDA)Zn(OH)(OTf)]3",
            "is a formula (C21H51F9N6O12S3Zn3, 1042.97 g/mol)",
        ),
        ("Qq", "No structure for the label “Qq”"),
    ] {
        let (atoms, unexpanded) = expand(&config, label);
        assert_eq!(atoms, 1, "{label} is not drawn");
        assert_eq!(unexpanded.as_deref(), Some(label), "{label} is reported");
        let message = fragments::not_expanded_message(label);
        assert!(message.contains(says), "{label}: {message}");
    }
    // What expanded before still does.
    for (label, atoms) in [("CN", 2), ("CHO", 2), ("COOH", 3), ("NHBoc", 8), ("tBu", 4)] {
        assert_eq!(expand(&config, label), (atoms, None), "{label}");
    }
}

/// Every formula-only group: its name can be written in a formula, is no element's, and its
/// formula weighs what was computed for it independently (from the structure named in the
/// file's `note`, with the standard atomic weights).
#[test]
fn the_formula_groups_are_valid_and_weigh_what_they_should() {
    let expected = [
        ("amphos", "C16H28NP", "265.38"),
        ("dppm", "C25H22P2", "384.40"),
        ("dppe", "C26H24P2", "398.43"),
        ("dppp", "C27H26P2", "412.45"),
        ("dppb", "C28H28P2", "426.48"),
        ("dppf", "C34H28FeP2", "554.39"),
        ("BINAP", "C44H32P2", "622.69"),
        ("Xantphos", "C39H32OP2", "578.63"),
        ("XPhos", "C33H49P", "476.73"),
        ("SPhos", "C26H35O2P", "410.54"),
        ("RuPhos", "C30H43O2P", "466.65"),
        ("dba", "C17H14O", "234.30"),
        ("cod", "C8H12", "108.18"),
        ("coe", "C8H14", "110.20"),
        ("nbd", "C7H8", "92.14"),
        ("Cp", "C5H5", "65.09"),
        ("Cp*", "C10H15", "135.23"),
        ("acac", "C5H7O2", "99.11"),
        ("pin", "C6H12O2", "116.16"),
        ("TMEDA", "C6H16N2", "116.21"),
        ("py", "C5H5N", "79.10"),
        ("bpy", "C10H8N2", "156.19"),
        ("dtbpy", "C18H24N2", "268.40"),
        ("phen", "C12H8N2", "180.21"),
        ("DMAP", "C7H10N2", "122.17"),
        ("DBU", "C9H16N2", "152.24"),
        ("DABCO", "C6H12N2", "112.18"),
        ("HMDS", "C6H18NSi2", "160.39"),
        ("DMSO", "C2H6OS", "78.13"),
        ("DME", "C4H10O2", "90.12"),
        ("IPr", "C27H36N2", "388.60"),
        ("IMes", "C21H24N2", "304.44"),
    ];
    let groups = fragments::formula_only_groups();
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    let listed: Vec<&str> = expected.iter().map(|e| e.0).collect();
    assert_eq!(
        names, listed,
        "every entry of the file reads and is checked here"
    );
    let mut spellings = std::collections::BTreeSet::new();
    for (group, (name, plain, mw)) in groups.iter().zip(expected) {
        assert_eq!(group.formula.plain(), plain, "{name}");
        assert_eq!(
            format!("{:.2}", group.formula.weight().unwrap()),
            mw,
            "{name}"
        );
        for spelling in std::iter::once(&group.name).chain(&group.aliases) {
            assert!(
                spellings.insert(spelling.to_lowercase()),
                "{spelling} is listed twice"
            );
            // Whatever its case, the name must not be an element's symbol.
            let mut symbol = spelling.to_lowercase();
            symbol[..1].make_ascii_uppercase();
            assert!(
                chembuider_rs::molecule::elements::atomic_number(&symbol).is_none(),
                "{spelling} is an element"
            );
        }
    }
    // A fragment the app lists is found under its own name, not shadowed by a group.
    app_config();
    assert_eq!(formula("THF").plain(), "C4H8O");
    assert_eq!(formula("DMF").plain(), "C3H7NO");
    assert_eq!(formula("DCM").plain(), "CH2Cl2");
    assert_eq!(formula("Tol").plain(), "C7H7");
}

/// The reaction table (equivalents, moles, masses) computes with a catalyst that is only a
/// label over the arrow.
#[test]
fn the_reaction_table_uses_the_weight_of_a_complex_label() {
    let config = app_config();
    let mut doc = Document::new(&config, 1);
    let mol = &mut doc.editor.molecule;
    // Bromobenzene's stand-in: bromoethane, 108.97 g/mol.
    let c1 = mol.add_atom("C".to_owned(), [0.0, 0.0], 0);
    let c2 = mol.add_atom("C".to_owned(), [1.5, 0.0], 0);
    let br = mol.add_atom("Br".to_owned(), [3.0, 0.0], 0);
    mol.add_bond(c1, c2, chembuider_rs::BondOrder::Single);
    mol.add_bond(c2, br, chembuider_rs::BondOrder::Single);
    mol.add_arrow([5.0, 0.0], [11.0, 0.0], ArrowKind::Reaction);
    mol.add_atom("PdCl2(amphos)2".to_owned(), [7.0, -1.0], 0);
    mol.add_atom("K2CO3".to_owned(), [9.0, -1.0], 0);
    let p1 = mol.add_atom("C".to_owned(), [13.0, 0.0], 0);
    let p2 = mol.add_atom("C".to_owned(), [14.5, 0.0], 0);
    mol.add_bond(p1, p2, chembuider_rs::BondOrder::Single);

    let id = doc
        .editor
        .insert_reaction_table()
        .expect("a table is added");
    let Some(PageObject::Table(table)) = doc.editor.molecule.object_by_id(id) else {
        panic!("the object added is a table");
    };
    let mut state = TableState::of(&doc.editor.molecule, table);
    assert_eq!(
        state.formulas().0,
        ["C2H5Br", "C32H56Cl2N2P2Pd", "CK2O3"],
        "substrate, catalyst, base"
    );
    // 10 mmol of substrate, 2 mol% of catalyst, 2 equivalents of base.
    state.analysis.reactants[0].limiting = true;
    state.analysis.reactants[0].sample_mass = Some(Mass::grams(1.0897));
    state.analysis.reactants[1].equivalents = Some(0.02);
    state.analysis.reactants[2].equivalents = Some(2.0);
    let result = state.analysis.compute();
    let grams = |i: usize| result.reactants[i].mass.expect("a mass").value;
    assert_eq!(format!("{:.2}", result.reactants[1].mw.unwrap()), "708.08");
    assert!(
        (grams(1) - 0.02 * 0.01 * 708.08).abs() < 1e-4,
        "{}",
        grams(1)
    );
    assert!(
        (grams(2) - 2.0 * 0.01 * 138.20).abs() < 1e-3,
        "{}",
        grams(2)
    );
}
