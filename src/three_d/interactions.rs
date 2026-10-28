//! Non-covalent contacts worth showing in a 3D view: hydrogen bonds and π-stacking between
//! aromatic rings. Pure geometry on element symbols, positions and bonds, so the 3D view
//! (one model) and the assembly view (many molecules, maybe in a periodic box) share it.
//!
//! # Hydrogen bonds
//!
//! A donor D ∈ {N, O, F} carrying a hydrogen H, and an acceptor A ∈ {N, O, F} (not D, not
//! bonded to H, not a four-coordinate N), form one when
//!
//! - H···A ≤ [`HBOND_H_ACCEPTOR_MAX`] (2.5 Å),
//! - D···A ≤ [`HBOND_DONOR_ACCEPTOR_MAX`] (3.5 Å), and
//! - the angle D–H···A ≥ [`HBOND_ANGLE_MIN`] (120°).
//!
//! These are the usual geometric criteria (Jeffrey, *An Introduction to Hydrogen Bonding*,
//! 1997; the IUPAC definition, Arunan et al., *Pure Appl. Chem.* 2011, 83, 1637, asks for a
//! D–H···A angle above 110° and prefers 180°). Inside one molecule an H and an A at most three
//! bonds apart are left out: their distance is fixed by the bonds, not a contact.
//!
//! # π-stacking
//!
//! Aromatic rings are perceived first: five- and six-membered chordless rings, planar within
//! [`RING_PLANARITY_MAX`], whose π electrons follow Hückel's 4n + 2 rule — counted from the
//! bond orders, so a Kekulé structure (alternating single/double), OpenBabel's aromatic bond
//! order 5 and heteroaromatics (pyridine, pyrrole, furan, thiophene …) all qualify. Two rings
//! that share no atom and are not bonded to each other (fused or biaryl rings are not a
//! contact) stack, after Marcou & Rognan (*J. Chem. Inf. Model.* 2007, 47, 195) and PLIP
//! (Salentin et al., *Nucleic Acids Res.* 2015, 43, W443), when, with d the distance between
//! the centroids, θ the angle between the ring planes and the offset the smaller of the two
//! lateral displacements of one centroid from the other's normal,
//!
//! - **parallel** (face to face, displaced): d ≤ 4.4 Å, θ ≤ 30°, offset ≤ 2.0 Å;
//! - **T-shaped** (edge to face): d ≤ 5.5 Å, 60° ≤ θ ≤ 90°, offset ≤ 2.0 Å, and an atom of
//!   one ring within 4.5 Å of the other ring's centroid (the edge points at the face).
//!
//! # Speed
//!
//! Both searches use the assembly's cell list ([`Grid`]), so they take time proportional to
//! the number of atoms and work in a periodic box: there a contact may reach a periodic image,
//! whose position the result carries so it can be drawn where it is.

use std::collections::HashMap;

use super::Model3d;
use super::geom::{Vec3, add, angle, cross, distance, dot, norm, scale, sub, unit};
use crate::assembly::Built;
use crate::assembly::clash::{Grid, Periodic};

/// H···A at most this far (Å).
pub const HBOND_H_ACCEPTOR_MAX: f64 = 2.5;
/// D···A at most this far (Å).
pub const HBOND_DONOR_ACCEPTOR_MAX: f64 = 3.5;
/// The angle D–H···A at least this wide (degrees).
pub const HBOND_ANGLE_MIN: f64 = 120.0;
/// Inside a molecule, H and A must be more than this many bonds apart.
pub const HBOND_MIN_BONDS: usize = 3;

/// Parallel stacking: centroid distance (Å) and angle between the planes (degrees).
pub const PARALLEL_DISTANCE_MAX: f64 = 4.4;
pub const PARALLEL_ANGLE_MAX: f64 = 30.0;
/// T-shaped stacking: centroid distance (Å) and the least angle between the planes.
pub const T_DISTANCE_MAX: f64 = 5.5;
pub const T_ANGLE_MIN: f64 = 60.0;
/// T-shaped: the nearest atom of one ring at most this far from the other's centroid (Å).
pub const T_EDGE_MAX: f64 = 4.5;
/// Both kinds: the lateral offset of the centroids at most this (Å).
pub const STACK_OFFSET_MAX: f64 = 2.0;
/// A ring's atoms at most this far from its mean plane (Å).
pub const RING_PLANARITY_MAX: f64 = 0.15;

/// At most this many of each kind are kept (a huge crystal would otherwise list millions).
pub const MAX_INTERACTIONS: usize = 100_000;

/// OpenBabel's aromatic bond order.
const AROMATIC: u8 = 5;

/// What to look for, and where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub hbonds: bool,
    pub pi_stacking: bool,
    /// Also contacts inside one molecule (not only between molecules).
    pub intramolecular: bool,
    /// A periodic box: contacts reach the atoms' periodic images too.
    pub periodic: Option<Periodic>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            hbonds: true,
            pi_stacking: true,
            intramolecular: true,
            periodic: None,
        }
    }
}

/// The atoms to search: element symbols, positions (Å), bonds (`(a, b, order)`) and, for an
/// assembly, the molecule of each atom (`None`: all one structure, contacts inside it being
/// intramolecular unless no path of bonds joins the two atoms).
#[derive(Debug, Clone, Copy)]
pub struct Atoms<'a> {
    pub elements: &'a [&'a str],
    pub positions: &'a [Vec3],
    pub bonds: &'a [(usize, usize, u8)],
    pub molecules: Option<&'a [usize]>,
}

