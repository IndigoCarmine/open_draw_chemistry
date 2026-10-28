//! Rigid-body maths for placing molecules: 3×3 rotations, rigid transforms (rotation then
//! translation), rotations from Euler angles, an axis and angle, or two directions, and the
//! unit cell a crystal is built in.
//!
//! Coordinates are ångström; angles at the API are degrees. A [`Rigid`] maps a point `p` to
//! `rot · p + trans`, and [`Rigid::then`] chains transforms in the order they are applied, so
//! the assembly steps read as the sequences of moves the reference scripts make.

use serde::{Deserialize, Serialize};

pub type Vec3 = [f64; 3];
/// Row-major: `m[row][col]`.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

pub fn distance(a: Vec3, b: Vec3) -> f64 {
    norm(sub(a, b))
}

/// `a` scaled to length 1, or `None` for a (near) zero vector.
pub fn unit(a: Vec3) -> Option<Vec3> {
    let n = norm(a);
    (n > 1e-12).then(|| scale(a, 1.0 / n))
}

pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

pub fn mat_vec(m: &Mat3, v: Vec3) -> Vec3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

pub fn transpose(m: &Mat3) -> Mat3 {
    let mut t = [[0.0; 3]; 3];
    for (i, row) in m.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            t[j][i] = v;
        }
    }
    t
}

/// Rotation by `deg` degrees about the x axis (counter-clockwise looking down the axis).
pub fn rot_x(deg: f64) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

pub fn rot_y(deg: f64) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

pub fn rot_z(deg: f64) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// Rotation from Euler angles in degrees, extrinsic x then y then z (scipy's `"xyz"`):
/// `Rz · Ry · Rx`.
pub fn euler_xyz(deg: Vec3) -> Mat3 {
    mat_mul(&rot_z(deg[2]), &mat_mul(&rot_y(deg[1]), &rot_x(deg[0])))
}

/// Rotation by `deg` degrees about `axis` (Rodrigues' formula); the identity for a zero axis.
pub fn axis_angle(axis: Vec3, deg: f64) -> Mat3 {
    let Some(k) = unit(axis) else {
        return IDENTITY;
    };
    let (s, c) = deg.to_radians().sin_cos();
    let kx = skew(k);
    let kx2 = mat_mul(&kx, &kx);
    let mut m = IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] += s * kx[i][j] + (1.0 - c) * kx2[i][j];
        }
    }
    m
}

/// The cross-product matrix of `k`: `skew(k) · v == k × v`.
fn skew(k: Vec3) -> Mat3 {
    [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]]
}

/// The smallest rotation turning the direction of `v` into that of `target`.
///
/// Ported from gmx_harness `build.py` `align`: Rodrigues' formula about `v × target`; parallel
/// vectors give the identity, and antiparallel ones a half turn about an axis perpendicular to
/// `v` (`2 p pᵀ − I`). A zero vector gives the identity.
pub fn align(v: Vec3, target: Vec3) -> Mat3 {
    let (Some(v), Some(t)) = (unit(v), unit(target)) else {
        return IDENTITY;
    };
    let axis = cross(v, t);
    let (s, c) = (norm(axis), dot(v, t));
    if s < 1e-12 {
        if c > 0.0 {
            return IDENTITY;
        }
        let helper = if v[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let p = unit(cross(v, helper)).expect("helper is not parallel to v");
        let mut m = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = 2.0 * p[i] * p[j] - if i == j { 1.0 } else { 0.0 };
            }
        }
        return m;
    }
    let kx = skew(scale(axis, 1.0 / s));
    let kx2 = mat_mul(&kx, &kx);
    let mut m = IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] += s * kx[i][j] + (1.0 - c) * kx2[i][j];
        }
    }
    m
}

/// A rigid transform: `p ↦ rot · p + trans`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rigid {
    pub rot: Mat3,
    pub trans: Vec3,
}

impl Default for Rigid {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Rigid {
    pub const IDENTITY: Rigid = Rigid {
        rot: IDENTITY,
        trans: [0.0; 3],
    };

    pub fn rotation(rot: Mat3) -> Self {
        Self {
            rot,
            trans: [0.0; 3],
        }
    }

