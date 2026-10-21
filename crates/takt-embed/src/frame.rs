//! Die Rahmung eines Eingabestroms im Treiber (8.6, „Rahmung“; M11 Schritt
//! 22a): aus Bytes, eines nach dem anderen, die Elemente des Stroms.
//!
//! Die Regeln stehen in 8.6, und jede endet in einem Fall des Rands (12.6):
//!
//! - **Ein zu langes Element** liefert der Rahmer mit einem Byte ueber der
//!   Kapazitaet `M`; der Rand kuerzt es nach seinem Typ (`line<N>` mit
//!   `.truncated`) oder verwirft es als `malformed`. Was darueber hinaus
//!   kommt, verwirft der Rahmer bis zur Grenze des Elements.
//! - **Ein verlorenes Element** — ein Rahmen, den der Rahmer nicht lesen
//!   kann, oder eines, in dem die Quelle Bytes verlor — kostet [`Framed`]
//!   seine Folgenummer: Der Rand sieht die Luecke (Zeile 2).
//!
//! Jeder Rahmer tut je Byte feste Arbeit und haelt ein Element und ein Byte;
//! `M` ist die Kapazitaet des Elements im Strom (`N` bei `line<N>` und
//! `bytes<N>`, die Groesse eines Records). [`Framed`] macht aus einer
//! [`Source`] — einem [`Ring`], den eine ISR fuellt, einer seriellen
//! Schnittstelle — und einem Rahmer einen Eingabestrom.

use crate::ring::{Pop, Ring};
use crate::{Piece, StreamInput};

/// Was ein Byte beim Rahmer bewirkt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Das Element ist noch nicht fertig.
    Wait,
    /// Ein Element ist fertig; so viele Bytes stehen in `out`.
    Element(usize),
    /// Ein Element ist verloren: ein Rahmen, den der Rahmer nicht lesen kann.
    Lost,
}

/// Ein Rahmer: nimmt Bytes und meldet jedes fertige oder verlorene Element.
pub trait Framer {
    /// Nimmt das naechste Byte. Vollendet es ein Element, stehen dessen
    /// Bytes in `out`, hoechstens `out.len()` viele.
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Step;

    /// Die Quelle haelt gerade nichts bereit. Nur `raw` vollendet damit ein
    /// Element; `length_prefixed` und `fixed` finden damit nach einem
    /// Verlust wieder einen Anfang.
    fn pause(&mut self, out: &mut [u8]) -> Step;

    /// Die Quelle verlor Bytes: Das laufende Element ist verloren, und der
    /// Rahmer sucht den naechsten Anfang.
    fn lose(&mut self);

    /// Wie viele Bytes eine Abfrage hoechstens liest: zwei Elemente mit
    /// ihrer Rahmung. Was darueber hinaus wartet, bleibt in der Quelle.
    fn budget(&self) -> usize;
}

/// Was eine Quelle auf eine Frage hin hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    /// Das naechste Byte.
    Byte(u8),
    /// Hier fehlen Bytes, die die Quelle verlor.
    Lost,
    /// Gerade liegt nichts bereit.
    Empty,
}

/// Eine Quelle von Bytes, die der Schritt liest, ohne zu warten; sie sagt,
/// an welcher Stelle sie Bytes verlor.
pub trait Source {
    /// Das naechste Byte, eine Luecke oder nichts.
    fn pull(&mut self) -> Pull;
}

impl<const N: usize> Source for &Ring<N> {
    fn pull(&mut self) -> Pull {
        match self.pop() {
            Pop::Byte(byte) => Pull::Byte(byte),
            Pop::Lost => Pull::Lost,
            Pop::Empty => Pull::Empty,
        }
    }
}

/// Ein Eingabestrom aus einer Quelle und einem Rahmer (12.6): je `poll`
/// das naechste Element, mit der Tickgrenze als Zeitstempel und einer
/// eigenen Folgenummer, die ein verlorenes Element ueberspringt.
#[derive(Debug, Default)]
pub struct Framed<S, F> {
    /// Woher die Bytes kommen.
    pub source: S,
    /// Wie sie zu Elementen werden.
    pub framer: F,
    /// Die Folgenummer des naechsten Elements.
    seq: i64,
}

impl<S, F> Framed<S, F> {
    /// Ein Strom aus `source`, gerahmt von `framer`.
    pub const fn new(source: S, framer: F) -> Framed<S, F> {
        Framed { source, framer, seq: 0 }
    }
}

