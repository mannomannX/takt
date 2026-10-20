//! Der Briefkasten zweier Programme auf einem Kern (12.11, M11 Schritt 16):
//! der Treiber, der einen Ausgang des einen Programms mit einem Eingang des
//! anderen verbindet. Gemeinsamen Speicher der Arenen gibt es nicht; der
//! Briefkasten gehoert dem Wirt, und der Rand (12.6) gilt auf beiden Seiten.
//!
//! **Was er haelt:** den Wert und die Grenze, die der Rahmen dem Treiber des
//! Schreibers beim Commit nennt. Der Leser bekommt beides; ob der Stempel
//! der erwartete ist, prueft der Rahmen des Lesers (12.11).
//!
//! **Ohne Warten, ohne zerrissene Werte.** Schreiber und Leser rechnen in
//! verschiedenen Prioritaeten eines Kerns, und keiner darf auf den anderen
//! warten: Wer hoeher ist, laeuft durch, bis er fertig ist. Der Schreiber
//! fuellt darum den Platz, den der Leser gerade nicht liest, und schaltet
//! dann um; jeder Platz traegt eine Fassung, die waehrend des Schreibens
//! ungerade ist. Ist der Leser der niedrigere und schrieb der hoehere
//! dazwischen, liest er noch einmal — der hoehere ist dann fertig. Ist der
//! Leser der hoehere, kann der Schreiber ihn nicht unterbrechen, und der
//! gerade aktive Platz ist ganz.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Ein Platz: Fassung, Wert und Stempel, je in zwei Haelften (der
/// Cortex-M4 hat keine atomaren 64 Bit).
struct Cell {
    version: AtomicU32,
    value: [AtomicU32; 2],
    stamp: [AtomicU32; 2],
}

impl Cell {
    const fn new() -> Cell {
        Cell {
            version: AtomicU32::new(0),
            value: [AtomicU32::new(0), AtomicU32::new(0)],
            stamp: [AtomicU32::new(0), AtomicU32::new(0)],
        }
    }
}

/// Legt `x` in zwei Haelften ab.
fn store(halves: &[AtomicU32; 2], x: i64) {
    let ([low, high], [low_cell, high_cell]) = (split(x), halves);
    low_cell.store(low, Ordering::Relaxed);
    high_cell.store(high, Ordering::Relaxed);
}

/// Liest `x` aus zwei Haelften.
fn load([low, high]: &[AtomicU32; 2]) -> i64 {
    join([low.load(Ordering::Relaxed), high.load(Ordering::Relaxed)])
}

/// Ein Briefkasten fuer einen Wert mit Stempel.
pub struct Mailbox {
    cells: [Cell; 2],
    /// Der Platz, den der Leser liest; `usize::MAX`, solange nichts geschrieben ist.
    active: AtomicUsize,
}

impl Mailbox {
    /// Ein leerer Briefkasten: Der Leser bekommt nichts, bis der Schreiber
    /// den ersten Wert legt.
    pub const fn new() -> Mailbox {
        Mailbox { cells: [Cell::new(), Cell::new()], active: AtomicUsize::new(usize::MAX) }
    }

    /// Legt `value` mit dem Stempel `stamp` ab; aus dem Kontext des Schreibers.
    pub fn write(&self, value: i64, stamp: i64) {
        let [first, second] = &self.cells;
        let (next, cell) = match self.active.load(Ordering::Acquire) {
            0 => (1, second),
            _ => (0, first),
        };
        cell.version.fetch_add(1, Ordering::AcqRel);
        store(&cell.value, value);
        store(&cell.stamp, stamp);
        cell.version.fetch_add(1, Ordering::AcqRel);
        self.active.store(next, Ordering::Release);
    }

    /// Der zuletzt abgelegte Wert mit Stempel; `None`, solange nichts
    /// abgelegt ist.
    pub fn read(&self) -> Option<(i64, i64)> {
        loop {
            let at = self.active.load(Ordering::Acquire);
            let cell = self.cells.get(at)?;
            let before = cell.version.load(Ordering::Acquire);
            let (value, stamp) = (load(&cell.value), load(&cell.stamp));
            let after = cell.version.load(Ordering::Acquire);
            if before == after && before & 1 == 0 {
                return Some((value, stamp));
            }
        }
    }
}

impl Default for Mailbox {
    fn default() -> Mailbox {
        Mailbox::new()
    }
}

/// Die untere und die obere Haelfte eines Stempels.
fn split(stamp: i64) -> [u32; 2] {
    let [a, b, c, d, e, f, g, h] = stamp.to_le_bytes();
    [u32::from_le_bytes([a, b, c, d]), u32::from_le_bytes([e, f, g, h])]
}

/// Der Stempel aus seinen Haelften.
fn join([low, high]: [u32; 2]) -> i64 {
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    i64::from_le_bytes([a, b, c, d, e, f, g, h])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Leer bis zum ersten Wert, danach der letzte mit seinem Stempel.
    #[test]
    fn the_last_value_with_its_stamp() {
        let m = Mailbox::new();
        assert_eq!(m.read(), None);
        m.write(7, 1_000_000);
        m.write(8, -5);
        assert_eq!(m.read(), Some((8, -5)));
        m.write(-9, i64::MAX);
        assert_eq!(m.read(), Some((-9, i64::MAX)));
    }
}
