//! Prueft die Garantien des Formatters an einer Eingabe (format.md, Einleitung):
//! gleiche Tokens, gleicher Baum, Kommentare erhalten, nicht mehr Leerzeilen,
//! idempotent. Fuer Tests, `fmt --verify` und den Mutationstest.

use super::{format, format_snippet};
use crate::parser::{parse_file, parse_snippet};
use crate::sexpr;
use crate::token::{TokenKind, Tokens, TriviaKind};
use crate::tokenize;

/// Formatiert `src` und prueft alle Garantien; liefert den formatierten Text
/// oder die Beschreibung der verletzten Garantie.
pub fn verify(src: &str, snippet: bool) -> Result<String, String> {
    let fmt = |s: &str| if snippet { format_snippet(s) } else { format(s) };
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

fn comments(toks: &Tokens<'_>) -> Vec<String> {
    let mut out: Vec<String> = toks
        .trivia
        .iter()
        .filter(|t| t.kind == TriviaKind::Comment)
        .map(|t| toks.src[t.start as usize..t.end as usize].trim_start_matches('#').trim().to_string())
        .collect();
    out.sort();
    out
}

fn blank_count(toks: &Tokens<'_>) -> usize {
    toks.trivia.iter().filter(|t| t.kind == TriviaKind::BlankLine).count()
}
