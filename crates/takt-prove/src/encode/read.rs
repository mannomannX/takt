//! Werte zwischen dem Interpreter und den Blaettern des Modells, in beide
//! Richtungen ueber die Gestalt eines Typs (`Enc::shape`): Ihre Pfade sind
//! die eine Quelle des Layouts. Ein Lauf des Modells (`Model::run`) liest
//! so seine Eingaben aus dem Stimulus und schreibt seine Outputs als Werte,
//! die der Interpreter formatiert.

use takt_diag::Span;
use takt_interp::Value;
use takt_mir::types::{FloatWidth, Type};
use takt_mir::{Program, TypeId};

use super::Enc;
use super::value::Shape;
use crate::eval::{Env, Val};
use crate::term::Sort;

/// Der Wert eines Typs an `base` im Zustand `env`; `None`, wenn ein Blatt
/// fehlt oder der Typ keine Textform im Trace hat.
pub fn value_at(p: &Program, ty: TypeId, base: &str, env: &Env) -> Option<Value> {
    let enc = Enc::new(p, Vec::new(), None);
    let shape = enc.shape(ty, Span::default()).ok()?;
    read(&enc, ty, &shape, base, env)
}

/// Die Blaetter eines Werts an `base`, mit der Sorte, die das Modell fuer
/// sie fuehrt; hinter der Laenge einer Sammlung und in einem leeren
/// Optional null.
pub fn leaves_at(p: &Program, ty: TypeId, v: &Value, base: &str) -> Option<Vec<(String, Val)>> {
    let enc = Enc::new(p, Vec::new(), None);
    let shape = enc.shape(ty, Span::default()).ok()?;
    let mut out = Vec::new();
    write(&enc, ty, &shape, Some(v), base, &mut out)?;
    Some(out)
}

fn read(enc: &Enc<'_>, ty: TypeId, shape: &Shape, base: &str, env: &Env) -> Option<Value> {
    let p = enc.p;
    let at = |path: &str| format!("{base}{path}");
    let int = |path: &str| match env.get(&at(path))? {
        Val::Int(x) => Some(*x),
        _ => None,
    };
    let flag = |path: &str| match env.get(&at(path))? {
        Val::Bool(b) => Some(*b),
        _ => None,
    };
    let Shape::Node(parts) = shape else { return scalar(p, ty, shape, env.get(base)?) };
    let len = |cap: usize| usize::try_from(int(".len")?).ok().filter(|n| *n <= cap);
    let items = |elem: TypeId| -> Option<Vec<Value>> {
        parts.iter().map(|(path, s)| read(enc, elem, s, &at(path), env)).collect()
    };
    let bytes = |n: usize| -> Option<Vec<u8>> {
        parts.iter().skip(1).take(n).map(|(path, _)| u8::try_from(int(path)?).ok()).collect()
    };
    Some(match p.types.get(ty) {
        Type::Record(r) => {
            let fields = &p.records[r.index()].fields;
            let values = fields.iter().zip(parts).map(|(f, (path, s))| read(enc, f.ty, s, &at(path), env));
            Value::Record(values.collect::<Option<_>>()?)
        }
        Type::Array { elem, .. } => Value::Array(items(*elem)?),
        Type::Samples { elem, .. } => Value::Samples(items(*elem)?),
        Type::Mat { rows, cols, .. } => {
            let data = parts.iter().map(|(path, _)| float(env.get(&at(path))?)).collect::<Option<_>>()?;
            Value::Mat { rows: *rows, cols: *cols, data }
        }
        Type::Bytes { cap } => Value::Bytes(bytes(len(*cap as usize)?)?),
        Type::Vec { elem, cap } => {
            let n = len(*cap as usize)?;
            let values = parts.iter().skip(1).take(n).map(|(path, s)| read(enc, *elem, s, &at(path), env));
            Value::Vec(values.collect::<Option<_>>()?)
        }
        Type::Str { cap } => Value::Str(String::from_utf8(bytes(len(*cap as usize)?)?).ok()?),
        Type::Line { cap } => {
            Value::Line { text: String::from_utf8(bytes(len(*cap as usize)?)?).ok()?, truncated: flag(".truncated")? }
        }
        Type::Optional(t) => {
            let (path, s) = parts.get(1)?;
            match flag(".has")? {
                true => Value::Optional(Some(Box::new(read(enc, *t, s, &at(path), env)?))),
                false => Value::Optional(None),
            }
        }
        Type::Result { ok, err } => {
            let (ok_path, ok_shape) = parts.get(1)?;
            let (err_path, err_shape) = parts.get(2)?;
            match flag(".is_err")? {
                true => {
                    let error = match err_shape {
                        Shape::Tag(_) => {
                            Value::Enum { variant: u32::try_from(int(err_path)?).ok()?, fields: Vec::new() }
                        }
                        s => read(enc, enc.enum_type(*err)?, s, &at(err_path), env)?,
                    };
                    Value::Result(Err(Box::new(error)))
                }
                false => Value::Result(Ok(Box::new(read(enc, *ok, ok_shape, &at(ok_path), env)?))),
            }
        }
        Type::Map { key, value, .. } => {
            let mut slots = Vec::new();
            for (path, slot) in parts {
                let Shape::Node(inner) = slot else { return None };
                let base = at(path);
                let full = match env.get(&format!("{base}.has"))? {
                    Val::Bool(b) => *b,
                    _ => return None,
                };
                slots.push(match full {
                    true => {
                        let (kp, ks) = inner.get(1)?;
                        let (vp, vs) = inner.get(2)?;
                        let k = read(enc, *key, ks, &format!("{base}{kp}"), env)?;
                        Some((k, read(enc, *value, vs, &format!("{base}{vp}"), env)?))
                    }
                    false => None,
                });
            }
            Value::Map(slots)
        }
        Type::Enum(e) => {
            let variant = u32::try_from(int(".tag")?).ok()?;
            let def = &p.enums[e.index()];
            let v = def.variants.get(variant as usize)?;
            let fields = v.fields.iter().map(|f| {
                let path = format!(".{}.{}", v.name, f.name);
                let (_, s) = parts.iter().find(|(q, _)| *q == path)?;
                read(enc, f.ty, s, &at(&path), env)
            });
            Value::Enum { variant, fields: fields.collect::<Option<_>>()? }
        }
        _ => return None,
    })
}

