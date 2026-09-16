//! Formatierung von Werten in feste Puffer (Referenz 3.9): `{x}`, `{x:hex}`,
//! `{x:.3}`, `{x:08}`; ungueltige Werte werden `<invalid>` (3.5).

use takt_mir::TypeId;
use takt_mir::pattern::{Format, FormatPiece};
use takt_mir::types::Type;

use crate::eval::Ctx;
use crate::value::{Trap, Value};

/// Ein Wert in Textform nach Formatangabe.
pub fn display(v: &Value, spec: Option<&str>, ty: TypeId, ctx: &Ctx<'_, '_>) -> String {
    let p = ctx.loaded.program;
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => match spec {
            Some("hex") => format!("{:x}", *i as u64),
            Some(s) if s.starts_with('0') => format!("{:0width$}", i, width = s.parse().unwrap_or(0)),
            _ => i.to_string(),
        },
        Value::UInt(u) => match spec {
            Some("hex") => format!("{u:x}"),
            Some(s) if s.starts_with('0') => format!("{:0width$}", u, width = s.parse().unwrap_or(0)),
            _ => u.to_string(),
        },
        Value::F32(f) => float32(*f, spec),
        Value::F64(f) => float(*f, spec),
        Value::Duration(d) => takt_mir::dump::duration(*d),
        Value::Enum { variant, fields } => {
            let name = match p.types.get(ty) {
                Type::Enum(id) => p.enums[id.index()].variants.get(*variant as usize).map(|v| v.name.clone()),
                _ => None,
            };
            let name = name.unwrap_or_else(|| format!("V{variant}"));
            if fields.is_empty() {
                name
            } else {
                let field_tys: Vec<TypeId> = match p.types.get(ty) {
                    Type::Enum(id) => {
                        p.enums[id.index()].variants[*variant as usize].fields.iter().map(|f| f.ty).collect()
                    }
                    _ => Vec::new(),
                };
                let parts: Vec<String> = fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| display(f, None, field_tys.get(i).copied().unwrap_or(ty), ctx))
                    .collect();
                format!("{name}({})", parts.join(", "))
            }
        }
        Value::Record(fields) => {
            let (name, tys): (String, Vec<TypeId>) = match p.types.get(ty) {
                Type::Record(id) => {
                    let r = &p.records[id.index()];
                    (r.name.clone(), r.fields.iter().map(|f| f.ty).collect())
                }
                _ => ("record".into(), Vec::new()),
            };
            let parts: Vec<String> = fields
                .iter()
                .enumerate()
                .map(|(i, f)| display(f, None, tys.get(i).copied().unwrap_or(ty), ctx))
                .collect();
            format!("{name}({})", parts.join(", "))
        }
        Value::Array(items) | Value::Vec(items) | Value::Samples(items) => {
            let elem = match p.types.get(ty) {
                Type::Array { elem, .. } | Type::Vec { elem, .. } | Type::Samples { elem, .. } => *elem,
                _ => ty,
            };
            let parts: Vec<String> = items.iter().map(|x| display(x, spec, elem, ctx)).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Bytes(b) => b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "),
        Value::Str(s) => s.clone(),
        Value::Line { text, .. } => text.clone(),
        Value::Optional(None) => "none".into(),
        Value::Optional(Some(inner)) => {
            let elem = match p.types.get(ty) {
                Type::Optional(t) => *t,
                _ => ty,
            };
            display(inner, spec, elem, ctx)
        }
        Value::Result(Ok(inner)) => {
            let elem = match p.types.get(ty) {
                Type::Result { ok, .. } => *ok,
                _ => ty,
            };
            format!("OK({})", display(inner, spec, elem, ctx))
        }
        Value::Result(Err(inner)) => {
            let elem = match p.types.get(ty) {
                Type::Result { err, .. } => {
                    p.types.list.iter().position(|t| *t == Type::Enum(*err)).map(|i| TypeId(i as u32))
                }
                _ => None,
            };
            format!("ERR({})", display(inner, spec, elem.unwrap_or(ty), ctx))
        }
        Value::Mat { rows, cols, data } => {
            let parts: Vec<String> = (0..*rows)
                .map(|r| {
                    let row: Vec<String> =
                        (0..*cols).map(|c| display(&data[(r * cols + c) as usize], spec, ty, ctx)).collect();
                    format!("[{}]", row.join(", "))
                })
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Table(points) => {
            let parts: Vec<String> = points
                .iter()
                .map(|(a, b)| format!("({}, {})", display(a, None, ty, ctx), display(b, None, ty, ctx)))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Map(slots) => {
            let parts: Vec<String> = slots
                .iter()
                .flatten()
                .map(|(a, b)| format!("({}, {})", display(a, None, ty, ctx), display(b, None, ty, ctx)))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Block(_) => "<block>".into(),
        Value::Handle => "<handle>".into(),
    }
}

/// Ein `f32` mit derselben Regel wie `float`, aber ohne Erweiterung nach
/// f64: sonst erschienen die Ziffern der f64-Darstellung (4.1).
fn float32(x: f32, spec: Option<&str>) -> String {
    match spec {
        Some(s) if s.starts_with('.') => format!("{:.prec$}", x, prec = s[1..].parse().unwrap_or(0)),
        _ => {
            if x == x.trunc() && x.abs() < 1e15 {
                format!("{x:.1}")
            } else {
                format!("{x}")
            }
        }
    }
}

fn float(x: f64, spec: Option<&str>) -> String {
    match spec {
        Some(s) if s.starts_with('.') => format!("{:.prec$}", x, prec = s[1..].parse().unwrap_or(0)),
        _ => {
            if x == x.trunc() && x.abs() < 1e15 {
                format!("{x:.1}")
            } else {
                format!("{x}")
            }
        }
    }
}

/// Formatstring rendern: Platzhalter auswerten, Faults werden `<invalid>`,
/// Ergebnis auf `len_max` Bytes gekuerzt (Zeichengrenze).
pub fn render(f: &Format, ctx: &mut Ctx<'_, '_>) -> String {
    let mut out = String::new();
    for piece in &f.pieces {
        match piece {
            FormatPiece::Text(t) => out.push_str(t),
            FormatPiece::Expr { expr, spec } => match ctx.eval(expr) {
                Ok(v) => out.push_str(&display(&v, spec.as_deref(), expr.ty, ctx)),
                Err(Trap::Fault(_)) => out.push_str("<invalid>"),
                Err(Trap::Bug(_)) => out.push_str("<invalid>"),
            },
        }
    }
    truncate(out, f.len_max as usize)
}

/// Kuerzt auf hoechstens `max` Bytes an einer Zeichengrenze.
pub fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}
