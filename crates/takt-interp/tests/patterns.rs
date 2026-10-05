//! Musterabgleich (Referenz 8.7): Zeichenklassen je Art, Capture-Werte,
//! leftmost-shortest bei offenem Ende, `has` als Kurzform, und die Faelle,
//! die „kein Match" statt eines Faults ergeben.

use takt_interp::Value;
use takt_interp::pattern::{match_has, match_text};
use takt_mir::pattern::{CaptureKind, PatternPiece};
use takt_mir::types::FloatWidth;

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
    assert_eq!(match_text(&p, "READY", FloatWidth::F64), Some(vec![]));
    // `matches` verlangt den ganzen Text (8.7).
    assert_eq!(match_text(&p, "READY NOW", FloatWidth::F64), None);
    assert_eq!(match_text(&p, "NOT READY", FloatWidth::F64), None);
}

#[test]
fn int_captures_digits_with_an_optional_sign() {
    let p = pattern(&["Erasing sector ", "#n:int"]);
    assert_eq!(match_text(&p, "Erasing sector 7", FloatWidth::F64), Some(vec![Value::Int(7)]));
    assert_eq!(match_text(&p, "Erasing sector -3", FloatWidth::F64), Some(vec![Value::Int(-3)]));
    assert_eq!(match_text(&p, "Erasing sector +12", FloatWidth::F64), Some(vec![Value::Int(12)]));
    // Ohne Ziffer gibt es kein Match: jede Klasse verlangt mindestens eine.
    assert_eq!(match_text(&p, "Erasing sector ", FloatWidth::F64), None);
    assert_eq!(match_text(&p, "Erasing sector x", FloatWidth::F64), None);
}

#[test]
fn an_int_that_overflows_is_no_match_not_a_fault() {
    // 8.7: „Ueberlauf -> kein Match". Zwanzig Ziffern sprengen die Klasse
    // `[0-9]{1,19}`, und der Wert selbst passt nicht in `int`.
    let p = pattern(&["n=", "#n:int"]);
    assert_eq!(match_text(&p, "n=9223372036854775807", FloatWidth::F64), Some(vec![Value::Int(i64::MAX)]));
    assert_eq!(match_text(&p, "n=9223372036854775808", FloatWidth::F64), None);
}

#[test]
fn hex_accepts_an_optional_prefix() {
    let p = pattern(&["crc ", "#c:hex"]);
    assert_eq!(match_text(&p, "crc 1A2B", FloatWidth::F64), Some(vec![Value::Int(0x1A2B)]));
    assert_eq!(match_text(&p, "crc 0xff", FloatWidth::F64), Some(vec![Value::Int(0xFF)]));
    assert_eq!(match_text(&p, "crc zz", FloatWidth::F64), None);
}

#[test]
fn float_reads_fraction_and_exponent() {
    let p = pattern(&["dt ", "#d:float", " ms"]);
    assert_eq!(match_text(&p, "dt 1.5 ms", FloatWidth::F64), Some(vec![Value::F64(1.5)]));
    assert_eq!(match_text(&p, "dt -0.25 ms", FloatWidth::F64), Some(vec![Value::F64(-0.25)]));
    assert_eq!(match_text(&p, "dt 2e3 ms", FloatWidth::F64), Some(vec![Value::F64(2000.0)]));
    assert_eq!(match_text(&p, "dt 7 ms", FloatWidth::F64), Some(vec![Value::F64(7.0)]));
}

#[test]
fn a_float_that_is_not_finite_is_no_match() {
    // 8.7: „nicht endlich -> kein Match".
    let p = pattern(&["x ", "#x:float"]);
    assert_eq!(match_text(&p, "x 1e400", FloatWidth::F64), None);
}

#[test]
fn word_stops_at_the_first_character_outside_its_class() {
    let p = pattern(&["Recovery: ", "#outcome:word"]);
    assert_eq!(match_text(&p, "Recovery: OK", FloatWidth::F64), Some(vec![Value::Str("OK".into())]));
    assert_eq!(match_text(&p, "Recovery: retry_2", FloatWidth::F64), Some(vec![Value::Str("retry_2".into())]));
    // Das Leerzeichen gehoert nicht zur Klasse, also endet das Wort dort.
    let p = pattern(&["Recovery: ", "#outcome:word", " done"]);
    assert_eq!(match_text(&p, "Recovery: OK done", FloatWidth::F64), Some(vec![Value::Str("OK".into())]));
}

#[test]
fn str_ends_at_the_first_occurrence_of_the_next_literal() {
    // 8.7: leftmost-shortest. Das Muster endet beim *ersten* `]`, nicht beim
    // letzten.
    let p = pattern(&["[", "#tag:32", "]"]);
    assert_eq!(match_text(&p, "[a]", FloatWidth::F64), Some(vec![Value::Str("a".into())]));
    let p = pattern(&["[", "#tag:32", "] rest"]);
    assert_eq!(match_text(&p, "[a] rest", FloatWidth::F64), Some(vec![Value::Str("a".into())]));
}

#[test]
fn str_respects_its_length_bound() {
    let p = pattern(&["v=", "#s:4"]);
    assert_eq!(match_text(&p, "v=abcd", FloatWidth::F64), Some(vec![Value::Str("abcd".into())]));
    assert_eq!(match_text(&p, "v=abcde", FloatWidth::F64), None);
}

#[test]
fn any_discards_its_text() {
    let p = pattern(&["#_", "CRC mismatch", "#_"]);
    assert_eq!(match_text(&p, "line 7: CRC mismatch at 0x40", FloatWidth::F64), Some(vec![]));
    assert_eq!(match_text(&p, "all good", FloatWidth::F64), None);
}

