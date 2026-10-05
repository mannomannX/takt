//! `takt-match` und der Interpreter sagen dasselbe (8.7, Satz 9.4.4).
//!
//! Der Interpreter vergleicht heute mit seiner eigenen Fassung
//! (`takt_interp::pattern`), der erzeugte Code wird `takt-match`
//! benutzen. Zwei Implementierungen derselben Regeln sind zwei
//! Gelegenheiten, sie verschieden zu lesen — der Test misst, dass es
//! nicht passiert.
//!
//! Er ist damit die Bruecke, bis der Interpreter selbst auf `takt-match`
//! umgestellt ist: Danach gibt es nur noch eine Quelle, und der Test
//! wird zur Erinnerung daran, warum.

mod oracle;

use oracle::{agree, as_match};
use takt_interp::pattern::match_text;
use takt_interp::value::Value;
use takt_mir::pattern::{CaptureKind, PatternPiece};
use takt_mir::types::FloatWidth;

fn mir_text(s: &str) -> PatternPiece {
    PatternPiece::Text(s.into())
}

fn mir_cap(kind: CaptureKind) -> PatternPiece {
    PatternPiece::Capture { name: "x".into(), kind }
}

/// Die Zeilen, an denen beide gemessen werden.
const LINES: [&str; 16] = [
    "",
    "READY",
    "READ",
    "READYY",
    "Boot v1",
    "Boot v1.2",
    "Boot v12.34",
    "Boot v",
    "Boot vx",
    "Boot v1.",
    "Erasing sector 3",
    "Erasing sector 4294967296",
    "Erasing sector -5",
    "Erasing sector ",
    "Recovery: RECOVERED",
    "Recovery: ",
];

/// Vergleicht Urteil und Werte auf jeder Zeile.
fn compare_all(pieces: &[PatternPiece]) {
    for line in LINES {
        agree(pieces, line, false);
    }
}

#[test]
fn a_literal_agrees() {
    compare_all(&[mir_text("READY")]);
}

#[test]
fn an_int_capture_agrees_in_value_and_verdict() {
    compare_all(&[mir_text("Erasing sector "), mir_cap(CaptureKind::Int)]);
}

#[test]
fn two_captures_agree() {
    compare_all(&[mir_text("Boot v"), mir_cap(CaptureKind::Int), mir_text("."), mir_cap(CaptureKind::Int)]);
}

#[test]
fn a_word_capture_agrees() {
    compare_all(&[mir_text("Recovery: "), mir_cap(CaptureKind::Word)]);
}

/// 8.7: „Ueberlauf → kein Match" — beide Seiten muessen dieselbe Grenze
/// ziehen, sonst liefert eine von ihnen einen Wert, den es nicht gibt.
#[test]
fn the_overflow_boundary_agrees() {
    let p = [mir_text("n="), mir_cap(CaptureKind::Int)];
    let mp = as_match(&p);
    for text in ["n=9223372036854775807", "n=9223372036854775808", "n=-9223372036854775808"] {
        let interp = match_text(&p, text, FloatWidth::F64);
        let eigen = takt_match::matches(&mp, text.as_bytes())
            .and_then(|m| takt_match::parse_int(m.spans[0].of(text.as_bytes())));
        let interp_value = interp.and_then(|c| match c.first() {
            Some(Value::Int(i)) => Some(*i),
            _ => None,
        });
        assert_eq!(interp_value, eigen, "`{text}`: die Grenze weicht ab");
    }
}

/// xorshift64*, deterministisch: Ein Fehlschlag ist reproduzierbar.
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

/// Zufallsmuster ohne zwei Platzhalter hintereinander (Pruefung 18), mit
/// allen Arten einschliesslich `float`.
fn random_pattern(rng: &mut Rng) -> Vec<PatternPiece> {
    const LITERALS: [&str; 10] = ["a", "ab", ":", "=", "x", "0", "-", "\u{e4}", "a:a", " "];
    let mut pieces = Vec::new();
    let mut placeholder_last = false;
    for _ in 0..1 + rng.below(4) {
        if placeholder_last || rng.below(2) == 0 {
            pieces.push(PatternPiece::Text(rng.pick(&LITERALS).to_string()));
            placeholder_last = false;
            continue;
        }
        pieces.push(match rng.below(7) {
            0 => PatternPiece::Any,
            1 => mir_cap(CaptureKind::Int),
            2 => mir_cap(CaptureKind::Hex),
            3 => mir_cap(CaptureKind::Word),
            4 => mir_cap(CaptureKind::Float),
            _ => mir_cap(CaptureKind::Str(1 + rng.below(6) as u32)),
        });
        placeholder_last = true;
    }
    pieces
}

/// Zufallszeilen mit Umlauten, langen Ziffernfolgen um die Grenzen von
/// `i64` und 16 Hexziffern, Exponenten und Vorzeichen.
fn random_line(rng: &mut Rng) -> String {
    const TEXT: [&str; 18] =
        ["a", "b", ":", "=", "x", "0", "7", "9", "-", "+", "f", "\u{e4}", " ", "0x", ".", "e", "E", "A"];
    let mut s = String::new();
    for _ in 0..rng.below(24) {
        match rng.below(12) {
            0 => s.push_str(rng.pick(&["9223372036854775807", "9223372036854775808", "12345678901234567890"])),
            1 => s.push_str(rng.pick(&["7fffffffffffffff", "8000000000000000", "0x8000000000000000"])),
            2 => s.push_str(rng.pick(&["1e308", "1e309", "-0.0", "2.5e-3", "1."])),
            _ => s.push_str(rng.pick(&TEXT)),
        }
    }
    s
}

/// INT-016, SYN-040: Zufallsmuster gegen Zufallszeilen, als `matches` und
/// als `has`, mit Werten.
#[test]
fn random_patterns_agree_with_takt_match() {
    let mut rng = Rng(0x2026_1004_0016);
    for _ in 0..3000 {
        let pieces = random_pattern(&mut rng);
        for _ in 0..20 {
            let line = random_line(&mut rng);
            agree(&pieces, &line, false);
            agree(&pieces, &line, true);
        }
    }
}

/// INT-015: Die Grenzen der Klassen (8.7).
#[test]
fn the_class_bounds_agree() {
    let hex = [mir_text("h="), mir_cap(CaptureKind::Hex)];
    let float = [mir_text("f="), mir_cap(CaptureKind::Float)];
    let word = [mir_text("w="), mir_cap(CaptureKind::Word), mir_text(";")];
    let w64 = format!("w={};", "w".repeat(64));
    let w65 = format!("w={};", "w".repeat(65));
    for (pieces, line) in [
        (&hex[..], "h=7fffffffffffffff"),
        (&hex[..], "h=8000000000000000"),
        (&hex[..], "h=0x8000000000000000"),
        (&hex[..], "h=0X1F"),
        (&hex[..], "h=0x1F"),
        (&float[..], "f=1."),
        (&float[..], "f=.5"),
        (&float[..], "f=1e"),
        (&float[..], "f=-0.0"),
        (&float[..], "f=1e309"),
        (&word[..], &w64),
        (&word[..], &w65),
    ] {
        agree(pieces, line, false);
        agree(pieces, line, true);
    }
}
