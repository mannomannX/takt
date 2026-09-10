//! `takt`: Kommandozeile fuer `check`, `fmt`, `parse` und `tokens`.
//!
//! ```text
//! takt check DATEI… [--warnings-as-errors] [--certification] [--format text|line]
//! takt fmt DATEI… [--check] [--stdout] [--snippet] [--verify] [--edition]
//! takt parse DATEI… [--ast] [--debug] [--snippet]
//! takt tokens DATEI
//! ```
//! Exit-Code 1 bei Fehlern oder nicht kanonischen Dateien (`fmt --check`).

use std::process::ExitCode;

use takt_diag::{Policy, SourceMap};
use takt_syntax::fmt::{insert_edition, verify};
use takt_syntax::{Edition, TokenKind, format, format_snippet, parse_file, parse_snippet, sexpr, tokenize};

const USAGE: &str = "takt check|fmt|parse|tokens DATEI… (siehe crates/takt-cli/src/main.rs)";

struct Args {
    flags: Vec<String>,
    files: Vec<String>,
}

impl Args {
    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }

    fn value(&self, flag: &str) -> Option<&str> {
        self.flags.iter().find_map(|f| f.strip_prefix(flag).and_then(|r| r.strip_prefix('=')))
    }
}

fn main() -> ExitCode {
    let mut argv = std::env::args().skip(1);
    let Some(command) = argv.next() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let rest: Vec<String> = argv.collect();
    let args = Args {
        flags: rest.iter().filter(|a| a.starts_with("--")).cloned().collect(),
        files: rest.iter().filter(|a| !a.starts_with("--")).cloned().collect(),
    };
    let ok = match command.as_str() {
        "check" => check(&args),
        "fmt" => fmt(&args),
        "parse" => parse(&args),
        "tokens" => tokens(&args),
        _ => {
            eprintln!("{USAGE}");
            false
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn read(path: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("{path}: {e}");
            None
        }
    }
}

/// `takt check`: Diagnosen aller Schichten mit Quellauszug.
fn check(args: &Args) -> bool {
    let policy =
        Policy { warnings_as_errors: args.has("--warnings-as-errors"), certification: args.has("--certification") };
    let line_format = args.value("--format") == Some("line");
    let mut ok = true;
    for path in &args.files {
        let Some(src) = read(path) else {
            ok = false;
            continue;
        };
        let map = SourceMap::single(path.as_str(), src.as_str());
        let checked = takt_sema::check(&src, policy);
        for d in &checked.diagnostics {
            if line_format {
                println!("{}", map.render_line(d));
            } else {
                println!("{}", map.render(d));
            }
        }
        if checked.has_errors() {
            ok = false;
        }
    }
    ok
}

/// `takt fmt`: kanonische Form; `--check` meldet nur, `--stdout` schreibt auf
/// die Standardausgabe, `--verify` prueft die Garantien aus format.md,
/// `--edition` traegt `language = N` ein.
fn fmt(args: &Args) -> bool {
    let snippet = args.has("--snippet");
    let mut ok = true;
    for path in &args.files {
        let Some(mut src) = read(path) else {
            ok = false;
            continue;
        };
        if args.has("--verify") {
            match verify(&src, snippet) {
                Ok(_) => println!("{path}: ok"),
                Err(e) => {
                    println!("{path}: {e}");
                    ok = false;
                }
            }
            continue;
        }
        if args.has("--edition") {
            if let Some(with_edition) = insert_edition(&src, Edition::LATEST) {
                src = with_edition;
            }
        }
        let result = if snippet { format_snippet(&src) } else { format(&src) };
        let out = match result {
            Ok(out) => out,
            Err(errors) => {
                let map = SourceMap::single(path.as_str(), src.as_str());
                for e in errors {
                    eprintln!("{}", map.render_line(&e));
                }
                ok = false;
                continue;
            }
        };
        let original = std::fs::read_to_string(path).unwrap_or_default();
        if args.has("--stdout") {
            print!("{out}");
        } else if args.has("--check") {
            if out != original {
                println!("{path}: nicht kanonisch");
                ok = false;
            }
        } else if out != original {
            match std::fs::write(path, &out) {
                Ok(()) => println!("{path}: formatiert"),
                Err(e) => {
                    eprintln!("{path}: {e}");
                    ok = false;
                }
            }
        }
    }
    ok
}

/// `takt parse`: Syntaxfehler, `--ast` S-Expressions, `--debug` Rust-Debug.
fn parse(args: &Args) -> bool {
    let snippet = args.has("--snippet");
    let mut ok = true;
    for path in &args.files {
        let Some(src) = read(path) else {
            ok = false;
            continue;
        };
        let map = SourceMap::single(path.as_str(), src.as_str());
        let toks = tokenize(&src);
        for e in &toks.errors {
            println!("{}", map.render_line(e));
        }
        let (count, errors) = if snippet {
            let (items, errors) = parse_snippet(&toks);
            if args.has("--ast") {
                print!("{}", sexpr::snippet(&items));
            }
            if args.has("--debug") {
                println!("{items:#?}");
            }
            (items.len(), errors)
        } else {
            let (file, errors) = parse_file(&toks);
            if args.has("--ast") {
                print!("{}", sexpr::file(&file));
            }
            if args.has("--debug") {
                println!("{file:#?}");
            }
            (file.items.len(), errors)
        };
        for e in &errors {
            println!("{}", map.render_line(e));
        }
        if toks.errors.is_empty() && errors.is_empty() {
            println!("{path}: ok ({count} Eintraege)");
        } else {
            ok = false;
        }
    }
    ok
}

/// `takt tokens`: Tokenstrom als `ART<TAB>Text[<TAB>~]` (Differenzvergleich mit dem Orakel).
fn tokens(args: &Args) -> bool {
    let mut ok = true;
    for path in &args.files {
        let Some(src) = read(path) else {
            ok = false;
            continue;
        };
        let map = SourceMap::single(path.as_str(), src.as_str());
        let toks = tokenize(&src);
        for t in &toks.tokens {
            let text = match t.kind {
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof => String::new(),
                TokenKind::Duration => t.value.to_string(),
                TokenKind::Str => toks.unescape(t),
                _ => toks.text(t).to_string(),
            };
            let joint = if t.joint { "\t~" } else { "" };
            println!("{:?}\t{text}{joint}", t.kind);
        }
        for e in &toks.errors {
            eprintln!("{}", map.render_line(e));
            ok = false;
        }
    }
    ok
}
