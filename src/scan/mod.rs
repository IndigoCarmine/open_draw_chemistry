//! Parameter scans (パラメータスキャン): one or two parameters of the assembly or the 3D model
//! stepped over a grid, the MM single-point energy of the whole structure at every point — or,
//! for the 3D model, a relaxed scan: the energy after minimising everything but the scanned
//! parameters ([`run::Run::start_relaxed`]).
//!
//! - [`param`]: what can be scanned, and how a value is read from and written into the
//!   design or the model's coordinates;
//! - [`Axis`] / [`grid`]: the values each parameter takes (endpoints included) and the order
//!   the points are visited in — the first parameter outermost, deterministic;
//! - [`source`]: the structure at a grid point (the design rebuilt, or the model's
//!   coordinates edited from where they were when the scan started);
//! - [`energy`]: the OpenBabel side — the structure parsed once, then only its coordinates
//!   replaced at every point (no re-parse, and OpenBabel keeps the force field's atom typing
//!   between points of the same topology);
//! - [`run`]: the scan on a background thread, its points streamed back, cancellable;
//! - [`Scan`]: a finished or partial scan's grid and energies, and its CSV.
//!
//! OpenBabel's force fields are not periodic: the energy of a crystal is that of the finite
//! supercell that was built (its repeats), with no images across the cell's faces.

use std::fmt::Write as _;

pub mod energy;
pub mod param;
pub mod run;
pub mod source;

pub use param::{Param, PoseField, StepField};
pub use run::{Progress, Run};

use crate::three_d::ForceField;

/// The most atoms a scan accepts: every point is a full single-point energy over all atom
/// pairs, so beyond this one point takes too long to be useful.
pub const MAX_ATOMS: usize = 20_000;

/// The most points along one axis.
pub const MAX_POINTS: usize = 200;

/// The most atoms a relaxed scan accepts: every point is a whole constrained minimisation of
/// up to [`crate::three_d::OPTIMIZE_STEPS`] steps over all atom pairs. Measured per point
/// (release build, an alkane's dihedral): ~0.2–0.5 s at 32 atoms, 1.2–3.7 s at 100, 5–14 s at
/// 200 and 13–48 s at 300 (MMFF94–UFF), growing as the square of the atoms; at 500 a point
/// takes ~0.5–2 min, and a 37-point dihedral scan most of an hour.
pub const MAX_RELAX_ATOMS: usize = 500;

/// The most points (all axes together) of a relaxed scan: a 10° × 10° map of two dihedrals
/// (37 × 37 = 1369) fits. Beyond, even a small molecule takes tens of minutes (see
/// [`MAX_RELAX_ATOMS`]); each point's minimised structure is also kept, for clicking.
pub const MAX_RELAX_POINTS: usize = 1_500;

/// One scanned parameter and the values it takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub param: Param,
    pub start: f64,
    pub end: f64,
    /// How many values, the endpoints included (at least 1: only `start`).
    pub points: usize,
}

impl Axis {
    /// The values, evenly spaced from `start` to `end`, both included.
    pub fn values(&self) -> Vec<f64> {
        let n = self.points.clamp(1, MAX_POINTS);
        if n == 1 {
            return vec![self.start];
        }
        let step = (self.end - self.start) / (n - 1) as f64;
        (0..n)
            .map(|i| {
                if i == n - 1 {
                    // Exactly `end`, without the rounding of `start + step × (n − 1)`.
                    self.end
                } else {
                    self.start + step * i as f64
                }
            })
            .collect()
    }
}

/// The grid of a scan over `axes` (one or two): every combination of their values, the first
/// axis outermost — point `i × n₂ + j` has the first axis's `i`th value and the second's `j`th.
pub fn grid(axes: &[Axis]) -> Vec<Vec<f64>> {
    let mut out: Vec<Vec<f64>> = vec![Vec::new()];
    for axis in axes {
        let values = axis.values();
        out = out
            .into_iter()
            .flat_map(|prefix| {
                values.iter().map(move |&v| {
                    let mut p = prefix.clone();
                    p.push(v);
                    p
                })
            })
            .collect();
    }
    out
}

