//! Crystallographic symmetry operations in the `x,y,z` notation of CIF files and the
//! International Tables (`-x,y+1/2,-z+1/2`, `x-y,x,z+1/6`), and the space groups offered as
//! presets.
//!
//! An operation acts on fractional coordinates: `f ↦ R·f + t` with `R` an integer matrix of
//! determinant ±1 and `t` a fractional translation. Translations are kept in `[0, 1)`: a whole
//! cell's shift changes nothing in a crystal, and the builder moves every copy back into the
//! cell by its centroid anyway (see [`super::model::Lattice`]).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::math::{self, Cell, Mat3, Rigid, Vec3};

/// One symmetry operation. Saved in `.odasm` files as its `x,y,z` text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SymOp {
    /// Row `i` gives fractional coordinate `i` of the image from those of the original.
    pub rot: [[i32; 3]; 3],
    /// The fractional translation, each in `[0, 1)`.
    pub trans: Vec3,
}

impl SymOp {
    pub const IDENTITY: SymOp = SymOp {
        rot: [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
        trans: [0.0; 3],
    };

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// The rotation part as a real matrix.
    pub fn rot_f64(&self) -> Mat3 {
        self.rot.map(|row| row.map(f64::from))
    }

    /// The image of the fractional point `f`.
    pub fn apply_frac(&self, f: Vec3) -> Vec3 {
        math::add(math::mat_vec(&self.rot_f64(), f), self.trans)
    }

    /// The operation in Cartesian Å for `cell`: `M·R·M⁻¹` and `M·t`, with `M` the
    /// fractional → Cartesian matrix.
    pub fn cartesian(&self, cell: &Cell) -> Rigid {
        let m = cell.frac_to_cart();
        Rigid {
            rot: math::mat_mul(&m, &math::mat_mul(&self.rot_f64(), &cell.cart_to_frac())),
            trans: math::mat_vec(&m, self.trans),
        }
    }

    /// Whether the operation keeps distances in `cell` (to 10⁻³ in the matrix entries). A
    /// fourfold axis in a cell with a ≠ b, say, does not, and would distort the molecules.
    pub fn fits(&self, cell: &Cell) -> bool {
        let r = self.cartesian(cell).rot;
        let rrt = math::mat_mul(&r, &math::transpose(&r));
        Rigid::rotation(rrt).approx_eq(&Rigid::IDENTITY, 1e-3)
    }

    /// Read one operation: three comma-separated expressions in x, y and z (any case, any
    /// spaces), each a sum of `±x`, `±y`, `±z` (each at most once) and fractions or decimals.
    pub fn parse(text: &str) -> Result<SymOp, String> {
        let text: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| match c {
                // The Unicode minus sign, as copied from papers.
                '−' | '–' => '-',
                c => c.to_ascii_lowercase(),
            })
            .collect();
        if text.is_empty() {
            return Err(t!("assembly.symop.error.empty_operation").to_owned());
        }
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() != 3 {
            return Err(t!(
                "assembly.symop.error.component_count",
                count = parts.len()
            ));
        }
        let mut rot = [[0; 3]; 3];
        let mut trans = [0.0; 3];
        for (i, part) in parts.iter().enumerate() {
            let (row, t) = parse_component(part).map_err(|e| {
                t!(
                    "assembly.symop.error.in_component",
                    index = i + 1,
                    error = e
                )
            })?;
            rot[i] = row;
            trans[i] = wrap_unit(t);
        }
        let op = SymOp { rot, trans };
        let det = math::det(&op.rot_f64());
        if (det.abs() - 1.0).abs() > 1e-9 {
            return Err(t!("assembly.symop.error.not_symmetry", det = det));
        }
        Ok(op)
    }
}

/// `t` moved into `[0, 1)`, with values a rounding error below 1 taken as 0.
fn wrap_unit(t: f64) -> f64 {
    let w = t.rem_euclid(1.0);
    if w > 1.0 - 1e-9 { 0.0 } else { w }
}

