//! Formatiert Takt-Dateien: `cargo run --example fmt -- [--check] [--stdout] [--snippet] DATEI…`.
//! Ohne Option wird die Datei ueberschrieben; `--check` meldet nur, ob sie
//! kanonisch ist (Exit-Code 1 sonst); `--stdout` schreibt das Ergebnis auf die
//! Standardausgabe; `--snippet` liest die Dateien als Schnipsel; `--verify`
//! formatiert nur im Speicher und prueft die Garantien aus format.md.

use std::process::ExitCode;

use takt_syntax::fmt::verify;
use takt_syntax::{format, format_snippet};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let stdout = args.iter().any(|a| a == "--stdout");
    let snippet = args.iter().any(|a| a == "--snippet");
    let verify_only = args.iter().any(|a| a == "--verify");
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
        if verify_only {
            match verify(&src, snippet) {
                Ok(_) => println!("{path}: ok"),
                Err(e) => {
                    println!("{path}: {e}");
                    failed = true;
                }
            }
            continue;
        }
        let result = if snippet { format_snippet(&src) } else { format(&src) };
        let out = match result {
            Ok(out) => out,
            Err(errors) => {
                for e in errors {
                    eprintln!("{path}: {e}");
                }
                failed = true;
                continue;
            }
        };
        if stdout {
            print!("{out}");
        } else if check {
            if out != src {
                println!("{path}: nicht kanonisch");
                failed = true;
            }
        } else if out != src {
            if let Err(e) = std::fs::write(path, &out) {
                eprintln!("{path}: {e}");
                failed = true;
            } else {
                println!("{path}: formatiert");
            }
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
