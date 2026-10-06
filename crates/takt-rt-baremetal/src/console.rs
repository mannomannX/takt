//! Tunes von der Konsole (8.4, 11.1 `takt tune`): die Gegenrichtung der
//! Trace-Leitung.
//!
//! **Die Zeile.** Der Host schreibt je Tune `tune <index> <hex>`: den Index
//! des Parameters im Programm und seinen Wert in kanonischer Byteform (5.9)
//! als Hexziffern, zwei je Byte, in der Reihenfolge der Bytes. Name und Wert
//! als Text uebersetzt der Host mit dem Programm; das Board kennt die Namen
//! nicht und rechnet keine Dezimalzahl um (4.2). Typ und Range prueft der
//! Rahmen (`P_tune`) wie der Interpreter und schreibt die Zeile in den
//! Golden-Trace, eine verworfene mit ` rejected`.
//!
//! **Was keine Tune-Zeile ist, verwirft der Leser still**: Auf derselben
//! Leitung schreibt der Host auch `TAKT` (FB-275), und ein Rauschen beim
//! Anstecken ist keine Eingabe des Programms. Eine Zeile, die laenger ist
//! als jede Tune-Zeile, verwirft er bis zu ihrem Ende.

use takt_rt_core::Tunables;

/// So viele Bytes traegt ein Wert hoechstens: der breiteste Skalar (5.9).
pub const VALUE_BYTES: usize = 8;

/// Die laengste Tune-Zeile: `tune `, ein `u32` dezimal, ein Leerzeichen und
/// zwei Hexziffern je Byte.
const LINE: usize = 5 + 10 + 1 + 2 * VALUE_BYTES;

/// So viele Bytes liest [`Console`] je Grenze hoechstens (4.1, von Hand):
/// genug fuer einige Zeilen, ohne dass ein Host, der ohne Pause schreibt,
/// den Tick aufhaelt.
const BYTES_PER_POLL: usize = 4 * (LINE + 1);

/// Ein Tune aus einer Zeile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tune {
    /// Der Index des Parameters im Programm.
    pub param: u32,
    value: [u8; VALUE_BYTES],
    len: u8,
}

impl Tune {
    /// Der Wert in kanonischer Byteform.
    pub fn value(&self) -> &[u8] {
        &self.value[..usize::from(self.len)]
    }
}

/// Liest `tune <index> <hex>`; `None` fuer jede andere Zeile.
pub fn parse(line: &[u8]) -> Option<Tune> {
    let rest = line.strip_prefix(b"tune ")?;
    let mut words = rest.split(|b| *b == b' ').filter(|w| !w.is_empty());
    let (index, hex) = (words.next()?, words.next()?);
    if words.next().is_some() || index.is_empty() || hex.is_empty() || hex.len() % 2 != 0 {
        return None;
    }
    let mut param: u32 = 0;
    for d in index {
        let digit = u32::from(d.checked_sub(b'0').filter(|d| *d <= 9)?);
        param = param.checked_mul(10)?.checked_add(digit)?;
    }
    let mut tune = Tune { param, value: [0; VALUE_BYTES], len: 0 };
    for pair in hex.chunks_exact(2) {
        let slot = tune.value.get_mut(usize::from(tune.len))?;
        *slot = nibble(pair[0])? << 4 | nibble(pair[1])?;
        tune.len += 1;
    }
    Some(tune)
}

/// Der Wert einer Hexziffer; Gross- und Kleinbuchstaben gelten.
fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Die Konsole als Quelle von Tunables: setzt die Bytes der Gegenrichtung zu
/// Zeilen zusammen und gibt jede Tune-Zeile an der naechsten Grenze weiter,
/// die der Kern abfragt.
pub struct Console<F> {
    read: F,
    line: [u8; LINE],
    len: usize,
    /// Die laufende Zeile ist zu lang und wird bis zu ihrem Ende verworfen.
    overlong: bool,
}

