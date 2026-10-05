//! Mustervergleich und Extraktion (8.7).
//!
//! Die Regeln stehen in 8.7 als Tabelle; die Tests gehen sie durch. Was
//! hier gruen ist, muss im Interpreter dasselbe liefern — der
//! differentielle Test in `takt-interp` haelt das fest.

use takt_match::{Kind, MAX_CAPTURES, Piece, Span, has, matches, parse_hex, parse_int};

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

/// `has` setzt nur an einem Zeichenanfang oder am Zeilenende an (8.7,
/// FB-358), wie der Interpreter (`has_start`) und der Codegen
/// (`takt_mir::scan`): Ein offenes Ende vorn bindet nie die Mitte eines
/// Mehrbytezeichens.
#[test]
fn has_starts_only_at_a_character_start() {
    let p = [Piece::Capture(Kind::Str(3)), t("x")];
    let text = "äbcx";
    let m = has(&p, text.as_bytes()).expect("Treffer ab dem b");
    assert_eq!(m.spans[0], Span { start: 2, end: 4 });
    assert_eq!(span_text(&m, 0, text), "bc");
    let semicolon = [Piece::Capture(Kind::Str(3)), t(";")];
    for (line, start) in [("abc;", 0), ("äöü;", 4), ("äbc;", 2)] {
        let m = has(&semicolon, line.as_bytes()).expect("Treffer");
        assert_eq!(m.spans[0].start, start, "{line}");
    }
    let m = matches(&p, "äbx".as_bytes()).expect("ä und b sind drei Bytes");
    assert_eq!(span_text(&m, 0, "äbx"), "äb");
}

fn sixteen_ints() -> Vec<Piece<'static>> {
    let mut p = Vec::new();
    for _ in 0..MAX_CAPTURES {
        p.extend([Piece::Capture(Kind::Int), t(",")]);
    }
    p
}

/// `{_}` belegt keinen Platz (8.7): Hinter sechzehn bindenden
/// Platzhaltern trifft das Muster weiter.
#[test]
fn an_anonymous_placeholder_after_the_last_free_slot_still_matches() {
    let line = "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,";
    let m = matches(&sixteen_ints(), line.as_bytes()).expect("sechzehn bindende Platzhalter");
    assert_eq!(m.count as usize, MAX_CAPTURES);
    assert_eq!(span_text(&m, 15, line), "15");
    let mut p = sixteen_ints();
    p.extend([Piece::Capture(Kind::Any)]);
    let line = "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,Rest";
    let m = matches(&p, line.as_bytes()).expect("`{_}` bindet nicht");
    assert_eq!(m.count as usize, MAX_CAPTURES);
}

/// Die Raender der Zeichenklassen (8.7), an denen der erzeugte Matcher
/// gegen dieses Orakel gemessen wird (`takt-llvm/tests/captures.rs`).
#[test]
fn the_classes_end_exactly_at_their_bounds() {
    let int = [t("="), Piece::Capture(Kind::Int)];
    assert!(matches(&int, b"=0000000000000000001").is_some(), "19 Ziffern");
    assert!(matches(&int, b"=00000000000000000001").is_none(), "20 Ziffern sind kein int");
    assert!(matches(&int, b"=-0000000000000000001").is_some(), "das Vorzeichen zaehlt nicht");
    let hex = [t("="), Piece::Capture(Kind::Hex)];
    assert!(matches(&hex, b"=0x0123456789abcdef").is_some(), "16 Ziffern hinter 0x");
    assert!(matches(&hex, b"=0123456789abcdef0").is_none(), "17 Ziffern");
    assert!(matches(&hex, b"=0x").is_none(), "0x ohne Ziffer");
    let word = [t("="), Piece::Capture(Kind::Word), t(";")];
    let w64 = format!("={};", "w".repeat(64));
    let w65 = format!("={};", "w".repeat(65));
    assert_eq!(matches(&word, w64.as_bytes()).map(|m| m.spans[0]), Some(Span { start: 1, end: 65 }));
    assert!(matches(&word, w65.as_bytes()).is_none(), "65 Zeichen sind kein word");
    let str8 = [t("<"), Piece::Capture(Kind::Str(8)), t(">")];
    assert!(matches(&str8, b"<12345678>").is_some());
    assert!(matches(&str8, b"<123456789>").is_none());
    let m = matches(&str8, "<äöü>".as_bytes()).expect("sechs Bytes");
    assert_eq!(span_text(&m, 0, "<äöü>"), "äöü");
    assert!(matches(&str8, "<äöüäö>".as_bytes()).is_none(), "zehn Bytes");
}

/// 8.7: `"{a}:{b:int}"` trifft `x:5`, aber nicht `x:y:5`, weil `{a}` am
/// ersten `:` endet (leftmost-shortest, kein Ruecksetzen).
#[test]
fn an_open_end_does_not_backtrack() {
    let p = [Piece::Capture(Kind::Str(u32::MAX)), t(":"), Piece::Capture(Kind::Int)];
    let m = matches(&p, b"x:5").expect("Treffer");
    assert_eq!((span_text(&m, 0, "x:5"), span_text(&m, 1, "x:5")), ("x", "5"));
    assert!(matches(&p, b"x:y:5").is_none());
    let any = [Piece::Capture(Kind::Any), t(":"), Piece::Capture(Kind::Int)];
    assert!(matches(&any, b"x:y:5").is_none());
    let m = has(&any, b"x:y:5").expect("ab dem y");
    assert_eq!((m.count, span_text(&m, 0, "x:y:5")), (1, "5"));
}

