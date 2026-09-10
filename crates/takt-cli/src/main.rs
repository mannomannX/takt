//! `takt`: Kommandozeile fuer `check`, `sim`, `mir`, `fmt`, `parse` und `tokens`.
//!
//! ```text
//! takt check DATEI… [--warnings-as-errors] [--certification] [--format text|line]
//!                   [--build sim|hw] [--profile P]
//! takt sim   DATEI --ticks N [--stim S.trace] [--golden G.trace] [--trace OUT.trace]
//!                   [--profile P] [--order random:SEED]
//! takt mir   DATEI [--dump] [--write OUT.mir] [--hash]
//! takt fmt   DATEI… [--check] [--stdout] [--snippet] [--verify] [--edition]
//! takt parse DATEI… [--ast] [--debug] [--snippet]
//! takt tokens DATEI
//! ```
//! Exit-Code 1 bei Fehlern, nicht kanonischen Dateien (`fmt --check`), einem
//! Golden-Unterschied oder dem Lauf-Verdikt FAIL (13.5).

use std::process::ExitCode;

use takt_diag::{Policy, SourceMap};
use takt_interp::{RunOptions, Trace, Verdict};
use takt_syntax::fmt::{insert_edition, verify};
use takt_syntax::{Edition, TokenKind, format, format_snippet, parse_file, parse_snippet, sexpr, tokenize};

const USAGE: &str = "takt check|sim|mir|fmt|parse|tokens DATEI… (siehe crates/takt-cli/src/main.rs)";

struct Args {
    flags: Vec<String>,
    files: Vec<String>,
    /// Werte, die als eigenes Argument folgten (`--ticks 20`).
    values: Vec<(String, String)>,
}

impl Args {
    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }

    /// Wert eines Schalters, als `--flag=wert` oder `--flag wert`.
    fn value(&self, flag: &str) -> Option<&str> {
        self.flags
            .iter()
            .find_map(|f| f.strip_prefix(flag).and_then(|r| r.strip_prefix('=')))
            .or_else(|| self.values.iter().find(|(f, _)| f == flag).map(|(_, v)| v.as_str()))
    }

    /// Zerlegt die Argumentliste: Schalter, ihre Werte und Dateien.
    fn parse(rest: &[String]) -> Args {
        const WITH_VALUE: &[&str] =
            &["--ticks", "--stim", "--golden", "--trace", "--profile", "--order", "--build", "--format", "--write"];
        let mut args = Args { flags: Vec::new(), files: Vec::new(), values: Vec::new() };
        let mut i = 0;
        while i < rest.len() {
            let a = &rest[i];
            if let Some(name) = a.strip_prefix("--") {
                let name = format!("--{}", name.split('=').next().unwrap_or_default());
                args.flags.push(a.clone());
                if !a.contains('=') && WITH_VALUE.contains(&name.as_str()) && i + 1 < rest.len() {
                    args.values.push((name, rest[i + 1].clone()));
                    i += 2;
                    continue;
                }
            } else {
                args.files.push(a.clone());
            }
            i += 1;
        }
        args
    }
}