#[test]
fn several_captures_bind_in_pattern_order() {
    let p = pattern(&["Boot v", "#major:int", ".", "#minor:int", " (", "#build:word", ")"]);
    assert_eq!(
        match_text(&p, "Boot v2.1 (rc3)", FloatWidth::F64),
        Some(vec![Value::Int(2), Value::Int(1), Value::Str("rc3".into())])
    );
}

#[test]
fn has_finds_the_pattern_anywhere() {
    // 8.7: `has P` ist `matches "{_}" + P + "{_}"`.
    let p = pattern(&["CRC mismatch"]);
    assert!(match_has(&p, "line 7: CRC mismatch at 0x40", FloatWidth::F64).is_some());
    assert!(match_has(&p, "CRC mismatch", FloatWidth::F64).is_some());
    assert!(match_has(&p, "all good", FloatWidth::F64).is_none());
}

#[test]
fn has_binds_captures_from_the_leftmost_occurrence() {
    let p = pattern(&["sector ", "#n:int"]);
    assert_eq!(match_has(&p, "log: sector 3 then sector 9", FloatWidth::F64), Some(vec![Value::Int(3)]));
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
            let _ = match_text(p, t, FloatWidth::F64);
            let _ = match_has(p, t, FloatWidth::F64);
        }
    }
}

/// INT-015: Die Raender der Klassen (8.7), mit dem Wert, den der
/// Interpreter liefert.
#[test]
fn the_classes_end_at_their_bounds() {
    let w = FloatWidth::F64;
    let hex = pattern(&["h=", "#x:hex"]);
    assert_eq!(match_text(&hex, "h=7fffffffffffffff", w), Some(vec![Value::Int(i64::MAX)]));
    assert_eq!(match_text(&hex, "h=8000000000000000", w), None, "Ueberlauf ist kein Match");
    assert_eq!(match_text(&hex, "h=0x1F", w), Some(vec![Value::Int(31)]));
    assert_eq!(match_text(&hex, "h=0X1F", w), None, "nur `0x` ist ein Praefix");
    let word = pattern(&["#w:word", ";"]);
    let w64 = "w".repeat(64);
    assert_eq!(match_text(&word, &format!("{w64};"), w), Some(vec![Value::Str(w64.clone())]));
    assert_eq!(match_text(&word, &format!("{w64}w;"), w), None, "65 Zeichen sind kein word");
    let float = pattern(&["#f:float"]);
    for text in ["1.", ".5", "1e", "1e+", "+", "-", "."] {
        assert_eq!(match_text(&float, text, w), None, "`{text}` ist als Ganzes keine Zahl");
    }
    assert_eq!(match_has(&float, "1.", w), Some(vec![Value::F64(1.0)]), "das Ganzzahlpraefix");
    assert_eq!(match_has(&float, ".5", w), Some(vec![Value::F64(5.0)]), "ab der Ziffer");
    assert_eq!(match_has(&float, "1e", w), Some(vec![Value::F64(1.0)]));
    let Some(caps) = match_text(&float, "-0.0", w) else { panic!("-0.0") };
    assert!(matches!(caps[..], [Value::F64(z)] if z.to_bits() == (-0.0f64).to_bits()), "{caps:?}");
}

/// 8.7 (SYN-039): Eine Klasse endet am ersten Zeichen ausserhalb oder am
/// Zeilenende, auch am Musterende unter `has`; ein laengerer Lauf als ihre
/// Hoechstlaenge trifft an dieser Stelle nicht und bindet nie seinen
/// Anfang. Ein `str<N>` am Musterende reicht bis zum Zeilenende und trifft
/// nicht, wenn es mehr als N Bytes fassen muesste — `has` setzt dann
/// spaeter an.
#[test]
fn a_class_at_the_end_of_the_pattern_takes_its_whole_run() {
    let w = FloatWidth::F64;
    let int = pattern(&["v", "#n:int"]);
    let d19 = "1234567890123456789";
    assert_eq!(match_has(&int, &format!("v{d19} ok"), w), Some(vec![Value::Int(1234567890123456789)]));
    assert_eq!(match_has(&int, &format!("v{d19}0"), w), None, "20 Ziffern");
    assert_eq!(match_has(&int, &format!("v-{d19}0 ok"), w), None, "20 Ziffern mit Vorzeichen");
    let hex = pattern(&["h=", "#x:hex"]);
    assert_eq!(match_has(&hex, "h=0x123456789abcdef0", w), Some(vec![Value::Int(0x1234_5678_9abc_def0)]));
    assert_eq!(match_has(&hex, "h=0x123456789abcdef01", w), None, "17 Ziffern");
    assert_eq!(match_has(&hex, "h=123456789abcdef01;", w), None, "17 Ziffern ohne Praefix");
    let word = pattern(&["u ", "#w:word"]);
    let w64 = "w".repeat(64);
    assert_eq!(match_has(&word, &format!("u {w64}!"), w), Some(vec![Value::Str(w64.clone())]));
    assert_eq!(match_has(&word, &format!("u {w64}w!"), w), None, "65 Zeichen");
    let s8 = pattern(&["#s:8"]);
    assert_eq!(match_has(&s8, "abcdefgh", w), Some(vec![Value::Str("abcdefgh".into())]));
    assert_eq!(match_has(&s8, "abcdefghij", w), Some(vec![Value::Str("cdefghij".into())]), "spaeterer Ansatz");
    assert_eq!(match_text(&s8, "abcdefghi", w), None, "9 Bytes fasst str<8> nicht");
    assert_eq!(match_has(&s8, "\u{e4}\u{f6}\u{fc}\u{e4}b", w), Some(vec![Value::Str("\u{f6}\u{fc}\u{e4}b".into())]));
}
