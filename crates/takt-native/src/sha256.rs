//! SHA-256 (FIPS 180-4) und HMAC-SHA256 (RFC 2104; Vektoren RFC 4231).
//!
//! `Ctx` ist der Zustand der Chunk-Natives `sha256_init/update/final`
//! (4.5): im Programm das Prelude-Record `Sha256Ctx` mit `h: [8] u32`,
//! `buf: bytes<64>`, `total: u64`, das die Grenze in der kanonischen
//! Byteform ueberquert (`to_bytes`/`from_bytes`, 5.9). Ein Programm liest
//! seine Felder nicht — „opak" ist Konvention, kein Typbegriff.

use crate::bytes::{Encoder, Overflow};

const H0: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
    0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
    0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

/// Obere Schranke der kanonischen Form von `Sha256Ctx` in Byte.
pub const CTX_MAX_BYTES: usize = 32 + 4 + 64 + 8;

/// Der Zustand zwischen zwei Chunks.
#[derive(Clone, Debug)]
pub struct Ctx {
    h: [u32; 8],
    block: [u8; 64],
    filled: usize,
    total: u64,
}

/// Die Bytes hinter `filled` sind Rest eines frueheren Blocks und
/// gehoeren nicht zum Zustand — die kanonische Form traegt sie nicht.
impl PartialEq for Ctx {
    fn eq(&self, other: &Ctx) -> bool {
        self.h == other.h
            && self.total == other.total
            && self.block.get(..self.filled) == other.block.get(..other.filled)
    }
}

impl Eq for Ctx {}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx::new()
    }
}

impl Ctx {
    /// `sha256_init()`.
    pub fn new() -> Ctx {
        Ctx { h: H0, block: [0; 64], filled: 0, total: 0 }
    }

    /// `sha256_update(ctx, chunk)`.
    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            if let Some(slot) = self.block.get_mut(self.filled) {
                *slot = b;
            }
            self.filled = self.filled.saturating_add(1);
            if self.filled == 64 {
                compress(&mut self.h, &self.block);
                self.filled = 0;
            }
        }
        self.total = self.total.wrapping_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
    }

    /// `sha256_final(ctx)`: `0x80`, Nullen bis Byte 56 des letzten Blocks,
    /// dann die Laenge in Bit (FIPS 180-4, 5.1.1).
    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.total.wrapping_mul(8);
        if let Some((first, rest)) = self.block.get_mut(self.filled..).and_then(<[u8]>::split_first_mut) {
            *first = 0x80;
            rest.fill(0);
        }
        if self.filled >= 56 {
            compress(&mut self.h, &self.block);
            self.block.fill(0);
        }
        if let Some(length) = self.block.last_chunk_mut::<8>() {
            *length = bits.to_be_bytes();
        }
        compress(&mut self.h, &self.block);
        let mut out = [0u8; 32];
        for (chunk, w) in out.chunks_exact_mut(4).zip(self.h) {
            chunk.copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    /// Die kanonische Form (5.9): `h` als acht u32, `buf` als Laenge
    /// plus die gefuellten Bytes, `total` als u64; liefert die Laenge.
    pub fn to_bytes(&self, out: &mut [u8]) -> Result<usize, Overflow> {
        let mut e = Encoder::new(out);
        for w in self.h {
            e.int(i64::from(w), 4)?;
        }
        let buffered = self.block.get(..self.filled).ok_or(Overflow)?;
        e.len(u32::try_from(buffered.len()).map_err(|_| Overflow)?)?;
        e.raw(buffered)?;
        e.int(i64::from_le_bytes(self.total.to_le_bytes()), 8)?;
        Ok(e.written().len())
    }

    /// Liest die kanonische Form; `None`, wenn sie kein `Sha256Ctx` ist.
    /// Jeder Wert des Records ist einer: Ein voller Puffer (64 Byte) ist
    /// der Block, den `update` als naechstes verdichtet haette.
    pub fn from_bytes(b: &[u8]) -> Option<Ctx> {
        let (words, rest) = b.split_first_chunk::<32>()?;
        let mut h = [0u32; 8];
        for (w, bytes) in h.iter_mut().zip(words.chunks_exact(4)) {
            *w = u32::from_le_bytes(bytes.try_into().ok()?);
        }
        let (length, rest) = rest.split_first_chunk::<4>()?;
        let filled = usize::try_from(u32::from_le_bytes(*length)).ok()?;
        if filled > 64 {
            return None;
        }
        let (buffered, rest) = rest.split_at_checked(filled)?;
        let mut block = [0u8; 64];
        block.get_mut(..filled)?.copy_from_slice(buffered);
        let total = u64::from_le_bytes(rest.try_into().ok()?);
        if filled == 64 {
            compress(&mut h, &block);
        }
        Some(Ctx { h, block, filled: filled % 64, total })
    }
}

/// `sha256(b)`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut ctx = Ctx::new();
    ctx.update(data);
    ctx.finish()
}

/// `hmac_sha256(key, msg)`: ein Schluessel ueber 64 Byte wird zuerst
/// gehasht (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    match k.get_mut(..key.len()) {
        Some(head) => head.copy_from_slice(key),
        None => k[..32].copy_from_slice(&sha256(key)),
    }
    let mut pad = [0u8; 64];
    for (p, x) in pad.iter_mut().zip(k) {
        *p = x ^ 0x36;
    }
    let mut inner = Ctx::new();
    inner.update(&pad);
    inner.update(msg);
    let digest = inner.finish();
    for (p, x) in pad.iter_mut().zip(k) {
        *p = x ^ 0x5c;
    }
    let mut outer = Ctx::new();
    outer.update(&pad);
    outer.update(&digest);
    outer.finish()
}

/// Eine Runde je Wort des Nachrichtenplans (FIPS 180-4, 6.2.2).
fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 16];
    for (word, bytes) in w.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes(bytes.try_into().unwrap_or_default());
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for (t, k) in K.iter().enumerate() {
        let wi = schedule(&mut w, t);
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(*k).wrapping_add(wi);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

/// W(t) des Nachrichtenplans. Der Plan liegt als Ring der letzten 16
/// Woerter: `w[t % 16]` traegt vor Runde `t` noch W(t-16) und wird zu W(t)
/// — 64 statt 256 Byte Stack (4.5).
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "MISRA C:2025 D.4.1: jeder Index ist `% 16` auf 16 Woertern, und subtrahiert wird nur fuer t >= 16"
)]
fn schedule(w: &mut [u32; 16], t: usize) -> u32 {
    if t >= 16 {
        let (w15, w2) = (w[(t - 15) % 16], w[(t - 2) % 16]);
        let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        w[t % 16] = w[t % 16].wrapping_add(s0).wrapping_add(w[(t - 7) % 16]).wrapping_add(s1);
    }
    w[t % 16]
}
