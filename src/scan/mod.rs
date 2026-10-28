//! Parameter scans (パラメータスキャン): any number of parameters of the assembly or the 3D model
//! stepped over a grid, the MM single-point energy of the whole structure at every point — or,
//! for the 3D model, a relaxed scan: the energy after minimising everything but the scanned
//! parameters ([`run::Run::start_relaxed`]).
//!
//! - [`param`]: what can be scanned, and how a value is read from and written into the
//!   design or the model's coordinates;
//! - [`Axis`] / [`grid`]: the values each parameter takes (endpoints included) and the order
//!   the points are visited in — the first parameter outermost, deterministic — and
//!   [`Scan::section`], a curve or map of some of them, the rest sliced or minimised over;
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
use std::time::Duration;

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

/// The most points of a scan, all axes together: the largest 2D map the axes allow (200 ×
/// 200), so no one- or two-parameter scan is refused, and three or more parameters at up to
/// 34 points each (37 × 37 × 29 for two dihedrals at 10° and a third parameter). A rigid point
/// of butane takes ~1 ms (UFF) to ~4 ms (MMFF94; debug build, the drift check included), so
/// this many take ~1–3 min; a large structure takes longer per point, as a 2D map of it always
/// did, with the time left shown and Cancel keeping the points done. Each point keeps only its
/// energy (a relaxed scan, which keeps each point's structure too, is held to
/// [`MAX_RELAX_POINTS`] and [`MAX_RELAX_WORK`]).
pub const MAX_SCAN_POINTS: usize = MAX_POINTS * MAX_POINTS;

/// How many points a scan over `axes` has (saturating: a product beyond `usize` is `MAX`).
pub fn total_points(axes: &[Axis]) -> usize {
    axes.iter()
        .fold(1usize, |total, a| total.saturating_mul(a.count()))
}

/// Why a scan of `points` points is refused, if it is: more than [`MAX_SCAN_POINTS`].
pub fn points_refusal(points: usize) -> Option<String> {
    (points > MAX_SCAN_POINTS).then(|| {
        t!(
            "scan.window.too_many_points",
            points = points,
            max = MAX_SCAN_POINTS
        )
    })
}

/// The flat index (in [`grid`] order: the first axis outermost) of the point at `indices` along
/// axes of `shape` values each.
pub fn flat_index(shape: &[usize], indices: &[usize]) -> usize {
    shape
        .iter()
        .zip(indices)
        .fold(0, |flat, (&n, &i)| flat * n.max(1) + i)
}

/// The indices along axes of `shape` values each of the point at flat `index`: the inverse of
/// [`flat_index`].
pub fn grid_indices(shape: &[usize], index: usize) -> Vec<usize> {
    let mut rest = index;
    let mut out = vec![0; shape.len()];
    for (k, &n) in shape.iter().enumerate().rev() {
        let n = n.max(1);
        out[k] = rest % n;
        rest /= n;
    }
    out
}

/// The point a relaxed scan starts point `index` from: one step back along the last axis it is
/// not at the start of — the previous point, else the first of the previous row, of the
/// previous slab… Always a neighbour on the grid and computed before it; `None` for the first
/// point.
pub fn previous_neighbour(shape: &[usize], index: usize) -> Option<usize> {
    let mut indices = grid_indices(shape, index);
    let k = indices.iter().rposition(|&i| i > 0)?;
    indices[k] -= 1;
    Some(flat_index(shape, &indices))
}

/// How an axis that is not plotted is reduced to one value per plotted cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reduce {
    /// At its value of this index (the last one if past the end).
    Slice(usize),
    /// At whichever of its values the energy is lowest.
    Minimum,
}

/// The most atoms a relaxed scan accepts: every point is a whole constrained minimisation of
/// up to [`crate::three_d::OPTIMIZE_STEPS`] steps over all atom pairs. Measured per point
/// (release build, an alkane's dihedral): ~0.2–0.5 s at 32 atoms, 1.2–3.7 s at 100, 5–14 s at
/// 200 and 13–48 s at 300 (MMFF94–UFF), growing as the square of the atoms; at 500 a point
/// takes ~0.5–2 min, and a 37-point dihedral scan most of an hour.
pub const MAX_RELAX_ATOMS: usize = 500;

