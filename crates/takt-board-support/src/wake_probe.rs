//! Das Pruefgeraet der Weckereignisse (5.10, 9.9; FB-388): zwei
//! Wake-Quellen, deren Ereignisse das Programm nicht vorher kennt.
//!
//! `wake/level` steht ab [`LEVEL_AT_NS`], `wake/bell` bringt bei
//! [`BELL_AT_NS`] einmal [`BELL`]. Beide Zeitpunkte liegen zwischen zwei
//! Grenzen von [`TICK_NS`]: Ein Schritt sieht das Ereignis an der Grenze
//! danach ([`first_tick`]), mit und ohne Schlaf (Satz 9.9.1). Die Geraete
//! rechnen mit der Tickgrenze, die der Rahmen ihnen gibt, und laufen auf
//! Board und Wirt gleich; der Wirt macht aus denselben Zahlen den Stimulus
//! des Interpreters. Das Programm dazu steht in
//! `takt-conformance/tests/programs/wake.takt`.

/// Der Tick des Pruefprogramms.
pub const TICK_NS: i64 = 10_000_000;

/// Ab hier steht der Pegel `wake/level`.
pub const LEVEL_AT_NS: i64 = 123_400_000;

/// Hier laeutet `wake/bell`, einmal.
pub const BELL_AT_NS: i64 = 412_300_000;

/// Was die Klingel bringt.
pub const BELL: u8 = 7;

/// Der erste Tick, dessen Grenze `at` erreicht: Sein Schritt sieht das
/// Ereignis.
pub const fn first_tick(at: i64) -> u64 {
    (at.div_euclid(TICK_NS) + (at.rem_euclid(TICK_NS) != 0) as i64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_events_fall_between_two_boundaries() {
        assert_eq!(first_tick(LEVEL_AT_NS), 13);
        assert_eq!(first_tick(BELL_AT_NS), 42);
        assert_eq!(first_tick(3 * TICK_NS), 3, "auf der Grenze sieht ihn schon ihr Schritt");
    }
}