    pub fn translation(trans: Vec3) -> Self {
        Self {
            rot: IDENTITY,
            trans,
        }
    }

    pub fn apply(&self, p: Vec3) -> Vec3 {
        add(mat_vec(&self.rot, p), self.trans)
    }

    /// `self` followed by `next`: `p ↦ next(self(p))`.
    pub fn then(&self, next: &Rigid) -> Rigid {
        Rigid {
            rot: mat_mul(&next.rot, &self.rot),
            trans: next.apply(self.trans),
        }
    }

    /// The transform undoing `self`.
    pub fn inverse(&self) -> Rigid {
        let rot = transpose(&self.rot);
        let trans = scale(mat_vec(&rot, self.trans), -1.0);
        Rigid { rot, trans }
    }

    /// Whether `self` and `other` move every point by less than `tol` differently (on the unit
    /// scale of the rotation entries and ångström for the translation).
    pub fn approx_eq(&self, other: &Rigid, tol: f64) -> bool {
        self.rot
            .iter()
            .flatten()
            .zip(other.rot.iter().flatten())
            .chain(self.trans.iter().zip(&other.trans))
            .all(|(a, b)| (a - b).abs() < tol)
    }
}

/// The determinant of `m`.
pub fn det(m: &Mat3) -> f64 {
    dot(m[0], cross(m[1], m[2]))
}

/// The inverse of `m`, or `None` when it is (near) singular.
pub fn inverse(m: &Mat3) -> Option<Mat3> {
    let d = det(m);
    if d.abs() < 1e-12 {
        return None;
    }
    // The rows of the inverse's transpose are the cross products of the rows (the adjugate).
    let adj_t = [cross(m[1], m[2]), cross(m[2], m[0]), cross(m[0], m[1])];
    let mut inv = transpose(&adj_t);
    for row in &mut inv {
        *row = scale(*row, 1.0 / d);
    }
    Some(inv)
}

/// How far (Å) from the line through them points may lie and still count as on it (see
/// [`fit_rotation`]).
const ON_A_LINE: f64 = 1e-3;

/// The rotation turning `points` onto `targets` (pairwise, both about their centroid — the
/// caller centres them) best in the least-squares sense, `Σ |rot · p − t|²` the least, every
/// point weighted alike (Horn's quaternion method: the eigenvector of the largest eigenvalue).
/// Targets on one line fix only the line (a turn about it fits as well): the smallest rotation
/// that lays the points' spread along it is taken; one point or none gives the identity.
pub fn fit_rotation(points: &[Vec3], targets: &[Vec3]) -> Mat3 {
    // The target farthest from the centre gives the line, if they are on one.
    let far = targets
        .iter()
        .copied()
        .max_by(|a, b| norm(*a).total_cmp(&norm(*b)));
    let Some(axis) = far.and_then(unit) else {
        return IDENTITY;
    };
    if targets.iter().all(|&t| norm(cross(t, axis)) <= ON_A_LINE) {
        // Σ (rot·p)·t = (rot · Σ (t·axis) p) · axis, the most with that sum along the line.
        let spread = points
            .iter()
            .zip(targets)
            .fold([0.0; 3], |s, (&p, &t)| add(s, scale(p, dot(t, axis))));
        return align(spread, axis);
    }
    // s[i][j] = Σ p_i t_j.
    let mut s = [[0.0; 3]; 3];
    for (p, t) in points.iter().zip(targets) {
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += p[i] * t[j];
            }
        }
    }
    let [[xx, xy, xz], [yx, yy, yz], [zx, zy, zz]] = s;
    let n = [
        [xx + yy + zz, yz - zy, zx - xz, xy - yx],
        [yz - zy, xx - yy - zz, xy + yx, zx + xz],
        [zx - xz, xy + yx, -xx + yy - zz, yz + zy],
        [xy - yx, zx + xz, yz + zy, -xx - yy + zz],
    ];
    let (values, vectors) = symmetric_eigen4(n);
    let top = (0..4)
        .max_by(|&a, &b| values[a].total_cmp(&values[b]))
        .unwrap_or(0);
    let [w, x, y, z] = [0, 1, 2, 3].map(|k| vectors[k][top]);
    let q = (w * w + x * x + y * y + z * z).sqrt();
    if q <= 0.0 || !q.is_finite() {
        return IDENTITY;
    }
    let [w, x, y, z] = [w, x, y, z].map(|c| c / q);
    [
        [
            w * w + x * x - y * y - z * z,
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            w * w - x * x + y * y - z * z,
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            w * w - x * x - y * y + z * z,
        ],
    ]
}

