//! Placing molecules by hand (自由配置): a hand-placed molecule's pose as the panel shows it
//! (where its centroid is, and Euler angles), the moves a mouse drag makes, and a free spot to
//! put a new molecule.
//!
//! A hand-placed molecule is an [`Instance`]: its component's oriented atoms moved by a rigid
//! transform. The panel edits it as a pose — the centroid's position and the rotation about
//! the centroid — so turning it never moves it and moving it never turns it.
//!
//! The drag maths takes the camera as a [`ScreenBasis`] (its right, up and toward-the-viewer
//! directions in the scene, and how many ångström a screen point spans at the molecule) so it
//! is tested without a window: Shift+drag moves the molecule with the pointer in the view
//! plane, Alt+drag turns it like a trackball — dragging right swings its near side right.

use super::build::Built;
use super::clash::{self, Grid};
use super::math::{self, Mat3, Rigid, Vec3};
use super::model::{Component, Instance};
use crate::three_d::geom::{self, ClashKind};

/// How far an Alt+drag turns the molecule, in degrees per screen point.
pub const DEGREES_PER_POINT: f64 = 0.5;

/// The camera as seen from the molecule: unit directions in the scene, and the scale at the
/// molecule's depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenBasis {
    /// To the right of the screen.
    pub right: Vec3,
    /// Up the screen.
    pub up: Vec3,
    /// Out of the screen, toward the viewer.
    pub toward_viewer: Vec3,
    /// Ångström per screen point at the molecule.
    pub angstrom_per_point: f64,
}

impl ScreenBasis {
    /// The scene vector of a screen drag `delta` (points, y down), unscaled.
    fn drag_direction(&self, [dx, dy]: [f64; 2]) -> Vec3 {
        math::sub(math::scale(self.right, dx), math::scale(self.up, dy))
    }
}

/// The move a Shift+drag of `delta` points makes: the molecule follows the pointer.
pub fn drag_translation(basis: &ScreenBasis, delta: [f64; 2]) -> Vec3 {
    math::scale(basis.drag_direction(delta), basis.angstrom_per_point)
}

/// The turn an Alt+drag of `delta` points makes, about an axis through the molecule: the axis
/// lies in the screen plane at right angles to the drag, so the side facing the viewer moves
/// with the pointer (a trackball). [`DEGREES_PER_POINT`] per point of drag.
pub fn drag_rotation(basis: &ScreenBasis, delta: [f64; 2]) -> Mat3 {
    let along = basis.drag_direction(delta);
    let axis = math::cross(basis.toward_viewer, along);
    let degrees = delta[0].hypot(delta[1]) * DEGREES_PER_POINT;
    math::axis_angle(axis, degrees)
}

/// `t` followed by a turn `rot` about `centre`.
pub fn rotate_about(t: &Rigid, centre: Vec3, rot: Mat3) -> Rigid {
    t.then(&Rigid::translation(math::scale(centre, -1.0)))
        .then(&Rigid::rotation(rot))
        .then(&Rigid::translation(centre))
}

/// `t` followed by a move `by`.
pub fn translate(t: &Rigid, by: Vec3) -> Rigid {
    t.then(&Rigid::translation(by))
}

/// The mean of `points` (the origin for none).
pub fn centroid(points: &[Vec3]) -> Vec3 {
    if points.is_empty() {
        return [0.0; 3];
    }
    let sum = points.iter().fold([0.0; 3], |s, p| math::add(s, *p));
    math::scale(sum, 1.0 / points.len() as f64)
}

/// A component's oriented atom positions (as loaded, when its orientation is unusable — the
/// build reports that).
pub fn local_positions(component: &Component) -> Vec<Vec3> {
    component
        .oriented_positions()
        .unwrap_or_else(|_| component.atoms.iter().map(|a| a.pos).collect())
}

/// A hand-placed molecule as the panel edits it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Where the molecule's centroid is (Å).
    pub position: Vec3,
    /// The rotation about the centroid: Euler angles in degrees, extrinsic x, y, z (as
    /// [`math::euler_xyz`]).
    pub euler_deg: Vec3,
}

/// The pose of a molecule moved by `t` whose atoms have their centroid at `local_centroid`.
pub fn pose_of(t: &Rigid, local_centroid: Vec3) -> Pose {
    Pose {
        position: t.apply(local_centroid),
        euler_deg: euler_xyz_of(&t.rot),
    }
}

/// The transform giving `pose` (the inverse of [`pose_of`]).
pub fn transform_of(pose: &Pose, local_centroid: Vec3) -> Rigid {
    let rot = math::euler_xyz(pose.euler_deg);
    Rigid {
        rot,
        trans: math::sub(pose.position, math::mat_vec(&rot, local_centroid)),
    }
}

