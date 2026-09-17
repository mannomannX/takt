//! Perioden, Prescaler, Timer-Schritte (7.1, 12.3).
//!
//! Die Rechnung zwischen `system: tick` und dem, was ein Hardware-Timer
//! zaehlt. Sie ist kurz und sieht harmlos aus — und ist genau deshalb
//! gefaehrlich: Ein falscher Prescaler bricht nichts, er verschiebt nur
//! die Zeit. Ein Programm mit halber Tickrate laeuft, rechnet, gibt aus;
//! nur stimmt nichts davon. Darum steht sie hier und nicht im
//! Board-Crate, wo sie ungetestet bliebe.

/// Warum eine Periode nicht darstellbar ist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodError {
    /// Kuerzer als ein Zaehlschritt des Timers.
    TooShort,
    /// Laenger, als der Zaehler fassen kann.
    TooLong,
    /// Der Timer laeuft mit null Hertz — es gibt keine Schritte.
    NoClock,
}

/// Die Dauer eines Zaehlschritts in Nanosekunden.
///
/// `None` bei einer Frequenz von null: Ein Timer ohne Takt hat keine
/// Schrittdauer, und `1e9 / 0` waere eine Division durch null.
pub fn ns_per_count(timer_hz: u32) -> Option<i64> {
    (timer_hz > 0).then(|| 1_000_000_000 / i64::from(timer_hz))
}

/// Zaehlschritte fuer eine Periode.
///
/// `counts` ist die Zahl der Schritte, die der Timer durchlaeuft — das
/// Register `arr` traegt einen weniger, weil der Zaehler bei null
/// beginnt. Diese Unterscheidung ist eine klassische Fehlerquelle; sie
/// steht darum in zwei Funktionen statt in einer mit einem Kommentar.
pub fn counts_for(timer_hz: u32, period_ns: i64) -> Result<u32, PeriodError> {
    if timer_hz == 0 {
        return Err(PeriodError::NoClock);
    }
    // Exakt in `u128`, nicht ueber `ns_per_count`: Bei 16 MHz ist ein
    // Schritt 62,5 ns, und die Ganzzahl 62 verschoebe jede Periode um
    // acht Promille (Board 2).
    let counts = u128::from(period_ns.max(0).unsigned_abs()) * u128::from(timer_hz) / 1_000_000_000;
    if counts == 0 {
        return Err(PeriodError::TooShort);
    }
    u32::try_from(counts).map_err(|_| PeriodError::TooLong)
}

/// Der Prescaler, der aus der Eingangsfrequenz die Zielfrequenz macht.
///
/// Der Registerwert ist der Teiler minus eins. `None`, wenn die Zielfrequenz
/// nicht ganzzahlig teilt oder null ist — ein krummer Teiler waere eine
/// Rundungsquelle in der Groessenordnung, die `tick_tolerance` (7.1)
/// messen soll, und darum lieber ein Fehler als eine Naeherung.
pub fn prescaler_for(input_hz: u32, target_hz: u32) -> Option<u16> {
    if target_hz == 0 || input_hz == 0 || input_hz % target_hz != 0 {
        return None;
    }
    u16::try_from(input_hz / target_hz - 1).ok()
}

/// Die tatsaechliche Periode, die eine Zaehlerzahl ergibt.
///
/// Die Gegenrechnung zu [`counts_for`]: Was kommt heraus, wenn der Timer
/// so laeuft, wie er gesetzt wurde? Weicht es von `system: tick` ab, ist
/// die Konfiguration falsch — und das faellt beim Start auf statt spaeter
/// als Drift.
pub fn period_ns(timer_hz: u32, counts: u32) -> i64 {
    if timer_hz == 0 {
        return 0;
    }
    i64::try_from(u128::from(counts) * 1_000_000_000 / u128::from(timer_hz)).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MHZ: u32 = 1_000_000;

    #[test]
    fn one_millisecond_is_a_thousand_counts_at_one_megahertz() {
        assert_eq!(counts_for(MHZ, 1_000_000), Ok(1000));
    }

    #[test]
    fn a_period_shorter_than_one_count_is_rejected() {
        assert_eq!(counts_for(MHZ, 999), Err(PeriodError::TooShort));
    }

    /// Ein 32-Bit-Timer fasst nicht mehr als `u32::MAX` Schritte.
    #[test]
    fn a_period_beyond_the_counter_is_rejected() {
        let too_long = (i64::from(u32::MAX) + 2) * 1_000;
        assert_eq!(counts_for(MHZ, too_long), Err(PeriodError::TooLong));
    }

    #[test]
    fn the_longest_representable_period_is_accepted() {
        assert_eq!(counts_for(MHZ, i64::from(u32::MAX) * 1_000), Ok(u32::MAX));
    }

    #[test]
    fn a_stopped_timer_has_no_counts() {
        assert_eq!(counts_for(0, 1_000_000), Err(PeriodError::NoClock));
        assert_eq!(ns_per_count(0), None);
    }

    /// 84 MHz auf 1 MHz sind Teiler 84, also Registerwert 83.
    #[test]
    fn the_prescaler_register_holds_one_less_than_the_divisor() {
        assert_eq!(prescaler_for(84 * MHZ, MHZ), Some(83));
    }

    /// **Ein krummer Teiler ist ein Fehler, keine Naeherung.**
    ///
    /// 84 MHz auf 9 MHz ginge nur mit 9,33 — und die Abweichung laege bei
    /// 3,7 Prozent, also ueber dem Default von `tick_tolerance` (2 pct,
    /// 7.1). Eine Rundung hier erzeugte genau den Fehler, den die Pruefung
    /// dort spaeter meldet.
    #[test]
    fn a_non_integral_divisor_is_refused() {
        assert_eq!(prescaler_for(84 * MHZ, 9 * MHZ), None);
    }

    #[test]
    fn a_zero_target_has_no_prescaler() {
        assert_eq!(prescaler_for(84 * MHZ, 0), None);
    }

    /// Hin und zurueck muss dasselbe ergeben.
    #[test]
    fn the_period_round_trips() {
        for ns in [1_000, 100_000, 1_000_000, 10_000_000] {
            let counts = counts_for(MHZ, ns).expect("darstellbar");
            assert_eq!(period_ns(MHZ, counts), ns, "Periode {ns} ns");
        }
    }
}
