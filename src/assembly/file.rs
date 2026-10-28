//! Saving and opening an assembly (`.odasm`): JSON holding the monomers, the steps and the
//! hand-placed molecules, so opening the file builds exactly the same assembly.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::{Design, Step};

/// The file extension.
pub const EXTENSION: &str = "odasm";

/// The newest version read; files of a newer version are refused.
///
/// 2 added the crystal step (`Lattice`, with its operations as `x,y,z` text). Version 1 files
/// read unchanged; an older app opening a version 2 file says it is newer instead of calling
/// it damaged. 3 added the move/rotate step (`Transform`) the same way.
pub const VERSION: u32 = 3;

/// The version a file of `design` needs: the oldest one that holds every step it uses, so
/// an assembly without a move/rotate step still opens in a release that reads only version 2.
fn version_for(design: &Design) -> u32 {
    if design
        .steps
        .iter()
        .any(|step| matches!(step, Step::Transform(_)))
    {
        VERSION
    } else {
        2
    }
}

#[derive(Serialize, Deserialize)]
struct FileContents {
    version: u32,
    #[serde(flatten)]
    design: Design,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// Not an assembly file, or a damaged one.
    Invalid(String),
    /// Written by a newer version of the app.
    Newer(u32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Invalid(e) => f.write_str(&t!("assembly.error.file_invalid", detail = e)),
            Error::Newer(v) => f.write_str(&t!(
                "assembly.error.file_newer",
                version = v,
                supported = VERSION
            )),
        }
    }
}

impl std::error::Error for Error {}

/// `design` as the text of an `.odasm` file.
pub fn to_string(design: &Design) -> String {
    let contents = FileContents {
        version: version_for(design),
        design: design.clone(),
    };
    serde_json::to_string_pretty(&contents).expect("a design always serialises")
}

/// The design in the text of an `.odasm` file.
pub fn from_str(text: &str) -> Result<Design, Error> {
    #[derive(Deserialize)]
    struct Version {
        version: Option<u32>,
    }
    let version: Version = serde_json::from_str(text).map_err(|e| Error::Invalid(e.to_string()))?;
    match version.version {
        Some(v) if v > VERSION => return Err(Error::Newer(v)),
        Some(_) => {}
        None => {
            return Err(Error::Invalid(
                t!("assembly.error.file_no_version").to_owned(),
            ));
        }
    }
    let contents: FileContents =
        serde_json::from_str(text).map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(contents.design)
}

pub fn save(path: &Path, design: &Design) -> Result<(), Error> {
    std::fs::write(path, to_string(design)).map_err(Error::Io)
}

