//! A calculator's nodes: numbers in, sums and products, one number out. All a kind of node
//! needs to be drawn, edited, grouped into custom nodes and evaluated by `flow_nodes_rs`.

use flow_nodes_rs::eval::{EvalCx, Evaluate, OutputsOf};
use flow_nodes_rs::{GroupRef, InDecl, NodeKind, OutDecl, Role, SocketType};
use serde::{Deserialize, Serialize};

/// One type of data: a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Num;

impl SocketType for Num {
    fn has_value(self) -> bool {
        true
    }

    fn is_primary(self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum In {
    A,
    B,
    Port(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Out {
    Value,
    Port(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Calc {
    Number(f64),
    Add { a: f64, b: f64 },
    Multiply { a: f64, b: f64 },
    Group(GroupRef<f64>),
    GroupInput,
    GroupOutput,
    Result,
}

impl Calc {
    #[allow(dead_code)] // used by the editor example only
    pub fn title(&self) -> &'static str {
        match self {
            Calc::Number(_) => "Number",
            Calc::Add { .. } => "Add",
            Calc::Multiply { .. } => "Multiply",
            Calc::Group(_) => "Custom",
            Calc::GroupInput => "Group input",
            Calc::GroupOutput => "Group output",
            Calc::Result => "Result",
        }
    }
}

impl NodeKind for Calc {
    type In = In;
    type Out = Out;
    type Socket = Num;
    type Value = f64;

    fn role(&self) -> Role<'_, f64> {
        match self {
            Calc::Group(g) => Role::Group(g),
            Calc::GroupInput => Role::GroupInput,
            Calc::GroupOutput => Role::GroupOutput,
            Calc::Result => Role::Output,
            _ => Role::Plain,
        }
    }

    fn group(r: GroupRef<f64>) -> Self {
        Calc::Group(r)
    }

    fn group_input() -> Self {
        Calc::GroupInput
    }

    fn group_output() -> Self {
        Calc::GroupOutput
    }

    fn port_in(key: u32) -> In {
        In::Port(key)
    }

    fn as_port_in(key: In) -> Option<u32> {
        match key {
            In::Port(k) => Some(k),
            _ => None,
        }
    }

    fn port_out(key: u32) -> Out {
        Out::Port(key)
    }

    fn as_port_out(key: Out) -> Option<u32> {
        match key {
            Out::Port(k) => Some(k),
            _ => None,
        }
    }

    fn own_inputs(&self) -> Vec<InDecl<In, Num>> {
        match self {
            Calc::Add { .. } | Calc::Multiply { .. } => {
                vec![InDecl::new(In::A, Num, "a"), InDecl::new(In::B, Num, "b")]
            }
            Calc::Result => vec![InDecl::new(In::A, Num, "value")],
            _ => Vec::new(),
        }
    }

    fn own_outputs(&self) -> Vec<OutDecl<Out, Num>> {
        match self {
            Calc::Number(_) | Calc::Add { .. } | Calc::Multiply { .. } => {
                vec![OutDecl::new(Out::Value, Num, "value")]
            }
            _ => Vec::new(),
        }
    }

    fn inline(&self, key: In) -> Option<f64> {
        match (self, key) {
            (Calc::Add { a, .. } | Calc::Multiply { a, .. }, In::A) => Some(*a),
            (Calc::Add { b, .. } | Calc::Multiply { b, .. }, In::B) => Some(*b),
            (Calc::Group(g), In::Port(p)) => g.inputs.get(&p).copied(),
            _ => None,
        }
    }

    fn set_inline(&mut self, key: In, value: f64) -> bool {
        match (self, key) {
            (Calc::Add { a, .. } | Calc::Multiply { a, .. }, In::A) => *a = value,
            (Calc::Add { b, .. } | Calc::Multiply { b, .. }, In::B) => *b = value,
            (Calc::Group(g), In::Port(p)) => {
                g.inputs.insert(p, value);
            }
            _ => return false,
        }
        true
    }
}

/// Nothing outside the graph is needed.
pub struct NoEnv;

impl Evaluate<NoEnv> for Calc {
    type Data = f64;
    type Error = String;

    fn compute(
        &self,
        cx: &mut EvalCx<'_, '_, Self, NoEnv>,
    ) -> Result<OutputsOf<Self, NoEnv>, String> {
        let input = |cx: &mut EvalCx<'_, '_, Self, NoEnv>, key| {
            cx.input(key).or(self.inline(key)).unwrap_or(0.0)
        };
        let value = match self {
            Calc::Number(v) => *v,
            Calc::Add { .. } => input(cx, In::A) + input(cx, In::B),
            Calc::Multiply { .. } => input(cx, In::A) * input(cx, In::B),
            _ => return Ok(Vec::new()),
        };
        if !value.is_finite() {
            return Err("not a number".into());
        }
        Ok(vec![(Out::Value, value)])
    }

    fn default_data(_: Num, value: Option<&f64>) -> f64 {
        value.copied().unwrap_or(0.0)
    }

    fn conform(data: f64, _: Num) -> Option<f64> {
        Some(data)
    }
}
