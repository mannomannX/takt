//! Terme der Kodierung: Bitvektoren fester Breite fuer Ganzzahlen, Enums
//! und Dauern, `Float32`/`Float64` der SMT-FP-Theorie fuer `float`, `Bool`
//! (plan/m6.md 2.8). Ein Term ist ein geteilter Graph (`Rc`): Die Mischung
//! der Zweige (`ite`) verweist auf dieselben Teilterme, und der Drucker
//! benennt jeden Knoten einmal.

use std::rc::Rc;

/// Sorte eines Terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// `Bool`
    Bool,
    /// `(_ BitVec 64)`, vorzeichenbehaftet gerechnet (i64 wie der Interpreter).
    Int,
    /// `Float32`
    F32,
    /// `Float64`
    F64,
}

/// Operation eines inneren Knotens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Op {
    Not,
    And,
    Or,
    Eq,
    Ite,
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Le,
    Gt,
    Ge,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    FNeg,
    FAdd,
    FSub,
    FMul,
    FDiv,
    FLt,
    FLe,
    FGt,
    FGe,
    FEq,
    FAbs,
    FSqrt,
    FFma,
    /// Vorzeichenbehaftete Ganzzahl nach `Float32` (RNE).
    ToF32,
    /// Vorzeichenbehaftete Ganzzahl nach `Float64` (RNE).
    ToF64,
    /// Weder NaN noch unendlich (4.1).
    IsFinite,
}

/// Ein Knoten.
#[derive(Debug)]
pub enum Node {
    /// Konstante.
    Bool(bool),
    /// Konstante.
    Int(i64),
    /// Konstante.
    F32(f32),
    /// Konstante.
    F64(f64),
    /// Variable: Zustand (`s.…`) oder Eingabe (`i.…`) des Schritts.
    Var(String, Sort),
    /// Anwendung.
    App(Op, Vec<Term>),
}

/// Ein geteilter Term.
#[derive(Clone, Debug)]
pub struct Term(pub Rc<Node>);

impl std::ops::Not for Term {
    type Output = Term;

    fn not(self) -> Term {
        Term::app(Op::Not, vec![self])
    }
}

impl Term {
    fn new(n: Node) -> Term {
        Term(Rc::new(n))
    }

    /// Konstante.
    pub fn bool(b: bool) -> Term {
        Term::new(Node::Bool(b))
    }

    /// Konstante.
    pub fn int(i: i64) -> Term {
        Term::new(Node::Int(i))
    }

    /// Konstante in der Breite.
    pub fn float(v: f64, sort: Sort) -> Term {
        match sort {
            Sort::F32 => Term::new(Node::F32(v as f32)),
            _ => Term::new(Node::F64(v)),
        }
    }

    /// Variable.
    pub fn var(name: impl Into<String>, sort: Sort) -> Term {
        Term::new(Node::Var(name.into(), sort))
    }

    /// Anwendung mit leichter Faltung von Konstanten.
    pub fn app(op: Op, args: Vec<Term>) -> Term {
        match (op, args.as_slice()) {
            (Op::Not, [a]) => match &*a.0 {
                Node::Bool(b) => return Term::bool(!b),
                Node::App(Op::Not, inner) => return inner[0].clone(),
                _ => {}
            },
            (Op::Ite, [c, a, b]) => match &*c.0 {
                Node::Bool(true) => return a.clone(),
                Node::Bool(false) => return b.clone(),
                _ => {
                    if Rc::ptr_eq(&a.0, &b.0) {
                        return a.clone();
                    }
                }
            },
            (Op::Eq, [a, b]) => match (&*a.0, &*b.0) {
                (Node::Int(x), Node::Int(y)) => return Term::bool(x == y),
                (Node::Bool(x), Node::Bool(y)) => return Term::bool(x == y),
                _ => {}
            },
            _ => {}
        }
        Term::new(Node::App(op, args))
    }

    /// `and`, geglaettet; `true` faellt weg, `false` gewinnt.
    pub fn and(args: Vec<Term>) -> Term {
        let mut out = Vec::new();
        for a in args {
            match &*a.0 {
                Node::Bool(true) => {}
                Node::Bool(false) => return Term::bool(false),
                Node::App(Op::And, inner) => out.extend(inner.iter().cloned()),
                _ => out.push(a),
            }
        }
        match out.len() {
            0 => Term::bool(true),
            1 => out.pop().expect("ein Term"),
            _ => Term::new(Node::App(Op::And, out)),
        }
    }

    /// `or`, geglaettet; `false` faellt weg, `true` gewinnt.
    pub fn or(args: Vec<Term>) -> Term {
        let mut out = Vec::new();
        for a in args {
            match &*a.0 {
                Node::Bool(false) => {}
                Node::Bool(true) => return Term::bool(true),
                Node::App(Op::Or, inner) => out.extend(inner.iter().cloned()),
                _ => out.push(a),
            }
        }
        match out.len() {
            0 => Term::bool(false),
            1 => out.pop().expect("ein Term"),
            _ => Term::new(Node::App(Op::Or, out)),
        }
    }

    /// `ite`.
    pub fn ite(c: Term, a: Term, b: Term) -> Term {
        Term::app(Op::Ite, vec![c, a, b])
    }

    /// `=`.
    pub fn eq(a: Term, b: Term) -> Term {
        Term::app(Op::Eq, vec![a, b])
    }

    /// Zweistellige Anwendung.
    pub fn bin(op: Op, a: Term, b: Term) -> Term {
        Term::app(op, vec![a, b])
    }

    /// Ist der Term dieselbe Konstante?
    pub fn is_bool(&self, b: bool) -> bool {
        matches!(&*self.0, Node::Bool(x) if *x == b)
    }

    /// Die Sorte.
    pub fn sort(&self) -> Sort {
        match &*self.0 {
            Node::Bool(_) => Sort::Bool,
            Node::Int(_) => Sort::Int,
            Node::F32(_) => Sort::F32,
            Node::F64(_) => Sort::F64,
            Node::Var(_, s) => *s,
            Node::App(op, args) => match op {
                Op::Not
                | Op::And
                | Op::Or
                | Op::Eq
                | Op::Lt
                | Op::Le
                | Op::Gt
                | Op::Ge
                | Op::FLt
                | Op::FLe
                | Op::FGt
                | Op::FGe
                | Op::FEq
                | Op::IsFinite => Sort::Bool,
                Op::ToF32 => Sort::F32,
                Op::ToF64 => Sort::F64,
                Op::Ite => args[1].sort(),
                Op::Neg
                | Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Div
                | Op::Rem
                | Op::BitAnd
                | Op::BitOr
                | Op::BitXor
                | Op::Shl
                | Op::Shr => Sort::Int,
                Op::FNeg | Op::FAdd | Op::FSub | Op::FMul | Op::FDiv | Op::FAbs | Op::FSqrt | Op::FFma => {
                    args[0].sort()
                }
            },
        }
    }
}