/// Euler angles (degrees, extrinsic xyz) of a rotation: the inverse of [`math::euler_xyz`].
/// y is kept within ±90°; at ±90° (gimbal lock) the z angle is taken as 0.
pub fn euler_xyz_of(m: &Mat3) -> Vec3 {
    // m = Rz·Ry·Rx: m[2][0] = −sin y, m[2][1] = cos y sin x, m[1][0] = sin z cos y.
    let sy = (-m[2][0]).clamp(-1.0, 1.0);
    let y = sy.asin();
    if y.cos() > 1e-6 {
        let x = m[2][1].atan2(m[2][2]);
        let z = m[1][0].atan2(m[0][0]);
        [x.to_degrees(), y.to_degrees(), z.to_degrees()]
    } else {
        let x = (-m[1][2]).atan2(m[1][1]);
        [x.to_degrees(), y.to_degrees(), 0.0]
    }
}

/// Each atom's clash kind in a component, from its bonds.
pub fn component_kinds(component: &Component) -> Vec<ClashKind> {
    let n = component.atoms.len();
    let mut neighbors: Vec<Vec<&str>> = vec![Vec::new(); n];
    for b in &component.bonds {
        if b.begin < n && b.end < n {
            neighbors[b.begin].push(&component.atoms[b.end].element);
            neighbors[b.end].push(&component.atoms[b.begin].element);
        }
    }
    component
        .atoms
        .iter()
        .zip(&neighbors)
        .map(|(a, n)| ClashKind::of(&a.element, n.iter().copied()))
        .collect()
}

/// How far a new molecule may be pushed looking for room, in 1 Å steps.
const SEARCH_STEPS: usize = 200;

/// Where to put the centroid of a new molecule so it clashes with nothing in `built`: `start`,
/// or the first point after it in 1 Å steps along `direction` that is clear (`start` again if
/// none within [`SEARCH_STEPS`] is). `shape` is the new molecule's atoms about its centroid.
pub fn free_spot(
    built: &Built,
    shape: &[Vec3],
    shape_kinds: &[ClashKind],
    start: Vec3,
    direction: Vec3,
) -> Vec3 {
    if built.atoms.is_empty() || shape.is_empty() {
        return start;
    }
    let kinds = clash::kinds(built);
    let positions: Vec<Vec3> = built.atoms.iter().map(|a| a.pos).collect();
    let largest = kinds
        .iter()
        .chain(shape_kinds)
        .map(|k| k.radius)
        .fold(0.0, f64::max);
    let grid = Grid::new(&positions, geom::CLASH_FACTOR * 2.0 * largest, None);
    let step = math::unit(direction).unwrap_or([1.0, 0.0, 0.0]);
    for s in 0..=SEARCH_STEPS {
        let at = math::add(start, math::scale(step, s as f64));
        let blocked = shape.iter().zip(shape_kinds).any(|(p, kind)| {
            grid.any_near(math::add(at, *p), |j| geom::clash_limit(*kind, kinds[j]))
        });
        if !blocked {
            return at;
        }
    }
    start
}

/// The farthest any atom is from the centroid.
pub fn radius(points: &[Vec3]) -> f64 {
    let c = centroid(points);
    points
        .iter()
        .map(|p| math::distance(*p, c))
        .fold(0.0, f64::max)
}

