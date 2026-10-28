//! Measuring the assembly: up to four atoms picked anywhere in it, in one molecule or across
//! several (copies of the same monomer included), for the distance, angle or dihedral between
//! them. Read only: nothing here changes the design.
//!
//! A picked atom is remembered by where it comes from — its monomer, its copy ([`Origin`]) and
//! its place in the monomer — not by its index in the build, so when the design changes and
//! the assembly is rebuilt the same atom is found again (or found gone) even though the
//! indices have moved. The arithmetic is the 3D view's ([`Measure`]).

use super::build::Built;
use super::model::{Design, Origin};
use crate::three_d::edit::{MAX_PICKED, Measure};

/// One atom of the assembly, named so that it survives a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtomRef {
    /// The monomer it is an atom of.
    pub component: usize,
    /// Which copy of the monomer.
    pub origin: Origin,
    /// Its index among the monomer's atoms.
    pub atom: usize,
}

impl AtomRef {
    /// Atom `a` of `built`, which was built from `design`.
    pub fn of(design: &Design, built: &Built, a: usize) -> Option<Self> {
        let &m = built.molecule_of_atom.get(a)?;
        Some(Self {
            component: *built.component_of_molecule.get(m)?,
            origin: design.origin_of(m)?,
            atom: a - built.molecule_start[m],
        })
    }

    /// Its index in `built` (built from `design`), if the assembly still has it.
    pub fn index(&self, design: &Design, built: &Built) -> Option<usize> {
        let m = design.molecule_of(self.origin)?;
        if built.component_of_molecule.get(m) != Some(&self.component) {
            return None;
        }
        let atoms = built.molecule_atoms(m);
        let a = atoms.start + self.atom;
        (a < atoms.end).then_some(a)
    }
}

/// The atoms picked to measure, in the order they were picked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeasurePick {
    atoms: Vec<AtomRef>,
}

