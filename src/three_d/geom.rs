//! Pure geometry for editing a 3D model: vectors, measurements, the bond graph, and the edits
//! (bond length, bond angle, dihedral, distance solving) that move one side of a bond.
//!
//! Nothing here knows about OpenBabel or the UI: coordinates are plain `[f64; 3]` in ångström
//! and connectivity is a list of atom-index pairs, so every edit can be tested on hand-built
//! structures. The reference design is chem_3d_builder_rs's `geom3d.rs` / `dihedral.rs` /
//! `ik.rs`; the differences are that the graph comes from the model's bond list rather than
//! from OpenBabel, that bond lengths and angles can be set as well as torsions, and that the
//! side that moves is the smaller one unless the caller pins an atom.

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};
use std::fmt;

pub type Vec3 = [f64; 3];

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

/// Unit vector, or `None` for one too short to have a direction or not finite. (`n < 1e-12`
/// alone is false for NaN, which would let a NaN through into the coordinates.)
pub fn unit(a: Vec3) -> Option<Vec3> {
    let n = norm(a);
    if !n.is_finite() || n < 1e-12 {
        return None;
    }
    Some(scale(a, 1.0 / n))
}

/// Any unit vector perpendicular to `a` (which must be non-zero).
fn perpendicular(a: Vec3) -> Vec3 {
    let other = if a[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    unit(cross(a, other)).unwrap_or([0.0, 0.0, 1.0])
}

/// Fold an angle in degrees into `(-180, 180]`.
pub fn wrap_degrees(degrees: f64) -> f64 {
    let x = (degrees + 180.0).rem_euclid(360.0) - 180.0;
    if x == -180.0 { 180.0 } else { x }
}

fn wrap_radians(radians: f64) -> f64 {
    (radians + PI).rem_euclid(TAU) - PI
}

/// The angle a–b–c at `b`, in degrees.
pub fn angle(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let (u, v) = (sub(a, b), sub(c, b));
    let (nu, nv) = (norm(u), norm(v));
    if nu < 1e-12 || nv < 1e-12 {
        return 0.0;
    }
    (dot(u, v) / (nu * nv)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// The dihedral a–b–c–d in degrees, in `(-180, 180]`, with the IUPAC sign (the same one
/// OpenBabel's `torsion` reports): turning the c side by +θ about the b→c axis
/// ([`rotate_about_axis`]) adds θ.
pub fn dihedral(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> f64 {
    let b1 = sub(b, a);
    let b2 = sub(c, b);
    let b3 = sub(d, c);
    let n1 = cross(b1, b2);
    let n2 = cross(b2, b3);
    let Some(axis) = unit(b2) else {
        return 0.0;
    };
    let m1 = cross(n1, axis);
    let x = dot(n1, n2);
    let y = dot(m1, n2);
    if x.abs() < 1e-18 && y.abs() < 1e-18 {
        return 0.0;
    }
    wrap_degrees(-y.atan2(x).to_degrees())
}

/// Rotate the atoms `moving` of `coords` by `radians` about the axis through `origin` along
/// `axis` (a unit vector), right-handed. Rodrigues' formula.
pub fn rotate_about_axis(
    coords: &mut [Vec3],
    moving: &[usize],
    origin: Vec3,
    axis: Vec3,
    radians: f64,
) {
    let (s, c) = radians.sin_cos();
    for &i in moving {
        let v = sub(coords[i], origin);
        let par = dot(v, axis);
        let perp = sub(v, scale(axis, par));
        let tang = cross(axis, perp);
        coords[i] = add(
            origin,
            add(scale(axis, par), add(scale(perp, c), scale(tang, s))),
        );
    }
}

/// Atom connectivity, from a model's bond list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    adjacency: Vec<Vec<usize>>,
}

impl Graph {
    /// The graph of `atoms` atoms joined by `bonds` (pairs naming a missing atom are skipped).
    pub fn new(atoms: usize, bonds: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let mut adjacency = vec![Vec::new(); atoms];
        for (a, b) in bonds {
            if a < atoms && b < atoms && a != b && !adjacency[a].contains(&b) {
                adjacency[a].push(b);
                adjacency[b].push(a);
            }
        }
        Self { adjacency }
    }

    pub fn len(&self) -> usize {
        self.adjacency.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adjacency.is_empty()
    }

    pub fn neighbors(&self, atom: usize) -> &[usize] {
        self.adjacency.get(atom).map_or(&[], Vec::as_slice)
    }

    pub fn bonded(&self, a: usize, b: usize) -> bool {
        self.neighbors(a).contains(&b)
    }

    /// Whether `a` and `b` share a neighbour (a 1–3 pair).
    pub fn one_three(&self, a: usize, b: usize) -> bool {
        self.neighbors(a).iter().any(|&n| self.bonded(n, b))
    }

    /// The atoms on `to`'s side of the `from`–`to` bond: everything reachable from `to` without
    /// crossing that bond, `to` included. `None` when `from` is reachable too — the bond is in a
    /// ring, and moving one "side" would tear the ring apart.
    pub fn side(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        if from >= self.len() || to >= self.len() || from == to {
            return None;
        }
        let mut seen = vec![false; self.len()];
        seen[to] = true;
        let mut queue = VecDeque::from([to]);
        let mut out = vec![to];
        while let Some(cur) = queue.pop_front() {
            for &next in self.neighbors(cur) {
                if cur == to && next == from {
                    continue;
                }
                if next == from {
                    return None;
                }
                if !seen[next] {
                    seen[next] = true;
                    out.push(next);
                    queue.push_back(next);
                }
            }
        }
        Some(out)
    }

    /// Whether the `a`–`b` bond lies in a ring.
    pub fn in_ring(&self, a: usize, b: usize) -> bool {
        self.bonded(a, b) && self.side(a, b).is_none()
    }

    /// Fewest-bonds path from `a` to `b`, both included, or `None` across fragments.
    pub fn shortest_path(&self, a: usize, b: usize) -> Option<Vec<usize>> {
        if a >= self.len() || b >= self.len() {
            return None;
        }
        let mut prev = vec![usize::MAX; self.len()];
        prev[a] = a;
        let mut queue = VecDeque::from([a]);
        while let Some(cur) = queue.pop_front() {
            if cur == b {
                let mut path = vec![b];
                let mut walk = b;
                while walk != a {
                    walk = prev[walk];
                    path.push(walk);
                }
                path.reverse();
                return Some(path);
            }
            for &next in self.neighbors(cur) {
                if prev[next] == usize::MAX {
                    prev[next] = cur;
                    queue.push_back(next);
                }
            }
        }
        None
    }

    /// Whether `atoms` are consecutive along bonds (a chain a–b, a–b–c or a–b–c–d).
    pub fn is_chain(&self, atoms: &[usize]) -> bool {
        atoms.windows(2).all(|w| self.bonded(w[0], w[1]))
    }
}

/// Why an edit could not be made. Shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// The atoms named are not bonded in the way the edit needs.
    NotBonded,
    /// The bond that would have to move is in a ring.
    InRing,
    /// The pinned atom is on both candidate sides' wrong end: nothing can move without it.
    AnchorBlocks,
    /// The two atoms are in different fragments.
    NotConnected,
    /// No single bond between the atoms can turn (rings and multiple bonds only).
    NoRotatableBond,
    /// The target value is out of range (a non-positive length, a NaN).
    BadTarget,
    /// The atoms are on top of each other, so the edit has no direction.
    Degenerate,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EditError::NotBonded => t!("three_d.error.edit_not_bonded"),
            EditError::InRing => t!("three_d.error.edit_in_ring"),
            EditError::AnchorBlocks => t!("three_d.error.edit_anchor_blocks"),
            EditError::NotConnected => t!("three_d.error.edit_not_connected"),
            EditError::NoRotatableBond => t!("three_d.error.edit_no_rotatable_bond"),
            EditError::BadTarget => t!("three_d.error.edit_bad_target"),
            EditError::Degenerate => t!("three_d.error.edit_degenerate"),
        })
    }
}

