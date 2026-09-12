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

use takt_interp::pattern::match_text;
use takt_interp::value::Value;
use takt_mir::pattern::{CaptureKind, PatternPiece};

fn mir_text(s: &str) -> PatternPiece {
    PatternPiece::Text(s.into())
}

fn mir_cap(kind: CaptureKind) -> PatternPiece {
    PatternPiece::Capture { name: "x".into(), kind }
}

/// Dasselbe Muster in der Form von `takt-match`.
fn als_match(pieces: &[PatternPiece]) -> Vec<takt_match::Piece<'_>> {
    pieces
        .iter()
        .map(|p| match p {
            PatternPiece::Text(t) => takt_match::Piece::Text(t.as_bytes()),
            PatternPiece::Any => takt_match::Piece::Capture(takt_match::Kind::Any),
            PatternPiece::Capture { kind, .. } => takt_match::Piece::Capture(match kind {
                CaptureKind::Int => takt_match::Kind::Int,
                CaptureKind::Hex => takt_match::Kind::Hex,
                CaptureKind::Float => takt_match::Kind::Float,
                CaptureKind::Word => takt_match::Kind::Word,
                CaptureKind::Str(n) => takt_match::Kind::Str(*n),
            }),
        })
        .collect()
}

/// Die Zeilen, an denen beide gemessen werden.
const ZEILEN: [&str; 16] = [
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
fn vergleiche(pieces: &[PatternPiece]) {
    let mp = als_match(pieces);
    for line in ZEILEN {
        let interp = match_text(pieces, line);
        let eigen = takt_match::matches(&mp, line.as_bytes());
        assert_eq!(
            interp.is_some(),
            eigen.is_some(),
            "`{line}`: Interpreter {} , takt-match {} ({pieces:?})",
            interp.is_some(),
            eigen.is_some()
        );
        let (Some(caps), Some(m)) = (interp, eigen) else { continue };
        // Die Werte: Der Interpreter liefert sie fertig, `takt-match`
        // die Bereiche. Fuer `int` und `word` muessen beide dasselbe
        // sagen — `float` bleibt aus, solange die Konversion fehlt.
        assert_eq!(caps.len(), m.count as usize, "`{line}`: verschieden viele Captures");
        for (n, c) in caps.iter().enumerate() {
            let text = core::str::from_utf8(m.spans[n].of(line.as_bytes())).unwrap_or("");
            match c {
                Value::Int(i) => {
                    let eigen =
                        takt_match::parse_int(text.as_bytes()).or_else(|| takt_match::parse_hex(text.as_bytes()));
                    assert_eq!(Some(*i), eigen, "`{line}`: Capture {n} weicht ab (`{text}`)");
                }
                Value::Str(s) => assert_eq!(s.as_str(), text, "`{line}`: Capture {n} weicht ab"),
                _ => {}
            }
        }
    }
}

#[test]
fn a_literal_agrees() {
    vergleiche(&[mir_text("READY")]);
}

#[test]
fn an_int_capture_agrees_in_value_and_verdict() {
    vergleiche(&[mir_text("Erasing sector "), mir_cap(CaptureKind::Int)]);
}

#[test]
fn two_captures_agree() {
    vergleiche(&[mir_text("Boot v"), mir_cap(CaptureKind::Int), mir_text("."), mir_cap(CaptureKind::Int)]);
}

#[test]
fn a_word_capture_agrees() {
    vergleiche(&[mir_text("Recovery: "), mir_cap(CaptureKind::Word)]);
}

/// 8.7: „Ueberlauf → kein Match" — beide Seiten muessen dieselbe Grenze
/// ziehen, sonst liefert eine von ihnen einen Wert, den es nicht gibt.
#[test]
fn the_overflow_boundary_agrees() {
    let p = [mir_text("n="), mir_cap(CaptureKind::Int)];
    let mp = als_match(&p);
    for text in ["n=9223372036854775807", "n=9223372036854775808", "n=-9223372036854775808"] {
        let interp = match_text(&p, text);
        let eigen = takt_match::matches(&mp, text.as_bytes())
            .and_then(|m| takt_match::parse_int(m.spans[0].of(text.as_bytes())));
        let interp_wert = interp.and_then(|c| match c.first() {
            Some(Value::Int(i)) => Some(*i),
            _ => None,
        });
        assert_eq!(interp_wert, eigen, "`{text}`: die Grenze weicht ab");
    }
}