/// 8.7: Eine Klasse endet beim ersten Zeichen ausserhalb oder am Zeilenende,
/// auch am Musterende unter `has`; ist ihr Lauf laenger als ihre Hoechstlaenge,
/// trifft das Muster an dieser Stelle nicht - es bindet nie nur den Anfang
/// einer Zahl oder eines Worts. `has` sucht dann an der naechsten Stelle.
#[test]
fn a_run_longer_than_its_class_binds_no_prefix() {
    let digits20 = "12345678901234567890";
    let int = [Piece::Capture(Kind::Int)];
    assert!(matches(&int, &digits20.as_bytes()[1..]).is_some(), "19 Ziffern");
    assert!(matches(&int, digits20.as_bytes()).is_none(), "20 Ziffern");
    let m = has(&int, digits20.as_bytes()).expect("ab der zweiten Ziffer");
    assert_eq!(m.spans[0], Span { start: 1, end: 20 }, "nicht die ersten 19");
    let int_semicolon = [Piece::Capture(Kind::Int), t(";")];
    let text = format!("{digits20};");
    assert_eq!(has(&int_semicolon, text.as_bytes()).map(|m| m.spans[0]), Some(Span { start: 1, end: 20 }));

    let hex = [Piece::Capture(Kind::Hex)];
    let hex17 = "0123456789abcdef0";
    assert!(matches(&hex, &hex17.as_bytes()[1..]).is_some(), "16 Ziffern");
    assert!(matches(&hex, hex17.as_bytes()).is_none(), "17 Ziffern");
    assert_eq!(has(&hex, hex17.as_bytes()).map(|m| m.spans[0]), Some(Span { start: 1, end: 17 }));

    let word = [Piece::Capture(Kind::Word)];
    let w65 = "w".repeat(65);
    assert!(matches(&word, &w65.as_bytes()[1..]).is_some(), "64 Zeichen");
    assert!(matches(&word, w65.as_bytes()).is_none(), "65 Zeichen");
    assert_eq!(has(&word, w65.as_bytes()).map(|m| m.spans[0]), Some(Span { start: 1, end: 65 }));
}

/// 8.7: `str` und `_` reichen am Musterende bis zum Zeilenende; ein `str<N>`,
/// das dort mehr als N Bytes fassen muesste, trifft nicht.
#[test]
fn an_open_end_at_the_end_of_the_pattern_takes_the_rest_of_the_line() {
    let p = [t("<"), Piece::Capture(Kind::Str(8))];
    assert_eq!(matches(&p, b"<12345678").map(|m| m.spans[0]), Some(Span { start: 1, end: 9 }));
    assert!(matches(&p, b"<123456789").is_none(), "neun Bytes");
    assert!(has(&p, b"x<123456789").is_none(), "auch unter has kein Anfangsstueck");
    assert_eq!(has(&p, b"x<1234").map(|m| m.spans[0]), Some(Span { start: 2, end: 6 }));
    let any = [t("<"), Piece::Capture(Kind::Any)];
    assert_eq!(has(&any, b"ab<cd").map(|m| m.count), Some(0));
}

/// 8.7: `float` ist `[+-]?[0-9]+(.[0-9]+)?([eE][+-]?[0-9]+)?`; ein Punkt oder
/// ein `e` ohne Ziffern dahinter gehoert nicht mehr zur Zahl.
#[test]
fn a_float_takes_exactly_its_class() {
    let p = [t("="), Piece::Capture(Kind::Float)];
    for (text, want) in
        [("=1.5e-3", Some("1.5e-3")), ("=-2", Some("-2")), ("=+7E+2", Some("+7E+2")), ("=3.25", Some("3.25"))]
    {
        let m = matches(&p, text.as_bytes());
        assert_eq!(m.map(|m| span_text(&m, 0, text).to_string()).as_deref(), want, "{text}");
    }
    for text in ["=1.", "=1e", "=1e+", "=.5", "=+", "=1.5.2"] {
        assert!(matches(&p, text.as_bytes()).is_none(), "{text}");
    }
    let dot = [Piece::Capture(Kind::Float), t(".")];
    let m = matches(&dot, b"12.").expect("die Zahl endet vor dem Punkt");
    assert_eq!(span_text(&m, 0, "12."), "12");
}

/// 8.7: `hex` ohne Ueberlauf; der groesste Wert ist `7FFFFFFFFFFFFFFF`.
#[test]
fn a_hex_value_beyond_i64_is_no_value() {
    assert_eq!(parse_hex(b"7FFFFFFFFFFFFFFF"), Some(i64::MAX));
    assert_eq!(parse_hex(b"0x7fffffffffffffff"), Some(i64::MAX));
    assert_eq!(parse_hex(b"8000000000000000"), None);
    assert_eq!(parse_hex(b"0x8000000000000000"), None);
    assert_eq!(parse_hex(b"0x"), None);
}
