//! Die Werte des Stimulus in der Form, in der die Treiber des Testwirts sie
//! liefern (12.6, [`crate::testhost`]): ein Skalar als C-Literal, ein
//! zusammengesetzter Wert in seiner Speicherform, ein Stromelement als
//! Bytes. Was der Rand daraus macht, entscheidet der Produktrahmen
//! (`takt_frame::edge`).

use takt_frame::edge::double;
use takt_interp::Value;
use takt_llvm::ty::LlvmType;
use takt_mir::program::Program;
use takt_mir::types::Type;

/// Die Bytes eines Stromelements, wie der Treiber sie liefert; ob sie ein
/// Wert sind, prueft der Rand (Zeile 5).
///
/// Dieselbe Lesart wie `element_value` im Interpreter: Record, Capture,
/// Text und Bytes duerfen in ihrer Drahtform stehen (`0x…`), sonst gilt die
/// Schreibweise des Typs — ein Text in Anfuehrungszeichen mit seinen
/// Escapes, ohne sie wie geschrieben. Eine Zeile geht ungekuerzt an den
/// Rand: `line<N>` kuerzt erst er auf `N` und merkt es fuer `.truncated`
/// (3.9, KON2-028), wie auf dem Board. `str` und `bytes` kuerzt schon das
/// Abtasten.
pub(crate) fn element_bytes(p: &Program, elem: takt_mir::TypeId, text: &str) -> Option<Vec<u8>> {
    let ty = p.types.list.get(elem.index());
    let wire = matches!(ty, Some(Type::Record(_) | Type::Capture { .. } | Type::Line { .. } | Type::Bytes { .. }));
    if let (true, Some(hex)) = (wire, text.trim().strip_prefix("0x")) {
        let mut bytes: Vec<u8> =
            (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
        if let Some(Type::Bytes { cap }) = ty {
            bytes.truncate(*cap as usize);
        }
        return Some(bytes);
    }
    if let Some(Type::Line { .. }) = ty {
        let text = text.trim();
        return Some(if text.starts_with('"') { unquoted(text)? } else { text.to_string() }.into_bytes());
    }
    match takt_interp::trace::parse_value(text, elem, p).ok()? {
        takt_interp::Value::Line { text, .. } | takt_interp::Value::Str(text) => Some(text.into_bytes()),
        value => takt_interp::bytes::encode(p, &value, elem).ok(),
    }
}

/// Ein Text in Anfuehrungszeichen, wie der Trace ihn schreibt
/// (`grammar/trace.md`), ohne seine Escapes und ungekuerzt. Gelesen vom
/// Leser des Interpreters (die Zeile `log` traegt denselben Text), damit es
/// nur eine Lesart der Escapes gibt; `parse_value` kuerzte schon auf den Typ.
fn unquoted(text: &str) -> Option<String> {
    let trace = takt_interp::Trace::parse(&format!("t=0 log m {text}")).ok()?;
    match &trace.lines.first()?.kind {
        takt_interp::trace::LineKind::Log { text, .. } => Some(text.clone()),
        _ => None,
    }
}

/// Ein Wert in der Speicherform seines Typs, wie der erzeugte Code ihn im
/// Abbild liest (`takt_llvm::ty::lower`, little-endian wie der Wirt): Felder
/// an ihrem natuerlich ausgerichteten Versatz, ein Enum mit Feldern als
/// Diskriminante und 8-Byte-Faecher (Ganzzahlen erweitert, Gleitkomma
/// bitgleich, `expr::into_slot`).
pub(crate) fn memory(p: &Program, ty: takt_mir::TypeId, llvm: &LlvmType, v: &Value) -> Option<Vec<u8>> {
    let mut out = vec![0u8; usize::try_from(llvm.aligned_size()).ok()?];
    write(p, ty, llvm, v, &mut out)?;
    Some(out)
}

fn write(p: &Program, ty: takt_mir::TypeId, llvm: &LlvmType, v: &Value, buf: &mut [u8]) -> Option<()> {
    let put = |buf: &mut [u8], bytes: &[u8]| -> Option<()> {
        buf.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(())
    };
    match (llvm, v) {
        (LlvmType::Int(bits), _) if !matches!(v, Value::Enum { fields, .. } if !fields.is_empty()) => {
            let x = scalar_bits(p, ty, v)?;
            put(buf, &x.to_le_bytes()[..bits.div_ceil(8) as usize])
        }
        (LlvmType::F32, Value::F32(f)) => put(buf, &f.to_le_bytes()),
        (LlvmType::F64, Value::F64(f)) => put(buf, &f.to_le_bytes()),
        (LlvmType::Struct(_), Value::Record(fields)) => {
            let Type::Record(r) = p.types.get(ty) else { return None };
            let defs = &p.records[r.index()].fields;
            let LlvmType::Struct(parts) = llvm else { return None };
            for (i, (f, value)) in defs.iter().zip(fields).enumerate() {
                let at = usize::try_from(llvm.field_offset(i)).ok()?;
                write(p, f.ty, parts.get(i)?, value, buf.get_mut(at..)?)?;
            }
            Some(())
        }
        (LlvmType::Struct(parts), Value::Enum { variant, fields }) => {
            let Type::Enum(e) = p.types.get(ty) else { return None };
            let def = p.enums[e.index()].variants.get(*variant as usize)?;
            put(buf, &i32::try_from(def.discriminant).ok()?.to_le_bytes())?;
            let base = usize::try_from(llvm.field_offset(1)).ok()?;
            let _ = parts;
            for (i, (f, value)) in def.fields.iter().zip(fields).enumerate() {
                let slot: u64 = match value {
                    Value::F64(x) => x.to_bits(),
                    Value::F32(x) => u64::from(x.to_bits()),
                    other => scalar_bits(p, f.ty, other)? as u64,
                };
                put(buf.get_mut(base + 8 * i..)?, &slot.to_le_bytes())?;
            }
            Some(())
        }
        (LlvmType::Array(elem, _), Value::Array(items) | Value::Samples(items)) => {
            let item = match p.types.get(ty) {
                Type::Array { elem, .. } | Type::Samples { elem, .. } => *elem,
                _ => return None,
            };
            let stride = usize::try_from(elem.aligned_size()).ok()?;
            for (i, value) in items.iter().enumerate() {
                write(p, item, elem, value, buf.get_mut(i * stride..)?)?;
            }
            Some(())
        }
        _ => None,
    }
}

/// Eine Ganzzahl, eine Dauer, ein Wahrheitswert oder ein Enum ohne Felder
/// als Bitmuster in 64 Bit: vorzeichenbehaftet erweitert, wie die
/// Speicherform es abschneidet.
fn scalar_bits(p: &Program, ty: takt_mir::TypeId, v: &Value) -> Option<i64> {
    Some(match v {
        Value::Int(n) | Value::Duration(n) => *n,
        Value::UInt(u) => *u as i64,
        Value::Bool(b) => i64::from(*b),
        Value::Enum { variant, fields } if fields.is_empty() => {
            let Type::Enum(e) = p.types.get(ty) else { return None };
            p.enums[e.index()].variants.get(*variant as usize)?.discriminant
        }
        _ => return None,
    })
}

/// Ein Skalar als C-Literal.
pub(crate) fn literal(p: &Program, ty: takt_mir::TypeId, v: &Value) -> Option<String> {
    Some(match v {
        Value::Int(n) | Value::Duration(n) => format!("{n}LL"),
        Value::UInt(u) => format!("{u}ULL"),
        Value::F64(f) => double(*f),
        Value::F32(f) => format!("(float){}", double(f64::from(*f))),
        Value::Bool(b) => u8::from(*b).to_string(),
        Value::Enum { variant, .. } => {
            let Type::Enum(e) = p.types.get(ty) else { return None };
            p.enums.get(e.index())?.variants.get(*variant as usize)?.discriminant.to_string()
        }
        _ => return None,
    })
}
