//! Totale Arithmetik (Referenz 4.1, 3.3, 3.10, 4.2): jede Operation liefert
//! einen Wert oder einen Fault. Ganzzahlen rechnen in der Breite ihres
//! statischen Typs, Fliesskomma in Programmbreite ohne `NaN`/`Inf`.

use takt_diag::Span;
use takt_mir::expr::{BinaryOp, UnaryOp};
use takt_mir::machine::{ArithKind, FaultKind};
use takt_mir::types::{FloatWidth, IntWidth};

use crate::value::{EvalResult, Fault, Trap, Value, bug};

fn arith(kind: ArithKind, what: impl Into<String>, span: Span, tick: u64) -> Trap {
    Trap::Fault(Fault::new(FaultKind::Arithmetic(kind), what, span, tick))
}

/// Wertebereich einer Breite.
pub fn bounds(width: IntWidth) -> (i128, i128) {
    let bits = width.bits();
    if width.signed() { (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1) } else { (0, (1i128 << bits) - 1) }
}

/// Prueft das exakte Ergebnis gegen die Breite (Overflow ist ein Fault, 4.1, 3.10).
pub fn fit(width: IntWidth, x: i128, span: Span, tick: u64) -> EvalResult<Value> {
    let (lo, hi) = bounds(width);
    if x < lo || x > hi {
        return Err(arith(ArithKind::Overflow, format!("Ueberlauf in {}", name(width)), span, tick));
    }
    Ok(Value::int(width, x))
}

/// Name einer Breite.
pub fn name(width: IntWidth) -> &'static str {
    match width {
        IntWidth::I8 => "i8",
        IntWidth::I16 => "i16",
        IntWidth::I32 => "i32",
        IntWidth::I64 => "int",
        IntWidth::U8 => "u8",
        IntWidth::U16 => "u16",
        IntWidth::U32 => "u32",
        IntWidth::U64 => "u64",
    }
}

/// Zweierkomplement-Wickelung in die Breite (fuer `<<`, `wrapping_*`, `.wrap_*()`).
pub fn wrap(width: IntWidth, x: i128) -> Value {
    let bits = width.bits();
    let mask = (1i128 << bits) - 1;
    let low = x & mask;
    let v = if width.signed() && low >= (1i128 << (bits - 1)) { low - (1i128 << bits) } else { low };
    Value::int(width, v)
}

/// Ganzzahloperation in einer Breite.
pub fn int_binary(op: BinaryOp, a: i128, b: i128, width: IntWidth, span: Span, tick: u64) -> EvalResult<Value> {
    let bits = i128::from(width.bits());
    match op {
        BinaryOp::Add => fit(width, a + b, span, tick),
        BinaryOp::Sub => fit(width, a - b, span, tick),
        BinaryOp::Mul => fit(width, a * b, span, tick),
        BinaryOp::Div => {
            if b == 0 {
                return Err(arith(ArithKind::DivZero, "Division durch null", span, tick));
            }
            fit(width, a / b, span, tick)
        }
        BinaryOp::Rem => {
            if b == 0 {
                return Err(arith(ArithKind::DivZero, "Division durch null", span, tick));
            }
            fit(width, a % b, span, tick)
        }
        BinaryOp::BitAnd => Ok(Value::int(width, a & b)),
        BinaryOp::BitOr => Ok(Value::int(width, a | b)),
        BinaryOp::BitXor => Ok(Value::int(width, a ^ b)),
        BinaryOp::Shl | BinaryOp::Shr => {
            if b < 0 || b >= bits {
                return Err(Trap::Fault(Fault::new(
                    FaultKind::Range,
                    format!("Shift-Betrag {b} ausserhalb 0..{}", bits - 1),
                    span,
                    tick,
                )));
            }
            if op == BinaryOp::Shl {
                Ok(wrap(width, a << b))
            } else {
                // arithmetisch fuer signierte, logisch fuer unsignierte Breiten (3.10)
                Ok(Value::int(width, a >> b))
            }
        }
        BinaryOp::Lt => Ok(Value::Bool(a < b)),
        BinaryOp::Le => Ok(Value::Bool(a <= b)),
        BinaryOp::Gt => Ok(Value::Bool(a > b)),
        BinaryOp::Ge => Ok(Value::Bool(a >= b)),
        BinaryOp::Eq => Ok(Value::Bool(a == b)),
        BinaryOp::Ne => Ok(Value::Bool(a != b)),
        BinaryOp::And | BinaryOp::Or => bug("and/or auf Ganzzahlen"),
    }
}

/// Endliches Ergebnis in Programmbreite oder `NonFinite`.
pub fn finite(width: FloatWidth, x: f64, span: Span, tick: u64) -> EvalResult<Value> {
    match width {
        FloatWidth::F32 => {
            let y = x as f32;
            if y.is_finite() {
                Ok(Value::F32(y))
            } else {
                Err(arith(ArithKind::NonFinite, "Ergebnis nicht endlich", span, tick))
            }
        }
        FloatWidth::F64 => {
            if x.is_finite() {
                Ok(Value::F64(x))
            } else {
                Err(arith(ArithKind::NonFinite, "Ergebnis nicht endlich", span, tick))
            }
        }
    }
}

