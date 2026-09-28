//! Der Produkt-DFA sagt dasselbe wie der Vorwaertsdurchlauf (8.7, 11.2).
//!
//! Zwei Wege, dieselbe Frage: Der Automat entscheidet in einem Durchlauf,
//! welche Muster treffen, der Interpreter laeuft jedes Muster ab — und
//! extrahiert daneben die Werte. Ein Muster ohne Platzhalter entscheidet
//! der Automat genau; eines mit Platzhaltern als Obermenge, deren Treffer
//! die Extraktion bestaetigt (`takt_mir::dfa::extracts`). Sagte der
//! Automat bei einer Zeile nein, auf der der Durchlauf gelingt, haette der
//! erzeugte Code eine andere Semantik als der Interpreter (Satz 9.4.4).
//!
//! Der Test ist differentiell und erschoepfend: jedes Muster als
//! `matches` und als `has`, einzeln und im Produkt, gegen alle Texte bis
//! zur Laenge fuenf aus einem Alphabet, das jede Klasse und jeden
//! Sonderfall beruehrt (Vorzeichen, `0x`, Klassenende, Folgeliteral).

use takt_interp::pattern::{match_has, match_text};
use takt_mir::dfa::{Entry, build, extracts};
use takt_mir::pattern::{CaptureKind, PatternPiece};

fn text(s: &str) -> PatternPiece {
    PatternPiece::Text(s.into())
}

fn cap(kind: CaptureKind) -> PatternPiece {
    PatternPiece::Capture { name: "x".into(), kind }
}

/// Muster, die Pruefung 18 annimmt, ueber dem Alphabet der Texte.
fn patterns() -> Vec<Vec<PatternPiece>> {
    use CaptureKind::{Hex, Int, Str, Word};
    vec![
        vec![text("a")],
        vec![text("a:")],
        vec![cap(Int)],
        vec![text("a"), cap(Int)],
        vec![cap(Int), text(":")],
        vec![cap(Int), text(":"), cap(Int)],
        vec![text("-"), cap(Int)],
        vec![cap(Int), text("x")],
        vec![cap(Hex)],
        vec![text("x"), cap(Hex), text(":")],
        vec![text("0x"), cap(Hex)],
        vec![cap(Hex), text("-")],
        vec![cap(Word)],
        vec![cap(Word), text(":"), cap(Int)],
        vec![PatternPiece::Any],
        vec![PatternPiece::Any, text(":")],
        vec![PatternPiece::Any, text(":"), cap(Int)],
        vec![text(":"), PatternPiece::Any],
        vec![PatternPiece::Any, text("a:"), PatternPiece::Any],
        vec![text("a"), PatternPiece::Any, text("a")],
        vec![PatternPiece::Any, text("aa:")],
        vec![cap(Str(2)), text(":")],
        vec![text("a"), cap(Str(3))],
        vec![cap(Str(2)), text("-"), cap(Str(1))],
        vec![cap(Str(3)), text("aa")],
    ]
}

/// Alle Texte bis zur Laenge fuenf ueber `a 1 0 x - :`.
fn texts() -> Vec<String> {
    const ALPHABET: [char; 6] = ['a', '1', '0', 'x', '-', ':'];
    let mut out = vec![String::new()];
    let mut layer = vec![String::new()];
    for _ in 0..5 {
        layer = layer.iter().flat_map(|t| ALPHABET.iter().map(move |c| format!("{t}{c}"))).collect();
        out.extend(layer.iter().cloned());
    }
    out
}

fn interpreter(pieces: &[PatternPiece], has: bool, line: &str) -> bool {
    if has { match_has(pieces, line).is_some() } else { match_text(pieces, line).is_some() }
}

/// Das Urteil des Codegens: der Automat, bei Platzhaltern bestaetigt vom
/// Durchlauf.
fn generated(automat: bool, pieces: &[PatternPiece], has: bool, line: &str) -> bool {
    automat && (!extracts(pieces) || interpreter(pieces, has, line))
}

/// Bis zur Laenge fuenf erreichen `int`, `hex` und `word` ihre
/// Hoechstlaenge nicht; nur `str<N>` hat eine erreichbare Grenze.
fn bounded(pieces: &[PatternPiece]) -> bool {
    pieces.iter().any(|p| matches!(p, PatternPiece::Capture { kind: CaptureKind::Str(_), .. }))
}

