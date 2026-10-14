//! Die Rahmung eines Eingabestroms im Treiber (8.6: „Rahmung ist ein
//! beschraenkter Transducer im Treiber“, M11 Schritt 22a): aus Bytes,
//! eines nach dem anderen, die Elemente des Stroms.
//!
//! Jeder Rahmer haelt das Element, das er gerade baut, in einem Puffer
//! fester Groesse `M`. Ein laengeres kuerzt er auf `M` Bytes, wie jeder
//! Treiber ein Element kuerzt, das nicht in den Puffer des Rahmens passt
//! ([`crate::StreamInput`]); die Grenze des naechsten findet er trotzdem.
//! [`Framed`] macht aus einer Quelle von Bytes — einem [`Ring`], den eine
//! ISR fuellt, einer seriellen Schnittstelle — und einem Rahmer einen
//! Eingabestrom.

use crate::ring::Ring;
use crate::{Piece, StreamInput};

/// Ein Rahmer: nimmt Bytes und meldet jedes fertige Element.
pub trait Framer {
    /// Nimmt das naechste Byte. Vollendet es ein Element, stehen dessen
    /// Bytes in `out`, gekuerzt auf seine Laenge, und `Some` nennt, wie
    /// viele.
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Option<usize>;

    /// Die Quelle hat gerade nichts mehr: `Some` wie bei [`Framer::push`],
    /// wenn das ein Element vollendet. Nur `raw` rahmt so; jeder andere
    /// Rahmer wartet auf seine Grenze.
    fn pause(&mut self, out: &mut [u8]) -> Option<usize> {
        let _ = out;
        None
    }
}

/// Eine Quelle von Bytes, die der Schritt liest, ohne zu warten.
pub trait Source {
    /// Das naechste Byte; `None`, wenn gerade keines bereitliegt.
    fn take(&mut self) -> Option<u8>;
}

impl<const N: usize> Source for &Ring<N> {
    fn take(&mut self) -> Option<u8> {
        self.pop()
    }
}

/// Ein Eingabestrom aus einer Quelle und einem Rahmer (12.6): je `poll`
/// das naechste Element, mit der Tickgrenze als Zeitstempel und der
/// Folgenummer, die der Rand erwartet.
#[derive(Debug, Default)]
pub struct Framed<S, F> {
    /// Woher die Bytes kommen.
    pub source: S,
    /// Wie sie zu Elementen werden.
    pub framer: F,
}

impl<S: Source, F: Framer> StreamInput for Framed<S, F> {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        let len = loop {
            match self.source.take() {
                Some(byte) => {
                    if let Some(len) = self.framer.push(byte, buf) {
                        break len;
                    }
                }
                None => break self.framer.pause(buf)?,
            }
        };
        Some(Piece { len, t: now, seq: None })
    }
}

/// Ein Element im Bau: hoechstens `M` Bytes.
#[derive(Debug)]
struct Element<const M: usize> {
    bytes: [u8; M],
    len: usize,
}

impl<const M: usize> Element<M> {
    const fn new() -> Element<M> {
        Element { bytes: [0; M], len: 0 }
    }

    /// Haengt `byte` an; was ueber `M` hinausgeht, faellt weg.
    fn add(&mut self, byte: u8) {
        if let Some(at) = self.bytes.get_mut(self.len) {
            *at = byte;
            self.len = self.len.wrapping_add(1);
        }
    }

    /// Gibt das fertige Element nach `out`, gekuerzt auf dessen Laenge;
    /// danach beginnt das naechste. Die Zahl der Bytes in `out`.
    fn take(&mut self, out: &mut [u8]) -> usize {
        let len = core::mem::take(&mut self.len).min(out.len());
        if let (Some(to), Some(from)) = (out.get_mut(..len), self.bytes.get(..len)) {
            to.copy_from_slice(from);
        }
        len
    }
}

/// `framing = raw`: ein Element aus dem, was die Quelle gerade hat, hoechstens
/// `M` Bytes; bei einem Strom aus `u8` ist `M` eins.
#[derive(Debug)]
pub struct Raw<const M: usize>(Element<M>);

impl<const M: usize> Raw<M> {
    /// Ein Rahmer fuer Stuecke aus hoechstens `M` Bytes.
    pub const fn new() -> Raw<M> {
        Raw(Element::new())
    }
}

impl<const M: usize> Default for Raw<M> {
    fn default() -> Raw<M> {
        Raw::new()
    }
}

