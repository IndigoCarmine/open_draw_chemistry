//! Expressions typed into nodes: `30*i + 5*sin(i)`, `layer % 2 == 0 && z > 0`.
//!
//! Hand-written (no dependency): a precedence-climbing parser into a small tree whose
//! variables are resolved to slots once, so evaluating it for every copy is a slice lookup.
//!
//! - Numbers `3`, `2.5`, `1e-3`; constants `pi`, `e`.
//! - `+ - * / % ^` (`%` is the remainder with the sign of the divisor, `^` the power, right to
//!   left), unary `-` and `!`.
//! - Comparisons `< <= > >= == !=` and `&& ||` give 1 or 0; `c ? a : b`.
//! - Functions: `sin cos tan asin acos atan` (radians), `sind cosd tand` (degrees),
//!   `atan2 sqrt abs floor ceil round min max pow rad deg sign clamp`.

use std::fmt;

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    root: Node,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Num(f64),
    Var(usize),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    If(Box<Node>, Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sind,
    Cosd,
    Tand,
    Atan2,
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Round,
    Min,
    Max,
    Pow,
    Rad,
    Deg,
    Sign,
    Clamp,
}

impl Func {
    fn of(name: &str) -> Option<(Func, usize)> {
        use Func::*;
        Some(match name {
            "sin" => (Sin, 1),
            "cos" => (Cos, 1),
            "tan" => (Tan, 1),
            "asin" => (Asin, 1),
            "acos" => (Acos, 1),
            "atan" => (Atan, 1),
            "sind" => (Sind, 1),
            "cosd" => (Cosd, 1),
            "tand" => (Tand, 1),
            "atan2" => (Atan2, 2),
            "sqrt" => (Sqrt, 1),
            "abs" => (Abs, 1),
            "floor" => (Floor, 1),
            "ceil" => (Ceil, 1),
            "round" => (Round, 1),
            "min" => (Min, 2),
            "max" => (Max, 2),
            "pow" => (Pow, 2),
            "rad" => (Rad, 1),
            "deg" => (Deg, 1),
            "sign" => (Sign, 1),
            "clamp" => (Clamp, 3),
            _ => return None,
        })
    }
}

