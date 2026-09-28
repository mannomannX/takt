//! Der Produkt-DFA der Muster eines Handler-Blocks (8.7, 11.2).
//!
//! Die Abnahme der Semantik steht in `takt-interp/tests/dfa_agreement.rs`
//! (erschoepfend gegen den Durchlauf des Interpreters). Hier die Form:
//! Bits in Quelltextreihenfolge, `matches` gegen `has`, offene Enden,
//! Alphabetklassen und eine rechteckige Tabelle.

use takt_mir::dfa::{Entry, build};
use takt_mir::pattern::{CaptureKind, PatternPiece};

fn text(s: &str) -> PatternPiece {
    PatternPiece::Text(s.into())
}

fn cap(kind: CaptureKind) -> PatternPiece {
    PatternPiece::Capture { name: "x".into(), kind }
}

fn matches(pieces: &[PatternPiece]) -> Entry<'_> {
    Entry { pieces, has: false }
}

#[test]
fn a_literal_pattern_matches_itself() {
    let p = [text("READY")];
    let dfa = build(&[matches(&p)]).expect("Automat");
    assert_eq!(dfa.run(b"READY"), 1);
    assert_eq!(dfa.run(b"READ"), 0);
    assert_eq!(dfa.run(b"READYX"), 0);
}

#[test]
fn a_capture_consumes_its_class() {
    // `Erasing sector {n:int}` — die Ziffern gehoeren zum Platzhalter.
    let p = [text("Erasing sector "), cap(CaptureKind::Int)];
    let dfa = build(&[matches(&p)]).expect("Automat");
    assert_eq!(dfa.run(b"Erasing sector 3"), 1);
    assert_eq!(dfa.run(b"Erasing sector -42"), 1);
    assert_eq!(dfa.run(b"Erasing sector "), 0, "ein Platzhalter verlangt mindestens ein Zeichen");
    assert_eq!(dfa.run(b"Erasing sector x"), 0);
}

/// 11.2: Die Alphabetklassen fassen Bytes zusammen, die alle Muster
/// gleich behandeln — „typisch 10-25 statt 256".
#[test]
fn the_alphabet_is_smaller_than_the_byte_range() {
    let p = [text("Boot v"), cap(CaptureKind::Int), text("."), cap(CaptureKind::Int)];
    let dfa = build(&[matches(&p)]).expect("Automat");
    assert!(dfa.class_count < 32, "{} Klassen sind zu viele", dfa.class_count);
    assert_eq!(dfa.classes.len(), 256, "die Abbildung deckt jedes Byte ab");
}

/// Der Zweck des Produkt-DFA (11.2): Ein Durchlauf beantwortet alle
/// Muster eines Blocks, jedes mit seinem Bit.
#[test]
fn one_automaton_answers_several_patterns() {
    let a = [text("READY")];
    let b = [text("Erasing sector "), cap(CaptureKind::Int)];
    let c = [text("ERR")];
    let dfa = build(&[matches(&a), matches(&b), Entry { pieces: &c, has: true }]).expect("Automat");
    assert_eq!(dfa.run(b"READY"), 0b001);
    assert_eq!(dfa.run(b"Erasing sector 7"), 0b010);
    assert_eq!(dfa.run(b"Erasing sector 7 ERR"), 0b100);
    assert_eq!(dfa.run(b"Irgendwas"), 0);
}

/// Ein offenes Ende endet am ersten Vorkommen des Folgeliterals (8.7):
/// `x:y:5` trifft `{a}:{b:int}` nicht, weil `{a}` am ersten `:` endet.
#[test]
fn an_open_end_stops_at_the_first_occurrence() {
    let p = [PatternPiece::Any, text(":"), cap(CaptureKind::Int)];
    let dfa = build(&[matches(&p)]).expect("Automat");
    assert_eq!(dfa.run(b"x:5"), 1);
    assert_eq!(dfa.run(b"x:y:5"), 0);
    let p = [cap(CaptureKind::Str(3)), text("!")];
    let dfa = build(&[matches(&p)]).expect("Automat");
    assert_eq!(dfa.run(b"abc!"), 1);
    assert_eq!(dfa.run(b"abcd!"), 1, "die Grenze von `str<3>` prueft die Extraktion");
}

/// `has` darf an jeder Stelle beginnen; die Zeile muss nicht enden, wo
/// das Muster endet.
#[test]
fn has_finds_a_later_start() {
    let p = [text("sector "), cap(CaptureKind::Int)];
    let dfa = build(&[Entry { pieces: &p, has: true }]).expect("Automat");
    assert_eq!(dfa.run(b"sector x, sector 5 done"), 1);
    assert_eq!(dfa.run(b"sector x"), 0);
}

/// `float` bekommt keinen Automaten (der Codegen kennt seine Umwandlung
/// nicht, 4.2); `build` sagt es durch `None`.
#[test]
fn a_float_capture_has_no_table() {
    assert!(build(&[matches(&[text("t="), cap(CaptureKind::Float)])]).is_none());
}

/// Die Tabelle ist vollstaendig: je Zustand eine Zeile mit
/// `class_count` Spalten und ein Eintrag der Trefferliste.
#[test]
fn the_table_is_rectangular() {
    let p = [text("Boot v"), cap(CaptureKind::Int)];
    let dfa = build(&[matches(&p)]).expect("Automat");
    assert_eq!(dfa.table.len(), dfa.states() * dfa.class_count as usize, "die Tabelle ist rechteckig");
    assert!(dfa.table.iter().all(|s| usize::from(*s) < dfa.states()), "jeder Uebergang zeigt in die Tabelle");
}
