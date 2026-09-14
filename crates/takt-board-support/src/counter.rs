//! Ein 64-Bit-Zaehler aus zwei atomaren 32-Bit-Haelften (12.3).
//!
//! **Warum nicht einfach ein `AtomicU64`.** ARMv7E-M hat keine
//! 64-Bit-Atomics (`rustc --print cfg` nennt 8, 16, 32 und ptr). Ein
//! `u64`, den eine ISR schreibt und die Hauptschleife liest, waere ohne
//! sie ein halb geschriebener Wert — und der Fehler traete genau einmal
//! alle 2^32 Ticks auf, also nach Tagen, einmal, unreproduzierbar.
//!
//! Die Loesung ist das Muster, mit dem jede 64-Bit-Uhr auf einem
//! 32-Bit-Kern gelesen wird: zwei Haelften, und der Leser prueft, ob die
//! obere waehrend seiner Lesung stehen geblieben ist.
//!
//! **Das ist testbar, und darum steht es hier.** Die Logik hat einen
//! Fehlerfall, der auf Hardware praktisch nie auftritt und im Test in
//! einer Zeile herzustellen ist.

use core::sync::atomic::{AtomicU32, Ordering};

/// Ein monoton steigender Zaehler ueber zwei atomare Haelften.
#[derive(Debug, Default)]
pub struct Counter64 {
    /// Die unteren 32 Bit; von der ISR erhoeht.
    low: AtomicU32,
    /// Wie oft [`Counter64::low`] uebergelaufen ist.
    high: AtomicU32,
}

impl Counter64 {
    /// Ein Zaehler bei null.
    pub const fn new() -> Counter64 {
        Counter64 { low: AtomicU32::new(0), high: AtomicU32::new(0) }
    }

    /// Erhoeht den Zaehler um eins — aus der ISR zu rufen.
    ///
    /// `fetch_add` liefert den Wert *vor* der Erhoehung: `u32::MAX`
    /// heisst, dass dieser Schritt auf null zurueckfaellt und die obere
    /// Haelfte mitzaehlen muss.
    pub fn tick(&self) {
        if self.low.fetch_add(1, Ordering::Relaxed) == u32::MAX {
            self.high.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Der Stand als 64-Bit-Wert.
    ///
    /// Beide Haelften werden einzeln gelesen, also kann die ISR dazwischen
    /// feuern. Gefaehrlich ist allein der Ueberlauf-Augenblick: Liest man
    /// die obere Haelfte davor und die untere danach, entstuende ein
    /// Sprung um 2^32 zurueck. Die Schleife liest die obere darum zweimal
    /// und nimmt das Ergebnis erst, wenn sie stehen geblieben ist.
    pub fn get(&self) -> u64 {
        loop {
            let hi = self.high.load(Ordering::Relaxed);
            let lo = self.low.load(Ordering::Relaxed);
            if self.high.load(Ordering::Relaxed) == hi {
                return (u64::from(hi) << 32) | u64::from(lo);
            }
        }
    }

    /// Setzt den Stand — nur fuer Tests und den Start.
    pub fn set(&self, value: u64) {
        self.high.store((value >> 32) as u32, Ordering::Relaxed);
        self.low.store(value as u32, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_counter_is_zero() {
        assert_eq!(Counter64::new().get(), 0);
    }

    #[test]
    fn ticking_counts_up() {
        let c = Counter64::new();
        for _ in 0..5 {
            c.tick();
        }
        assert_eq!(c.get(), 5);
    }

    /// **Der Fall, der auf Hardware nach 49 Tagen eintritt.**
    ///
    /// Bei 1 ms Tick laeuft die untere Haelfte nach 2^32 ms ueber — das
    /// sind 49,7 Tage. Ohne die obere Haelfte spraenge der Zaehler dort
    /// auf null zurueck, und die logische Zeit liefe rueckwaerts.
    #[test]
    fn the_counter_survives_the_wrap() {
        let c = Counter64::new();
        c.set(u64::from(u32::MAX));
        c.tick();
        assert_eq!(c.get(), u64::from(u32::MAX) + 1, "der Ueberlauf traegt in die obere Haelfte");
    }

    /// Und der zweite Ueberlauf ebenso.
    #[test]
    fn the_counter_survives_the_second_wrap() {
        let c = Counter64::new();
        c.set(2 * u64::from(u32::MAX) + 1);
        c.tick();
        assert_eq!(c.get(), 2 * u64::from(u32::MAX) + 2);
    }

    /// Ein gesetzter Wert kommt unveraendert zurueck — beide Haelften
    /// muessen dabei getroffen sein.
    #[test]
    fn setting_and_reading_agree() {
        let c = Counter64::new();
        for v in [0, 1, u64::from(u32::MAX), u64::from(u32::MAX) + 1, 0x1234_5678_9ABC_DEF0] {
            c.set(v);
            assert_eq!(c.get(), v, "Wert {v:#x}");
        }
    }

    /// Der Zaehler bleibt monoton, auch ueber viele Ueberlaeufe.
    ///
    /// Der Test steht hier, weil ein Fehler in der Zusammensetzung sich
    /// als *Ruecksprung* zeigt — und ein Ruecksprung in der logischen
    /// Zeit bricht Satz 9.4.1.
    #[test]
    fn the_counter_never_goes_backwards() {
        let c = Counter64::new();
        c.set(u64::from(u32::MAX) - 2);
        let mut last = c.get();
        for _ in 0..8 {
            c.tick();
            let now = c.get();
            assert!(now > last, "{now} nach {last}");
            last = now;
        }
    }
}