impl std::error::Error for EditError {}

/// For the `a`–`b` bond, which end moves and which atoms move with it: the smaller side,
/// unless `anchor` sits on it, in which case the other. Returns `(pivot, moving)` where `pivot`
/// is the fixed end and `moving` the atoms on the other end's side (that end included).
pub fn moving_side(
    graph: &Graph,
    a: usize,
    b: usize,
    anchor: Option<usize>,
) -> Result<(usize, Vec<usize>), EditError> {
    moving_side_avoiding(graph, a, b, anchor.as_slice())
}

/// [`moving_side`] keeping every atom of `pinned` still: the smaller side holding none of
/// them; [`EditError::AnchorBlocks`] when both sides hold one.
pub fn moving_side_avoiding(
    graph: &Graph,
    a: usize,
    b: usize,
    pinned: &[usize],
) -> Result<(usize, Vec<usize>), EditError> {
    if !graph.bonded(a, b) {
        return Err(EditError::NotBonded);
    }
    let side_b = graph.side(a, b).ok_or(EditError::InRing)?;
    let side_a = graph.side(b, a).ok_or(EditError::InRing)?;
    let ok = |side: &Vec<usize>| !side.iter().any(|x| pinned.contains(x));
    let mut options = Vec::new();
    if ok(&side_b) {
        options.push((a, side_b));
    }
    if ok(&side_a) {
        options.push((b, side_a));
    }
    options
        .into_iter()
        .min_by_key(|(_, side)| side.len())
        .ok_or(EditError::AnchorBlocks)
}

/// Whether the `a`–`b` bond can be turned: single (`order` 1), not in a ring, and with
/// something on both ends to turn.
pub fn is_rotatable(graph: &Graph, a: usize, b: usize, order: u8) -> Result<(), RotateReject> {
    if !graph.bonded(a, b) {
        return Err(RotateReject::NotABond);
    }
    if order != 1 {
        return Err(RotateReject::Multiple);
    }
    if graph.in_ring(a, b) {
        return Err(RotateReject::Ring);
    }
    if graph.neighbors(a).len() < 2 || graph.neighbors(b).len() < 2 {
        return Err(RotateReject::Terminal);
    }
    Ok(())
}

/// Why a bond cannot be turned about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateReject {
    NotABond,
    Multiple,
    Ring,
    Terminal,
}

impl fmt::Display for RotateReject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RotateReject::NotABond => t!("three_d.error.rotate_not_a_bond"),
            RotateReject::Multiple => t!("three_d.error.rotate_multiple"),
            RotateReject::Ring => t!("three_d.error.rotate_ring"),
            RotateReject::Terminal => t!("three_d.error.rotate_terminal"),
        })
    }
}

