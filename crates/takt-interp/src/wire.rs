//! Drahtformat der Records (Referenz 3.7): `R.decode(b) -> R?` und
//! `f.encode() -> bytes<SIZE>`.
//!
//! Der Plan steht im Typ (`FieldDef::offset`, `RecordDef::wire_size`,
//! `WireLayout::endian`); das Sema hat ihn beim Registrieren gerechnet und
//! geprueft (Pruefung 46). Hier ist nur noch die Umsetzung je Richtung
//! (plan/m2.md 1.8). `decode` liefert `none` bei zu kurzem Puffer,
//! Konstantenverstoss oder Range-Verletzung eines Feldes; es faultet nie.

use takt_mir::program::Program;
use takt_mir::types::{BitfieldDef, Const, Endian, FloatWidth, IntWidth, Type};
use takt_mir::{RecordId, TypeId};

use crate::loaded::Loaded;
use crate::value::Value;

/// `R.decode(b)`: `Some(record)` oder `None` (3.7).
pub fn decode(loaded: &Loaded<'_>, record: RecordId, bytes: &[u8]) -> Option<Value> {
    let def = &loaded.program.records[record.index()];
    let endian = def.layout.as_ref()?.endian;
    let size = def.wire_size? as usize;
    if bytes.len() < size {
        return None;
    }
    let mut fields = Vec::with_capacity(def.fields.len());
    for f in &def.fields {
        let at = f.offset? as usize;
        let width = field_size(loaded, f.ty)? as usize;
        let raw = bytes.get(at..at + width)?;
        let value = read(loaded, f.ty, raw, endian)?;
        // Ein Konstantenfeld muss den deklarierten Wert tragen (3.7).
        if let Some(want) = &f.const_value {
            if !same_const(&value, want) {
                return None;
            }
        }
        // Eine Range-Verletzung macht `decode` zu `none`, nicht zu einem Fault.
        if let Some(range) = range_of(loaded, f.ty) {
            if !crate::eval::in_range(&value, &range) {
                return None;
            }
        }
        fields.push(value);
    }
    Some(Value::Record(fields))
}

/// `f.encode()`: der Record als Bytes seiner deklarierten Laenge (3.7).
/// Konstantenfelder werden dabei gesetzt.
pub fn encode(loaded: &Loaded<'_>, record: RecordId, value: &Value) -> Option<Vec<u8>> {
    let def = &loaded.program.records[record.index()];
    let endian = def.layout.as_ref()?.endian;
    let size = def.wire_size? as usize;
    let Value::Record(fields) = value else { return None };
    let mut out = vec![0u8; size];
    for (i, f) in def.fields.iter().enumerate() {
        let at = f.offset? as usize;
        let width = field_size(loaded, f.ty)? as usize;
        let v = match &f.const_value {
            Some(c) => const_value(c),
            None => fields.get(i)?.clone(),
        };
        let slot = out.get_mut(at..at + width)?;
        write(loaded, f.ty, &v, endian, slot)?;
    }
    Some(out)
}

/// Groesse eines Feldtyps in Bytes; dieselbe Rechnung wie im Sema.
fn field_size(loaded: &Loaded<'_>, ty: TypeId) -> Option<u32> {
    match loaded.ty(ty) {
        Type::Bool => Some(1),
        Type::Int { width, .. } => Some(width.bits() / 8),
        Type::Float { width, .. } => Some(match width {
            FloatWidth::F32 => 4,
            FloatWidth::F64 => 8,
        }),
        Type::Bytes { cap } => Some(*cap),
        Type::Array { elem, len } => field_size(loaded, *elem).map(|n| n * len),
        Type::Enum(e) => Some(loaded.program.enums[e.index()].layout.map_or(1, |w| w.bits() / 8)),
        Type::Record(r) => loaded.program.records[r.index()].wire_size,
        _ => None,
    }
}

/// Liest einen Wert aus seinem Byteausschnitt.
fn read(loaded: &Loaded<'_>, ty: TypeId, raw: &[u8], endian: Endian) -> Option<Value> {
    match loaded.ty(ty) {
        Type::Bool => Some(Value::Bool(raw.first()? != &0)),
        Type::Int { width, .. } => {
            let n = uint(raw, endian);
            Some(if width.signed() { Value::Int(sign_extend(n, *width)) } else { Value::UInt(n) })
        }
        Type::Float { width, .. } => Some(match width {
            FloatWidth::F32 => Value::F32(f32::from_bits(uint(raw, endian) as u32)),
            FloatWidth::F64 => Value::F64(f64::from_bits(uint(raw, endian))),
        }),
        Type::Bytes { .. } => Some(Value::Bytes(raw.to_vec())),
        Type::Array { elem, len } => {
            let width = field_size(loaded, *elem)? as usize;
            let mut out = Vec::with_capacity(*len as usize);
            for i in 0..*len as usize {
                out.push(read(loaded, *elem, raw.get(i * width..(i + 1) * width)?, endian)?);
            }
            Some(Value::Array(out))
        }
        Type::Enum(e) => {
            let n = uint(raw, endian);
            // Der Wert muss eine deklarierte Diskriminante treffen (3.7).
            let variants = &loaded.program.enums[e.index()].variants;
            let index = variants.iter().position(|v| v.discriminant == n as i64)?;
            Some(Value::Enum { variant: index as u32, fields: Vec::new() })
        }
        Type::Record(r) => decode(loaded, *r, raw),
        _ => None,
    }
}

