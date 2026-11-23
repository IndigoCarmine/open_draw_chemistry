# flow_nodes_rs

A foundation for Blender-style node graphs in Rust: typed sockets, custom nodes (node groups),
a pull evaluator, number expressions, a node library on disk and an [egui](https://github.com/emilk/egui)
node editor widget.

It knows nothing about what your nodes do. You describe your kinds of node by implementing two
traits for your own types, and the crate gives you the rest. It was split out of
[Open DrawChemistry](https://github.com/IndigoCarmine/open_draw_chemistry), where it runs the
molecular assembly builder's node editor.

## What you get

| Module | What it does |
|---|---|
| `graph` | `Graph<K>`: nodes and wires, saved as plain serde data, with validation |
| `ops` | The edits an editor makes: connect (type-checked, never looping), disconnect, delete (joining around the deleted node), duplicate, group into a custom node, ungroup, expose an input, prune, lay out |
| `def` | Custom nodes: a graph behind one node with ports of its own, nested and reused, and a fingerprint that tells a copy from the library's |
| `eval` | A pull evaluator: each node reached is computed once; bypassed and failing nodes pass their data through; custom nodes are evaluated inside; loops and recursion are reported per node |
| `expr`, `field` | Number expressions (`30*i + 5*sin(i)`) and values per element (Blender's fields) |
| `scope` | Finds the graph inside nested custom nodes |
| `library` (feature `library`, on by default) | Custom nodes kept as files in a folder |
| `editor` (feature `egui`) | The node editor widget |

The crate has no language of its own. Errors are values (`ConnectError`, `eval::Problem`, …)
with English `Display`s, and every label comes from your code.

## Describing your nodes

```rust
use flow_nodes_rs::{GroupRef, InDecl, NodeKind, OutDecl, Role, SocketType};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
struct Num;
impl SocketType for Num {
    fn has_value(self) -> bool { true }      // an unwired input keeps a value
}

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
enum Calc {
    Number(f64),
    Add { a: f64, b: f64 },
    // The custom-node kinds are your variants too, so graphs save as your enum does.
    Group(GroupRef<f64>),
    GroupInput,
    GroupOutput,
    Result,
}

impl NodeKind for Calc {
    type In = In;        // your input keys
    type Out = Out;      // your output keys
    type Socket = Num;
    type Value = f64;    // what an unwired input holds
    fn role(&self) -> Role<'_, f64> { /* Group / GroupInput / GroupOutput / Output / Plain */ }
    fn own_inputs(&self) -> Vec<InDecl<In, Num>> { /* the sockets of your plain kinds */ }
    // … group(), port_in(), inline(), set_inline(), …
}
```

The crate never matches on your kinds. Adding a kind means adding a variant and its match arms;
the crate works out a custom node's sockets from its ports.

Then implement `eval::Evaluate<Env>` (`compute`, `default_data`, `conform`) to compute your nodes in
an environment of your choosing, and call `eval::evaluate(&graph, &defs, &env)`.

[`examples/common/calc.rs`](examples/common/calc.rs) is a complete calculator domain, about 150 lines.

## Examples

```bash
cargo run -p flow_nodes_rs --example calc                    # build, group, save, evaluate
cargo run -p flow_nodes_rs --example editor --features egui  # the same nodes in the editor
```

## The editor

`editor::NodeEditor::show(ui, &viewer, &mut state)` draws the graph your `editor::Viewer`
describes and returns the `editor::Action`s the user made. It never changes the graph itself:
you apply the actions, so undo stays yours. Colours, sizes, words and keys come in through
`EditorStyle`, `Labels` and `KeyMap`; the defaults are a dark look, English and Blender's keys.

## Extending

- **New kinds of node.** Add variants to your enum and implement their sockets and `compute`.
- **New data types.** Add a socket type. `SocketType::feeds` decides which conversions a wire
  allows, and `Evaluate::conform` carries them out.
- **Nodes made by users.** Custom nodes, expressions and the library work for any kind of node
  without extra code.
- **Another look, language or key map.** Pass your own `EditorStyle`, `Labels` and `KeyMap`.

## License

BSD-3-Clause. See [LICENSE](LICENSE).