/// A scan's grid and the energies found so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    /// Each axis's name (with its atoms or step), unit and values.
    pub axes: Vec<ScanAxis>,
    /// One per grid point, in [`grid`] order: `None` not computed (yet), `Some(NaN)` failed.
    pub energies: Vec<Option<f64>>,
    pub force_field: ForceField,
    /// The energy unit (MMFF94: kcal/mol, UFF and GAFF: kJ/mol).
    pub unit: String,
    /// Whether the parameters are the assembly's (else the 3D model's).
    pub assembly: bool,
    /// Whether every point was minimised with the parameters held (else a single point).
    pub relaxed: bool,
    /// One per grid point: whether its minimisation ran out of steps still moving (only ever
    /// set in a relaxed scan).
    pub unconverged: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanAxis {
    pub param: Param,
    pub label: String,
    pub values: Vec<f64>,
}

impl ScanAxis {
    pub fn unit(&self) -> &'static str {
        self.param.unit()
    }

    /// "label (unit)", for an axis title or a CSV column.
    pub fn title(&self) -> String {
        format!("{} ({})", self.label, self.unit())
    }
}

impl Scan {
    pub fn new(axes: Vec<ScanAxis>, force_field: ForceField, unit: String) -> Self {
        let total = axes.iter().map(|a| a.values.len()).product();
        let assembly = axes.first().is_some_and(|a| a.param.is_assembly());
        Self {
            axes,
            energies: vec![None; total],
            force_field,
            unit,
            assembly,
            relaxed: false,
            unconverged: vec![false; total],
        }
    }

    /// Whether point `index` was computed but its minimisation did not converge.
    pub fn is_unconverged(&self, index: usize) -> bool {
        self.unconverged.get(index).copied().unwrap_or(false)
            && self.energies.get(index).is_some_and(Option::is_some)
    }

    pub fn total(&self) -> usize {
        self.energies.len()
    }

    /// Points computed (failed ones included).
    pub fn done(&self) -> usize {
        self.energies.iter().filter(|e| e.is_some()).count()
    }

    pub fn is_2d(&self) -> bool {
        self.axes.len() == 2
    }

    /// The parameter values of point `index`.
    pub fn values_at(&self, index: usize) -> Vec<f64> {
        let mut rest = index;
        let mut out = vec![0.0; self.axes.len()];
        for (k, axis) in self.axes.iter().enumerate().rev() {
            let n = axis.values.len().max(1);
            out[k] = axis.values[rest % n];
            rest /= n;
        }
        out
    }

    /// The point at row `i` of the first axis and column `j` of the second.
    pub fn index(&self, i: usize, j: usize) -> usize {
        let n2 = self.axes.get(1).map_or(1, |a| a.values.len());
        i * n2 + j
    }