fn main() -> ExitCode {
    let mut argv = std::env::args().skip(1);
    let Some(command) = argv.next() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let rest: Vec<String> = argv.collect();
    let args = Args::parse(&rest);
    let ok = match command.as_str() {
        "check" => check(&args),
        "sim" => sim(&args),
        "mir" => mir(&args),
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
        let options = takt_sema::Options { policy, build: build_of(args), profile: profile_of(args) };
        let checked = takt_sema::compile(&src, &options);
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

/// Build aus `--build sim|hw` (Default `sim`).
fn build_of(args: &Args) -> takt_sema::Build {
    match args.value("--build") {
        Some("hw") => takt_sema::Build::Hw,
        _ => takt_sema::Build::Sim,
    }
}

/// Profil aus `--profile` (8.4).
fn profile_of(args: &Args) -> Option<String> {
    args.value("--profile").map(str::to_string)
}

/// Uebersetzt eine Datei und meldet die Diagnosen; `None` bei Fehlern.
fn compile_file(path: &str, args: &Args) -> Option<takt_mir::Program> {
    let src = read(path)?;
    let map = SourceMap::single(path, src.as_str());
    let options = takt_sema::Options { policy: Policy::default(), build: build_of(args), profile: profile_of(args) };
    let out = takt_sema::compile(&src, &options);
    for d in &out.diagnostics {
        eprintln!("{}", map.render(d));
    }
    if out.has_errors() { None } else { out.program }
}

/// `takt sim`: fuehrt ein Programm mit einem Stimulus aus, schreibt den Trace
/// und vergleicht ihn mit einem Golden-Trace. Der Exit-Code folgt dem
/// Lauf-Verdikt (13.5) und dem Vergleich.
fn sim(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    let ticks = match args.value("--ticks").map(str::parse::<u64>) {
        Some(Ok(n)) => n,
        Some(Err(e)) => {
            eprintln!("--ticks: {e}");
            return false;
        }
        None => {
            eprintln!("--ticks fehlt");
            return false;
        }
    };
    let stimulus = match args.value("--stim") {
        Some(p) => {
            let Some(text) = read(p) else { return false };
            match Trace::parse(&text) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{p}: {e}");
                    return false;
                }
            }
        }
        None => Trace::default(),
    };
    let order_seed = match args.value("--order").and_then(|v| v.strip_prefix("random:")) {
        Some(seed) => match seed.parse::<u64>() {
            Ok(n) => Some(n),
            Err(e) => {
                eprintln!("--order: {e}");
                return false;
            }
        },
        None => None,
    };
    let options = RunOptions { ticks, profile: profile_of(args), order_seed };
    let result = match takt_interp::run(&program, &stimulus, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    let text = result.trace.render();
    let mut ok = result.verdict != Verdict::Fail;
    if let Some(out) = args.value("--trace") {
        if let Err(e) = std::fs::write(out, &text) {
            eprintln!("{out}: {e}");
            return false;
        }
    }
    match args.value("--golden") {
        Some(golden) => {
            let Some(expected) = read(golden) else { return false };
            if text != expected {
                print!("{}", diff(&expected, &text));
                eprintln!("{golden}: Trace weicht ab");
                ok = false;
            }
        }
        None => {
            if args.value("--trace").is_none() {
                print!("{text}");
            }
        }
    }
    println!("{path}: {} nach {ticks} Ticks", result.verdict.name());
    ok
}

/// Zeilenweiser Unterschied zweier Traces (`-` erwartet, `+` erhalten).
fn diff(expected: &str, actual: &str) -> String {
    let want: Vec<&str> = expected.lines().collect();
    let got: Vec<&str> = actual.lines().collect();
    let mut out = String::new();
    for i in 0..want.len().max(got.len()) {
        match (want.get(i), got.get(i)) {
            (Some(a), Some(b)) if a == b => {}
            (a, b) => {
                if let Some(a) = a {
                    out.push_str(&format!("-{a}\n"));
                }
                if let Some(b) = b {
                    out.push_str(&format!("+{b}\n"));
                }
            }
        }
    }
    out
}

/// `takt mir`: Textdump je Maschine, Datei im Format TAKT-MIR oder Logik-Hash.
fn mir(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    if let Some(out) = args.value("--write") {
        let bytes = takt_mir::format::write_program(&program, env!("CARGO_PKG_VERSION"));
        if let Err(e) = std::fs::write(out, &bytes) {
            eprintln!("{out}: {e}");
            return false;
        }
        println!("{out}: {} Bytes", bytes.len());
    }
    if args.has("--hash") {
        let hash = takt_mir::hash::logic_hash(&program);
        println!("{hash}");
    }
    if args.has("--dump") || (args.value("--write").is_none() && !args.has("--hash")) {
        for i in 0..program.machines.len() {
            print!("{}", takt_mir::dump::dump_machine(&program, takt_mir::MachineId(i as u32)));
        }
    }
    true
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