/// One component: the coefficients of x, y, z and the constant.
fn parse_component(s: &str) -> Result<([i32; 3], f64), String> {
    if s.is_empty() {
        return Err(t!("assembly.symop.error.empty_component").to_owned());
    }
    let chars: Vec<char> = s.chars().collect();
    let mut row = [0; 3];
    let mut constant = 0.0;
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        let sign = match chars[i] {
            '+' => {
                i += 1;
                1
            }
            '-' => {
                i += 1;
                -1
            }
            _ if first => 1,
            c => return Err(t!("assembly.symop.error.sign_needed", char = c)),
        };
        first = false;
        let Some(&c) = chars.get(i) else {
            return Err(t!("assembly.symop.error.ends_with_sign").to_owned());
        };
        match c {
            'x' | 'y' | 'z' => {
                let axis = (c as u8 - b'x') as usize;
                if row[axis] != 0 {
                    return Err(t!("assembly.symop.error.repeated_variable", variable = c));
                }
                row[axis] = sign;
                i += 1;
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | '/'))
                {
                    i += 1;
                }
                let number: String = chars[start..i].iter().collect();
                let value = parse_number(&number)?;
                if let Some(&next) = chars.get(i)
                    && matches!(next, 'x' | 'y' | 'z' | '*')
                {
                    return Err(t!(
                        "assembly.symop.error.coefficient",
                        example = format!("{number}{next}")
                    ));
                }
                constant += f64::from(sign) * value;
            }
            c => return Err(t!("assembly.symop.error.bad_char", char = c)),
        }
    }
    Ok((row, constant))
}

/// A decimal (`0.5`) or a fraction (`1/2`).
fn parse_number(s: &str) -> Result<f64, String> {
    let bad = || t!("assembly.symop.error.bad_number", number = s);
    let value = match s.split_once('/') {
        Some((n, d)) => {
            let n: f64 = n.parse().map_err(|_| bad())?;
            let d: f64 = d.parse().map_err(|_| bad())?;
            if d == 0.0 {
                return Err(t!("assembly.symop.error.divide_by_zero", number = s));
            }
            n / d
        }
        None => s.parse().map_err(|_| bad())?,
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err(bad())
    }
}

/// A translation as a short fraction (`1/2`, `1/6`, `3/4`), or a decimal when it is none.
fn format_fraction(t: f64) -> String {
    for den in [1i64, 2, 3, 4, 6, 8, 12] {
        let num = (t * den as f64).round();
        if (t * den as f64 - num).abs() < 1e-6 {
            let num = num as i64;
            return if den == 1 {
                num.to_string()
            } else {
                format!("{num}/{den}")
            };
        }
    }
    let s = format!("{t:.6}");
    s.trim_end_matches('0').to_owned()
}

impl fmt::Display for SymOp {
    /// `-x,y+1/2,-z+1/2`: no spaces, variables before the translation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (row, &t)) in self.rot.iter().zip(&self.trans).enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            let mut empty = true;
            for (coefficient, name) in row.iter().zip(["x", "y", "z"]) {
                match *coefficient {
                    0 => continue,
                    c if c > 0 && empty => {}
                    c if c > 0 => f.write_str("+")?,
                    _ => f.write_str("-")?,
                }
                f.write_str(name)?;
                empty = false;
            }
            if t != 0.0 || empty {
                if !empty {
                    f.write_str("+")?;
                }
                f.write_str(&format_fraction(t))?;
            }
        }
        Ok(())
    }
}

impl FromStr for SymOp {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SymOp::parse(s)
    }
}

impl TryFrom<String> for SymOp {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        SymOp::parse(&s)
    }
}

impl From<SymOp> for String {
    fn from(op: SymOp) -> String {
        op.to_string()
    }
}

