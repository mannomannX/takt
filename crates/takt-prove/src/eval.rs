//! Konkrete Auswertung der Terme: dieselbe Semantik, die der Drucker nach
//! SMT-LIB2 traegt — i64 wie `bvsdiv`/`bvsrem`, IEEE-754 in der Breite. Damit
//! laesst sich die Kodierung ohne Solver gegen den Interpreter pruefen
//! (plan/m6.md 2.8): Ein Gegenbeispiel des Solvers oder ein Lauf des
//! Modells muss im Interpreter dasselbe tun.

use std::collections::{BTreeMap, HashMap};

use crate::term::{Fun, Node, Op, Rounding, Sort, Term};

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

/// Ein Stapel statt Rekursion: Ein ausgerollter Pfad ist tausende Knoten
/// tief (FB-403). Ein Knoten, dem ein Argument fehlt, legt sich mit ihm
/// zurueck auf den Stapel; `ite` wertet nur den genommenen Zweig, `and`
/// und `or` brechen ab, sobald ihr Wert feststeht.
fn eval_in(t: &Term, env: &Env, memo: &mut HashMap<usize, Val>) -> Val {
    let mut stack = vec![(t.clone(), 0)];
    while let Some((n, from)) = stack.pop() {
        if !memo.contains_key(&n.key())
            && let Some(v) = visit(&n, from, env, memo, &mut stack)
        {
            memo.insert(n.key(), v);
        }
    }
    memo[&t.key()]
}

/// Der Wert von `t`, wenn seine Argumente bekannt sind; sonst legt er sich
/// und das naechste fehlende Argument auf den Stapel. `from` ist bei `and`
/// und `or` das erste noch ungepruefte Argument.
fn visit(t: &Term, from: usize, env: &Env, memo: &HashMap<usize, Val>, stack: &mut Vec<(Term, usize)>) -> Option<Val> {
    let Node::App(op, args) = &*t.0 else { return Some(leaf(t, env)) };
    let known = |a: &Term| memo.get(&a.key()).copied();
    let mut wait = |a: &Term, from: usize| {
        stack.push((t.clone(), from));
        stack.push((a.clone(), 0));
        None
    };
    match op {
        Op::Ite => {
            let Some(c) = known(&args[0]) else { return wait(&args[0], 0) };
            let pick = &args[if c.as_bool() { 1 } else { 2 }];
            known(pick).or_else(|| wait(pick, 0))
        }
        Op::And | Op::Or => {
            let decisive = matches!(op, Op::Or);
            for (i, a) in args.iter().enumerate().skip(from) {
                match known(a) {
                    Some(v) if v.as_bool() == decisive => return Some(Val::Bool(decisive)),
                    Some(_) => {}
                    None => return wait(a, i),
                }
            }
            Some(Val::Bool(!decisive))
        }
        _ => match args.iter().find(|a| known(a).is_none()) {
            Some(a) => wait(a, 0),
            None => Some(apply(*op, args, memo)),
        },
    }
}

fn leaf(t: &Term, env: &Env) -> Val {
    match &*t.0 {
        Node::Bool(b) => Val::Bool(*b),
        Node::Int(i) => Val::Int(*i),
        Node::F32(f) => Val::F32(*f),
        Node::F64(f) => Val::F64(*f),
        Node::Var(name, sort) => env.get(name).copied().unwrap_or(Val::zero(*sort)),
        Node::App(..) => unreachable!("kein Blatt"),
    }
}

/// Eine Operation ueber ihren bekannten Argumenten.
fn apply(op: Op, args: &[Term], memo: &HashMap<usize, Val>) -> Val {
    let a = |i: usize| memo[&args[i].key()];
    match op {
        Op::Ite | Op::And | Op::Or => unreachable!("im Stapel ausgewertet"),
        Op::Not => Val::Bool(!a(0).as_bool()),
        Op::Eq => Val::Bool(same(a(0), a(1))),
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
        // Wie `bvshl`/`bvashr`: Der Betrag zaehlt ohne Vorzeichen, ab
        // 64 ist alles hinausgeschoben.
        Op::Shl => {
            let (x, n) = (a(0).as_int(), a(1).as_int() as u64);
            Val::Int(if n >= 64 { 0 } else { x << n })
        }
        Op::Shr => {
            let (x, n) = (a(0).as_int(), a(1).as_int() as u64);
            Val::Int(if n >= 64 { x >> 63 } else { x >> n })
        }
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
        Op::Wrap { bits, signed } => Val::Int(wrap(a(0).as_int(), bits, signed)),
        Op::AddOverflows => Val::Bool(a(0).as_int().checked_add(a(1).as_int()).is_none()),
        Op::SubOverflows => Val::Bool(a(0).as_int().checked_sub(a(1).as_int()).is_none()),
        Op::MulOverflows => Val::Bool(a(0).as_int().checked_mul(a(1).as_int()).is_none()),
        Op::Scale { num, den } => match a(0) {
            Val::F32(x) => Val::F32(libtaktm::scale_f32(x, num, den)),
            x => Val::F64(libtaktm::scale_f64(x.as_f64(), num, den)),
        },
        Op::Math(f) => {
            let x = a(0);
            let y = if args.len() > 1 { a(1) } else { Val::F64(0.0) };
            math(f, x, y)
        }
        Op::Round(r) => rounded(r, a(0)),
        Op::Native { f, part, .. } => native(f, part, &(0..args.len()).map(a).collect::<Vec<_>>()),
        Op::FloatToInt => Val::Int(a(0).as_f64() as i64),
    }
}