/// A hydrogen bond D–H···A.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HBond {
    pub donor: usize,
    pub hydrogen: usize,
    pub acceptor: usize,
    /// Where the acceptor is: its own position, or that of its periodic image.
    pub acceptor_pos: Vec3,
    /// H···A (Å).
    pub distance: f64,
    /// D···A (Å).
    pub donor_distance: f64,
    /// D–H···A (degrees).
    pub angle: f64,
}

/// An aromatic ring.
#[derive(Debug, Clone, PartialEq)]
pub struct Ring {
    /// Its atoms in order around it.
    pub atoms: Vec<usize>,
    pub centroid: Vec3,
    /// Unit normal of its mean plane.
    pub normal: Vec3,
}

/// How two rings stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackKind {
    /// Face to face (possibly displaced).
    Parallel,
    /// Edge to face.
    TShaped,
}

impl StackKind {
    pub fn label(self) -> &'static str {
        match self {
            StackKind::Parallel => t!("three_d.interactions.stack_parallel"),
            StackKind::TShaped => t!("three_d.interactions.stack_t_shaped"),
        }
    }
}

/// Two stacked rings (indices into [`Interactions::rings`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PiStack {
    pub a: usize,
    pub b: usize,
    pub kind: StackKind,
    /// Ring `b`'s centroid where it stacks: its own, or that of a periodic image.
    pub centroid_b: Vec3,
    /// Centroid to centroid (Å).
    pub distance: f64,
    /// Between the ring planes (degrees, 0–90).
    pub angle: f64,
    /// The smaller lateral displacement of the centroids (Å).
    pub offset: f64,
}

/// What [`find`] found, each list nearest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Interactions {
    pub hbonds: Vec<HBond>,
    pub rings: Vec<Ring>,
    pub stacks: Vec<PiStack>,
    /// More than [`MAX_INTERACTIONS`] of a kind: the lists are only part.
    pub truncated: bool,
}

/// Bonds per atom, with their orders.
struct Topology {
    adjacency: Vec<Vec<(usize, u8)>>,
}

impl Topology {
    fn new(n: usize, bonds: &[(usize, usize, u8)]) -> Self {
        let mut adjacency = vec![Vec::new(); n];
        for &(a, b, order) in bonds {
            if a < n && b < n && a != b && !adjacency[a].iter().any(|&(x, _)| x == b) {
                adjacency[a].push((b, order));
                adjacency[b].push((a, order));
            }
        }
        Self { adjacency }
    }

    fn neighbors(&self, a: usize) -> impl Iterator<Item = usize> + '_ {
        self.adjacency[a].iter().map(|&(b, _)| b)
    }

    fn bonded(&self, a: usize, b: usize) -> bool {
        self.adjacency[a].iter().any(|&(x, _)| x == b)
    }

    fn order(&self, a: usize, b: usize) -> Option<u8> {
        self.adjacency[a]
            .iter()
            .find(|&&(x, _)| x == b)
            .map(|&(_, o)| o)
    }

    /// Whether `b` is at most `k` bonds from `a`.
    fn within_bonds(&self, a: usize, b: usize, k: usize) -> bool {
        let mut frontier = vec![a];
        let mut seen = vec![a];
        for _ in 0..k {
            let mut next = Vec::new();
            for &x in &frontier {
                for y in self.neighbors(x) {
                    if y == b {
                        return true;
                    }
                    if !seen.contains(&y) {
                        seen.push(y);
                        next.push(y);
                    }
                }
            }
            frontier = next;
        }
        false
    }
}

/// N, O and F: the atoms that give and take hydrogen bonds.
fn is_polar(element: &str) -> bool {
    matches!(element, "N" | "O" | "F")
}

/// Hydrogen bonds and π-stacking among `atoms`.
pub fn find(atoms: Atoms<'_>, options: &Options) -> Interactions {
    let n = atoms.elements.len().min(atoms.positions.len());
    let topology = Topology::new(n, atoms.bonds);
    let mut out = Interactions::default();
    if options.hbonds {
        let (hbonds, truncated) = hbonds(&atoms, n, &topology, options);
        out.hbonds = hbonds;
        out.truncated |= truncated;
    }
    if options.pi_stacking {
        out.rings = aromatic_rings(&atoms, n, &topology);
        let (stacks, truncated) = stacks(&atoms, &topology, &out.rings, options);
        out.stacks = stacks;
        out.truncated |= truncated;
    }
    out
}

/// Both kinds of contact in one 3D model, those inside it included.
pub fn of_model(model: &Model3d) -> Interactions {
    let elements: Vec<&str> = model.atoms.iter().map(|a| a.element.as_str()).collect();
    let positions: Vec<Vec3> = model.atoms.iter().map(|a| a.pos).collect();
    let bonds: Vec<(usize, usize, u8)> = model
        .bonds
        .iter()
        .map(|b| (b.begin, b.end, b.order))
        .collect();
    let atoms = Atoms {
        elements: &elements,
        positions: &positions,
        bonds: &bonds,
        molecules: None,
    };
    find(atoms, &Options::default())
}

/// The contacts in a built assembly, in its periodic box when it has one.
pub fn of_built(built: &Built, options: Options) -> Interactions {
    let elements: Vec<&str> = built.atoms.iter().map(|a| a.element.as_str()).collect();
    let positions: Vec<Vec3> = built.atoms.iter().map(|a| a.pos).collect();
    let bonds: Vec<(usize, usize, u8)> = built
        .bonds
        .iter()
        .map(|b| (b.begin, b.end, b.order))
        .collect();
    let atoms = Atoms {
        elements: &elements,
        positions: &positions,
        bonds: &bonds,
        molecules: Some(&built.molecule_of_atom),
    };
    let periodic = built.cell.as_ref().and_then(|c| Periodic::new(c.vectors()));
    find(
        atoms,
        &Options {
            periodic,
            ..options
        },
    )
}

