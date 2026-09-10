//! Gibt den Tokenstrom einer Datei aus, ein Token je Zeile: `ART<TAB>Text`.
//! Dient dem Vergleich mit `grammar/parse_corpus.py --tokens`.

use takt_syntax::{TokenKind, tokenize};

fn main() {
    let path = std::env::args().nth(1).expect("Aufruf: tokens DATEI");
    let src = std::fs::read_to_string(&path).expect("Datei lesbar");
    let toks = tokenize(&src);
    for t in &toks.tokens {
        let text = match t.kind {
            TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof => String::new(),
            TokenKind::Duration => t.value.to_string(),
            TokenKind::Str => toks.unescape(t),
            _ => toks.text(t).to_string(),
        };
        println!("{:?}\t{}{}", t.kind, text, if t.joint { "\t~" } else { "" });
    }
    for e in &toks.errors {
        eprintln!("{e}");
    }
    if !toks.errors.is_empty() {
        std::process::exit(1);
    }
}
