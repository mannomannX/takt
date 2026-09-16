//! `Value` in die kanonische Byteform und zurueck (5.9).
//!
//! Die Form selbst steht in `takt_mir::bytes`; hier steht nur,
//! wie ein `Value` sich in ihre Bestandteile zerlegt. Der Codegen zerlegt
//! dieselbe Struktur ueber LLVM-Register, damit beide Seiten dieselben
//! Bytes erzeugen (Satz 9.4.4).

use takt_mir::TypeId;
use takt_mir::bytes::{Decoder, Encoder, Error};
use takt_mir::program::Program;
use takt_mir::types::{FloatWidth, Type};

use crate::value::Value;

/// Kodiert einen Wert in die kanonische Form.
pub fn encode(p: &Program, v: &Value, ty: TypeId) -> Result<Vec<u8>, Error> {
    let mut out = Encoder::new();
    write(p, v, ty, &mut out, 0)?;
    Ok(out.bytes)
}

/// Liest einen Wert aus der kanonischen Form.
///
/// Ueberzaehlige Bytes sind ein Fehler: Ein Eintrag, der laenger ist als
/// sein Typ, passt nicht zu ihm.
pub fn decode(p: &Program, bytes: &[u8], ty: TypeId) -> Result<Value, Error> {
    let mut d = Decoder::new(bytes);
    let v = read(p, ty, &mut d, 0)?;
    if d.is_empty() { Ok(v) } else { Err(Error::Malformed) }
}

fn write(p: &Program, v: &Value, ty: TypeId, out: &mut Encoder, depth: u32) -> Result<(), Error> {
    if depth > 32 {
        return Err(Error::NotPod);
    }
    let t = p.types.list.get(ty.index()).ok_or(Error::NotPod)?;
    match (t, v) {
        (Type::Bool, Value::Bool(b)) => out.bool(*b),
        (Type::Int { width, .. }, Value::Int(n)) => out.int(*n, *width),
        (Type::Int { width, .. }, Value::UInt(n)) => out.int(*n as i64, *width),
        (Type::Float { width: FloatWidth::F32, .. }, Value::F32(f)) => {
            out.float(u64::from(f.to_bits()), FloatWidth::F32);
        }
        (Type::Float { width: FloatWidth::F64, .. }, Value::F64(f)) => out.float(f.to_bits(), FloatWidth::F64),
        (Type::Duration { .. }, Value::Duration(ns)) => out.duration(*ns),
        (Type::Enum(e), Value::Enum { variant, fields }) => {
            let def = p.enums.get(e.index()).ok_or(Error::NotPod)?;
            let var = def.variants.get(*variant as usize).ok_or(Error::Malformed)?;
            if var.fields.len() != fields.len() {
                return Err(Error::Malformed);
            }
            out.discriminant(var.discriminant);
            for (f, value) in var.fields.iter().zip(fields) {
                write(p, value, f.ty, out, depth + 1)?;
            }
        }
        (Type::Record(r), Value::Record(fields)) => {
            let def = p.records.get(r.index()).ok_or(Error::NotPod)?;
            if def.fields.len() != fields.len() {
                return Err(Error::Malformed);
            }
            for (f, value) in def.fields.iter().zip(fields) {
                write(p, value, f.ty, out, depth + 1)?;
            }
        }
        (Type::Array { elem, len }, Value::Array(items)) => {
            if items.len() != *len as usize {
                return Err(Error::Malformed);
            }
            for i in items {
                write(p, i, *elem, out, depth + 1)?;
            }
        }
        (Type::Bytes { cap }, Value::Bytes(b)) => {
            if b.len() > *cap as usize {
                return Err(Error::Malformed);
            }
            out.len(b.len() as u32);
            out.raw(b);
        }
        (Type::Str { cap }, Value::Str(s)) => {
            let b = s.as_bytes();
            if s.chars().count() > *cap as usize {
                return Err(Error::Malformed);
            }
            out.len(b.len() as u32);
            out.raw(b);
        }
        (Type::Vec { elem, cap }, Value::Vec(items)) => {
            if items.len() > *cap as usize {
                return Err(Error::Malformed);
            }
            out.len(items.len() as u32);
            for i in items {
                write(p, i, *elem, out, depth + 1)?;
            }
        }
        (Type::Map { key, value, cap }, Value::Map(items)) => {
            if items.len() > *cap as usize {
                return Err(Error::Malformed);
            }
            out.len(items.len() as u32);
            for (k, val) in items {
                write(p, k, *key, out, depth + 1)?;
                write(p, val, *value, out, depth + 1)?;
            }
        }
        _ => return Err(Error::NotPod),
    }
    Ok(())
}

