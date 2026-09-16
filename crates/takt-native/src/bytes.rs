//! Die kanonische Byteform auf der Seite der TCB (5.9; plan/m6.md 2.2).
//!
//! Dieselben Bytes wie `takt_mir::bytes::Encoder`, nur ohne Allokation:
//! `map` hasht seine Schluessel darueber (3.9), ein Job kopiert seine
//! Argumente hindurch (4.5). Der Test `canonical_bytes.rs` in `takt-sema`
//! haelt beide Seiten Byte fuer Byte gleich.
//!
//! Little-endian, ohne Padding: `bool` ein Byte, Laengen vier, Dauern und
//! Diskriminanten acht.

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
