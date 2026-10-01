//! Die kanonische Byteform eines Werts (5.9; plan/m6.md 2.2).
//!
//! Sie entstand fuer `persist` und ist die Form aller Grenzen, die
//! Interpreter und nativer Code gleich bilden muessen: das Journal,
//! `map`-Schluessel (3.9), Job-Argumente (4.5), Record-Elemente auf
//! Stroemen (8.6) und Records an nativen Funktionen. `takt-native` traegt
//! das `no_std`-Gegenstueck (`takt_native::bytes`).
//!
//! **Warum eine eigene Form.** `wire::encode` braucht `layout`, und 3.7
//! sagt dazu „Ohne `layout` gibt es keine Byte-Repraesentation" — aber
//! Beispiel 14.7 persistiert `SelftestResult` ohne `layout`. `layout`
//! beschreibt *externe* Formate (Protokolle, C-Structs); das Journal ist
//! intern, nur die Runtime liest es.
//!
//! **Warum kein Speicherabzug.** Der Codegen bildet `bool` auf `i1` ab;
//! im Speicher stehen sieben undefinierte Bits, und das Struct-Layout
//! bestimmt LLVM. Beides macht einen CRC32 unbrauchbar.
//!
//! Die Form ist nicht selbstbeschreibend: Sie laesst sich nur mit dem Typ
//! lesen, den der Typ-Hash bestaetigt. Tags je Wert wuerden die Aussage
//! verdoppeln, die der Hash schon macht.
//!
//! Alles little-endian, ohne Padding. Beide Ziele (Cortex-M4F, RV32IMAC)
//! sind little-endian; eine Wahl waere eine Fehlerquelle ohne Nutzen.

use crate::program::Program;
use crate::types::{FloatWidth, IntWidth, Type};
use crate::{RecordId, TypeId};

/// Schreibt die kanonische Form in einen Puffer.
///
/// `takt-mir` kennt `Value` nicht — der Typ lebt im Interpreter, der von
/// hier abhaengt. Der Kodierer nimmt darum Bestandteile entgegen, nicht
/// Werte; wer sie liefert, weiss sie zu zerlegen.
#[derive(Debug, Default)]
pub struct Encoder {
    /// Die Bytes.
    pub bytes: Vec<u8>,
}

impl Encoder {
    /// Leerer Kodierer.
    pub fn new() -> Encoder {
        Encoder::default()
    }

    /// Ein Wahrheitswert.
    pub fn bool(&mut self, v: bool) {
        self.bytes.push(u8::from(v));
    }

    /// Eine Ganzzahl in ihrer Breite.
    pub fn int(&mut self, v: i64, width: IntWidth) {
        let n = (width.bits() / 8) as usize;
        self.bytes.extend_from_slice(&v.to_le_bytes()[..n]);
    }

    /// Eine Fliesskommazahl als Bitmuster.
    pub fn float(&mut self, bits: u64, width: FloatWidth) {
        match width {
            FloatWidth::F32 => self.bytes.extend_from_slice(&(bits as u32).to_le_bytes()),
            FloatWidth::F64 => self.bytes.extend_from_slice(&bits.to_le_bytes()),
        }
    }

    /// Eine Dauer in Nanosekunden.
    pub fn duration(&mut self, ns: i64) {
        self.bytes.extend_from_slice(&ns.to_le_bytes());
    }

    /// Eine Laenge oder Anzahl (`bytes`, `str`, `vec`, `map`).
    pub fn len(&mut self, n: u32) {
        self.bytes.extend_from_slice(&n.to_le_bytes());
    }

    /// Die Diskriminante einer Variante; immer acht Byte, auch bei
    /// `layout u8`. Die deklarierte Breite gehoert zum Drahtformat; sie
    /// hier zu benutzen koppelte die Journal-Form an eine Angabe, die
    /// jemand fuer ein Protokoll aendern koennte.
    pub fn discriminant(&mut self, d: i64) {
        self.bytes.extend_from_slice(&d.to_le_bytes());
    }

    /// Rohe Bytes eines `bytes<N>`.
    pub fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
}

/// Fehler beim Lesen oder Schreiben.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Der Typ hat keine Byte-Form (kein POD, 5.9).
    NotPod,
    /// Die Bytes sind zu kurz oder ergeben keinen gueltigen Wert.
    Malformed,
}

/// Kopf eines `capture`-Elements: `t: i64, pre: u32, post: u32, rate: f64` (8.9).
pub const CAPTURE_HEAD: u32 = 24;

