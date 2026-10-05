//! Der Schluessel eines Abbilds (KON1-009): Wer ein Bring-up baut, gibt
//! ihm in `TAKT_IMAGE_KEY` den Schluessel seines Baus mit; das Bauskript
//! legt ihn als Symbol `__takt_image_key_<schluessel>` ins ELF. Bauen zwei
//! Laeufe parallel in dasselbe Zielverzeichnis, prueft jeder an seiner
//! Kopie, dass sie seinen Schluessel traegt, bevor er sie veroeffentlicht.
//!
//! Ein Symbol statt Daten im Abbild: Es kostet keinen Flash, kein Linker
//! wirft es als unbenutzt weg, und `objcopy -O binary` entfernt es — die
//! Pruefung gilt dem ELF, nicht dem geflashten Rohabbild.

/// Der Anfang des Symbolnamens; der Schluessel folgt.
pub const SYMBOL: &str = "__takt_image_key_";

/// Taugt `key` als Schluessel: 1 bis 32 Hexziffern, damit der Symbolname
/// gueltig bleibt.
pub fn valid(key: &str) -> bool {
    !key.is_empty() && key.len() <= 32 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Traegt das ELF `image` den Schluessel `key`? Der Symbolname steht
/// nullterminiert in der Stringtabelle; das Nullbyte unterscheidet `ab`
/// von `abc`.
pub fn carries(image: &[u8], key: &str) -> bool {
    if !valid(key) {
        return false;
    }
    let (symbol, key) = (SYMBOL.as_bytes(), key.as_bytes());
    let n = symbol.len() + key.len() + 1;
    image.windows(n).any(|w| w[..symbol.len()] == *symbol && w[symbol.len()..n - 1] == *key && w[n - 1] == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_a_short_hex_word() {
        assert!(valid("0123456789abcdef") && valid("ABCDEF"));
        assert!(!valid("") && !valid("12 34") && !valid("xyz") && !valid(&"a".repeat(33)));
    }

    /// Nur der ganze Name mit seinem Nullbyte zaehlt.
    #[test]
    fn only_the_whole_symbol_carries_the_key() {
        let image = b"\x7fELF....\0__takt_tick_at\0__takt_image_key_00ff12\0rest";
        assert!(carries(image, "00ff12"));
        assert!(!carries(image, "00ff1"), "ein Praefix des Schluessels");
        assert!(!carries(image, "00ff123"));
        assert!(!carries(b"__takt_image_key_00ff12", "00ff12"), "ohne Nullbyte");
        assert!(!carries(image, "zz"));
    }
}