/// Turn the `b`–`c` bond by `degrees`. The side that moves is chosen by [`moving_side`]; the
/// sign is that of the dihedral seen from b to c (moving the c side by +θ, or the b side by
/// −θ, both add θ to every x–b–c–y dihedral).
pub fn rotate_bond(
    coords: &[Vec3],
    graph: &Graph,
    b: usize,
    c: usize,
    degrees: f64,
    anchor: Option<usize>,
) -> Result<Vec<Vec3>, EditError> {
    rotate_bond_avoiding(coords, graph, b, c, degrees, anchor.as_slice())
}

/// [`rotate_bond`] keeping every atom of `pinned` still (see [`moving_side_avoiding`]).
pub fn rotate_bond_avoiding(
    coords: &[Vec3],
    graph: &Graph,
    b: usize,
    c: usize,
    degrees: f64,
    pinned: &[usize],
) -> Result<Vec<Vec3>, EditError> {
    if !degrees.is_finite() {
        return Err(EditError::BadTarget);
    }
    let (pivot, moving) = moving_side_avoiding(graph, b, c, pinned)?;
    let axis = unit(sub(coords[c], coords[b])).ok_or(EditError::Degenerate)?;
    let radians = if pivot == b { degrees } else { -degrees }.to_radians();
    let mut out = coords.to_vec();
    rotate_about_axis(&mut out, &moving, coords[b], axis, radians);
    Ok(out)
}

/// Set the dihedral a–b–c–d (a chain) to `degrees` by turning the b–c bond.
pub fn set_dihedral(
    coords: &[Vec3],
    graph: &Graph,
    atoms: [usize; 4],
    degrees: f64,
    anchor: Option<usize>,
) -> Result<Vec<Vec3>, EditError> {
    set_dihedral_avoiding(coords, graph, atoms, degrees, anchor.as_slice())
}

/// [`set_dihedral`] keeping every atom of `pinned` still.
pub fn set_dihedral_avoiding(
    coords: &[Vec3],
    graph: &Graph,
    atoms: [usize; 4],
    degrees: f64,
    pinned: &[usize],
) -> Result<Vec<Vec3>, EditError> {
    if !graph.is_chain(&atoms) {
        return Err(EditError::NotBonded);
    }
    if !degrees.is_finite() {
        return Err(EditError::BadTarget);
    }
    let [a, b, c, d] = atoms;
    let current = dihedral(coords[a], coords[b], coords[c], coords[d]);
    rotate_bond_avoiding(coords, graph, b, c, wrap_degrees(degrees - current), pinned)
}

/// Set the a–b bond length to `length` Å by sliding one side along the bond.
pub fn set_bond_length(
    coords: &[Vec3],
    graph: &Graph,
    a: usize,
    b: usize,
    length: f64,
    anchor: Option<usize>,
) -> Result<Vec<Vec3>, EditError> {
    set_bond_length_avoiding(coords, graph, a, b, length, anchor.as_slice())
}

/// [`set_bond_length`] keeping every atom of `pinned` still.
pub fn set_bond_length_avoiding(
    coords: &[Vec3],
    graph: &Graph,
    a: usize,
    b: usize,
    length: f64,
    pinned: &[usize],
) -> Result<Vec<Vec3>, EditError> {
    if !(length.is_finite() && length > 0.0) {
        return Err(EditError::BadTarget);
    }
    let (pivot, moving) = moving_side_avoiding(graph, a, b, pinned)?;
    let end = if pivot == a { b } else { a };
    let dir = unit(sub(coords[end], coords[pivot])).ok_or(EditError::Degenerate)?;
    let shift = scale(dir, length - distance(coords[a], coords[b]));
    let mut out = coords.to_vec();
    for i in moving {
        out[i] = add(out[i], shift);
    }
    Ok(out)
}

/// Set the angle a–b–c (a chain) to `degrees` by swinging one arm about `b`, in the plane of
/// the three atoms.
pub fn set_angle(
    coords: &[Vec3],
    graph: &Graph,
    atoms: [usize; 3],
    degrees: f64,
    anchor: Option<usize>,
) -> Result<Vec<Vec3>, EditError> {
    set_angle_avoiding(coords, graph, atoms, degrees, anchor.as_slice())
}

/// For the angle a–b–c (a chain), the arm that swings: of the c arm (bond b–c) and the a arm
/// (bond b–a), the smaller one that is not in a ring and holds none of `pinned`. Returns
/// `(end, moving)`: the arm's end atom and the atoms on its side, that end included.
pub fn swinging_arm(
    graph: &Graph,
    atoms: [usize; 3],
    pinned: &[usize],
) -> Result<(usize, Vec<usize>), EditError> {
    if !graph.is_chain(&atoms) {
        return Err(EditError::NotBonded);
    }
    let [a, b, c] = atoms;
    let arm = |end: usize| graph.side(b, end).filter(|s| !s.contains(&b));
    let fits = |side: &Vec<usize>| !side.iter().any(|x| pinned.contains(x));
    let mut options: Vec<(usize, Vec<usize>)> = [c, a]
        .into_iter()
        .filter_map(|end| arm(end).map(|s| (end, s)))
        .collect();
    if options.is_empty() {
        return Err(EditError::InRing);
    }
    options.retain(|(_, s)| fits(s));
    options
        .into_iter()
        .min_by_key(|(_, s)| s.len())
        .ok_or(EditError::AnchorBlocks)
}

