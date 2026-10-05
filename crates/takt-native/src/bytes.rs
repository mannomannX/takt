//! Die kanonische Byteform auf der Seite der TCB (5.9; plan/m6.md 2.2).
//!
//! Dieselben Bytes wie `takt_mir::bytes::Encoder`, nur ohne Allokation:
//! `map` hasht seine Schluessel darueber (3.9), ein Job kopiert seine
//! Argumente hindurch (4.5). Der Test `canonical_bytes.rs` in `takt-sema`
//! haelt beide Seiten Byte fuer Byte gleich.
//!
//! Little-endian, ohne Padding: `bool` ein Byte, Laengen vier, Dauern und
//! Diskriminanten acht.
//!
//! Lesen muss die TCB nur in einer Form: entscheiden, ob Bytes ein Wert
//! sind ([`decodes`]). Das braucht der Treiberrand fuer ein Stromelement,
//! das ein Treiber als Bytes liefert (12.6, Zeile 5). Den Typ kennt die
//! TCB nicht; sie bekommt seine Gestalt ([`shape`]), die `takt_mir::bytes::
//! shape` aus dem Typ schreibt, und urteilt nach denselben Regeln wie der
//! Leser des Interpreters — `canonical_shape.rs` in `takt-sema` haelt beide
//! gleich.

/// Der Puffer ist voll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overflow;

