//! Die Rechnung der MPU nach ARMv7-M (12.3): Regionen, Subregionen,
//! Zugriffsrechte, und wie weit ein Befehl reicht, den der MemManage-Handler
//! ueberspringt. Das Schreiben der Register steht im Board-Crate.
//!
//! **Warum Subregionen.** Eine Region ist eine Zweierpotenz gross und an
//! ihrer Groesse ausgerichtet. Der Programmzustand ist es nicht; die Region
//! darum die naechste Zweierpotenz, und die Achtel dahinter, die er nicht
//! beruehrt, schaltet das Subregion-Feld ab — sie bleiben frei fuer das
//! uebrige RAM, und der Verschnitt ist kleiner als ein Achtel.

/// Zugriffsrechte einer Region (Feld `AP` in `RASR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Kein Zugriff: der Waechter unter dem Stack.
    None,
    /// Nur lesen (privilegiert): der Programmzustand ausserhalb des Ticks.
    ReadOnly,
    /// Lesen und schreiben (privilegiert): der Programmzustand im Tick.
    ReadWrite,
}

impl Access {
    /// Das Feld `AP` (Bits 24 bis 26 von `RASR`), ungeschoben.
    pub fn ap(self) -> u32 {
        match self {
            Access::None => 0b000,
            Access::ReadOnly => 0b101,
            Access::ReadWrite => 0b001,
        }
    }
}

/// Eine Region: Basis, Groesse als Zweierpotenz, und wie viele Achtel von
/// vorn sie schuetzt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    /// Basisadresse, an `size` ausgerichtet.
    pub base: u32,
    /// Groesse in Byte, eine Zweierpotenz ab 32.
    pub size: u32,
    /// So viele Achtel von vorn sind aktiv (1 bis 8); ab 256 Byte, darunter
    /// gibt es keine Subregionen, und die Region ist ganz aktiv.
    pub eighths: u32,
}

impl Region {
    /// Die kleinste Region an `base`, die `bytes` ueberdeckt; `None`, wenn
    /// `base` nicht an ihrer Groesse ausgerichtet ist.
    pub fn covering(base: u32, bytes: u32) -> Option<Region> {
        let size = bytes.max(32).checked_next_power_of_two()?;
        if base % size != 0 {
            return None;
        }
        let eighths = if size >= 256 { bytes.max(1).div_ceil(size / 8) } else { 8 };
        Some(Region { base, size, eighths })
    }

    /// Wie viel RAM die Region schuetzt; dahinter ist es frei.
    pub fn protected(&self) -> u32 {
        self.size / 8 * self.eighths
    }

    /// `RBAR` mit gueltiger Regionsnummer.
    pub fn rbar(&self, number: u32) -> u32 {
        self.base | 1 << 4 | (number & 0xF)
    }

    /// `RASR`: normales RAM (TEX 000, S, C), nie ausfuehrbar, eingeschaltet.
    pub fn rasr(&self, access: Access) -> u32 {
        let size_field = self.size.trailing_zeros() - 1;
        let disabled = if self.eighths >= 8 { 0 } else { (0xFFu32 << self.eighths) & 0xFF };
        1 << 28 | access.ap() << 24 | 1 << 18 | 1 << 17 | disabled << 8 | size_field << 1 | 1
    }
}

/// Wie viele Byte der Thumb-Befehl an `halfword` belegt: 4, wenn seine
/// oberen fuenf Bits 0b11101, 0b11110 oder 0b11111 sind, sonst 2.
pub fn instruction_bytes(halfword: u16) -> u32 {
    if halfword >> 11 >= 0b11101 { 4 } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_covers_its_bytes_with_whole_eighths() {
        // 3000 Byte: 4 KiB Region, Achtel von 512 Byte, sechs davon aktiv.
        let r = Region::covering(0x2000_0000, 3000).expect("ausgerichtet");
        assert_eq!((r.size, r.eighths, r.protected()), (4096, 6, 3072));
        // SIZE = log2(4096) - 1 = 11, SRD schaltet die Achtel 6 und 7 ab.
        assert_eq!(
            r.rasr(Access::ReadOnly),
            1 << 28 | 0b101 << 24 | 1 << 18 | 1 << 17 | 0b1100_0000 << 8 | 11 << 1 | 1
        );
        assert_eq!(r.rbar(0), 0x2000_0010);
    }

    #[test]
    fn a_small_region_has_no_subregions() {
        let r = Region::covering(0x2000_0000, 20).expect("ausgerichtet");
        assert_eq!((r.size, r.eighths), (32, 8));
        assert_eq!(r.rasr(Access::None) & 0xFF00, 0, "keine Subregion abgeschaltet");
    }

    #[test]
    fn an_unaligned_base_is_refused() {
        assert_eq!(Region::covering(0x2000_0100, 3000), None);
    }

    #[test]
    fn thumb_instructions_are_two_or_four_bytes() {
        assert_eq!(instruction_bytes(0x6008), 2, "str r0, [r1]");
        assert_eq!(instruction_bytes(0xF8C1), 4, "str.w r0, [r1, #…]");
        assert_eq!(instruction_bytes(0xE92D), 4, "push.w");
        assert_eq!(instruction_bytes(0xE7FE), 2, "b .");
    }
}
