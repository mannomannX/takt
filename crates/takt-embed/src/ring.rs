//! Ein Ring fuer einen Schreiber und einen Leser (12.6, M11 Schritt 22a):
//! Der Treiber eines Stroms schreibt in seiner ISR, `poll` liest im
//! Schritt, ohne Sperre und ohne `unsafe`.
//!
//! **Nur Laden und Speichern.** Kopf und Ende schreibt je nur ihre Seite;
//! jedes Byte ist ein eigenes Atomic. So braucht der Ring kein
//! Compare-and-Swap und laeuft auch auf Kernen ohne (`thumbv6m`). Schreiben
//! ihn zwei Seiten zugleich, gehen Bytes verloren, aber nie Speicher: Es
//! gibt keinen Zugriff ausserhalb der Atomics. Alle Zugriffe sind `SeqCst`
//! (Stufe 2, plan/cert.md Abschnitt 6).
//!
//! **Kopf und Ende laufen modulo `2 N`.** So unterscheiden sie einen vollen
//! Ring von einem leeren, und der Platz eines Bytes ist ihr Rest modulo
//! `N` fuer jedes `N`, nicht nur fuer Zweierpotenzen.
//!
//! **Kein still verlorenes Byte** (8.6, „Rahmung“). Ist der Ring voll,
//! merkt sich der Schreiber die Stelle der Luecke und nimmt bis zu ihr
//! nichts mehr an; erreicht der Leser sie, meldet er [`Pop::Lost`] und der
//! Schreiber nimmt wieder an. So ist jede Luecke eine Stelle im Strom, und
//! ein Rahmer weiss, welches Element sie traf.

use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicU8, AtomicUsize};

/// Kein Stand: Es gibt keine Luecke.
const NO_GAP: usize = usize::MAX;

/// Was der Leser aus dem Ring bekommt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pop {
    /// Das aelteste Byte.
    Byte(u8),
    /// Hier fehlen Bytes, die der Schreiber in einem vollen Ring verwarf.
    Lost,
    /// Nichts wartet.
    Empty,
}

/// Ein Ring aus `N` Bytes; `static` angelegt, teilen ihn ISR und Schritt.
///
/// ```
/// use takt_embed::ring::{Pop, Ring};
/// static RX: Ring<64> = Ring::new();
/// assert!(RX.push(b'a'));
/// assert_eq!(RX.pop(), Pop::Byte(b'a'));
/// ```
#[derive(Debug)]
pub struct Ring<const N: usize> {
    slots: [AtomicU8; N],
    /// Der naechste Platz des Schreibers, modulo `2 N`.
    head: AtomicUsize,
    /// Der naechste Platz des Lesers, modulo `2 N`.
    tail: AtomicUsize,
    /// Der Stand der Luecke, die der Leser noch nicht erreichte, oder
    /// [`NO_GAP`]. Der Schreiber setzt ihn, der Leser loescht ihn.
    gap: AtomicUsize,
    /// Wie viele Bytes der Schreiber verwarf; die Zahl bleibt an ihrer
    /// Grenze stehen.
    lost: AtomicUsize,
}

impl<const N: usize> Ring<N> {
    /// Der Umlauf von Kopf und Ende.
    const SPAN: usize = {
        assert!(N > 0 && N <= usize::MAX / 4, "ein Ring hat 1 bis usize::MAX / 4 Plaetze");
        N.wrapping_mul(2)
    };

