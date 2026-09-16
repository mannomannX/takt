//! Matrizen (3.11): Operatoren und Methoden ueber `libtaktm::mat`, damit
//! Interpreter und erzeugter Code dieselbe Numerik nehmen (plan/m6.md 2.10).
//! Ein nicht endliches Ergebnis ist `ArithmeticFault(NonFinite)`, eine
//! singulaere Matrix bei `inv`/`solve` `ArithmeticFault(Singular)` (3.11).

use libtaktm::mat::{self, Scalar};
use takt_diag::Span;
use takt_mir::expr::{BinaryOp, MatOp};
use takt_mir::machine::ArithKind;

use crate::arith::arith;
use crate::value::{EvalResult, Trap, Value, bug};

/// Die Elemente einer Matrix in einer Breite.
enum Data {
    F32(Vec<f32>),
    F64(Vec<f64>),
}

fn data_of(v: &Value) -> Option<(usize, usize, Data)> {
    let Value::Mat { rows, cols, data } = v else { return None };
    let (rows, cols) = (*rows as usize, *cols as usize);
    if data.iter().all(|x| matches!(x, Value::F64(_))) {
        let d = data.iter().map(|x| if let Value::F64(f) = x { *f } else { 0.0 }).collect();
        return Some((rows, cols, Data::F64(d)));
    }
    if data.iter().all(|x| matches!(x, Value::F32(_))) {
        let d = data.iter().map(|x| if let Value::F32(f) = x { *f } else { 0.0 }).collect();
        return Some((rows, cols, Data::F32(d)));
    }
    None
}

fn finite<S: Scalar>(d: &[S], span: Span, tick: u64) -> EvalResult<()> {
    if d.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(arith(ArithKind::NonFinite, "Matrixergebnis nicht endlich", span, tick))
    }
}

fn value(rows: usize, cols: usize, d: Data, span: Span, tick: u64) -> EvalResult<Value> {
    let data = match d {
        Data::F64(v) => {
            finite(&v, span, tick)?;
            v.into_iter().map(Value::F64).collect()
        }
        Data::F32(v) => {
            finite(&v, span, tick)?;
            v.into_iter().map(Value::F32).collect()
        }
    };
    Ok(Value::Mat { rows: rows as u32, cols: cols as u32, data })
}

fn scalar_of(v: &Value) -> Option<Data> {
    match v {
        Value::F64(f) => Some(Data::F64(vec![*f])),
        Value::F32(f) => Some(Data::F32(vec![*f])),
        _ => None,
    }
}

/// `+`, `-`, `*`, `/` mit einer Matrix; die Formen hat die Sema geprueft.
pub fn binary(op: BinaryOp, a: &Value, b: &Value, span: Span, tick: u64) -> EvalResult<Value> {
    match (data_of(a), data_of(b)) {
        (Some((m, k, da)), Some((k2, n, db))) => {
            let out = match (op, da, db) {
                (BinaryOp::Add, Data::F64(x), Data::F64(y)) => Data::F64(elementwise(&x, &y, mat::add)),
                (BinaryOp::Add, Data::F32(x), Data::F32(y)) => Data::F32(elementwise(&x, &y, mat::add)),
                (BinaryOp::Sub, Data::F64(x), Data::F64(y)) => Data::F64(elementwise(&x, &y, mat::sub)),
                (BinaryOp::Sub, Data::F32(x), Data::F32(y)) => Data::F32(elementwise(&x, &y, mat::sub)),
                (BinaryOp::Mul, Data::F64(x), Data::F64(y)) if k == k2 => Data::F64(product(&x, &y, m, k, n)),
                (BinaryOp::Mul, Data::F32(x), Data::F32(y)) if k == k2 => Data::F32(product(&x, &y, m, k, n)),
                _ => return bug(format!("{op:?} auf mat<{m}, {k}> und mat<{k2}, {n}>")),
            };
            let cols = if op == BinaryOp::Mul { n } else { k };
            value(m, cols, out, span, tick)
        }
        (Some((m, n, da)), None) => match (op, da, scalar_of(b)) {
            (BinaryOp::Mul, Data::F64(x), Some(Data::F64(s))) => {
                value(m, n, Data::F64(scaled(&x, s[0], mat::scale)), span, tick)
            }
            (BinaryOp::Mul, Data::F32(x), Some(Data::F32(s))) => {
                value(m, n, Data::F32(scaled(&x, s[0], mat::scale)), span, tick)
            }
            (BinaryOp::Div, Data::F64(x), Some(Data::F64(s))) => {
                value(m, n, Data::F64(scaled(&x, s[0], mat::divide)), span, tick)
            }
            (BinaryOp::Div, Data::F32(x), Some(Data::F32(s))) => {
                value(m, n, Data::F32(scaled(&x, s[0], mat::divide)), span, tick)
            }
            _ => bug(format!("{op:?} auf Matrix und {}", b.kind_name())),
        },
        (None, Some((m, n, db))) => match (op, scalar_of(a), db) {
            (BinaryOp::Mul, Some(Data::F64(s)), Data::F64(x)) => {
                value(m, n, Data::F64(scaled(&x, s[0], mat::scale)), span, tick)
            }
            (BinaryOp::Mul, Some(Data::F32(s)), Data::F32(x)) => {
                value(m, n, Data::F32(scaled(&x, s[0], mat::scale)), span, tick)
            }
            _ => bug(format!("{op:?} auf {} und Matrix", a.kind_name())),
        },
        (None, None) => bug("Matrixoperation ohne Matrix"),
    }
}

