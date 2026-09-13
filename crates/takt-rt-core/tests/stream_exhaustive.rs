//! Alle Folgen bis zur Tiefe sechs auf einem winzigen Ring (8.6, 9.6).
//!
//! Die Nachbartests messen gewaehlte Faelle und zufaellige Lasten; beide
//! koennen eine Ecke auslassen. Hier gibt es keine Auswahl: Bei drei
//! Plaetzen, vier Byte und vier Operationen je Schritt zaehlt der Test
//! **jede** Folge bis zur Laenge sechs durch — rund 4096 Ablaeufe, und
//! nach jedem Schritt ein vollstaendiger Abgleich gegen ein Modell.
//!
//! **Warum ein Modell.** Der Ring rechnet mit Kopf, Laenge und Modulo;
//! das Modell haelt dieselbe FIFO als schlichte Liste, ohne Umbruch und
//! ohne Versatz. Wo beide auseinandergehen, liegt der Fehler in der
//! Ringarithmetik — genau der Stelle, die sich nicht ansehen laesst.

use takt_rt_core::stream::{Delivery, Desc, Ring};

const CAP: usize = 3;
const CAPB: usize = 4;

/// Dieselbe FIFO ohne Ringarithmetik: die Referenz fuer den Abgleich.
#[derive(Default)]
struct Model {
    items: Vec<(i64, Vec<u8>)>,
    next_seq: i64,
    dropped: u32,
    overflowed: u32,
}

impl Model {
    fn bytes(&self) -> usize {
        self.items.iter().map(|(_, v)| v.len()).sum()
    }

    fn push(&mut self, content: &[u8], drop_oldest: bool) -> Delivery {
        if content.len() > CAPB {
            self.overflowed += 1;
            return Delivery::Overflow;
        }
        let fits = |m: &Self| m.items.len() < CAP && m.bytes() + content.len() <= CAPB;
        if !fits(self) {
            if !drop_oldest {
                self.overflowed += 1;
                return Delivery::Overflow;
            }
            let mut dropped_n = 0;
            while !fits(self) && !self.items.is_empty() {
                self.items.remove(0);
                dropped_n += 1;
            }
            self.dropped += dropped_n;
            self.items.push((self.next_seq, content.to_vec()));
            self.next_seq += 1;
            return Delivery::Dropped(dropped_n);
        }
        self.items.push((self.next_seq, content.to_vec()));
        self.next_seq += 1;
        Delivery::Ok
    }

    fn evict(&mut self, min_cursor: i64) {
        self.items.retain(|(seq, _)| *seq >= min_cursor);
    }

    /// Das Fenster ab dem Cursor (9.6).
    fn window(&self, cursor: i64) -> Vec<&(i64, Vec<u8>)> {
        self.items.iter().filter(|(seq, _)| *seq >= cursor).collect()
    }
}

/// Vergleicht Ring und Modell in allem, was ein Programm sehen kann.
fn same(r: &Ring<'_>, m: &Model, cursor: i64, at_label: &str) {
    assert_eq!(r.len(), m.items.len(), "{at_label}: die Zahl der Elemente");
    assert_eq!(r.end(), m.next_seq, "{at_label}: die naechste Nummer");
    assert_eq!(r.dropped, m.dropped, "{at_label}: `s.dropped`");
    assert_eq!(r.overflowed, m.overflowed, "{at_label}: `s.overflowed`");
    let window = m.window(cursor);
    assert_eq!(r.count(cursor), window.len(), "{at_label}: `s.count`");
    for (i, (seq, content)) in window.iter().enumerate() {
        let d = r.at(cursor, i).unwrap_or_else(|| panic!("{at_label}: Element {i} fehlt"));
        assert_eq!(d.seq, *seq, "{at_label}: `seq` von Element {i}");
        assert_eq!(d.len as usize, content.len(), "{at_label}: die Laenge von Element {i}");
        let mut buf = [0u8; CAPB];
        let n = r.read(d, &mut buf);
        assert_eq!(&buf[..n], &content[..], "{at_label}: der Inhalt von Element {i}");
    }
    assert!(r.at(cursor, window.len()).is_none(), "{at_label}: ein Element zu viel");
}

/// Vier Operationen: zwei Laengen, beide Ueberlaufregeln, lesen,
/// freigeben.
fn step_of(r: &mut Ring<'_>, m: &mut Model, cursor: &mut i64, op: u32, mark: u8) {
    match op {
        0 | 1 => {
            let n = if op == 0 { 1 } else { 3 };
            let drop_oldest = op == 1;
            let content = [mark, mark.wrapping_add(1), mark.wrapping_add(2)];
            let a = r.push(0, &content[..n], drop_oldest);
            let b = m.push(&content[..n], drop_oldest);
            assert_eq!(a, b, "die Lieferung weicht vom Modell ab");
        }
        2 => {
            // „Untersucht heisst konsumiert" (8.6): Der Cursor rueckt vor.
            if let Some(e) = r.at(*cursor, 0) {
                *cursor = e.seq + 1;
            }
        }
        _ => {
            r.evict(*cursor);
            m.evict(*cursor);
        }
    }
}

/// Jede Folge aus vier Operationen bis zur Laenge sechs.
#[test]
fn every_short_sequence_agrees_with_the_model() {
    const DEPTH: u32 = 6;
    let mut laeufe = 0u32;
    for code in 0..4u32.pow(DEPTH) {
        let (mut d, mut b) = ([Desc::default(); CAP], [0u8; CAPB]);
        let mut r = Ring::new(&mut d, &mut b);
        let mut m = Model::default();
        let mut cursor = 0i64;
        for i in 0..DEPTH {
            let op = (code >> (2 * i)) & 3;
            step_of(&mut r, &mut m, &mut cursor, op, (i * 10) as u8);
            same(&r, &m, cursor, &format!("Folge {code:#08b}, Schritt {i}"));
            // Auch ein Cursor abseits des Konsumenten muss stimmen.
            for c in [0i64, 1, 2, m.next_seq] {
                same(&r, &m, c, &format!("Folge {code:#08b}, Schritt {i}, Cursor {c}"));
            }
        }
        laeufe += 1;
    }
    assert_eq!(laeufe, 4096, "die Suche war nicht vollstaendig");
}
