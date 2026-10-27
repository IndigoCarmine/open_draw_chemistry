//! 「IUPAC 名をコピー」「IUPAC 名をテキストとして挿入」: the name of the selected structures
//! (or of the whole drawing), from chembuider-rs's namer.

use chembuider_rs::naming::{NameResult, iupac_name_with_config};
use chembuider_rs::widget::analysis::analysis_target;
use chembuider_rs::{ChemStructEditor, Config, Molecule};

/// How far below the lowest atom the inserted name's baseline goes (one and a half bonds).
const GAP_BELOW: f32 = 2.2;

/// The structures the commands name: those with a selected atom, or everything drawn.
fn target(editor: &ChemStructEditor) -> Molecule {
    analysis_target(&editor.molecule, &editor.selected_atoms)
}

/// The IUPAC name of the selected structures (or the whole drawing), or why there is none —
/// in Japanese, for the status bar.
pub fn name(editor: &ChemStructEditor, config: &Config) -> Result<String, String> {
    let target = target(editor);
    if target.atoms.is_empty() {
        return Err("名前を付ける構造がありません".to_owned());
    }
    match iupac_name_with_config(&target, config) {
        NameResult::Ok(name) => Ok(name),
        NameResult::Unsupported(reason) => {
            Err(format!("この構造の IUPAC 名には未対応です（{reason}）"))
        }
    }
}

/// Put the name on the page as a text object under the named structures, as one undoable
/// step. Returns the name.
pub fn insert_name(editor: &mut ChemStructEditor, config: &Config) -> Result<String, String> {
    let name = name(editor, config)?;
    let target = target(editor);
    let left = target
        .atoms
        .iter()
        .map(|a| a.pos[0])
        .fold(f32::INFINITY, f32::min);
    // The editor's y axis points down: the lowest atom has the largest y.
    let bottom = target
        .atoms
        .iter()
        .map(|a| a.pos[1])
        .fold(f32::NEG_INFINITY, f32::max);
    editor.push_undo();
    editor
        .molecule
        .add_text([left, bottom + GAP_BELOW], markup(&name));
    Ok(name)
}

/// The name in the page text's markup: its special characters escaped, and the italic parts
/// (`N` locants, `tert`) set in italics.
pub fn markup(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    let chars: Vec<char> = name.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rest: String = chars[i..].iter().collect();
        let starts_word = i == 0 || matches!(chars[i - 1], '-' | ',' | '(' | '[' | '{' | ' ');
        if starts_word && rest.starts_with("tert-") {
            out += "//tert//";
            i += 4;
            continue;
        }
        if starts_word && c == 'N' && matches!(chars.get(i + 1), Some(',' | '-' | '\'' | '′')) {
            out += "//N//";
            i += 1;
            continue;
        }
        if matches!(c, '_' | '^' | '*' | '/' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chembuider_rs::BondOrder;

    fn ethanol_and_methane() -> (ChemStructEditor, u32) {
        let mut editor = ChemStructEditor::default();
        let mol = &mut editor.molecule;
        let c1 = mol.add_atom("C".into(), [0.0, 0.0], 0);
        let c2 = mol.add_atom("C".into(), [1.3, 0.75], 0);
        let o = mol.add_atom("O".into(), [2.6, 0.0], 0);
        mol.add_bond(c1, c2, BondOrder::Single);
        mol.add_bond(c2, o, BondOrder::Single);
        let methane = mol.add_atom("C".into(), [8.0, 3.0], 0);
        (editor, methane)
    }

    #[test]
    fn names_the_selection_or_everything() {
        let config = Config::default();
        let (mut editor, methane) = ethanol_and_methane();
        assert_eq!(name(&editor, &config).unwrap(), "ethanol; methane");
        editor.selected_atoms.insert(methane);
        assert_eq!(name(&editor, &config).unwrap(), "methane");
        assert!(name(&ChemStructEditor::default(), &config).is_err());
    }

    #[test]
    fn inserts_the_name_under_the_structure_undoably() {
        let config = Config::default();
        let (mut editor, methane) = ethanol_and_methane();
        editor.selected_atoms.insert(methane);
        assert_eq!(insert_name(&mut editor, &config).unwrap(), "methane");
        let [chembuider_rs::molecule::PageObject::Text(text)] = editor.molecule.objects.as_slice()
        else {
            panic!("one text object expected: {:?}", editor.molecule.objects);
        };
        assert_eq!(text.text, "methane");
        assert_eq!(text.pos, [8.0, 3.0 + GAP_BELOW]);
        assert!(editor.undo());
        assert!(editor.molecule.objects.is_empty());
    }

    #[test]
    fn markup_escapes_and_italicises() {
        assert_eq!(markup("N,N-dimethylaniline"), "//N//,//N//-dimethylaniline");
        assert_eq!(markup("4-tert-butylphenol"), "4-//tert//-butylphenol");
        assert_eq!(markup("a{b}"), "a\\{b\\}");
        assert_eq!(markup("2-methylpropan-1-ol"), "2-methylpropan-1-ol");
    }
}