fn elementwise<S: Scalar>(x: &[S], y: &[S], f: fn(&[S], &[S], &mut [S])) -> Vec<S> {
    let mut out = vec![S::ZERO; x.len()];
    f(x, y, &mut out);
    out
}

fn scaled<S: Scalar>(x: &[S], s: S, f: fn(&[S], S, &mut [S])) -> Vec<S> {
    let mut out = vec![S::ZERO; x.len()];
    f(x, s, &mut out);
    out
}

fn product<S: Scalar>(x: &[S], y: &[S], m: usize, k: usize, n: usize) -> Vec<S> {
    let mut out = vec![S::ZERO; m * n];
    mat::mul(x, y, m, k, n, &mut out);
    out
}

/// `transpose`, `inv`, `det`, `solve`, `cholesky` (3.11).
pub fn op(op: MatOp, args: Vec<Value>, span: Span, tick: u64) -> EvalResult<Value> {
    let Some((m, n, d)) = args.first().and_then(data_of) else {
        return bug(format!("{op:?} ohne Matrix"));
    };
    match (op, d) {
        (MatOp::Transpose, Data::F64(x)) => value(n, m, Data::F64(transposed(&x, m, n)), span, tick),
        (MatOp::Transpose, Data::F32(x)) => value(n, m, Data::F32(transposed(&x, m, n)), span, tick),
        (MatOp::Det, Data::F64(x)) => {
            let d = determinant(&x, n);
            finite(&[d], span, tick)?;
            Ok(Value::F64(d))
        }
        (MatOp::Det, Data::F32(x)) => {
            let d = determinant(&x, n);
            finite(&[d], span, tick)?;
            Ok(Value::F32(d))
        }
        (MatOp::Inv, Data::F64(x)) => {
            value(n, n, Data::F64(inverse(&x, n).ok_or_else(|| singular(span, tick))?), span, tick)
        }
        (MatOp::Inv, Data::F32(x)) => {
            value(n, n, Data::F32(inverse(&x, n).ok_or_else(|| singular(span, tick))?), span, tick)
        }
        (MatOp::Solve, d) => {
            let Some((rb, k, db)) = args.get(1).and_then(data_of) else { return bug("`solve` ohne rechte Seite") };
            if rb != n {
                return bug(format!("`solve`: mat<{n}, {n}> gegen mat<{rb}, {k}>"));
            }
            match (d, db) {
                (Data::F64(x), Data::F64(y)) => {
                    value(n, k, Data::F64(solved(&x, n, &y, k).ok_or_else(|| singular(span, tick))?), span, tick)
                }
                (Data::F32(x), Data::F32(y)) => {
                    value(n, k, Data::F32(solved(&x, n, &y, k).ok_or_else(|| singular(span, tick))?), span, tick)
                }
                _ => bug("`solve` mit gemischten Breiten"),
            }
        }
        (MatOp::Cholesky, Data::F64(x)) => Ok(Value::Optional(match factor(&x, n) {
            Some(l) => Some(Box::new(value(n, n, Data::F64(l), span, tick)?)),
            None => None,
        })),
        (MatOp::Cholesky, Data::F32(x)) => Ok(Value::Optional(match factor(&x, n) {
            Some(l) => Some(Box::new(value(n, n, Data::F32(l), span, tick)?)),
            None => None,
        })),
    }
}

fn singular(span: Span, tick: u64) -> Trap {
    arith(ArithKind::Singular, "Matrix singulaer", span, tick)
}

fn transposed<S: Scalar>(x: &[S], m: usize, n: usize) -> Vec<S> {
    let mut out = vec![S::ZERO; m * n];
    mat::transpose(x, m, n, &mut out);
    out
}

fn determinant<S: Scalar>(x: &[S], n: usize) -> S {
    let (mut scratch, mut perm) = (vec![S::ZERO; mat::scratch_len(n)], vec![0usize; n]);
    mat::det(x, n, &mut scratch, &mut perm)
}

fn inverse<S: Scalar>(x: &[S], n: usize) -> Option<Vec<S>> {
    let (mut out, mut scratch, mut perm) = (vec![S::ZERO; n * n], vec![S::ZERO; mat::scratch_len(n)], vec![0usize; n]);
    mat::inv(x, n, &mut out, &mut scratch, &mut perm).ok()?;
    Some(out)
}

fn solved<S: Scalar>(x: &[S], n: usize, b: &[S], k: usize) -> Option<Vec<S>> {
    let (mut out, mut scratch, mut perm) = (vec![S::ZERO; n * k], vec![S::ZERO; mat::scratch_len(n)], vec![0usize; n]);
    mat::solve(x, n, b, k, &mut out, &mut scratch, &mut perm).ok()?;
    Some(out)
}

fn factor<S: Scalar>(x: &[S], n: usize) -> Option<Vec<S>> {
    let mut out = vec![S::ZERO; n * n];
    mat::cholesky(x, n, &mut out)?;
    Some(out)
}
