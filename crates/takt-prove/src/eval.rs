//! Konkrete Auswertung der Terme: dieselbe Semantik, die der Drucker nach
//! SMT-LIB2 traegt — i64 mit Rust-Division, IEEE-754 in der Breite. Damit
//! laesst sich die Kodierung ohne Solver gegen den Interpreter pruefen
//! (plan/m6.md 2.8): Ein Gegenbeispiel des Solvers oder ein Lauf des
//! Modells muss im Interpreter dasselbe tun.

use std::collections::BTreeMap;

use crate::term::{Node, Op, Sort, Term};

/// Ein konkreter Wert.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Val {
    /// `Bool`
    Bool(bool),
    /// Ganzzahl.
    Int(i64),
    /// `float` in f32.
    F32(f32),
    /// `float` in f64.
    F64(f64),
}

impl Val {
    /// Der Nullwert einer Sorte.
    pub fn zero(sort: Sort) -> Val {
        match sort {
            Sort::Bool => Val::Bool(false),
            Sort::Int => Val::Int(0),
            Sort::F32 => Val::F32(0.0),
            Sort::F64 => Val::F64(0.0),
        }
    }

    fn as_bool(self) -> bool {
        matches!(self, Val::Bool(true))
    }

    fn as_int(self) -> i64 {
        match self {
            Val::Int(i) => i,
            Val::Bool(b) => i64::from(b),
            Val::F32(f) => f as i64,
            Val::F64(f) => f as i64,
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Val::F64(f) => f,
            Val::F32(f) => f64::from(f),
            Val::Int(i) => i as f64,
            Val::Bool(b) => f64::from(u8::from(b)),
        }
    }
}

/// Werte der Variablen.
pub type Env = BTreeMap<String, Val>;

/// Wertet `t` unter `env`; eine fehlende Variable ist ihr Nullwert.
pub fn eval(t: &Term, env: &Env) -> Val {
    match &*t.0 {
        Node::Bool(b) => Val::Bool(*b),
        Node::Int(i) => Val::Int(*i),
        Node::F32(f) => Val::F32(*f),
        Node::F64(f) => Val::F64(*f),
        Node::Var(name, sort) => env.get(name).copied().unwrap_or(Val::zero(*sort)),
        Node::App(op, args) => {
            let a = |i: usize| eval(&args[i], env);
            match op {
                Op::Not => Val::Bool(!a(0).as_bool()),
                Op::And => Val::Bool(args.iter().all(|x| eval(x, env).as_bool())),
                Op::Or => Val::Bool(args.iter().any(|x| eval(x, env).as_bool())),
                Op::Eq => Val::Bool(same(a(0), a(1))),
                Op::Ite => {
                    if a(0).as_bool() {
                        a(1)
                    } else {
                        a(2)
                    }
                }
                Op::Neg => Val::Int(a(0).as_int().wrapping_neg()),
                Op::Add => Val::Int(a(0).as_int().wrapping_add(a(1).as_int())),
                Op::Sub => Val::Int(a(0).as_int().wrapping_sub(a(1).as_int())),
                Op::Mul => Val::Int(a(0).as_int().wrapping_mul(a(1).as_int())),
                Op::Div => Val::Int(a(0).as_int().checked_div(a(1).as_int()).unwrap_or(-1)),
                Op::Rem => Val::Int(a(0).as_int().checked_rem(a(1).as_int()).unwrap_or_else(|| a(0).as_int())),
                Op::Lt => Val::Bool(a(0).as_int() < a(1).as_int()),
                Op::Le => Val::Bool(a(0).as_int() <= a(1).as_int()),
                Op::Gt => Val::Bool(a(0).as_int() > a(1).as_int()),
                Op::Ge => Val::Bool(a(0).as_int() >= a(1).as_int()),
                Op::BitAnd => Val::Int(a(0).as_int() & a(1).as_int()),
                Op::BitOr => Val::Int(a(0).as_int() | a(1).as_int()),
                Op::BitXor => Val::Int(a(0).as_int() ^ a(1).as_int()),
                Op::Shl => Val::Int(a(0).as_int().wrapping_shl(a(1).as_int() as u32)),
                Op::Shr => Val::Int(a(0).as_int().wrapping_shr(a(1).as_int() as u32)),
                Op::FNeg => fp1(a(0), |x| -x, |x| -x),
                Op::FAbs => fp1(a(0), f64::abs, f32::abs),
                Op::FSqrt => fp1(a(0), f64::sqrt, f32::sqrt),
                Op::FAdd => fp2(a(0), a(1), |x, y| x + y, |x, y| x + y),
                Op::FSub => fp2(a(0), a(1), |x, y| x - y, |x, y| x - y),
                Op::FMul => fp2(a(0), a(1), |x, y| x * y, |x, y| x * y),
                Op::FDiv => fp2(a(0), a(1), |x, y| x / y, |x, y| x / y),
                Op::FFma => match (a(0), a(1), a(2)) {
                    (Val::F32(x), Val::F32(y), Val::F32(z)) => Val::F32(x.mul_add(y, z)),
                    (x, y, z) => Val::F64(x.as_f64().mul_add(y.as_f64(), z.as_f64())),
                },
                Op::FLt => Val::Bool(a(0).as_f64() < a(1).as_f64()),
                Op::FLe => Val::Bool(a(0).as_f64() <= a(1).as_f64()),
                Op::FGt => Val::Bool(a(0).as_f64() > a(1).as_f64()),
                Op::FGe => Val::Bool(a(0).as_f64() >= a(1).as_f64()),
                Op::FEq => Val::Bool(a(0).as_f64() == a(1).as_f64()),
                Op::ToF32 => Val::F32(a(0).as_int() as f32),
                Op::ToF64 => Val::F64(a(0).as_int() as f64),
                Op::IsFinite => Val::Bool(a(0).as_f64().is_finite()),
            }
        }
    }
}

fn same(a: Val, b: Val) -> bool {
    match (a, b) {
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Int(x), Val::Int(y)) => x == y,
        (x, y) => x.as_f64() == y.as_f64(),
    }
}

fn fp1(x: Val, f64_: fn(f64) -> f64, f32_: fn(f32) -> f32) -> Val {
    match x {
        Val::F32(v) => Val::F32(f32_(v)),
        v => Val::F64(f64_(v.as_f64())),
    }
}

fn fp2(x: Val, y: Val, f64_: fn(f64, f64) -> f64, f32_: fn(f32, f32) -> f32) -> Val {
    match (x, y) {
        (Val::F32(a), Val::F32(b)) => Val::F32(f32_(a, b)),
        (a, b) => Val::F64(f64_(a.as_f64(), b.as_f64())),
    }
}
