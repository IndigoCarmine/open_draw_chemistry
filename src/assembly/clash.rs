//! Atoms of different molecules that come too close, found with a cell list (a spatial grid)
//! in time proportional to the number of atoms.
//!
//! A pair clashes by the same rule as the 3D view's [`geom::clashes`]: closer than
//! [`geom::clash_limit`] (a fraction of the van der Waals contact, a smaller one for a
//! hydrogen bond). Only pairs in different molecules count — inside a molecule the bonds
//! decide the distances. With a periodic box (a crystal) every periodic image counts as a
//! molecule of its own, and a pair is reported once, at its closest image.
//!
//! The grid's bins are at least as wide as the largest possible clash distance, so every
//! partner of an atom lies in its own bin or one of the 26 around it. In a periodic box the
//! bins are laid out in fractional coordinates and the search wraps around, adding the lattice
//! translation it crossed, so the images found are exact even in a skewed (triclinic) cell.

use super::build::Built;
use super::math::{self, Mat3, Vec3};
use crate::three_d::geom::{self, Clash, ClashKind};

/// At most this many clashing pairs are kept; past it the search stops and says so (a design
/// with everything on top of everything else would otherwise list millions).
pub const MAX_CLASHES: usize = 100_000;

/// A periodic box: the three lattice vectors (ångström). The crystal builder's unit cell
/// gives them; every atom then repeats at every lattice translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Periodic {
    /// Lattice vectors as the columns: a point at fractional `f` is `lattice · f`.
    lattice: Mat3,
    inverse: Mat3,
}

impl Periodic {
    /// The box spanned by `a`, `b` and `c`, or `None` when they span no volume.
    pub fn new([a, b, c]: [Vec3; 3]) -> Option<Self> {
        let lattice = math::transpose(&[a, b, c]);
        let volume = math::dot(a, math::cross(b, c));
        if volume.abs() < 1e-9 {
            return None;
        }
        // The inverse's rows are the reciprocal vectors (b×c, c×a, a×b) / V.
        let inverse = [
            math::scale(math::cross(b, c), 1.0 / volume),
            math::scale(math::cross(c, a), 1.0 / volume),
            math::scale(math::cross(a, b), 1.0 / volume),
        ];
        Some(Self { lattice, inverse })
    }

    fn vectors(&self) -> [Vec3; 3] {
        math::transpose(&self.lattice)
    }

    fn fractional(&self, p: Vec3) -> Vec3 {
        math::mat_vec(&self.inverse, p)
    }

    fn cartesian(&self, f: Vec3) -> Vec3 {
        math::mat_vec(&self.lattice, f)
    }

    /// The distance between the opposite faces along each axis.
    fn widths(&self) -> Vec3 {
        let [a, b, c] = self.vectors();
        let volume = math::dot(a, math::cross(b, c)).abs();
        [
            volume / math::norm(math::cross(b, c)),
            volume / math::norm(math::cross(c, a)),
            volume / math::norm(math::cross(a, b)),
        ]
    }
}

/// The clashing pairs of an assembly, worst (closest relative to its limit) first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clashes {
    pub pairs: Vec<Clash>,
    /// More than [`MAX_CLASHES`] pairs: the search stopped early, and `pairs` is only part.
    pub truncated: bool,
}

/// Each atom's clash kind in `built`, from its element and its bonded neighbours.
pub fn kinds(built: &Built) -> Vec<ClashKind> {
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); built.atoms.len()];
    for b in &built.bonds {
        if b.begin < neighbors.len() && b.end < neighbors.len() {
            neighbors[b.begin].push(b.end);
            neighbors[b.end].push(b.begin);
        }
    }
    built
        .atoms
        .iter()
        .zip(&neighbors)
        .map(|(atom, n)| {
            ClashKind::of(
                &atom.element,
                n.iter().map(|&j| built.atoms[j].element.as_str()),
            )
        })
        .collect()
}

/// The clashes between the molecules of `built`, in an optional periodic box.
pub fn find(built: &Built, periodic: Option<&Periodic>) -> Clashes {
    let positions: Vec<Vec3> = built.atoms.iter().map(|a| a.pos).collect();
    find_in(&positions, &kinds(built), &built.molecule_of_atom, periodic)
}

/// The periodic box of `built`: its supercell (the box of what was built), if it has one.
pub fn periodic_of(built: &Built) -> Option<Periodic> {
    built
        .cell
        .as_ref()
        .and_then(|cell| Periodic::new(cell.vectors()))
}

