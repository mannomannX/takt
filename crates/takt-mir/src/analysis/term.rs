//! Terme fuer Differenzschranken (Referenz 3.4, der Differenzanteil der
//! Oktagone): die Form eines Ausdrucks, dessen Wert sich nur durch eine
//! Zuweisung an eine seiner Variablen aendert.

use crate::expr::{Accessor, BinaryOp, Expr, ExprKind};
use crate::stmt::Place;
use crate::types::Type;
use crate::{TypeId, VarId};

/// Variable, Feld oder Laenge davon.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Term {
    /// `v`
    Var(u32),
    /// `t.f`
    Field(Box<Term>, u32),
    /// `t.len`
    Len(Box<Term>),
}

impl Term {
    /// Nennt der Term die Variable?
    pub fn mentions(&self, v: VarId) -> bool {
        match self {
            Term::Var(x) => *x == v.0,
            Term::Field(b, _) | Term::Len(b) => b.mentions(v),
        }
    }

    /// Die Stelle als Term.
    pub fn of_place(p: &Place) -> Option<Term> {
        match p {
            Place::Var(v) => Some(Term::Var(v.0)),
            Place::Field(b, i) => Some(Term::Field(Box::new(Term::of_place(b)?), *i)),
            _ => None,
        }
    }

    /// Die Wurzelvariable einer Stelle.
    pub fn root(p: &Place) -> Option<VarId> {
        match p {
            Place::Var(v) => Some(*v),
            Place::Field(b, _) | Place::Index(b, _) | Place::Index2(b, ..) => Term::root(b),
            _ => None,
        }
    }
}

/// `term + c`, oder nur `c`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lin {
    /// Der Term.
    pub term: Option<Term>,
    /// Die Konstante.
    pub c: i128,
}

/// Die lineare Form eines Ausdrucks. Casts zwischen Ganzzahlen sind
/// wertgleich oder faulten; ein bestandener Pruefknoten ist eine Funktion
/// seines Operanden.
pub fn lin(e: &Expr, types: &[Type]) -> Option<Lin> {
    let integral = |t: TypeId| matches!(types.get(t.index()), Some(Type::Int { .. } | Type::Duration { .. }));
    let only = |t: Term| Some(Lin { term: Some(t), c: 0 });
    match &e.kind {
        ExprKind::Int(v) | ExprKind::Duration(v) => Some(Lin { term: None, c: i128::from(*v) }),
        ExprKind::Var(v) => only(Term::Var(v.0)),
        ExprKind::Field { base, field } => only(Term::Field(Box::new(term(base, types)?), *field)),
        ExprKind::Accessor { base, accessor: Accessor::Len, args } if args.is_empty() => {
            only(Term::Len(Box::new(term(base, types)?)))
        }
        ExprKind::Cast { expr, to } if integral(expr.ty) && integral(*to) => lin(expr, types),
        ExprKind::Checked { expr, .. } => lin(expr, types),
        ExprKind::Binary { op: op @ (BinaryOp::Add | BinaryOp::Sub), lhs, rhs } => {
            let (a, b) = (lin(lhs, types)?, lin(rhs, types)?);
            let sub = *op == BinaryOp::Sub;
            let c = if sub { a.c - b.c } else { a.c + b.c };
            match (a.term, b.term) {
                (t, None) => Some(Lin { term: t, c }),
                (None, t) if !sub => Some(Lin { term: t, c }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Der Term eines Ausdrucks ohne Konstante.
pub fn term(e: &Expr, types: &[Type]) -> Option<Term> {
    match lin(e, types)? {
        Lin { term: Some(t), c: 0 } => Some(t),
        _ => None,
    }
}
