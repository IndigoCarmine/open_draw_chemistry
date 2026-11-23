//! Expressions typed into nodes: `30*i + 5*sin(i)`, `layer % 2 == 0 && z > 0`. The parser is
//! the `flow_nodes_rs` crate's ([`flow_nodes_rs::expr`]); here its errors are worded in the UI
//! language.

pub use flow_nodes_rs::expr::{ErrorKind, Expr, ExprError, eval_const};

/// Why an expression could not be read, in the UI language.
pub fn message(e: &ExprError) -> String {
    let at = e.at + 1;
    match &e.kind {
        ErrorKind::Empty => t!("assembly.expr.error.empty").to_owned(),
        ErrorKind::Unexpected(s) => t!("assembly.expr.error.unexpected", text = s, at = at),
        ErrorKind::UnknownName(s) => t!("assembly.expr.error.unknown_name", name = s),
        ErrorKind::UnknownFunction(s) => {
            t!("assembly.expr.error.unknown_function", name = s)
        }
        ErrorKind::Arity(s, n) => t!("assembly.expr.error.arity", name = s, count = n),
        ErrorKind::Unclosed => t!("assembly.expr.error.unclosed", at = at),
        ErrorKind::Trailing => t!("assembly.expr.error.trailing", at = at),
        ErrorKind::Incomplete => t!("assembly.expr.error.incomplete").to_owned(),
    }
}