    /// Ein leerer Ring.
    pub const fn new() -> Ring<N> {
        let _ = Self::SPAN;
        Ring {
            slots: [const { AtomicU8::new(0) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            gap: AtomicUsize::new(NO_GAP),
            lost: AtomicUsize::new(0),
        }
    }

    /// Der Stand nach `at`.
    fn next(at: usize) -> usize {
        at.wrapping_add(1).checked_rem(Self::SPAN).unwrap_or(0)
    }

    /// Wie viele Bytes zwischen Ende und Kopf liegen.
    fn between(head: usize, tail: usize) -> usize {
        let wrapped = if head < tail { Self::SPAN } else { 0 };
        head.wrapping_add(wrapped).wrapping_sub(tail)
    }

    /// Der Platz eines Stands.
    fn slot(&self, at: usize) -> Option<&AtomicU8> {
        self.slots.get(at.checked_rem(N)?)
    }

    /// Schreiber: legt `byte` ab. `false`, wenn der Ring voll ist oder eine
    /// Luecke noch aussteht; das Byte geht verloren, zaehlt in
    /// [`Ring::lost`] und gehoert zur Luecke, die der Leser meldet.
    pub fn push(&self, byte: u8) -> bool {
        let head = self.head.load(SeqCst);
        let full = Self::between(head, self.tail.load(SeqCst)) >= N;
        let pending = self.gap.load(SeqCst) != NO_GAP;
        if let Some(slot) = self.slot(head).filter(|_| !full && !pending) {
            slot.store(byte, SeqCst);
            self.head.store(Self::next(head), SeqCst);
            return true;
        }
        self.lost.store(self.lost.load(SeqCst).saturating_add(1), SeqCst);
        // Schloss der Leser die Luecke, waehrend dieses Byte verworfen wurde,
        // beginnt mit ihm eine neue.
        if self.gap.load(SeqCst) == NO_GAP {
            self.gap.store(head, SeqCst);
        }
        false
    }

    /// Leser: das aelteste Byte, die Luecke, an der Bytes fehlen, oder
    /// nichts.
    pub fn pop(&self) -> Pop {
        let tail = self.tail.load(SeqCst);
        if tail == self.head.load(SeqCst) {
            // Bis zur Luecke nimmt der Schreiber nichts an: Ist der Ring
            // leer, steht der Leser an ihr.
            if self.gap.load(SeqCst) == NO_GAP {
                return Pop::Empty;
            }
            self.gap.store(NO_GAP, SeqCst);
            return Pop::Lost;
        }
        let Some(slot) = self.slot(tail) else { return Pop::Empty };
        let byte = slot.load(SeqCst);
        self.tail.store(Self::next(tail), SeqCst);
        Pop::Byte(byte)
    }

    /// Wie viele Bytes warten.
    pub fn len(&self) -> usize {
        Self::between(self.head.load(SeqCst), self.tail.load(SeqCst))
    }

    /// Ob kein Byte wartet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Wie viele Bytes der Schreiber seit dem Start verwarf.
    pub fn lost(&self) -> usize {
        self.lost.load(SeqCst)
    }
}

impl<const N: usize> Default for Ring<N> {
    fn default() -> Ring<N> {
        Ring::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Pop, Ring};
    use std::vec::Vec;

    /// Was der Leser bekommt, bis der Ring leer ist; eine Luecke als `None`.
    fn drain<const N: usize>(ring: &Ring<N>) -> Vec<Option<u8>> {
        core::iter::from_fn(|| match ring.pop() {
            Pop::Byte(b) => Some(Some(b)),
            Pop::Lost => Some(None),
            Pop::Empty => None,
        })
        .collect()
    }

    #[test]
    fn bytes_come_out_in_the_order_they_went_in() {
        let ring: Ring<4> = Ring::new();
        for b in [1, 2, 3] {
            assert!(ring.push(b));
        }
        assert_eq!((ring.pop(), ring.pop()), (Pop::Byte(1), Pop::Byte(2)));
        assert!(ring.push(4) && ring.push(5));
        assert_eq!(drain(&ring), [Some(3), Some(4), Some(5)]);
        assert!(ring.is_empty());
    }

    /// Ein voller Ring verwirft, was kommt, und zaehlt es. Der Leser bekommt
    /// erst alles davor, dann genau eine Luecke, und erst danach nimmt der
    /// Schreiber wieder an — auch wenn zwischendurch Platz frei wurde.
    #[test]
    fn a_full_ring_leaves_one_gap_where_the_bytes_went_missing() {
        let ring: Ring<2> = Ring::new();
        assert!(ring.push(1) && ring.push(2));
        assert!(!ring.push(3));
        assert_eq!(ring.pop(), Pop::Byte(1));
        assert!(!ring.push(4), "bis zur Luecke nimmt der Schreiber nichts an");
        assert_eq!(ring.lost(), 2);
        assert_eq!(drain(&ring), [Some(2), None]);
        assert!(ring.push(5));
        assert_eq!(drain(&ring), [Some(5)]);
    }

    /// Viele Umlaeufe mit einer Zahl von Plaetzen, die keine Zweierpotenz
    /// ist: Was ankommt, ist die Folge mit einer Luecke genau dort, wo
    /// Bytes verloren gingen.
    #[test]
    fn many_laps_keep_every_byte_or_mark_its_gap() {
        let ring: Ring<3> = Ring::new();
        let mut sent: Vec<Option<u8>> = Vec::new();
        let mut got: Vec<Option<u8>> = Vec::new();
        let mut next = 0u8;
        for lap in 0..300usize {
            for _ in 0..=(lap % 5) {
                if ring.push(next) {
                    sent.push(Some(next));
                } else if sent.last() != Some(&None) {
                    sent.push(None);
                }
                next = next.wrapping_add(1);
            }
            for _ in 0..(lap % 4) {
                match ring.pop() {
                    Pop::Byte(b) => got.push(Some(b)),
                    Pop::Lost => got.push(None),
                    Pop::Empty => {}
                }
            }
        }
        got.extend(drain(&ring));
        assert_eq!(got, sent);
    }

    /// Eine ISR auf einem Faden schreibt, der Schritt liest auf einem
    /// anderen: Jedes Byte kommt genau einmal und in seiner Reihenfolge an,
    /// oder an seiner Stelle steht eine Luecke.
    #[test]
    fn one_writer_and_one_reader_on_two_threads_agree() {
        static RING: Ring<16> = Ring::new();
        // Unter Miri reicht ein Bruchteil: Jeder Schritt der beiden Faeden ist
        // dort tausendfach teurer.
        const COUNT: u32 = if cfg!(miri) { 300 } else { 20_000 };
        let byte = |i: u32| u8::try_from(i % 251).unwrap_or(0);
        let writer = std::thread::spawn(move || {
            let mut i = 0u32;
            while i < COUNT {
                if RING.push(byte(i)) {
                    i += 1;
                }
            }
        });
        let mut seen = 0u32;
        while seen < COUNT {
            match RING.pop() {
                Pop::Byte(b) => {
                    assert_eq!(b, byte(seen), "Byte {seen}");
                    seen += 1;
                }
                // Der Schreiber wiederholt ein verworfenes Byte; die Luecke
                // steht vor ihm, und kein Byte fehlt.
                Pop::Lost | Pop::Empty => {}
            }
        }
        writer.join().expect("Schreiber");
    }
}
