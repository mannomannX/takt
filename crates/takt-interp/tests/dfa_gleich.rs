//! Der Produkt-DFA sagt dasselbe wie der Vorwaertsdurchlauf (8.7, 11.2).
//!
//! Zwei Wege, dieselbe Frage: Der Automat entscheidet, *ob* ein Muster
//! trifft, der Durchlauf entscheidet es auch — und extrahiert daneben
//! die Werte. Wo sie sich unterscheiden, haette der erzeugte Code eine
//! andere Semantik als der Interpreter, und Satz 9.4.4 waere verletzt.
//!
//! Der Test ist darum kein Einheitentest des Automaten, sondern ein
//! differentieller: Er fuettert beiden dieselben Zeilen und vergleicht
//! das Urteil.

use takt_interp::pattern::match_text;
use takt_mir::dfa::build;
use takt_mir::pattern::{CaptureKind, Dfa, PatternPiece};

fn text(s: &str) -> PatternPiece {
    PatternPiece::Text(s.into())
}

fn cap(kind: CaptureKind) -> PatternPiece {
    PatternPiece::Capture { name: "x".into(), kind }
}

/// Das Urteil des Automaten.
fn dfa_says(dfa: &Dfa, line: &str) -> bool {
    let mut state = 0u32;
    for b in line.bytes() {
        let class = u32::from(dfa.classes[b as usize]);
        let Some(&next) = dfa.table.get((state * dfa.class_count + class) as usize) else { return false };
        state = next;
    }
    dfa.accept.contains(&state)
}

/// Die Zeilen, an denen beide gemessen werden.
///
/// Sie decken die Faelle ab, die 8.7 nennt: Treffer, Beinahe-Treffer
/// (ein Zeichen zu kurz, eines zu viel), leerer Platzhalter, falsche
/// Zeichenklasse und leerer Text.
const ZEILEN: [&str; 14] = [
    "",
    "READY",
    "READ",
    "READYY",
    "Boot v1",
    "Boot v12",
    "Boot v",
    "Boot vx",
    "Erasing sector 3",
    "Erasing sector 42",
    "Erasing sector ",
    "Erasing sector x",
    "xBoot v1",
    "Boot v1x",
];

fn vergleiche(pieces: &[PatternPiece]) {
    let Some(dfa) = build(&[pieces]) else { return };
    for line in ZEILEN {
        let durchlauf = match_text(pieces, line).is_some();
        let automat = dfa_says(&dfa, line);
        assert_eq!(
            durchlauf, automat,
            "`{line}`: Durchlauf sagt {durchlauf}, Automat sagt {automat} (Muster {pieces:?})"
        );
    }
}

#[test]
fn a_literal_agrees_on_every_line() {
    vergleiche(&[text("READY")]);
}

#[test]
fn a_trailing_capture_agrees_on_every_line() {
    vergleiche(&[text("Boot v"), cap(CaptureKind::Int)]);
    vergleiche(&[text("Erasing sector "), cap(CaptureKind::Int)]);
}

#[test]
fn an_inner_capture_agrees_on_every_line() {
    // `Boot v{major:int}.{minor:int}` — zwei Platzhalter mit Literal
    // dazwischen, wie 8.7 es verlangt (auf `int` folgt ein Zeichen
    // ausserhalb der Klasse).
    vergleiche(&[text("Boot v"), cap(CaptureKind::Int), text("."), cap(CaptureKind::Int)]);
}

#[test]
fn a_word_capture_agrees_on_every_line() {
    vergleiche(&[text("Recovery: "), cap(CaptureKind::Word)]);
}

/// Die Zeilen, die 14.6 wirklich sieht (Referenzbeispiel).
#[test]
fn the_patterns_of_the_reference_example_agree() {
    let muster: [Vec<PatternPiece>; 3] = [
        vec![text("Boot v"), cap(CaptureKind::Int), text("."), cap(CaptureKind::Int)],
        vec![text("Erasing sector "), cap(CaptureKind::Int)],
        vec![text("READY")],
    ];
    for m in &muster {
        vergleiche(m);
    }
    // Und als Produkt: Ein Durchlauf beantwortet alle drei (11.2).
    let refs: Vec<&[PatternPiece]> = muster.iter().map(Vec::as_slice).collect();
    let dfa = build(&refs).expect("Produkt-Automat");
    for line in ZEILEN {
        let einzeln = muster.iter().any(|m| match_text(m, line).is_some());
        assert_eq!(einzeln, dfa_says(&dfa, line), "`{line}`: das Produkt weicht von den Einzelmustern ab");
    }
}
