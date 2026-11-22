//! A calculator graph built, grouped into a custom node, saved and evaluated — no window.
//!
//! `cargo run -p flow_nodes_rs --example calc`

#[path = "common/calc.rs"]
mod calc;

use calc::{Calc, In, NoEnv, Out};
use flow_nodes_rs::{Graph, NodeDef, def, eval, ops};

fn result(graph: &Graph<Calc>, defs: &[NodeDef<Calc>]) -> f64 {
    let outcome = eval::evaluate(graph, defs, &NoEnv);
    for issue in &outcome.issues {
        eprintln!("node {}: {:?}", issue.node.0, issue.problem);
    }
    outcome.output.first().map_or(0.0, |(_, v)| *v)
}

fn main() {
    // (2 + 3) × 4
    let mut g = Graph::<Calc>::default();
    let two = g.add(Calc::Number(2.0), [0.0, 0.0]);
    let add = g.add(Calc::Add { a: 0.0, b: 3.0 }, [220.0, 0.0]);
    let times = g.add(Calc::Multiply { a: 0.0, b: 4.0 }, [440.0, 0.0]);
    let out = g.add(Calc::Result, [660.0, 0.0]);
    let mut defs = Vec::new();
    ops::connect(&mut g, &defs, None, two, Out::Value, add, In::A).unwrap();
    ops::connect(&mut g, &defs, None, add, Out::Value, times, In::A).unwrap();
    ops::connect(&mut g, &defs, None, times, Out::Value, out, In::A).unwrap();
    println!("(2 + 3) × 4 = {}", result(&g, &defs));

    // Gather "+ 3, × 4" into a custom node, and use it twice.
    let (id, first) = ops::group(
        &mut g,
        &mut defs,
        None,
        &[add, times].into(),
        "plus 3 times 4",
    )
    .expect("groupable");
    println!("grouped: {} = {}", defs[0].name, result(&g, &defs));
    let second = g.add(
        Calc::Group(flow_nodes_rs::GroupRef::new(id)),
        [660.0, 120.0],
    );
    let port_out = defs[0].outputs[0].key;
    let port_in = defs[0].inputs[0].key;
    ops::connect(
        &mut g,
        &defs,
        None,
        first,
        Out::Port(port_out),
        second,
        In::Port(port_in),
    )
    .unwrap();
    ops::connect(&mut g, &defs, None, second, Out::Port(port_out), out, In::A).unwrap();
    println!("((2 + 3) × 4 + 3) × 4 = {}", result(&g, &defs));

    // Graphs and custom nodes are plain serde data.
    def::validate(&defs).unwrap();
    g.validate(&defs, None).unwrap();
    let saved = serde_json::to_string(&(&g, &defs)).unwrap();
    let (back, back_defs): (Graph<Calc>, Vec<NodeDef<Calc>>) =
        serde_json::from_str(&saved).unwrap();
    println!(
        "saved {} bytes; read back: {}",
        saved.len(),
        result(&back, &back_defs)
    );
}
