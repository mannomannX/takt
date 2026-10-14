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

use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicU8, AtomicUsize};

/// Ein Ring aus `N` Bytes; `static` angelegt, teilen ihn ISR und Schritt.
///
/// ```
/// use takt_embed::ring::Ring;
/// static RX: Ring<64> = Ring::new();
/// assert!(RX.push(b'a'));
/// assert_eq!(RX.pop(), Some(b'a'));
/// ```
#[derive(Debug)]
pub struct Ring<const N: usize> {
    slots: [AtomicU8; N],
    /// Der naechste Platz des Schreibers, modulo `2 N`.
    head: AtomicUsize,
    /// Der naechste Platz des Lesers, modulo `2 N`.
    tail: AtomicUsize,
    /// Wie viele Bytes der Schreiber verwarf, weil der Ring voll war; die
    /// Zahl bleibt an ihrer Grenze stehen.
    lost: AtomicUsize,
}

impl<const N: usize> Ring<N> {
    /// Der Umlauf von Kopf und Ende.
    const SPAN: usize = {
        assert!(N > 0 && N <= usize::MAX / 2, "ein Ring hat 1 bis usize::MAX / 2 Plaetze");
        N.wrapping_mul(2)
    };

    /// Ein leerer Ring.
    pub const fn new() -> Ring<N> {
        let _ = Self::SPAN;
        Ring {
            slots: [const { AtomicU8::new(0) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
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

    /// Schreiber: legt `byte` ab. `false`, wenn der Ring voll ist; das Byte
    /// geht verloren und zaehlt in [`Ring::lost`].
    pub fn push(&self, byte: u8) -> bool {
        let head = self.head.load(SeqCst);
        let full = Self::between(head, self.tail.load(SeqCst)) >= N;
        match self.slot(head).filter(|_| !full) {
            Some(slot) => {
                slot.store(byte, SeqCst);
                self.head.store(Self::next(head), SeqCst);
                true
            }
            None => {
                self.lost.store(self.lost.load(SeqCst).saturating_add(1), SeqCst);
                false
            }
        }
    }

    /// Leser: das aelteste Byte; `None`, wenn keines wartet.
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(SeqCst);
        if tail == self.head.load(SeqCst) {
            return None;
        }
        let byte = self.slot(tail)?.load(SeqCst);
        self.tail.store(Self::next(tail), SeqCst);
        Some(byte)
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
    use super::Ring;
    use std::vec::Vec;

    fn drain<const N: usize>(ring: &Ring<N>) -> Vec<u8> {
        core::iter::from_fn(|| ring.pop()).collect()
    }

    #[test]
    fn bytes_come_out_in_the_order_they_went_in() {
        let ring: Ring<4> = Ring::new();
        for b in [1, 2, 3] {
            assert!(ring.push(b));
        }
        assert_eq!((ring.pop(), ring.pop()), (Some(1), Some(2)));
        assert!(ring.push(4) && ring.push(5));
        assert_eq!(drain(&ring), [3, 4, 5]);
        assert!(ring.is_empty());
    }

    /// Ein voller Ring verwirft, was kommt, und zaehlt es; was schon drin
    /// ist, bleibt.
    #[test]
    fn a_full_ring_drops_and_counts_what_comes() {
        let ring: Ring<2> = Ring::new();
        assert!(ring.push(1) && ring.push(2));
        assert!(!ring.push(3));
        assert_eq!((ring.len(), ring.lost()), (2, 1));
        assert_eq!(drain(&ring), [1, 2]);
    }

    /// Viele Umlaeufe mit einer Zahl von Plaetzen, die keine Zweierpotenz
    /// ist: Jeder Fuellstand von leer bis voll kommt vor, und nichts geht
    /// verloren oder kommt doppelt.
    #[test]
    fn many_laps_keep_every_byte() {
        let ring: Ring<3> = Ring::new();
        let mut next = 0u8;
        let mut expected = 0u8;
        for lap in 0..200usize {
            for _ in 0..=(lap % 4).min(3) {
                if ring.push(next) {
                    next = next.wrapping_add(1);
                }
            }
            for _ in 0..(lap % 3) {
                if let Some(b) = ring.pop() {
                    assert_eq!(b, expected, "Umlauf {lap}");
                    expected = expected.wrapping_add(1);
                }
            }
            assert!(ring.len() <= 3);
        }
        for b in drain(&ring) {
            assert_eq!(b, expected);
            expected = expected.wrapping_add(1);
        }
        assert_eq!(expected, next);
    }

    /// Eine ISR auf einem Faden schreibt, der Schritt liest auf einem
    /// anderen: Nichts geht verloren, nichts kommt doppelt, die Reihenfolge
    /// haelt.
    #[test]
    fn one_writer_and_one_reader_on_two_threads_agree() {
        static RING: Ring<16> = Ring::new();
        const COUNT: u32 = 20_000;
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
            if let Some(b) = RING.pop() {
                assert_eq!(b, byte(seen), "Byte {seen}");
                seen += 1;
            }
        }
        writer.join().expect("Schreiber");
    }
}