impl<S: Source, F: Framer> StreamInput for Framed<S, F> {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        for _ in 0..self.framer.budget() {
            let step = match self.source.pull() {
                Pull::Byte(byte) => self.framer.push(byte, buf),
                Pull::Lost => {
                    self.framer.lose();
                    Step::Lost
                }
                Pull::Empty => match self.framer.pause(buf) {
                    Step::Wait => return None,
                    step => step,
                },
            };
            let seq = self.seq;
            match step {
                Step::Wait => {}
                Step::Lost => self.seq = seq.saturating_add(1),
                Step::Element(len) => {
                    self.seq = seq.saturating_add(1);
                    return Some(Piece { len, t: now, seq: Some(seq) });
                }
            }
        }
        None
    }
}

/// Ein Element im Bau: hoechstens `M` Bytes, dazu das erste Byte darueber
/// hinaus, das ein zu langes Element anzeigt.
#[derive(Debug)]
struct Element<const M: usize> {
    bytes: [u8; M],
    len: usize,
    over: Option<u8>,
}

impl<const M: usize> Element<M> {
    const fn new() -> Element<M> {
        Element { bytes: [0; M], len: 0, over: None }
    }

    /// Haengt `byte` an; ueber `M` hinaus merkt es sich nur das erste.
    fn add(&mut self, byte: u8) {
        match self.bytes.get_mut(self.len) {
            Some(at) => {
                *at = byte;
                self.len = self.len.wrapping_add(1);
            }
            None => {
                self.over = self.over.or(Some(byte));
            }
        }
    }

    /// Ob es schon zu lang ist.
    fn full(&self) -> bool {
        self.len >= M
    }

    /// Ob noch nichts darin steht.
    fn is_empty(&self) -> bool {
        self.len == 0 && self.over.is_none()
    }

    /// Gibt das fertige Element nach `out`, ein zu langes mit einem Byte
    /// ueber `M`; danach beginnt das naechste. Die Zahl der Bytes in `out`.
    fn take(&mut self, out: &mut [u8]) -> usize {
        let len = core::mem::take(&mut self.len).min(out.len());
        if let (Some(to), Some(from)) = (out.get_mut(..len), self.bytes.get(..len)) {
            to.copy_from_slice(from);
        }
        match (self.over.take(), out.get_mut(len)) {
            (Some(byte), Some(at)) if len == M => {
                *at = byte;
                len.wrapping_add(1)
            }
            _ => len,
        }
    }

    /// Verwirft, was darin steht.
    fn clear(&mut self) {
        self.len = 0;
        self.over = None;
    }
}

/// Zwei Elemente mit Rahmung: so viel liest eine Abfrage hoechstens.
const fn budget_of(m: usize) -> usize {
    m.saturating_mul(2).saturating_add(4)
}

/// `framing = raw`: ein Element aus dem, was die Quelle gerade bereithaelt,
/// hoechstens `M` Bytes; bei einem Strom aus `u8` ist `M` eins und jedes
/// Byte ein Element.
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
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Step {
        self.0.add(byte);
        if self.0.full() { Step::Element(self.0.take(out)) } else { Step::Wait }
    }

    fn pause(&mut self, out: &mut [u8]) -> Step {
        if self.0.is_empty() { Step::Wait } else { Step::Element(self.0.take(out)) }
    }

    fn lose(&mut self) {
        self.0.clear();
    }

    fn budget(&self) -> usize {
        budget_of(M)
    }
}

/// `framing = fixed(N)`: je `M` Bytes ein Element; ein Rest wartet auf die
/// uebrigen.
#[derive(Debug)]
pub struct Fixed<const M: usize> {
    element: Element<M>,
    /// Nach einem Verlust: verwerfen, bis die Quelle einmal leer war.
    lost: bool,
}

impl<const M: usize> Fixed<M> {
    /// Ein Rahmer fuer Elemente aus `M` Bytes.
    pub const fn new() -> Fixed<M> {
        Fixed { element: Element::new(), lost: false }
    }
}

impl<const M: usize> Default for Fixed<M> {
    fn default() -> Fixed<M> {
        Fixed::new()
    }
}

impl<const M: usize> Framer for Fixed<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Step {
        if self.lost {
            return Step::Wait;
        }
        self.element.add(byte);
        if self.element.full() { Step::Element(self.element.take(out)) } else { Step::Wait }
    }

    fn pause(&mut self, _out: &mut [u8]) -> Step {
        self.lost = false;
        Step::Wait
    }

    fn lose(&mut self) {
        self.element.clear();
        self.lost = true;
    }

    fn budget(&self) -> usize {
        budget_of(M)
    }
}