/// Schreibt einen Wert in seinen Byteausschnitt.
fn write(loaded: &Loaded<'_>, ty: TypeId, v: &Value, endian: Endian, slot: &mut [u8]) -> Option<()> {
    match loaded.ty(ty) {
        Type::Bool => {
            slot[0] = u8::from(matches!(v, Value::Bool(true)));
            Some(())
        }
        Type::Int { .. } => {
            let n = match v {
                Value::Int(i) => *i as u64,
                Value::UInt(u) => *u,
                _ => return None,
            };
            put(slot, n, endian);
            Some(())
        }
        Type::Float { width, .. } => {
            let bits = match (width, v) {
                (FloatWidth::F32, Value::F32(f)) => u64::from(f.to_bits()),
                (FloatWidth::F32, Value::F64(f)) => u64::from((*f as f32).to_bits()),
                (FloatWidth::F64, Value::F64(f)) => f.to_bits(),
                (FloatWidth::F64, Value::F32(f)) => f64::from(*f).to_bits(),
                _ => return None,
            };
            put(slot, bits, endian);
            Some(())
        }
        Type::Bytes { .. } => {
            let Value::Bytes(b) = v else { return None };
            // Kuerzere Werte werden mit Nullen aufgefuellt (feste Feldbreite).
            let n = b.len().min(slot.len());
            slot[..n].copy_from_slice(&b[..n]);
            Some(())
        }
        Type::Array { elem, len } => {
            let Value::Array(items) = v else { return None };
            let width = field_size(loaded, *elem)? as usize;
            for i in 0..*len as usize {
                let value = items.get(i).cloned().unwrap_or_else(|| Value::default_for(*elem, loaded.program));
                write(loaded, *elem, &value, endian, slot.get_mut(i * width..(i + 1) * width)?)?;
            }
            Some(())
        }
        Type::Enum(e) => {
            let Value::Enum { variant, .. } = v else { return None };
            let d = loaded.program.enums[e.index()].variants.get(*variant as usize)?.discriminant;
            put(slot, d as u64, endian);
            Some(())
        }
        Type::Record(r) => {
            let bytes = encode(loaded, *r, v)?;
            let n = bytes.len().min(slot.len());
            slot[..n].copy_from_slice(&bytes[..n]);
            Some(())
        }
        _ => None,
    }
}

/// Bytes als vorzeichenlose Zahl in der Byte-Reihenfolge des Layouts.
fn uint(raw: &[u8], endian: Endian) -> u64 {
    let mut n = 0u64;
    match endian {
        Endian::Little => {
            for (i, b) in raw.iter().take(8).enumerate() {
                n |= u64::from(*b) << (8 * i);
            }
        }
        Endian::Big => {
            for b in raw.iter().take(8) {
                n = (n << 8) | u64::from(*b);
            }
        }
    }
    n
}

/// Schreibt eine Zahl in die Bytes des Feldes.
fn put(slot: &mut [u8], n: u64, endian: Endian) {
    let len = slot.len().min(8);
    for (i, byte) in slot.iter_mut().take(len).enumerate() {
        let shift = match endian {
            Endian::Little => 8 * i,
            Endian::Big => 8 * (len - 1 - i),
        };
        *byte = ((n >> shift) & 0xFF) as u8;
    }
}

/// Vorzeichenerweiterung einer gelesenen Zahl auf ihre Breite.
fn sign_extend(n: u64, width: IntWidth) -> i64 {
    let bits = width.bits();
    if bits >= 64 {
        return n as i64;
    }
    let sign = 1u64 << (bits - 1);
    if n & sign == 0 { n as i64 } else { (n | !((1u64 << bits) - 1)) as i64 }
}

fn range_of(loaded: &Loaded<'_>, ty: TypeId) -> Option<takt_mir::types::Range> {
    match loaded.ty(ty) {
        Type::Int { range, .. } | Type::Float { range, .. } | Type::Duration { range } => *range,
        _ => None,
    }
}

fn const_value(c: &Const) -> Value {
    match c {
        Const::Int(i) => Value::Int(*i),
        Const::Float(f) => Value::F64(*f),
        Const::Duration(d) => Value::Duration(*d),
        Const::Bool(b) => Value::Bool(*b),
    }
}

fn same_const(v: &Value, c: &Const) -> bool {
    match (v, c) {
        (Value::Int(a), Const::Int(b)) => a == b,
        (Value::UInt(a), Const::Int(b)) => i64::try_from(*a).is_ok_and(|a| a == *b),
        (Value::Bool(a), Const::Bool(b)) => a == b,
        (Value::F32(a), Const::Float(b)) => f64::from(*a) == *b,
        (Value::F64(a), Const::Float(b)) => a == b,
        (Value::Duration(a), Const::Duration(b)) => a == b,
        _ => false,
    }
}

/// Wert eines Bitfelds aus seinem Traegerfeld (3.7).
pub fn read_bits(carrier: u64, bits: &BitfieldDef) -> u64 {
    let width = u32::from(bits.hi - bits.lo) + 1;
    let mask = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
    (carrier >> bits.lo) & mask
}

/// Setzt ein Bitfeld in seinem Traegerfeld (3.7).
pub fn write_bits(carrier: u64, bits: &BitfieldDef, value: u64) -> u64 {
    let width = u32::from(bits.hi - bits.lo) + 1;
    let mask = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
    (carrier & !(mask << bits.lo)) | ((value & mask) << bits.lo)
}

/// Groesse eines Records im Drahtformat, fuer `encode` und `bytes<SIZE>`.
pub fn record_size(p: &Program, record: RecordId) -> Option<u32> {
    p.records[record.index()].wire_size
}
