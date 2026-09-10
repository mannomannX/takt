//! Parst Takt-Dateien und meldet Syntaxfehler: `cargo run --example parse -- datei.takt …`.
//! Mit `--snippet` werden die Dateien als Schnipsel gelesen (Deklarationen,
//! Zustandsinhalte und Anweisungen gemischt); `--ast` gibt den Baum als
//! S-Expressions aus, `--debug` als Rust-Debug mit Spannen. Exit-Code 1 bei Fehlern.

use std::process::ExitCode;

use takt_syntax::{parse_file, parse_snippet, sexpr, tokenize};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let snippet = args.iter().any(|a| a == "--snippet");
    let show_ast = args.iter().any(|a| a == "--ast");
    let show_debug = args.iter().any(|a| a == "--debug");
    let mut failed = false;
    for path in args.iter().filter(|a| !a.starts_with("--")) {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{path}: {e}");
                failed = true;
                continue;
            }
        };
        let toks = tokenize(&src);
        for e in &toks.errors {
            println!("{path}: {e}");
        }
        let (count, errors) = if snippet {
            let (items, errors) = parse_snippet(&toks);
            if show_ast {
                print!("{}", sexpr::snippet(&items));
            }
            if show_debug {
                println!("{items:#?}");
            }
            (items.len(), errors)
        } else {
            let (file, errors) = parse_file(&toks);
            if show_ast {
                print!("{}", sexpr::file(&file));
            }
            if show_debug {
                println!("{file:#?}");
            }
            (file.items.len(), errors)
        };
        for e in &errors {
            println!("{path}: {e}");
        }
        if toks.errors.is_empty() && errors.is_empty() {
            println!("{path}: ok ({count} Eintraege)");
        } else {
            failed = true;
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