/// Teil `part` des Ergebnisses einer Native, wie der Interpreter sie ruft
/// (`call::call_native`): die Bloecke `[Kapazitaet, Laenge, Byte …]` als
/// Bytes der Grenze.
fn native(f: takt_native::Native, part: u16, args: &[Val]) -> Val {
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut rest = args;
    while let [cap, len, tail @ ..] = rest {
        let cap = usize::try_from(cap.as_int()).unwrap_or(0).min(tail.len());
        let len = usize::try_from(len.as_int()).unwrap_or(0).min(cap);
        blocks.push(tail[..len].iter().map(|b| b.as_int() as u8).collect());
        rest = &tail[cap..];
    }
    let inputs: Vec<&[u8]> = blocks.iter().map(Vec::as_slice).collect();
    Val::Int(match takt_native::call(f, &inputs) {
        Some(takt_native::Output::Scalar(raw)) => raw as i64,
        Some(takt_native::Output::Digest(d)) => d.get(usize::from(part)).copied().map_or(0, i64::from),
        _ => 0,
    })
}

/// Auf eine ganze Zahl gerundet, in der Breite des Werts.
fn rounded(r: Rounding, v: Val) -> Val {
    match (r, v) {
        (Rounding::HalfAway, Val::F32(x)) => Val::F32(x.round()),
        (Rounding::Down, Val::F32(x)) => Val::F32(x.floor()),
        (Rounding::Up, Val::F32(x)) => Val::F32(x.ceil()),
        (Rounding::TowardZero, Val::F32(x)) => Val::F32(x.trunc()),
        (Rounding::HalfAway, x) => Val::F64(x.as_f64().round()),
        (Rounding::Down, x) => Val::F64(x.as_f64().floor()),
        (Rounding::Up, x) => Val::F64(x.as_f64().ceil()),
        (Rounding::TowardZero, x) => Val::F64(x.as_f64().trunc()),
    }
}

/// Eine Funktion aus `libtaktm` in der Breite ihres Arguments, wie der
/// Interpreter sie ruft (`call::math_f64`, `math_f32`).
fn math(f: Fun, x: Val, y: Val) -> Val {
    match x {
        Val::F32(x) => {
            let y = match y {
                Val::F32(y) => y,
                other => other.as_f64() as f32,
            };
            Val::F32(match f {
                Fun::Sin => libtaktm::sin_f32(x),
                Fun::Cos => libtaktm::cos_f32(x),
                Fun::Tan => libtaktm::tan_f32(x),
                Fun::Asin => libtaktm::asin_f32(x),
                Fun::Acos => libtaktm::acos_f32(x),
                Fun::Atan => libtaktm::atan_f32(x),
                Fun::Atan2 => libtaktm::atan2_f32(x, y),
                Fun::Exp => libtaktm::exp_f32(x),
                Fun::Log => libtaktm::log_f32(x),
                Fun::Pow => libtaktm::pow_f32(x, y),
            })
        }
        x => {
            let (x, y) = (x.as_f64(), y.as_f64());
            Val::F64(match f {
                Fun::Sin => libtaktm::sin_f64(x),
                Fun::Cos => libtaktm::cos_f64(x),
                Fun::Tan => libtaktm::tan_f64(x),
                Fun::Asin => libtaktm::asin_f64(x),
                Fun::Acos => libtaktm::acos_f64(x),
                Fun::Atan => libtaktm::atan_f64(x),
                Fun::Atan2 => libtaktm::atan2_f64(x, y),
                Fun::Exp => libtaktm::exp_f64(x),
                Fun::Log => libtaktm::log_f64(x),
                Fun::Pow => libtaktm::pow_f64(x, y),
            })
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

/// `=` wie in SMT-LIB: Fliesskomma gleicht aufs Bit, `+0` nicht `-0`, und
/// es gibt nur ein NaN.
fn same(a: Val, b: Val) -> bool {
    match (a, b) {
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::F32(x), Val::F32(y)) => (x.is_nan() && y.is_nan()) || x.to_bits() == y.to_bits(),
        (x, y) => {
            let (x, y) = (x.as_f64(), y.as_f64());
            (x.is_nan() && y.is_nan()) || x.to_bits() == y.to_bits()
        }
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