/// The eigenvalues of the symmetric `a` and its eigenvectors (the columns), by cyclic Jacobi
/// rotations.
fn symmetric_eigen4(mut a: [[f64; 4]; 4]) -> ([f64; 4], [[f64; 4]; 4]) {
    let mut v = [[0.0; 4]; 4];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    let scale: f64 = a.iter().flatten().map(|x| x * x).sum::<f64>().sqrt();
    for _ in 0..50 {
        let off: f64 = (0..4)
            .flat_map(|i| (0..4).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum::<f64>()
            .sqrt();
        if off <= 1e-15 * scale {
            break;
        }
        for p in 0..3 {
            for q in p + 1..4 {
                if a[p][q] == 0.0 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                // a ← Jᵀ a J, v ← v J.
                for row in &mut a {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (rp, rq) = (a[p], a[q]);
                a[p] = std::array::from_fn(|k| c * rp[k] - s * rq[k]);
                a[q] = std::array::from_fn(|k| s * rp[k] + c * rq[k]);
                for row in &mut v {
                    let (vp, vq) = (row[p], row[q]);
                    row[p] = c * vp - s * vq;
                    row[q] = s * vp + c * vq;
                }
            }
        }
    }
    ([0, 1, 2, 3].map(|i| a[i][i]), v)
}

/// A unit cell: edge lengths in ångström and angles in degrees (α between b and c, β between
/// a and c, γ between a and b).
///
/// Cartesian axes follow the usual crystallographic convention (the PDB's and OpenBabel's):
/// **a** along x, **b** in the xy plane, **c** completing a right-handed set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            a: 10.0,
            b: 10.0,
            c: 10.0,
            alpha: 90.0,
            beta: 90.0,
            gamma: 90.0,
        }
    }
}

impl Cell {
    /// `1 − cos²α − cos²β − cos²γ + 2 cosα cosβ cosγ`: the squared volume of the cell with unit
    /// edges. Positive exactly when the three angles can meet at a corner.
    fn unit_volume_sq(&self) -> f64 {
        let (ca, cb, cg) = (
            self.alpha.to_radians().cos(),
            self.beta.to_radians().cos(),
            self.gamma.to_radians().cos(),
        );
        1.0 - ca * ca - cb * cb - cg * cg + 2.0 * ca * cb * cg
    }

    /// Whether the cell can exist: positive finite edges, angles strictly between 0° and 180°
    /// that can meet at a corner (each less than the sum of the other two, all three less than
    /// 360°). The message says what is wrong, in the UI language.
    pub fn check(&self) -> Result<(), String> {
        for (name, v) in [("a", self.a), ("b", self.b), ("c", self.c)] {
            if !v.is_finite() || v <= 0.0 {
                return Err(t!("assembly.error.cell_length", name = name));
            }
        }
        let angles = [("α", self.alpha), ("β", self.beta), ("γ", self.gamma)];
        for (name, v) in angles {
            if !v.is_finite() || v <= 0.0 || v >= 180.0 {
                return Err(t!("assembly.error.cell_angle", name = name));
            }
        }
        let [x, y, z] = [self.alpha, self.beta, self.gamma];
        if x + y + z >= 360.0 || x >= y + z || y >= x + z || z >= x + y {
            return Err(t!("assembly.error.cell_angles").to_owned());
        }
        if self.unit_volume_sq() <= 1e-9 {
            return Err(t!("assembly.error.cell_volume").to_owned());
        }
        Ok(())
    }

    /// The volume in Å³ (0 for an impossible cell).
    pub fn volume(&self) -> f64 {
        self.a * self.b * self.c * self.unit_volume_sq().max(0.0).sqrt()
    }

