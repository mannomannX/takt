//! Ereignisstroeme (Referenz 8.6, 9.6): Puffer, Fenster und Cursor.
//!
//! `buf[s]` lebt im Prozessabbild, `cur[s, m]` im Maschinenzustand — die
//! Aufteilung aus 9.1. Damit ist das Fenster `W` einer Aktivierung eine
//! Funktion zweier zu Tick-Beginn fester Groessen, und die
//! Ordnungsunabhaengigkeit (Satz 9.4.1) traegt ohne Zusatzarbeit
//! (plan/m2.md 1.2).
//!
//! Der Byte-Ring aus 8.6 ist eine Speicherdarstellung, keine beobachtbare
//! Semantik („Fenster und Cursor bleiben unveraendert"). Hier zaehlt nur
//! seine semantische Folge: die zweite Schranke `CAPB` neben `CAP`
//! (plan/m2.md 1.3).

use std::collections::VecDeque;

use crate::value::Value;

/// Ein Element im Puffer: `(seq, t, value)` nach 9.6.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// Laufende Nummer je Stream, streng steigend.
    pub seq: i64,
    /// Zeitstempel in Nanosekunden (Hardware-Aufloesung, 7.5).
    pub t: i64,
    /// Der Wert.
    pub value: Value,
    /// Bytelast dieses Elements (Byte-Ring, 8.6).
    pub bytes: u32,
}

/// Was beim Einlagern geschah (9.6, `deliver`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Alles eingelagert.
    Ok,
    /// Ueberlauf mit `overflow = fault`: jeder Konsument bekommt einen
    /// `StreamOverflow` bei seiner naechsten Aktivierung.
    Overflow,
    /// Ueberlauf mit `overflow = drop_oldest`: die aeltesten Elemente sind
    /// verworfen, gemeldet wird ein Alert.
    Dropped(u32),
}

/// Puffer eines Stroms: FIFO mit zwei Schranken (8.6, 9.6).
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    /// Elemente in seq-Reihenfolge.
    pub items: VecDeque<Element>,
    /// Naechste zu vergebende Nummer.
    pub next_seq: i64,
    /// Summe der Bytelasten in `items`.
    pub bytes: u32,
    /// `s.dropped`: verworfene Elemente (8.6).
    pub dropped: u32,
    /// `s.overflowed`: Ueberlaeufe mit `overflow = fault`.
    pub overflowed: u32,
    /// `s.malformed`: Elemente, die der Rand nicht dekodieren konnte.
    pub malformed: u32,
    /// Kapazitaet in Elementen (`CAP`).
    pub cap: u32,
    /// Kapazitaet in Bytes (`CAPB`).
    pub cap_bytes: u32,
}

impl Buffer {
    /// Leerer Puffer mit den Schranken des Stroms.
    pub fn new(cap: u32, cap_bytes: u32) -> Buffer {
        Buffer { cap, cap_bytes, ..Default::default() }
    }

    /// Lagert ein Element ein und vergibt seine Nummer (9.6, `deliver`).
    /// `drop_oldest` verwirft von vorn, `fault` lehnt das Element ab.
    pub fn push(&mut self, t: i64, value: Value, bytes: u32, drop_oldest: bool) -> Delivery {
        let over_count = self.items.len() as u32 + 1 > self.cap;
        let over_bytes = self.bytes + bytes > self.cap_bytes;
        if !(over_count || over_bytes) {
            self.append(t, value, bytes);
            return Delivery::Ok;
        }
        if !drop_oldest {
            // Das ueberzaehlige Element wird verworfen; der Fault trifft die
            // Konsumenten bei ihrer naechsten Aktivierung (9.6).
            self.overflowed += 1;
            return Delivery::Overflow;
        }
        let mut n = 0;
        while self.items.len() as u32 + 1 > self.cap || self.bytes + bytes > self.cap_bytes {
            let Some(old) = self.items.pop_front() else { break };
            self.bytes -= old.bytes;
            n += 1;
        }
        self.dropped += n;
        // Ein Element, das allein die Byteschranke sprengt, passt auch in
        // den geleerten Puffer nicht; es ist ein Ueberlauf, kein Verwerfen.
        if bytes > self.cap_bytes || self.cap == 0 {
            self.overflowed += 1;
            return Delivery::Overflow;
        }
        self.append(t, value, bytes);
        Delivery::Dropped(n)
    }

    fn append(&mut self, t: i64, value: Value, bytes: u32) {
        self.items.push_back(Element { seq: self.next_seq, t, value, bytes });
        self.next_seq += 1;
        self.bytes += bytes;
    }

    /// Fenster eines Konsumenten (9.6, `windows`): die Elemente ab seinem
    /// Cursor. Im Entry-Modus ist es leer — der Aufrufer prueft das.
    pub fn window(&self, cursor: i64) -> Vec<Element> {
        self.items.iter().filter(|e| e.seq >= cursor).cloned().collect()
    }

    /// Zahl der Elemente im Fenster (`s.count`, 8.6): liest ohne zu
    /// untersuchen, konsumiert also nichts.
    pub fn count(&self, cursor: i64) -> usize {
        self.items.iter().filter(|e| e.seq >= cursor).count()
    }

    /// Groesste vergebene Nummer plus eins: der Cursor nach `s.skip()`
    /// und nach dem Ueberspringen eines `idle`-Zustands (9.6).
    pub fn end(&self) -> i64 {
        self.next_seq
    }

    /// Entfernt Elemente, die kein Konsument mehr sehen kann (9.6,
    /// Eviction): alles unterhalb des kleinsten Cursors.
    pub fn evict(&mut self, min_cursor: i64) {
        while self.items.front().is_some_and(|e| e.seq < min_cursor) {
            if let Some(e) = self.items.pop_front() {
                self.bytes -= e.bytes;
            }
        }
    }
}

/// Bytelast eines Werts fuer den Byte-Ring (8.6). Elemente fester Groesse
/// zaehlen ihre Breite, Elemente variabler Laenge ihren Inhalt.
pub fn byte_len(v: &Value) -> u32 {
    match v {
        Value::Bytes(b) => b.len() as u32,
        Value::Str(s) => s.len() as u32,
        Value::Line { text, .. } => text.len() as u32,
        Value::Array(items) | Value::Vec(items) => items.iter().map(byte_len).sum(),
        Value::Record(fields) => fields.iter().map(byte_len).sum::<u32>().max(1),
        _ => 1,
    }
}