impl MeasurePick {
    pub fn atoms(&self) -> &[AtomRef] {
        &self.atoms
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    /// A click on `atom`: a picked atom is let go, any other is added — after four (a dihedral)
    /// it starts a new measurement.
    pub fn click(&mut self, atom: AtomRef) {
        if let Some(at) = self.atoms.iter().position(|&a| a == atom) {
            self.atoms.remove(at);
            return;
        }
        if self.atoms.len() >= MAX_PICKED {
            self.atoms.clear();
        }
        self.atoms.push(atom);
    }

    /// Pick exactly these two atoms (a bond's ends).
    pub fn pick_pair(&mut self, a: AtomRef, b: AtomRef) {
        self.atoms = vec![a, b];
    }

    pub fn clear(&mut self) {
        self.atoms.clear();
    }

    /// Follow `built` (built from `design`): let go of the atoms it no longer has and return
    /// the indices of the others, in order.
    pub fn follow(&mut self, design: &Design, built: &Built) -> Vec<usize> {
        let mut indices = Vec::with_capacity(self.atoms.len());
        self.atoms.retain(|r| match r.index(design, built) {
            Some(a) => {
                indices.push(a);
                true
            }
            None => false,
        });
        indices
    }
}

/// What atoms `atoms` of `built` measure (two to four of them).
pub fn measure(built: &Built, atoms: &[usize]) -> Option<Measure> {
    let points = atoms
        .iter()
        .map(|&a| built.atoms.get(a).map(|atom| atom.pos))
        .collect::<Option<Vec<_>>>()?;
    Measure::of_points(&points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::build::build;
    use crate::assembly::math::Rigid;
    use crate::assembly::model::{Component, Instance, Ring, Step};
    use crate::three_d::{Atom3d, Bond3d};

    fn component(name: &str, atoms: &[(&str, [f64; 3])]) -> Component {
        Component {
            name: name.into(),
            atoms: atoms
                .iter()
                .map(|&(element, pos)| Atom3d {
                    element: element.into(),
                    pos,
                })
                .collect(),
            bonds: (1..atoms.len())
                .map(|i| Bond3d {
                    begin: 0,
                    end: i,
                    order: 1,
                })
                .collect(),
            orient: None,
        }
    }

    fn water() -> Component {
        component(
            "water",
            &[
                ("O", [0.0, 0.0, 0.0]),
                ("H", [0.96, 0.0, 0.0]),
                ("H", [-0.24, 0.93, 0.0]),
            ],
        )
    }

    fn placed(component: usize, at: [f64; 3]) -> Instance {
        Instance {
            component,
            transform: Rigid::translation(at),
        }
    }

    fn ring(n: u32, radius: f64) -> Step {
        Step::Ring(Ring {
            enabled: true,
            n,
            radius,
            tilt_deg: 0.0,
        })
    }

    fn picked(design: &Design, built: &Built, atoms: &[usize]) -> MeasurePick {
        let mut pick = MeasurePick::default();
        for &a in atoms {
            pick.click(AtomRef::of(design, built, a).unwrap());
        }
        pick
    }

    fn value(design: &Design, built: &Built, pick: &mut MeasurePick) -> Measure {
        let atoms = pick.follow(design, built);
        measure(built, &atoms).unwrap()
    }

    /// Atoms of four different molecules — the steps' copy of one monomer and hand-placed
    /// copies of it and of another — at known places.
    #[test]
    fn distance_angle_and_dihedral_across_molecules() {
        let design = Design {
            components: vec![
                component("dot", &[("C", [0.0, 0.0, 0.0])]),
                component("pair", &[("N", [0.0, 0.0, 0.0]), ("O", [0.0, 0.0, 1.0])]),
            ],
            manual: vec![
                placed(0, [3.0, 0.0, 0.0]),
                placed(1, [3.0, 4.0, -1.0]),
                placed(0, [3.0, 4.0, 5.0]),
            ],
            ..Default::default()
        };
        let built = build(&design, 1).unwrap();
        assert_eq!(built.molecule_count(), 4);
        // C (origin), C (3,0,0), the pair's O (3,4,0), C (3,4,5).
        let [a, b, c, d] = [0, 1, 3, 4];
        assert_eq!(built.atoms[c].pos, [3.0, 4.0, 0.0]);
        let mut pick = picked(&design, &built, &[a, b]);
        let Measure::Distance(ab) = value(&design, &built, &mut pick) else {
            panic!("two atoms measure a distance");
        };
        assert!((ab - 3.0).abs() < 1e-9, "{ab}");
        pick.click(AtomRef::of(&design, &built, c).unwrap());
        let Measure::Angle(abc) = value(&design, &built, &mut pick) else {
            panic!("three atoms measure an angle");
        };
        assert!((abc - 90.0).abs() < 1e-9, "{abc}");
        // a–c is the hypotenuse of a 3-4-5 triangle: the angle at a is atan(4/3).
        let mut pick = picked(&design, &built, &[b, a, c]);
        let Measure::Angle(bac) = value(&design, &built, &mut pick) else {
            panic!("three atoms measure an angle");
        };
        assert!(
            (bac - (4.0f64 / 3.0).atan().to_degrees()).abs() < 1e-9,
            "{bac}"
        );
        let mut pick = picked(&design, &built, &[a, b, c, d]);
        let Measure::Dihedral(abcd) = value(&design, &built, &mut pick) else {
            panic!("four atoms measure a dihedral");
        };
        assert!((abcd.abs() - 90.0).abs() < 1e-9, "{abcd}");
        assert_eq!(Measure::Distance(ab).to_string(), "3.000 Å");
        assert_eq!(Measure::Angle(abc).to_string(), "90.0°");
    }

    /// Two copies the ring makes of the same monomer.
    #[test]
    fn copies_of_one_monomer_are_measured_apart() {
        let design = Design {
            components: vec![water()],
            steps: vec![ring(6, 5.0)],
            ..Default::default()
        };
        let built = build(&design, 1).unwrap();
        // The oxygens of neighbouring copies sit a chord of the ring apart: 2 r sin(π/6) = r.
        let mut pick = picked(&design, &built, &[0, 3]);
        let Measure::Distance(d) = value(&design, &built, &mut pick) else {
            panic!("a distance");
        };
        assert!((d - 5.0).abs() < 1e-9, "{d}");
    }

    #[test]
    fn clicks_let_go_and_a_fifth_starts_over() {
        let design = Design {
            components: vec![water()],
            steps: vec![ring(6, 5.0)],
            ..Default::default()
        };
        let built = build(&design, 1).unwrap();
        let r = |a| AtomRef::of(&design, &built, a).unwrap();
        let mut pick = picked(&design, &built, &[0, 3, 6]);
        pick.click(r(3));
        assert_eq!(
            pick.atoms(),
            &[r(0), r(6)],
            "a picked atom clicked again is let go"
        );
        pick.click(r(9));
        pick.click(r(12));
        assert_eq!(pick.atoms().len(), 4);
        pick.click(r(15));
        assert_eq!(pick.atoms(), &[r(15)], "the fifth starts a new measurement");
        pick.pick_pair(r(1), r(2));
        assert_eq!(pick.atoms(), &[r(1), r(2)]);
        pick.clear();
        assert!(pick.is_empty());
    }

    /// The design changes under a measurement: the atoms that remain are found again at their
    /// new places (and new indices); those that went are let go.
    #[test]
    fn a_rebuild_updates_the_atoms_that_remain_and_drops_the_others() {
        let mut design = Design {
            components: vec![water()],
            steps: vec![ring(6, 5.0)],
            ..Default::default()
        };
        let built = build(&design, 1).unwrap();
        // The oxygens of copies 1 and 3, and an H of copy 5.
        let mut pick = picked(&design, &built, &[3, 9, 16]);
        let before = pick.atoms().to_vec();
        assert_eq!(before[2].origin, Origin::Step(5));
        assert_eq!(pick.follow(&design, &built), vec![3, 9, 16]);

        // A wider ring: same atoms, new places. Copies 1 and 3 are opposite-ish: 2 r sin(π/3).
        design.steps = vec![ring(6, 8.0)];
        let built = build(&design, 2).unwrap();
        pick.atoms.truncate(2);
        let Measure::Distance(d) = value(&design, &built, &mut pick) else {
            panic!("a distance");
        };
        assert!((d - 2.0 * 8.0 * (std::f64::consts::PI / 3.0).sin()).abs() < 1e-9);

        // Copy 0 deleted: every molecule after it moves down, yet the picks are the same atoms.
        let mut pick = MeasurePick {
            atoms: before.clone(),
        };
        assert!(design.remove_molecule(0));
        let built = build(&design, 3).unwrap();
        assert_eq!(pick.follow(&design, &built), vec![0, 6, 13]);
        assert_eq!(pick.atoms(), &before[..]);

        // A smaller ring: copy 5 is gone and let go; copies 1 and 3 stay.
        design.steps = vec![ring(4, 8.0)];
        let built = build(&design, 4).unwrap();
        assert_eq!(pick.follow(&design, &built), vec![0, 6]);
        assert_eq!(pick.atoms(), &before[..2]);
        // Deleting a picked molecule lets its atom go too.
        assert!(design.remove_molecule(0));
        let built = build(&design, 5).unwrap();
        assert_eq!(pick.follow(&design, &built), vec![3]);
        assert_eq!(pick.atoms(), &before[1..2]);
    }

    /// A hand-placed molecule is found again when one before it is removed, and its atom is let
    /// go when it is itself removed or its monomer is replaced by a smaller one.
    #[test]
    fn hand_placed_atoms_follow_their_molecule() {
        let mut design = Design {
            components: vec![water()],
            manual: vec![placed(0, [10.0, 0.0, 0.0]), placed(0, [20.0, 0.0, 0.0])],
            ..Default::default()
        };
        let built = build(&design, 1).unwrap();
        let mut pick = picked(&design, &built, &[0, 8]);
        assert_eq!(pick.atoms()[1].origin, Origin::Manual(1));
        design.manual[1].transform = Rigid::translation([30.0, 0.0, 0.0]);
        let built = build(&design, 2).unwrap();
        let mut copy = pick.clone();
        let Measure::Distance(d) = value(&design, &built, &mut copy) else {
            panic!("a distance");
        };
        assert!((d - (30.0f64 - 0.24).hypot(0.93)).abs() < 1e-9, "{d}");
        // The monomer loses its last atom: the picked H is gone, the O stays.
        design.components[0].atoms.pop();
        design.components[0].bonds.pop();
        let built = build(&design, 3).unwrap();
        assert_eq!(pick.follow(&design, &built), vec![0]);
    }
}
