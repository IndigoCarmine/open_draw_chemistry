//! Measuring the assembly: up to four atoms picked anywhere in it, in one molecule or across
//! several (copies of the same monomer included), for the distance, angle or dihedral between
//! them. Read only: nothing here changes the design.
//!
//! A picked atom is remembered by where it comes from — its monomer (by its id), its copy
//! ([`Origin`]) and its place in the monomer — not by its index in the build, so when the
//! design changes and the assembly is rebuilt the same atom is found again (or found gone)
//! even though the indices have moved: molecules and monomers removed before it, its copy
//! detached to be moved by hand, undo and redo. The arithmetic is the 3D view's ([`Measure`]).

use super::build::Built;
use super::model::{Design, Origin};
use crate::three_d::edit::{MAX_PICKED, Measure};

/// One atom of the assembly, named so that it survives a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtomRef {
    /// The monomer it is an atom of, by its id ([`Design::component_id`]).
    pub component: u64,
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
            component: design.component_id(*built.component_of_molecule.get(m)?),
            origin: design.origin_of(m)?,
            atom: a - built.molecule_start[m],
        })
    }

    /// Its index in `built` (built from `design`), if the assembly still has it.
    pub fn index(&self, design: &Design, built: &Built) -> Option<usize> {
        let m = design.molecule_of(self.origin)?;
        let component = design.component_index(self.component)?;
        if built.component_of_molecule.get(m) != Some(&component) {
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
    use crate::assembly::math::{self, Rigid};
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
        let mut smaller = design.clone();
        smaller.components[0].atoms.pop();
        smaller.components[0].bonds.pop();
        let built = build(&smaller, 3).unwrap();
        assert_eq!(pick.clone().follow(&smaller, &built), vec![0]);
        // The molecule before it is deleted: the H is found again, a molecule lower.
        assert!(design.remove_molecule(1));
        let built = build(&design, 4).unwrap();
        assert_eq!(pick.follow(&design, &built), vec![0, 5]);
        assert!(math::distance(built.atoms[5].pos, [29.76, 0.93, 0.0]) < 1e-9);
        // It is deleted itself: its H goes.
        assert!(design.remove_molecule(1));
        let built = build(&design, 5).unwrap();
        assert_eq!(pick.follow(&design, &built), vec![0]);
    }
    /// Three hand-placed copies A, B, C of one monomer: deleting A shifts B and C down the list,
    /// yet a pick on B stays on B (not on C) and a pick on A goes (not onto B). Undo — the
    /// design as it was — finds A and B again.
    #[test]
    fn a_pick_stays_on_its_hand_placed_molecule_when_one_before_it_goes() {
        let mut design = Design {
            components: vec![water()],
            manual: vec![
                placed(0, [10.0, 0.0, 0.0]),
                placed(0, [20.0, 0.0, 0.0]),
                placed(0, [30.0, 0.0, 0.0]),
            ],
            ..Default::default()
        };
        let before = design.clone();
        let built = build(&design, 1).unwrap();
        // Molecule 0 is the base at the origin; A, B, C are 1, 2, 3: A's O and B's O.
        let (a, b) = (3, 6);
        assert_eq!(built.atoms[b].pos, [20.0, 0.0, 0.0]);
        let mut pick = picked(&design, &built, &[a, b]);
        assert!(design.remove_molecule(1));
        let built = build(&design, 2).unwrap();
        let atoms = pick.follow(&design, &built);
        assert_eq!(atoms.len(), 1, "A's atom is let go");
        assert_eq!(built.atoms[atoms[0]].pos, [20.0, 0.0, 0.0], "still B's O");
        // Deleting C, after B, changes nothing; deleting B lets it go.
        let mut kept = pick.clone();
        let mut design_c = design.clone();
        assert!(design_c.remove_molecule(2));
        let built_c = build(&design_c, 3).unwrap();
        let atoms = kept.follow(&design_c, &built_c);
        assert_eq!(built_c.atoms[atoms[0]].pos, [20.0, 0.0, 0.0]);
        assert!(design.remove_molecule(1));
        let built = build(&design, 4).unwrap();
        assert!(
            pick.follow(&design, &built).is_empty(),
            "B's atom goes with B"
        );

        // Undo of deleting A: both picks are found again where they were.
        let built = build(&before, 5).unwrap();
        let mut pick = picked(&before, &built, &[a, b]);
        let mut after = before.clone();
        assert!(after.remove_molecule(1));
        let built_after = build(&after, 6).unwrap();
        assert_eq!(pick.follow(&after, &built_after).len(), 1);
        assert_eq!(pick.follow(&before, &built), vec![b]);
        // A molecule added after a deletion is a new one: no pick moves onto it.
        let mut pick = picked(&after, &built_after, &[built_after.atoms.len() - 3]);
        assert!(after.remove_molecule(2));
        after.manual.push(placed(0, [40.0, 0.0, 0.0]));
        let built_after = build(&after, 7).unwrap();
        assert!(pick.follow(&after, &built_after).is_empty());
    }

    /// Removing a monomer: the picks on the others' molecules stay on the same atoms, those on
    /// its own go — none moves to the monomer that takes its place in the list, the base's
    /// successor included.
    #[test]
    fn removing_a_monomer_keeps_the_others_picks_and_drops_its_own() {
        let dot = || component("dot", &[("C", [0.0, 0.0, 0.0])]);
        let mut design = Design {
            components: vec![water(), dot(), dot()],
            manual: vec![
                placed(1, [10.0, 0.0, 0.0]),
                placed(2, [20.0, 0.0, 0.0]),
                placed(0, [30.0, 0.0, 0.0]),
            ],
            ..Default::default()
        };
        let built = build(&design, 1).unwrap();
        // The base water's O, the first dot, the second dot, the hand-placed water's H.
        let mut pick = picked(&design, &built, &[0, 3, 4, 6]);
        design.remove_component(1);
        let built = build(&design, 2).unwrap();
        let atoms = pick.follow(&design, &built);
        let places: Vec<_> = atoms.iter().map(|&a| built.atoms[a].pos).collect();
        let expected = [[0.0; 3], [20.0, 0.0, 0.0], [30.96, 0.0, 0.0]];
        assert_eq!(
            places.len(),
            3,
            "the first dot's pick goes, the others stay"
        );
        for (p, e) in places.iter().zip(expected) {
            assert!(math::distance(*p, e) < 1e-9, "{p:?} is not {e:?}");
        }
        // Removing the base: the steps now copy the dot, whose atom 0 is not the water's O.
        design.remove_component(0);
        assert_eq!(design.base, 0);
        let built = build(&design, 3).unwrap();
        assert_eq!(built.atoms[0].element, "C");
        let atoms = pick.follow(&design, &built);
        assert_eq!(atoms.len(), 1);
        assert_eq!(built.atoms[atoms[0]].pos, [20.0, 0.0, 0.0]);
    }

    /// A copy of the steps detached to be moved by hand is the same molecule: its picks stay
    /// on the same atoms, a click on one of them lets it go, and undoing the detach keeps them.
    #[test]
    fn detaching_a_copy_keeps_its_picks() {
        let mut design = Design {
            components: vec![water()],
            steps: vec![ring(6, 5.0)],
            ..Default::default()
        };
        let before = design.clone();
        let built = build(&design, 1).unwrap();
        // An O of copy 0 and an H of copy 2.
        let (o, h) = (0, 7);
        let h_at = built.atoms[h].pos;
        let mut pick = picked(&design, &built, &[o, h]);
        let moved = design.detach(2).unwrap();
        let built = build(&design, 2).unwrap();
        let atoms = pick.follow(&design, &built);
        assert_eq!(atoms, vec![0, built.molecule_start[moved] + 1]);
        assert!(math::distance(built.atoms[atoms[1]].pos, h_at) < 1e-9);
        // Moving it by hand moves the pick with it.
        design.manual[0].transform = design.manual[0]
            .transform
            .then(&Rigid::translation([0.0, 0.0, 3.0]));
        let built = build(&design, 3).unwrap();
        let atoms = pick.follow(&design, &built);
        assert!((built.atoms[atoms[1]].pos[2] - (h_at[2] + 3.0)).abs() < 1e-9);
        // The detached atom clicked again is the picked one, let go.
        let mut again = pick.clone();
        again.click(AtomRef::of(&design, &built, atoms[1]).unwrap());
        assert_eq!(again.follow(&design, &built), vec![0]);
        // Undo of the detach: the same atoms in the steps' copy.
        let built = build(&before, 4).unwrap();
        assert_eq!(pick.clone().follow(&before, &built), vec![o, h]);
        // Deleting the detached molecule lets its atom go.
        let m = design.manual_molecule(0);
        assert!(design.remove_molecule(m));
        let built = build(&design, 5).unwrap();
        assert_eq!(pick.follow(&design, &built), vec![0]);
    }
}
