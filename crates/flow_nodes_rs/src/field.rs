//! Fields: a number for each element of whatever uses it (each object, each point), evaluated
//! only once the element count is known — as Blender's geometry nodes do. A node that works
//! element by element asks a field for its [`Field::column`].

use std::rc::Rc;

use crate::expr::Expr;

/// A number for each element of whatever uses it.
#[derive(Debug, Clone)]
pub enum Field {
    Const(f64),
    /// The element's place, from 0.
    Index,
    /// An expression of other fields (its variables, in order).
    Expr(Rc<Expr>, Vec<Field>),
}

impl Field {
    /// The values for `len` elements.
    pub fn column(&self, len: usize) -> Vec<f64> {
        match self {
            Field::Const(v) => vec![*v; len],
            Field::Index => (0..len).map(|k| k as f64).collect(),
            Field::Expr(expr, args) => {
                let columns: Vec<Vec<f64>> = args.iter().map(|a| a.column(len)).collect();
                let mut values = vec![0.0; args.len()];
                (0..len)
                    .map(|k| {
                        for (v, c) in values.iter_mut().zip(&columns) {
                            *v = c[k];
                        }
                        expr.eval(&values)
                    })
                    .collect()
            }
        }
    }

    /// The one value every element has, if it is a constant.
    pub fn constant(&self) -> Option<f64> {
        match self {
            Field::Const(v) => Some(*v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns() {
        assert_eq!(Field::Const(2.0).column(3), vec![2.0; 3]);
        assert_eq!(Field::Index.column(3), vec![0.0, 1.0, 2.0]);
        let e = Expr::parse("a * 10 + b", &|n| ["a", "b"].iter().position(|m| *m == n)).unwrap();
        let f = Field::Expr(Rc::new(e), vec![Field::Index, Field::Const(1.0)]);
        assert_eq!(f.column(3), vec![1.0, 11.0, 21.0]);
        assert_eq!(f.constant(), None);
    }
}
