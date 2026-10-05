//! Sprachedition (Referenz 2.5): je Edition der Wortschatz — Schluesselwoerter,
//! reservierte Woerter, kontextuelle Terminale, reservierte Membernamen und
//! offene Enums. Neue Woerter kommen nur mit einer Edition; der Tokenizer liest
//! mit dem Wortschatz der Edition, die `system: language = N` nennt.

use takt_diag::Span;

use crate::keywords;

/// Eine Edition der Sprache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Edition {
    /// Edition 1: die Referenz v0.2.8.
    E1,
}

impl Edition {
    /// Die neueste dem Compiler bekannte Edition.
    pub const LATEST: Edition = Edition::E1;

    /// Alle bekannten Editionen, aufsteigend.
    pub const ALL: &'static [Edition] = &[Edition::E1];

    /// Edition zur Nummer aus `system: language = N`.
    pub fn from_number(n: u32) -> Option<Edition> {
        match n {
            1 => Some(Edition::E1),
            _ => None,
        }
    }

    /// Nummer der Edition.
    pub fn number(self) -> u32 {
        match self {
            Edition::E1 => 1,
        }
    }

    /// Schluesselwoerter (2.2), sortiert.
    pub fn keywords(self) -> &'static [&'static str] {
        keywords::KEYWORDS
    }

    /// Reservierte Woerter ohne Bedeutung (2.5), sortiert.
    pub fn reserved_words(self) -> &'static [&'static str] {
        keywords::RESERVED
    }

    /// Kontextuelle Terminale der Grammatik, sortiert.
    pub fn contextual(self) -> &'static [&'static str] {
        keywords::CONTEXTUAL
    }

    /// Reservierte Membernamen der eingebauten Typen (2.5), in Reihenfolge der Referenz.
    pub fn reserved_members(self) -> &'static [&'static str] {
        keywords::RESERVED_MEMBERS
    }

    /// Zugriffe der Wrapper: als Feldnamen von Records und Varianten verboten (2.5).
    pub fn wrapper_accessors(self) -> &'static [&'static str] {
        keywords::WRAPPER_ACCESSORS
    }

    /// Namen, die eine Muster-Bindung selbst traegt: als Capture-Namen verboten (2.5).
    pub fn capture_names(self) -> &'static [&'static str] {
        keywords::CAPTURE_NAMES
    }

    /// Vordefinierte offene Enums (2.5) mit den in der Referenz genannten Varianten.
    pub fn open_enums(self) -> &'static [(&'static str, &'static [&'static str])] {
        keywords::OPEN_ENUMS
    }

    /// Ist `word` ein Schluesselwort dieser Edition?
    pub fn is_keyword(self, word: &str) -> bool {
        self.keywords().binary_search(&word).is_ok()
    }

    /// Ist `word` reserviert?
    pub fn is_reserved(self, word: &str) -> bool {
        self.reserved_words().binary_search(&word).is_ok()
    }

    /// Ist `word` ein kontextuelles Terminal?
    pub fn is_contextual(self, word: &str) -> bool {
        self.contextual().binary_search(&word).is_ok()
    }

    /// Ist `name` ein reservierter Membername (einschliesslich `wrap_*`)?
    pub fn is_reserved_member(self, name: &str) -> bool {
        self.reserved_members().iter().any(|m| m == &name || (m.ends_with('*') && name.starts_with(&m[..m.len() - 1])))
    }
}

/// Vorlauf: liest `language = N` aus dem `system:`-Block, bevor der Tokenizer den
/// Wortschatz waehlt. Liefert den Zahlentext mit Position, oder `None`, wenn der
/// Eintrag fehlt. Syntaxfehler meldet spaeter der Parser.
pub fn declared_edition(src: &str) -> Option<(String, Span)> {
    // BOM und Tabulator meldet der Tokenizer; hier verschieben sie nur die Stelle.
    let body = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut offset = src.len() - body.len();
    let mut in_system = false;
    for line in body.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let trimmed = content.trim_start_matches([' ', '\t']);
        let indent = content.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            offset += line.len();
            continue;
        }
        if indent == 0 {
            in_system = trimmed.starts_with("system") && trimmed[6..].trim_start().starts_with(':');
        } else if in_system {
            let entry = trimmed.split('#').next().unwrap_or_default();
            if let Some((key, value)) = entry.split_once('=')
                && key.trim() == "language"
            {
                let start = offset + indent + key.len() + 1 + (value.len() - value.trim_start().len());
                let value = value.trim();
                return Some((value.to_string(), Span::new(start as u32, (start + value.len()) as u32)));
            }
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_edition_is_read_before_tokenizing() {
        assert_eq!(declared_edition("system:\n    tick = 1 ms\n    language = 1\n").map(|(n, _)| n), Some("1".into()));
        assert_eq!(
            declared_edition("# Kopf\n\nsystem:\n    language = 7  # x\nmachine m:\n").map(|(n, _)| n),
            Some("7".into())
        );
        assert_eq!(declared_edition("machine m:\n    initial A\n"), None);
        assert_eq!(declared_edition("system:\n    tick = 1 ms\nconst LANG = 3\n"), None);
        let (_, span) = declared_edition("system:\n    language = 1\n").expect("gefunden");
        assert_eq!((span.start, span.end), (23, 24));
    }

    /// Die Spanne zeigt auf den Wert, auch wenn er nicht numerisch ist oder
    /// BOM, Tabulator oder CRLF davor stehen; es gilt der erste Eintrag.
    #[test]
    fn the_span_points_at_the_value() {
        let at = |src: &str| declared_edition(src).map(|(v, s)| (v, s.start, s.end));
        assert_eq!(at("system:\n    language = e\n"), Some(("e".into(), 23, 24)), "nicht im Wort language");
        assert_eq!(at("system:\n    language=7\n"), Some(("7".into(), 21, 22)));
        assert_eq!(at("\u{feff}system:\n    language = 1\n"), Some(("1".into(), 26, 27)), "hinter der BOM");
        assert_eq!(at("system:\n\tlanguage = 1\n"), Some(("1".into(), 20, 21)), "Tabulator als Einrueckung");
        assert_eq!(at("system:\r\n    language = 1\r\n"), Some(("1".into(), 24, 25)));
        assert_eq!(at("system:\n    language = 1\n    language = 2\n"), Some(("1".into(), 23, 24)));
        assert_eq!(at("system:\n    language = 99999999999\n"), Some(("99999999999".into(), 23, 34)));
    }

    #[test]
    fn reserved_members_include_wrap_prefix() {
        assert!(Edition::E1.is_reserved_member("wrap_u8"));
        assert!(Edition::E1.is_reserved_member("valid"));
        assert!(!Edition::E1.is_reserved_member("payload"));
    }
}