/// Read a list of operations, one per line. Blank lines and lines starting with `#` are
/// skipped; a leading number (`1 x,y,z`, as CIF loops number them) and quotes are taken off,
/// so a `_space_group_symop` loop can be pasted. The identity must be among them (the
/// asymmetric unit is then part of the crystal), and none may repeat.
pub fn parse_list(text: &str) -> Result<Vec<SymOp>, String> {
    let mut ops: Vec<SymOp> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let mut line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // "1 'x, y, z'" → "x, y, z".
        if let Some((head, rest)) = line.split_once(char::is_whitespace)
            && !head.is_empty()
            && head.chars().all(|c| c.is_ascii_digit())
        {
            line = rest.trim();
        }
        let line = line.trim_matches(|c| c == '\'' || c == '"').trim();
        let op = SymOp::parse(line).map_err(|e| {
            t!(
                "assembly.symop.error.in_line",
                line = n + 1,
                text = line,
                error = e
            )
        })?;
        if ops.contains(&op) {
            return Err(t!(
                "assembly.symop.error.duplicate",
                line = n + 1,
                text = line
            ));
        }
        ops.push(op);
    }
    if ops.is_empty() {
        return Err(t!("assembly.symop.error.no_operations").to_owned());
    }
    if !ops.iter().any(SymOp::is_identity) {
        return Err(t!("assembly.symop.error.no_identity").to_owned());
    }
    Ok(ops)
}

/// The operations as text, one per line (what [`parse_list`] reads back).
pub fn format_list(ops: &[SymOp]) -> String {
    ops.iter()
        .map(SymOp::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The crystal system of a preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrystalSystem {
    Triclinic,
    Monoclinic,
    Orthorhombic,
}

impl CrystalSystem {
    /// The system and the cell it needs, as the panel shows it.
    pub fn label(self) -> &'static str {
        match self {
            CrystalSystem::Triclinic => t!("assembly.symop.system.triclinic"),
            CrystalSystem::Monoclinic => t!("assembly.symop.system.monoclinic"),
            CrystalSystem::Orthorhombic => t!("assembly.symop.system.orthorhombic"),
        }
    }
}

impl fmt::Display for CrystalSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl From<CrystalSystem> for String {
    fn from(system: CrystalSystem) -> String {
        system.label().to_owned()
    }
}

impl From<CrystalSystem> for egui::WidgetText {
    fn from(system: CrystalSystem) -> egui::WidgetText {
        system.label().into()
    }
}

/// A space group offered in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// The short symbol shown in the panel.
    pub name: &'static str,
    /// The full Hermann–Mauguin symbol (with the setting), written to CIF.
    pub hm: &'static str,
    /// The crystal system, whose label (with the cell it needs) is a reminder in the panel.
    pub system: CrystalSystem,
    /// The general position, as in the International Tables (centring included).
    pub ops: &'static [&'static str],
}

impl Preset {
    pub fn symops(&self) -> Vec<SymOp> {
        self.ops
            .iter()
            .map(|s| SymOp::parse(s).expect("the presets parse"))
            .collect()
    }
}