/// Whether `clash` was found through a periodic image: its distance is not the direct one.
pub fn through_image(built: &Built, clash: &Clash) -> bool {
    let d = math::norm(math::sub(
        built.atoms[clash.a].pos,
        built.atoms[clash.b].pos,
    ));
    (d - clash.distance).abs() > 1e-6
}

/// The largest distance at which any two of these atoms can clash.
fn cutoff(kinds: &[ClashKind]) -> f64 {
    let r = kinds.iter().map(|k| k.radius).fold(0.0, f64::max);
    geom::CLASH_FACTOR * 2.0 * r
}

/// The clashes between atoms of different `molecules` (one entry per atom).
pub fn find_in(
    positions: &[Vec3],
    kinds: &[ClashKind],
    molecules: &[usize],
    periodic: Option<&Periodic>,
) -> Clashes {
    let n = positions.len().min(kinds.len()).min(molecules.len());
    if n == 0 {
        return Clashes::default();
    }
    let grid = Grid::new(&positions[..n], cutoff(&kinds[..n]), periodic);
    let mut hits: Vec<(f64, Clash)> = Vec::new();
    let mut truncated = false;
    'atoms: for i in 0..n {
        let p = grid.wrapped[i];
        let mut stop = false;
        grid.visit(grid.bin_of[i], |j, shift, cells| {
            if j <= i || stop {
                return;
            }
            // Wrapping moved each atom by its own lattice translation: the pair is the
            // molecule with itself only when the translations cancel.
            let own = (0..3).all(|k| cells[k] + grid.moved_by[i][k] == grid.moved_by[j][k]);
            if own && molecules[i] == molecules[j] {
                return;
            }
            let q = math::add(grid.wrapped[j], shift);
            let limit = geom::clash_limit(kinds[i], kinds[j]);
            let d2 = squared(math::sub(q, p));
            if d2 < limit * limit {
                let d = d2.sqrt();
                hits.push((
                    d / limit,
                    Clash {
                        a: i,
                        b: j,
                        distance: d,
                    },
                ));
                if hits.len() > MAX_CLASHES {
                    stop = true;
                }
            }
        });
        if stop {
            truncated = true;
            break 'atoms;
        }
    }
    if periodic.is_some() {
        // A pair may clash through several images: keep the closest.
        hits.sort_by(|x, y| {
            (x.1.a, x.1.b)
                .cmp(&(y.1.a, y.1.b))
                .then(x.0.total_cmp(&y.0))
        });
        hits.dedup_by(|later, kept| (later.1.a, later.1.b) == (kept.1.a, kept.1.b));
    }
    hits.sort_by(|x, y| x.0.total_cmp(&y.0));
    hits.truncate(MAX_CLASHES);
    Clashes {
        pairs: hits.into_iter().map(|(_, c)| c).collect(),
        truncated,
    }
}

/// The same as [`find_in`] by testing every pair (and, in a box, the images up to two cells
/// away): the reference the cell list is tested against. Quadratic — for tests only.
#[doc(hidden)]
pub fn brute_force(
    positions: &[Vec3],
    kinds: &[ClashKind],
    molecules: &[usize],
    periodic: Option<&Periodic>,
) -> Vec<Clash> {
    let n = positions.len().min(kinds.len()).min(molecules.len());
    let mut shifts = vec![([0.0; 3], [0i64; 3])];
    let mut positions = positions[..n].to_vec();
    let mut moved_by = vec![[0i64; 3]; n];
    if let Some(box_) = periodic {
        // Into the box first, so two cells either way reach every close image.
        for (p, by) in positions.iter_mut().zip(&mut moved_by) {
            let cell = box_.fractional(*p).map(f64::floor);
            *by = cell.map(|x| x as i64);
            *p = math::sub(*p, box_.cartesian(cell));
        }
        shifts.clear();
        for x in -2..=2 {
            for y in -2..=2 {
                for z in -2..=2 {
                    let cells = [x, y, z];
                    shifts.push((box_.cartesian(cells.map(|x| x as f64)), cells));
                }
            }
        }
    }
    let mut out = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let limit = geom::clash_limit(kinds[i], kinds[j]);
            let closest = shifts
                .iter()
                .filter(|(_, cells)| {
                    molecules[i] != molecules[j]
                        || (0..3).any(|k| cells[k] + moved_by[i][k] != moved_by[j][k])
                })
                .map(|(s, _)| math::distance(positions[i], math::add(positions[j], *s)))
                .fold(f64::INFINITY, f64::min);
            if closest < limit {
                out.push((
                    closest / limit,
                    Clash {
                        a: i,
                        b: j,
                        distance: closest,
                    },
                ));
            }
        }
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out.into_iter().map(|(_, c)| c).collect()
}

fn squared(v: Vec3) -> f64 {
    math::dot(v, v)
}

