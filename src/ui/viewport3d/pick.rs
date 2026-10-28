//! Ray picking against the atoms as they are drawn.
//!
//! moleucle_3dview_rs's `MoleculeViewer::pick` tests every atom as a ball-and-stick ball
//! (0.3 × the van der Waals radius) whatever radii `set_atom_radii` draws it at, so in the
//! spheres style a click on the visible edge of an atom missed it. This picks against the
//! per-atom radii the viewport gives the viewer for its style. The library's spatial grid is
//! private, so the atoms go into a grid of their own here, built once per viewer revision
//! (which `set_atom_radii` bumps) and walked along the ray, so a pick costs the cells the ray
//! crosses rather than a test per atom. Bonds, periodic images and hidden atoms are handled as
//! the library handles them.

use std::collections::HashMap;

use lin_alg::f32::Vec3;
use moleucle_3dview_rs::{MoleculeViewer, ball_stick_radius, default_ball_stick_bond_radius};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Picked {
    Atom(usize),
    Bond(usize),
}

/// The most cells the grid spans along any axis, so a ray walks at most about three times
/// this many cells whatever the spread of the atoms.
const MAX_CELLS_PER_AXIS: i32 = 128;

/// The atoms bucketed into cubic cells, each atom in every cell its sphere's bounding box
/// overlaps, so the cell holding any point of a sphere lists that atom.
pub(super) struct AtomGrid {
    /// The viewer revision the grid was built from.
    pub(super) revision: u64,
    radii: Vec<f32>,
    /// Corners of the box around every sphere and bond.
    min: [f32; 3],
    max: [f32; 3],
    cell: f32,
    dims: [i32; 3],
    cells: HashMap<[i32; 3], Vec<u32>>,
}

impl AtomGrid {
    pub(super) fn build(viewer: &MoleculeViewer) -> Self {
        let atoms = viewer.molecule().map_or(&[][..], |m| m.atoms.as_slice());
        let radii: Vec<f32> = atoms
            .iter()
            .enumerate()
            .map(|(i, atom)| {
                viewer
                    .atom_radii
                    .as_ref()
                    .and_then(|radii| radii.get(i).copied())
                    .unwrap_or_else(|| ball_stick_radius(&atom.element, false))
            })
            .collect();
        let bond_radius = default_ball_stick_bond_radius();
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for (atom, &r) in atoms.iter().zip(&radii) {
            let p = arr(atom.position);
            let pad = r.max(bond_radius);
            for k in 0..3 {
                min[k] = min[k].min(p[k] - pad);
                max[k] = max[k].max(p[k] + pad);
            }
        }
        if atoms.is_empty() {
            (min, max) = ([0.0; 3], [0.0; 3]);
        }
        // A cell at least as wide as the largest sphere, so each atom lands in at most eight,
        // and wide enough that no axis spans more than MAX_CELLS_PER_AXIS of them, so atoms far
        // apart do not leave a ray walking cell by cell across the empty space between them.
        let extent = (0..3).fold(0.0_f32, |e, k| e.max(max[k] - min[k]));
        let cell = (radii.iter().fold(0.0_f32, |m, &r| m.max(r)) * 2.0)
            .max(extent / MAX_CELLS_PER_AXIS as f32 * 1.001)
            .max(1e-3);
        let dims = [0, 1, 2]
            .map(|k| (((max[k] - min[k]) / cell).ceil() as i32).clamp(1, MAX_CELLS_PER_AXIS));
        let mut grid = Self {
            revision: viewer.revision(),
            radii,
            min,
            max,
            cell,
            dims,
            cells: HashMap::new(),
        };
        for (i, atom) in atoms.iter().enumerate() {
            let p = arr(atom.position);
            let r = grid.radii[i];
            let lo = grid.cell_of([p[0] - r, p[1] - r, p[2] - r]);
            let hi = grid.cell_of([p[0] + r, p[1] + r, p[2] + r]);
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        grid.cells.entry([x, y, z]).or_default().push(i as u32);
                    }
                }
            }
        }
        grid
    }

    fn cell_of(&self, p: [f32; 3]) -> [i32; 3] {
        [0, 1, 2]
            .map(|k| (((p[k] - self.min[k]) / self.cell).floor() as i32).clamp(0, self.dims[k] - 1))
    }

    /// Walk the cells the ray crosses, nearest first, handing `visit` each occupied cell's
    /// atoms and the distance at which the ray enters it, until `visit` returns false. The
    /// number of cells walked through, or `None` when the ray misses the grid's box altogether.
    fn along_ray(
        &self,
        origin: Vec3,
        dir: Vec3,
        mut visit: impl FnMut(&[u32], f32) -> bool,
    ) -> Option<usize> {
        let (o, d) = (arr(origin), arr(dir));
        let (mut enter, mut exit) = (0.0_f32, f32::MAX);
        for k in 0..3 {
            if d[k] == 0.0 {
                if o[k] < self.min[k] || o[k] > self.max[k] {
                    return None;
                }
                continue;
            }
            let a = (self.min[k] - o[k]) / d[k];
            let b = (self.max[k] - o[k]) / d[k];
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
        }
        if enter > exit {
            return None;
        }

        let start = [0, 1, 2].map(|k| o[k] + d[k] * enter);
        let mut cell = self.cell_of(start);
        let mut step = [0; 3];
        let mut next = [f32::MAX; 3];
        let mut delta = [f32::MAX; 3];
        for k in 0..3 {
            if d[k] > 0.0 {
                step[k] = 1;
                let wall = self.min[k] + (cell[k] + 1) as f32 * self.cell;
                next[k] = enter + (wall - start[k]) / d[k];
                delta[k] = self.cell / d[k];
            } else if d[k] < 0.0 {
                step[k] = -1;
                let wall = self.min[k] + cell[k] as f32 * self.cell;
                next[k] = enter + (wall - start[k]) / d[k];
                delta[k] = -self.cell / d[k];
            }
        }
        let mut t = enter;
        let mut walked = 0;
        loop {
            walked += 1;
            if let Some(atoms) = self.cells.get(&cell)
                && !visit(atoms, t)
            {
                return Some(walked);
            }
            let k = (0..3).fold(0, |m, k| if next[k] < next[m] { k } else { m });
            if next[k] > exit {
                return Some(walked);
            }
            t = next[k];
            cell[k] += step[k];
            if !(0..self.dims[k]).contains(&cell[k]) {
                return Some(walked);
            }
            next[k] += delta[k];
        }
    }
}

