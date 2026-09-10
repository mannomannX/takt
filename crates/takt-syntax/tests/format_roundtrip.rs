//! Formatter-Roundtrip ueber Korpus und Referenzschnipsel: gleiche Tokens,
//! gleicher Baum, idempotent, Kommentare erhalten, nicht mehr Leerzeilen, und
//! eine zerknitterte Variante formatiert zum selben Ergebnis.

use std::path::{Path, PathBuf};

use takt_syntax::fmt::verify;
use takt_syntax::{TokenKind, format, format_snippet, tokenize};

fn corpus_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try"))
}

fn takt_files(dir: &Path, positive_only: bool) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("Verzeichnis lesbar")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .filter(|p| !positive_only || !p.file_name().and_then(|n| n.to_str()).unwrap_or_default().starts_with("n0"))
        .collect();
    files.sort();
    files
}

/// Zerknittert eine Datei, ohne Tokens oder Bedeutung zu aendern: doppelter
/// Leerraum zwischen Tokens, verschobene Fortsetzungszeilen, Kommentare ohne
/// Leerzeichen nach `#`, Leerraum am Zeilenende, doppelte Leerzeilen.
fn crumple(src: &str) -> String {
    let toks = tokenize(src);
    let mut line_starts = vec![0usize];
    line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let mut out = String::new();
    let mut prev_kind = TokenKind::Newline;
    for (n, line) in src.split_inclusive('\n').enumerate() {
        let start = line_starts[n];
        let end = start + line.trim_end_matches(['\n', '\r']).len();
        let on_line: Vec<_> =
            toks.tokens.iter().filter(|t| (t.start as usize) >= start && (t.start as usize) < end).collect();
        let mut text = String::new();
        let first_real =
            on_line.iter().find(|t| !matches!(t.kind, TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline));
        let continuation = first_real.is_some()
            && !matches!(prev_kind, TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof);
        let mut pos = start;
        for t in &on_line {
            if matches!(t.kind, TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline) {
                continue;
            }
            let gap = &src[pos..t.start as usize];
            if pos == start {
                text.push_str(gap);
                if continuation {
                    text.push_str("  ");
                }
            } else if !gap.is_empty() {
                text.push_str(gap);
                text.push(' ');
            }
            text.push_str(&src[t.start as usize..t.end as usize]);
            pos = t.end as usize;
            prev_kind = t.kind;
        }
        let rest = &src[pos..end];
        match rest.find('#') {
            // genau ein Leerzeichen nach `#` entfernen; mehr Leerraum ist Inhalt
            Some(i) if rest[i + 1..].starts_with(' ') && !rest[i + 1..].starts_with("  ") => {
                text.push_str(&rest[..i]);
                text.push('#');
                text.push_str(&rest[i + 2..]);
            }
            _ => text.push_str(rest),
        }
        if toks.tokens.iter().any(|t| t.kind == TokenKind::Newline && t.start as usize == end) {
            prev_kind = TokenKind::Newline;
        }
        if text.trim().is_empty() {
            out.push_str("\n\n");
        } else {
            out.push_str(text.trim_end());
            out.push_str("   \n");
        }
    }
    out
}

fn check(path: &Path, snippet: bool, failures: &mut Vec<String>) {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    let src = std::fs::read_to_string(path).expect("lesbar");
    let fmt = |s: &str| if snippet { format_snippet(s) } else { format(s) };
    let out = match verify(&src, snippet) {
        Ok(o) => o,
        Err(e) => {
            failures.push(format!("{name}: {e}"));
            return;
        }
    };
    let crumpled = crumple(&src);
    match fmt(&crumpled) {
        Ok(same) if same == out => {}
        Ok(same) => {
            let line = out.lines().zip(same.lines()).position(|(a, b)| a != b).map(|i| i + 1);
            failures.push(format!("{name}: zerknitterte Variante formatiert anders ab Zeile {}", line.unwrap_or(0)));
        }
        Err(e) => failures.push(format!("{name}: zerknitterte Variante: {}", e[0])),
    }
}

#[test]
fn corpus_roundtrips() {
    let mut failures = Vec::new();
    for path in takt_files(&corpus_root(), true) {
        check(&path, false, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn reference_snippets_roundtrip() {
    let mut failures = Vec::new();
    let files = takt_files(&corpus_root().join("ref"), false);
    assert!(files.len() > 30);
    for path in files {
        check(&path, true, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn broken_input_is_left_alone() {
    assert!(format("machine m:\n    state A:\n        pass\n").is_err());
    assert!(format("x = 1\ty\n").is_err());
}
