//! Der Baudraten-Teiler eines STM32-USART (12.3).
//!
//! **Noch eine stille Fehlerquelle.** Ein falscher Teiler bricht nichts:
//! Die Schnittstelle sendet, nur liest sie niemand richtig — man sieht
//! Zeichensalat und sucht den Fehler im Programm. Die Rechnung steht
//! darum hier, wo sie getestet ist, und nicht neben dem Registerzugriff.
//!
//! Der STM32 fuehrt den Teiler als Festkommazahl: die oberen 12 Bit
//! ganzzahlig, die unteren 4 als Sechzehntel. `USARTDIV = pclk / baud`,
//! und das Register traegt `USARTDIV * 16` — die Bruchbits sind also
//! nicht Zierde, sondern die Stellen, die eine krumme Baudrate braucht.

/// Warum eine Baudrate nicht einstellbar ist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaudError {
    /// Null Baud gibt es nicht.
    Zero,
    /// Zu langsam fuer diesen Takt: Der Teiler passt nicht in 16 Bit.
    ///
    /// **Das ist eine harte Grenze, keine Rundung.** Bei 84 MHz sind 9600
    /// und 19200 Baud nicht erreichbar — der Teiler waere 140000 bzw.
    /// 70000, und `BRR` fasst 65535. Wer sie braucht, muss den Bustakt
    /// senken (APB-Prescaler), und das ist eine Entscheidung, keine
    /// Rechnung. Sie still auf `u16::MAX` zu klemmen hiesse, mit 20513
    /// Baud zu senden und den Nutzer raten zu lassen.
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
    // `* 16` fuer die Bruchbits, `+ baud/2` rundet statt abzuschneiden.
    let scaled = u64::from(pclk_hz) * 16 + u64::from(baud) / 2;
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
    u32::try_from(u64::from(pclk_hz) * 16 / u64::from(brr)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const F401: u32 = 84_000_000;

    /// 84 MHz, 115200 Baud: `84e6 * 16 / 115200 = 11666,67` → 11667.
    #[test]
    fn the_common_case_is_exact_enough() {
        let brr = divisor(F401, 115_200).expect("darstellbar");
        assert_eq!(brr, 11667);
        let error = actual_baud(F401, brr).abs_diff(115_200) * 1000 / 115_200;
        assert!(error < 5, "unter 0,5 Prozent Abweichung, gemessen: {error} Promille");
    }

    /// Eine glatte Rate trifft genau.
    #[test]
    fn a_clean_ratio_is_exact() {
        assert_eq!(divisor(16_000_000, 1_000_000), Ok(256), "16e6 * 16 / 1e6");
        assert_eq!(actual_baud(16_000_000, 256), 1_000_000);
    }

    /// **Gerundet, nicht abgeschnitten.**
    ///
    /// 84e6 * 16 / 115200 sind 11666,67. Abschneiden gaebe 11666 und
    /// machte die Schnittstelle systematisch zu schnell.
    #[test]
    fn the_divisor_rounds_to_nearest() {
        assert_eq!(divisor(F401, 115_200), Ok(11667));
    }

    #[test]
    fn a_zero_baud_rate_is_refused() {
        assert_eq!(divisor(F401, 0), Err(BaudError::Zero));
        assert_eq!(actual_baud(F401, 0), 0);
    }

    /// **Bei 84 MHz sind 9600 und 19200 Baud nicht erreichbar.**
    ///
    /// Der Teiler waere 140000 bzw. 70000, und `BRR` fasst 65535. Das ist
    /// eine harte Grenze des Chips, keine Rundung — und sie ist der Grund
    /// fuer [`BaudError`]: Ein stilles Klemmen auf `u16::MAX` sendete mit
    /// 20513 Baud und liesse den Nutzer raten, warum nichts ankommt.
    #[test]
    fn slow_rates_are_unreachable_at_full_clock() {
        assert_eq!(divisor(F401, 9_600), Err(BaudError::TooSlowForClock));
        assert_eq!(divisor(F401, 19_200), Err(BaudError::TooSlowForClock));
    }

    /// Mit langsamerem Bustakt gehen sie wieder.
    ///
    /// Der Ausweg, den die Fehlermeldung meint: Der APB-Prescaler senkt
    /// den Takt, und der Teiler passt wieder.
    #[test]
    fn slow_rates_work_at_a_lower_bus_clock() {
        assert!(divisor(21_000_000, 9_600).is_ok(), "bei 21 MHz passt 9600 Baud");
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