/// How positions map onto the grid.
#[derive(Debug, Clone, Copy)]
enum Frame {
    /// The bounding box of the atoms, no wrapping.
    Open {
        min: Vec3,
        size: Vec3,
    },
    Periodic(Periodic),
}

/// A cell list: atoms sorted into bins at least `cutoff` wide.
pub struct Grid {
    frame: Frame,
    bins: [usize; 3],
    /// How many bins either way the search must look along each axis.
    reach: [i64; 3],
    /// The atoms of bin `k` are `order[start[k]..start[k + 1]]`.
    start: Vec<usize>,
    order: Vec<usize>,
    /// Each atom's position, moved into the box in a periodic frame.
    wrapped: Vec<Vec3>,
    /// The lattice translation (in cells) each atom was moved back by when wrapped.
    moved_by: Vec<[i64; 3]>,
    bin_of: Vec<[i64; 3]>,
}

impl Grid {
    /// The grid of `positions` for neighbours within `cutoff`, in an optional periodic box.
    pub fn new(positions: &[Vec3], cutoff: f64, periodic: Option<&Periodic>) -> Self {
        let cutoff = cutoff.max(1e-6);
        let (frame, widths) = match periodic {
            Some(box_) => (Frame::Periodic(*box_), box_.widths()),
            None => {
                let mut min = [f64::INFINITY; 3];
                let mut max = [f64::NEG_INFINITY; 3];
                for p in positions {
                    for k in 0..3 {
                        min[k] = min[k].min(p[k]);
                        max[k] = max[k].max(p[k]);
                    }
                }
                if positions.is_empty() {
                    (min, max) = ([0.0; 3], [0.0; 3]);
                }
                let size = [
                    (max[0] - min[0]).max(1e-9),
                    (max[1] - min[1]).max(1e-9),
                    (max[2] - min[2]).max(1e-9),
                ];
                (Frame::Open { min, size }, size)
            }
        };
        // Bins no narrower than the cutoff, and not many more of them than atoms (a sparse
        // assembly in a large box would otherwise allocate a huge, empty grid).
        let mut bins = widths.map(|w| ((w / cutoff).floor() as usize).clamp(1, 1 << 20));
        let cap = (2 * positions.len()).max(8);
        while bins.iter().product::<usize>() > cap {
            let k = (0..3).max_by_key(|&k| bins[k]).expect("three axes");
            bins[k] = (bins[k] / 2).max(1);
        }
        let reach: [i64; 3] = std::array::from_fn(|k| match frame {
            Frame::Open { .. } => 1,
            // A box narrower than the cutoff needs images further than one bin away.
            Frame::Periodic(_) => ((cutoff * bins[k] as f64 / widths[k]).ceil() as i64).max(1),
        });

        let mut wrapped = Vec::with_capacity(positions.len());
        let mut bin_of = Vec::with_capacity(positions.len());
        let mut moved_by = Vec::with_capacity(positions.len());
        for &p in positions {
            let (fraction, moved) = match &frame {
                Frame::Open { min, size } => {
                    (std::array::from_fn(|k| (p[k] - min[k]) / size[k]), p)
                }
                Frame::Periodic(box_) => {
                    let f = box_.fractional(p);
                    let cell = f.map(f64::floor);
                    moved_by.push(cell.map(|x| x as i64));
                    let f = math::sub(f, cell);
                    (f, math::sub(p, box_.cartesian(cell)))
                }
            };
            moved_by.resize(wrapped.len() + 1, [0; 3]);
            let fraction: Vec3 = fraction;
            wrapped.push(moved);
            bin_of.push(std::array::from_fn(|k| {
                ((fraction[k] * bins[k] as f64).floor() as i64).clamp(0, bins[k] as i64 - 1)
            }));
        }
        let index =
            |b: [i64; 3]| (b[0] as usize * bins[1] + b[1] as usize) * bins[2] + b[2] as usize;
        let total = bins.iter().product::<usize>();
        let mut start = vec![0usize; total + 1];
        for b in &bin_of {
            start[index(*b) + 1] += 1;
        }
        for k in 0..total {
            start[k + 1] += start[k];
        }
        let mut fill = start.clone();
        let mut order = vec![0usize; positions.len()];
        for (i, b) in bin_of.iter().enumerate() {
            let k = index(*b);
            order[fill[k]] = i;
            fill[k] += 1;
        }
        Self {
            frame,
            bins,
            reach,
            start,
            order,
            wrapped,
            moved_by,
            bin_of,
        }
    }