    /// The cell vectors **a**, **b**, **c** in Cartesian Å.
    pub fn vectors(&self) -> [Vec3; 3] {
        let (ca, cb) = (self.alpha.to_radians().cos(), self.beta.to_radians().cos());
        let (sg, cg) = self.gamma.to_radians().sin_cos();
        let cy = (ca - cb * cg) / sg;
        let cz = (1.0 - cb * cb - cy * cy).max(0.0).sqrt();
        [
            [self.a, 0.0, 0.0],
            [self.b * cg, self.b * sg, 0.0],
            [self.c * cb, self.c * cy, self.c * cz],
        ]
    }

    /// The matrix taking fractional coordinates to Cartesian Å: its columns are the cell
    /// vectors.
    pub fn frac_to_cart(&self) -> Mat3 {
        transpose(&self.vectors())
    }

    /// The matrix taking Cartesian Å to fractional coordinates (the identity for an impossible
    /// cell, which [`Cell::check`] refuses first).
    pub fn cart_to_frac(&self) -> Mat3 {
        inverse(&self.frac_to_cart()).unwrap_or(IDENTITY)
    }

    pub fn to_cart(&self, frac: Vec3) -> Vec3 {
        mat_vec(&self.frac_to_cart(), frac)
    }

    pub fn to_frac(&self, cart: Vec3) -> Vec3 {
        mat_vec(&self.cart_to_frac(), cart)
    }