/// Each donor hydrogen and its donor (an H bonded to N, O or F).
fn donors(atoms: &Atoms<'_>, n: usize, topology: &Topology) -> Vec<(usize, usize)> {
    (0..n)
        .filter(|&h| atoms.elements[h] == "H")
        .flat_map(|h| {
            topology
                .neighbors(h)
                .filter(|&d| is_polar(atoms.elements[d]))
                .map(move |d| (h, d))
        })
        .collect()
}

/// Atoms that may accept a hydrogen bond.
fn acceptors(atoms: &Atoms<'_>, n: usize, topology: &Topology) -> Vec<usize> {
    (0..n)
        .filter(|&a| {
            let e = atoms.elements[a];
            is_polar(e) && !(e == "N" && topology.adjacency[a].len() >= 4)
        })
        .collect()
}

/// Whether `a` and `b` are in one molecule (and the same periodic image).
fn same_molecule(atoms: &Atoms<'_>, a: usize, b: usize, same_image: bool) -> bool {
    same_image && atoms.molecules.is_none_or(|m| m.get(a) == m.get(b))
}

/// The hydrogen bond D–H···A with A at `a_pos` (its position or an image's), if there is one.
fn hbond_at(
    atoms: &Atoms<'_>,
    topology: &Topology,
    options: &Options,
    (h, d): (usize, usize),
    a: usize,
    a_pos: Vec3,
) -> Option<HBond> {
    let p = atoms.positions;
    let same_image = distance(a_pos, p[a]) < 1e-6;
    if same_image && (a == d || topology.bonded(a, h)) {
        return None;
    }
    if same_molecule(atoms, h, a, same_image)
        && (!options.intramolecular || topology.within_bonds(h, a, HBOND_MIN_BONDS))
    {
        return None;
    }
    let ha = distance(p[h], a_pos);
    let da = distance(p[d], a_pos);
    let theta = angle(p[d], p[h], a_pos);
    (ha <= HBOND_H_ACCEPTOR_MAX && da <= HBOND_DONOR_ACCEPTOR_MAX && theta >= HBOND_ANGLE_MIN)
        .then_some(HBond {
            donor: d,
            hydrogen: h,
            acceptor: a,
            acceptor_pos: a_pos,
            distance: ha,
            donor_distance: da,
            angle: theta,
        })
}

/// Keep the nearest of each `key`, sort nearest first and cap the list.
fn finish<T: Copy, K: std::hash::Hash + Eq>(
    found: Vec<T>,
    key: impl Fn(&T) -> K,
    dist: impl Fn(&T) -> f64,
) -> (Vec<T>, bool) {
    let mut best: HashMap<K, T> = HashMap::new();
    for x in found {
        let k = key(&x);
        match best.get(&k) {
            Some(y) if dist(y) <= dist(&x) => {}
            _ => {
                best.insert(k, x);
            }
        }
    }
    let mut out: Vec<T> = best.into_values().collect();
    out.sort_by(|x, y| dist(x).total_cmp(&dist(y)));
    let truncated = out.len() > MAX_INTERACTIONS;
    out.truncate(MAX_INTERACTIONS);
    (out, truncated)
}

fn hbonds(
    atoms: &Atoms<'_>,
    n: usize,
    topology: &Topology,
    options: &Options,
) -> (Vec<HBond>, bool) {
    let acceptors = acceptors(atoms, n, topology);
    if acceptors.is_empty() {
        return (Vec::new(), false);
    }
    let positions: Vec<Vec3> = acceptors.iter().map(|&a| atoms.positions[a]).collect();
    let grid = Grid::new(&positions, HBOND_H_ACCEPTOR_MAX, options.periodic.as_ref());
    let mut found = Vec::new();
    for (h, d) in donors(atoms, n, topology) {
        let at = atoms.positions[h];
        grid.for_each_within(at, HBOND_H_ACCEPTOR_MAX, |k, delta| {
            if found.len() > 4 * MAX_INTERACTIONS {
                return;
            }
            let a = acceptors[k];
            if let Some(hb) = hbond_at(atoms, topology, options, (h, d), a, add(at, delta)) {
                found.push(hb);
            }
        });
    }
    finish(found, |b| (b.hydrogen, b.acceptor), |b| b.distance)
}

/// Elements that may sit in an aromatic ring.
fn ring_element(element: &str) -> bool {
    matches!(element, "C" | "N" | "O" | "S" | "P" | "B" | "Se")
}

/// Five- and six-membered chordless rings through atoms that could be aromatic (ring elements
/// with two or three neighbours), each once, in order around the ring.
fn small_rings(atoms: &Atoms<'_>, n: usize, topology: &Topology) -> Vec<Vec<usize>> {
    let eligible: Vec<bool> = (0..n)
        .map(|i| ring_element(atoms.elements[i]) && (2..=3).contains(&topology.adjacency[i].len()))
        .collect();
    let mut rings = Vec::new();
    let mut path = Vec::with_capacity(6);
    for start in 0..n {
        if !eligible[start] {
            continue;
        }
        path.clear();
        path.push(start);
        extend(topology, &eligible, &mut path, &mut rings);
    }
    rings.retain(|ring| {
        let k = ring.len();
        (0..k).all(|i| {
            (i + 2..k).all(|j| (i == 0 && j == k - 1) || !topology.bonded(ring[i], ring[j]))
        })
    });
    rings
}