/// The presets: the space groups most organic molecular crystals take. The first seven are
/// the ones the plan asks for; P2₁/n (the setting of P2₁/c many papers report), Pna2₁, Pnma,
/// Cc and C2 are the next most frequent in the Cambridge Structural Database, so a structure
/// copied from a paper usually finds its group here.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "P1",
        hm: "P 1",
        system: CrystalSystem::Triclinic,
        ops: &["x,y,z"],
    },
    Preset {
        name: "P-1",
        hm: "P -1",
        system: CrystalSystem::Triclinic,
        ops: &["x,y,z", "-x,-y,-z"],
    },
    Preset {
        name: "P2₁",
        hm: "P 1 21 1",
        system: CrystalSystem::Monoclinic,
        ops: &["x,y,z", "-x,y+1/2,-z"],
    },
    Preset {
        name: "P2₁/c",
        hm: "P 1 21/c 1",
        system: CrystalSystem::Monoclinic,
        ops: &["x,y,z", "-x,y+1/2,-z+1/2", "-x,-y,-z", "x,-y+1/2,z+1/2"],
    },
    Preset {
        name: "P2₁/n",
        hm: "P 1 21/n 1",
        system: CrystalSystem::Monoclinic,
        ops: &[
            "x,y,z",
            "-x+1/2,y+1/2,-z+1/2",
            "-x,-y,-z",
            "x+1/2,-y+1/2,z+1/2",
        ],
    },
    Preset {
        name: "C2/c",
        hm: "C 1 2/c 1",
        system: CrystalSystem::Monoclinic,
        ops: &[
            "x,y,z",
            "-x,y,-z+1/2",
            "-x,-y,-z",
            "x,-y,z+1/2",
            "x+1/2,y+1/2,z",
            "-x+1/2,y+1/2,-z+1/2",
            "-x+1/2,-y+1/2,-z",
            "x+1/2,-y+1/2,z+1/2",
        ],
    },
    Preset {
        name: "Cc",
        hm: "C 1 c 1",
        system: CrystalSystem::Monoclinic,
        ops: &["x,y,z", "x,-y,z+1/2", "x+1/2,y+1/2,z", "x+1/2,-y+1/2,z+1/2"],
    },
    Preset {
        name: "C2",
        hm: "C 1 2 1",
        system: CrystalSystem::Monoclinic,
        ops: &["x,y,z", "-x,y,-z", "x+1/2,y+1/2,z", "-x+1/2,y+1/2,-z"],
    },
    Preset {
        name: "P2₁2₁2₁",
        hm: "P 21 21 21",
        system: CrystalSystem::Orthorhombic,
        ops: &[
            "x,y,z",
            "-x+1/2,-y,z+1/2",
            "-x,y+1/2,-z+1/2",
            "x+1/2,-y+1/2,-z",
        ],
    },
    Preset {
        name: "Pna2₁",
        hm: "P n a 21",
        system: CrystalSystem::Orthorhombic,
        ops: &[
            "x,y,z",
            "-x,-y,z+1/2",
            "x+1/2,-y+1/2,z",
            "-x+1/2,y+1/2,z+1/2",
        ],
    },
    Preset {
        name: "Pbca",
        hm: "P b c a",
        system: CrystalSystem::Orthorhombic,
        ops: &[
            "x,y,z",
            "-x+1/2,-y,z+1/2",
            "-x,y+1/2,-z+1/2",
            "x+1/2,-y+1/2,-z",
            "-x,-y,-z",
            "x+1/2,y,-z+1/2",
            "x,-y+1/2,z+1/2",
            "-x+1/2,y+1/2,z",
        ],
    },
    Preset {
        name: "Pnma",
        hm: "P n m a",
        system: CrystalSystem::Orthorhombic,
        ops: &[
            "x,y,z",
            "-x+1/2,-y,z+1/2",
            "-x,y+1/2,-z",
            "x+1/2,-y+1/2,-z+1/2",
            "-x,-y,-z",
            "x+1/2,y,-z+1/2",
            "x,-y+1/2,z",
            "-x+1/2,y+1/2,z+1/2",
        ],
    },
];