/// `framing = lines`: ein Element je Zeile, ohne ihr Ende `\n`; ein `\r`
/// direkt davor gehoert zum Ende, jedes andere zum Text.
#[derive(Debug)]
pub struct Lines<const M: usize> {
    line: Element<M>,
    /// Ein `\r` steht noch aus: Folgt `\n`, gehoert es zum Ende.
    cr: bool,
    /// Nach einem Verlust: verwerfen bis zum naechsten `\n`.
    lost: bool,
}

impl<const M: usize> Lines<M> {
    /// Ein Rahmer fuer Zeilen; `M` ist das `N` von `line<N>`.
    pub const fn new() -> Lines<M> {
        Lines { line: Element::new(), cr: false, lost: false }
    }
}

impl<const M: usize> Default for Lines<M> {
    fn default() -> Lines<M> {
        Lines::new()
    }
}

impl<const M: usize> Framer for Lines<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Step {
        let cr = core::mem::replace(&mut self.cr, byte == b'\r');
        if byte == b'\n' {
            if core::mem::take(&mut self.lost) {
                return Step::Wait;
            }
            return Step::Element(self.line.take(out));
        }
        if self.lost {
            return Step::Wait;
        }
        if cr {
            self.line.add(b'\r');
        }
        if byte != b'\r' {
            self.line.add(byte);
        }
        Step::Wait
    }

    fn pause(&mut self, _out: &mut [u8]) -> Step {
        Step::Wait
    }

    fn lose(&mut self) {
        self.line.clear();
        self.cr = false;
        self.lost = true;
    }

    fn budget(&self) -> usize {
        budget_of(M)
    }
}

/// `framing = cobs`: ein Element je Rahmen, durch `0` getrennt und nach
/// COBS dekodiert. Nullen ohne etwas dazwischen sind Fuellung; ein Rahmen,
/// dessen Block nicht vor der naechsten Null endet, ist verloren.
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
    /// Nach einem Verlust: verwerfen bis zur naechsten Null.
    lost: bool,
}

impl<const M: usize> Cobs<M> {
    /// Ein Rahmer fuer Rahmen mit hoechstens `M` Bytes Nutzlast.
    pub const fn new() -> Cobs<M> {
        Cobs { frame: Element::new(), left: 0, zero: false, started: false, lost: false }
    }

    /// Beginnt den naechsten Rahmen.
    fn restart(&mut self) {
        self.frame.clear();
        self.left = 0;
        self.zero = false;
        self.started = false;
    }
}

impl<const M: usize> Default for Cobs<M> {
    fn default() -> Cobs<M> {
        Cobs::new()
    }
}

impl<const M: usize> Framer for Cobs<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Step {
        if byte == 0 {
            let step = match (core::mem::take(&mut self.lost), self.started, self.left) {
                (true, _, _) | (false, false, _) => Step::Wait,
                (false, true, 0) => Step::Element(self.frame.take(out)),
                (false, true, _) => Step::Lost,
            };
            self.restart();
            return step;
        }
        if self.lost {
            return Step::Wait;
        }
        if self.left > 0 {
            self.frame.add(byte);
            self.left = self.left.wrapping_sub(1);
            return Step::Wait;
        }
        // Ein Codebyte: Vor seinem Block steht die `0` des vorigen.
        if core::mem::replace(&mut self.started, true) && self.zero {
            self.frame.add(0);
        }
        self.zero = byte != 0xFF;
        self.left = byte.wrapping_sub(1);
        Step::Wait
    }

    fn pause(&mut self, _out: &mut [u8]) -> Step {
        Step::Wait
    }

    fn lose(&mut self) {
        self.restart();
        self.lost = true;
    }

    fn budget(&self) -> usize {
        // Je 254 Bytes ein Codebyte, dazu die Null.
        budget_of(M.saturating_add(M.checked_div(254).unwrap_or(0)))
    }
}

/// `framing = length_prefixed(u16_le)`: zwei Bytes Laenge, das
/// niederwertige zuerst, dann die Nutzlast. Eine Laenge ueber `M` liest er
/// ganz und liefert ein zu langes Element.
#[derive(Debug)]
pub struct LengthPrefixed<const M: usize> {
    payload: Element<M>,
    /// Die Bytes des Kopfs, die schon kamen.
    head: [u8; 2],
    have: u8,
    /// Wie viele Bytes der Nutzlast noch fehlen.
    left: u16,
    /// Nach einem Verlust: verwerfen, bis die Quelle einmal leer war.
    lost: bool,
}

