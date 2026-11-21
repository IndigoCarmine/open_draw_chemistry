//! The numbers of a building step ([`Step`]) as named slots: what an operation node's number
//! sockets are keyed by, and what a parameter scan reads and writes.

use serde::{Deserialize, Serialize};

use super::kinds::{PortValue, SocketType};
use crate::assembly::math::Vec3;
use crate::assembly::model::Step;

/// The most copies one step may make (the panel's limit too).
pub const MAX_COPIES: u32 = 10_000;

/// One number (or triple) of a building step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StepField {
    RingCount,
    RingRadius,
    RingTilt,
    StackCount,
    StackRise,
    StackTwist,
    StackSlip,
    ArrayCount,
    /// The array's offset per copy (x, y, z).
    ArrayOffset,
    /// The array's turn per copy about x, y and z.
    ArrayEuler,
    /// The move/rotate step's offset.
    TransformOffset,
    /// The move/rotate step's turn about x, y and z.
    TransformEuler,
    BendRadius,
    BendDirection,
    CellA,
    CellB,
    CellC,
    CellAlpha,
    CellBeta,
    CellGamma,
    /// How many unit cells along a, b and c.
    LatticeReps,
}

const XYZ: [&str; 3] = ["x", "y", "z"];

impl StepField {
    /// The fields `step` has, in panel order.
    pub fn of(step: &Step) -> Vec<StepField> {
        use StepField::*;
        match step {
            Step::Ring(_) => vec![RingCount, RingRadius, RingTilt],
            Step::Stack(_) => vec![StackCount, StackRise, StackTwist, StackSlip],
            Step::Array(_) => vec![ArrayCount, ArrayOffset, ArrayEuler],
            Step::Lattice(_) => vec![
                CellA,
                CellB,
                CellC,
                CellAlpha,
                CellBeta,
                CellGamma,
                LatticeReps,
            ],
            Step::Transform(_) => vec![TransformOffset, TransformEuler],
            Step::Bend(_) => vec![BendRadius, BendDirection],
        }
    }

    /// A number, or three.
    pub fn ty(self) -> SocketType {
        use StepField::*;
        match self {
            ArrayOffset | ArrayEuler | TransformOffset | TransformEuler | LatticeReps => {
                SocketType::Vec3
            }
            _ => SocketType::Number,
        }
    }

    /// Whether the field counts something (rounded, at least one).
    pub fn is_count(self) -> bool {
        use StepField::*;
        matches!(self, RingCount | StackCount | ArrayCount | LatticeReps)
    }