/// The most points (all axes together) of a relaxed scan: a 10° × 10° map of two dihedrals
/// (37 × 37 = 1369) fits — of a molecule small enough for [`MAX_RELAX_WORK`]. Beyond, even a
/// small molecule takes tens of minutes (see [`MAX_RELAX_ATOMS`]); each point's minimised
/// structure is also kept, for clicking.
pub const MAX_RELAX_POINTS: usize = 1_500;

// A relaxed scan is a scan: its cap on points is within that of any scan.
const _: () = assert!(MAX_RELAX_POINTS <= MAX_SCAN_POINTS);

/// Seconds a relaxed point takes per atom squared, on the fastest and the slowest force field
/// (MMFF94 and UFF; release build, an alkane's dihedral — see [`MAX_RELAX_ATOMS`] and the
/// `relaxed_point_timing` report): a point minimises over all atom pairs.
pub const RELAX_SECONDS_PER_ATOM2: [f64; 2] = [1.2e-4, 5.0e-4];

/// Seconds one minimiser segment (the steps between two looks at the cancel flag) takes per
/// atom squared at worst: how long Cancel can take to stop a relaxed scan.
pub const RELAX_SEGMENT_SECONDS_PER_ATOM2: f64 = 2.0e-5;

/// The most work a relaxed scan accepts, in atoms² × points: ~3 h at the slowest rate of
/// [`RELAX_SECONDS_PER_ATOM2`] (the caps on atoms and points alone would let through 500
/// atoms × 1500 points, most of two days). A 37 × 37 map fits up to ~120 atoms, a 37-point
/// dihedral up to the most atoms.
pub const MAX_RELAX_WORK: f64 = 2.0e7;

/// From this estimated time (at the slowest rate) on, the window warns before a relaxed scan
/// starts.
pub const LONG_RELAX: Duration = Duration::from_secs(10 * 60);

/// How long a relaxed scan of `atoms` atoms and `points` points should take: at the fastest
/// and at the slowest rate of [`RELAX_SECONDS_PER_ATOM2`].
pub fn relax_estimate(atoms: usize, points: usize) -> [Duration; 2] {
    let work = relax_work(atoms, points);
    RELAX_SECONDS_PER_ATOM2.map(|rate| Duration::from_secs_f64(work * rate))
}

/// How long Cancel can take to stop a relaxed scan of `atoms` atoms: one minimiser segment.
pub fn relax_cancel_latency(atoms: usize) -> Duration {
    Duration::from_secs_f64(relax_work(atoms, 1) * RELAX_SEGMENT_SECONDS_PER_ATOM2)
}

fn relax_work(atoms: usize, points: usize) -> f64 {
    (atoms as f64).powi(2) * points as f64
}