impl<const M: usize> Framer for Raw<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Option<usize> {
        self.0.add(byte);
        if self.0.len < M {
            return None;
        }
        Some(self.0.take(out))
    }

    fn pause(&mut self, out: &mut [u8]) -> Option<usize> {
        if self.0.len == 0 {
            return None;
        }
        Some(self.0.take(out))
    }
}

/// `framing = fixed(N)`: je `M` Bytes ein Element.
#[derive(Debug)]
pub struct Fixed<const M: usize>(Element<M>);

impl<const M: usize> Fixed<M> {
    /// Ein Rahmer fuer Elemente aus `M` Bytes.
    pub const fn new() -> Fixed<M> {
        Fixed(Element::new())
    }
}

impl<const M: usize> Default for Fixed<M> {
    fn default() -> Fixed<M> {
        Fixed::new()
    }
}

impl<const M: usize> Framer for Fixed<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Option<usize> {
        self.0.add(byte);
        if self.0.len < M {
            return None;
        }
        Some(self.0.take(out))
    }
}

/// `framing = lines`: ein Element je Zeile, ohne ihr Ende (`\n`, davor
/// ein `\r`).
#[derive(Debug)]
pub struct Lines<const M: usize> {
    line: Element<M>,
    /// Ein `\r` steht noch aus: Folgt `\n`, gehoert es zum Zeilenende.
    cr: bool,
}

impl<const M: usize> Lines<M> {
    /// Ein Rahmer fuer Zeilen aus hoechstens `M` Bytes.
    pub const fn new() -> Lines<M> {
        Lines { line: Element::new(), cr: false }
    }
}

impl<const M: usize> Default for Lines<M> {
    fn default() -> Lines<M> {
        Lines::new()
    }
}

impl<const M: usize> Framer for Lines<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Option<usize> {
        let cr = core::mem::replace(&mut self.cr, byte == b'\r');
        if byte == b'\n' {
            return Some(self.line.take(out));
        }
        if cr {
            self.line.add(b'\r');
        }
        if byte != b'\r' {
            self.line.add(byte);
        }
        None
    }
}

/// `framing = cobs`: ein Element je Rahmen, durch `0` getrennt und nach
/// COBS dekodiert. Zwei Nullen ohne etwas dazwischen sind kein Rahmen;
/// einen Rahmen, dessen letzter Block zu kurz ist, verwirft er.
#[derive(Debug)]
pub struct Cobs<const M: usize> {
    frame: Element<M>,
    /// Wie viele Datenbytes der laufende Block noch hat.
    left: u8,
    /// Ob auf den laufenden Block eine `0` folgt: nach jedem ausser einem
    /// vollen (Codebyte 255) und dem letzten des Rahmens.
    zero: bool,
    /// Ob schon ein Byte des Rahmens kam.
    started: bool,
}

impl<const M: usize> Cobs<M> {
    /// Ein Rahmer fuer Rahmen mit hoechstens `M` Bytes Nutzlast.
    pub const fn new() -> Cobs<M> {
        Cobs { frame: Element::new(), left: 0, zero: false, started: false }
    }
}

impl<const M: usize> Default for Cobs<M> {
    fn default() -> Cobs<M> {
        Cobs::new()
    }
}

impl<const M: usize> Framer for Cobs<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Option<usize> {
        if byte == 0 {
            let whole = core::mem::take(&mut self.started) && self.left == 0;
            self.left = 0;
            self.zero = false;
            let len = self.frame.take(out);
            return whole.then_some(len);
        }
        if self.left > 0 {
            self.frame.add(byte);
            self.left = self.left.wrapping_sub(1);
            return None;
        }
        // Ein Codebyte: Vor seinem Block steht die `0` des vorigen.
        if core::mem::replace(&mut self.started, true) && self.zero {
            self.frame.add(0);
        }
        self.zero = byte != 0xFF;
        self.left = byte.wrapping_sub(1);
        None
    }
}

/// `framing = length_prefixed(u16_le)`: zwei Bytes Laenge, kleinstes
/// zuerst, dann die Nutzlast.
#[derive(Debug)]
pub struct LengthPrefixed<const M: usize> {
    payload: Element<M>,
    /// Die Bytes des Kopfs, die schon kamen.
    head: [u8; 2],
    have: u8,
    /// Wie viele Bytes der Nutzlast noch fehlen.
    left: u16,
}

impl<const M: usize> LengthPrefixed<M> {
    /// Ein Rahmer fuer Nutzlasten mit hoechstens `M` Bytes.
    pub const fn new() -> LengthPrefixed<M> {
        LengthPrefixed { payload: Element::new(), head: [0; 2], have: 0, left: 0 }
    }
}