/// Ein Blatt als Wert seines Typs: ein `u64` steht im Modell als Bitmuster.
fn scalar(p: &Program, ty: TypeId, shape: &Shape, v: &Val) -> Option<Value> {
    if !matches!(shape, Shape::Leaf(_)) {
        return None;
    }
    Some(match (p.types.get(ty), v) {
        (Type::Bool, Val::Bool(b)) => Value::Bool(*b),
        (Type::Int { width, .. }, Val::Int(x)) if width.signed() => Value::Int(*x),
        (Type::Int { .. }, Val::Int(x)) => Value::UInt(*x as u64),
        (Type::Duration { .. }, Val::Int(x)) => Value::Duration(*x),
        (Type::Enum(_), Val::Int(x)) => Value::Enum { variant: u32::try_from(*x).ok()?, fields: Vec::new() },
        (Type::Float { width: FloatWidth::F32, .. }, Val::F32(x)) => Value::F32(*x),
        (Type::Float { width: FloatWidth::F64, .. }, Val::F64(x)) => Value::F64(*x),
        _ => return None,
    })
}

fn float(v: &Val) -> Option<Value> {
    match v {
        Val::F32(x) => Some(Value::F32(*x)),
        Val::F64(x) => Some(Value::F64(*x)),
        _ => None,
    }
}