    /// The cell `reps` times as long along each edge (a supercell).
    pub fn scaled(&self, reps: [u32; 3]) -> Cell {
        Cell {
            a: self.a * f64::from(reps[0].max(1)),
            b: self.b * f64::from(reps[1].max(1)),
            c: self.c * f64::from(reps[2].max(1)),
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        distance(a, b) < 1e-9
    }

    #[test]
    fn composing_and_inverting_rigid_transforms() {
        let a = Rigid {
            rot: euler_xyz([10.0, -35.0, 70.0]),
            trans: [1.0, 2.0, -3.0],
        };
        let b = Rigid {
            rot: axis_angle([1.0, 1.0, 0.0], 50.0),
            trans: [-0.5, 4.0, 0.25],
        };
        let p = [0.3, -1.2, 2.5];
        assert!(close(a.then(&b).apply(p), b.apply(a.apply(p))));
        assert!(close(a.inverse().apply(a.apply(p)), p));
        assert!(a.then(&a.inverse()).approx_eq(&Rigid::IDENTITY, 1e-12));
        assert!(a.inverse().then(&a).approx_eq(&Rigid::IDENTITY, 1e-12));
    }

    #[test]
    fn align_turns_one_direction_into_another() {
        for (v, t) in [
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.2, -1.0, 3.0], [-4.0, 0.5, 0.1]),
            ([1.0, 2.0, 3.0], [2.0, 4.0, 6.0]),
            // Antiparallel, along and off the x axis.
            ([1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]),
            ([0.0, 0.3, -2.0], [0.0, -0.3, 2.0]),
        ] {
            let m = align(v, t);
            let turned = mat_vec(&m, unit(v).unwrap());
            assert!(close(turned, unit(t).unwrap()), "{v:?} → {t:?}: {turned:?}");
            // A proper rotation: orthonormal with determinant +1.
            assert!(
                Rigid::rotation(mat_mul(&m, &transpose(&m))).approx_eq(&Rigid::IDENTITY, 1e-12)
            );
            let det = dot(m[0], cross(m[1], m[2]));
            assert!((det - 1.0).abs() < 1e-12, "det {det}");
        }
    }

    #[test]
    fn a_triclinic_cell_maps_fractional_to_cartesian_and_back() {
        let cell = Cell {
            a: 7.3,
            b: 9.1,
            c: 11.6,
            alpha: 81.2,
            beta: 104.5,
            gamma: 95.7,
        };
        cell.check().unwrap();
        let [va, vb, vc] = cell.vectors();
        // The edges have their lengths and the angles between them.
        assert!((norm(va) - 7.3).abs() < 1e-9 && va[1] == 0.0 && va[2] == 0.0);
        assert!((norm(vb) - 9.1).abs() < 1e-9 && vb[2] == 0.0);
        assert!((norm(vc) - 11.6).abs() < 1e-9);
        let angle = |u: Vec3, v: Vec3| (dot(u, v) / (norm(u) * norm(v))).acos().to_degrees();
        assert!((angle(vb, vc) - 81.2).abs() < 1e-9);
        assert!((angle(va, vc) - 104.5).abs() < 1e-9);
        assert!((angle(va, vb) - 95.7).abs() < 1e-9);
        // The volume is the triple product.
        assert!((cell.volume() - det(&[va, vb, vc])).abs() < 1e-9);
        for f in [[0.1, 0.2, 0.3], [-1.5, 0.75, 2.25], [0.0, 0.0, 1.0]] {
            let back = cell.to_frac(cell.to_cart(f));
            assert!(distance(back, f) < 1e-12, "{f:?} → {back:?}");
        }
        assert!(close(cell.to_cart([0.0, 0.0, 1.0]), vc));
        let m = mat_mul(&cell.frac_to_cart(), &cell.cart_to_frac());
        assert!(Rigid::rotation(m).approx_eq(&Rigid::IDENTITY, 1e-12));
    }

    #[test]
    fn cell_volumes_match_known_values() {
        // Cubic and orthorhombic: the product of the edges.
        let cubic = Cell {
            a: 5.43,
            b: 5.43,
            c: 5.43,
            ..Cell::default()
        };
        assert!((cubic.volume() - 5.43f64.powi(3)).abs() < 1e-9);
        // Monoclinic: abc·sinβ.
        let mono = Cell {
            a: 7.39,
            b: 9.42,
            c: 6.81,
            beta: 109.0,
            ..Cell::default()
        };
        let expected = 7.39 * 9.42 * 6.81 * 109.0f64.to_radians().sin();
        assert!((mono.volume() - expected).abs() < 1e-9);
        // Hexagonal: (√3/2)a²c.
        let hex = Cell {
            a: 4.0,
            b: 4.0,
            c: 6.0,
            gamma: 120.0,
            ..Cell::default()
        };
        assert!((hex.volume() - 3f64.sqrt() / 2.0 * 16.0 * 6.0).abs() < 1e-9);
        assert!((mono.scaled([2, 3, 1]).volume() - 6.0 * expected).abs() < 1e-6);
    }

    #[test]
    fn impossible_cells_are_refused_in_japanese() {
        let _lang = crate::i18n::ScopedLanguage::new(crate::i18n::Lang::Ja);
        let ok = Cell::default();
        assert!(ok.check().is_ok());
        for bad in [
            Cell { a: 0.0, ..ok },
            Cell { c: f64::NAN, ..ok },
            Cell { beta: 180.0, ..ok },
            Cell { alpha: -5.0, ..ok },
            // α ≥ β + γ: the corner cannot close.
            Cell {
                alpha: 150.0,
                beta: 40.0,
                gamma: 100.0,
                ..ok
            },
            // The angles sum to 360°: a flat cell.
            Cell {
                alpha: 120.0,
                beta: 120.0,
                gamma: 120.0,
                ..ok
            },
        ] {
            let err = bad.check().unwrap_err();
            assert!(!err.is_ascii(), "{bad:?}: {err}");
        }
    }

    #[test]
    fn matrices_invert() {
        let m = euler_xyz([20.0, -40.0, 75.0]);
        let s = [[2.0, 0.5, 0.0], [0.0, 3.0, 1.0], [0.25, 0.0, 4.0]];
        for m in [m, s] {
            let inv = inverse(&m).unwrap();
            assert!(Rigid::rotation(mat_mul(&m, &inv)).approx_eq(&Rigid::IDENTITY, 1e-12));
        }
        assert!(inverse(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]]).is_none());
    }

    /// `points` turned by `rot`, then a little off each (a shape that is not quite the same).
    fn turned(points: &[Vec3], rot: &Mat3, off: f64) -> Vec<Vec3> {
        points
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let wobble = [(i as f64).sin(), (2.0 * i as f64).cos(), 0.5];
                add(mat_vec(rot, p), scale(wobble, off))
            })
            .collect()
    }

    fn centred(points: &[Vec3]) -> Vec<Vec3> {
        let n = points.len() as f64;
        let c = scale(points.iter().fold([0.0; 3], |s, &p| add(s, p)), 1.0 / n);
        points.iter().map(|&p| sub(p, c)).collect()
    }

    fn squared_misfit(rot: &Mat3, points: &[Vec3], targets: &[Vec3]) -> f64 {
        points
            .iter()
            .zip(targets)
            .map(|(&p, &t)| {
                let d = sub(mat_vec(rot, p), t);
                dot(d, d)
            })
            .sum()
    }

    /// The fitted rotation undoes a rotation exactly, fits a slightly other shape no worse
    /// than any nearby rotation (the least-squares one), is never a reflection, and of a
    /// straight target is the smallest turn onto its line.
    #[test]
    fn fit_rotation_is_the_least_squares_rotation() {
        let shape = centred(&[
            [0.0, 0.0, 0.0],
            [1.4, 0.0, 0.0],
            [1.8, 0.9, 0.0],
            [-0.4, -0.5, 0.8],
            [0.3, 1.2, -0.6],
        ]);
        for angles in [
            [0.0, 0.0, 0.0],
            [20.0, -40.0, 75.0],
            [180.0, 0.0, 0.0],
            [0.0, 179.0, 90.0],
        ] {
            let rot = euler_xyz(angles);
            let target = turned(&shape, &rot, 0.0);
            let fit = fit_rotation(&shape, &target);
            assert!(
                Rigid::rotation(fit).approx_eq(&Rigid::rotation(rot), 1e-9),
                "{angles:?}: {fit:?}"
            );
            let target = centred(&turned(&shape, &rot, 0.05));
            let fit = fit_rotation(&shape, &target);
            assert!((det(&fit) - 1.0).abs() < 1e-9, "a rotation");
            let best = squared_misfit(&fit, &shape, &target);
            for axis in [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, -1.0],
            ] {
                for deg in [-0.5, 0.5] {
                    let near = mat_mul(&axis_angle(axis, deg), &fit);
                    assert!(squared_misfit(&near, &shape, &target) > best, "{angles:?}");
                }
            }
        }
        // A flat shape (one plane) still has one best rotation, not its mirror image.
        let flat = centred(&[
            [0.0, 0.0, 0.0],
            [1.2, 0.0, 0.0],
            [0.5, 1.0, 0.0],
            [-0.7, 0.4, 0.0],
        ]);
        let rot = euler_xyz([10.0, 50.0, -30.0]);
        let fit = fit_rotation(&flat, &turned(&flat, &rot, 0.0));
        assert!(Rigid::rotation(fit).approx_eq(&Rigid::rotation(rot), 1e-9));

        // A straight target: any turn about its line fits as well; the smallest is taken.
        let line = [[-1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let bent = [[0.0, -1.0, 0.1], [0.0, 0.0, -0.2], [0.0, 1.0, 0.1]];
        let fit = fit_rotation(&bent, &line);
        assert!(Rigid::rotation(fit).approx_eq(
            &Rigid::rotation(align([0.0, 1.0, 0.0], [1.0, 0.0, 0.0])),
            1e-9
        ));
        // One point, or none: nothing to turn.
        assert_eq!(fit_rotation(&[[0.0; 3]], &[[0.0; 3]]), IDENTITY);
        assert_eq!(fit_rotation(&[], &[]), IDENTITY);
    }

    #[test]
    fn euler_angles_apply_x_then_y_then_z() {
        // 90° about x takes y to z; then 90° about z leaves z alone.
        let m = euler_xyz([90.0, 0.0, 90.0]);
        assert!(close(mat_vec(&m, [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        // x: unchanged by Rx, then Rz takes it to y.
        assert!(close(mat_vec(&m, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        // A single angle agrees with the axis-angle form.
        let r = euler_xyz([0.0, 40.0, 0.0]);
        assert!(
            Rigid::rotation(r)
                .approx_eq(&Rigid::rotation(axis_angle([0.0, 1.0, 0.0], 40.0)), 1e-12)
        );
    }
}