    /// Every atom in the bins around `bin`, with the lattice translation to add to its
    /// (wrapped) position — zero outside a periodic box — and that translation in cells.
    fn visit(&self, bin: [i64; 3], mut f: impl FnMut(usize, Vec3, [i64; 3])) {
        let bins = self.bins.map(|b| b as i64);
        let [rx, ry, rz] = self.reach;
        for dx in -rx..=rx {
            for dy in -ry..=ry {
                for dz in -rz..=rz {
                    let at = [bin[0] + dx, bin[1] + dy, bin[2] + dz];
                    let mut cells = [0i64; 3];
                    let shift = match &self.frame {
                        Frame::Open { .. } => {
                            if (0..3).any(|k| at[k] < 0 || at[k] >= bins[k]) {
                                continue;
                            }
                            [0.0; 3]
                        }
                        Frame::Periodic(box_) => {
                            cells = std::array::from_fn(|k| at[k].div_euclid(bins[k]));
                            if cells == [0; 3] {
                                [0.0; 3]
                            } else {
                                box_.cartesian(cells.map(|x| x as f64))
                            }
                        }
                    };
                    let b: [i64; 3] = std::array::from_fn(|k| at[k].rem_euclid(bins[k]));
                    let k = (b[0] as usize * self.bins[1] + b[1] as usize) * self.bins[2]
                        + b[2] as usize;
                    for &j in &self.order[self.start[k]..self.start[k + 1]] {
                        f(j, shift, cells);
                    }
                }
            }
        }
    }

    /// Whether any atom of the grid is closer to `p` than `within(atom)`, which must not exceed
    /// the grid's cutoff.
    pub fn any_near(&self, p: Vec3, mut within: impl FnMut(usize) -> f64) -> bool {
        let (p, bin) = self.locate(p);
        let mut found = false;
        self.visit(bin, |j, shift, _| {
            if !found {
                let r = within(j);
                found = squared(math::sub(math::add(self.wrapped[j], shift), p)) < r * r;
            }
        });
        found
    }

    /// Every atom of the grid within `r` (which must not exceed the grid's cutoff) of any point
    /// `p`: its index and the vector from `p` to it. In a periodic box an atom comes once for
    /// each of its images within reach, the vector pointing at that image.
    pub fn for_each_within(&self, p: Vec3, r: f64, mut f: impl FnMut(usize, Vec3)) {
        let (moved, bin) = self.locate(p);
        self.visit(bin, |j, shift, _| {
            let d = math::sub(math::add(self.wrapped[j], shift), moved);
            if squared(d) <= r * r {
                f(j, d);
            }
        });
    }

    /// The bin of any point (clamped to the grid outside an open frame), and the point moved
    /// into the box in a periodic one.
    fn locate(&self, p: Vec3) -> (Vec3, [i64; 3]) {
        let (fraction, moved): (Vec3, Vec3) = match &self.frame {
            Frame::Open { min, size } => (std::array::from_fn(|k| (p[k] - min[k]) / size[k]), p),
            Frame::Periodic(box_) => {
                let f = box_.fractional(p);
                let cell = f.map(f64::floor);
                (math::sub(f, cell), math::sub(p, box_.cartesian(cell)))
            }
        };
        let bin = std::array::from_fn(|k| {
            let b = (fraction[k] * self.bins[k] as f64).floor();
            (b.clamp(0.0, self.bins[k] as f64 - 1.0)) as i64
        });
        (moved, bin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    /// `molecules` molecules of three to six atoms scattered in a `size` box.
    fn random_system(
        seed: u64,
        molecules: usize,
        size: f64,
    ) -> (Vec<Vec3>, Vec<ClashKind>, Vec<usize>) {
        let mut rng = Lcg(seed);
        let elements = ["C", "H", "O", "N", "H"];
        let (mut pos, mut kinds, mut mol) = (Vec::new(), Vec::new(), Vec::new());
        for m in 0..molecules {
            let centre = [rng.next() * size, rng.next() * size, rng.next() * size];
            let atoms = 3 + (rng.next() * 4.0) as usize;
            for _ in 0..atoms {
                pos.push([
                    centre[0] + rng.next() * 2.0 - 1.0,
                    centre[1] + rng.next() * 2.0 - 1.0,
                    centre[2] + rng.next() * 2.0 - 1.0,
                ]);
                let e = elements[(rng.next() * elements.len() as f64) as usize];
                // Some hydrogens are polar, to exercise the hydrogen-bond threshold.
                let neighbor = if rng.next() < 0.5 { "O" } else { "C" };
                kinds.push(ClashKind::of(e, std::iter::once(neighbor)));
                mol.push(m);
            }
        }
        (pos, kinds, mol)
    }

    fn same(grid: &Clashes, brute: &[Clash]) {
        assert!(!grid.truncated);
        let key = |c: &Clash| (c.a, c.b);
        let mut a: Vec<_> = grid.pairs.iter().map(|c| (key(c), c.distance)).collect();
        let mut b: Vec<_> = brute.iter().map(|c| (key(c), c.distance)).collect();
        a.sort_by_key(|x| x.0);
        b.sort_by_key(|x| x.0);
        assert_eq!(a.len(), b.len(), "pair counts differ");
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.0, y.0);
            assert!((x.1 - y.1).abs() < 1e-9, "{x:?} vs {y:?}");
        }
    }