fn check(pieces: &[PatternPiece], has: bool, line: &str, automat: bool, exact: bool, what: &str) {
    let durchlauf = interpreter(pieces, has, line);
    assert!(automat || !durchlauf, "`{line}`: der Automat verfehlt einen Treffer ({what}, {pieces:?}, has {has})");
    if exact {
        assert_eq!(automat, durchlauf, "`{line}`: ({what}, {pieces:?}, has {has})");
    }
    assert_eq!(generated(automat, pieces, has, line), durchlauf, "`{line}`: ({what}, {pieces:?}, has {has})");
}

#[test]
fn every_pattern_agrees_on_every_short_text() {
    let lines = texts();
    for pieces in patterns() {
        for has in [false, true] {
            let dfa = build(&[Entry { pieces: &pieces, has }]).expect("Automat");
            for line in &lines {
                check(&pieces, has, line, dfa.run(line.as_bytes()) == 1, !bounded(&pieces), "einzeln");
            }
        }
    }
}

/// Ein Durchlauf beantwortet alle Muster eines Blocks (11.2): Jedes Bit
/// des Produkts ist das Urteil seines Musters.
#[test]
fn the_product_answers_each_pattern() {
    let all = patterns();
    // Zwoelf Handler je Strom, jeder Kopf als `matches` und als `has`: mehr,
    // als ein Zustand ueblicherweise traegt.
    let chosen: Vec<(&[PatternPiece], bool)> = [2, 5, 9, 13, 16, 18, 20, 21, 22, 24, 11, 0]
        .iter()
        .flat_map(|i| [(all[*i].as_slice(), false), (all[*i].as_slice(), true)])
        .collect();
    let entries: Vec<Entry<'_>> = chosen.iter().map(|(pieces, has)| Entry { pieces, has: *has }).collect();
    let dfa = build(&entries).expect("Produkt-Automat");
    for line in texts() {
        let mask = dfa.run(line.as_bytes());
        for (bit, (pieces, has)) in chosen.iter().enumerate() {
            check(pieces, *has, &line, mask >> bit & 1 == 1, !bounded(pieces), &format!("Bit {bit}"));
        }
    }
}

/// Die Hoechstlaengen kennt der Automat nicht; er nimmt die zu langen
/// Zeilen an, und die Extraktion weist sie ab.
#[test]
fn the_bounds_are_left_to_the_extraction() {
    let cases: [(Vec<PatternPiece>, &[&str], &str); 4] = [
        (
            vec![text("v"), cap(CaptureKind::Int)],
            &["v1234567890123456789", "v+1", "v-", "v+-1"],
            "v12345678901234567890",
        ),
        (
            vec![text("h"), cap(CaptureKind::Hex), text(":")],
            &["h0123456789abcdef:", "h0x0123456789abcdef:", "h0X1:", "h0x:"],
            "h0123456789abcdef0:",
        ),
        (vec![cap(CaptureKind::Word), text(":")], &[&format!("{}:", "w".repeat(64))], &format!("{}:", "w".repeat(65))),
        (vec![cap(CaptureKind::Str(4)), text(":")], &["abcd:", "ab:cd:", ":"], "abcde:"),
    ];
    for (pieces, lines, too_long) in cases {
        for has in [false, true] {
            let dfa = build(&[Entry { pieces: &pieces, has }]).expect("Automat");
            for line in lines.iter().chain([&too_long]) {
                check(&pieces, has, line, dfa.run(line.as_bytes()) == 1, false, "Grenze");
            }
            if !has {
                assert_eq!(dfa.run(too_long.as_bytes()), 1, "`{too_long}`: die Obermenge nimmt ihn an");
                assert!(!interpreter(&pieces, has, too_long), "`{too_long}`: der Durchlauf nicht");
            }
        }
    }
}

/// Den Wertebereich kennt der Automat nicht: 19 Ziffern ueber `i64::MAX`
/// treffen im Automaten, im Durchlauf nicht.
#[test]
fn the_value_range_is_left_to_the_extraction() {
    let pieces = vec![text("v"), cap(CaptureKind::Int)];
    let dfa = build(&[Entry { pieces: &pieces, has: false }]).expect("Automat");
    assert_eq!(dfa.run(b"v9999999999999999999"), 1);
    assert!(match_text(&pieces, "v9999999999999999999").is_none());
}
