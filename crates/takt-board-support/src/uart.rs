//! Der Baudraten-Teiler eines STM32-USART (12.3).
//!
//! **Noch eine stille Fehlerquelle.** Ein falscher Teiler bricht nichts:
//! Die Schnittstelle sendet, nur liest sie niemand richtig — man sieht
//! Zeichensalat und sucht den Fehler im Programm. Die Rechnung steht
//! darum hier, wo sie getestet ist, und nicht neben dem Registerzugriff.
//!
//! Der STM32 fuehrt den Teiler als Festkommazahl: die oberen 12 Bit
//! ganzzahlig, die unteren 4 als Sechzehntel. Die Rate ist
//! `pclk / (16 * USARTDIV)` (RM0368, 16-fache Ueberabtastung), und das
//! Register traegt `USARTDIV * 16` — also schlicht `pclk / baud`. Die
//! erste Fassung multiplizierte hier noch einmal mit 16 und sendete mit
//! einem Sechzehntel der Rate; auf dem Board fiel es erst auf, als ein
//! Lauf den Trace bei 7200 statt 115200 Baud lesbar fand (FB-274).

/// Warum eine Baudrate nicht einstellbar ist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaudError {
    /// Null Baud gibt es nicht.
    Zero,
    /// Zu langsam fuer diesen Takt: Der Teiler passt nicht in 16 Bit.
    ///
    /// **Das ist eine harte Grenze, keine Rundung.** Bei 84 MHz liegt sie
    /// bei 1282 Baud (`84e6 / 65535`). Wer weniger braucht, muss den
    /// Bustakt senken (APB-Prescaler), und das ist eine Entscheidung,
    /// keine Rechnung. Sie still auf `u16::MAX` zu klemmen hiesse, mit
    /// 1282 Baud zu senden und den Nutzer raten zu lassen.
    TooSlowForClock,
}

/// Der Registerwert fuer `BRR`.
///
/// Rundet auf das naechste Sechzehntel: Das Restbit Abweichung ist bei
/// UART unkritisch (die Norm erlaubt einige Prozent ueber einen Rahmen).
/// Was *nicht* gerundet wird, ist eine Rate, die gar nicht hineinpasst —
/// siehe [`BaudError::TooSlowForClock`].
pub fn divisor(pclk_hz: u32, baud: u32) -> Result<u16, BaudError> {
    if baud == 0 {
        return Err(BaudError::Zero);
    }
    // `+ baud/2` rundet statt abzuschneiden.
    let scaled = u64::from(pclk_hz) + u64::from(baud) / 2;
    u16::try_from(scaled / u64::from(baud)).map_err(|_| BaudError::TooSlowForClock)
}

/// Die Baudrate, die ein Registerwert tatsaechlich ergibt.
///
/// Die Gegenrechnung: Wer wissen will, wie weit die Rundung traegt,
/// rechnet zurueck. `takt driver-test` (13.8) wird das brauchen — die
/// tatsaechliche Rate gehoert in die Hardware-Konfiguration, nicht die
/// gewuenschte.
pub fn actual_baud(pclk_hz: u32, brr: u16) -> u32 {
    if brr == 0 {
        return 0;
    }
    pclk_hz / u32::from(brr)
}

#[cfg(test)]
mod tests {
    use super::*;

    const F401: u32 = 84_000_000;

    /// 84 MHz, 115200 Baud: `84e6 / 115200 = 729,17` → 729.
    #[test]
    fn the_common_case_is_exact_enough() {
        let brr = divisor(F401, 115_200).expect("darstellbar");
        assert_eq!(brr, 729);
        let error = actual_baud(F401, brr).abs_diff(115_200) * 1000 / 115_200;
        assert!(error < 5, "unter 0,5 Prozent Abweichung, gemessen: {error} Promille");
    }

    /// Eine glatte Rate trifft genau.
    #[test]
    fn a_clean_ratio_is_exact() {
        assert_eq!(divisor(16_000_000, 1_000_000), Ok(16), "16e6 / 1e6");
        assert_eq!(actual_baud(16_000_000, 16), 1_000_000);
    }

    /// **Gerundet, nicht abgeschnitten.**
    ///
    /// 84e6 / 230400 sind 364,58. Abschneiden gaebe 364 und machte die
    /// Schnittstelle systematisch zu schnell.
    #[test]
    fn the_divisor_rounds_to_nearest() {
        assert_eq!(divisor(F401, 230_400), Ok(365));
    }

    #[test]
    fn a_zero_baud_rate_is_refused() {
        assert_eq!(divisor(F401, 0), Err(BaudError::Zero));
        assert_eq!(actual_baud(F401, 0), 0);
    }

    /// 9600 und 19200 Baud sind bei 84 MHz gewoehnliche Raten.
    ///
    /// Die erste Fassung hielt sie fuer unerreichbar — der Teiler war
    /// sechzehnfach zu gross (FB-274).
    #[test]
    fn ordinary_slow_rates_fit() {
        assert_eq!(divisor(F401, 9_600), Ok(8750));
        assert_eq!(divisor(F401, 19_200), Ok(4375));
    }

    /// **Unter 1282 Baud passt der Teiler bei 84 MHz nicht mehr.**
    ///
    /// Das ist eine harte Grenze des Chips, keine Rundung — und sie ist
    /// der Grund fuer [`BaudError`]: Ein stilles Klemmen auf `u16::MAX`
    /// sendete mit 1282 Baud und liesse den Nutzer raten, warum nichts
    /// ankommt.
    #[test]
    fn very_slow_rates_are_unreachable_at_full_clock() {
        assert_eq!(divisor(F401, 1_000), Err(BaudError::TooSlowForClock));
        assert_eq!(divisor(F401, 1_281), Err(BaudError::TooSlowForClock));
    }

    /// Mit langsamerem Bustakt gehen sie wieder.
    ///
    /// Der Ausweg, den die Fehlermeldung meint: Der APB-Prescaler senkt
    /// den Takt, und der Teiler passt wieder.
    #[test]
    fn very_slow_rates_work_at_a_lower_bus_clock() {
        assert_eq!(divisor(21_000_000, 1_000), Ok(21000), "bei 21 MHz passt 1000 Baud");
    }

    /// Die erreichbaren Raten liegen alle innerhalb eines Prozents.
    #[test]
    fn every_reachable_rate_stays_within_one_percent() {
        for baud in [38_400, 57_600, 115_200, 230_400, 460_800, 921_600] {
            let brr = divisor(F401, baud).unwrap_or_else(|e| panic!("{baud} Baud: {e:?}"));
            let error = actual_baud(F401, brr).abs_diff(baud) * 100 / baud;
            assert!(error < 1, "{baud} Baud weicht um {error} Prozent ab (BRR {brr})");
        }
    }
}