/// Why a relaxed scan of `atoms` atoms and `points` points is refused, if it is: more than
/// [`MAX_RELAX_ATOMS`] atoms, [`MAX_RELAX_POINTS`] points, or [`MAX_RELAX_WORK`] together.
pub fn relax_refusal(atoms: usize, points: usize) -> Option<String> {
    if atoms > MAX_RELAX_ATOMS {
        Some(t!(
            "scan.window.relax_too_many_atoms",
            atoms = atoms,
            max = MAX_RELAX_ATOMS
        ))
    } else if points > MAX_RELAX_POINTS {
        Some(t!(
            "scan.window.relax_too_many_points",
            points = points,
            max = MAX_RELAX_POINTS
        ))
    } else if relax_work(atoms, points) > MAX_RELAX_WORK {
        let [low, high] = relax_estimate(atoms, points);
        Some(t!(
            "scan.window.relax_too_much_work",
            points = points,
            atoms = atoms,
            low = run::format_duration(low),
            high = run::format_duration(high),
            max = (MAX_RELAX_WORK / relax_work(atoms, 1)) as usize
        ))
    } else {
        None
    }
}

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
    /// How many values it takes (`points`, from 1 to [`MAX_POINTS`]).
    pub fn count(&self) -> usize {
        self.points.clamp(1, MAX_POINTS)
    }

    /// The values, evenly spaced from `start` to `end`, both included.
    pub fn values(&self) -> Vec<f64> {
        let n = self.count();
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

/// The grid of a scan over `axes` (any number): every combination of their values, the first
/// axis outermost, the last turning fastest — of two, point `i × n₂ + j` has the first axis's
/// `i`th value and the second's `j`th ([`flat_index`]).
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
        self.indices_at(index)
            .into_iter()
            .zip(&self.axes)
            .map(|(i, axis)| axis.values[i])
            .collect()
    }

    /// How many values each axis takes.
    pub fn shape(&self) -> Vec<usize> {
        self.axes.iter().map(|a| a.values.len()).collect()
    }

    /// The index along each axis of point `index` (the inverse of [`Scan::index_of`]).
    pub fn indices_at(&self, index: usize) -> Vec<usize> {
        grid_indices(&self.shape(), index)
    }

    /// The point at index `indices[k]` along each axis `k`.
    pub fn index_of(&self, indices: &[usize]) -> usize {
        flat_index(&self.shape(), indices)
    }

    /// The point at row `i` of the first axis and column `j` of the second (of a 2D scan; see
    /// [`Scan::index_of`] for any number of axes).
    pub fn index(&self, i: usize, j: usize) -> usize {
        let n2 = self.axes.get(1).map_or(1, |a| a.values.len());
        i * n2 + j
    }

    /// The scan seen along the axes `shown` (one for a curve, two for a map): for each of
    /// their cells — row-major, the first shown axis outermost — the point it shows. Every
    /// other axis `k` is reduced by `rest[k]` (missing: [`Reduce::Minimum`]): held at one
    /// value, or at the value where the energy is lowest. That point is a real one, so its
    /// values can be shown and applied. Of the candidates, the lowest finite energy wins, then a
    /// failed point, then one not computed yet (the first of equals).
    pub fn section(&self, shown: &[usize], rest: &[Reduce]) -> Vec<usize> {
        let shape = self.shape();
        let cell_shape: Vec<usize> = shown.iter().map(|&k| shape[k]).collect();
        let mut cells: Vec<Option<usize>> = vec![None; cell_shape.iter().product()];
        // Lower is better: computed with a finite energy, then failed, then not computed.
        let rank = |i: usize| match self.energies[i] {
            Some(e) if e.is_finite() => (0, e),
            Some(_) => (1, 0.0),
            None => (2, 0.0),
        };
        // Each axis's slice, or `None` for one shown or minimised over.
        let slice: Vec<Option<usize>> = (0..shape.len())
            .map(|k| match rest.get(k) {
                Some(Reduce::Slice(at)) if !shown.contains(&k) => {
                    Some((*at).min(shape[k].saturating_sub(1)))
                }
                _ => None,
            })
            .collect();
        // The points in grid order, their indices turning as an odometer (this runs every
        // frame, over up to `MAX_SCAN_POINTS` points).
        let mut indices = vec![0; shape.len()];
        for index in 0..self.total() {
            let held = indices
                .iter()
                .zip(&slice)
                .all(|(&i, s)| s.is_none_or(|s| s == i));
            if held {
                let cell = shown
                    .iter()
                    .fold(0, |flat, &k| flat * shape[k].max(1) + indices[k]);
                let cell = &mut cells[cell];
                let better = cell.is_none_or(|best| {
                    let ((r, e), (best_r, best_e)) = (rank(index), rank(best));
                    r < best_r || (r == best_r && r == 0 && e < best_e)
                });
                if better {
                    *cell = Some(index);
                }
            }
            for k in (0..shape.len()).rev() {
                indices[k] += 1;
                if indices[k] < shape[k] {
                    break;
                }
                indices[k] = 0;
            }
        }
        // Every cell has at least one point: each axis has at least one value.
        cells.into_iter().map(|c| c.unwrap_or(0)).collect()
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
    /// scan has a last column saying whether each point's minimisation converged (`false` for
    /// a failed point).
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
                // A failed point did not converge to anything.
                let _ = write!(out, ",{}", e.is_finite() && !self.unconverged[i]);
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

    /// A relaxed scan's failed point did not converge: its CSV row says so, as an unconverged
    /// one's does, while a converged one's says it did.
    #[test]
    fn a_relaxed_csv_does_not_call_failed_points_converged() {
        let mut scan = scan_1d(&[Some(5.0), Some(f64::NAN), Some(2.0), None]);
        scan.relaxed = true;
        scan.unconverged[2] = true;
        let csv = scan.to_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].ends_with(",\"converged\""), "{}", lines[0]);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1], "1,5,3,true");
        assert!(lines[2].ends_with("NaN,NaN,false"), "{}", lines[2]);
        assert_eq!(lines[3], "1.6666666666666665,2,0,false");
    }

    fn scan_nd(sizes: &[usize]) -> Scan {
        let axes = sizes
            .iter()
            .enumerate()
            .map(|(k, &n)| ScanAxis {
                param: Param::Bond([k, k + 1]),
                label: format!("r{}", k + 1),
                values: axis(1.0, 1.0 + (n.max(2) - 1) as f64 / 10.0, n).values(),
            })
            .collect();
        Scan::new(axes, ForceField::Uff, "kJ/mol".into())
    }

    /// Every point's indices along the axes and its place in the grid round-trip, in
    /// [`grid`]'s order (the first axis outermost), for 1 to 4 axes; the 1D and 2D order is
    /// the one [`Scan::index`] has always had.
    #[test]
    fn n_dimensional_indices_round_trip_in_grid_order() {
        for sizes in [&[5][..], &[3, 4], &[2, 3, 4], &[3, 1, 2], &[2, 2, 3, 2]] {
            let scan = scan_nd(sizes);
            assert_eq!(scan.shape(), sizes);
            assert_eq!(scan.total(), sizes.iter().product::<usize>());
            let mut expected = vec![0; sizes.len()];
            for index in 0..scan.total() {
                let indices = scan.indices_at(index);
                assert_eq!(indices, expected, "{sizes:?} point {index}");
                assert_eq!(scan.index_of(&indices), index, "{sizes:?} {indices:?}");
                let values: Vec<f64> = indices
                    .iter()
                    .zip(&scan.axes)
                    .map(|(&i, a)| a.values[i])
                    .collect();
                assert_eq!(scan.values_at(index), values);
                // Odometer: the last axis turns fastest.
                for k in (0..sizes.len()).rev() {
                    expected[k] += 1;
                    if expected[k] < sizes[k] {
                        break;
                    }
                    expected[k] = 0;
                }
            }
            let axes: Vec<Axis> = scan
                .axes
                .iter()
                .map(|a| axis(a.values[0], *a.values.last().unwrap(), a.values.len()))
                .collect();
            assert_eq!(grid(&axes).len(), scan.total());
        }
        let scan = scan_nd(&[3, 4]);
        for i in 0..3 {
            for j in 0..4 {
                assert_eq!(scan.index_of(&[i, j]), scan.index(i, j));
                assert_eq!(scan.index(i, j), i * 4 + j);
            }
        }
        let scan = scan_nd(&[6]);
        assert_eq!(scan.index_of(&[4]), 4);
    }

    /// A relaxed point starts from the point one step back along the last axis it is not at
    /// the start of: in 1D and 2D the previous point, or the first of the previous row, as
    /// always; in more, always a grid neighbour computed before it.
    #[test]
    fn each_point_warm_starts_from_a_neighbour_computed_before_it() {
        assert_eq!(previous_neighbour(&[4], 0), None);
        assert_eq!(previous_neighbour(&[4], 3), Some(2));
        let (rows, cols) = (3, 4);
        for i in 0..rows {
            for j in 0..cols {
                let want = if j > 0 {
                    Some(i * cols + j - 1)
                } else if i > 0 {
                    Some((i - 1) * cols)
                } else {
                    None
                };
                assert_eq!(previous_neighbour(&[rows, cols], i * cols + j), want);
            }
        }
        for shape in [&[2, 3, 4][..], &[3, 1, 2], &[2, 2, 2, 3]] {
            let total: usize = shape.iter().product();
            assert_eq!(previous_neighbour(shape, 0), None);
            for index in 1..total {
                let from = previous_neighbour(shape, index).expect("a neighbour");
                assert!(from < index, "{shape:?}: {index} from {from}");
                let (a, b) = (grid_indices(shape, index), grid_indices(shape, from));
                let steps: usize = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).sum();
                assert_eq!(steps, 1, "{shape:?}: {a:?} from {b:?}");
            }
        }
        assert_eq!(
            previous_neighbour(&[2, 3, 4], flat_index(&[2, 3, 4], &[1, 0, 0])),
            Some(0),
            "a new slab starts from the previous slab's first point"
        );
    }

    /// The energies of three axes, `100 a + 10 b + c` with the indices `[a, b, c]`, so every
    /// minimum is at index 0 of the axes reduced — unless overridden.
    fn scan_3d() -> Scan {
        let mut scan = scan_nd(&[2, 3, 4]);
        for i in 0..scan.total() {
            let [a, b, c] = scan.indices_at(i)[..] else {
                panic!()
            };
            scan.energies[i] = Some((100 * a + 10 * b + c) as f64);
        }
        scan
    }

    #[test]
    fn a_slice_shows_the_points_at_the_chosen_values() {
        let scan = scan_3d();
        // Axes 0 (across) and 2 (up), axis 1 at its third value.
        let cells = scan.section(
            &[0, 2],
            &[Reduce::Minimum, Reduce::Slice(2), Reduce::Minimum],
        );
        assert_eq!(cells.len(), 2 * 4);
        for a in 0..2 {
            for c in 0..4 {
                assert_eq!(cells[a * 4 + c], scan.index_of(&[a, 2, c]));
            }
        }
        // A curve along axis 1 with both others sliced; an index past the end is the last.
        let cells = scan.section(&[1], &[Reduce::Slice(1), Reduce::Minimum, Reduce::Slice(9)]);
        assert_eq!(
            cells,
            (0..3)
                .map(|b| scan.index_of(&[1, b, 3]))
                .collect::<Vec<_>>()
        );
        // Shown in another order: the first shown axis is the outer one.
        let cells = scan.section(&[2, 0], &[Reduce::Slice(0); 3]);
        assert_eq!(cells[1], scan.index_of(&[1, 0, 0]));
        assert_eq!(cells[2], scan.index_of(&[0, 0, 1]));
    }

    /// Projecting takes, for each cell, the lowest energy along the axes projected over; the
    /// cell is that very point (for its values and clicking). Failed points lose to computed
    /// ones, and uncomputed ones to both.
    #[test]
    fn a_minimum_projection_shows_the_lowest_point_along_the_projected_axes() {
        let mut scan = scan_3d();
        let low = scan.index_of(&[1, 2, 3]);
        scan.energies[low] = Some(-50.0);
        let cells = scan.section(&[0, 1], &[Reduce::Minimum; 3]);
        assert_eq!(cells.len(), 6);
        for a in 0..2 {
            for b in 0..3 {
                let at = cells[a * 3 + b];
                let want = if (a, b) == (1, 2) {
                    low
                } else {
                    scan.index_of(&[a, b, 0])
                };
                assert_eq!(at, want, "cell {a},{b}");
                let along: Vec<f64> = (0..4)
                    .map(|c| scan.energies[scan.index_of(&[a, b, c])].unwrap())
                    .collect();
                assert_eq!(
                    scan.energies[at],
                    Some(along.iter().copied().fold(f64::INFINITY, f64::min))
                );
            }
        }
        // Over two axes at once: a curve along axis 1, the global minimum's cell is it.
        let cells = scan.section(&[1], &[Reduce::Minimum; 3]);
        assert_eq!(cells[2], low);
        assert_eq!(scan.minimum().unwrap().0, low);
        // A failed point never wins over a computed one, an uncomputed one over neither.
        let mut scan = scan_3d();
        let first = scan.index_of(&[0, 0, 0]);
        scan.energies[first] = Some(f64::NAN);
        let cells = scan.section(&[0, 1], &[Reduce::Minimum; 3]);
        assert_eq!(cells[0], scan.index_of(&[0, 0, 1]));
        for c in 0..4 {
            let i = scan.index_of(&[0, 0, c]);
            scan.energies[i] = if c == 2 { Some(f64::NAN) } else { None };
        }
        let cells = scan.section(&[0, 1], &[Reduce::Minimum; 3]);
        assert_eq!(cells[0], scan.index_of(&[0, 0, 2]), "failed, not missing");
        for c in 0..4 {
            let i = scan.index_of(&[0, 0, c]);
            scan.energies[i] = None;
        }
        let cells = scan.section(&[0, 1], &[Reduce::Minimum; 3]);
        assert_eq!(scan.energies[cells[0]], None, "nothing computed there yet");
        // Mixed: axis 2 sliced, axis 1 projected.
        let scan = scan_3d();
        let cells = scan.section(&[0], &[Reduce::Minimum, Reduce::Minimum, Reduce::Slice(3)]);
        assert_eq!(
            cells,
            [scan.index_of(&[0, 0, 3]), scan.index_of(&[1, 0, 3])]
        );
    }

    /// With every axis shown, the section is the scan itself, point for point.
    #[test]
    fn a_section_of_all_axes_is_the_whole_scan() {
        let scan = scan_nd(&[3, 4]);
        assert_eq!(scan.section(&[0, 1], &[]), (0..12).collect::<Vec<_>>());
        let scan = scan_nd(&[5]);
        assert_eq!(scan.section(&[0], &[]), (0..5).collect::<Vec<_>>());
    }

    /// The CSV of a three-axis scan: a column per axis, then the energies, one row per
    /// computed point in grid order with that point's values.
    #[test]
    fn a_3d_csv_has_every_axis_and_matches_the_scan() {
        let mut scan = scan_3d();
        scan.energies[5] = None;
        scan.energies[7] = Some(f64::NAN);
        let csv = scan.to_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "\"r1 (Å)\",\"r2 (Å)\",\"r3 (Å)\",\"E UFF (kJ/mol)\",\"ΔE (kJ/mol)\""
        );
        let rows = &lines[1..];
        assert_eq!(rows.len(), scan.done());
        let computed: Vec<usize> = (0..scan.total())
            .filter(|&i| scan.energies[i].is_some())
            .collect();
        let min = scan.minimum().unwrap().1;
        for (row, &i) in rows.iter().zip(&computed) {
            let cells: Vec<&str> = row.split(',').collect();
            assert_eq!(cells.len(), 5, "{row}");
            for (k, v) in scan.values_at(i).iter().enumerate() {
                assert_eq!(cells[k].parse::<f64>().unwrap(), *v, "{row}");
            }
            let e = scan.energies[i].unwrap();
            if e.is_nan() {
                assert_eq!(&cells[3..], ["NaN", "NaN"]);
            } else {
                assert_eq!(cells[3].parse::<f64>().unwrap(), e);
                assert_eq!(cells[4].parse::<f64>().unwrap(), e - min);
            }
        }
    }

    /// The cap on a scan's points (all axes together) counts without overflowing, lets every
    /// 1D and 2D scan the axes allow through, and refuses beyond.
    #[test]
    fn scans_of_too_many_points_are_refused() {
        let a = |n| axis(0.0, 1.0, n);
        assert_eq!(total_points(&[a(37), a(37), a(11)]), 37 * 37 * 11);
        assert_eq!(
            total_points(&[a(0), a(4)]),
            4,
            "an axis has at least one point"
        );
        assert_eq!(total_points(&[]), 1);
        let huge = vec![a(MAX_POINTS); 12];
        assert_eq!(total_points(&huge), usize::MAX, "saturates");
        assert_eq!(points_refusal(total_points(&[a(MAX_POINTS)])), None);
        assert_eq!(
            points_refusal(total_points(&[a(MAX_POINTS), a(MAX_POINTS)])),
            None,
            "the largest 2D map"
        );
        assert_eq!(points_refusal(MAX_SCAN_POINTS), None);
        let why = points_refusal(MAX_SCAN_POINTS + 1).unwrap();
        assert!(why.contains("40001") && why.contains("40000"), "{why}");
        assert!(points_refusal(total_points(&huge)).is_some());
    }

    /// The work guard: the caps on atoms and points alone, then their product, which the
    /// largest atoms × the most points exceed many times over.
    #[test]
    fn relaxed_scans_too_long_to_run_are_refused_before_starting() {
        assert_eq!(relax_refusal(32, 37), None);
        assert_eq!(
            relax_refusal(MAX_RELAX_ATOMS, 37),
            None,
            "a dihedral of the most atoms"
        );
        assert_eq!(relax_refusal(100, 37 * 37), None, "a 10° map of 100 atoms");
        let too_many_atoms = relax_refusal(MAX_RELAX_ATOMS + 1, 2).unwrap();
        assert!(
            too_many_atoms.contains("Too many atoms"),
            "{too_many_atoms}"
        );
        let too_many_points = relax_refusal(10, MAX_RELAX_POINTS + 1).unwrap();
        assert!(
            too_many_points.contains("Too many points"),
            "{too_many_points}"
        );
        // Each alone allowed, together ~2 days.
        let why = relax_refusal(MAX_RELAX_ATOMS, MAX_RELAX_POINTS).unwrap();
        assert!(why.contains("1500 points"), "{why}");
        assert!(why.contains("at most 80 points"), "{why}");
        let [low, high] = relax_estimate(MAX_RELAX_ATOMS, MAX_RELAX_POINTS);
        assert!(low < high);
        assert!(high > Duration::from_secs(24 * 3600), "{high:?}");
        // Right at the budget is allowed; one point more is not.
        let atoms = 200;
        let most = (MAX_RELAX_WORK / (atoms * atoms) as f64) as usize;
        assert_eq!(relax_refusal(atoms, most), None);
        assert!(relax_refusal(atoms, most + 1).is_some());
        // Whatever is allowed ends within ~3 h even at the slowest rate.
        assert!(relax_estimate(atoms, most)[1] <= Duration::from_secs(3 * 3600));
    }

    /// The estimate warns of a long run (and how long Cancel takes) before it starts.
    #[test]
    fn the_relaxed_estimate_grows_as_atoms_squared_times_points() {
        let [low, high] = relax_estimate(100, 10);
        let [low4, high4] = relax_estimate(200, 10);
        assert!((low4.as_secs_f64() / low.as_secs_f64() - 4.0).abs() < 1e-9);
        assert!((high4.as_secs_f64() / high.as_secs_f64() - 4.0).abs() < 1e-9);
        let [_, double] = relax_estimate(100, 20);
        assert!((double.as_secs_f64() / high.as_secs_f64() - 2.0).abs() < 1e-9);
        assert!(
            relax_estimate(32, 13)[1] < LONG_RELAX,
            "butane-sized: short"
        );
        assert!(
            relax_estimate(MAX_RELAX_ATOMS, 37)[1] > LONG_RELAX,
            "warned"
        );
        assert!(relax_cancel_latency(MAX_RELAX_ATOMS) < Duration::from_secs(30));
        assert!(relax_cancel_latency(MAX_RELAX_ATOMS) > relax_cancel_latency(100));
    }
}