/// [`set_angle`] keeping every atom of `pinned` still (see [`swinging_arm`]).
pub fn set_angle_avoiding(
    coords: &[Vec3],
    graph: &Graph,
    atoms: [usize; 3],
    degrees: f64,
    pinned: &[usize],
) -> Result<Vec<Vec3>, EditError> {
    if !graph.is_chain(&atoms) {
        return Err(EditError::NotBonded);
    }
    if !(degrees.is_finite() && (0.0..=180.0).contains(&degrees)) {
        return Err(EditError::BadTarget);
    }
    let [a, b, c] = atoms;
    let (end, moving) = swinging_arm(graph, atoms, pinned)?;
    let other = if end == c { a } else { c };

    let u = sub(coords[other], coords[b]);
    let v = sub(coords[end], coords[b]);
    if unit(u).is_none() || unit(v).is_none() {
        return Err(EditError::Degenerate);
    }
    // The plane's normal; for a straight angle any perpendicular serves.
    let normal = unit(cross(u, v)).unwrap_or_else(|| perpendicular(u));
    let current = angle(coords[other], coords[b], coords[end]);
    // Rotating `end` about `normal` (= u × v) by +δ opens the angle by δ.
    let delta = (degrees - current).to_radians();
    let mut out = coords.to_vec();
    rotate_about_axis(&mut out, &moving, coords[b], normal, delta);
    Ok(out)
}

/// What a distance solve should drive the two atoms to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Goal {
    Distance(f64),
    Farthest,
    Closest,
}

/// The outcome of [`solve_distance`].
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    pub coords: Vec<Vec3>,
    pub start: f64,
    pub achieved: f64,
    /// For [`Goal::Distance`]: the target could not be reached; `achieved` is the closest.
    pub unreachable: bool,
    /// Roughly how near and how far the pair can be brought by these bonds.
    pub reach: (f64, f64),
}

/// One turnable bond: rotating `moving` about `b`→`c`.
struct Dof {
    b: usize,
    c: usize,
    moving: Vec<usize>,
}

impl Dof {
    fn apply(&self, coords: &mut [Vec3], radians: f64) {
        if let Some(axis) = unit(sub(coords[self.c], coords[self.b])) {
            let origin = coords[self.b];
            rotate_about_axis(coords, &self.moving, origin, axis, radians);
        }
    }

    /// How turning this bond changes |a − t|²: `C − 2R cos(φ − α)`, as `(C, R, α)`. `None` when
    /// `t` is on the axis and the turn cannot change the distance.
    fn response(&self, coords: &[Vec3], a: usize, t: usize) -> Option<(f64, f64, f64)> {
        let p = coords[self.b];
        let axis = unit(sub(coords[self.c], p))?;
        let v = sub(coords[t], p);
        let par = dot(v, axis);
        let perp = sub(v, scale(axis, par));
        let w = sub(sub(coords[a], p), scale(axis, par));
        let c = dot(w, w) + dot(perp, perp);
        let p_term = dot(w, perp);
        let q_term = dot(w, cross(axis, perp));
        let r = p_term.hypot(q_term);
        (r > 1e-12).then(|| (c, r, q_term.atan2(p_term)))
    }
}

fn best_turn(goal: Goal, c: f64, r: f64, alpha: f64) -> f64 {
    match goal {
        Goal::Closest => wrap_radians(alpha),
        Goal::Farthest => wrap_radians(alpha + PI),
        Goal::Distance(target) => {
            let cos_theta = (c - target * target) / (2.0 * r);
            if cos_theta > 1.0 {
                wrap_radians(alpha)
            } else if cos_theta < -1.0 {
                wrap_radians(alpha + PI)
            } else {
                // Two solutions; the smaller turn rearranges less.
                let theta = cos_theta.acos();
                let (x, y) = (wrap_radians(alpha + theta), wrap_radians(alpha - theta));
                if x.abs() <= y.abs() { x } else { y }
            }
        }
    }
}

const SOLVE_TOLERANCE: f64 = 1e-4;
const SOLVE_SWEEPS: usize = 200;

