//! Kampagnen (13.7): der Laufraum einer `campaign` als Parametervektoren.
//!
//! Jeder Lauf ist eine deterministische Funktion seines Vektors und der
//! Inputs (9.4.1); `repeat` liefert in der Simulation darum identische
//! Laeufe, und die Tabelle zeigt das.

use takt_mir::program::{Campaign, Sweep};
use takt_mir::{ParamId, Program};

use crate::value::{Trap, Value};

/// Ein Lauf der Kampagne.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Lauf-ID, ab 1 in Laufraumordnung.
    pub id: u32,
    /// Wiederholung, ab 1.
    pub repeat: u32,
    /// Die gesweepten Parameter als Name und Literal.
    pub params: Vec<(String, String)>,
}

/// Der Laufraum: kartesisches Produkt der Sweeps mal `repeat`, erster
/// Sweep aussen, Wiederholungen innen.
pub fn runs(p: &Program, c: &Campaign) -> Result<Vec<Run>, Trap> {
    let mut vectors: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for sweep in &c.sweeps {
        let (param, values) = sweep_values(p, sweep)?;
        let def = &p.params[param.index()];
        let texts: Vec<String> = values.iter().map(|v| crate::trace::value_text(v, def.ty, p)).collect();
        vectors = vectors
            .iter()
            .flat_map(|v| {
                texts.iter().map(|t| {
                    let mut w = v.clone();
                    w.push((def.name.clone(), t.clone()));
                    w
                })
            })
            .collect();
    }
    let mut out = Vec::new();
    for v in vectors {
        for repeat in 1..=c.repeat.max(1) {
            out.push(Run { id: out.len() as u32 + 1, repeat, params: v.clone() });
        }
    }
    Ok(out)
}

/// Die Werte eines Sweeps in Reihenfolge.
fn sweep_values(p: &Program, sweep: &Sweep) -> Result<(ParamId, Vec<Value>), Trap> {
    match sweep {
        Sweep::List { param, values } => {
            let values = values.iter().map(|e| crate::eval_const(p, e)).collect::<Result<_, _>>()?;
            Ok((*param, values))
        }
        Sweep::Range { param, from, to, step } => {
            let (from, to, step) =
                (crate::eval_const(p, from)?, crate::eval_const(p, to)?, crate::eval_const(p, step)?);
            let mut out = Vec::new();
            for k in 0i64.. {
                let Some(v) = nth(&from, &step, k) else {
                    return Err(Trap::Bug("Sweep-Bereich ohne Zahl- oder Dauertyp oder mit Ueberlauf".into()));
                };
                if !at_most(&v, &to) {
                    break;
                }
                out.push(v);
            }
            Ok((*param, out))
        }
    }
}

/// `from + k * step`.
fn nth(from: &Value, step: &Value, k: i64) -> Option<Value> {
    Some(match (from, step) {
        (Value::Int(a), Value::Int(s)) => Value::Int(a.checked_add(k.checked_mul(*s)?)?),
        (Value::UInt(a), Value::UInt(s)) => Value::UInt(a.checked_add(u64::try_from(k).ok()?.checked_mul(*s)?)?),
        (Value::Duration(a), Value::Duration(s)) => Value::Duration(a.checked_add(k.checked_mul(*s)?)?),
        (Value::F64(a), Value::F64(s)) => Value::F64(a + k as f64 * s),
        (Value::F32(a), Value::F32(s)) => Value::F32(a + k as f32 * s),
        _ => return None,
    })
}

fn at_most(v: &Value, to: &Value) -> bool {
    match (v, to) {
        (Value::Int(a), Value::Int(b)) | (Value::Duration(a), Value::Duration(b)) => a <= b,
        (Value::UInt(a), Value::UInt(b)) => a <= b,
        (Value::F64(a), Value::F64(b)) => a <= b,
        (Value::F32(a), Value::F32(b)) => a <= b,
        _ => false,
    }
}
