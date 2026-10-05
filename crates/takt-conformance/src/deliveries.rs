//! Die Lieferungen des Stimulus an den Treiberrand (12.6) im Wirtsrahmen.
//!
//! Auf dem Board liefern Treiber, auf dem Wirt der Stimulus: Seine
//! `in`-Zeilen werden hier zu Aufrufen von `takt_edge_reading` und
//! `takt_edge_element`, gerichtet an denselben Rand wie auf dem Board
//! (`takt_frame::edge`).

use std::fmt::Write as _;

use takt_frame::edge::{KIND_FLOAT, KIND_INT, KIND_NONE, double};
use takt_frame::layout::{Layout, c_type};
use takt_llvm::symbols::Prefix;
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
        // Eine Lieferung nur mit Qualitaet braucht keinen Wert und darum keine
        // C-Form; einen Wert zusammengesetzten Typs liefert der Rahmen nicht
        // (LIMITS), und das bricht den Bau, statt still zu fehlen.
        let value = match &sample.value {
            Some(v) => match (c_type(&slot.ty, slot.signed), number(p, ty, v)) {
                (Some(ct), Some(number)) => Some((ct, number)),
                _ => {
                    entry.0.push(format!(
                        "#error \"Lieferung an `{channel}` in Tick {tick}: kein skalarer Wert (LIMITS)\""
                    ));
                    continue;
                }
            },
            None => None,
        };
        let call = match value {
            Some((ct, (literal, kind, i, f))) => format!(
                "takt_edge_reading(a, {c}, &({ct}){{ {literal} }}, (int)sizeof({ct}), {kind}, {i}LL, {}, {quality}, {reason}, 1, {t}LL, {}LL);",
                double(f),
                sample.age
            ),
            None => {
                format!(
                    "takt_edge_reading(a, {c}, 0, 0, 0, 0LL, 0.0, {quality}, {reason}, 0, {t}LL, {}LL);",
                    sample.age
                )
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
/// Dieselbe Lesart wie `element_value` im Interpreter: Record, Text und
/// Bytes duerfen in ihrer Drahtform stehen (`0x…`), sonst gilt die
/// Schreibweise des Typs — ein Text in Anfuehrungszeichen mit seinen
/// Escapes, ohne sie wie geschrieben. Eine Zeile geht ungekuerzt an den
/// Rand: `line<N>` kuerzt erst er auf `N` und merkt es fuer `.truncated`
/// (3.9, KON2-028), wie auf dem Board. `str` und `bytes` kuerzt schon das
/// Abtasten.
fn element_bytes(p: &Program, elem: takt_mir::TypeId, text: &str) -> Option<Vec<u8>> {
    let ty = p.types.list.get(elem.index());
    let wire = matches!(ty, Some(Type::Record(_) | Type::Line { .. } | Type::Bytes { .. }));
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
