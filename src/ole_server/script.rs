//! A scripted edit for testing the OLE server end to end without a person at the window
//! (`--ole-test-script <file>`): when the first embedded document opens, its steps are applied
//! in order. One step per line; blank lines and `#` comments are skipped.
//!
//! ```text
//! add_atom N 3.0 0.0   # an atom of element N at (3, 0)
//! save                 # write back into the container's document
//! close                # close the tab (ends a COM-launched process)
//! ```

use chembuider_rs::Molecule;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    AddAtom { element: String, x: f32, y: f32 },
    Save,
    Close,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Script {
    pub steps: Vec<Step>,
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let words: Vec<&str> = line.split_whitespace().collect();
            let bad = || format!("{} 行目を読めません: {line}", n + 1);
            let step = match words.as_slice() {
                ["add_atom", element, x, y] => Step::AddAtom {
                    element: (*element).to_owned(),
                    x: x.parse().map_err(|_| bad())?,
                    y: y.parse().map_err(|_| bad())?,
                },
                ["save"] => Step::Save,
                ["close"] => Step::Close,
                _ => return Err(bad()),
            };
            steps.push(step);
        }
        Ok(Self { steps })
    }

    /// Apply the drawing steps to `mol`; the save and close steps are left to the caller, in
    /// order, as the returned list.
    pub fn apply(&self, mol: &mut Molecule) -> Vec<Step> {
        let mut rest = Vec::new();
        for step in &self.steps {
            match step {
                Step::AddAtom { element, x, y } => {
                    mol.add_atom(element.clone(), [*x, *y], 0);
                }
                other => rest.push(other.clone()),
            }
        }
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_adds_atoms_and_leaves_save_and_close() {
        let script = Script::parse("# edit\nadd_atom N 3 0.5\n\nsave\nclose  # done\n").unwrap();
        let mut mol = Molecule::default();
        let rest = script.apply(&mut mol);
        assert_eq!(mol.atoms.len(), 1);
        assert_eq!(mol.atoms[0].element, "N");
        assert_eq!(rest, vec![Step::Save, Step::Close]);
    }

    #[test]
    fn unknown_steps_are_errors() {
        assert!(Script::parse("explode").is_err());
        assert!(Script::parse("add_atom C x 0").is_err());
    }
}