/// Depth-first search for rings whose smallest atom is `path[0]`.
fn extend(
    topology: &Topology,
    eligible: &[bool],
    path: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
) {
    let start = path[0];
    let last = *path.last().expect("path starts with an atom");
    for next in topology.neighbors(last) {
        if next == start && path.len() >= 5 && path[1] < last {
            out.push(path.clone());
        }
        if next > start && eligible[next] && path.len() < 6 && !path.contains(&next) {
            path.push(next);
            extend(topology, eligible, path, out);
            path.pop();
        }
    }
}

/// The centroid and unit normal of a ring, and how far its atoms stray from the mean plane.
fn plane(positions: &[Vec3], ring: &[usize]) -> Option<(Vec3, Vec3, f64)> {
    let k = ring.len() as f64;
    let c = scale(
        ring.iter().fold([0.0; 3], |acc, &i| add(acc, positions[i])),
        1.0 / k,
    );
    // Newell's method: the summed cross products of consecutive spokes.
    let mut sum = [0.0; 3];
    for (i, &a) in ring.iter().enumerate() {
        let b = ring[(i + 1) % ring.len()];
        sum = add(sum, cross(sub(positions[a], c), sub(positions[b], c)));
    }
    let normal = unit(sum)?;
    let deviation = ring
        .iter()
        .map(|&i| dot(sub(positions[i], c), normal).abs())
        .fold(0.0, f64::max);
    Some((c, normal, deviation))
}

/// The ring's π electrons by Hückel counting, or `None` when an atom breaks the conjugation.
/// `in_ring` marks the atoms of every candidate ring, so a double bond into a fused ring counts
/// (naphthalene's Kekulé structures) while an exocyclic C=O does not (quinones).
fn pi_electrons(
    atoms: &Atoms<'_>,
    topology: &Topology,
    ring: &[usize],
    in_ring: &[bool],
) -> Option<usize> {
    let k = ring.len();
    let ring_bond = |i: usize| topology.order(ring[i], ring[(i + 1) % k]);
    if (0..k).all(|i| ring_bond(i) == Some(AROMATIC)) {
        return Some(6);
    }
    let mut total = 0;
    for (i, &a) in ring.iter().enumerate() {
        let (prev, next) = (ring[(i + k - 1) % k], ring[(i + 1) % k]);
        let inside = [prev, next]
            .iter()
            .any(|&b| matches!(topology.order(a, b), Some(2 | AROMATIC)));
        let outside = topology.adjacency[a]
            .iter()
            .find(|&&(b, o)| b != prev && b != next && matches!(o, 2 | AROMATIC));
        let e = atoms.elements[a];
        total += if inside {
            1
        } else if let Some(&(b, _)) = outside {
            // A double bond out of the ring: part of the π system only into another ring.
            if in_ring[b] && ring_element(atoms.elements[b]) {
                1
            } else {
                0
            }
        } else {
            match e {
                // A lone pair in the π system: pyrrole's NH, furan's O, thiophene's S.
                "N" | "O" | "S" | "Se" | "P" => 2,
                "B" => 0,
                _ => return None,
            }
        };
    }
    Some(total)
}

/// The aromatic rings among `atoms`.
pub fn aromatic_rings_of(atoms: Atoms<'_>) -> Vec<Ring> {
    let n = atoms.elements.len().min(atoms.positions.len());
    aromatic_rings(&atoms, n, &Topology::new(n, atoms.bonds))
}

fn aromatic_rings(atoms: &Atoms<'_>, n: usize, topology: &Topology) -> Vec<Ring> {
    let candidates: Vec<(Vec<usize>, Vec3, Vec3)> = small_rings(atoms, n, topology)
        .into_iter()
        .filter_map(|ring| {
            let (c, normal, deviation) = plane(atoms.positions, &ring)?;
            (deviation <= RING_PLANARITY_MAX).then_some((ring, c, normal))
        })
        .collect();
    let mut in_ring = vec![false; n];
    for (ring, _, _) in &candidates {
        for &a in ring {
            in_ring[a] = true;
        }
    }
    candidates
        .into_iter()
        .filter(|(ring, _, _)| {
            pi_electrons(atoms, topology, ring, &in_ring)
                .is_some_and(|e| e >= 2 && (e - 2) % 4 == 0)
        })
        .map(|(atoms, centroid, normal)| Ring {
            atoms,
            centroid,
            normal,
        })
        .collect()
}