/// Bring atoms `a` and `b` to `goal` by turning the rotatable single bonds on the path between
/// them (cyclic coordinate descent, each bond solved in closed form). `orders` gives each
/// bond's order for `bonds`, so multiple bonds are left alone.
pub fn solve_distance(
    coords: &[Vec3],
    graph: &Graph,
    bond_order: impl Fn(usize, usize) -> u8,
    a: usize,
    b: usize,
    goal: Goal,
) -> Result<Solution, EditError> {
    if a == b || a >= coords.len() || b >= coords.len() {
        return Err(EditError::NotConnected);
    }
    if let Goal::Distance(t) = goal
        && !(t.is_finite() && t > 0.0)
    {
        return Err(EditError::BadTarget);
    }
    let path = graph.shortest_path(a, b).ok_or(EditError::NotConnected)?;
    let dofs: Vec<Dof> = path
        .windows(2)
        .filter(|w| is_rotatable(graph, w[0], w[1], bond_order(w[0], w[1])).is_ok())
        .filter_map(|w| {
            let moving = graph.side(w[0], w[1])?;
            moving.contains(&b).then(|| Dof {
                b: w[0],
                c: w[1],
                moving,
            })
        })
        .collect();
    if dofs.is_empty() {
        return Err(EditError::NoRotatableBond);
    }
    let objective = |d: f64| match goal {
        Goal::Distance(t) => (d - t).abs(),
        Goal::Farthest => -d,
        Goal::Closest => d,
    };
    let mut work = coords.to_vec();
    let start = distance(work[a], work[b]);
    let mut best = objective(start);
    for _ in 0..SOLVE_SWEEPS {
        for dof in &dofs {
            if let Some((c, r, alpha)) = dof.response(&work, a, b) {
                let turn = best_turn(goal, c, r, alpha);
                if turn.abs() > 1e-12 {
                    dof.apply(&mut work, turn);
                }
            }
        }
        let d = distance(work[a], work[b]);
        if matches!(goal, Goal::Distance(t) if (d - t).abs() < SOLVE_TOLERANCE) {
            break;
        }
        let score = objective(d);
        let improved = best - score;
        best = score;
        if improved < 1e-9 {
            break;
        }
    }
    let achieved = distance(work[a], work[b]);
    let sweep = |goal: Goal| {
        let mut probe = work.clone();
        for dof in &dofs {
            if let Some((c, r, alpha)) = dof.response(&probe, a, b) {
                dof.apply(&mut probe, best_turn(goal, c, r, alpha));
            }
        }
        distance(probe[a], probe[b])
    };
    let (near, far) = (sweep(Goal::Closest), sweep(Goal::Farthest));
    Ok(Solution {
        unreachable: matches!(goal, Goal::Distance(t) if (achieved - t).abs() >= SOLVE_TOLERANCE),
        reach: (near.min(far).min(achieved), near.max(far).max(achieved)),
        coords: work,
        start,
        achieved,
    })
}

/// Atoms that should be bent or tetrahedral but sit on a straight line: C, N or O with two to
/// four neighbours, all by single bonds, with two neighbours at more than 170°. (Linear
/// centres are real only with a multiple bond — alkynes, nitriles, allenes — or around
/// hypervalent atoms, which are left alone.) A 3D builder leaves these when the drawing had a
/// straight run of atoms.
pub fn straightened_centres(
    coords: &[Vec3],
    elements: &[&str],
    graph: &Graph,
    bond_order: impl Fn(usize, usize) -> u8,
) -> Vec<usize> {
    (0..coords.len().min(elements.len()).min(graph.len()))
        .filter(|&i| matches!(elements[i], "C" | "N" | "O"))
        .filter(|&i| {
            let n = graph.neighbors(i);
            (2..=4).contains(&n.len())
                && n.iter().all(|&j| bond_order(i, j) == 1)
                && n.iter().enumerate().any(|(k, &a)| {
                    n[k + 1..]
                        .iter()
                        .any(|&b| angle(coords[a], coords[i], coords[b]) > 170.0)
                })
        })
        .collect()
}

/// Bondi van der Waals radius in Å (1.8 for elements not listed).
pub fn vdw_radius(element: &str) -> f64 {
    match element {
        "H" => 1.20,
        "He" => 1.40,
        "B" => 1.92,
        "C" => 1.70,
        "N" => 1.55,
        "O" => 1.52,
        "F" => 1.47,
        "Ne" => 1.54,
        "Si" => 2.10,
        "P" => 1.80,
        "S" => 1.80,
        "Cl" => 1.75,
        "Ar" => 1.88,
        "As" => 1.85,
        "Se" => 1.90,
        "Br" => 1.85,
        "Kr" => 2.02,
        "I" => 1.98,
        "Xe" => 2.16,
        "Li" => 1.82,
        "Na" => 2.27,
        "K" => 2.75,
        "Mg" => 1.73,
        _ => 1.80,
    }
}

/// Non-bonded pairs closer than this fraction of their van der Waals contact count as a clash.
pub const CLASH_FACTOR: f64 = 0.75;
/// The same for a hydrogen bond (a hydrogen on N/O/F next to an N/O/F acceptor): a strong one
/// sits at about 1.6–1.8 Å, ≈0.62 of the H···O contact, which is no clash. Under 0.55
/// (≈1.5 Å) it is.
pub const HBOND_CLASH_FACTOR: f64 = 0.55;

/// N, O and F: the atoms that give and take hydrogen bonds.
fn is_polar(element: &str) -> bool {
    matches!(element, "N" | "O" | "F")
}

/// What decides whether an atom clashes with another: its van der Waals radius and its part in
/// a hydrogen bond. Shared by [`clashes`] here and the assembly builder's cell-list search
/// (`assembly::clash`), so both judge a pair the same way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClashKind {
    pub radius: f64,
    /// N, O or F: may accept a hydrogen bond.
    pub acceptor: bool,
    /// A hydrogen on N, O or F: may give one.
    pub donor_h: bool,
}

impl ClashKind {
    /// The kind of an atom of `element` bonded to atoms of `neighbors`.
    pub fn of<'a>(element: &str, mut neighbors: impl Iterator<Item = &'a str>) -> Self {
        Self {
            radius: vdw_radius(element),
            acceptor: is_polar(element),
            donor_h: element == "H" && neighbors.any(is_polar),
        }
    }
}