/// Why an expression could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    /// Where, in characters from the start.
    pub at: usize,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    Empty,
    Unexpected(String),
    UnknownName(String),
    UnknownFunction(String),
    /// A function called with the wrong number of arguments: the name and how many it takes.
    Arity(String, usize),
    Unclosed,
    Trailing,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = self.at + 1;
        let text = match &self.kind {
            ErrorKind::Empty => t!("assembly.expr.error.empty").to_owned(),
            ErrorKind::Unexpected(s) => t!("assembly.expr.error.unexpected", text = s, at = at),
            ErrorKind::UnknownName(s) => t!("assembly.expr.error.unknown_name", name = s),
            ErrorKind::UnknownFunction(s) => {
                t!("assembly.expr.error.unknown_function", name = s)
            }
            ErrorKind::Arity(s, n) => t!("assembly.expr.error.arity", name = s, count = n),
            ErrorKind::Unclosed => t!("assembly.expr.error.unclosed", at = at),
            ErrorKind::Trailing => t!("assembly.expr.error.trailing", at = at),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for ExprError {}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Sym(&'static str),
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, ExprError> {
    const SYMS: [&str; 22] = [
        "<=", ">=", "==", "!=", "&&", "||", "**", "+", "-", "*", "/", "%", "^", "<", ">", "!", "(",
        ")", ",", "?", ":", "=",
    ];
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            let value = text.parse::<f64>().map_err(|_| ExprError {
                at: start,
                kind: ErrorKind::Unexpected(text.clone()),
            })?;
            out.push((Tok::Num(value), start));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push((Tok::Name(chars[start..i].iter().collect()), start));
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
        match SYMS.iter().find(|s| rest.starts_with(**s)) {
            Some(&sym) => {
                // `=` alone is taken as `==`, `**` as `^`.
                let tok = match sym {
                    "=" => "==",
                    "**" => "^",
                    s => s,
                };
                out.push((Tok::Sym(tok), i));
                i += sym.len();
            }
            None => {
                return Err(ExprError {
                    at: i,
                    kind: ErrorKind::Unexpected(c.to_string()),
                });
            }
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    end: usize,
    resolve: &'a dyn Fn(&str) -> Option<usize>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    fn at(&self) -> usize {
        self.toks.get(self.pos).map_or(self.end, |(_, at)| *at)
    }

    fn eat(&mut self, sym: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(s)) if *s == sym) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn unexpected(&self) -> ExprError {
        let kind = match self.toks.get(self.pos) {
            None => ErrorKind::Unclosed,
            Some((Tok::Num(v), _)) => ErrorKind::Unexpected(v.to_string()),
            Some((Tok::Name(n), _)) => ErrorKind::Unexpected(n.clone()),
            Some((Tok::Sym(s), _)) => ErrorKind::Unexpected((*s).to_owned()),
        };
        ExprError {
            at: self.at(),
            kind,
        }
    }

    /// `a ? b : c`, the loosest.
    fn ternary(&mut self) -> Result<Node, ExprError> {
        let cond = self.binary(0)?;
        if self.eat("?") {
            let a = self.ternary()?;
            if !self.eat(":") {
                return Err(self.unexpected());
            }
            let b = self.ternary()?;
            return Ok(Node::If(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }

    fn binary(&mut self, min: u8) -> Result<Node, ExprError> {
        let mut left = self.unary()?;
        while let Some(Tok::Sym(sym)) = self.peek() {
            let Some((op, prec, right_assoc)) = binop(sym) else {
                break;
            };
            if prec < min {
                break;
            }
            self.pos += 1;
            let next = if right_assoc { prec } else { prec + 1 };
            let right = self.binary(next)?;
            left = Node::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Node, ExprError> {
        if self.eat("-") {
            return Ok(Node::Neg(Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok(Node::Not(Box::new(self.unary()?)));
        }
        self.power()
    }

    /// `^` binds tighter than unary minus on its left (`-2^2` is −4) and takes a unary on its
    /// right (`2^-1`).
    fn power(&mut self) -> Result<Node, ExprError> {
        let base = self.atom()?;
        if self.eat("^") {
            let exp = self.unary()?;
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Node, ExprError> {
        let at = self.at();
        match self.peek().cloned() {
            Some(Tok::Num(v)) => {
                self.pos += 1;
                Ok(Node::Num(v))
            }
            Some(Tok::Sym("(")) => {
                self.pos += 1;
                let inner = self.ternary()?;
                if !self.eat(")") {
                    return Err(ExprError {
                        at: self.at(),
                        kind: ErrorKind::Unclosed,
                    });
                }
                Ok(inner)
            }
            Some(Tok::Name(name)) => {
                self.pos += 1;
                if self.eat("(") {
                    let Some((func, arity)) = Func::of(&name) else {
                        return Err(ExprError {
                            at,
                            kind: ErrorKind::UnknownFunction(name),
                        });
                    };
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.ternary()?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return Err(self.unexpected());
                            }
                        }
                    }
                    if args.len() != arity {
                        return Err(ExprError {
                            at,
                            kind: ErrorKind::Arity(name, arity),
                        });
                    }
                    return Ok(Node::Call(func, args));
                }
                if let Some(slot) = (self.resolve)(&name) {
                    return Ok(Node::Var(slot));
                }
                match name.as_str() {
                    "pi" | "PI" => Ok(Node::Num(std::f64::consts::PI)),
                    "e" => Ok(Node::Num(std::f64::consts::E)),
                    "true" => Ok(Node::Num(1.0)),
                    "false" => Ok(Node::Num(0.0)),
                    _ => Err(ExprError {
                        at,
                        kind: ErrorKind::UnknownName(name),
                    }),
                }
            }
            _ => Err(self.unexpected()),
        }
    }
}

fn binop(sym: &str) -> Option<(Op, u8, bool)> {
    Some(match sym {
        "||" => (Op::Or, 1, false),
        "&&" => (Op::And, 2, false),
        "==" => (Op::Eq, 3, false),
        "!=" => (Op::Ne, 3, false),
        "<" => (Op::Lt, 4, false),
        "<=" => (Op::Le, 4, false),
        ">" => (Op::Gt, 4, false),
        ">=" => (Op::Ge, 4, false),
        "+" => (Op::Add, 5, false),
        "-" => (Op::Sub, 5, false),
        "*" => (Op::Mul, 6, false),
        "/" => (Op::Div, 6, false),
        "%" => (Op::Rem, 6, false),
        _ => return None,
    })
}

impl Expr {
    /// Parse `src`, each name looked up by `resolve` to a slot of the values [`Expr::eval`]
    /// is given (names it does not know are the constants, or an error).
    pub fn parse(src: &str, resolve: &dyn Fn(&str) -> Option<usize>) -> Result<Expr, ExprError> {
        let toks = lex(src)?;
        if toks.is_empty() {
            return Err(ExprError {
                at: 0,
                kind: ErrorKind::Empty,
            });
        }
        let mut p = Parser {
            toks,
            pos: 0,
            end: src.chars().count(),
            resolve,
        };
        let root = p.ternary()?;
        if p.pos < p.toks.len() {
            return Err(ExprError {
                at: p.at(),
                kind: ErrorKind::Trailing,
            });
        }
        Ok(Expr { root })
    }

    /// The names `src` uses outside function calls (constants such as `pi` included), in order
    /// of first use; empty if it does not parse.
    pub fn names(src: &str) -> Vec<String> {
        let names = std::cell::RefCell::new(Vec::<String>::new());
        let resolve = |name: &str| {
            let mut names = names.borrow_mut();
            Some(match names.iter().position(|n| n == name) {
                Some(i) => i,
                None => {
                    names.push(name.to_owned());
                    names.len() - 1
                }
            })
        };
        match Expr::parse(src, &resolve) {
            Ok(_) => names.into_inner(),
            Err(_) => Vec::new(),
        }
    }

    /// The value with the variables' slots holding `vars`.
    pub fn eval(&self, vars: &[f64]) -> f64 {
        eval(&self.root, vars)
    }

    /// Whether the expression is a plain number (no variables).
    pub fn constant(&self) -> Option<f64> {
        fn has_var(n: &Node) -> bool {
            match n {
                Node::Num(_) => false,
                Node::Var(_) => true,
                Node::Neg(a) | Node::Not(a) => has_var(a),
                Node::Bin(_, a, b) => has_var(a) || has_var(b),
                Node::If(a, b, c) => has_var(a) || has_var(b) || has_var(c),
                Node::Call(_, args) => args.iter().any(has_var),
            }
        }
        (!has_var(&self.root)).then(|| self.eval(&[]))
    }
}

/// A number typed as an expression without variables (`360/7`, `2*pi`), for number fields.
pub fn eval_const(src: &str) -> Option<f64> {
    Expr::parse(src, &|_| None).ok()?.constant()
}

fn truth(v: f64) -> bool {
    v != 0.0 && !v.is_nan()
}

fn bool_num(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

fn eval(node: &Node, vars: &[f64]) -> f64 {
    match node {
        Node::Num(v) => *v,
        Node::Var(slot) => vars.get(*slot).copied().unwrap_or(f64::NAN),
        Node::Neg(a) => -eval(a, vars),
        Node::Not(a) => bool_num(!truth(eval(a, vars))),
        Node::If(c, a, b) => {
            if truth(eval(c, vars)) {
                eval(a, vars)
            } else {
                eval(b, vars)
            }
        }
        Node::Bin(op, a, b) => {
            let x = eval(a, vars);
            // Short-circuit, so `n > 0 && 1/n > 2` is safe.
            match op {
                Op::And => return bool_num(truth(x) && truth(eval(b, vars))),
                Op::Or => return bool_num(truth(x) || truth(eval(b, vars))),
                _ => {}
            }
            let y = eval(b, vars);
            match op {
                Op::Add => x + y,
                Op::Sub => x - y,
                Op::Mul => x * y,
                Op::Div => x / y,
                Op::Rem => x - y * (x / y).floor(),
                Op::Pow => x.powf(y),
                Op::Lt => bool_num(x < y),
                Op::Le => bool_num(x <= y),
                Op::Gt => bool_num(x > y),
                Op::Ge => bool_num(x >= y),
                // Copy numbers and attributes are whole: compare with a little slack for the
                // ones computed (0.1 + 0.2 == 0.3).
                Op::Eq => bool_num((x - y).abs() <= 1e-9 * (1.0 + x.abs().max(y.abs()))),
                Op::Ne => bool_num((x - y).abs() > 1e-9 * (1.0 + x.abs().max(y.abs()))),
                Op::And | Op::Or => unreachable!("handled above"),
            }
        }
        Node::Call(f, args) => {
            let a = |k: usize| eval(&args[k], vars);
            use Func::*;
            match f {
                Sin => a(0).sin(),
                Cos => a(0).cos(),
                Tan => a(0).tan(),
                Asin => a(0).asin(),
                Acos => a(0).acos(),
                Atan => a(0).atan(),
                Sind => a(0).to_radians().sin(),
                Cosd => a(0).to_radians().cos(),
                Tand => a(0).to_radians().tan(),
                Atan2 => a(0).atan2(a(1)),
                Sqrt => a(0).sqrt(),
                Abs => a(0).abs(),
                Floor => a(0).floor(),
                Ceil => a(0).ceil(),
                Round => a(0).round(),
                Min => a(0).min(a(1)),
                Max => a(0).max(a(1)),
                Pow => a(0).powf(a(1)),
                Rad => a(0).to_radians(),
                Deg => a(0).to_degrees(),
                Sign => {
                    let v = a(0);
                    if v > 0.0 {
                        1.0
                    } else if v < 0.0 {
                        -1.0
                    } else {
                        0.0
                    }
                }
                Clamp => a(0).max(a(1)).min(a(2)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(src: &str, names: &[(&str, f64)]) -> f64 {
        let resolve = |n: &str| names.iter().position(|(m, _)| *m == n);
        let values: Vec<f64> = names.iter().map(|(_, v)| *v).collect();
        Expr::parse(src, &resolve)
            .unwrap_or_else(|e| panic!("{src}: {e:?}"))
            .eval(&values)
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(value("1 + 2 * 3", &[]), 7.0);
        assert_eq!(value("(1 + 2) * 3", &[]), 9.0);
        assert_eq!(value("2 ^ 3 ^ 2", &[]), 512.0);
        assert_eq!(value("-2 ^ 2", &[]), -4.0);
        assert_eq!(value("2 ^ -1", &[]), 0.5);
        assert_eq!(value("10 - 4 - 3", &[]), 3.0);
        assert_eq!(value("7 % 3", &[]), 1.0);
        assert_eq!(value("-1 % 3", &[]), 2.0, "the sign of the divisor");
        assert_eq!(value("1 + 1 == 2 && 3 > 2", &[]), 1.0);
        assert_eq!(value("1 < 0 || 0", &[]), 0.0);
        assert_eq!(value("!0", &[]), 1.0);
        assert_eq!(value("1 ? 2 : 3", &[]), 2.0);
        assert_eq!(value("0 ? 2 : 0 ? 3 : 4", &[]), 4.0);
        assert_eq!(value("0.1 + 0.2 == 0.3", &[]), 1.0);
        assert_eq!(value("2 ** 3", &[]), 8.0);
        assert_eq!(value("1e-3 * 1E3", &[]), 1.0);
    }

    #[test]
    fn variables_functions_and_constants() {
        let v = value("30*i + 5*sin(i)", &[("i", 2.0)]);
        assert!((v - (60.0 + 5.0 * 2f64.sin())).abs() < 1e-12);
        assert!((value("sind(30)", &[]) - 0.5).abs() < 1e-12);
        assert!((value("2*pi", &[]) - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(
            value("max(a, b) - min(a, b)", &[("a", 3.0), ("b", 5.0)]),
            2.0
        );
        assert_eq!(value("clamp(9, 0, 4)", &[]), 4.0);
        assert_eq!(value("layer % 2 == 0", &[("layer", 4.0)]), 1.0);
        assert_eq!(
            value("e", &[("e", 3.0)]),
            3.0,
            "a variable hides a constant"
        );
    }

    #[test]
    fn errors_name_the_place() {
        let err = |src: &str| Expr::parse(src, &|n| (n == "i").then_some(0)).unwrap_err();
        assert_eq!(err("").kind, ErrorKind::Empty);
        assert_eq!(err("1 + q").kind, ErrorKind::UnknownName("q".into()));
        assert_eq!(err("1 + q").at, 4);
        assert_eq!(err("foo(1)").kind, ErrorKind::UnknownFunction("foo".into()));
        assert_eq!(err("min(1)").kind, ErrorKind::Arity("min".into(), 2));
        assert_eq!(err("(1 + 2").kind, ErrorKind::Unclosed);
        assert_eq!(err("1 2").kind, ErrorKind::Trailing);
    }

    #[test]
    fn constants_and_names() {
        assert_eq!(eval_const("360/7"), Some(360.0 / 7.0));
        assert_eq!(eval_const("i"), None);
        assert_eq!(eval_const("1 +"), None);
        assert_eq!(Expr::names("a*b + sin(a) + pi"), vec!["a", "b", "pi"]);
    }

    #[test]
    fn nan_and_division_by_zero_stay_numbers() {
        assert!(value("1/0", &[]).is_infinite());
        assert!(value("sqrt(-1)", &[]).is_nan());
        assert_eq!(value("0 && 1/0", &[]), 0.0);
    }
}
