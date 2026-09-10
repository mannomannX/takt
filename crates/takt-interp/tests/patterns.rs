//! Musterabgleich (Referenz 8.7): Zeichenklassen je Art, Capture-Werte,
//! leftmost-shortest bei offenem Ende, `has` als Kurzform, und die Faelle,
//! die „kein Match" statt eines Faults ergeben.

use takt_interp::Value;
use takt_interp::pattern::{match_has, match_text};
use takt_mir::pattern::{CaptureKind, PatternPiece};

/// Baut ein Muster aus einer knappen Schreibweise: `#name:kind` ist ein
/// Platzhalter, `#_` das offene Ende, alles andere Literal.
fn pattern(spec: &[&str]) -> Vec<PatternPiece> {
    spec.iter()
        .map(|p| match p.strip_prefix('#') {
            Some("_") => PatternPiece::Any,
            Some(rest) => {
                let (name, kind) = rest.split_once(':').expect("name:kind");
                let kind = match kind {
                    "int" => CaptureKind::Int,
                    "hex" => CaptureKind::Hex,
                    "float" => CaptureKind::Float,
                    "word" => CaptureKind::Word,
                    other => CaptureKind::Str(other.parse().expect("str<N>")),
                };
                PatternPiece::Capture { name: name.to_string(), kind }
            }
            None => PatternPiece::Text((*p).to_string()),
        })
        .collect()
}

#[test]
fn a_literal_pattern_matches_the_whole_text() {
    let p = pattern(&["READY"]);
    assert_eq!(match_text(&p, "READY"), Some(vec![]));
    // `matches` verlangt den ganzen Text (8.7).
    assert_eq!(match_text(&p, "READY NOW"), None);
    assert_eq!(match_text(&p, "NOT READY"), None);
}

#[test]
fn int_captures_digits_with_an_optional_sign() {
    let p = pattern(&["Erasing sector ", "#n:int"]);
    assert_eq!(match_text(&p, "Erasing sector 7"), Some(vec![Value::Int(7)]));
    assert_eq!(match_text(&p, "Erasing sector -3"), Some(vec![Value::Int(-3)]));
    assert_eq!(match_text(&p, "Erasing sector +12"), Some(vec![Value::Int(12)]));
    // Ohne Ziffer gibt es kein Match: jede Klasse verlangt mindestens eine.
    assert_eq!(match_text(&p, "Erasing sector "), None);
    assert_eq!(match_text(&p, "Erasing sector x"), None);
}

#[test]
fn an_int_that_overflows_is_no_match_not_a_fault() {
    // 8.7: „Ueberlauf -> kein Match". Zwanzig Ziffern sprengen die Klasse
    // `[0-9]{1,19}`, und der Wert selbst passt nicht in `int`.
    let p = pattern(&["n=", "#n:int"]);
    assert_eq!(match_text(&p, "n=9223372036854775807"), Some(vec![Value::Int(i64::MAX)]));
    assert_eq!(match_text(&p, "n=9223372036854775808"), None);
}

#[test]
fn hex_accepts_an_optional_prefix() {
    let p = pattern(&["crc ", "#c:hex"]);
    assert_eq!(match_text(&p, "crc 1A2B"), Some(vec![Value::Int(0x1A2B)]));
    assert_eq!(match_text(&p, "crc 0xff"), Some(vec![Value::Int(0xFF)]));
    assert_eq!(match_text(&p, "crc zz"), None);
}

#[test]
fn float_reads_fraction_and_exponent() {
    let p = pattern(&["dt ", "#d:float", " ms"]);
    assert_eq!(match_text(&p, "dt 1.5 ms"), Some(vec![Value::F64(1.5)]));
    assert_eq!(match_text(&p, "dt -0.25 ms"), Some(vec![Value::F64(-0.25)]));
    assert_eq!(match_text(&p, "dt 2e3 ms"), Some(vec![Value::F64(2000.0)]));
    assert_eq!(match_text(&p, "dt 7 ms"), Some(vec![Value::F64(7.0)]));
}

