//! Der Produkt-DFA der Muster (8.7, 11.2).
//!
//! Die Abnahme ist die Uebereinstimmung mit dem Vorwaertsdurchlauf: Der
//! Automat sagt, *ob* ein Muster trifft; was in den Captures steht,
//! bleibt Sache der Extraktion. Wo beide dasselbe beantworten, muessen
//! sie dasselbe sagen — sonst haette der Codegen eine andere Semantik
//! als der Interpreter.

use takt_mir::dfa::build;
use takt_mir::pattern::{CaptureKind, PatternPiece};

fn text(s: &str) -> PatternPiece {
    PatternPiece::Text(s.into())
}

fn cap(kind: CaptureKind) -> PatternPiece {
    PatternPiece::Capture { name: "x".into(), kind }
}

/// Laeuft der Automat ueber den Text und endet er akzeptierend?
fn matches(dfa: &takt_mir::pattern::Dfa, text: &str) -> bool {
    let mut state = 0u32;
    for b in text.bytes() {
        let class = u32::from(dfa.classes[b as usize]);
        let i = (state * dfa.class_count + class) as usize;
        let Some(&next) = dfa.table.get(i) else { return false };
        state = next;
    }
    dfa.accept.contains(&state)
}

#[test]
fn a_literal_pattern_matches_itself() {
    let p = [text("READY")];
    let dfa = build(&[&p]).expect("Automat");
    assert!(matches(&dfa, "READY"));
    assert!(!matches(&dfa, "READ"));
    assert!(!matches(&dfa, "READYX"));
}

#[test]
fn a_capture_consumes_its_class() {
    // `Erasing sector {n:int}` — die Ziffern gehoeren zum Platzhalter.
    let p = [text("Erasing sector "), cap(CaptureKind::Int)];
    let dfa = build(&[&p]).expect("Automat");
    assert!(matches(&dfa, "Erasing sector 3"));
    assert!(matches(&dfa, "Erasing sector 42"));
    assert!(!matches(&dfa, "Erasing sector "), "ein Platzhalter verlangt mindestens ein Zeichen");
    assert!(!matches(&dfa, "Erasing sector x"));
}

/// 11.2: Die Alphabetklassen fassen Bytes zusammen, die alle Muster
/// gleich behandeln — „typisch 10-25 statt 256".
#[test]
fn the_alphabet_is_smaller_than_the_byte_range() {
    let p = [text("Boot v"), cap(CaptureKind::Int), text("."), cap(CaptureKind::Int)];
    let dfa = build(&[&p]).expect("Automat");
    assert!(dfa.class_count < 32, "{} Klassen sind zu viele", dfa.class_count);
    assert_eq!(dfa.classes.len(), 256, "die Abbildung deckt jedes Byte ab");
}

/// Der Zweck des Produkt-DFA (11.2): Ein Durchlauf beantwortet alle
/// Muster eines Zustands, nicht einer je Muster.
#[test]
fn one_automaton_answers_several_patterns() {
    let a = [text("READY")];
    let b = [text("Erasing sector "), cap(CaptureKind::Int)];
    let c = [text("Boot v"), cap(CaptureKind::Int)];
    let dfa = build(&[&a, &b, &c]).expect("Automat");
    assert!(matches(&dfa, "READY"));
    assert!(matches(&dfa, "Erasing sector 7"));
    assert!(matches(&dfa, "Boot v2"));
    assert!(!matches(&dfa, "Irgendwas"));
}

/// Ein offenes Ende (`str<N>`, `{_}`) endet erst am Folgeliteral; das
/// kann der Vorwaertsdurchlauf besser als eine Tabelle, und `build`
/// sagt es durch `None`.
#[test]
fn an_open_ended_pattern_has_no_table() {
    assert!(build(&[&[text("x"), PatternPiece::Any][..]]).is_none());
    assert!(build(&[&[cap(CaptureKind::Str(8)), text("!")][..]]).is_none());
}

/// Die Tabelle ist vollstaendig: je Zustand eine Zeile mit
/// `class_count` Spalten.
#[test]
fn the_table_is_rectangular() {
    let p = [text("Boot v"), cap(CaptureKind::Int)];
    let dfa = build(&[&p]).expect("Automat");
    assert_eq!(dfa.table.len() % dfa.class_count as usize, 0, "die Tabelle ist rechteckig");
    let states = dfa.table.len() / dfa.class_count as usize;
    assert!(dfa.accept.iter().all(|s| (*s as usize) < states), "ein akzeptierender Zustand liegt in der Tabelle");
    assert!(dfa.table.iter().all(|s| (*s as usize) < states), "jeder Uebergang zeigt in die Tabelle");
}