pub fn open(path: &Path) -> Result<Design, Error> {
    let text = std::fs::read_to_string(path).map_err(Error::Io)?;
    from_str(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::build::build;
    use crate::assembly::math::Rigid;
    use crate::assembly::model::{Component, Instance, Step};
    use crate::three_d::{Atom3d, Bond3d};

    #[test]
    fn a_saved_assembly_opens_to_the_same_build() {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        let mut stack = Step::new_stack();
        if let Step::Stack(s) = &mut stack {
            s.twist_deg = 12.5;
            s.slip = 0.7;
        }
        let mut off = Step::new_array();
        *off.enabled_mut() = false;
        let design = Design {
            components: vec![Component {
                name: "urea 尿素".into(),
                atoms: vec![
                    atom("C", [0.1, 0.2, 0.3]),
                    atom("O", [1.33, 0.2, 0.3]),
                    atom("N", [-0.6, 1.4, 0.3]),
                ],
                bonds: vec![Bond3d {
                    begin: 0,
                    end: 1,
                    order: 2,
                }],
                orient: Some([0, 1, 2]),
            }],
            base: 0,
            steps: vec![Step::new_ring(), stack, off],
            manual: vec![Instance {
                component: 0,
                transform: Rigid::translation([1.0, 2.0, 3.0]),
            }],
            excluded: vec![3],
            ..Default::default()
        };
        let text = to_string(&design);
        assert!(text.contains("\"version\": 2"));
        let back = from_str(&text).unwrap();
        assert_eq!(back, design);
        assert_eq!(build(&back, 1).unwrap(), build(&design, 1).unwrap());

        let dir = std::env::temp_dir().join(format!("odasm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.odasm");
        save(&path, &design).unwrap();
        assert_eq!(open(&path).unwrap(), design);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The ids that keep picks on their molecules ([`crate::assembly::model::Ids`]) are saved
    /// once something has been removed, and a file without them — an older one — opens with
    /// the molecules named by their places.
    #[test]
    fn ids_are_saved_and_older_files_open_without_them() {
        use crate::assembly::model::Origin;
        let atom = Atom3d {
            element: "C".into(),
            pos: [0.0; 3],
        };
        let component = Component {
            name: "c".into(),
            atoms: vec![atom],
            bonds: Vec::new(),
            orient: None,
        };
        let placed = |x| Instance {
            component: 0,
            transform: Rigid::translation([x, 0.0, 0.0]),
        };
        let old = r#"{"version": 2, "components": [{"name": "c", "atoms": [{"element": "C", "pos": [0, 0, 0]}], "bonds": []}], "manual": [{"component": 0, "transform": {"rot": [[1, 0, 0], [0, 1, 0], [0, 0, 1]], "trans": [5, 0, 0]}}]}"#;
        let design = from_str(old).unwrap();
        assert_eq!(design.manual.len(), 1);
        assert_eq!(design.molecule_of(Origin::Manual(0)), Some(1));
        // Nothing removed yet: nothing more is written than before.
        assert!(!to_string(&design).contains("ids"));

        let mut design = Design {
            components: vec![component],
            manual: vec![placed(1.0), placed(2.0), placed(3.0)],
            ..Default::default()
        };
        let third = design.origin_of(3).unwrap();
        assert!(design.remove_molecule(1));
        let back = from_str(&to_string(&design)).unwrap();
        assert_eq!(back, design);
        assert_eq!(back.molecule_of(third), Some(2));
    }

    #[test]
    fn other_files_and_newer_versions_are_refused() {
        let _lang = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
        assert!(matches!(from_str("not json"), Err(Error::Invalid(_))));
        assert!(matches!(from_str("{}"), Err(Error::Invalid(_))));
        let newer = from_str(r#"{"version": 99, "components": []}"#).unwrap_err();
        assert!(matches!(newer, Error::Newer(99)));
        assert!(newer.to_string().contains("新しい版"));
        // Missing optional parts default.
        let empty = from_str(r#"{"version": 1}"#).unwrap();
        assert_eq!(empty, Design::default());
    }

    /// An assembly without a move/rotate step is written exactly as the release before the
    /// step (v0.1.0, which reads up to version 2) wrote it, so that release still opens it.
    #[test]
    fn an_assembly_without_a_transform_is_written_as_version_2() {
        use crate::assembly::model::Array;
        let design = Design {
            components: vec![Component {
                name: "c".into(),
                atoms: vec![Atom3d {
                    element: "C".into(),
                    pos: [0.0, 0.5, -1.0],
                }],
                bonds: Vec::new(),
                orient: None,
            }],
            steps: vec![Step::Array(Array {
                enabled: true,
                n: 3,
                offset: [0.0, 0.0, 3.5],
                euler_deg: [0.0; 3],
            })],
            excluded: vec![1],
            ..Default::default()
        };
        let text = to_string(&design);
        assert_eq!(text, GOLDEN_V2);
        assert_eq!(from_str(&text).unwrap(), design);
    }

    /// What v0.1.0's `to_string` writes for the design in
    /// [`an_assembly_without_a_transform_is_written_as_version_2`].
    const GOLDEN_V2: &str = r#"{
  "version": 2,
  "components": [
    {
      "name": "c",
      "atoms": [
        {
          "element": "C",
          "pos": [
            0.0,
            0.5,
            -1.0
          ]
        }
      ],
      "bonds": [],
      "orient": null
    }
  ],
  "base": 0,
  "steps": [
    {
      "Array": {
        "enabled": true,
        "n": 3,
        "offset": [
          0.0,
          0.0,
          3.5
        ],
        "euler_deg": [
          0.0,
          0.0,
          0.0
        ]
      }
    }
  ],
  "manual": [],
  "excluded": [
    1
  ]
}"#;

    /// A move/rotate step is saved with its pivot and opens to the same build; a version 2
    /// file (written before the step existed) still opens.
    #[test]
    fn a_transform_step_is_saved_and_older_files_still_open() {
        use crate::assembly::model::{Pivot, Transform};
        let design = Design {
            components: vec![Component {
                name: "co".into(),
                atoms: vec![
                    Atom3d {
                        element: "C".into(),
                        pos: [0.0; 3],
                    },
                    Atom3d {
                        element: "O".into(),
                        pos: [1.13, 0.0, 0.0],
                    },
                ],
                bonds: Vec::new(),
                orient: None,
            }],
            steps: vec![
                Step::new_ring(),
                Step::Transform(Transform {
                    enabled: true,
                    offset: [1.5, -2.0, 0.25],
                    euler_deg: [10.0, 20.0, 30.0],
                    pivot: Pivot::Origin,
                }),
                Step::new_transform(),
            ],
            ..Default::default()
        };
        let text = to_string(&design);
        assert!(text.contains("\"version\": 3"));
        let back = from_str(&text).unwrap();
        assert_eq!(back, design);
        assert_eq!(build(&back, 1).unwrap(), build(&design, 1).unwrap());

        // A disabled one still needs version 3: an older app could not read the step.
        let off = Design {
            steps: vec![{
                let mut t = Step::new_transform();
                *t.enabled_mut() = false;
                t
            }],
            ..Default::default()
        };
        assert!(to_string(&off).contains("\"version\": 3"));
        assert_eq!(from_str(&to_string(&off)).unwrap(), off);

        let old = r#"{"version": 2, "components": [{"name": "c", "atoms": [{"element": "C", "pos": [0, 0, 0]}], "bonds": []}], "steps": [{"Array": {"enabled": true, "n": 3, "offset": [0, 0, 3.5], "euler_deg": [0, 0, 0]}}]}"#;
        let design = from_str(old).unwrap();
        assert_eq!(design.steps.len(), 1);
        assert_eq!(design.molecule_count(), 3);
    }

    #[test]
    fn a_crystal_step_is_saved_with_its_operations_as_text() {
        use crate::assembly::math::Cell;
        use crate::assembly::model::Lattice;
        use crate::assembly::symop;
        let design = Design {
            components: vec![Component {
                name: "co".into(),
                atoms: vec![
                    Atom3d {
                        element: "C".into(),
                        pos: [0.0; 3],
                    },
                    Atom3d {
                        element: "O".into(),
                        pos: [1.13, 0.0, 0.0],
                    },
                ],
                bonds: Vec::new(),
                orient: None,
            }],
            steps: vec![Step::Lattice(Lattice {
                enabled: true,
                cell: Cell {
                    a: 5.1,
                    b: 6.2,
                    c: 7.3,
                    alpha: 90.0,
                    beta: 98.5,
                    gamma: 90.0,
                },
                reps: [2, 1, 3],
                symops: symop::parse_list("x,y,z\n-x,y+1/2,-z+1/2\n-x,-y,-z\nx,-y+1/2,z+1/2")
                    .unwrap(),
            })],
            ..Default::default()
        };
        let text = to_string(&design);
        assert!(text.contains("\"-x,y+1/2,-z+1/2\""), "{text}");
        let back = from_str(&text).unwrap();
        assert_eq!(back, design);
        assert_eq!(build(&back, 1).unwrap(), build(&design, 1).unwrap());
        // A damaged operation is reported, not ignored.
        let broken = text.replace("-x,y+1/2,-z+1/2", "-x,y+1/2");
        assert!(matches!(from_str(&broken), Err(Error::Invalid(_))));
    }
}
