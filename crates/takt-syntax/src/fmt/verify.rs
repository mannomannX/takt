//! Prueft die Garantien des Formatters an einer Eingabe (format.md, Einleitung):
//! gleiche Tokens, gleicher Baum, Kommentare erhalten, nicht mehr Leerzeilen,
//! idempotent. Fuer Tests, `fmt --verify` und den Mutationstest.

use takt_diag::Diagnostic;

use super::{format, format_snippet};
use crate::parser::{parse_file, parse_snippet};
use crate::sexpr;
use crate::token::{TokenKind, Tokens, TriviaKind};
use crate::tokenize;

/// Formatiert `src` und prueft alle Garantien; liefert den formatierten Text
/// oder die Beschreibung der verletzten Garantie.
pub fn verify(src: &str, snippet: bool) -> Result<String, String> {
    if snippet { verify_with(src, snippet, format_snippet) } else { verify_with(src, snippet, format) }
}

/// [`verify`] mit einem beliebigen Formatter: Die Tests lassen jede Garantie an
/// einem absichtlich falschen Formatter scheitern.
fn verify_with(
    src: &str,
    snippet: bool,
    fmt: impl Fn(&str) -> Result<String, Vec<Diagnostic>>,
) -> Result<String, String> {
    let out = fmt(src).map_err(|e| e[0].to_string())?;
    let before = tokenize(src);
    let after = tokenize(&out);
    let shape_a = token_shape(&before);
    let shape_b = token_shape(&after);
    if shape_a != shape_b {
        let i = shape_a.iter().zip(&shape_b).position(|(a, b)| a != b).unwrap_or(shape_a.len().min(shape_b.len()));
        let line = before.tokens.get(i).map_or(0, |t| t.line);
        return Err(format!("Tokens weichen ab (Token {i}, Zeile {line})"));
    }
    if tree(&before, snippet) != tree(&after, snippet) {
        return Err("Baum weicht ab".into());
    }
    if comments(&before) != comments(&after) {
        return Err("Kommentare weichen ab".into());
    }
    if blank_count(&after) > blank_count(&before) {
        return Err("mehr Leerzeilen als vorher".into());
    }
    match fmt(&out) {
        Ok(again) if again == out => Ok(out),
        Ok(again) => {
            let line = out.lines().zip(again.lines()).position(|(a, b)| a != b).map_or(0, |i| i + 1);
            Err(format!("nicht idempotent ab Zeile {line}"))
        }
        Err(e) => Err(format!("zweiter Lauf: {}", e[0])),
    }
}

/// Tokens ohne Beiwerk als (Art, Text mit normiertem Leerraum).
fn token_shape(toks: &Tokens<'_>) -> Vec<(TokenKind, String)> {
    toks.tokens.iter().map(|t| (t.kind, toks.text(t).split_whitespace().collect::<Vec<_>>().join(" "))).collect()
}

fn tree(toks: &Tokens<'_>, snippet: bool) -> String {
    if snippet { sexpr::snippet(&parse_snippet(toks).0) } else { sexpr::file(&parse_file(toks).0) }
}

/// Die Kommentare mit dem Index des Tokens, vor dem sie als Beiwerk stehen: Ein
/// Kommentar bleibt an seiner Zeile (lexer.md L7), nicht nur irgendwo in der Datei.
fn comments(toks: &Tokens<'_>) -> Vec<(usize, String)> {
    toks.tokens
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            toks.trivia_of(t).iter().filter(|c| c.kind == TriviaKind::Comment).map(move |c| {
                (i, toks.src[c.start as usize..c.end as usize].trim_start_matches('#').trim().to_string())
            })
        })
        .collect()
}

fn blank_count(toks: &Tokens<'_>) -> usize {
    toks.trivia.iter().filter(|t| t.kind == TriviaKind::BlankLine).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein Formatter, der den echten laufen laesst und danach `from` durch `to` ersetzt.
    fn faulty(from: &'static str, to: &'static str) -> impl Fn(&str) -> Result<String, Vec<Diagnostic>> {
        move |s| format_snippet(s).map(|out| out.replacen(from, to, 1))
    }

    #[test]
    fn every_guarantee_catches_its_violation() {
        let cases = [
            ("x = a + b\n", faulty("a + b", "b + a"), "Tokens weichen ab"),
            ("x = 5 K/min\n", faulty("K/min", "K / min"), "Baum weicht ab"),
            ("x = 1  # c\ny = 2\n", faulty("1  # c\n", "1\n# c\n"), "Kommentare weichen ab"),
            ("x = 1  # c\n", faulty("  # c", ""), "Kommentare weichen ab"),
            ("x = 1\ny = 2\n", faulty("\n", "\n\n"), "mehr Leerzeilen als vorher"),
        ];
        for (src, fmt, want) in cases {
            let got = verify_with(src, true, fmt).expect_err(src);
            assert!(got.starts_with(want), "{src:?}: {got}");
        }
    }

    #[test]
    fn a_formatter_must_be_idempotent_and_must_not_fail() {
        // Der zweite Lauf verdoppelt den Abstand vor dem Kommentar erneut.
        let growing = |s: &str| Ok::<_, Vec<Diagnostic>>(s.replace(" #", "  #"));
        let got = verify_with("x = 1 # c\n", true, growing).expect_err("waechst");
        assert_eq!(got, "nicht idempotent ab Zeile 1");
        let once = std::cell::Cell::new(true);
        let second_fails = |s: &str| {
            if once.replace(false) { format_snippet(s) } else { format_snippet("x = = 1\n") }
        };
        let got = verify_with("x = 1\n", true, second_fails).expect_err("zweiter Lauf");
        assert!(got.starts_with("zweiter Lauf: error[P]"), "{got}");
        let got = verify("x = = 1\n", true).expect_err("kein gueltiger Schnipsel");
        assert!(got.starts_with("error[P]"), "{got}");
    }
}
