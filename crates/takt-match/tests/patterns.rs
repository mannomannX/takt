//! Mustervergleich und Extraktion (8.7).
//!
//! Die Regeln stehen in 8.7 als Tabelle; die Tests gehen sie durch. Was
//! hier gruen ist, muss im Interpreter dasselbe liefern — der
//! differentielle Test in `takt-interp` haelt das fest.

use takt_match::{Kind, Piece, has, matches, parse_hex, parse_int};

fn t(s: &str) -> Piece<'_> {
    Piece::Text(s.as_bytes())
}

/// Der Wert eines Platzhalters als Text.
fn span_text<'a>(m: &takt_match::Match, n: usize, text: &'a str) -> &'a str {
    let s = m.spans[n].of(text.as_bytes());
    core::str::from_utf8(s).unwrap_or("")
}

#[test]
fn a_literal_matches_only_itself() {
    let p = [t("READY")];
    assert!(matches(&p, b"READY").is_some());
    assert!(matches(&p, b"READ").is_none());
    assert!(matches(&p, b"READYX").is_none(), "`matches` verlangt den ganzen Text (8.7)");
}

/// 8.7: `int` ist `[+-]?[0-9]{1,19}` — das Vorzeichen zaehlt nicht zu
/// den 19 Ziffern.
#[test]
fn an_int_capture_takes_its_class() {
    let p = [t("sector "), Piece::Capture(Kind::Int)];
    let m = matches(&p, b"sector 42").expect("Treffer");
    assert_eq!(m.count, 1);
    assert_eq!(span_text(&m, 0, "sector 42"), "42");
    assert_eq!(parse_int(b"42"), Some(42));
    assert_eq!(parse_int(b"-7"), Some(-7));
    assert!(matches(&p, b"sector ").is_none(), "ein Platzhalter verlangt mindestens ein Zeichen");
    assert!(matches(&p, b"sector x").is_none());
}

/// 8.7: „Ueberlauf → kein Match". Der Fault gehoert nicht hierher — ein
/// Muster, das nicht trifft, ist kein Fehler.
#[test]
fn an_int_that_overflows_is_not_a_value() {
    assert_eq!(parse_int(b"9223372036854775807"), Some(i64::MAX));
    assert_eq!(parse_int(b"9223372036854775808"), None);
    // Der Zweierkomplementbereich ist asymmetrisch: Die Untergrenze ist
    // gueltig, ihr Betrag nicht (derselbe Fall wie FB-95).
    assert_eq!(parse_int(b"-9223372036854775808"), Some(i64::MIN));
    assert_eq!(parse_int(b"-9223372036854775809"), None);
    assert_eq!(parse_int(b""), None);
}

/// 8.7: `hex` ist `(0x)?[0-9a-fA-F]{1,16}`.
#[test]
fn a_hex_capture_accepts_both_forms() {
    let p = [t("crc "), Piece::Capture(Kind::Hex)];
    let m = matches(&p, b"crc 0xBEEF").expect("Treffer");
    assert_eq!(span_text(&m, 0, "crc 0xBEEF"), "0xBEEF");
    assert_eq!(parse_hex(b"0xBEEF"), Some(0xBEEF));
    assert_eq!(parse_hex(b"beef"), Some(0xBEEF));
}

/// 8.7: Zwei Platzhalter mit Literal dazwischen; das Literal beendet den
/// ersten eindeutig, weil sein erstes Zeichen nicht zur Klasse gehoert.
#[test]
fn two_captures_split_at_the_literal() {
    let p = [t("Boot v"), Piece::Capture(Kind::Int), t("."), Piece::Capture(Kind::Int)];
    let m = matches(&p, b"Boot v2.14").expect("Treffer");
    assert_eq!(m.count, 2);
    assert_eq!(span_text(&m, 0, "Boot v2.14"), "2");
    assert_eq!(span_text(&m, 1, "Boot v2.14"), "14");
}

/// 8.7: `word` ist `[A-Za-z0-9_]{1,64}`.
#[test]
fn a_word_capture_stops_at_its_class() {
    let p = [t("Recovery: "), Piece::Capture(Kind::Word)];
    let m = matches(&p, b"Recovery: RECOVERED").expect("Treffer");
    assert_eq!(span_text(&m, 0, "Recovery: RECOVERED"), "RECOVERED");
    assert!(matches(&p, b"Recovery: ").is_none());
}

/// 8.7: `str` und `_` enden beim ersten Vorkommen des folgenden
/// Literals (leftmost-shortest).
#[test]
fn an_open_end_stops_at_the_next_literal() {
    let p = [t("["), Piece::Capture(Kind::Str(32)), t("]")];
    let m = matches(&p, b"[abc]").expect("Treffer");
    assert_eq!(span_text(&m, 0, "[abc]"), "abc");
    // Leftmost-shortest: das *erste* `]` beendet, nicht das letzte.
    let m = matches(&p, b"[a]b]").map(|m| span_text(&m, 0, "[a]b]"));
    assert_eq!(m, None, "`]b]` bleibt uebrig, und `matches` verlangt den ganzen Text");
}

/// `{_}` bindet nicht (8.7) und belegt darum keinen Platz.
#[test]
fn an_anonymous_placeholder_binds_nothing() {
    let p = [Piece::Capture(Kind::Any), t("CRC mismatch"), Piece::Capture(Kind::Any)];
    let m = matches(&p, b"xxCRC mismatchyy").expect("Treffer");
    assert_eq!(m.count, 0, "`{{_}}` bindet nicht");
}

/// `has` sucht ein Vorkommen, `matches` den ganzen Text (8.7).
#[test]
fn has_finds_an_occurrence_where_matches_does_not() {
    let p = [t("PANIC")];
    assert!(matches(&p, b"kernel PANIC now").is_none());
    assert!(has(&p, b"kernel PANIC now").is_some());
    assert!(has(&p, b"nichts davon").is_none());
}

/// `has` nimmt das linkeste Vorkommen.
#[test]
fn has_takes_the_leftmost_occurrence() {
    let p = [t("v"), Piece::Capture(Kind::Int)];
    let text = "v1 und v2";
    let m = has(&p, text.as_bytes()).expect("Treffer");
    assert_eq!(span_text(&m, 0, text), "1");
}