/// Die Gestalt eines Typs fuer `takt_native::bytes::decodes`: was die TCB
/// ohne den Typ wissen muss, um zu entscheiden, ob Bytes ein Wert sind
/// (12.6, Zeile 5). Dieselben Faelle wie der Leser im Interpreter; was er
/// nicht liest, ist kein POD.
pub fn shape(p: &Program, ty: TypeId) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    shape_at(p, ty, 0, &mut out)?;
    Ok(out)
}

fn shape_at(p: &Program, ty: TypeId, depth: u32, out: &mut Vec<u8>) -> Result<(), Error> {
    use takt_native::bytes::shape as op;
    if depth > 32 {
        return Err(Error::NotPod);
    }
    let count = |n: usize| u16::try_from(n).map(u16::to_le_bytes).map_err(|_| Error::NotPod);
    match p.types.list.get(ty.index()).ok_or(Error::NotPod)? {
        Type::Bool => out.push(op::BOOL),
        Type::Int { width, .. } => out.extend([op::INT, (width.bits() / 8) as u8]),
        Type::Float { width: FloatWidth::F32, .. } => out.push(op::F32),
        Type::Float { width: FloatWidth::F64, .. } => out.push(op::F64),
        Type::Duration { .. } => out.push(op::DURATION),
        Type::Enum(e) => {
            let def = p.enums.get(e.index()).ok_or(Error::NotPod)?;
            out.push(op::ENUM);
            out.extend(count(def.variants.len())?);
            for v in &def.variants {
                out.extend(v.discriminant.to_le_bytes());
                out.extend(count(v.fields.len())?);
                for f in &v.fields {
                    shape_at(p, f.ty, depth + 1, out)?;
                }
            }
        }
        Type::Record(r) => {
            let def = p.records.get(r.index()).ok_or(Error::NotPod)?;
            out.push(op::RECORD);
            out.extend(count(def.fields.len())?);
            for f in &def.fields {
                shape_at(p, f.ty, depth + 1, out)?;
            }
        }
        Type::Array { elem, len } | Type::Capture { elem, len } => {
            let code =
                if matches!(p.types.list.get(ty.index()), Some(Type::Array { .. })) { op::ARRAY } else { op::CAPTURE };
            out.push(code);
            out.extend(len.to_le_bytes());
            shape_at(p, *elem, depth + 1, out)?;
        }
        Type::Bytes { cap } => {
            out.push(op::BYTES);
            out.extend(cap.to_le_bytes());
        }
        Type::Str { cap } => {
            out.push(op::STR);
            out.extend(cap.to_le_bytes());
        }
        Type::Vec { elem, cap } => {
            out.push(op::VEC);
            out.extend(cap.to_le_bytes());
            shape_at(p, *elem, depth + 1, out)?;
        }
        Type::Map { key, value, cap } => {
            out.push(op::MAP);
            out.extend(cap.to_le_bytes());
            out.extend(max_size(p, *key)?.to_le_bytes());
            out.extend(max_size(p, *value)?.to_le_bytes());
            shape_at(p, *key, depth + 1, out)?;
            shape_at(p, *value, depth + 1, out)?;
        }
        _ => return Err(Error::NotPod),
    }
    Ok(())
}

/// Obere Schranke der kodierten Laenge in Byte.
///
/// Fuer `bytes`, `str`, `vec` und `map` ist es die Kapazitaet, nicht die
/// tatsaechliche Laenge — `takt size` braucht eine Schranke, keine Zahl.
pub fn max_size(p: &Program, ty: TypeId) -> Result<u32, Error> {
    size_at(p, ty, 0)
}

/// Die Groesse der kanonischen Form eines Records (5.9), ohne dass sein
/// Typ in der Typtabelle stehen muss.
pub fn record_size(p: &Program, r: RecordId) -> Result<u32, Error> {
    record_size_at(p, r, 0)
}

fn record_size_at(p: &Program, r: RecordId, depth: u32) -> Result<u32, Error> {
    let def = p.records.get(r.index()).ok_or(Error::NotPod)?;
    let mut n = 0;
    for f in &def.fields {
        n += size_at(p, f.ty, depth + 1)?;
    }
    Ok(n)
}