/// How rings `i` and `j` (its centroid at `cj`, maybe an image's) stack, if they do.
fn stack_at(
    atoms: &Atoms<'_>,
    topology: &Topology,
    rings: &[Ring],
    options: &Options,
    i: usize,
    j: usize,
    cj: Vec3,
) -> Option<PiStack> {
    let (a, b) = (&rings[i], &rings[j]);
    let shift = sub(cj, b.centroid);
    let same_image = norm(shift) < 1e-6;
    if same_image {
        if i == j
            || a.atoms
                .iter()
                .any(|&x| b.atoms.iter().any(|&y| x == y || topology.bonded(x, y)))
        {
            return None;
        }
        if !options.intramolecular && same_molecule(atoms, a.atoms[0], b.atoms[0], true) {
            return None;
        }
    }
    let delta = sub(cj, a.centroid);
    let d = norm(delta);
    let theta = dot(a.normal, b.normal)
        .abs()
        .clamp(0.0, 1.0)
        .acos()
        .to_degrees();
    let lateral = |n: Vec3| (d * d - dot(delta, n).powi(2)).max(0.0).sqrt();
    let offset = lateral(a.normal).min(lateral(b.normal));
    if offset > STACK_OFFSET_MAX {
        return None;
    }
    let kind = if d <= PARALLEL_DISTANCE_MAX && theta <= PARALLEL_ANGLE_MAX {
        StackKind::Parallel
    } else if d <= T_DISTANCE_MAX && theta >= T_ANGLE_MIN {
        let p = atoms.positions;
        let edge_b = b
            .atoms
            .iter()
            .map(|&y| distance(add(p[y], shift), a.centroid))
            .fold(f64::INFINITY, f64::min);
        let edge_a = a
            .atoms
            .iter()
            .map(|&x| distance(p[x], cj))
            .fold(f64::INFINITY, f64::min);
        if edge_a.min(edge_b) > T_EDGE_MAX {
            return None;
        }
        StackKind::TShaped
    } else {
        return None;
    };
    Some(PiStack {
        a: i,
        b: j,
        kind,
        centroid_b: cj,
        distance: d,
        angle: theta,
        offset,
    })
}

/// Whether `v` points into the positive half-space by the first non-zero coordinate: of a
/// ring paired with its own image, one direction of the two.
fn positive(v: Vec3) -> bool {
    v.iter().find(|x| x.abs() > 1e-9).is_some_and(|&x| x > 0.0)
}

fn stacks(
    atoms: &Atoms<'_>,
    topology: &Topology,
    rings: &[Ring],
    options: &Options,
) -> (Vec<PiStack>, bool) {
    if rings.len() < 2 && options.periodic.is_none() {
        return (Vec::new(), false);
    }
    let centroids: Vec<Vec3> = rings.iter().map(|r| r.centroid).collect();
    let grid = Grid::new(&centroids, T_DISTANCE_MAX, options.periodic.as_ref());
    let mut found = Vec::new();
    for (i, c) in centroids.iter().enumerate() {
        grid.for_each_within(*c, T_DISTANCE_MAX, |j, delta| {
            // Each pair once: from its lower ring, or one way round for a ring and its image.
            if j < i || (j == i && !positive(delta)) || found.len() > 4 * MAX_INTERACTIONS {
                return;
            }
            if let Some(s) = stack_at(atoms, topology, rings, options, i, j, add(*c, delta)) {
                found.push(s);
            }
        });
    }
    finish(found, |s| (s.a, s.b), |s| s.distance)
}