/// Schreibt die kanonische Form in einen festen Puffer.
#[derive(Debug)]
pub struct Encoder<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl<'a> Encoder<'a> {
    /// Schreibt ab dem Anfang von `buf`.
    pub fn new(buf: &'a mut [u8]) -> Encoder<'a> {
        Encoder { buf, at: 0 }
    }

    /// Die bisher geschriebenen Bytes.
    pub fn written(&self) -> &[u8] {
        &self.buf[..self.at]
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), Overflow> {
        let end = self.at.checked_add(bytes.len()).ok_or(Overflow)?;
        self.buf.get_mut(self.at..end).ok_or(Overflow)?.copy_from_slice(bytes);
        self.at = end;
        Ok(())
    }

    /// Ein Wahrheitswert.
    pub fn bool(&mut self, v: bool) -> Result<(), Overflow> {
        self.put(&[u8::from(v)])
    }

    /// Eine Ganzzahl in `width` Byte (1, 2, 4 oder 8).
    pub fn int(&mut self, v: i64, width: usize) -> Result<(), Overflow> {
        self.put(&v.to_le_bytes()[..width.min(8)])
    }

    /// Das Bitmuster einer `f32`.
    pub fn f32(&mut self, bits: u32) -> Result<(), Overflow> {
        self.put(&bits.to_le_bytes())
    }

    /// Das Bitmuster einer `f64`.
    pub fn f64(&mut self, bits: u64) -> Result<(), Overflow> {
        self.put(&bits.to_le_bytes())
    }

    /// Eine Dauer in Nanosekunden.
    pub fn duration(&mut self, ns: i64) -> Result<(), Overflow> {
        self.put(&ns.to_le_bytes())
    }

    /// Eine Laenge oder Anzahl (`bytes`, `str`, `vec`, `map`).
    pub fn len(&mut self, n: u32) -> Result<(), Overflow> {
        self.put(&n.to_le_bytes())
    }

    /// Die Diskriminante einer Variante, immer acht Byte.
    pub fn discriminant(&mut self, d: i64) -> Result<(), Overflow> {
        self.put(&d.to_le_bytes())
    }

    /// Rohe Bytes eines `bytes<N>`.
    pub fn raw(&mut self, bytes: &[u8]) -> Result<(), Overflow> {
        self.put(bytes)
    }
}

/// Die Gestalt eines Typs fuer [`decodes`]: ein Opcode je Typknoten, dahinter
/// seine Zahlen little-endian und die Gestalten seiner Teile.
pub mod shape {
    /// `bool`.
    pub const BOOL: u8 = 1;
    /// Ganzzahl; dahinter ihre Breite in Byte (`u8`).
    pub const INT: u8 = 2;
    /// `f32`.
    pub const F32: u8 = 3;
    /// `f64`.
    pub const F64: u8 = 4;
    /// Dauer.
    pub const DURATION: u8 = 5;
    /// Enum: Zahl der Varianten (`u16`), je Variante Diskriminante (`i64`),
    /// Zahl der Felder (`u16`) und deren Gestalten.
    pub const ENUM: u8 = 6;
    /// Record: Zahl der Felder (`u16`) und deren Gestalten.
    pub const RECORD: u8 = 7;
    /// Array: Laenge (`u32`) und die Gestalt des Elements.
    pub const ARRAY: u8 = 8;
    /// `capture` (8.9): Laenge (`u32`) und die Gestalt des Abtastwerts.
    pub const CAPTURE: u8 = 9;
    /// `bytes<N>`: Kapazitaet (`u32`).
    pub const BYTES: u8 = 10;
    /// `str<N>`: Kapazitaet in Bytes (`u32`).
    pub const STR: u8 = 11;
    /// `vec<T, N>`: Kapazitaet (`u32`) und die Gestalt des Elements.
    pub const VEC: u8 = 12;
    /// `map<K, V, N>`: Kapazitaet, Slotbreite von Schluessel und Wert (je
    /// `u32`), dann beider Gestalten.
    pub const MAP: u8 = 13;
}

/// Tiefer verschachtelt liest der Interpreter nicht (`bytes::read`).
const DEPTH: u32 = 32;

/// Ob `bytes` ein Wert der Gestalt `shape` in kanonischer Form ist. Mit
/// `exact` muessen die Bytes genau reichen (`decode`); sonst darf
/// Fuellung folgen (`decode_slot`, ein Slot fester Groesse).
pub fn decodes(shape: &[u8], bytes: &[u8], exact: bool) -> bool {
    let (mut s, mut d) = (Cursor::new(shape), Cursor::new(bytes));
    check(&mut s, &mut d, 0).is_some() && s.at == shape.len() && (!exact || d.at == bytes.len())
}

/// Ein Lesezeiger.
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Cursor<'a> {
        Cursor { data, at: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let slice = self.data.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// Eine Laenge, hoechstens `cap`.
    fn len(&mut self, cap: u32) -> Option<usize> {
        let n = self.u32()?;
        if n > cap { None } else { usize::try_from(n).ok() }
    }
}

/// Ein Gleitkommawert in Little-Endian ist endlich, wenn sein Exponent
/// (`mask`) nicht aus lauter Einsen besteht.
fn finite(bytes: &[u8], mask: u64) -> Option<()> {
    let bits = bytes.iter().rev().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    (bits & mask != mask).then_some(())
}

/// Liest einen Wert der Gestalt unter `s` aus `d`; `None`, wenn es keiner
/// ist.
fn check(s: &mut Cursor<'_>, d: &mut Cursor<'_>, depth: u32) -> Option<()> {
    if depth > DEPTH {
        return None;
    }
    match s.u8()? {
        shape::BOOL => (d.u8()? <= 1).then_some(()),
        shape::INT => d.take(usize::from(s.u8()?)).map(drop),
        // 5.9, 3.7: NaN und Inf sind keine kanonische Form; ihr Exponent
        // besteht aus lauter Einsen.
        shape::F32 => finite(d.take(4)?, 0x7F80_0000),
        shape::F64 => finite(d.take(8)?, 0x7FF0_0000_0000_0000),
        shape::DURATION => d.take(8).map(drop),
        shape::ENUM => {
            let variants = s.u16()?;
            let disc = d.i64()?;
            let mut found = false;
            for _ in 0..variants {
                let hit = s.i64()? == disc && !found;
                for _ in 0..s.u16()? {
                    if hit {
                        check(s, d, depth + 1)?;
                    } else {
                        skip(s, depth + 1)?;
                    }
                }
                found |= hit;
            }
            found.then_some(())
        }
        shape::RECORD => {
            for _ in 0..s.u16()? {
                check(s, d, depth + 1)?;
            }
            Some(())
        }
        shape::ARRAY => {
            let n = s.u32()?;
            repeat(s, d, n, depth)
        }
        shape::CAPTURE => {
            let n = s.u32()?;
            // 8.9: Kopf `t: i64, pre: u32, post: u32, rate: f64`.
            d.take(8 + 4 + 4)?;
            finite(d.take(8)?, 0x7FF0_0000_0000_0000)?;
            repeat(s, d, n, depth)
        }
        shape::BYTES => {
            let n = d.len(s.u32()?)?;
            d.take(n).map(drop)
        }
        shape::STR => {
            let n = d.len(s.u32()?)?;
            core::str::from_utf8(d.take(n)?).ok().map(drop)
        }
        shape::VEC => {
            let n = d.len(s.u32()?)?;
            repeat(s, d, u32::try_from(n).ok()?, depth)
        }
        shape::MAP => {
            let (cap, klen, vlen) = (s.u32()?, s.u32()? as usize, s.u32()? as usize);
            let key = s.at;
            skip(s, depth + 1)?;
            let value = s.at;
            skip(s, depth + 1)?;
            let end = s.at;
            for _ in 0..cap {
                match d.u8()? {
                    0 => {
                        d.take(klen.checked_add(vlen)?)?;
                    }
                    1 => {
                        for (at, width) in [(key, klen), (value, vlen)] {
                            let from = d.at;
                            s.at = at;
                            check(s, d, depth + 1)?;
                            d.take(width.saturating_sub(d.at - from))?;
                        }
                    }
                    _ => return None,
                }
            }
            s.at = end;
            Some(())
        }
        _ => None,
    }
}

/// `n` Werte derselben Gestalt; danach steht `s` hinter ihr.
fn repeat(s: &mut Cursor<'_>, d: &mut Cursor<'_>, n: u32, depth: u32) -> Option<()> {
    let at = s.at;
    if n == 0 {
        return skip(s, depth + 1);
    }
    for _ in 0..n {
        s.at = at;
        check(s, d, depth + 1)?;
    }
    Some(())
}

/// Ueberspringt eine Gestalt, ohne Daten zu lesen.
fn skip(s: &mut Cursor<'_>, depth: u32) -> Option<()> {
    if depth > DEPTH {
        return None;
    }
    match s.u8()? {
        shape::BOOL | shape::F32 | shape::F64 | shape::DURATION => Some(()),
        shape::INT => s.u8().map(drop),
        shape::BYTES | shape::STR => s.u32().map(drop),
        shape::ENUM => {
            for _ in 0..s.u16()? {
                s.i64()?;
                for _ in 0..s.u16()? {
                    skip(s, depth + 1)?;
                }
            }
            Some(())
        }
        shape::RECORD => {
            for _ in 0..s.u16()? {
                skip(s, depth + 1)?;
            }
            Some(())
        }
        shape::ARRAY | shape::CAPTURE | shape::VEC => {
            s.u32()?;
            skip(s, depth + 1)
        }
        shape::MAP => {
            s.take(12)?;
            skip(s, depth + 1)?;
            skip(s, depth + 1)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein Record `{ ok: bool, n: u16 }`.
    const PAIR: [u8; 6] = [shape::RECORD, 2, 0, shape::BOOL, shape::INT, 2];

    #[test]
    fn a_record_decodes_exactly_or_as_a_slot() {
        assert!(decodes(&PAIR, &[1, 0x34, 0x12], true));
        assert!(!decodes(&PAIR, &[2, 0x34, 0x12], true), "`bool` ist 0 oder 1");
        assert!(!decodes(&PAIR, &[1, 0x34], true), "zu kurz");
        assert!(!decodes(&PAIR, &[1, 0x34, 0x12, 0], true), "ein Byte zu viel");
        assert!(decodes(&PAIR, &[1, 0x34, 0x12, 0], false), "im Slot darf Fuellung folgen");
    }

    #[test]
    fn a_string_is_utf8_within_its_bytes() {
        let s = [shape::STR, 3, 0, 0, 0];
        assert!(decodes(&s, &[3, 0, 0, 0, b'a', b'b', b'c'], true));
        assert!(!decodes(&s, &[4, 0, 0, 0, b'a', b'b', b'c', b'd'], true), "mehr als N Bytes");
        assert!(!decodes(&s, &[2, 0, 0, 0, 0xc3, b'x'], true), "kein UTF-8");
        assert!(decodes(&s, &[2, 0, 0, 0, 0xc3, 0xa4], true), "`ä` sind zwei Bytes");
    }

    #[test]
    fn an_enum_needs_a_known_discriminant() {
        // `A | B(u8)` mit den Diskriminanten 0 und 7.
        let e = [shape::ENUM, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 1, 0, shape::INT, 1];
        assert!(decodes(&e, &[0; 8], true));
        assert!(decodes(&e, &[7, 0, 0, 0, 0, 0, 0, 0, 42], true));
        assert!(!decodes(&e, &[7, 0, 0, 0, 0, 0, 0, 0], true), "B traegt ein Feld");
        assert!(!decodes(&e, &[1, 0, 0, 0, 0, 0, 0, 0], true), "1 ist keine Variante");
    }
}
