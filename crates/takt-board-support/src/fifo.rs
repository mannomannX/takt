//! Ein Byte-FIFO zwischen Programm und Interrupt (12.2, 12.3).
//!
//! **Ein Schreiber, ein Leser.** Das Programm legt Bytes hinein, ein
//! Interrupt nimmt sie heraus — der Sendeweg einer Leitung ohne
//! Hardware-FIFO. Keiner von beiden wartet auf den anderen: Ist der FIFO
//! voll, sagt `push` nein, und der Ring davor zaehlt, was er verwirft
//! (12.2: „wer nicht mitkommt, verwirft und zaehlt").
//!
//! Die Plaetze sind `AtomicU8`: So braucht der FIFO kein `unsafe`, und auf
//! einem Kern ohne Cache sind atomare Byte-Zugriffe gewoehnliche Lade- und
//! Speicherbefehle. Die Indizes laufen frei und werden erst beim Zugriff
//! gefaltet; `head == tail` heisst leer, `tail - head == N` voll. Das
//! Falten traegt ueber den Ueberlauf der Indizes nur, wenn `N` den
//! Wertebereich teilt — darum ist `N` eine Zweierpotenz, geprueft beim
//! Uebersetzen.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

/// Ein FIFO fuer `N` Bytes mit einem Schreiber und einem Leser.
pub struct ByteFifo<const N: usize> {
    slots: [AtomicU8; N],
    /// Der naechste Platz zum Lesen; nur der Leser schreibt ihn.
    head: AtomicUsize,
    /// Der naechste Platz zum Schreiben; nur der Schreiber schreibt ihn.
    tail: AtomicUsize,
}

impl<const N: usize> ByteFifo<N> {
    /// `N` ist eine Zweierpotenz; sonst uebersetzt der FIFO nicht.
    const POWER_OF_TWO: () = assert!(N.is_power_of_two(), "ByteFifo braucht eine Zweierpotenz als Groesse");

    /// Ein leerer FIFO; `const` fuer statische FIFOs.
    pub const fn new() -> ByteFifo<N> {
        let () = Self::POWER_OF_TWO;
        ByteFifo { slots: [const { AtomicU8::new(0) }; N], head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Legt ein Byte hinein; falsch, wenn kein Platz ist. Nur der Schreiber.
    pub fn push(&self, b: u8) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail.wrapping_sub(self.head.load(Ordering::Acquire)) >= N {
            return false;
        }
        self.slots[tail & (N - 1)].store(b, Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    /// Nimmt das aelteste Byte heraus. Nur der Leser.
    pub fn pop(&self) -> Option<u8> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        let b = self.slots[head & (N - 1)].load(Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(b)
    }

    /// Wie viele Bytes warten.
    pub fn len(&self) -> usize {
        self.tail.load(Ordering::Acquire).wrapping_sub(self.head.load(Ordering::Acquire))
    }

    /// Wartet nichts?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<const N: usize> Default for ByteFifo<N> {
    fn default() -> ByteFifo<N> {
        ByteFifo::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_come_out_in_order() {
        let f = ByteFifo::<4>::new();
        assert!(f.push(1) && f.push(2));
        assert_eq!((f.pop(), f.pop(), f.pop()), (Some(1), Some(2), None));
    }

    #[test]
    fn a_full_fifo_refuses_without_losing_what_it_holds() {
        let f = ByteFifo::<2>::new();
        assert!(f.push(7) && f.push(8));
        assert!(!f.push(9), "voll");
        assert_eq!(f.len(), 2);
        assert_eq!((f.pop(), f.pop()), (Some(7), Some(8)));
        assert!(f.is_empty());
    }

    /// Die Indizes laufen ueber; der FIFO merkt es nicht.
    #[test]
    fn the_indices_wrap_without_harm() {
        let f = ByteFifo::<4>::new();
        f.head.store(usize::MAX - 1, Ordering::Relaxed);
        f.tail.store(usize::MAX - 1, Ordering::Relaxed);
        for b in 0..4 {
            assert!(f.push(b));
        }
        assert!(!f.push(4));
        assert_eq!((f.pop(), f.pop(), f.pop(), f.pop(), f.pop()), (Some(0), Some(1), Some(2), Some(3), None));
    }
}