/// The same as [`find`] by testing every pair (and, in a box spanned by `vectors`, the images
/// up to two cells away; the atoms must lie in the box): the reference the cell list is tested
/// against. Quadratic — for tests only.
#[doc(hidden)]
pub fn brute_force(
    atoms: Atoms<'_>,
    options: &Options,
    vectors: Option<[Vec3; 3]>,
) -> Interactions {
    let n = atoms.elements.len().min(atoms.positions.len());
    let topology = Topology::new(n, atoms.bonds);
    let mut shifts = vec![[0.0; 3]];
    if let Some([a, b, c]) = vectors {
        shifts.clear();
        for x in -2..=2 {
            for y in -2..=2 {
                for z in -2..=2 {
                    let f = |v: Vec3, k: i32| scale(v, f64::from(k));
                    shifts.push(add(add(f(a, x), f(b, y)), f(c, z)));
                }
            }
        }
    }
    let mut found = Vec::new();
    for (h, d) in donors(&atoms, n, &topology) {
        for a in acceptors(&atoms, n, &topology) {
            for s in &shifts {
                let at = add(atoms.positions[a], *s);
                if let Some(hb) = hbond_at(&atoms, &topology, options, (h, d), a, at) {
                    found.push(hb);
                }
            }
        }
    }
    let (hbonds, _) = finish(found, |b| (b.hydrogen, b.acceptor), |b| b.distance);
    let rings = aromatic_rings(&atoms, n, &topology);
    let mut found = Vec::new();
    for i in 0..rings.len() {
        for j in i..rings.len() {
            for s in &shifts {
                if j == i && !positive(*s) {
                    continue;
                }
                let cj = add(rings[j].centroid, *s);
                if let Some(st) = stack_at(&atoms, &topology, &rings, options, i, j, cj) {
                    found.push(st);
                }
            }
        }
    }
    let (stacks, _) = finish(found, |s| (s.a, s.b), |s| s.distance);
    Interactions {
        hbonds,
        rings,
        stacks,
        truncated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An owned system for building test inputs.
    #[derive(Default)]
    struct Sys {
        elements: Vec<&'static str>,
        positions: Vec<Vec3>,
        bonds: Vec<(usize, usize, u8)>,
        molecules: Vec<usize>,
    }

    impl Sys {
        fn atoms(&self) -> Atoms<'_> {
            Atoms {
                elements: &self.elements,
                positions: &self.positions,
                bonds: &self.bonds,
                molecules: Some(&self.molecules),
            }
        }

        /// Append a molecule: its atoms and bonds (indices local to it).
        fn add(&mut self, atoms: &[(&'static str, Vec3)], bonds: &[(usize, usize, u8)]) {
            let base = self.elements.len();
            let m = self.molecules.last().map_or(0, |m| m + 1);
            for &(e, p) in atoms {
                self.elements.push(e);
                self.positions.push(p);
                self.molecules.push(m);
            }
            self.bonds
                .extend(bonds.iter().map(|&(a, b, o)| (a + base, b + base, o)));
        }

        fn find(&self) -> Interactions {
            find(self.atoms(), &Options::default())
        }
    }

    /// Rotate `p` by `deg` about z, then move it by `t`.
    fn place(p: Vec3, deg: f64, t: Vec3) -> Vec3 {
        let (s, c) = deg.to_radians().sin_cos();
        add([c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]], t)
    }

    /// Water: O at the origin, H1 along +x (0.96 Å), H2 at 104.5°.
    fn water(sys: &mut Sys, map: impl Fn(Vec3) -> Vec3) {
        let a = 104.5f64.to_radians();
        sys.add(
            &[
                ("O", map([0.0; 3])),
                ("H", map([0.96, 0.0, 0.0])),
                ("H", map([0.96 * a.cos(), 0.96 * a.sin(), 0.0])),
            ],
            &[(0, 1, 1), (0, 2, 1)],
        );
    }

    /// A linear water dimer: the first water's H1 points at the second's O, 2.9 Å O···O. The
    /// second sits `bend` degrees round the first's O from that line, its hydrogens pointing
    /// away from it, so the D–H···A angle closes as `bend` grows.
    fn water_dimer(bend: f64) -> Sys {
        let mut sys = Sys::default();
        water(&mut sys, |p| p);
        let o2 = place([2.9, 0.0, 0.0], bend, [0.0; 3]);
        // The bisector of the acceptor's hydrogens along the O···O line, outwards.
        water(&mut sys, |p| place(place(p, -52.25, [0.0; 3]), bend, o2));
        sys
    }

    #[test]
    fn a_water_dimer_has_one_hydrogen_bond_and_a_bent_one_none() {
        let found = water_dimer(0.0).find();
        assert_eq!(found.hbonds.len(), 1, "{:?}", found.hbonds);
        let hb = found.hbonds[0];
        assert_eq!((hb.donor, hb.hydrogen, hb.acceptor), (0, 1, 3));
        assert!((hb.distance - 1.94).abs() < 1e-9);
        assert!((hb.donor_distance - 2.9).abs() < 1e-9);
        assert!(hb.angle > 179.0);
        // Swing the acceptor 50° about the donor O: O···O stays 2.9 Å and H···A ≈ 2.40 Å, but
        // the D–H···A angle closes to ≈ 112°.
        let bent = water_dimer(-50.0).find();
        assert!(bent.hbonds.is_empty(), "{:?}", bent.hbonds);
        // Only between molecules when asked.
        let options = Options {
            intramolecular: false,
            ..Options::default()
        };
        assert_eq!(find(water_dimer(0.0).atoms(), &options).hbonds.len(), 1);
    }

    #[test]
    fn a_formic_acid_dimer_has_two_hydrogen_bonds() {
        // HCOOH in the xy plane: C at the origin, C=O up-right, O–H down-right, the H pointing
        // along +x. The partner is the same turned 180° about z and moved so O–H···O=C pair up.
        let monomer: [(&str, Vec3); 5] = [
            ("C", [0.0, 0.0, 0.0]),
            ("O", [0.6, 1.04, 0.0]),   // carbonyl O
            ("O", [0.6, -1.17, 0.0]),  // hydroxyl O
            ("H", [1.57, -1.17, 0.0]), // hydroxyl H
            ("H", [-1.09, 0.0, 0.0]),
        ];
        let bonds = [(0, 1, 2), (0, 2, 1), (2, 3, 1), (0, 4, 1)];
        let mut sys = Sys::default();
        sys.add(&monomer, &bonds);
        // Each O–H points at the other's C=O: H···O ≈ 1.70 Å, O···O ≈ 2.67 Å.
        let shift = [3.87, 0.0, 0.0];
        let turned: Vec<(&str, Vec3)> = monomer
            .iter()
            .map(|&(e, p)| (e, place(p, 180.0, shift)))
            .collect();
        sys.add(&turned, &bonds);
        let found = sys.find();
        assert_eq!(found.hbonds.len(), 2, "{:?}", found.hbonds);
        for hb in &found.hbonds {
            assert_eq!(sys.elements[hb.acceptor], "O");
            assert!(hb.distance < 2.0 && hb.angle > 150.0, "{hb:?}");
        }
        // Inside one monomer, the O–H and C=O are only three bonds apart: no hydrogen bond.
        let mut one = Sys::default();
        one.add(&monomer, &bonds);
        assert!(one.find().hbonds.is_empty());
    }

    /// Benzene in the xy plane about the origin, Kekulé bonds, hydrogens included.
    fn benzene(sys: &mut Sys, map: impl Fn(Vec3) -> Vec3) {
        hexagon(sys, ["C"; 6], true, map);
    }

    /// A six-membered ring of `elements`, C–C 1.39 Å; `kekule` alternates single and double
    /// bonds, otherwise all are aromatic (order 5). Each carbon gets a hydrogen.
    fn hexagon(
        sys: &mut Sys,
        elements: [&'static str; 6],
        kekule: bool,
        map: impl Fn(Vec3) -> Vec3,
    ) {
        let mut atoms = Vec::new();
        let mut bonds = Vec::new();
        for (k, e) in elements.iter().enumerate() {
            let t = (60.0 * k as f64).to_radians();
            atoms.push((*e, map([1.39 * t.cos(), 1.39 * t.sin(), 0.0])));
            let order = if !kekule {
                AROMATIC
            } else if k % 2 == 0 {
                2
            } else {
                1
            };
            bonds.push((k, (k + 1) % 6, order));
        }
        for (k, e) in elements.iter().enumerate() {
            if *e == "C" {
                let t = (60.0 * k as f64).to_radians();
                bonds.push((k, atoms.len(), 1));
                atoms.push(("H", map([2.47 * t.cos(), 2.47 * t.sin(), 0.0])));
            }
        }
        sys.add(&atoms, &bonds);
    }

    /// `p` turned 90° about x (so the ring stands in the xz plane), then moved by `t`.
    fn upright(p: Vec3, t: Vec3) -> Vec3 {
        add([p[0], -p[2], p[1]], t)
    }

    #[test]
    fn benzene_dimers_stack_parallel_and_t_shaped_but_not_far_apart() {
        // Face to face, displaced 1.5 Å sideways, 3.5 Å apart: centroid distance ≈ 3.81 Å.
        let mut sys = Sys::default();
        benzene(&mut sys, |p| p);
        benzene(&mut sys, |p| add(p, [1.5, 0.0, 3.5]));
        let found = sys.find();
        assert_eq!(found.rings.len(), 2);
        assert_eq!(found.stacks.len(), 1, "{:?}", found.stacks);
        let s = found.stacks[0];
        assert_eq!(s.kind, StackKind::Parallel);
        assert!((s.distance - 3.808).abs() < 1e-3, "{s:?}");
        assert!(s.angle < 1e-3 && (s.offset - 1.5).abs() < 1e-6, "{s:?}");

        // Edge to face: the second ring stands upright 5.0 Å above the first's centroid.
        let mut sys = Sys::default();
        benzene(&mut sys, |p| p);
        benzene(&mut sys, |p| upright(p, [0.0, 0.0, 5.0]));
        let found = sys.find();
        assert_eq!(found.stacks.len(), 1, "{:?}", found.stacks);
        assert_eq!(found.stacks[0].kind, StackKind::TShaped);
        assert!((found.stacks[0].angle - 90.0).abs() < 1e-6);

        // 6 Å apart, face to face: nothing.
        let mut sys = Sys::default();
        benzene(&mut sys, |p| p);
        benzene(&mut sys, |p| add(p, [0.0, 0.0, 6.0]));
        let found = sys.find();
        assert_eq!(found.rings.len(), 2);
        assert!(found.stacks.is_empty());
    }

    #[test]
    fn fused_rings_are_aromatic_but_not_stacked_with_each_other() {
        // Naphthalene, Kekulé, the fusion bond single: C0..C5 one ring, C4 C5 shared.
        let r = 1.39;
        let h = r * 3f64.sqrt() / 2.0;
        let atoms: Vec<(&'static str, Vec3)> = vec![
            ("C", [0.0, r, 0.0]),            // 0
            ("C", [h, r / 2.0, 0.0]),        // 1
            ("C", [h, -r / 2.0, 0.0]),       // 2 (fusion)
            ("C", [0.0, -r, 0.0]),           // 3
            ("C", [-h, -r / 2.0, 0.0]),      // 4
            ("C", [-h, r / 2.0, 0.0]),       // 5
            ("C", [2.0 * h, r, 0.0]),        // 6
            ("C", [3.0 * h, r / 2.0, 0.0]),  // 7
            ("C", [3.0 * h, -r / 2.0, 0.0]), // 8
            ("C", [2.0 * h, -r, 0.0]),       // 9
        ];
        // Ring A: 0-1-2-3-4-5, ring B: 1-6-7-8-9-2; fusion bond 1–2.
        let bonds = vec![
            (0, 1, 2),
            (1, 2, 1),
            (2, 3, 2),
            (3, 4, 1),
            (4, 5, 2),
            (5, 0, 1),
            (1, 6, 1),
            (6, 7, 2),
            (7, 8, 1),
            (8, 9, 2),
            (9, 2, 1),
        ];
        let mut sys = Sys::default();
        sys.add(&atoms, &bonds);
        let found = sys.find();
        assert_eq!(found.rings.len(), 2, "{:?}", found.rings);
        assert!(found.stacks.is_empty());

        // Cyclohexa-1,4-dien-3,6-dione (benzoquinone) is not aromatic.
        let mut sys = Sys::default();
        let mut quinone = Vec::new();
        for k in 0..6 {
            let t = (60.0 * k as f64).to_radians();
            quinone.push(("C", [1.4 * t.cos(), 1.4 * t.sin(), 0.0]));
        }
        quinone.push(("O", [2.6, 0.0, 0.0]));
        quinone.push(("O", [-2.6, 0.0, 0.0]));
        sys.add(
            &quinone,
            &[
                (0, 1, 1),
                (1, 2, 2),
                (2, 3, 1),
                (3, 4, 1),
                (4, 5, 2),
                (5, 0, 1),
                (0, 6, 2),
                (3, 7, 2),
            ],
        );
        assert!(sys.find().rings.is_empty());
    }

    #[test]
    fn heteroaromatic_rings_count() {
        // Pyridine (Kekulé and aromatic-flagged).
        for kekule in [true, false] {
            let mut sys = Sys::default();
            hexagon(&mut sys, ["N", "C", "C", "C", "C", "C"], kekule, |p| p);
            assert_eq!(sys.find().rings.len(), 1, "kekule {kekule}");
        }
        // Pyrrole: N–H, two C=C.
        let mut sys = Sys::default();
        let mut atoms = Vec::new();
        for k in 0..5 {
            let t = (72.0 * k as f64).to_radians();
            atoms.push((
                if k == 0 { "N" } else { "C" },
                [1.2 * t.cos(), 1.2 * t.sin(), 0.0],
            ));
        }
        atoms.push(("H", [2.2, 0.0, 0.0]));
        sys.add(
            &atoms,
            &[
                (0, 1, 1),
                (1, 2, 2),
                (2, 3, 1),
                (3, 4, 2),
                (4, 0, 1),
                (0, 5, 1),
            ],
        );
        assert_eq!(sys.find().rings.len(), 1);
        // A puckered (chair) "benzene" is not planar enough.
        let mut chair = Sys::default();
        benzene(&mut chair, |p| p);
        for (i, pos) in chair.positions.iter_mut().take(6).enumerate() {
            pos[2] = if i % 2 == 0 { 0.25 } else { -0.25 };
        }
        assert!(chair.find().rings.is_empty());
    }

    /// A small deterministic generator (no rand dependency).
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        fn point(&mut self, size: f64) -> Vec3 {
            [self.next() * size, self.next() * size, self.next() * size]
        }
    }

    /// Waters and benzenes at random positions and orientations in a `size` box.
    fn random_system(seed: u64, count: usize, size: f64) -> Sys {
        let mut rng = Lcg(seed);
        let mut sys = Sys::default();
        for _ in 0..count {
            let at = rng.point(size);
            // A random rotation from two random angles: about z, then about x.
            let (a, b) = (rng.next() * 360.0, rng.next() * 360.0);
            let turn = move |p: Vec3| {
                let p = place(p, a, [0.0; 3]);
                let (s, c) = b.to_radians().sin_cos();
                add([p[0], c * p[1] - s * p[2], s * p[1] + c * p[2]], at)
            };
            if rng.next() < 0.7 {
                water(&mut sys, turn);
            } else {
                benzene(&mut sys, turn);
            }
        }
        sys
    }

    fn same(grid: &Interactions, brute: &Interactions) {
        let key = |b: &HBond| (b.hydrogen, b.acceptor, (b.distance * 1e6).round() as i64);
        let mut a: Vec<_> = grid.hbonds.iter().map(key).collect();
        let mut b: Vec<_> = brute.hbonds.iter().map(key).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
        let key = |s: &PiStack| (s.a, s.b, (s.distance * 1e6).round() as i64);
        let mut a: Vec<_> = grid.stacks.iter().map(key).collect();
        let mut b: Vec<_> = brute.stacks.iter().map(key).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }

    #[test]
    fn the_cell_list_finds_what_testing_every_pair_finds() {
        let (mut hbonds, mut stacks) = (0, 0);
        for seed in 1..=6 {
            let sys = random_system(seed, 120, 16.0);
            let options = Options::default();
            let grid = find(sys.atoms(), &options);
            let brute = brute_force(sys.atoms(), &options, None);
            same(&grid, &brute);
            hbonds += grid.hbonds.len();
            stacks += grid.stacks.len();
        }
        assert!(hbonds > 0 && stacks > 0, "{hbonds} {stacks}");
    }

    #[test]
    fn periodic_boxes_find_contacts_across_the_faces() {
        let vectors = [[16.0, 0.0, 0.0], [3.0, 15.0, 0.0], [-2.0, 1.5, 14.0]];
        let periodic = Periodic::new(vectors).unwrap();
        let options = Options {
            periodic: Some(periodic),
            ..Options::default()
        };
        let (mut hbonds, mut open) = (0, 0);
        for seed in 20..=24 {
            // Molecules about the origin, then spread over the skewed box.
            let mut sys = random_system(seed, 100, 1.0);
            let mut rng = Lcg(seed);
            let mut k = 0;
            while k < sys.positions.len() {
                let m = sys.molecules[k];
                let f = rng.point(1.0);
                let origin = add(
                    add(scale(vectors[0], f[0]), scale(vectors[1], f[1])),
                    scale(vectors[2], f[2]),
                );
                while k < sys.positions.len() && sys.molecules[k] == m {
                    sys.positions[k] = add(sys.positions[k], origin);
                    k += 1;
                }
            }
            let grid = find(sys.atoms(), &options);
            let brute = brute_force(sys.atoms(), &options, Some(vectors));
            same(&grid, &brute);
            hbonds += grid.hbonds.len();
            open += find(sys.atoms(), &Options::default()).hbonds.len();
        }
        assert!(hbonds > open, "{hbonds} vs {open}");

        // One water dimer split by the box's face: the bond goes to the image.
        let mut sys = water_dimer(0.0);
        let cube = [[10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]];
        for k in 0..3 {
            sys.positions[k] = add(sys.positions[k], [8.5, 5.0, 5.0]);
        }
        for k in 3..6 {
            // 8.5 + 2.9 = 11.4 → 1.4, across the face.
            sys.positions[k] = add(sys.positions[k], [8.5 - 10.0, 5.0, 5.0]);
        }
        assert!(sys.find().hbonds.is_empty());
        let options = Options {
            periodic: Periodic::new(cube),
            ..Options::default()
        };
        let found = find(sys.atoms(), &options);
        assert_eq!(found.hbonds.len(), 1);
        let hb = found.hbonds[0];
        assert!((hb.distance - 1.94).abs() < 1e-9);
        // Drawn towards the image beyond the face, not across the box.
        assert!((hb.acceptor_pos[0] - 11.4).abs() < 1e-9, "{hb:?}");
    }
}