fn read(p: &Program, ty: TypeId, d: &mut Decoder<'_>, depth: u32) -> Result<Value, Error> {
    if depth > 32 {
        return Err(Error::NotPod);
    }
    let t = p.types.list.get(ty.index()).ok_or(Error::NotPod)?;
    let v = match t {
        Type::Bool => Value::Bool(d.bool()?),
        Type::Int { width, .. } => {
            let n = d.int(*width)?;
            if width.signed() { Value::Int(n) } else { Value::UInt(n as u64) }
        }
        Type::Float { width: FloatWidth::F32, .. } => Value::F32(f32::from_bits(d.float(FloatWidth::F32)? as u32)),
        Type::Float { width: FloatWidth::F64, .. } => Value::F64(f64::from_bits(d.float(FloatWidth::F64)?)),
        Type::Duration { .. } => Value::Duration(d.duration()?),
        Type::Enum(e) => {
            let def = p.enums.get(e.index()).ok_or(Error::NotPod)?;
            let disc = d.discriminant()?;
            let index = def.variants.iter().position(|v| v.discriminant == disc).ok_or(Error::Malformed)?;
            let mut fields = Vec::with_capacity(def.variants[index].fields.len());
            for f in &def.variants[index].fields {
                fields.push(read(p, f.ty, d, depth + 1)?);
            }
            Value::Enum { variant: index as u32, fields }
        }
        Type::Record(r) => {
            let def = p.records.get(r.index()).ok_or(Error::NotPod)?;
            let mut fields = Vec::with_capacity(def.fields.len());
            for f in &def.fields {
                fields.push(read(p, f.ty, d, depth + 1)?);
            }
            Value::Record(fields)
        }
        Type::Array { elem, len } => {
            let mut items = Vec::with_capacity(*len as usize);
            for _ in 0..*len {
                items.push(read(p, *elem, d, depth + 1)?);
            }
            Value::Array(items)
        }
        Type::Bytes { cap } => {
            let n = d.len(*cap)?;
            Value::Bytes(d.raw(n as usize)?.to_vec())
        }
        Type::Str { cap } => {
            // Die Laenge zaehlt Bytes, `cap` zaehlt Zeichen (3.9): Ein
            // mehrbyteiges Zeichen darf die Byte-Schranke ueberschreiten.
            let n = d.len(cap.saturating_mul(4))?;
            let raw = d.raw(n as usize)?;
            let s = core::str::from_utf8(raw).map_err(|_| Error::Malformed)?;
            if s.chars().count() > *cap as usize {
                return Err(Error::Malformed);
            }
            Value::Str(s.to_string())
        }
        Type::Vec { elem, cap } => {
            let n = d.len(*cap)?;
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n {
                items.push(read(p, *elem, d, depth + 1)?);
            }
            Value::Vec(items)
        }
        Type::Map { key, value, cap } => {
            let n = d.len(*cap)?;
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let k = read(p, *key, d, depth + 1)?;
                let val = read(p, *value, d, depth + 1)?;
                items.push((k, val));
            }
            Value::Map(items)
        }
        _ => return Err(Error::NotPod),
    };
    Ok(v)
}