/// Fliesskommaoperation; `f32` rechnet nativ in `f32` (IEEE, round-to-nearest-even).
pub fn float_binary(op: BinaryOp, a: &Value, b: &Value, span: Span, tick: u64) -> EvalResult<Value> {
    match (a, b) {
        (Value::F32(x), Value::F32(y)) => {
            let (x, y) = (*x, *y);
            match op {
                BinaryOp::Add => finite(FloatWidth::F32, f64::from(x + y), span, tick),
                BinaryOp::Sub => finite(FloatWidth::F32, f64::from(x - y), span, tick),
                BinaryOp::Mul => finite(FloatWidth::F32, f64::from(x * y), span, tick),
                BinaryOp::Div => finite(FloatWidth::F32, f64::from(x / y), span, tick),
                _ => compare_floats(op, f64::from(x), f64::from(y)),
            }
        }
        (Value::F64(x), Value::F64(y)) => {
            let (x, y) = (*x, *y);
            match op {
                BinaryOp::Add => finite(FloatWidth::F64, x + y, span, tick),
                BinaryOp::Sub => finite(FloatWidth::F64, x - y, span, tick),
                BinaryOp::Mul => finite(FloatWidth::F64, x * y, span, tick),
                BinaryOp::Div => finite(FloatWidth::F64, x / y, span, tick),
                _ => compare_floats(op, x, y),
            }
        }
        _ => bug(format!("Fliesskomma-Operation auf {} und {}", a.kind_name(), b.kind_name())),
    }
}

fn compare_floats(op: BinaryOp, x: f64, y: f64) -> EvalResult<Value> {
    Ok(Value::Bool(match op {
        BinaryOp::Lt => x < y,
        BinaryOp::Le => x <= y,
        BinaryOp::Gt => x > y,
        BinaryOp::Ge => x >= y,
        BinaryOp::Eq => x == y,
        BinaryOp::Ne => x != y,
        _ => return bug("Operator auf Fliesskomma"),
    }))
}

/// Dauer-Arithmetik (3.3): `± Duration`, `· int`, `/ int` (Trunkierung),
/// `Duration / Duration → int`, Vergleiche.
pub fn duration_binary(op: BinaryOp, a: &Value, b: &Value, span: Span, tick: u64) -> EvalResult<Value> {
    let overflow = || arith(ArithKind::Overflow, "Ueberlauf einer Dauer", span, tick);
    match (op, a, b) {
        (BinaryOp::Add, Value::Duration(x), Value::Duration(y)) => {
            x.checked_add(*y).map(Value::Duration).ok_or_else(overflow)
        }
        (BinaryOp::Sub, Value::Duration(x), Value::Duration(y)) => {
            x.checked_sub(*y).map(Value::Duration).ok_or_else(overflow)
        }
        (BinaryOp::Mul, Value::Duration(x), Value::Int(n)) | (BinaryOp::Mul, Value::Int(n), Value::Duration(x)) => {
            x.checked_mul(*n).map(Value::Duration).ok_or_else(overflow)
        }
        (BinaryOp::Div, Value::Duration(x), Value::Int(n)) => {
            if *n == 0 {
                return Err(arith(ArithKind::DivZero, "Division durch null", span, tick));
            }
            x.checked_div(*n).map(Value::Duration).ok_or_else(overflow)
        }
        (BinaryOp::Div, Value::Duration(x), Value::Duration(y)) => {
            if *y == 0 {
                return Err(arith(ArithKind::DivZero, "Division durch null", span, tick));
            }
            x.checked_div(*y).map(Value::Int).ok_or_else(overflow)
        }
        (BinaryOp::Lt, Value::Duration(x), Value::Duration(y)) => Ok(Value::Bool(x < y)),
        (BinaryOp::Le, Value::Duration(x), Value::Duration(y)) => Ok(Value::Bool(x <= y)),
        (BinaryOp::Gt, Value::Duration(x), Value::Duration(y)) => Ok(Value::Bool(x > y)),
        (BinaryOp::Ge, Value::Duration(x), Value::Duration(y)) => Ok(Value::Bool(x >= y)),
        (BinaryOp::Eq, Value::Duration(x), Value::Duration(y)) => Ok(Value::Bool(x == y)),
        (BinaryOp::Ne, Value::Duration(x), Value::Duration(y)) => Ok(Value::Bool(x != y)),
        _ => bug(format!("Dauer-Operation {op:?} auf {} und {}", a.kind_name(), b.kind_name())),
    }
}

/// Einstellige Operatoren.
pub fn unary(op: UnaryOp, v: &Value, width: Option<IntWidth>, span: Span, tick: u64) -> EvalResult<Value> {
    match (op, v) {
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, Value::Int(_) | Value::UInt(_)) => {
            let w = width.ok_or_else(|| Trap::Bug("Negation ohne Breite".into()))?;
            fit(w, -v.as_int().expect("Ganzzahl"), span, tick)
        }
        (UnaryOp::Neg, Value::F32(x)) => Ok(Value::F32(-x)),
        (UnaryOp::Neg, Value::F64(x)) => Ok(Value::F64(-x)),
        (UnaryOp::Neg, Value::Duration(d)) => d
            .checked_neg()
            .map(Value::Duration)
            .ok_or_else(|| arith(ArithKind::Overflow, "Ueberlauf einer Dauer", span, tick)),
        (UnaryOp::BitNot, Value::Int(_) | Value::UInt(_)) => {
            let w = width.ok_or_else(|| Trap::Bug("Bitnegation ohne Breite".into()))?;
            Ok(wrap(w, !v.as_int().expect("Ganzzahl")))
        }
        _ => bug(format!("{op:?} auf {}", v.kind_name())),
    }
}