fn size_at(p: &Program, ty: TypeId, depth: u32) -> Result<u32, Error> {
    if depth > 32 {
        return Err(Error::NotPod);
    }
    let t = p.types.list.get(ty.index()).ok_or(Error::NotPod)?;
    match t {
        Type::Bool => Ok(1),
        Type::Int { width, .. } => Ok(width.bits() / 8),
        Type::Float { width, .. } => Ok(if *width == FloatWidth::F32 { 4 } else { 8 }),
        Type::Duration { .. } => Ok(8),
        Type::Enum(e) => {
            let def = p.enums.get(e.index()).ok_or(Error::NotPod)?;
            let mut worst = 0;
            for v in &def.variants {
                let mut n = 0;
                for f in &v.fields {
                    n += size_at(p, f.ty, depth + 1)?;
                }
                worst = worst.max(n);
            }
            Ok(8 + worst)
        }
        Type::Record(r) => record_size_at(p, *r, depth),
        Type::Array { elem, len } => Ok(size_at(p, *elem, depth + 1)? * len),
        // 8.9: Kopf `t, pre, post, rate`, dann `N` Abtastwerte.
        Type::Capture { elem, len } => Ok(CAPTURE_HEAD + size_at(p, *elem, depth + 1)? * len),
        Type::Bytes { cap } | Type::Str { cap } => Ok(4 + cap),
        Type::Vec { elem, cap } => Ok(4 + size_at(p, *elem, depth + 1)? * cap),
        // 3.9: `N` Slots zu je `1 + K + V` Byte — die Slots selbst sind die
        // Form, damit `persist` sie kopiert und die Sondierketten behaelt.
        Type::Map { key, value, cap } => {
            let pair = size_at(p, *key, depth + 1)? + size_at(p, *value, depth + 1)?;
            Ok((1 + pair) * cap)
        }
        _ => Err(Error::NotPod),
    }
}

/// Liest Bytes in kanonischer Reihenfolge.
#[derive(Debug)]
pub struct Decoder<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Decoder<'a> {
    /// Liest ab dem Anfang.
    pub fn new(bytes: &'a [u8]) -> Decoder<'a> {
        Decoder { bytes, at: 0 }
    }

    /// Wie viele Bytes gelesen wurden.
    pub fn position(&self) -> usize {
        self.at
    }

    /// Sind alle Bytes verbraucht?
    pub fn is_empty(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Malformed)?;
        let slice = self.bytes.get(self.at..end).ok_or(Error::Malformed)?;
        self.at = end;
        Ok(slice)
    }

    /// Ein Wahrheitswert; alles ausser 0 und 1 ist ungueltig.
    pub fn bool(&mut self) -> Result<bool, Error> {
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Malformed),
        }
    }

    /// Eine Ganzzahl in ihrer Breite, vorzeichenrichtig erweitert.
    pub fn int(&mut self, width: IntWidth) -> Result<i64, Error> {
        let n = (width.bits() / 8) as usize;
        let slice = self.take(n)?;
        let mut buf = [0u8; 8];
        buf[..n].copy_from_slice(slice);
        let raw = u64::from_le_bytes(buf);
        if width.signed() && n < 8 {
            let shift = 64 - width.bits();
            Ok(((raw << shift) as i64) >> shift)
        } else {
            Ok(raw as i64)
        }
    }

    /// Das Bitmuster einer Fliesskommazahl.
    pub fn float(&mut self, width: FloatWidth) -> Result<u64, Error> {
        match width {
            FloatWidth::F32 => {
                let slice = self.take(4)?;
                Ok(u64::from(u32::from_le_bytes(slice.try_into().map_err(|_| Error::Malformed)?)))
            }
            FloatWidth::F64 => {
                let slice = self.take(8)?;
                Ok(u64::from_le_bytes(slice.try_into().map_err(|_| Error::Malformed)?))
            }
        }
    }

    /// Eine Dauer in Nanosekunden.
    pub fn duration(&mut self) -> Result<i64, Error> {
        let slice = self.take(8)?;
        Ok(i64::from_le_bytes(slice.try_into().map_err(|_| Error::Malformed)?))
    }

    /// Eine Laenge; `cap` ist die Obergrenze aus dem Typ.
    pub fn len(&mut self, cap: u32) -> Result<u32, Error> {
        let slice = self.take(4)?;
        let n = u32::from_le_bytes(slice.try_into().map_err(|_| Error::Malformed)?);
        if n > cap { Err(Error::Malformed) } else { Ok(n) }
    }

    /// Eine Diskriminante.
    pub fn discriminant(&mut self) -> Result<i64, Error> {
        let slice = self.take(8)?;
        Ok(i64::from_le_bytes(slice.try_into().map_err(|_| Error::Malformed)?))
    }

    /// Rohe Bytes eines `bytes<N>`.
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], Error> {
        self.take(n)
    }
}