/// The distance under which two non-bonded atoms clash: [`CLASH_FACTOR`] of their van der
/// Waals contact, or [`HBOND_CLASH_FACTOR`] of it for a hydrogen bond (a hydrogen on N/O/F
/// next to an N/O/F acceptor). Never more than `CLASH_FACTOR · (rₐ + r_b)`.
pub fn clash_limit(a: ClashKind, b: ClashKind) -> f64 {
    let hbond = (a.donor_h && b.acceptor) || (b.donor_h && a.acceptor);
    let factor = if hbond {
        HBOND_CLASH_FACTOR
    } else {
        CLASH_FACTOR
    };
    factor * (a.radius + b.radius)
}

/// A pair of atoms closer than [`CLASH_FACTOR`] of their van der Waals contact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clash {
    pub a: usize,
    pub b: usize,
    pub distance: f64,
}

/// Overlapping non-bonded pairs, worst (relative to the contact distance) first. 1–2 and 1–3
/// pairs are fixed by bond lengths and angles and are left out; 1–4 pairs are kept, since a
/// torsion is exactly what drives them together.
pub fn clashes(coords: &[Vec3], elements: &[&str], graph: &Graph) -> Vec<Clash> {
    let n = coords.len().min(elements.len());
    let kinds: Vec<ClashKind> = (0..n)
        .map(|i| {
            ClashKind::of(
                elements[i],
                graph
                    .neighbors(i)
                    .iter()
                    .filter_map(|&j| elements.get(j).copied()),
            )
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let limit = clash_limit(kinds[i], kinds[j]);
            let d = distance(coords[i], coords[j]);
            if d < limit && !graph.bonded(i, j) && !graph.one_three(i, j) {
                out.push((
                    d / limit,
                    Clash {
                        a: i,
                        b: j,
                        distance: d,
                    },
                ));
            }
        }
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    fn same_point(a: Vec3, b: Vec3) -> bool {
        distance(a, b) < 1e-9
    }

    /// n-butane's carbon chain plus one H on each end carbon, roughly anti:
    /// H4–C0–C1–C2–C3–H5.
    fn butane() -> (Vec<Vec3>, Graph) {
        let coords = vec![
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [2.0, 1.4, 0.0],
            [3.5, 1.4, 0.3],
            [-0.5, -0.9, 0.3],
            [4.0, 2.3, 0.0],
        ];
        let graph = Graph::new(6, [(0, 1), (1, 2), (2, 3), (0, 4), (3, 5)]);
        (coords, graph)
    }

    #[test]
    fn a_direction_that_is_not_a_number_has_no_unit_vector() {
        assert_eq!(unit([f64::NAN, 0.0, 0.0]), None);
        assert_eq!(unit([0.0, f64::INFINITY, 0.0]), None);
        assert_eq!(unit([0.0, 0.0, 0.0]), None);
        assert!(unit([3.0, 0.0, 4.0]).is_some());
    }

    #[test]
    fn a_quarter_turn_about_z_sends_x_to_y() {
        let mut coords = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        rotate_about_axis(&mut coords, &[1], [0.0; 3], [0.0, 0.0, 1.0], PI / 2.0);
        assert!(same_point(coords[1], [0.0, 1.0, 0.0]), "{:?}", coords[1]);
        assert_eq!(coords[0], [0.0; 3]);
    }

    #[test]
    fn measurements_of_known_shapes() {
        assert!(close(distance([0.0; 3], [3.0, 4.0, 0.0]), 5.0, 1e-12));
        assert!(close(
            angle([1.0, 0.0, 0.0], [0.0; 3], [0.0, 1.0, 0.0]),
            90.0,
            1e-9
        ));
        assert!(close(
            angle([1.0, 0.0, 0.0], [0.0; 3], [-1.0, 0.0, 0.0]),
            180.0,
            1e-9
        ));
        // Planar cis and trans.
        let (a, b, c) = ([0.0, 1.0, 0.0], [0.0; 3], [1.0, 0.0, 0.0]);
        assert!(close(dihedral(a, b, c, [1.0, 1.0, 0.0]), 0.0, 1e-9));
        assert!(close(
            dihedral(a, b, c, [1.0, -1.0, 0.0]).abs(),
            180.0,
            1e-9
        ));
    }

    /// IUPAC: looking along b→c, the front bond (to a) turns clockwise by θ to eclipse the back
    /// bond (to d) for a dihedral of +θ. With b→c along +x, a at +y and d at +z (seen from −x
    /// looking towards +x, +y is up and +z is to the left), a → d is a clockwise quarter turn,
    /// so the dihedral is +90°.
    #[test]
    fn the_dihedral_sign_is_iupac() {
        let d = dihedral([0.0, 1.0, 0.0], [0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]);
        assert!(close(d, 90.0, 1e-9), "{d}");
    }

    #[test]
    fn turning_the_c_side_adds_to_the_dihedral() {
        let (coords, graph) = butane();
        let before = dihedral(coords[0], coords[1], coords[2], coords[3]);
        for degrees in [30.0, -75.0, 170.0] {
            let after = rotate_bond(&coords, &graph, 1, 2, degrees, None).unwrap();
            let d = dihedral(after[0], after[1], after[2], after[3]);
            assert!(
                close(wrap_degrees(d - before), degrees, 1e-9),
                "{degrees}: {before} -> {d}"
            );
            // Bond lengths are untouched.
            for (x, y) in [(0, 1), (1, 2), (2, 3), (0, 4), (3, 5)] {
                assert!(close(
                    distance(after[x], after[y]),
                    distance(coords[x], coords[y]),
                    1e-9
                ));
            }
        }
    }

    #[test]
    fn the_smaller_side_moves_unless_the_anchor_is_on_it() {
        let (coords, graph) = butane();
        // C2–C3: the C3 side (C3, H5) is smaller than the C2 side (four atoms).
        let (pivot, moving) = moving_side(&graph, 2, 3, None).unwrap();
        assert_eq!(pivot, 2);
        assert_eq!(moving.len(), 2);
        // Pinning H5 makes the other side move instead.
        let (pivot, moving) = moving_side(&graph, 2, 3, Some(5)).unwrap();
        assert_eq!(pivot, 3);
        assert!(moving.contains(&0) && !moving.contains(&5));

        // Either way, the dihedral lands on the target.
        for anchor in [None, Some(5), Some(4)] {
            let out = set_dihedral(&coords, &graph, [0, 1, 2, 3], 60.0, anchor).unwrap();
            let d = dihedral(out[0], out[1], out[2], out[3]);
            assert!(close(d, 60.0, 1e-9), "{anchor:?}: {d}");
            if let Some(x) = anchor {
                assert!(same_point(out[x], coords[x]), "the anchor moved");
            }
        }
    }

    /// Several pinned atoms: the side holding none moves, even the larger one; with pinned
    /// atoms on both sides nothing can.
    #[test]
    fn the_side_without_pinned_atoms_moves() {
        let (coords, graph) = butane();
        // C2–C3 with C3's hydrogen (5) and C0's (4) pinned: no side is free.
        assert_eq!(
            moving_side_avoiding(&graph, 2, 3, &[5, 4]),
            Err(EditError::AnchorBlocks)
        );
        assert_eq!(
            set_dihedral_avoiding(&coords, &graph, [0, 1, 2, 3], 60.0, &[5, 4]),
            Err(EditError::AnchorBlocks)
        );
        // Pinning only atoms of the smaller side moves the larger one.
        let out = set_bond_length_avoiding(&coords, &graph, 2, 3, 1.7, &[3, 5]).unwrap();
        assert!(close(distance(out[2], out[3]), 1.7, 1e-9));
        assert!(same_point(out[3], coords[3]) && same_point(out[5], coords[5]));
        let out = set_dihedral_avoiding(&coords, &graph, [0, 1, 2, 3], -75.0, &[3, 5]).unwrap();
        assert!(close(dihedral(out[0], out[1], out[2], out[3]), -75.0, 1e-9));
        assert!(same_point(out[3], coords[3]) && same_point(out[5], coords[5]));
        // An angle swings the arm holding no pinned atom; both arms pinned blocks it.
        let out = set_angle_avoiding(&coords, &graph, [1, 2, 3], 100.0, &[3]).unwrap();
        assert!(close(angle(out[1], out[2], out[3]), 100.0, 1e-6));
        assert!(same_point(out[3], coords[3]) && same_point(out[2], coords[2]));
        assert_eq!(
            swinging_arm(&graph, [1, 2, 3], &[0, 5]),
            Err(EditError::AnchorBlocks)
        );
        // No pins: the same as no anchor.
        assert_eq!(
            moving_side_avoiding(&graph, 2, 3, &[]),
            moving_side(&graph, 2, 3, None)
        );
    }

    #[test]
    fn ring_bonds_cannot_move() {
        // Cyclopropane with one substituent.
        let graph = Graph::new(4, [(0, 1), (1, 2), (2, 0), (0, 3)]);
        assert!(graph.in_ring(0, 1));
        assert!(!graph.in_ring(0, 3));
        assert_eq!(is_rotatable(&graph, 0, 1, 1), Err(RotateReject::Ring));
        let coords = vec![
            [0.0; 3],
            [1.5, 0.0, 0.0],
            [0.75, 1.3, 0.0],
            [-1.0, -1.0, 0.0],
        ];
        assert_eq!(
            set_bond_length(&coords, &graph, 0, 1, 2.0, None),
            Err(EditError::InRing)
        );
        // A substituent's bond length can change: the substituent moves.
        let out = set_bond_length(&coords, &graph, 0, 3, 2.0, None).unwrap();
        assert!(close(distance(out[0], out[3]), 2.0, 1e-9));
        assert!(same_point(out[1], coords[1]));
    }

    #[test]
    fn bond_lengths_and_angles_are_set() {
        let (coords, graph) = butane();
        let out = set_bond_length(&coords, &graph, 1, 2, 1.40, None).unwrap();
        assert!(close(distance(out[1], out[2]), 1.40, 1e-9));
        // Other bonds keep their lengths.
        assert!(close(
            distance(out[0], out[1]),
            distance(coords[0], coords[1]),
            1e-9
        ));
        assert!(close(
            distance(out[2], out[3]),
            distance(coords[2], coords[3]),
            1e-9
        ));

        for target in [90.0, 109.5, 150.0, 180.0] {
            let out = set_angle(&coords, &graph, [0, 1, 2], target, None).unwrap();
            let a = angle(out[0], out[1], out[2]);
            assert!(close(a, target, 1e-4), "{target}: {a}");
            assert!(close(
                distance(out[1], out[2]),
                distance(coords[1], coords[2]),
                1e-9
            ));
        }
        assert_eq!(
            set_angle(&coords, &graph, [0, 2, 3], 100.0, None),
            Err(EditError::NotBonded)
        );
        assert_eq!(
            set_bond_length(&coords, &graph, 0, 1, -1.0, None),
            Err(EditError::BadTarget)
        );
        // Pinning an atom on one arm swings the other.
        let blocked = Graph::new(3, [(0, 1), (1, 2)]);
        let three = vec![[1.0, 0.0, 0.0], [0.0; 3], [0.0, 1.0, 0.0]];
        let out = set_angle(&three, &blocked, [0, 1, 2], 120.0, Some(0)).unwrap();
        assert!(same_point(out[0], three[0]) && same_point(out[1], three[1]));
    }

    #[test]
    fn a_straight_angle_can_be_bent() {
        let graph = Graph::new(3, [(0, 1), (1, 2)]);
        let coords = vec![[-1.0, 0.0, 0.0], [0.0; 3], [1.0, 0.0, 0.0]];
        let out = set_angle(&coords, &graph, [0, 1, 2], 120.0, None).unwrap();
        assert!(close(angle(out[0], out[1], out[2]), 120.0, 1e-6));
    }

    #[test]
    fn the_distance_solver_reaches_a_target_or_says_how_far_it_can() {
        let (coords, graph) = butane();
        let single = |_: usize, _: usize| 1u8;
        let start = distance(coords[4], coords[5]);
        let s = solve_distance(&coords, &graph, single, 4, 5, Goal::Distance(start - 1.0)).unwrap();
        assert!(!s.unreachable);
        assert!(close(s.achieved, start - 1.0, 1e-3), "{}", s.achieved);
        assert!(same_point(s.coords[4], coords[4]), "atom a stays put");

        let s = solve_distance(&coords, &graph, single, 4, 5, Goal::Distance(50.0)).unwrap();
        assert!(s.unreachable);
        assert!(s.reach.1 < 50.0);

        let far = solve_distance(&coords, &graph, single, 4, 5, Goal::Farthest).unwrap();
        let near = solve_distance(&coords, &graph, single, 4, 5, Goal::Closest).unwrap();
        assert!(far.achieved > near.achieved);

        // With every bond double nothing can turn.
        let double = |_: usize, _: usize| 2u8;
        assert_eq!(
            solve_distance(&coords, &graph, double, 4, 5, Goal::Farthest),
            Err(EditError::NoRotatableBond)
        );
        let apart = Graph::new(6, [(0, 1)]);
        assert_eq!(
            solve_distance(&coords, &apart, single, 0, 5, Goal::Farthest),
            Err(EditError::NotConnected)
        );
    }

    #[test]
    fn clashes_skip_bonded_and_geminal_pairs() {
        // H–C–H with the two hydrogens 1.0 Å apart (1–3: ignored) and a third, unbonded H
        // 1.4 Å from one of them (clash).
        let coords = vec![[0.0; 3], [0.5, 0.9, 0.0], [-0.5, 0.9, 0.0], [1.5, 1.9, 0.0]];
        let graph = Graph::new(4, [(0, 1), (0, 2)]);
        let found = clashes(&coords, &["C", "H", "H", "H"], &graph);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].a, found[0].b), (1, 3));
    }

    #[test]
    fn a_hydrogen_bond_has_its_own_threshold() {
        let water = ClashKind::of("O", std::iter::empty());
        let hydroxyl_h = ClashKind::of("H", ["O"].into_iter());
        let methyl_h = ClashKind::of("H", ["C"].into_iter());
        assert!(hydroxyl_h.donor_h && !methyl_h.donor_h && water.acceptor);
        // 1.7 Å: a hydrogen bond, no clash; the same distance from a C–H is one.
        assert!(1.7 > clash_limit(hydroxyl_h, water));
        assert!(1.7 < clash_limit(methyl_h, water));
        assert_eq!(
            clash_limit(water, hydroxyl_h),
            clash_limit(hydroxyl_h, water)
        );
    }

    #[test]
    fn straight_single_bonded_centres_are_found() {
        // C0–C1–C2 straight, then bent at C2 towards C3.
        let coords = vec![
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [3.5, 1.4, 0.0],
        ];
        let graph = Graph::new(4, [(0, 1), (1, 2), (2, 3)]);
        let elements = ["C", "C", "C", "C"];
        assert_eq!(
            straightened_centres(&coords, &elements, &graph, |_, _| 1),
            vec![1]
        );
        // The same line through a triple bond is an alkyne, not a fault.
        let alkyne = |a: usize, b: usize| if (a.min(b), a.max(b)) == (1, 2) { 3 } else { 1 };
        assert!(straightened_centres(&coords, &elements, &graph, alkyne).is_empty());
        // Only C, N and O are judged.
        assert!(straightened_centres(&coords, &["C", "S", "C", "C"], &graph, |_, _| 1).is_empty());
    }

    #[test]
    fn wrapping_keeps_angles_in_range() {
        assert!(close(wrap_degrees(190.0), -170.0, 1e-12));
        assert!(close(wrap_degrees(-180.0), 180.0, 1e-12));
        assert!(close(wrap_degrees(540.0), 180.0, 1e-12));
    }
}