impl<const M: usize> LengthPrefixed<M> {
    /// Ein Rahmer fuer Nutzlasten mit hoechstens `M` Bytes.
    pub const fn new() -> LengthPrefixed<M> {
        LengthPrefixed { payload: Element::new(), head: [0; 2], have: 0, left: 0, lost: false }
    }
}

impl<const M: usize> Default for LengthPrefixed<M> {
    fn default() -> LengthPrefixed<M> {
        LengthPrefixed::new()
    }
}

impl<const M: usize> Framer for LengthPrefixed<M> {
    fn push(&mut self, byte: u8, out: &mut [u8]) -> Step {
        if self.lost {
            return Step::Wait;
        }
        if self.have < 2 {
            if let Some(at) = self.head.get_mut(usize::from(self.have)) {
                *at = byte;
            }
            self.have = self.have.wrapping_add(1);
            if self.have < 2 {
                return Step::Wait;
            }
            self.left = u16::from_le_bytes(self.head);
        } else {
            self.payload.add(byte);
            self.left = self.left.wrapping_sub(1);
        }
        if self.left > 0 {
            return Step::Wait;
        }
        self.have = 0;
        Step::Element(self.payload.take(out))
    }

    fn pause(&mut self, _out: &mut [u8]) -> Step {
        self.lost = false;
        Step::Wait
    }

    fn lose(&mut self) {
        self.payload.clear();
        self.have = 0;
        self.left = 0;
        self.lost = true;
    }

    fn budget(&self) -> usize {
        budget_of(M)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cobs, Fixed, Framed, Framer, LengthPrefixed, Lines, Pull, Raw, Source, Step};
    use crate::StreamInput;
    use std::collections::VecDeque;
    use std::vec;
    use std::vec::Vec;

    /// Was ein Rahmer aus einer Folge macht: `Some(b)` ist ein Byte, `None`
    /// eine Pause der Quelle. Ein Element steht als seine Bytes da, ein
    /// verlorenes als `None`. `out` fasst ein Element und ein Byte, wie der
    /// Puffer des Rahmens.
    fn run<const M: usize>(framer: &mut impl Framer, input: &[Option<u8>]) -> Vec<Option<Vec<u8>>> {
        let mut out = [0u8; 64];
        let out = &mut out[..M + 1];
        let mut found = Vec::new();
        for item in input {
            let step = match item {
                Some(b) => framer.push(*b, out),
                None => framer.pause(out),
            };
            match step {
                Step::Wait => {}
                Step::Element(n) => found.push(Some(out[..n].to_vec())),
                Step::Lost => found.push(None),
            }
        }
        found
    }

    fn bytes(text: &[u8]) -> Vec<Option<u8>> {
        text.iter().map(|b| Some(*b)).collect()
    }

    fn el(text: &[u8]) -> Option<Vec<u8>> {
        Some(text.to_vec())
    }

    /// `raw`: was bereitliegt, ist ein Element, hoechstens `M` Bytes; was
    /// darueber hinaus kommt, beginnt das naechste, und nichts wartet.
    #[test]
    fn raw_takes_what_is_ready_in_pieces_of_at_most_m() {
        let mut input = bytes(b"abcde");
        input.push(None);
        input.extend(bytes(b"f"));
        input.push(None);
        let out = run::<3>(&mut Raw::<3>::new(), &input);
        assert_eq!(out, [el(b"abc"), el(b"de"), el(b"f")]);
        assert_eq!(run::<1>(&mut Raw::<1>::new(), &bytes(b"xy")), [el(b"x"), el(b"y")], "`u8`: jedes Byte");
    }

    /// `fixed(N)`: genau `N` Bytes; ein Rest wartet auf die uebrigen, auch
    /// ueber eine Pause hinweg.
    #[test]
    fn fixed_cuts_every_n_bytes_and_a_rest_waits() {
        let mut input = bytes(b"abcde");
        input.push(None);
        input.extend(bytes(b"f"));
        assert_eq!(run::<3>(&mut Fixed::<3>::new(), &input), [el(b"abc"), el(b"def")]);
    }

