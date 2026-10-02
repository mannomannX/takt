//! Was ein Bring-up seinen Treibern ueber den Wirtszeiger gibt (8.10, 12.6).
//!
//! Der Rahmen reicht den Zeiger, den der Wirt bei `P_init` uebergab, an
//! jeden Treiber weiter. Die eigenen Bring-ups uebergeben einen [`Host`]:
//! den Tick, den der Rahmen gerade rechnet. Ein Treiber, der nach Tick
//! liefert — das Pruefgeraet des Rands —, liest ihn dort statt ueber eine
//! Funktion des Rahmens; ausserhalb der Arena fuehrt ein Programm keinen
//! Zustand (12.11).
//!
//! Den Tick haelt ein `AtomicU32`, weil beide Zielkerne keine 64-Bit-Atomics
//! haben; geschrieben und gelesen wird er im selben Kontext, vor und
//! waehrend des Schritts. 2^32 Ticks reichen bei 10 ms fuer 497 Tage.

// TODO(M11 Schritt 5): Mit dem Trait `Drivers` ist der Wirtszeiger das
// Treiberobjekt des Wirts, und dieser Typ entfaellt.

use core::sync::atomic::{AtomicU32, Ordering};

/// Der Zustand, den die eigenen Bring-ups ihren Treibern zeigen.
#[derive(Debug, Default)]
pub struct Host {
    tick: AtomicU32,
}

impl Host {
    /// Vor dem Start: Tick 0.
    pub const fn new() -> Host {
        Host { tick: AtomicU32::new(0) }
    }

    /// Der Tick, den der Rahmen als naechstes rechnet; die Schleife setzt ihn
    /// vor dem Schritt.
    pub fn set_tick(&self, k: u64) {
        self.tick.store(k as u32, Ordering::Relaxed);
    }

    /// Der Tick, den der Rahmen gerade rechnet.
    pub fn tick(&self) -> u64 {
        u64::from(self.tick.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::Host;

    #[test]
    fn the_host_shows_the_tick_it_was_given() {
        let host = Host::new();
        assert_eq!(host.tick(), 0);
        host.set_tick(42);
        assert_eq!(host.tick(), 42);
    }
}