#[test]
fn a_float_that_is_not_finite_is_no_match() {
    // 8.7: „nicht endlich -> kein Match".
    let p = pattern(&["x ", "#x:float"]);
    assert_eq!(match_text(&p, "x 1e400"), None);
}

#[test]
fn word_stops_at_the_first_character_outside_its_class() {
    let p = pattern(&["Recovery: ", "#outcome:word"]);
    assert_eq!(match_text(&p, "Recovery: OK"), Some(vec![Value::Str("OK".into())]));
    assert_eq!(match_text(&p, "Recovery: retry_2"), Some(vec![Value::Str("retry_2".into())]));
    // Das Leerzeichen gehoert nicht zur Klasse, also endet das Wort dort.
    let p = pattern(&["Recovery: ", "#outcome:word", " done"]);
    assert_eq!(match_text(&p, "Recovery: OK done"), Some(vec![Value::Str("OK".into())]));
}

#[test]
fn str_ends_at_the_first_occurrence_of_the_next_literal() {
    // 8.7: leftmost-shortest. Das Muster endet beim *ersten* `]`, nicht beim
    // letzten.
    let p = pattern(&["[", "#tag:32", "]"]);
    assert_eq!(match_text(&p, "[a]"), Some(vec![Value::Str("a".into())]));
    let p = pattern(&["[", "#tag:32", "] rest"]);
    assert_eq!(match_text(&p, "[a] rest"), Some(vec![Value::Str("a".into())]));
}

#[test]
fn str_respects_its_length_bound() {
    let p = pattern(&["v=", "#s:4"]);
    assert_eq!(match_text(&p, "v=abcd"), Some(vec![Value::Str("abcd".into())]));
    assert_eq!(match_text(&p, "v=abcde"), None);
}

#[test]
fn any_discards_its_text() {
    let p = pattern(&["#_", "CRC mismatch", "#_"]);
    assert_eq!(match_text(&p, "line 7: CRC mismatch at 0x40"), Some(vec![]));
    assert_eq!(match_text(&p, "all good"), None);
}

#[test]
fn several_captures_bind_in_pattern_order() {
    let p = pattern(&["Boot v", "#major:int", ".", "#minor:int", " (", "#build:word", ")"]);
    assert_eq!(match_text(&p, "Boot v2.1 (rc3)"), Some(vec![Value::Int(2), Value::Int(1), Value::Str("rc3".into())]));
}

#[test]
fn has_finds_the_pattern_anywhere() {
    // 8.7: `has P` ist `matches "{_}" + P + "{_}"`.
    let p = pattern(&["CRC mismatch"]);
    assert!(match_has(&p, "line 7: CRC mismatch at 0x40").is_some());
    assert!(match_has(&p, "CRC mismatch").is_some());
    assert!(match_has(&p, "all good").is_none());
}

#[test]
fn has_binds_captures_from_the_leftmost_occurrence() {
    let p = pattern(&["sector ", "#n:int"]);
    assert_eq!(match_has(&p, "log: sector 3 then sector 9"), Some(vec![Value::Int(3)]));
}

#[test]
fn a_pattern_never_faults_on_any_input() {
    // 8.7: Matching ist total. Kein Eingabetext darf panicken; auch
    // mehrbytige Zeichen nicht, weil der Durchlauf an Zeichengrenzen
    // schneidet.
    let patterns = [
        pattern(&["#a:int"]),
        pattern(&["#a:hex"]),
        pattern(&["#a:float"]),
        pattern(&["#a:word"]),
        pattern(&["#a:8"]),
        pattern(&["#_"]),
        pattern(&["x", "#a:int", "y"]),
        pattern(&["#_", "z", "#_"]),
    ];
    let texts = ["", " ", "abc", "123", "0x", "-", "+", ".", "e", "üäö", "a\tb", "\u{1F600}", "xyz", "z"];
    for p in &patterns {
        for t in texts {
            let _ = match_text(p, t);
            let _ = match_has(p, t);
        }
    }
}