    /// `fixed(N)` nach einem Verlust: verwerfen, bis die Quelle einmal leer
    /// war, dann ein neuer Anfang.
    #[test]
    fn fixed_starts_over_after_a_loss_once_the_source_was_empty() {
        let mut framer = Fixed::<2>::new();
        assert_eq!(run::<2>(&mut framer, &bytes(b"a")), []);
        framer.lose();
        let mut input = bytes(b"bc");
        input.push(None);
        input.extend(bytes(b"de"));
        assert_eq!(run::<2>(&mut framer, &input), [el(b"de")]);
    }

    /// `lines`: das Ende ist `\n`; ein `\r` direkt davor gehoert zum Ende,
    /// jedes andere zum Text; eine leere Zeile ist ein leeres Element.
    #[test]
    fn lines_end_at_newline_and_a_carriage_return_right_before_belongs_to_the_end() {
        let out = run::<16>(&mut Lines::<16>::new(), &bytes(b"one\ntwo\r\n\nthr\ree\r\r\n"));
        assert_eq!(out, [el(b"one"), el(b"two"), el(b""), el(b"thr\ree\r")]);
    }

    /// Eine Zeile ueber `line<N>` kommt mit einem Byte mehr, damit der Rand
    /// sie kuerzt und `.truncated` setzt (3.9); der Rest bis `\n` faellt
    /// weg, und die naechste beginnt richtig.
    #[test]
    fn a_long_line_comes_one_byte_over_and_the_next_starts_clean() {
        let out = run::<4>(&mut Lines::<4>::new(), &bytes(b"abcdefgh\nxy\nwxyz\n"));
        assert_eq!(out, [el(b"abcde"), el(b"xy"), el(b"wxyz")]);
    }

    /// Ein Rest ohne `\n` wartet, auch ueber Pausen hinweg.
    #[test]
    fn a_rest_without_newline_waits_for_its_end() {
        let mut input = bytes(b"par");
        input.push(None);
        input.extend(bytes(b"t\n"));
        assert_eq!(run::<8>(&mut Lines::<8>::new(), &input), [el(b"part")]);
    }