/// The nearest atom or bond along the ray, in the molecule or any of its periodic images
/// (reported by the primary atom's or bond's index). `dir` is normalised.
pub(super) fn pick(
    viewer: &MoleculeViewer,
    grid: &AtomGrid,
    origin: Vec3,
    dir: Vec3,
) -> Option<Picked> {
    let mut closest = f32::MAX;
    let mut picked = None;
    // Testing the ray against a translated image is testing the translated ray against the
    // molecule, so every image shares the grid and the hit distances stay comparable.
    match viewer.periodic_images() {
        Some(images) => {
            for translation in images.translations() {
                pick_in_image(
                    viewer,
                    grid,
                    origin - translation,
                    dir,
                    &mut closest,
                    &mut picked,
                );
            }
        }
        None => pick_in_image(viewer, grid, origin, dir, &mut closest, &mut picked),
    }
    picked
}

fn pick_in_image(
    viewer: &MoleculeViewer,
    grid: &AtomGrid,
    origin: Vec3,
    dir: Vec3,
    closest: &mut f32,
    picked: &mut Option<Picked>,
) {
    let Some(molecule) = viewer.molecule() else {
        return;
    };
    let reached = grid.along_ray(origin, dir, |candidates, entry| {
        // Cells come nearest first: nothing from this one on can beat a hit already found.
        if *closest < entry {
            return false;
        }
        for &candidate in candidates {
            let i = candidate as usize;
            // Hidden atoms are not drawn, so they are not clickable either.
            if !viewer.is_atom_visible(i) {
                continue;
            }
            let (Some(atom), Some(&radius)) = (molecule.atoms.get(i), grid.radii.get(i)) else {
                continue;
            };
            if let Some(t) = ray_sphere(origin, dir, atom.position, radius)
                && t > 0.0
                && t < *closest
            {
                *closest = t;
                *picked = Some(Picked::Atom(i));
            }
        }
        true
    });
    if reached.is_none() {
        return;
    }

    // Bonds at the radius the ball-and-stick sticks are drawn at, by a linear scan as in the
    // library: bonds come from files, which keeps them far fewer than the atoms of an MD box.
    let radius = default_ball_stick_bond_radius();
    for (i, bond) in molecule.bonds.iter().enumerate() {
        let (a, b) = bond.endpoints();
        if !viewer.is_atom_visible(a) || !viewer.is_atom_visible(b) {
            continue;
        }
        let Some((p1, p2)) = molecule.bond_endpoints(bond) else {
            continue;
        };
        let enclosing = (p2 - p1).magnitude() * 0.5 + radius;
        if !ray_passes_within(origin, dir, (p1 + p2) * 0.5, enclosing) {
            continue;
        }
        if let Some(t) = ray_cylinder(origin, dir, p1, p2, radius)
            && t > 0.0
            && t < *closest
        {
            *closest = t;
            *picked = Some(Picked::Bond(i));
        }
    }
}

