//! Die PLL-Teiler des STM32F4 (12.3).
//!
//! **Die dritte stille Fehlerquelle.** Ein falscher Teiler bricht nichts:
//! Der Kern laeuft, nur mit einer anderen Frequenz als gedacht. Jede
//! Zeitmessung, jede Baudrate und die ganze Kalibrierung (13.8)
//! verschieben sich still mit. Darum steht die Rechnung hier und nicht
//! neben dem Registerzugriff.
//!
//! Die Kette ist `f_vco = f_in / M * N` und `f_out = f_vco / P`. Der
//! Referenzhandbuch-Zwang: `f_in / M` muss zwischen 1 und 2 MHz liegen
//! (empfohlen: genau 1), und `f_vco` zwischen 100 und 432 MHz.

/// Die Zielfrequenz des Kerns: das Maximum des F401.
pub const TARGET_HZ: u32 = 84_000_000;

/// Der VCO, den [`divider_n`] ansteuert.
///
/// 336 MHz durch den Teiler 4 ergeben genau 84 — und 336 liegt im
/// erlaubten Band (100 bis 432).
pub const VCO_HZ: u32 = 336_000_000;

/// Der Eingangsteiler `M`.
///
/// Er bringt den Quarz auf 1 MHz, wie das Referenzhandbuch es empfiehlt:
/// Ein Eingangstakt von genau 1 MHz macht `N` unmittelbar zur
/// VCO-Frequenz in Megahertz und haelt den Jitter klein.
///
/// `None`, wenn der Quarz nicht ganzzahlig auf 1 MHz teilt oder `M`
/// ausserhalb von 2..=63 laege. Lieber ein Fehler als eine Naeherung —
/// eine krumme Eingangsfrequenz verschoebe den ganzen Takt.
pub fn divider_m(hse_hz: u32) -> Option<u8> {
    if hse_hz == 0 || hse_hz % 1_000_000 != 0 {
        return None;
    }
    let m = hse_hz / 1_000_000;
    (2..=63).contains(&m).then_some(m as u8)
}

/// Der Multiplikator `N` fuer den VCO.
///
/// Bei 1 MHz Eingang ist er die VCO-Frequenz in Megahertz. Der erlaubte
/// Bereich ist 50..=432.
pub fn divider_n() -> u16 {
    (VCO_HZ / 1_000_000) as u16
}

/// Der Ausgangsteiler `P`, als Registerwert.
///
/// Das Register kennt nur vier Werte: 2, 4, 6, 8 — kodiert als 0 bis 3.
/// `None`, wenn der VCO nicht durch einen davon auf die Zielfrequenz
/// faellt.
pub fn divider_p() -> Option<u8> {
    let p = VCO_HZ / TARGET_HZ;
    match p {
        2 => Some(0b00),
        4 => Some(0b01),
        6 => Some(0b10),
        8 => Some(0b11),
        _ => None,
    }
}

/// Die Frequenz, die eine Teilerkette tatsaechlich ergibt.
///
/// Die Gegenrechnung: Sie belegt, dass die gewaehlten Teiler zum Ziel
/// fuehren, statt es zu behaupten.
pub fn result_hz(hse_hz: u32, m: u8, n: u16, p_code: u8) -> u32 {
    let p = u32::from(p_code) * 2 + 2;
    if m == 0 || p == 0 {
        return 0;
    }
    hse_hz / u32::from(m) * u32::from(n) / p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Der Quarz der Black Pill.
    #[test]
    fn a_25_megahertz_crystal_divides_to_one() {
        assert_eq!(divider_m(25_000_000), Some(25));
    }

    /// Die zweite gaengige Bestueckung.
    #[test]
    fn an_8_megahertz_crystal_divides_to_one() {
        assert_eq!(divider_m(8_000_000), Some(8));
    }

    /// **Die Kette trifft 84 MHz — bei beiden Quarzen.**
    ///
    /// Das ist der Test, der die ganze Rechnung traegt: Wenn er faellt,
    /// laeuft der Kern mit einer anderen Frequenz als `CORE_HZ`
    /// behauptet, und jede Zyklenmessung ist falsch.
    #[test]
    fn the_chain_hits_the_target_for_both_crystals() {
        for hse in [25_000_000, 8_000_000] {
            let m = divider_m(hse).expect("Teiler");
            let p = divider_p().expect("Ausgangsteiler");
            assert_eq!(result_hz(hse, m, divider_n(), p), TARGET_HZ, "Quarz {hse} Hz");
        }
    }

    /// Ein krummer Quarz wird abgelehnt statt gerundet.
    #[test]
    fn a_non_integral_crystal_is_refused() {
        assert_eq!(divider_m(25_600_000), None, "25,6 MHz teilt nicht auf 1 MHz");
        assert_eq!(divider_m(0), None);
    }

    /// Ein zu langsamer Quarz ebenfalls: `M` muss mindestens 2 sein.
    #[test]
    fn a_crystal_below_the_divider_range_is_refused() {
        assert_eq!(divider_m(1_000_000), None, "M=1 ist nicht erlaubt");
    }

    /// Der VCO bleibt im erlaubten Band (100 bis 432 MHz).
    #[test]
    fn the_vco_stays_within_its_band() {
        assert!((100_000_000..=432_000_000).contains(&VCO_HZ), "VCO {VCO_HZ} Hz");
    }

    /// `N` bleibt im erlaubten Bereich (50 bis 432).
    #[test]
    fn the_multiplier_stays_within_range() {
        assert!((50..=432).contains(&divider_n()), "N = {}", divider_n());
    }
}