/// Schreibt die Blaetter von `v` — ohne Wert null — nach `out`.
fn write(
    enc: &Enc<'_>,
    ty: TypeId,
    shape: &Shape,
    v: Option<&Value>,
    base: &str,
    out: &mut Vec<(String, Val)>,
) -> Option<()> {
    let p = enc.p;
    let at = |path: &str| format!("{base}{path}");
    let Shape::Node(parts) = shape else {
        let sort = enc.leaf_sort(shape, Span::default()).ok()?;
        out.push((base.to_string(), v.map_or(Some(Val::zero(sort)), |v| leaf(v, sort))?));
        return Some(());
    };
    // Die Teile, die `v` belegt, je mit ihrem Wert; die uebrigen null.
    let put = |i: usize, t: TypeId, item: Option<&Value>, out: &mut Vec<(String, Val)>| {
        let (path, s) = parts.get(i)?;
        write(enc, t, s, item, &at(path), out)
    };
    match (p.types.get(ty), v) {
        (_, None) => {
            for (path, s) in parts {
                zero(enc, s, &at(path), out)?;
            }
        }
        (Type::Record(r), Some(Value::Record(values))) => {
            for (i, f) in p.records[r.index()].fields.iter().enumerate() {
                put(i, f.ty, Some(values.get(i)?), out)?;
            }
        }
        (Type::Array { elem, .. }, Some(Value::Array(values) | Value::Samples(values)))
        | (Type::Samples { elem, .. }, Some(Value::Array(values) | Value::Samples(values))) => {
            for i in 0..parts.len() {
                put(i, *elem, Some(values.get(i)?), out)?;
            }
        }
        // 8.9: `[t, pre, post, rate, samples]`; fehlende Abtastwerte sind null.
        (Type::Capture { elem, .. }, Some(Value::Record(values))) => {
            for (i, (path, s)) in parts.iter().take(4).enumerate() {
                let sort = enc.leaf_sort(s, Span::default()).ok()?;
                out.push((at(path), leaf(values.get(i)?, sort)?));
            }
            let (path, Shape::Node(items)) = parts.get(4)? else { return None };
            let Some(Value::Array(samples)) = values.get(4) else { return None };
            for (j, (q, s)) in items.iter().enumerate() {
                write(enc, *elem, s, samples.get(j), &format!("{}{q}", at(path)), out)?;
            }
        }
        (Type::Mat { .. }, Some(Value::Mat { data, .. })) => {
            for (i, (path, s)) in parts.iter().enumerate() {
                let sort = enc.leaf_sort(s, Span::default()).ok()?;
                out.push((at(path), leaf(data.get(i)?, sort)?));
            }
        }
        (Type::Bytes { .. }, Some(Value::Bytes(b))) => text(parts, b, None, base, out)?,
        (Type::Str { .. }, Some(Value::Str(s))) => text(parts, s.as_bytes(), None, base, out)?,
        (Type::Line { .. }, Some(Value::Line { text: s, truncated })) => {
            text(parts, s.as_bytes(), Some(*truncated), base, out)?
        }
        (Type::Vec { elem, .. }, Some(Value::Vec(values))) => {
            out.push((at(".len"), Val::Int(values.len() as i64)));
            for i in 1..parts.len() {
                put(i, *elem, values.get(i - 1), out)?;
            }
        }
        (Type::Optional(t), Some(Value::Optional(inner))) => {
            out.push((at(".has"), Val::Bool(inner.is_some())));
            put(1, *t, inner.as_deref(), out)?;
        }
        (Type::Enum(e), Some(Value::Enum { variant, fields })) if matches!(parts.first(), Some((_, Shape::Tag(_)))) => {
            out.push((at(".tag"), Val::Int(i64::from(*variant))));
            let def = &p.enums[e.index()];
            for (j, v) in def.variants.iter().enumerate() {
                for (k, f) in v.fields.iter().enumerate() {
                    let path = format!(".{}.{}", v.name, f.name);
                    let i = parts.iter().position(|(q, _)| *q == path)?;
                    put(i, f.ty, fields.get(k).filter(|_| j == *variant as usize), out)?;
                }
            }
        }
        _ => return None,
    }
    Some(())
}

/// Text oder Bytes: die Laenge, die Bytes, dahinter null, bei einer Zeile
/// `truncated`.
fn text(
    parts: &[(String, Shape)],
    bytes: &[u8],
    truncated: Option<bool>,
    base: &str,
    out: &mut Vec<(String, Val)>,
) -> Option<()> {
    let cap = parts.iter().filter(|(_, s)| matches!(s, Shape::Byte)).count();
    if bytes.len() > cap {
        return None;
    }
    out.push((format!("{base}.len"), Val::Int(bytes.len() as i64)));
    for i in 0..cap {
        out.push((format!("{base}[{i}]"), Val::Int(bytes.get(i).copied().map_or(0, i64::from))));
    }
    if let Some(t) = truncated {
        out.push((format!("{base}.truncated"), Val::Bool(t)));
    }
    Some(())
}

/// Null in jedem Blatt unter `shape`.
fn zero(enc: &Enc<'_>, shape: &Shape, base: &str, out: &mut Vec<(String, Val)>) -> Option<()> {
    let mut locs = Vec::new();
    Enc::leaf_locs(base, shape, &mut locs);
    for (loc, s) in locs {
        out.push((loc, Val::zero(enc.leaf_sort(&s, Span::default()).ok()?)));
    }
    Some(())
}

/// Ein skalarer Wert in der Sorte seines Blatts.
fn leaf(v: &Value, sort: Sort) -> Option<Val> {
    Some(match (v, sort) {
        (Value::Bool(b), Sort::Bool) => Val::Bool(*b),
        (Value::Int(x) | Value::Duration(x), Sort::Int) => Val::Int(*x),
        (Value::UInt(x), Sort::Int) => Val::Int(*x as i64),
        (Value::Enum { variant, fields }, Sort::Int) if fields.is_empty() => Val::Int(i64::from(*variant)),
        (Value::F32(x), Sort::F32) => Val::F32(*x),
        (Value::F64(x), Sort::F64) => Val::F64(*x),
        _ => return None,
    })
}