    #[test]
    fn the_cell_list_finds_what_testing_every_pair_finds() {
        for seed in 1..=6 {
            let (pos, kinds, mol) = random_system(seed, 60, 14.0);
            let grid = find_in(&pos, &kinds, &mol, None);
            let brute = brute_force(&pos, &kinds, &mol, None);
            assert!(!brute.is_empty(), "the test system should have clashes");
            same(&grid, &brute);
            // Worst first.
            assert!(grid.pairs.windows(2).all(|w| {
                let r = |c: &Clash| c.distance / geom::clash_limit(kinds[c.a], kinds[c.b]);
                r(&w[0]) <= r(&w[1]) + 1e-12
            }));
        }
    }

    #[test]
    fn the_cell_list_finds_periodic_images_in_skewed_boxes() {
        let boxes = [
            [[14.0, 0.0, 0.0], [0.0, 14.0, 0.0], [0.0, 0.0, 14.0]],
            // Triclinic, and one axis narrower than three bins.
            [[13.0, 0.0, 0.0], [4.0, 12.0, 0.0], [-3.0, 2.5, 6.0]],
        ];
        for (seed, vectors) in (10..).zip(boxes) {
            let periodic = Periodic::new(vectors).unwrap();
            let (pos, kinds, mol) = random_system(seed, 50, 14.0);
            let grid = find_in(&pos, &kinds, &mol, Some(&periodic));
            let brute = brute_force(&pos, &kinds, &mol, Some(&periodic));
            same(&grid, &brute);
            // Wrapping makes clashes across the faces: more than in the open box.
            let open = brute_force(&pos, &kinds, &mol, None);
            assert!(
                brute.len() > open.len(),
                "{} vs {}",
                brute.len(),
                open.len()
            );
        }
        assert!(Periodic::new([[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
    }

    #[test]
    fn atoms_of_one_molecule_never_clash_except_with_its_images() {
        let kinds = vec![ClashKind::of("C", std::iter::empty()); 2];
        let pos = vec![[0.0; 3], [0.5, 0.0, 0.0]];
        assert!(find_in(&pos, &kinds, &[0, 0], None).pairs.is_empty());
        assert_eq!(find_in(&pos, &kinds, &[0, 1], None).pairs.len(), 1);
        // In a 2 Å box the molecule meets its own image 1.5 Å away.
        let periodic = Periodic::new([[2.0, 0.0, 0.0], [0.0, 30.0, 0.0], [0.0, 0.0, 30.0]]);
        let found = find_in(&pos, &kinds, &[0, 0], periodic.as_ref());
        assert_eq!(found.pairs.len(), 1);
        assert!((found.pairs[0].distance - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_molecule_across_the_box_face_does_not_clash_with_itself() {
        let kinds = vec![ClashKind::of("C", std::iter::empty()); 2];
        // Straddling x = 0 of a 10 Å box: wrapping puts the two atoms 9.5 Å apart.
        let pos = vec![[-0.25, 5.0, 5.0], [0.25, 5.0, 5.0]];
        let periodic = Periodic::new([[10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]]);
        assert!(
            find_in(&pos, &kinds, &[0, 0], periodic.as_ref())
                .pairs
                .is_empty()
        );
        assert!(brute_force(&pos, &kinds, &[0, 0], periodic.as_ref()).is_empty());
        assert_eq!(
            find_in(&pos, &kinds, &[0, 1], periodic.as_ref())
                .pairs
                .len(),
            1
        );
    }

    #[test]
    fn a_query_finds_nearby_atoms_only() {
        let pos = vec![[0.0; 3], [10.0, 0.0, 0.0], [10.0, 10.0, 10.0]];
        let grid = Grid::new(&pos, 2.5, None);
        assert!(grid.any_near([9.0, 0.5, 0.0], |_| 2.0));
        assert!(!grid.any_near([5.0, 5.0, 5.0], |_| 2.5));
        assert!(grid.any_near([-1.0, 0.0, 0.0], |_| 1.5), "outside the box");
    }
}