impl<F: FnMut() -> Option<u8>> Console<F> {
    /// Liest mit `read` je Aufruf ein Byte der Gegenrichtung, `None`, wenn
    /// keines wartet.
    pub const fn new(read: F) -> Console<F> {
        Console { read, line: [0; LINE], len: 0, overlong: false }
    }

    /// Nimmt ein Byte; liefert den Tune der Zeile, die es abschliesst.
    pub fn feed(&mut self, b: u8) -> Option<Tune> {
        match b {
            b'\n' | b'\r' => {
                let tune = if self.overlong { None } else { parse(&self.line[..self.len]) };
                self.len = 0;
                self.overlong = false;
                tune
            }
            _ if self.len < LINE => {
                self.line[self.len] = b;
                self.len += 1;
                None
            }
            _ => {
                self.overlong = true;
                None
            }
        }
    }
}

impl<F: FnMut() -> Option<u8>> Tunables for Console<F> {
    fn poll(&mut self, _k: u64, apply: &mut dyn FnMut(u32, &[u8])) {
        for _ in 0..BYTES_PER_POLL {
            let Some(b) = (self.read)() else { return };
            if let Some(t) = self.feed(b) {
                apply(t.param, t.value());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::*;

    fn tune(param: u32, value: &[u8]) -> Tune {
        let mut t = Tune { param, value: [0; VALUE_BYTES], len: value.len() as u8 };
        t.value[..value.len()].copy_from_slice(value);
        t
    }

    #[test]
    fn a_tune_line_names_the_parameter_and_its_bytes() {
        assert_eq!(parse(b"tune 3 0700000000000000"), Some(tune(3, &[7, 0, 0, 0, 0, 0, 0, 0])));
        assert_eq!(parse(b"tune 0 01"), Some(tune(0, &[1])));
        assert_eq!(parse(b"tune 4294967295 Ff"), Some(tune(u32::MAX, &[0xff])));
        assert_eq!(parse(b"tune  12   abcd "), Some(tune(12, &[0xab, 0xcd])), "Leerraum zaehlt nicht doppelt");
    }

    #[test]
    fn anything_else_is_not_a_tune() {
        for line in [
            &b""[..],
            b"TAKT",
            b"tune",
            b"tune 1",
            b"tune 1 0",
            b"tune x 01",
            b"tune -1 01",
            b"tune 4294967296 01",
            b"tune 1 0g",
            b"tune 1 01 02",
            b"tune 1 010203040506070809",
            b"Tune 1 01",
        ] {
            assert_eq!(parse(line), None, "{:?}", std::string::String::from_utf8_lossy(line));
        }
    }

    /// Die Bytes kommen, wie die Leitung sie bringt: Zeilen ueber mehrere
    /// Abfragen, `\r\n` wie `\n`, eine ueberlange Zeile bis zu ihrem Ende
    /// verworfen, der Rest weiter gelesen.
    #[test]
    fn the_console_assembles_lines_across_polls() {
        let text = b"rauschen\r\ntune 2 0a\r\nx".iter().chain([b'y'; 80].iter()).chain(b"\ntune 1 ff00\n".iter());
        let mut bytes: std::collections::VecDeque<u8> = text.copied().collect();
        let mut console = Console::new(|| bytes.pop_front());
        let mut got: Vec<(u32, Vec<u8>)> = Vec::new();
        for k in 0..4 {
            console.poll(k, &mut |p, v| got.push((p, v.to_vec())));
        }
        assert_eq!(got, [(2, std::vec![0x0a]), (1, std::vec![0xff, 0x00])]);
    }

    /// Eine Abfrage liest hoechstens `BYTES_PER_POLL` Bytes; der Rest
    /// wartet auf die naechste Grenze.
    #[test]
    fn a_poll_reads_a_bounded_number_of_bytes() {
        let mut left = 10_000usize;
        let mut console = Console::new(|| {
            left = left.checked_sub(1)?;
            Some(b'z')
        });
        console.poll(0, &mut |_, _| {});
        assert_eq!(left, 10_000 - BYTES_PER_POLL);
    }
}
