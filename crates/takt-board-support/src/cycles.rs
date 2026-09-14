//! Zyklen in Zeit (12.3, 13.8).
//!
//! Die Umrechnung, aus der `c_target` entsteht: Der DWT zaehlt Zyklen,
//! die Kalibrierung braucht Nanosekunden. Das Lesen des Registers steht
//! im Board-Crate — die Rechnung hier, weil sie einen Ueberlauf hat, den
//! man testen will.

/// Eine abgeschlossene Messung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    /// Wie viele Zyklen vergangen sind.
    pub cycles: u32,
    /// Die Taktfrequenz des Kerns in Hertz.
    pub core_hz: u32,
}

impl Measurement {
    /// Die Dauer in Nanosekunden.
    ///
    /// **Die Rechnung laeuft in `u64`, und das ist nicht Vorsicht.**
    /// `cycles * 1e9` ueberschreitet `u32` schon bei 4295 Zyklen — bei
    /// 84 MHz sind das 51 Mikrosekunden. Jede Messung, die laenger dauert
    /// als ein Wimpernschlag, waere ohne die Erweiterung falsch.
    pub fn ns(self) -> i64 {
        if self.core_hz == 0 {
            return 0;
        }
        let ns = u64::from(self.cycles) * 1_000_000_000 / u64::from(self.core_hz);
        i64::try_from(ns).unwrap_or(i64::MAX)
    }

    /// Die Differenz zweier Zaehlerstaende.
    ///
    /// Der DWT-Zaehler ist 32-bittig und laeuft bei 84 MHz alle 51
    /// Sekunden ueber. `wrapping_sub` liefert trotzdem die richtige
    /// Differenz, solange zwischen beiden Punkten weniger als ein
    /// Ueberlauf liegt — was fuer eine Messung immer gilt, sonst waere sie
    /// keine.
    pub fn between(start: u32, end: u32, core_hz: u32) -> Measurement {
        Measurement { cycles: end.wrapping_sub(start), core_hz }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F401: u32 = 84_000_000;

    #[test]
    fn a_measurement_converts_to_nanoseconds() {
        assert_eq!(Measurement { cycles: 84_000, core_hz: F401 }.ns(), 1_000_000, "84000 Zyklen bei 84 MHz sind 1 ms");
    }

    /// Der Fall, den eine `u32`-Rechnung verloere.
    #[test]
    fn a_measurement_beyond_u32_nanoseconds_is_exact() {
        // 4295 Zyklen mal 1e9 liegt ueber u32::MAX.
        let m = Measurement { cycles: 4295, core_hz: F401 };
        assert_eq!(m.ns(), 51_130, "51 Mikrosekunden, nicht ein Ueberlauf");
    }

    #[test]
    fn the_longest_measurement_is_exact() {
        let m = Measurement { cycles: u32::MAX, core_hz: F401 };
        // 4294967295 * 1e9 / 84e6, ganzzahlig abgerundet.
        assert_eq!(m.ns(), 51_130_563_035, "gut 51 Sekunden");
    }

    #[test]
    fn a_zero_clock_is_survivable() {
        assert_eq!(Measurement { cycles: 1000, core_hz: 0 }.ns(), 0);
    }

    /// **Die Differenz stimmt auch ueber den Ueberlauf hinweg.**
    ///
    /// Bei 84 MHz laeuft der DWT alle 51 Sekunden ueber. Eine Messung,
    /// die zufaellig darueber liegt, darf nicht negativ werden — sie ist
    /// ja nicht rueckwaerts gelaufen.
    #[test]
    fn a_measurement_across_the_counter_wrap_is_correct() {
        let m = Measurement::between(u32::MAX - 99, 100, F401);
        assert_eq!(m.cycles, 200, "100 vor dem Ueberlauf, 100 danach");
    }

    #[test]
    fn a_measurement_without_elapsed_cycles_is_zero() {
        assert_eq!(Measurement::between(42, 42, F401).cycles, 0);
    }
}
