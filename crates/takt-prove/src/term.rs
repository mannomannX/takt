//! Terme der Kodierung: Bitvektoren fester Breite fuer Ganzzahlen, Enums
//! und Dauern, `Float32`/`Float64` der SMT-FP-Theorie fuer `float`, `Bool`
//! (plan/m6.md 2.8). Ein Term ist ein geteilter Graph (`Rc`): Die Mischung
//! der Zweige (`ite`) verweist auf dieselben Teilterme, und der Drucker
//! benennt jeden Knoten einmal.
//!
//! Ein ausgerollter Pfad ist tausende Knoten tief (FB-403); nichts, was
//! einen Term durchlaeuft oder abbaut, rekursiert darum ueber seine Tiefe:
//! [`post_order`] liefert die Knoten in der Reihenfolge, in der jeder nur
//! seine schon behandelten Argumente braucht.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

thread_local! {
    /// Die Knoten, die dieser Thread gebaut hat; der Kodierer misst daran
    /// die Groesse seines Modells (`encode::MODEL_LIMIT`).
    static BUILT: Cell<u64> = const { Cell::new(0) };
}

/// Wie viele Knoten dieser Thread bisher gebaut hat.
pub fn built() -> u64 {
    BUILT.with(Cell::get)
}

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

/// Eine korrekt gerundete Funktion aus `libtaktm` (4.2): in der Auswertung
/// genau, im Solver uninterpretiert, mit den Schranken ihres Wertebereichs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Fun {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Log,
    Pow,
}

impl Fun {
    /// Der Name im Solver.
    pub fn name(self) -> &'static str {
        match self {
            Fun::Sin => "sin",
            Fun::Cos => "cos",
            Fun::Tan => "tan",
            Fun::Asin => "asin",
            Fun::Acos => "acos",
            Fun::Atan => "atan",
            Fun::Atan2 => "atan2",
            Fun::Exp => "exp",
            Fun::Log => "log",
            Fun::Pow => "pow",
        }
    }
}

/// Die Richtung einer Rundung auf eine ganze Zahl.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Rounding {
    /// Halbe von null weg (`f64::round`).
    HalfAway,
    Down,
    Up,
    TowardZero,
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
    /// Vorzeichenbehaftete Ganzzahl oder Fliesskommazahl nach `Float32` (RNE).
    ToF32,
    /// Vorzeichenbehaftete Ganzzahl oder Fliesskommazahl nach `Float64` (RNE).
    ToF64,
    /// Weder NaN noch unendlich (4.1).
    IsFinite,
    /// Die unteren `bits` Bits, mit oder ohne Vorzeichen erweitert: die
    /// Zweierkomplement-Wickelung in eine Breite (`<<`, 3.10).
    Wrap {
        /// Breite.
        bits: u32,
        /// Vorzeichenbehaftet?
        signed: bool,
    },
    /// Die exakte Summe zweier `i64` liegt ausserhalb von `i64` (4.1).
    AddOverflows,
    /// Die exakte Differenz.
    SubOverflows,
    /// Das exakte Produkt.
    MulOverflows,
    /// `x · num / den`, einmal gerundet (`libtaktm::scale`, 3.2).
    Scale {
        num: u128,
        den: u128,
    },
    /// Eine Funktion aus `libtaktm`.
    Math(Fun),
    /// Teil `part` des Ergebnisses einer Native der kuratierten Menge (4.5),
    /// eine Zahl aus `bits` Bit ohne Vorzeichen. Die Argumente sind Bloecke
    /// `[Kapazitaet, Laenge, Byte …]`, so dass die Auswertung sie ohne den
    /// Typ in die Bytes der Grenze zerlegt; der Solver sieht die Funktion
    /// uninterpretiert.
    Native {
        f: takt_native::Native,
        part: u16,
        bits: u8,
    },
    /// Teil `part` des Ergebnisses einer Matrixfunktion aus `libtaktm::mat`
    /// (3.11) ueber einer `n`×`n`-Matrix und bei `solve` einer `n`×`k`
    /// rechten Seite, die Argumente ihre Elemente zeilenweise. Hinter den
    /// Elementen des Ergebnisses steht ein Wahrheitswert: singulaer bei `inv`
    /// und `solve`, zerlegbar bei `cholesky`. Der Solver sieht die Funktion
    /// uninterpretiert.
    Mat {
        f: MatFun,
        n: u8,
        k: u8,
        part: u16,
    },
    /// Auf eine ganze Zahl gerundet, in der Breite des Arguments.
    Round(Rounding),
    /// Eine ganzzahlige Fliesskommazahl als Ganzzahl.
    FloatToInt,
}

/// Eine Matrixfunktion aus `libtaktm::mat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum MatFun {
    Det,
    Inv,
    Solve,
    Cholesky,
}

impl MatFun {
    /// Wie das Programm sie nennt.
    pub fn name(self) -> &'static str {
        match self {
            MatFun::Det => "det",
            MatFun::Inv => "inv",
            MatFun::Solve => "solve",
            MatFun::Cholesky => "cholesky",
        }
    }

    /// Die Elemente des Ergebnisses; der Teil danach ist der Wahrheitswert.
    pub fn elements(self, n: u8, k: u8) -> u16 {
        let (n, k) = (u16::from(n), u16::from(k));
        match self {
            MatFun::Det => 1,
            MatFun::Inv | MatFun::Cholesky => n * n,
            MatFun::Solve => n * k,
        }
    }
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

