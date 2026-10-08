//! Konkrete Auswertung der Terme: dieselbe Semantik, die der Drucker nach
//! SMT-LIB2 traegt — i64 wie `bvsdiv`/`bvsrem`, IEEE-754 in der Breite. Damit
//! laesst sich die Kodierung ohne Solver gegen den Interpreter pruefen
//! (plan/m6.md 2.8): Ein Gegenbeispiel des Solvers oder ein Lauf des
//! Modells muss im Interpreter dasselbe tun.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

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

/// Wertet `t` unter `env`; eine fehlende Variable ist ihr Nullwert. Jeder
/// geteilte Knoten wird einmal ausgewertet: Der Term ist ein Graph, und
/// ohne Gedaechtnis waechst die Arbeit mit der Zahl seiner Pfade.
pub fn eval(t: &Term, env: &Env) -> Val {
    eval_in(t, env, &mut HashMap::new())
}

/// Wertet mehrere Terme unter derselben Belegung, mit einem Gedaechtnis
/// fuer alle: Die Folgezustaende eines Modells teilen den groessten Teil
/// ihres Graphen.
pub fn eval_all<'a>(ts: impl IntoIterator<Item = &'a Term>, env: &Env) -> Vec<Val> {
    let mut memo = HashMap::new();
    ts.into_iter().map(|t| eval_in(t, env, &mut memo)).collect()
}

fn eval_in(t: &Term, env: &Env, memo: &mut HashMap<usize, Val>) -> Val {
    let key = Rc::as_ptr(&t.0) as usize;
    if let Some(v) = memo.get(&key) {
        return *v;
    }
    let v = eval_node(t, env, memo);
    memo.insert(key, v);
    v
}

fn eval_node(t: &Term, env: &Env, memo: &mut HashMap<usize, Val>) -> Val {
    match &*t.0 {
        Node::Bool(b) => Val::Bool(*b),
        Node::Int(i) => Val::Int(*i),
        Node::F32(f) => Val::F32(*f),
        Node::F64(f) => Val::F64(*f),
        Node::Var(name, sort) => env.get(name).copied().unwrap_or(Val::zero(*sort)),
        Node::App(op, args) => {
            let mut a = |i: usize| eval_in(&args[i], env, memo);
            match op {
                Op::Not => Val::Bool(!a(0).as_bool()),
                Op::And => Val::Bool(args.iter().all(|x| eval_in(x, env, memo).as_bool())),
                Op::Or => Val::Bool(args.iter().any(|x| eval_in(x, env, memo).as_bool())),
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
                Op::Div => Val::Int(sdiv(a(0).as_int(), a(1).as_int())),
                Op::Rem => Val::Int(srem(a(0).as_int(), a(1).as_int())),
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
                Op::Wrap { bits, signed } => Val::Int(wrap(a(0).as_int(), *bits, *signed)),
                Op::AddOverflows => Val::Bool(a(0).as_int().checked_add(a(1).as_int()).is_none()),
                Op::SubOverflows => Val::Bool(a(0).as_int().checked_sub(a(1).as_int()).is_none()),
                Op::MulOverflows => Val::Bool(a(0).as_int().checked_mul(a(1).as_int()).is_none()),
            }
        }
    }
}

/// `bvsdiv`: durch null `-1` fuer nichtnegative, `1` fuer negative
/// Dividenden; `MIN / -1` laeuft um.
fn sdiv(a: i64, b: i64) -> i64 {
    match b {
        0 if a < 0 => 1,
        0 => -1,
        _ => a.wrapping_div(b),
    }
}

/// `bvsrem`: durch null der Dividend; `MIN % -1` ist null.
fn srem(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { a.wrapping_rem(b) }
}

/// Die unteren `bits` Bits, erweitert wie `Op::Wrap`.
fn wrap(x: i64, bits: u32, signed: bool) -> i64 {
    if bits >= 64 {
        return x;
    }
    let shift = 64 - bits;
    if signed { (x << shift) >> shift } else { ((x as u64) << shift >> shift) as i64 }
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