    /// The field's name, short enough for a socket.
    pub fn label(self) -> &'static str {
        use StepField::*;
        match self {
            RingCount => t!("assembly.model.ring.count"),
            RingRadius => t!("assembly.model.ring.radius"),
            RingTilt => t!("assembly.model.ring.tilt"),
            StackCount => t!("assembly.model.stack.layers"),
            StackRise => t!("assembly.model.stack.rise"),
            StackTwist => t!("assembly.model.stack.twist"),
            StackSlip => t!("assembly.model.stack.slip"),
            ArrayCount => t!("assembly.model.array.count"),
            ArrayOffset => t!("assembly.model.array.offset"),
            ArrayEuler | TransformEuler => t!("assembly.model.array.rotation"),
            TransformOffset => t!("assembly.model.transform.offset"),
            BendRadius => t!("assembly.model.bend.radius"),
            BendDirection => t!("assembly.model.bend.direction"),
            CellA => "a",
            CellB => "b",
            CellC => "c",
            CellAlpha => "α",
            CellBeta => "β",
            CellGamma => "γ",
            LatticeReps => t!("assembly.model.lattice.repeats"),
        }
    }

    /// The field's name with the axis of a triple: "Spacing z".
    pub fn axis_label(self, axis: Option<usize>) -> String {
        match axis {
            Some(i) if self.ty() == SocketType::Vec3 => format!("{} {}", self.label(), XYZ[i]),
            _ => self.label().to_owned(),
        }
    }

    pub fn is_angle(self) -> bool {
        use StepField::*;
        matches!(
            self,
            RingTilt
                | StackTwist
                | ArrayEuler
                | TransformEuler
                | BendDirection
                | CellAlpha
                | CellBeta
                | CellGamma
        )
    }

    /// The field's value in `step`, or `None` when the step is of another kind.
    pub fn read(self, step: &Step) -> Option<PortValue> {
        use StepField::*;
        let n = |v: f64| Some(PortValue::Number(v));
        let v = |v: Vec3| Some(PortValue::Vec3(v));
        match (self, step) {
            (RingCount, Step::Ring(s)) => n(f64::from(s.n)),
            (RingRadius, Step::Ring(s)) => n(s.radius),
            (RingTilt, Step::Ring(s)) => n(s.tilt_deg),
            (StackCount, Step::Stack(s)) => n(f64::from(s.n)),
            (StackRise, Step::Stack(s)) => n(s.rise),
            (StackTwist, Step::Stack(s)) => n(s.twist_deg),
            (StackSlip, Step::Stack(s)) => n(s.slip),
            (ArrayCount, Step::Array(s)) => n(f64::from(s.n)),
            (ArrayOffset, Step::Array(s)) => v(s.offset),
            (ArrayEuler, Step::Array(s)) => v(s.euler_deg),
            (TransformOffset, Step::Transform(s)) => v(s.offset),
            (TransformEuler, Step::Transform(s)) => v(s.euler_deg),
            (BendRadius, Step::Bend(s)) => n(s.radius),
            (BendDirection, Step::Bend(s)) => n(s.direction_deg),
            (CellA, Step::Lattice(l)) => n(l.cell.a),
            (CellB, Step::Lattice(l)) => n(l.cell.b),
            (CellC, Step::Lattice(l)) => n(l.cell.c),
            (CellAlpha, Step::Lattice(l)) => n(l.cell.alpha),
            (CellBeta, Step::Lattice(l)) => n(l.cell.beta),
            (CellGamma, Step::Lattice(l)) => n(l.cell.gamma),
            (LatticeReps, Step::Lattice(l)) => v(l.reps.map(f64::from)),
            _ => None,
        }
    }

    /// Put `value` into `step` (a number into a triple sets all three; counts are rounded and
    /// kept within 1..=[`MAX_COPIES`]). Returns whether the step has the field.
    pub fn write(self, step: &mut Step, value: PortValue) -> bool {
        use StepField::*;
        let number = value.number();
        let triple = value.vec3();
        let count = |v: f64| count_of(v);
        match (self, step) {
            (RingCount, Step::Ring(s)) => s.n = count(number),
            (RingRadius, Step::Ring(s)) => s.radius = number,
            (RingTilt, Step::Ring(s)) => s.tilt_deg = number,
            (StackCount, Step::Stack(s)) => s.n = count(number),
            (StackRise, Step::Stack(s)) => s.rise = number,
            (StackTwist, Step::Stack(s)) => s.twist_deg = number,
            (StackSlip, Step::Stack(s)) => s.slip = number,
            (ArrayCount, Step::Array(s)) => s.n = count(number),
            (ArrayOffset, Step::Array(s)) => s.offset = triple,
            (ArrayEuler, Step::Array(s)) => s.euler_deg = triple,
            (TransformOffset, Step::Transform(s)) => s.offset = triple,
            (TransformEuler, Step::Transform(s)) => s.euler_deg = triple,
            (BendRadius, Step::Bend(s)) => s.radius = number,
            (BendDirection, Step::Bend(s)) => s.direction_deg = number,
            (CellA, Step::Lattice(l)) => l.cell.a = number,
            (CellB, Step::Lattice(l)) => l.cell.b = number,
            (CellC, Step::Lattice(l)) => l.cell.c = number,
            (CellAlpha, Step::Lattice(l)) => l.cell.alpha = number,
            (CellBeta, Step::Lattice(l)) => l.cell.beta = number,
            (CellGamma, Step::Lattice(l)) => l.cell.gamma = number,
            (LatticeReps, Step::Lattice(l)) => l.reps = triple.map(count),
            _ => return false,
        }
        true
    }
}

/// A count from a number: rounded, at least one, at most [`MAX_COPIES`] (NaN is one).
pub fn count_of(v: f64) -> u32 {
    if v.is_nan() {
        return 1;
    }
    v.round().clamp(1.0, f64::from(MAX_COPIES)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_round_trips_and_belongs_to_its_step() {
        let steps = [
            Step::new_ring(),
            Step::new_stack(),
            Step::new_array(),
            Step::new_lattice(),
            Step::new_transform(),
            Step::new_bend(),
        ];
        for step in &steps {
            for field in StepField::of(step) {
                let mut s = step.clone();
                let value = match field.ty() {
                    SocketType::Vec3 => PortValue::Vec3([2.0, 3.0, 4.0]),
                    _ => PortValue::Number(7.0),
                };
                assert!(field.write(&mut s, value), "{field:?}");
                assert_eq!(field.read(&s), Some(value), "{field:?}");
                for other in &steps {
                    if std::mem::discriminant(other) != std::mem::discriminant(step) {
                        assert_eq!(field.read(other), None);
                    }
                }
            }
        }
    }

    #[test]
    fn counts_are_rounded_and_clamped() {
        assert_eq!(count_of(2.6), 3);
        assert_eq!(count_of(-4.0), 1);
        assert_eq!(count_of(f64::NAN), 1);
        assert_eq!(count_of(1e9), MAX_COPIES);
        let mut ring = Step::new_ring();
        StepField::RingCount.write(&mut ring, PortValue::Number(0.2));
        assert_eq!(
            StepField::RingCount.read(&ring),
            Some(PortValue::Number(1.0))
        );
    }
}