    /// The lowest finite energy and its point.
    pub fn minimum(&self) -> Option<(usize, f64)> {
        self.energies
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.filter(|e| e.is_finite()).map(|e| (i, e)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The highest finite energy.
    pub fn maximum(&self) -> Option<f64> {
        self.energies
            .iter()
            .filter_map(|e| e.filter(|e| e.is_finite()))
            .max_by(f64::total_cmp)
    }

    /// The energy of point `index` as shown: relative to the minimum when `relative`.
    /// `None` for a point not computed, NaN for one that failed.
    pub fn shown(&self, index: usize, relative: bool) -> Option<f64> {
        let e = (*self.energies.get(index)?)?;
        if relative {
            Some(e - self.minimum().map_or(0.0, |m| m.1))
        } else {
            Some(e)
        }
    }

    /// The scan as CSV: a header naming each parameter and the energy with their units, then
    /// one row per computed point in grid order (a failed point's energy is `NaN`). A relaxed
    /// scan has a last column saying whether each point's minimisation converged.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        for axis in &self.axes {
            let _ = write!(out, "{},", quote(&axis.title()));
        }
        let _ = write!(
            out,
            "{},{}",
            quote(&format!("E {} ({})", self.force_field, self.unit)),
            quote(&format!("ΔE ({})", self.unit))
        );
        if self.relaxed {
            let _ = write!(out, ",{}", quote("converged"));
        }
        let _ = writeln!(out);
        let min = self.minimum().map(|m| m.1);
        for (i, e) in self.energies.iter().enumerate() {
            let Some(e) = *e else {
                continue;
            };
            for v in self.values_at(i) {
                let _ = write!(out, "{v},");
            }
            if e.is_finite() {
                let _ = write!(out, "{e},{}", e - min.unwrap_or(e));
            } else {
                let _ = write!(out, "NaN,NaN");
            }
            if self.relaxed {
                let _ = write!(out, ",{}", !self.unconverged[i]);
            }
            let _ = writeln!(out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(start: f64, end: f64, points: usize) -> Axis {
        Axis {
            param: Param::Dihedral([0, 1, 2, 3]),
            start,
            end,
            points,
        }
    }

    #[test]
    fn axis_values_include_both_endpoints() {
        assert_eq!(
            axis(-180.0, 180.0, 5).values(),
            [-180.0, -90.0, 0.0, 90.0, 180.0]
        );
        assert_eq!(axis(3.0, 4.0, 1).values(), [3.0]);
        assert_eq!(axis(3.0, 4.0, 0).values(), [3.0], "at least one point");
        let v = axis(0.1, 0.7, 7).values();
        assert_eq!(v.len(), 7);
        assert_eq!(*v.last().unwrap(), 0.7, "exactly the end");
        // Downwards works too.
        assert_eq!(axis(2.0, 1.0, 3).values(), [2.0, 1.5, 1.0]);
        assert_eq!(axis(0.0, 1.0, 10_000).values().len(), MAX_POINTS);
    }

    #[test]
    fn a_2d_grid_is_row_major_with_the_first_axis_outermost() {
        let axes = [axis(0.0, 2.0, 3), axis(10.0, 30.0, 3)];
        let g = grid(&axes);
        assert_eq!(g.len(), 9);
        assert_eq!(g[0], [0.0, 10.0]);
        assert_eq!(g[1], [0.0, 20.0]);
        assert_eq!(g[3], [1.0, 10.0]);
        assert_eq!(g[8], [2.0, 30.0]);
        let scan = Scan::new(
            axes.iter()
                .map(|a| ScanAxis {
                    param: a.param.clone(),
                    label: "d".into(),
                    values: a.values(),
                })
                .collect(),
            ForceField::Uff,
            "kJ/mol".into(),
        );
        assert_eq!(scan.total(), 9);
        assert!(scan.is_2d());
        for (i, point) in g.iter().enumerate() {
            assert_eq!(&scan.values_at(i), point);
        }
        assert_eq!(scan.index(2, 1), 7);
        assert_eq!(grid(&[]).len(), 1, "no axes: the structure as it is");
    }

    fn scan_1d(energies: &[Option<f64>]) -> Scan {
        let mut scan = Scan::new(
            vec![ScanAxis {
                param: Param::Bond([0, 1]),
                label: "結合長 C1–C2".into(),
                values: axis(1.0, 2.0, energies.len()).values(),
            }],
            ForceField::Mmff94,
            "kcal/mol".into(),
        );
        scan.energies = energies.to_vec();
        scan
    }

    #[test]
    fn failed_points_are_nan_and_skipped_by_the_minimum() {
        let scan = scan_1d(&[Some(5.0), Some(f64::NAN), Some(2.0), None]);
        assert_eq!(scan.done(), 3);
        assert_eq!(scan.minimum(), Some((2, 2.0)));
        assert_eq!(scan.maximum(), Some(5.0));
        assert_eq!(scan.shown(0, true), Some(3.0));
        assert!(scan.shown(1, false).unwrap().is_nan());
        assert_eq!(scan.shown(3, false), None);
    }

    #[test]
    fn csv_has_units_rows_in_order_and_nan_for_failures() {
        let scan = scan_1d(&[Some(5.0), Some(f64::NAN), Some(2.0), None]);
        let csv = scan.to_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "\"結合長 C1–C2 (Å)\",\"E MMFF94 (kcal/mol)\",\"ΔE (kcal/mol)\""
        );
        assert_eq!(lines.len(), 4, "uncomputed points are left out");
        assert_eq!(lines[1], "1,5,3");
        assert!(lines[2].ends_with("NaN,NaN"), "{}", lines[2]);
        assert_eq!(lines[3], "1.6666666666666665,2,0");
    }
}