    /// Nach einem Verlust beginnt das naechste Element hinter dem naechsten
    /// `\n`.
    #[test]
    fn lines_resume_after_the_next_newline_after_a_loss() {
        let mut framer = Lines::<8>::new();
        assert_eq!(run::<8>(&mut framer, &bytes(b"ab")), []);
        framer.lose();
        assert_eq!(run::<8>(&mut framer, &bytes(b"cd\nef\n")), [el(b"ef")]);
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
    /// leer und ueber einen vollen Block von 254 Bytes hinaus; Nullen davor
    /// sind Fuellung.
    #[test]
    fn cobs_decodes_what_cobs_encodes() {
        let mut payloads: Vec<Vec<u8>> = vec![Vec::new(), vec![0], vec![0, 0], vec![1, 0, 2], vec![0x11; 3]];
        payloads.push((1..=254).collect());
        payloads.push((1..=255).collect());
        let mut framer = Cobs::<63>::new();
        for payload in payloads.iter().filter(|p| p.len() <= 63) {
            let mut input = bytes(&[0, 0]);
            input.extend(bytes(&cobs_encode(payload)));
            assert_eq!(run::<63>(&mut framer, &input), [Some(payload.clone())], "Nutzlast {payload:?}");
        }
        let mut long = Cobs::<600>::new();
        let mut out = vec![0u8; 601];
        for payload in payloads.iter().filter(|p| p.len() > 63) {
            let steps: Vec<Step> = cobs_encode(payload).iter().map(|b| long.push(*b, &mut out)).collect();
            assert_eq!(steps.last(), Some(&Step::Element(payload.len())));
            assert_eq!(&out[..payload.len()], &payload[..]);
        }
    }

    /// Ein Block, der nicht vor der naechsten Null endet, macht den Rahmen
    /// ungueltig: verloren, und der naechste kommt heil.
    #[test]
    fn a_cut_cobs_frame_is_lost_and_the_next_comes_whole() {
        let mut input = bytes(&[5, 1, 2, 0]);
        input.extend(bytes(&cobs_encode(&[9, 0, 9])));
        assert_eq!(run::<16>(&mut Cobs::<16>::new(), &input), [None, Some(vec![9, 0, 9])]);
    }

    /// Eine Nutzlast ueber der Kapazitaet kommt mit einem Byte mehr; der
    /// Rand kuerzt oder verwirft sie nach ihrem Typ.
    #[test]
    fn a_cobs_payload_over_capacity_comes_one_byte_over() {
        let out = run::<2>(&mut Cobs::<2>::new(), &bytes(&cobs_encode(&[1, 2, 3, 4])));
        assert_eq!(out, [Some(vec![1, 2, 3])]);
    }

    /// Nach einem Verlust beginnt der naechste Rahmen hinter der naechsten
    /// Null.
    #[test]
    fn cobs_resumes_after_the_next_zero_after_a_loss() {
        let mut framer = Cobs::<8>::new();
        assert_eq!(run::<8>(&mut framer, &bytes(&[3, 1])), []);
        framer.lose();
        let mut input = bytes(&[1, 0]);
        input.extend(bytes(&cobs_encode(&[7])));
        assert_eq!(run::<8>(&mut framer, &input), [Some(vec![7])]);
    }

    /// `length_prefixed(u16_le)`: Laenge, niederwertiges Byte zuerst, dann
    /// die Nutzlast; Laenge null ist ein leeres Element.
    #[test]
    fn length_prefixed_reads_a_little_endian_length() {
        let out = run::<8>(&mut LengthPrefixed::<8>::new(), &bytes(&[2, 0, 7, 8, 0, 0, 1, 0, 9]));
        assert_eq!(out, [Some(vec![7, 8]), Some(Vec::new()), Some(vec![9])]);
    }

    /// Eine Laenge ueber der Kapazitaet liest er ganz, liefert ein Byte mehr
    /// und findet die naechste Grenze aus der Laenge.
    #[test]
    fn a_length_over_capacity_is_read_whole_and_comes_one_byte_over() {
        let out = run::<2>(&mut LengthPrefixed::<2>::new(), &bytes(&[4, 0, 1, 2, 3, 4, 1, 0, 5]));
        assert_eq!(out, [Some(vec![1, 2, 3]), Some(vec![5])]);
    }

    /// Nach einem Verlust: verwerfen, bis die Quelle einmal leer war; danach
    /// ist das naechste Byte ein Kopf.
    #[test]
    fn length_prefixed_starts_over_after_a_loss_once_the_source_was_empty() {
        let mut framer = LengthPrefixed::<4>::new();
        assert_eq!(run::<4>(&mut framer, &bytes(&[3, 0, 1])), []);
        framer.lose();
        let mut input = bytes(&[2, 3]);
        input.push(None);
        input.extend(bytes(&[1, 0, 6]));
        assert_eq!(run::<4>(&mut framer, &input), [Some(vec![6])]);
    }

    /// Eine Quelle aus einer Folge: `Some` ein Byte, `None` eine Luecke.
    struct Queue(VecDeque<Option<u8>>);

    impl Source for Queue {
        fn pull(&mut self) -> Pull {
            match self.0.pop_front() {
                Some(Some(b)) => Pull::Byte(b),
                Some(None) => Pull::Lost,
                None => Pull::Empty,
            }
        }
    }

    /// Je verlorenes Element eine Folgenummer weniger: ein ungueltiger
    /// Rahmen und eine Luecke der Quelle. Der Rand sieht beide (12.6 Zeile 2).
    #[test]
    fn every_lost_element_costs_its_sequence_number() {
        let mut input: Vec<Option<u8>> = cobs_encode(&[1]).into_iter().map(Some).collect();
        input.extend([Some(5), Some(1), Some(0)]);
        input.extend(cobs_encode(&[2]).into_iter().map(Some));
        input.extend([Some(2), None, Some(0)]);
        input.extend(cobs_encode(&[3]).into_iter().map(Some));
        let mut rx = Framed::new(Queue(input.into()), Cobs::<4>::new());
        let mut buf = [0u8; 5];
        let seqs: Vec<(Vec<u8>, Option<i64>)> =
            core::iter::from_fn(|| rx.poll(&mut buf, 0).map(|p| (buf[..p.len].to_vec(), p.seq))).collect();
        assert_eq!(seqs, [(vec![1], Some(0)), (vec![2], Some(2)), (vec![3], Some(4))]);
    }

    /// Eine Abfrage liest hoechstens zwei Elemente mit Rahmung; was dahinter
    /// wartet, bleibt fuer die naechste.
    #[test]
    fn a_poll_reads_a_bounded_number_of_bytes() {
        let endless: VecDeque<Option<u8>> = core::iter::repeat_n(Some(b'x'), 1000).collect();
        let mut rx = Framed::new(Queue(endless), Lines::<8>::new());
        let mut buf = [0u8; 9];
        assert!(rx.poll(&mut buf, 0).is_none());
        assert_eq!(rx.source.0.len(), 1000 - (2 * 8 + 4));
    }
}