impl<const M: usize> Default for LengthPrefixed<M> {
    fn default() -> LengthPrefixed<M> {
        LengthPrefixed::new()
    }
}

impl<const M: usize> Framer for LengthPrefixed<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Option<usize> {
        if self.have < 2 {
            if let Some(at) = self.head.get_mut(usize::from(self.have)) {
                *at = byte;
            }
            self.have = self.have.wrapping_add(1);
            if self.have < 2 {
                return None;
            }
            self.left = u16::from_le_bytes(self.head);
        } else {
            self.payload.add(byte);
            self.left = self.left.wrapping_sub(1);
        }
        if self.left > 0 {
            return None;
        }
        self.have = 0;
        Some(self.payload.take(out))
    }
}

#[cfg(test)]
mod tests {
    use super::{Cobs, Fixed, Framer, LengthPrefixed, Lines};
    use std::vec;
    use std::vec::Vec;

    /// Was ein Rahmer aus `bytes` macht, Element fuer Element.
    fn elements(framer: &mut impl Framer, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = [0u8; 2048];
        bytes.iter().filter_map(|b| framer.push(*b, &mut out).map(|n| out[..n].to_vec())).collect()
    }

    #[test]
    fn fixed_cuts_every_m_bytes() {
        assert_eq!(elements(&mut Fixed::<3>::new(), b"abcdefg"), [b"abc".to_vec(), b"def".to_vec()]);
    }

    #[test]
    fn lines_end_at_newline_without_it_and_without_a_carriage_return() {
        let out = elements(&mut Lines::<16>::new(), b"one\ntwo\r\n\nthr\ree\n");
        assert_eq!(out, [b"one".to_vec(), b"two".to_vec(), Vec::new(), b"thr\ree".to_vec()]);
    }

    /// Eine zu lange Zeile wird gekuerzt; die naechste beginnt richtig.
    #[test]
    fn a_long_line_is_cut_and_the_next_starts_clean() {
        let out = elements(&mut Lines::<4>::new(), b"abcdefgh\nxy\n");
        assert_eq!(out, [b"abcd".to_vec(), b"xy".to_vec()]);
    }

    #[test]
    fn length_prefixed_reads_a_little_endian_length() {
        let out = elements(&mut LengthPrefixed::<8>::new(), &[2, 0, 7, 8, 0, 0, 1, 0, 9]);
        assert_eq!(out, [vec![7, 8], Vec::new(), vec![9]]);
    }

    /// COBS nach Cheshire und Baker, als Vergleich.
    fn cobs_encode(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0];
        let mut code_at = 0;
        for &b in data {
            if b == 0 {
                out[code_at] = u8::try_from(out.len() - code_at).expect("Block");
                code_at = out.len();
                out.push(0);
            } else {
                out.push(b);
                if out.len() - code_at == 0xFF {
                    out[code_at] = 0xFF;
                    code_at = out.len();
                    out.push(0);
                }
            }
        }
        out[code_at] = u8::try_from(out.len() - code_at).expect("Block");
        out.push(0);
        out
    }

    /// Jede Nutzlast kommt so heraus, wie sie hineinging, auch mit Nullen,
    /// leer und ueber einen vollen Block von 254 Bytes hinaus.
    #[test]
    fn cobs_decodes_what_cobs_encodes() {
        let mut payloads: Vec<Vec<u8>> = vec![Vec::new(), vec![0], vec![0, 0], vec![1, 0, 2], vec![0x11; 3]];
        payloads.push((1..=254).collect());
        payloads.push((1..=255).collect());
        payloads.push((0..600u32).map(|i| u8::try_from(i % 7).expect("Byte")).collect());
        let mut framer = Cobs::<1024>::new();
        for payload in &payloads {
            let out = elements(&mut framer, &cobs_encode(payload));
            assert_eq!(out, [payload.clone()], "Nutzlast {payload:?}");
        }
    }

    /// Ein abgeschnittener Rahmen verschwindet, zwei Nullen sind keiner,
    /// und der Rahmer findet danach den naechsten.
    #[test]
    fn cobs_drops_a_cut_frame_and_resynchronises() {
        let mut bytes = vec![0, 0, 5, 1, 2, 0];
        bytes.extend(cobs_encode(&[9, 0, 9]));
        assert_eq!(elements(&mut Cobs::<16>::new(), &bytes), [vec![9, 0, 9]]);
    }
}