/// Baut die Argumente ohne Rekursion ab: Ein Argument, das nur dieser
/// Knoten haelt, gibt seine eigenen an den Stapel weiter.
impl Drop for Node {
    fn drop(&mut self) {
        let Node::App(_, args) = self else { return };
        let mut stack = std::mem::take(args);
        while let Some(t) = stack.pop() {
            if let Ok(mut n) = Rc::try_unwrap(t.0)
                && let Node::App(_, inner) = &mut n
            {
                stack.append(inner);
            }
        }
    }
}

/// Ein geteilter Term.
#[derive(Clone, Debug)]
pub struct Term(pub Rc<Node>);

impl Term {
    /// Die Identitaet des Knotens im geteilten Graphen.
    pub fn key(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }
}

/// Die inneren Knoten unter `root`, jeder einmal und nach seinen
/// Argumenten; einen Knoten, den `known` schon kennt, laesst sie samt
/// seinem Graphen aus.
pub fn post_order(root: &Term, known: impl Fn(&Term) -> bool) -> Vec<Term> {
    let mut out = Vec::new();
    let mut expanded = HashSet::new();
    let mut stack = vec![(root.clone(), false)];
    while let Some((t, ready)) = stack.pop() {
        let Node::App(_, args) = &*t.0 else { continue };
        if ready {
            out.push(t);
            continue;
        }
        if known(&t) || !expanded.insert(t.key()) {
            continue;
        }
        stack.push((t.clone(), true));
        stack.extend(args.iter().rev().map(|a| (a.clone(), false)));
    }
    out
}

impl std::ops::Not for Term {
    type Output = Term;

    fn not(self) -> Term {
        Term::app(Op::Not, vec![self])
    }
}

impl Term {
    fn new(n: Node) -> Term {
        BUILT.with(|b| b.set(b.get() + 1));
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
            // Wie `eval`: Ganzzahlen wickeln in 64 Bit.
            (Op::Add | Op::Sub | Op::Lt | Op::Le | Op::Gt | Op::Ge, [a, b]) => {
                if let (Node::Int(x), Node::Int(y)) = (&*a.0, &*b.0) {
                    return match op {
                        Op::Add => Term::int(x.wrapping_add(*y)),
                        Op::Sub => Term::int(x.wrapping_sub(*y)),
                        Op::Lt => Term::bool(x < y),
                        Op::Le => Term::bool(x <= y),
                        Op::Gt => Term::bool(x > y),
                        _ => Term::bool(x >= y),
                    };
                }
            }
            _ => {}
        }
        Term::new(Node::App(op, args))
    }

    /// `and`; `true` faellt weg, `false` gewinnt. Ein inneres `and` bleibt
    /// ein geteilter Knoten: Geglaettet kaeme jede Bedingung eines Pfads in
    /// jede spaetere Konjunktion, und das Modell wuechse mit den Pruefungen
    /// eines Pfads quadratisch.
    pub fn and(args: Vec<Term>) -> Term {
        let mut out = Vec::new();
        for a in args {
            match &*a.0 {
                Node::Bool(true) => {}
                Node::Bool(false) => return Term::bool(false),
                _ => out.push(a),
            }
        }
        match out.len() {
            0 => Term::bool(true),
            1 => out.pop().expect("ein Term"),
            _ => Term::new(Node::App(Op::And, out)),
        }
    }

    /// `or`; `false` faellt weg, `true` gewinnt, ein inneres `or` bleibt
    /// geteilt wie bei [`Term::and`].
    pub fn or(args: Vec<Term>) -> Term {
        let mut out = Vec::new();
        for a in args {
            match &*a.0 {
                Node::Bool(false) => {}
                Node::Bool(true) => return Term::bool(true),
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

    /// Die Sorte; ein Zweig und ein Fliesskommaargument tragen sie weiter.
    pub fn sort(&self) -> Sort {
        let mut t = self;
        loop {
            t = match &*t.0 {
                Node::Bool(_) => return Sort::Bool,
                Node::Int(_) => return Sort::Int,
                Node::F32(_) => return Sort::F32,
                Node::F64(_) => return Sort::F64,
                Node::Var(_, s) => return *s,
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
                    | Op::IsFinite
                    | Op::AddOverflows
                    | Op::SubOverflows
                    | Op::MulOverflows => return Sort::Bool,
                    Op::ToF32 => return Sort::F32,
                    Op::ToF64 => return Sort::F64,
                    Op::Ite => &args[1],
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
                    | Op::Shr
                    | Op::Wrap { .. }
                    | Op::FloatToInt
                    | Op::Native { .. } => return Sort::Int,
                    Op::Mat { f, n, k, part } if *part == f.elements(*n, *k) => return Sort::Bool,
                    Op::FNeg
                    | Op::FAdd
                    | Op::FSub
                    | Op::FMul
                    | Op::FDiv
                    | Op::FAbs
                    | Op::FSqrt
                    | Op::FFma
                    | Op::Scale { .. }
                    | Op::Math(_)
                    | Op::Mat { .. }
                    | Op::Round(_) => &args[0],
                },
            };
        }
    }
}