fn arr(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

/// Whether the ray passes within `radius` of `center`, not behind it: a cheap rejection.
fn ray_passes_within(origin: Vec3, dir: Vec3, center: Vec3, radius: f32) -> bool {
    let to_center = center - origin;
    let along = to_center.dot(dir);
    if along < -radius {
        return false;
    }
    to_center.magnitude_squared() - along * along <= radius * radius
}

/// Distance along the ray to the sphere's near surface, `None` when the ray misses it or its
/// centre lies behind the origin.
fn ray_sphere(origin: Vec3, dir: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let l = center - origin;
    let along = l.dot(dir);
    if along < 0.0 {
        return None;
    }
    let off_sq = l.dot(l) - along * along;
    let r_sq = radius * radius;
    (off_sq <= r_sq).then(|| along - (r_sq - off_sq).sqrt())
}

/// Distance along the ray to the side of the cylinder from `p1` to `p2` (caps ignored).
fn ray_cylinder(origin: Vec3, dir: Vec3, p1: Vec3, p2: Vec3, radius: f32) -> Option<f32> {
    let ba = p2 - p1;
    let oa = origin - p1;
    let baba = ba.dot(ba);
    let bard = ba.dot(dir);
    let baoa = ba.dot(oa);
    let a = baba - bard * bard;
    let b = baba * oa.dot(dir) - baoa * bard;
    let c = baba * oa.dot(oa) - baoa * baoa - radius * radius * baba;
    let h = b * b - a * c;
    if h < 0.0 {
        return None;
    }
    let t = (-b - h.sqrt()) / a;
    let y = baoa + t * bard;
    (y > 0.0 && y < baba).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moleucle_3dview_rs::{
        Atom, Bond, Element, Molecule, PeriodicImages, SimulationCellState, viewer::ViewerEvent,
    };

    /// A deterministic jumble of atoms (a cluster plus a stray far away, so the grid has
    /// empty stretches) with a chain of bonds.
    fn jumble() -> MoleculeViewer {
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let elements = ["C", "H", "O", "N", "S", "I"];
        let mut atoms: Vec<Atom> = (0..60)
            .map(|i| {
                let p = Vec3::new(next() * 1.5, next() * 1.5, next() * 1.5);
                Atom::new(p, Element::new(elements[i % elements.len()]))
            })
            .collect();
        atoms.push(Atom::new(Vec3::new(6.0, -2.0, 3.0), Element::new("C")));
        let bonds = (0..59).step_by(3).map(|i| Bond::new(i, i + 1, 1)).collect();
        let mut viewer = MoleculeViewer::new();
        viewer.set_molecule(Molecule::from_atoms_bonds(atoms, bonds));
        viewer
    }

    /// Rays from all around the jumble, aimed at points inside it.
    fn rays() -> Vec<(Vec3, Vec3)> {
        let mut rays = Vec::new();
        for i in 0..40 {
            for j in 0..12 {
                let (theta, phi) = (i as f32 * 0.157, j as f32 * 0.52);
                let from = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.sin() * phi.sin(),
                    theta.cos(),
                ) * 20.0;
                let to = Vec3::new(
                    0.75 + (i as f32 * 0.37).sin() * 0.6,
                    0.75 + (j as f32 * 0.91).cos() * 0.6,
                    0.75 + ((i * j) as f32 * 0.13).sin() * 0.6,
                );
                rays.push((from, (to - from).to_normalized()));
            }
        }
        // Along the axes, through the stray.
        rays.push((Vec3::new(6.0, -2.0, -10.0), Vec3::new(0.0, 0.0, 1.0)));
        rays.push((Vec3::new(20.0, -2.0, 3.0), Vec3::new(-1.0, 0.0, 0.0)));
        rays
    }

    fn library(viewer: &MoleculeViewer, origin: Vec3, dir: Vec3) -> Option<Picked> {
        match viewer.pick(origin, dir) {
            Some(ViewerEvent::AtomClicked(i)) => Some(Picked::Atom(i)),
            Some(ViewerEvent::BondClicked(i)) => Some(Picked::Bond(i)),
            _ => None,
        }
    }

    /// With no radii set, picking agrees with the library's own picking (which tests the
    /// ball-and-stick balls): through the molecule, its periodic images and hidden atoms.
    #[test]
    fn without_radii_it_picks_what_the_library_picks() {
        let mut viewer = jumble();
        let check = |viewer: &MoleculeViewer| {
            let grid = AtomGrid::build(viewer);
            let mut atoms = 0;
            let mut bonds = 0;
            for (origin, dir) in rays() {
                let ours = pick(viewer, &grid, origin, dir);
                assert_eq!(ours, library(viewer, origin, dir), "{origin:?} {dir:?}");
                match ours {
                    Some(Picked::Atom(_)) => atoms += 1,
                    Some(Picked::Bond(_)) => bonds += 1,
                    None => {}
                }
            }
            // The rays exercise all three outcomes.
            assert!(
                atoms > 50 && bonds > 0 && atoms + bonds < rays().len(),
                "{atoms} {bonds}"
            );
        };
        check(&viewer);

        viewer.set_visible_atoms(Some((0..61).map(|i| i % 4 != 0).collect()));
        check(&viewer);

        viewer.set_periodic_images(Some(PeriodicImages::new(
            SimulationCellState::rectangular(Vec3::new(2.0, 2.0, 2.0)),
            [3, 1, 2],
        )));
        check(&viewer);
    }

    /// Two atoms 170 µm apart: the grid's cells grow with the extent rather than the ray
    /// walking the empty stretch between them cell by cell, and picks still find the nearest.
    #[test]
    fn far_flung_atoms_keep_the_walk_short() {
        let far = Vec3::new(100_000.0, 100_000.0, 100_000.0);
        let atoms = vec![
            Atom::new(Vec3::new(0.0, 0.0, 0.0), Element::new("C")),
            Atom::new(far, Element::new("C")),
        ];
        let mut viewer = MoleculeViewer::new();
        viewer.set_molecule(Molecule::from_atoms_bonds(atoms, Vec::new()));
        let grid = AtomGrid::build(&viewer);
        let diagonal = Vec3::new(1.0, 1.0, 1.0).to_normalized();
        let bound = 3 * MAX_CELLS_PER_AXIS as usize;

        // The nearest atom where it is near enough for f32 to tell; `None` to only check
        // against the library.
        for (origin, dir, nearest) in [
            // From beside the first atom, along the diagonal to the other, 170 µm off.
            (Vec3::new(1.0, 1.0, 1.0), diagonal, None),
            // From before the first atom, through both.
            (
                Vec3::new(-1.0, -1.0, -1.0),
                diagonal,
                Some(Some(Picked::Atom(0))),
            ),
            // From beyond the far atom, back through both.
            (far + diagonal, -diagonal, Some(Some(Picked::Atom(1)))),
            // Past the first atom, missing both.
            (
                Vec3::new(0.0, 5.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Some(None),
            ),
        ] {
            let walked = grid.along_ray(origin, dir, |_, _| true).unwrap();
            assert!(walked <= bound, "{walked} cells walked from {origin:?}");
            let ours = pick(&viewer, &grid, origin, dir);
            assert_eq!(ours, library(&viewer, origin, dir), "{origin:?}");
            if let Some(nearest) = nearest {
                assert_eq!(ours, nearest, "{origin:?}");
            }
        }
    }

    /// Atoms strewn sparsely over a wide box: the walk stays bounded and picks agree with a
    /// brute-force scan for the nearest atom.
    #[test]
    fn sparse_atoms_are_picked_nearest_first() {
        let mut seed = 0x9e37_79b9_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        // A few atoms on the diagonal (so rays along it pass several), and a scatter.
        let mut atoms: Vec<Atom> = [0.0, 3.0, 40.0, 150.0, 400.0]
            .iter()
            .map(|&s| Atom::new(Vec3::new(s, s, s), Element::new("O")))
            .collect();
        atoms.extend(
            (0..40)
                .map(|_| Atom::new(Vec3::new(next(), next(), next()) * 400.0, Element::new("C"))),
        );
        let mut viewer = MoleculeViewer::new();
        let count = atoms.len();
        viewer.set_molecule(Molecule::from_atoms_bonds(atoms, Vec::new()));
        // Spheres a nm across, wide enough against f32 rounding hundreds of nm off.
        viewer.set_atom_radii(Some(vec![1.0; count]));
        let grid = AtomGrid::build(&viewer);
        let positions: Vec<Vec3> = viewer
            .molecule()
            .unwrap()
            .atoms
            .iter()
            .map(|a| a.position)
            .collect();
        let bound = 3 * MAX_CELLS_PER_AXIS as usize;

        let diagonal = Vec3::new(1.0, 1.0, 1.0).to_normalized();
        let mut rays = vec![
            (Vec3::new(-10.0, -10.0, -10.0), diagonal),
            (Vec3::new(1.0, 1.0, 1.0), diagonal),
            (Vec3::new(450.0, 450.0, 450.0), -diagonal),
            (Vec3::new(100.0, 100.0, 100.0), -diagonal),
        ];
        // At each scattered atom from a far corner.
        for p in &positions {
            let from = Vec3::new(-100.0, 500.0, -50.0);
            rays.push((from, (*p - from).to_normalized()));
        }
        for (origin, dir) in rays {
            let walked = grid.along_ray(origin, dir, |_, _| true).unwrap();
            assert!(walked <= bound, "{walked} cells walked from {origin:?}");
            let nearest = positions
                .iter()
                .zip(&grid.radii)
                .enumerate()
                .filter_map(|(i, (&p, &r))| {
                    ray_sphere(origin, dir, p, r)
                        .filter(|&t| t > 0.0)
                        .map(|t| (t, i))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, i)| Picked::Atom(i));
            assert!(nearest.is_some(), "{origin:?} {dir:?}");
            assert_eq!(pick(&viewer, &grid, origin, dir), nearest, "{origin:?}");
        }
    }

    /// Picking finds the nearest atom at the radii set: a brute-force scan over every atom
    /// agrees, at radii larger and smaller than the library's.
    #[test]
    fn it_finds_the_nearest_atom_at_the_radii_set() {
        let mut viewer = jumble();
        viewer.molecule_mut().unwrap().bonds.clear();
        for scale in [0.25, 1.0] {
            let radii: Vec<f32> = viewer
                .molecule()
                .unwrap()
                .atoms
                .iter()
                .map(|a| moleucle_3dview_rs::vdw_radius(&a.element) * scale)
                .collect();
            viewer.set_atom_radii(Some(radii.clone()));
            let grid = AtomGrid::build(&viewer);
            for (origin, dir) in rays() {
                let nearest = viewer
                    .molecule()
                    .unwrap()
                    .atoms
                    .iter()
                    .zip(&radii)
                    .enumerate()
                    .filter_map(|(i, (a, &r))| {
                        ray_sphere(origin, dir, a.position, r)
                            .filter(|&t| t > 0.0)
                            .map(|t| (t, i))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, i)| Picked::Atom(i));
                assert_eq!(
                    pick(&viewer, &grid, origin, dir),
                    nearest,
                    "{scale} {origin:?}"
                );
            }
        }
    }
}
