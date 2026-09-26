//! Der Watchdog ohne Register (12.3): was ein Board zu seiner Frist rechnet.

/// Vorteiler (`PR`, 0 fuer /4 bis 6 fuer /256) und Nachladewert (`RLR`,
/// 12 Bit) des IWDG einer STM32 fuer die Frist `timeout_ns` bei einem
/// LSI von `lsi_hz` (RM0368 17.3).
///
/// Der feinste Vorteiler, der die Frist fasst, und aufgerundet: Der
/// Watchdog schlaegt nie vor ihr zu. Laenger als der groesste Wert geht
/// es nicht; dann der groesste.
pub fn iwdg(timeout_ns: i64, lsi_hz: u32) -> (u8, u16) {
    const NS: u128 = 1_000_000_000;
    let product = u128::try_from(timeout_ns.max(0)).unwrap_or(0) * u128::from(lsi_hz);
    for pr in 0..=6u8 {
        let div = 4u128 << pr;
        let counts = product.div_ceil(div * NS).max(1);
        if counts <= 4096 {
            return (pr, (counts - 1) as u16);
        }
    }
    (6, 0x0FFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Der schnellste LSI des F401 laut Datenblatt.
    const LSI_MAX: u32 = 47_000;

    #[test]
    fn a_short_timeout_counts_finely_and_never_early() {
        // 20 ms bei 47 kHz und /4: 235 Schritte.
        assert_eq!(iwdg(20_000_000, LSI_MAX), (0, 234));
        assert_eq!(iwdg(1, LSI_MAX), (0, 0));
    }

    #[test]
    fn a_long_timeout_takes_the_finest_divider_that_fits() {
        // 8 s bei 47 kHz: /128 ergibt 2938 Schritte, /64 waeren 5875.
        assert_eq!(iwdg(8_000_000_000, LSI_MAX), (5, 2937));
        assert_eq!(iwdg(60_000_000_000, LSI_MAX), (6, 0x0FFF), "ueber 22 s: der groesste Wert");
    }
}
