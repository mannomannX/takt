//! Die Lieferungen des Stimulus an den Treiberrand (12.6) im Wirtsrahmen.
//!
//! Auf dem Board liefern Treiber, auf dem Wirt der Stimulus: Seine
//! `in`-Zeilen werden hier zu Aufrufen von `takt_edge_reading` und
//! `takt_edge_element`, gerichtet an denselben Rand wie auf dem Board
//! (`takt_frame::edge`).

use std::fmt::Write as _;

use takt_frame::edge::{KIND_FLOAT, KIND_INT, KIND_NONE, double};
use takt_frame::layout::{Layout, c_type};
use takt_interp::Value;
use takt_llvm::symbols::Prefix;
use takt_llvm::ty::LlvmType;
use takt_mir::program::{Direction, Program};
use takt_mir::types::Type;

use crate::stimulus::Stimulus;

/// Die Lieferungen des Stimulus (`in`-Zeilen) an den Rand, je Tick: die
/// Skalare zuerst, dann die Elemente, je in ihrer Reihenfolge — wie
/// `deliver` im Interpreter. Ohne `t=` gilt die Tickgrenze, ohne `seq=`
/// die naechste Nummer der lueckenlosen Folge; beides steht schon vor dem
/// Lauf fest, denn der Rand gleicht nach jeder Lieferung ab. Ein
/// Record-Element darf als Bytes stehen; ob es einer ist, prueft der
/// Rand (Zeile 5).
///
/// Das Ergebnis ist die Zahl der Lieferungen im vollsten Tick.
pub(crate) fn stimulus(s: &mut String, p: &Program, layout: &Layout, inputs: &[Stimulus], x: &Prefix) -> usize {
    use std::collections::BTreeMap;
    let tick_ns = p.config.tick;
    let mut ticks: BTreeMap<u64, (Vec<String>, Vec<String>)> = BTreeMap::new();
    let mut last_seq: BTreeMap<usize, i64> = BTreeMap::new();
    for stim in inputs {
        let Stimulus::Input { tick, channel, sample } = stim else { continue };
        let Some(c) = p.channels.iter().position(|ch| ch.name == *channel && ch.dir == Direction::Input) else {
            continue;
        };
        let t = sample.t.unwrap_or_else(|| i64::try_from(*tick).unwrap_or(i64::MAX).saturating_mul(tick_ns));
        let ty = p.channels[c].ty;
        let entry = ticks.entry(*tick).or_default();
        if let Some(Type::Stream(elem)) = p.types.list.get(ty.index()) {
            let seq = sample.seq.unwrap_or_else(|| last_seq.get(&c).map_or(0, |s| s.saturating_add(1)));
            last_seq.insert(c, seq);
            let Some(bytes) = element_bytes(p, *elem, sample.value.as_deref().unwrap_or_default()) else {
                entry.1.push(format!("#error \"Element an `{channel}` in Tick {tick}: unlesbar\""));
                continue;
            };
            let text: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
            entry.1.push(format!(
                "takt_edge_element(a, {c}, (const unsigned char *)\"{text}\", {}, {t}LL, {seq}LL); /* {channel} */",
                bytes.len()
            ));
            continue;
        }
        let Some(slot) = layout.inputs.iter().find(|sl| sl.name == *channel) else { continue };
        let Ok(sample) = takt_interp::trace::sample_from_text(sample, ty, p) else {
            entry.0.push(format!("#error \"Lieferung an `{channel}` in Tick {tick}: unlesbar\""));
            continue;
        };
        let quality = match sample.quality {
            takt_interp::value::Quality::Good => 0,
            takt_interp::value::Quality::Suspect => 1,
            takt_interp::value::Quality::Stale => 2,
            takt_interp::value::Quality::Bad => 3,
        };
        let reason = match sample.reason {
            Some(takt_interp::value::Reason::OutOfRange) => 1,
            Some(takt_interp::value::Reason::Implausible) => 2,
            Some(takt_interp::value::Reason::Driver) => 3,
            Some(takt_interp::value::Reason::Node) => 4,
            Some(takt_interp::value::Reason::Stale) | None => 0,
        };
        // 8.9: Ein Tick-Array unter `N` Samples gilt nicht — `Stale`, ohne
        // Wert, wie `Image::through_edge`.
        let short = matches!((&sample.value, p.types.get(ty)), (Some(Value::Samples(items)), Type::Samples { len, .. })
            if items.len() < *len as usize);
        // Eine Lieferung nur mit Qualitaet braucht keinen Wert und darum keine
        // C-Form. Ein Wert zusammengesetzten Typs geht in seiner Speicherform
        // an den Rand (FB-430); was der Rahmen nicht abbilden kann, bricht den
        // Bau, statt still zu fehlen.
        let value = match &sample.value {
            Some(_) if short => None,
            Some(v) => match (c_type(&slot.ty, slot.signed), number(p, ty, v)) {
                (Some(ct), Some(number)) => Some(Delivered::Number(ct, number)),
                _ => match memory(p, ty, &slot.ty, v) {
                    Some(bytes) => Some(Delivered::Bytes(bytes)),
                    None => {
                        entry.0.push(format!(
                            "#error \"Lieferung an `{channel}` in Tick {tick}: Wert ohne Speicherform\""
                        ));
                        continue;
                    }
                },
            },
            None => None,
        };
        let (quality, reason, age) = if short { (2, 0, 0) } else { (quality, reason, sample.age) };
        let call = match value {
            Some(Delivered::Number(ct, (literal, kind, i, f))) => format!(
                "takt_edge_reading(a, {c}, &({ct}){{ {literal} }}, (int)sizeof({ct}), {kind}, {i}LL, {}, {quality}, {reason}, 1, {t}LL, {age}LL);",
                double(f)
            ),
            Some(Delivered::Bytes(bytes)) => {
                let text: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
                format!(
                    "takt_edge_reading(a, {c}, (const unsigned char *)\"{text}\", {}, {KIND_NONE}, 0LL, 0.0, {quality}, {reason}, 1, {t}LL, {age}LL);",
                    bytes.len()
                )
            }
            None => {
                format!("takt_edge_reading(a, {c}, 0, 0, 0, 0LL, 0.0, {quality}, {reason}, 0, {t}LL, {age}LL);")
            }
        };
        entry.0.push(format!("{call} /* {channel} */"));
    }
    let _ = writeln!(s, "/* Die Lieferungen des Stimulus an den Treiberrand (12.6). */");
    let _ = writeln!(s, "static void takt_edge_stimulus(struct {x}_arena *a, long long tick) {{");
    let _ = writeln!(s, "    switch (tick) {{");
    for (tick, (readings, elements)) in &ticks {
        let _ = writeln!(s, "    case {tick}:");
        for line in readings.iter().chain(elements) {
            let _ = writeln!(s, "        {line}");
        }
        let _ = writeln!(s, "        break;");
    }
    let _ = writeln!(s, "    default: break;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    takt_edge_commit(a, tick);");
    let _ = writeln!(s, "}}\n");
    ticks.values().map(|(r, e)| r.len() + e.len()).max().unwrap_or(0)
}

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
fn element_bytes(p: &Program, elem: takt_mir::TypeId, text: &str) -> Option<Vec<u8>> {
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

/// Was an den Rand geht: eine Zahl als C-Literal mit ihrer Form fuer das
/// Tor, oder die Bytes eines zusammengesetzten Werts.
enum Delivered {
    Number(&'static str, (String, u8, i64, f64)),
    Bytes(Vec<u8>),
}

/// Ein Wert in der Speicherform seines Typs, wie der erzeugte Code ihn im
/// Abbild liest (`takt_llvm::ty::lower`, little-endian wie der Wirt): Felder
/// an ihrem natuerlich ausgerichteten Versatz, ein Enum mit Feldern als
/// Diskriminante und 8-Byte-Faecher (Ganzzahlen erweitert, Gleitkomma
/// bitgleich, `expr::into_slot`).
fn memory(p: &Program, ty: takt_mir::TypeId, llvm: &LlvmType, v: &Value) -> Option<Vec<u8>> {
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

/// Ein Wert als C-Literal, dazu seine Form fuer das Tor (`Scalar`):
/// Art, `as_i64`, `as_f64`.
fn number(p: &Program, ty: takt_mir::TypeId, v: &takt_interp::Value) -> Option<(String, u8, i64, f64)> {
    use takt_interp::Value;
    Some(match v {
        Value::Int(n) => (format!("{n}LL"), KIND_INT, *n, *n as f64),
        Value::Duration(n) => (format!("{n}LL"), KIND_INT, *n, *n as f64),
        Value::UInt(u) => match i64::try_from(*u) {
            Ok(n) => (format!("{u}ULL"), KIND_INT, n, *u as f64),
            Err(_) => (format!("{u}ULL"), KIND_FLOAT, 0, *u as f64),
        },
        Value::F64(f) => (double(*f), KIND_FLOAT, 0, *f),
        Value::F32(f) => (format!("(float){}", double(f64::from(*f))), KIND_FLOAT, 0, f64::from(*f)),
        Value::Bool(b) => (u8::from(*b).to_string(), KIND_NONE, 0, 0.0),
        Value::Enum { variant, .. } => {
            let Type::Enum(e) = p.types.get(ty) else { return None };
            let d = p.enums.get(e.index())?.variants.get(*variant as usize)?.discriminant;
            (d.to_string(), KIND_NONE, 0, 0.0)
        }
        _ => return None,
    })
}
