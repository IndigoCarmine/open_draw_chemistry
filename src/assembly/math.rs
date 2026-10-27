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

/// A unit cell: edge lengths in ångström and angles in degrees. Only carried through in A1
/// (no step makes one yet); the crystal builder (A2) adds the fractional ↔ Cartesian maps.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
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
