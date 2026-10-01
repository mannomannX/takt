//! Der Durchlaufautomat des Codegens (`takt_mir::scan`, FB-351) findet fuer
//! `has` dieselbe frueheste Stelle wie der Interpreter — die Spezifikation,
//! die den Durchlauf an jeder Stelle neu ansetzt (8.7).
//!
//! Zufallsmuster aus Literalen, `int`, `hex`, `word`, `str<N>` und `{_}`
//! (ohne zwei Platzhalter hintereinander, Pruefung 18) gegen Zufallszeilen
//! mit Umlauten, langen Ziffernfolgen um den Wertebereich von `i64`, `0x`
//! und Vorzeichen. Der Generator hat einen festen Startwert; ein Fehlschlag
//! ist reproduzierbar.

use takt_interp::pattern::has_start;
use takt_mir::pattern::{CaptureKind, PatternPiece};
use takt_mir::scan::Scan;

/// xorshift64*, deterministisch.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const LITERALS: [&str; 9] = ["a", "ab", ":", "=", "x", "0", "-", "ä", "a:a"];
const TEXT: [&str; 14] = ["a", "b", ":", "=", "x", "0", "7", "9", "-", "+", "f", "ä", " ", "0x"];

fn pattern(rng: &mut Rng) -> Vec<PatternPiece> {
    let mut pieces = Vec::new();
    let mut placeholder_last = false;
    for _ in 0..1 + rng.below(4) {
        if placeholder_last || rng.below(2) == 0 {
            pieces.push(PatternPiece::Text(rng.pick(&LITERALS).to_string()));
            placeholder_last = false;
            continue;
        }
        let kind = match rng.below(6) {
            0 => None,
            1 => Some(CaptureKind::Int),
            2 => Some(CaptureKind::Hex),
            3 => Some(CaptureKind::Word),
            _ => Some(CaptureKind::Str(1 + rng.below(6) as u32)),
        };
        pieces.push(match kind {
            None => PatternPiece::Any,
            Some(kind) => PatternPiece::Capture { name: "x".into(), kind },
        });
        placeholder_last = true;
    }
    pieces
}

fn line(rng: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..rng.below(24) {
        match rng.below(12) {
            // Ziffernfolgen um 19 Stellen: ueber und unter `i64::MAX`.
            0 => s.push_str(rng.pick(&["9223372036854775807", "9223372036854775808", "12345678901234567890"])),
            // Sechzehn Hexziffern um die Grenze.
            1 => s.push_str(rng.pick(&["7fffffffffffffff", "8000000000000000", "0x8000000000000000"])),
            _ => s.push_str(rng.pick(&TEXT)),
        }
    }
    s
}

/// Dazu haelt jeder Lauf die Schranke, mit der die Kostenanalyse rechnet.
#[test]
fn the_automaton_finds_the_earliest_start_of_the_interpreter() {
    let mut rng = Rng(0x2026_1001_0351);
    let (mut checked, mut hits, mut bounded) = (0, 0, 0);
    for _ in 0..4000 {
        let pieces = pattern(&mut rng);
        let scan = Scan::of(&pieces, 64).unwrap_or_else(|| panic!("kein Automat fuer {pieces:?}"));
        let worst = scan.worst_step();
        bounded += usize::from(worst.is_some());
        for _ in 0..25 {
            let text = line(&mut rng);
            let want = has_start(&pieces, &text);
            // Die Kostenanalyse rechnet mit `worst_step`; kein Lauf darf
            // mehr Arbeit an einer Stelle haben.
            let got = scan.trace(text.as_bytes(), |work| {
                if let Some(w) = worst {
                    assert!(
                        work.threads <= w.threads && work.checks <= w.checks,
                        "Muster {pieces:?}\nZeile {text:?}: {work:?} ueber {w:?}"
                    );
                }
            });
            assert_eq!(got, want, "Muster {pieces:?}\nZeile {text:?}");
            checked += 1;
            hits += usize::from(want.is_some());
        }
    }
    // Genug Treffer und genug Fehlschlaege, dass beide Seiten geprueft sind,
    // und genug Muster mit Schranke.
    assert!(hits > checked / 10 && hits < checked * 9 / 10, "{hits} Treffer von {checked}");
    assert!(bounded > 3000, "nur {bounded} von 4000 Mustern mit Schranke");
}

/// Ein `str<N>`, dessen Grenze die Zeile nicht erreichen kann, braucht keinen
/// Zaehler; das Ergebnis bleibt dasselbe.
#[test]
fn a_limit_beyond_the_line_changes_nothing() {
    let pieces =
        [PatternPiece::Capture { name: "s".into(), kind: CaptureKind::Str(40) }, PatternPiece::Text(";".into())];
    let wide = Scan::of(&pieces, 32).expect("Automat");
    let counted = Scan::of(&pieces, 64).expect("Automat");
    assert!(wide.states() < counted.states());
    for text in ["abc;", "ä;", ";", "abc"] {
        assert_eq!(wide.first(text.as_bytes()), has_start(&pieces, text), "{text}");
        assert_eq!(counted.first(text.as_bytes()), has_start(&pieces, text), "{text}");
    }
}