/// The preset whose operations are exactly `ops` (in any order), if any: the space group a
/// CIF names.
pub fn matching_preset(ops: &[SymOp]) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| {
        let preset = p.symops();
        preset.len() == ops.len() && preset.iter().all(|op| ops.contains(op))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_parse_and_format_back() {
        let op = SymOp::parse(" -X, y + 1/2 ,-z+0.5").unwrap();
        assert_eq!(op.rot, [[-1, 0, 0], [0, 1, 0], [0, 0, -1]]);
        assert_eq!(op.trans, [0.0, 0.5, 0.5]);
        assert_eq!(op.to_string(), "-x,y+1/2,-z+1/2");

        let hex = SymOp::parse("x-y,x,z+1/6").unwrap();
        assert_eq!(hex.rot, [[1, -1, 0], [1, 0, 0], [0, 0, 1]]);
        assert_eq!(hex.to_string(), "x-y,x,z+1/6");
        // A constant first, a negative translation (wrapped into the cell), a Unicode minus.
        let op = SymOp::parse("1/2+x, −y−1/4, 3/4-z").unwrap();
        assert_eq!(op.to_string(), "x+1/2,-y+3/4,-z+3/4");
        assert_eq!(SymOp::parse("x,y,z+1").unwrap(), SymOp::IDENTITY);
        assert_eq!(
            SymOp::parse("x,y,z+0.123").unwrap().to_string(),
            "x,y,z+0.123"
        );

        for preset in PRESETS {
            for op in preset.symops() {
                let text = op.to_string();
                assert_eq!(SymOp::parse(&text).unwrap(), op, "{text}");
            }
            assert!(preset.symops()[0].is_identity(), "{}", preset.name);
        }
        let list = parse_list("1 'x, y, z'\n\n# comment\n2 '-x, -y, -z'\n").unwrap();
        assert_eq!(format_list(&list), "x,y,z\n-x,-y,-z");
        assert_eq!(matching_preset(&list).map(|p| p.name), Some("P-1"));
        let reversed: Vec<SymOp> = list.into_iter().rev().collect();
        assert_eq!(matching_preset(&reversed).map(|p| p.hm), Some("P -1"));
    }

    #[test]
    fn malformed_operations_are_refused_with_a_message() {
        for bad in [
            "",
            "x,y",
            "x,y,z,x",
            "x,,z",
            "2x,y,z",
            "x,y,z+1/0",
            "x+x,y,z",
            "x,y,w",
            "x y,y,z",
            "x,y,z-",
            "x,x,z",
            "x,y,1/2",
        ] {
            let err = SymOp::parse(bad).unwrap_err();
            assert!(
                !err.is_empty() && !err.starts_with("assembly."),
                "{bad:?}: {err}"
            );
        }
        let err = parse_list("x,y,z\n-x,-y,-z\n-x,-y,-z").unwrap_err();
        assert!(err.starts_with("Line 3"), "{err}");
        assert!(parse_list("-x,-y,-z").unwrap_err().contains("identity"));
        assert!(parse_list("\n  \n").is_err());
        assert!(parse_list("x,y,z\nbad").unwrap_err().starts_with("Line 2"));
    }

    #[test]
    fn operations_act_on_fractional_and_cartesian_coordinates() {
        let op = SymOp::parse("-x,y+1/2,-z+1/2").unwrap();
        let f = op.apply_frac([0.1, 0.2, 0.3]);
        assert!(math::distance(f, [-0.1, 0.7, 0.2]) < 1e-12);
        let cell = Cell {
            a: 6.0,
            b: 7.0,
            c: 8.0,
            alpha: 90.0,
            beta: 105.0,
            gamma: 90.0,
        };
        let cart = op.cartesian(&cell);
        let p = cell.to_cart([0.1, 0.2, 0.3]);
        assert!(math::distance(cart.apply(p), cell.to_cart(f)) < 1e-9);
        assert!(op.fits(&cell));
        // A fourfold axis needs a = b and γ = 90°.
        let four = SymOp::parse("-y,x,z").unwrap();
        assert!(!four.fits(&cell));
        assert!(four.fits(&Cell {
            a: 5.0,
            b: 5.0,
            ..Cell::default()
        }));
        // Every preset fits a cell of its system.
        let orthogonal = Cell { beta: 90.0, ..cell };
        for p in PRESETS {
            let cell = if p.system == CrystalSystem::Orthorhombic {
                orthogonal
            } else {
                cell
            };
            assert!(p.symops().iter().all(|op| op.fits(&cell)), "{}", p.name);
        }
        // Saved as text.
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, "\"-x,y+1/2,-z+1/2\"");
        assert_eq!(serde_json::from_str::<SymOp>(&json).unwrap(), op);
        assert!(serde_json::from_str::<SymOp>("\"x,y\"").is_err());
    }
}
