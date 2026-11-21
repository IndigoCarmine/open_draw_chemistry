//! Short texts about the building steps: the line under a step node's title.

use crate::assembly::model::{Array, Bend, BendOrient, Pivot, Ring, Stack, Transform};
use crate::assembly::{Step, symop};

/// The step's parameters in a few characters: "7個 · z 3.20Å".
pub(super) fn summary(step: &Step) -> String {
    match step {
        Step::Array(Array {
            n,
            offset,
            euler_deg,
            ..
        }) => {
            let mut s = format!(
                "{} · {}",
                tn!("assembly.pipeline.summary.copies", *n),
                offset_text(offset)
            );
            if euler_deg.iter().any(|&a| a != 0.0) {
                s.push_str(" · ");
                s.push_str(t!("assembly.pipeline.summary.rotated"));
            }
            s
        }
        Step::Ring(Ring {
            n,
            radius,
            tilt_deg,
            ..
        }) => {
            let mut s = format!(
                "{} · r {radius:.2}Å",
                tn!("assembly.pipeline.summary.copies", *n)
            );
            if *tilt_deg != 0.0 {
                s.push_str(&format!(" · {tilt_deg:.1}°"));
            }
            s
        }
        Step::Stack(Stack {
            n,
            rise,
            twist_deg,
            slip,
            ..
        }) => {
            let mut s = format!(
                "{} · Δz {rise:.2}Å · {twist_deg:.1}°",
                tn!("assembly.pipeline.summary.copies", *n)
            );
            if *slip != 0.0 {
                s.push_str(" · ");
                s.push_str(&t!(
                    "assembly.pipeline.summary.slip",
                    value = format!("{slip:.2}")
                ));
            }
            s
        }
        Step::Transform(Transform {
            offset,
            euler_deg,
            pivot,
            ..
        }) => {
            let mut parts = Vec::new();
            if offset.iter().any(|&v| v != 0.0) {
                parts.push(offset_text(offset));
            }
            if euler_deg.iter().any(|&a| a != 0.0) {
                parts.push(
                    match pivot {
                        Pivot::Centroid => t!("assembly.pipeline.summary.rotated_centroid"),
                        Pivot::Origin => t!("assembly.pipeline.summary.rotated_origin"),
                    }
                    .to_owned(),
                );
            }
            if parts.is_empty() {
                t!("assembly.pipeline.summary.unmoved").to_owned()
            } else {
                parts.join(" · ")
            }
        }
        Step::Bend(Bend {
            radius,
            direction_deg,
            orient,
            ..
        }) => {
            let mut s = format!("r {radius:.2}Å · φ {direction_deg:.0}°");
            if *orient == BendOrient::Keep {
                s.push_str(" · ");
                s.push_str(t!("assembly.pipeline.summary.orientation_kept"));
            }
            s
        }
        Step::Lattice(l) => {
            let group = symop::matching_preset(&l.symops).map_or_else(
                || tn!("assembly.pipeline.summary.operations", l.symops.len()),
                |p| p.name.to_owned(),
            );
            let [a, b, c] = l.reps.map(|r| r.max(1));
            format!("{group} · {a}×{b}×{c}")
        }
    }
}

/// An array's spacing: "z 3.20Å" along one axis, all three otherwise.
fn offset_text(offset: &[f64; 3]) -> String {
    let along: Vec<(&str, f64)> = ["x", "y", "z"]
        .into_iter()
        .zip(offset.iter().copied())
        .filter(|&(_, v)| v != 0.0)
        .collect();
    match along.as_slice() {
        [] => t!("assembly.pipeline.summary.no_spacing").to_owned(),
        [(axis, v)] => format!("{axis} {v:.2}Å"),
        _ => format!("({:.1}, {:.1}, {:.1})Å", offset[0], offset[1], offset[2]),
    }
}
