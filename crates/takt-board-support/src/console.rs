//! Das Wort, mit dem der Host ein Board zurueckverlangt (FB-264, FB-275).
//!
//! Beide Boards lesen die Gegenrichtung ihrer Trace-Leitung und erkennen
//! darin `TAKT`. Was danach geschieht, entscheidet das Board: Der ESP32-C6
//! setzt sich auf RTC-Ebene zurueck, damit sein USB-Geraet neu anlegt; die
//! Black Pill springt in den DFU-Bootloader des ROM, weil sie ohne Probe
//! nur so vom Host aus geflasht werden kann. Der Erkenner ist derselbe, und
//! er steht hier, wo er ohne Board getestet wird.

/// `TAKT` als Wort, Big-Endian: `0x54 0x41 0x4B 0x54`.
pub const MAGIC: u32 = 0x5441_4B54;

/// Erkennt `TAKT` in einem Bytestrom, ueber Aufrufe hinweg.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Magic {
    matched: usize,
}

impl Magic {
    /// Ein Erkenner am Anfang der Suche; `const` fuer statische Erkenner.
    pub const fn new() -> Magic {
        Magic { matched: 0 }
    }

    /// Wahr mit dem letzten Byte von `TAKT`; danach beginnt die Suche neu.
    pub fn feed(&mut self, b: u8) -> bool {
        let magic = MAGIC.to_be_bytes();
        self.matched = if b == magic[self.matched] { self.matched + 1 } else { usize::from(b == magic[0]) };
        if self.matched == magic.len() {
            self.matched = 0;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(bytes: &[u8]) -> usize {
        let mut m = Magic::default();
        bytes.iter().filter(|b| m.feed(**b)).count()
    }

    #[test]
    fn the_word_is_recognised_once_at_its_last_byte() {
        let mut m = Magic::default();
        assert!(!m.feed(b'T') && !m.feed(b'A') && !m.feed(b'K'));
        assert!(m.feed(b'T'));
        assert_eq!(m, Magic::default(), "die Suche beginnt danach neu");
    }

    #[test]
    fn noise_around_and_inside_the_word_does_not_count() {
        assert_eq!(fed(b"xxTAKTxx"), 1);
        assert_eq!(fed(b"TAKTAKT"), 1, "das zweite T eroeffnet kein neues Wort mitten im ersten");
        assert_eq!(fed(b"TA KT"), 0);
    }

    #[test]
    fn a_false_start_can_still_lead_into_the_word() {
        assert_eq!(fed(b"TTAKT"), 1);
        assert_eq!(fed(b"TATAKT"), 1);
    }

    #[test]
    fn the_word_repeats_across_calls() {
        assert_eq!(fed(b"TAKTTAKT"), 2);
    }
}