/// A new hand-placed molecule of `component` (index `index`), turned by `rot` about its
/// centroid and put where it clashes with nothing in `built`: at `near` or further along
/// `direction`.
pub fn place_new(
    built: &Built,
    component: &Component,
    index: usize,
    rot: Mat3,
    near: Vec3,
    direction: Vec3,
) -> Instance {
    let local = local_positions(component);
    let c = centroid(&local);
    let shape: Vec<Vec3> = local
        .iter()
        .map(|p| math::mat_vec(&rot, math::sub(*p, c)))
        .collect();
    let at = free_spot(built, &shape, &component_kinds(component), near, direction);
    Instance {
        component: index,
        transform: Rigid {
            rot,
            trans: math::sub(at, math::mat_vec(&rot, c)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::build::build;
    use crate::assembly::model::Design;
    use crate::three_d::{Atom3d, Bond3d};

    fn close(a: Vec3, b: Vec3) -> bool {
        math::distance(a, b) < 1e-9
    }

    /// A camera looking down −z with x to the right: the textbook view.
    fn front() -> ScreenBasis {
        ScreenBasis {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            toward_viewer: [0.0, 0.0, 1.0],
            angstrom_per_point: 0.05,
        }
    }

    #[test]
    fn a_shift_drag_moves_with_the_pointer_in_the_view_plane() {
        let b = front();
        // Right 20 points, down 10 (screen y grows downward).
        assert!(close(drag_translation(&b, [20.0, 10.0]), [1.0, -0.5, 0.0]));
        // Seen from the side the same drag moves along the side's own axes.
        let side = ScreenBasis {
            right: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            toward_viewer: [1.0, 0.0, 0.0],
            angstrom_per_point: 0.1,
        };
        let d = drag_translation(&side, [10.0, 0.0]);
        assert!(close(d, [0.0, 0.0, -1.0]));
        assert!(
            math::dot(d, side.toward_viewer).abs() < 1e-12,
            "never in depth"
        );
    }

    #[test]
    fn an_alt_drag_turns_the_near_side_with_the_pointer() {
        for basis in [
            front(),
            ScreenBasis {
                right: [0.0, 0.0, -1.0],
                up: [0.0, 1.0, 0.0],
                toward_viewer: [1.0, 0.0, 0.0],
                angstrom_per_point: 0.1,
            },
        ] {
            // Dragging right swings the point nearest the viewer to the right…
            let r = drag_rotation(&basis, [30.0, 0.0]);
            let near = math::mat_vec(&r, basis.toward_viewer);
            assert!(math::dot(near, basis.right) > 0.2, "{near:?}");
            // …and dragging down swings it down; the axis is in the screen plane.
            let r = drag_rotation(&basis, [0.0, 30.0]);
            let near = math::mat_vec(&r, basis.toward_viewer);
            assert!(math::dot(near, basis.up) < -0.2, "{near:?}");
            assert!(
                close(math::mat_vec(&r, basis.right), basis.right),
                "about the right axis"
            );
        }
        // The angle grows with the drag: 0.5° a point.
        let r = drag_rotation(&front(), [90.0, 0.0]);
        let x = math::mat_vec(&r, [0.0, 0.0, 1.0]);
        assert!((x[0] - 45f64.to_radians().sin()).abs() < 1e-9);
        assert_eq!(drag_rotation(&front(), [0.0, 0.0]), math::IDENTITY);
    }

    #[test]
    fn turning_about_the_centroid_keeps_it_in_place() {
        let t = Rigid {
            rot: math::euler_xyz([10.0, 20.0, 30.0]),
            trans: [1.0, 2.0, 3.0],
        };
        let local_c = [0.5, -0.5, 1.0];
        let c = t.apply(local_c);
        let turned = rotate_about(&t, c, math::axis_angle([1.0, 1.0, 0.0], 70.0));
        assert!(close(turned.apply(local_c), c));
        let moved = translate(&t, [0.0, 0.0, 5.0]);
        assert!(close(moved.apply(local_c), math::add(c, [0.0, 0.0, 5.0])));
    }

    #[test]
    fn a_pose_round_trips_through_its_transform() {
        let c = [0.3, 1.1, -0.4];
        for euler in [
            [10.0, 20.0, 30.0],
            [-170.0, 60.0, 95.0],
            [0.0, 0.0, 0.0],
            [45.0, -89.0, -120.0],
        ] {
            let pose = Pose {
                position: [4.0, -2.0, 7.5],
                euler_deg: euler,
            };
            let back = pose_of(&transform_of(&pose, c), c);
            assert!(close(back.position, pose.position));
            assert!(
                close(back.euler_deg, euler),
                "{euler:?} → {:?}",
                back.euler_deg
            );
        }
        // At gimbal lock the angles differ but the rotation is the same.
        let locked = math::euler_xyz([30.0, 90.0, 40.0]);
        let again = math::euler_xyz(euler_xyz_of(&locked));
        assert!(Rigid::rotation(locked).approx_eq(&Rigid::rotation(again), 1e-9));
    }

    fn methane_like() -> Component {
        let atom = |e: &str, pos| Atom3d {
            element: e.into(),
            pos,
        };
        Component {
            name: "ch".into(),
            atoms: vec![atom("C", [0.0; 3]), atom("H", [1.09, 0.0, 0.0])],
            bonds: vec![Bond3d {
                begin: 0,
                end: 1,
                order: 1,
            }],
            orient: None,
        }
    }

    #[test]
    fn a_new_molecule_is_put_where_it_clashes_with_nothing() {
        let design = Design {
            components: vec![methane_like()],
            ..Default::default()
        };
        let built = build(&design, 1).unwrap();
        // Asked to go right on top of the molecule there, it moves along +x until clear.
        let placed = place_new(
            &built,
            &design.components[0],
            0,
            math::IDENTITY,
            [0.545, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        );
        let mut both = design.clone();
        both.manual.push(placed.clone());
        let b = build(&both, 2).unwrap();
        assert!(clash::find(&b, None).pairs.is_empty());
        let x = placed.transform.apply([0.0; 3])[0];
        assert!(x > 1.0 && x < 6.0, "moved only as far as needed: {x}");
        // Into empty space it goes exactly where asked.
        let far = place_new(
            &built,
            &design.components[0],
            0,
            math::IDENTITY,
            [20.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        );
        assert!(close(
            far.transform.apply([0.545, 0.0, 0.0]),
            [20.0, 0.0, 0.0]
        ));
    }
}
