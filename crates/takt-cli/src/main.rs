//! `takt`: Kommandozeile fuer `check`, `build`, `sim`, `size`, `cost`,
//! `latency`, `mir`, `fmt`, `parse` und `tokens`.
//!
//! ```text
//! takt check DATEI… [--warnings-as-errors] [--certification] [--format text|line]
//!                   [--hardware DATEI.hw --target NAME]
//!                   [--build sim|hw] [--params-profile P]
//! takt sim   DATEI --ticks N [--stim S.trace] [--golden G.trace] [--trace OUT.trace]
//!                   [--params-profile P] [--order random:SEED] [--steps OUT.steps]
//! takt test  DATEI [--ticks N] [--params-profile P] [--scenario NAME] [--coverage OUT.csv] [--out DIR]
//! takt campaign DATEI [NAME] --ticks N [--stim S.trace] [--params-profile P] [--scenario NAME]
//!                   [--out DIR] [--hardware DATEI.hw]
//! takt tune  DATEI --ticks N --save PROFIL [--stim S.trace] [--params-profile P] [--out DATEI]
//! takt run   DATEI --ticks N [--stim S.trace] [--record R.trace] [--trace OUT.trace] [--params-profile P]
//! takt replay DATEI --record R.trace [--golden G.trace] [--ticks N]
//!                   [--machine M [--extract SCHEIBE.trace]]
//! takt verify-trace TRACE.trace --record R.trace
//! takt timing TRACE.trace --tick NS
//! takt build DATEI [--target x86_64|aarch64|thumbv7em|riscv32imac]
//!                   [--emit ir|obj|consts|consts-rs] [--out PFAD] [--hardware DATEI.hw]
//!                   [--instrument statements|states|off] [--diagnostics ids|none]
//! takt size  DATEI… [--build sim|hw] [--params-profile P] [--object DATEI.o] [--target NAME]
//!                   [--hardware DATEI.hw] [--baseline DATEI] [--save-baseline DATEI]
//! takt cost  DATEI… [--build sim|hw] [--params-profile P]
//! takt latency DATEI… [--build sim|hw] [--params-profile P]
//! takt mir   DATEI [--dump] [--write OUT.mir] [--hash]
//! takt fmt   DATEI… [--check] [--stdout] [--snippet] [--verify] [--edition]
//! takt parse DATEI… [--ast] [--debug] [--snippet]
//! takt tokens DATEI
//! takt tcb review DATEI --native NAME --by NAME --date JJJJ-MM-TT [--review DATEI]
//! ```
//! Exit-Code 1 bei Fehlern, nicht kanonischen Dateien (`fmt --check`), einem
//! Golden-Unterschied oder dem Lauf-Verdikt FAIL (13.5).

use std::collections::BTreeMap;
use std::process::ExitCode;

use std::fmt::Write as _;

use takt_diag::{Policy, SourceMap};
use takt_interp::{RunOptions, Trace, Verdict};
use takt_syntax::fmt::{insert_edition, verify};
use takt_syntax::{Edition, TokenKind, format, format_snippet, parse_file, parse_snippet, sexpr, tokenize};

const USAGE: &str = "takt check|build|sim|run|replay|verify-trace|timing|test|campaign|prove|tune|size|cost|latency|graph|mir|fmt|parse|tokens|tcb DATEI… (siehe crates/takt-cli/src/main.rs)";

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
        const WITH_VALUE: &[&str] = &[
            "--ticks",
            "--native",
            "--by",
            "--date",
            "--review",
            "--stim",
            "--golden",
            "--trace",
            "--profile",
            "--order",
            "--build",
            "--format",
            "--write",
            "--record",
            "--target",
            "--emit",
            "--out",
            "--object",
            "--hardware",
            "--baseline",
            "--save-baseline",
            "--scenario",
            "--export",
            "--depth",
            "--solver",
            "--timeout",
            "--save",
            "--proof",
            "--save-proof",
            "--coverage",
            "--machine",
            "--extract",
            "--instrument",
            "--diagnostics",
            "--steps",
            "--tick",
            "--params-profile",
        ];
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
        "test" => test(&args),
        "campaign" => campaign(&args),
        "prove" => prove(&args),
        "tune" => tune(&args),
        "run" => run_cmd(&args),
        "replay" => replay(&args),
        "verify-trace" => verify_trace(&args),
        "timing" => timing(&args),
        "mir" => mir(&args),
        "fmt" => fmt(&args),
        "build" => build(&args),
        "size" => size(&args),
        "cost" => cost(&args),
        "latency" => latency(&args),
        "graph" => graph(&args),
        "parse" => parse(&args),
        "tokens" => tokens(&args),
        "tcb" => tcb(&args),
        _ => {
            eprintln!("{USAGE}");
            false
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Pfad der Review-Datei; `--review` oder `natives.review` neben dem Programm.
fn review_path(args: &Args, program: &str) -> std::path::PathBuf {
    match args.value("--review") {
        Some(p) => std::path::PathBuf::from(p),
        None => std::path::Path::new(program).with_file_name("natives.review"),
    }
}

/// Die Review-Datei, falls sie da ist; eine leere sonst.
fn review_of(path: &std::path::Path) -> Option<takt_mir::review::Review> {
    let Ok(text) = std::fs::read_to_string(path) else { return Some(takt_mir::review::Review::default()) };
    match takt_mir::review::parse(&text) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("{}:{}: {}", path.display(), e.line, e.message);
            None
        }
    }
}

/// Die Quelle eines Natives, relativ zum Programm gesucht.
fn native_source(program: &str, from: &str) -> Option<Vec<u8>> {
    let beside = std::path::Path::new(program).with_file_name(from);
    std::fs::read(&beside).or_else(|_| std::fs::read(from)).ok()
}

/// `takt tcb review DATEI --native NAME --by NAME` (4.5, v1.2).
///
/// Schreibt die Zeile, nachdem jemand hingesehen hat. Das Werkzeug
/// bezeugt nichts — es haelt fest, was der Pruefer sagt, und der Commit
/// sagt, wer es war.
fn tcb(args: &Args) -> bool {
    if args.files.first().map(String::as_str) != Some("review") {
        eprintln!("takt tcb review DATEI --native NAME --by NAME --date JJJJ-MM-TT [--review DATEI]");
        return false;
    }
    let Some(path) = args.files.get(1) else {
        eprintln!("takt tcb review: kein Programm genannt");
        return false;
    };
    // Das Datum steht im Aufruf und kommt nicht aus der Systemuhr: Ein
    // Werkzeug, das dieselbe Eingabe zweimal verschieden beantwortet,
    // passt nicht zu reproduzierbaren Builds (11.3).
    let (Some(native), Some(by), Some(date)) = (args.value("--native"), args.value("--by"), args.value("--date"))
    else {
        eprintln!("takt tcb review: `--native NAME`, `--by NAME` und `--date JJJJ-MM-TT` sind Pflicht");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    let Some(def) = program.natives.iter().find(|n| n.name == native && n.from.is_some()) else {
        eprintln!("{path}: kein Projekt-Native `{native}`");
        return false;
    };
    let from = def.from.as_deref().unwrap_or_default();
    let Some(source) = native_source(path, from) else {
        eprintln!("{path}: `{from}` ist nicht lesbar");
        return false;
    };
    let file = review_path(args, path);
    let Some(mut review) = review_of(&file) else { return false };
    review.insert(takt_mir::review::Entry {
        native: native.to_string(),
        hash: takt_mir::review::hash_of(&source),
        by: by.to_string(),
        date: date.to_string(),
    });
    if let Err(e) = std::fs::write(&file, review.render()) {
        eprintln!("{}: {e}", file.display());
        return false;
    }
    println!("{}: `{native}` geprueft von {by}", file.display());
    true
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
        let Some(proof) = proof_of(args) else {
            ok = false;
            continue;
        };
        let checked = takt_sema::compile_with(&src, &options, proof.as_ref());
        // Mit Kalibrierung urteilt SC-12 gleich (siehe unten); sein
        // Hinweis „noch nicht entscheidbar" waere daneben ein Widerspruch.
        let kalibriert = calibration(args);
        for d in checked.diagnostics.iter().filter(|d| !(kalibriert.is_some() && d.code == "SC-12")) {
            if line_format {
                println!("{}", map.render_line(d));
            } else {
                println!("{}", map.render(d));
            }
        }
        // Pruefung 12, 32 und 39 brauchen das Ziel, 28, 59 und 60 die Kanaele
        // der Konfiguration (8.10); ohne sie bleibt es beim Hinweis.
        if let Some(program) = &checked.program {
            let span = takt_diag::Span::new(0, 0);
            let mut diags = Vec::new();
            if let Some(target) = &kalibriert {
                diags.extend(takt_sema::calibrated::check(program, target, span));
            }
            if let Some(hw) = hardware(args) {
                diags.extend(takt_sema::calibrated::check_bindings(program, &hw));
                diags.extend(takt_sema::calibrated::polling(program, &hw, kalibriert.as_ref()));
            }
            // 4.5: `tcb_policy = reviewed(…)` verlangt je Projekt-Native
            // eine Zeile in `natives.review`.
            if program.config.tcb_reviewed {
                match review_of(&review_path(args, path)) {
                    Some(review) => diags
                        .extend(takt_sema::calibrated::reviewed(program, &review, &|from| native_source(path, from))),
                    None => ok = false,
                }
            }
            for d in diags {
                println!("{}", if line_format { map.render_line(&d) } else { map.render(&d) });
                if d.is_error() {
                    ok = false;
                }
            }
        }
        if checked.has_errors() {
            ok = false;
        }
        // Kennzahlen des statischen Gates (3.4; plan/m3.md 5).
        if args.has("--report") && !checked.has_errors() {
            for line in checked.report.lines() {
                println!("  {line}");
            }
            // `--checks`: jede verbliebene Pruefung mit Stelle (3.4).
            if args.has("--checks") {
                let mut sites = checked.report.sites.clone();
                sites.sort_by_key(|c| (c.span.file.0, c.span.start));
                for c in sites {
                    let (line, col) = map.line_col(c.span);
                    let note = if c.warns { " (Schleife oder Aktionsblock)" } else { "" };
                    let kind = takt_mir::analysis::walk::name_of_tag(c.tag);
                    println!("  Pruefung {} {kind} {line}:{col} @{}{note}", c.cause.name(), c.span.start);
                }
            }
            if let Some(program) = &checked.program {
                for line in requirement_lines(&takt_mir::requirements::index(program), &map, None) {
                    println!("  {line}");
                }
            }
        }
    }
    ok
}

/// Der Abschnitt „Anforderungen" des Reports (13.4): je `req`-ID ihre
/// pruefenden Stellen. `by_scenario` nennt je Stelle die Szenarien, die
/// sie durchliefen; ohne sie bleibt es bei der Stelle.
fn requirement_lines(
    index: &takt_mir::requirements::Index,
    map: &SourceMap,
    by_scenario: Option<&BTreeMap<(String, String), Vec<String>>>,
) -> Vec<String> {
    if index.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!("Anforderungen:        {} (13.4)", index.len())];
    for (req, sites) in &index.by_req {
        out.push(format!("  {req}"));
        for s in sites {
            let (line, _) = map.line_col(s.span);
            let mut text =
                format!("    {} {}:{line} {} {}", s.kind.name(), map.name(s.span.file), s.machine_name, s.block);
            if let Some(by) = by_scenario {
                let (machine, key) = s.cover_key();
                let names = by.get(&(machine.to_string(), key)).map(Vec::as_slice).unwrap_or(&[]);
                let _ = write!(text, " — {}", if names.is_empty() { "kein Szenario".into() } else { names.join(", ") });
            }
            out.push(text);
        }
    }
    out
}

/// `takt latency`: Safe-State-Latenz je Output (9.4.5).
fn latency(args: &Args) -> bool {
    let policy =
        Policy { warnings_as_errors: args.has("--warnings-as-errors"), certification: args.has("--certification") };
    let mut ok = true;
    for path in &args.files {
        let Some(src) = read(path) else {
            ok = false;
            continue;
        };
        let map = SourceMap::single(path.as_str(), src.as_str());
        let options = takt_sema::Options { policy, build: build_of(args), profile: profile_of(args) };
        let checked = takt_sema::compile(&src, &options);
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("{}", map.render(d));
        }
        let Some(program) = &checked.program else {
            ok = false;
            continue;
        };
        println!("{path}:");
        for line in takt_mir::analysis::latency::latency(program).lines(program) {
            println!("{line}");
        }
    }
    ok
}

/// `takt graph`: Wer schreibt worauf, wer liest von wem (11.1).
///
/// Die Antworten stehen in der MIR verstreut; das Kommando sammelt sie.
/// Ausgegeben wird Text, damit die Ausgabe in einen Bericht passt und
/// sich mit `diff` vergleichen laesst.
fn graph(args: &Args) -> bool {
    let policy =
        Policy { warnings_as_errors: args.has("--warnings-as-errors"), certification: args.has("--certification") };
    let mut ok = true;
    for path in &args.files {
        let Some(src) = read(path) else {
            ok = false;
            continue;
        };
        let map = SourceMap::single(path.as_str(), src.as_str());
        let options = takt_sema::Options { policy, build: build_of(args), profile: profile_of(args) };
        let checked = takt_sema::compile(&src, &options);
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("{}", map.render(d));
        }
        let Some(program) = &checked.program else {
            ok = false;
            continue;
        };
        println!("{path}:");
        for line in takt_mir::analysis::graph::graph(program).lines(program) {
            println!("{line}");
        }
    }
    ok
}

/// `takt build`: ein Programm fuer ein Ziel uebersetzen (11.2, 12.8).
///
/// ```text
/// takt build DATEI [--target NAME] [--emit ir|obj|consts|consts-rs] [--out PFAD]
/// ```
///
/// **Warum es dieses Kommando gibt.** Bis M5 endete jeder Weg aus einer
/// `.takt`-Datei entweder im Interpreter (`takt sim`) oder in einem Lauf
/// auf dem Wirt (`takt run`). Fuer ein fremdes Ziel gab es nichts — das
/// erste Bring-up-Programm behalf sich mit einer `build.rs`, die Sema,
/// Lowering, clang und llvm-ar hintereinander rief. Was dort steht, ist
/// nicht aufrufbar, meldet Fehler als Warnung und ist unauffindbar
/// (FB-138).
///
/// **Was es tut und was nicht.** Es uebersetzt und assembliert; es
/// *bindet nicht*. Ein fertiges Binary braucht Startcode, Linker-Skript
/// und Speicherkarte, und die gehoeren zum Board, nicht zum Compiler:
/// Dieselbe `.o`-Datei laeuft auf jedem F401, aber jedes Board hat seinen
/// eigenen Flash-Versatz. Wer bindet, weiss das; wer uebersetzt, muss es
/// nicht wissen.
///
/// **Der Rahmen (12.1) gehoert nicht hierher.** Das C-Stueck, das
/// Prozessabbild und Latch haelt und `<maschine>_step` ruft, steht in
/// `takt-conformance::mcu` — zusammen mit der Speicherform, die es
/// braucht. Ihn hier zu erzeugen hiesse, die halbe Abnahmesuite in das
/// CLI zu ziehen; wer ihn braucht, erzeugt ihn dort. Das ist die
/// unbequemere, aber ehrlichere Trennung: `takt build` uebersetzt ein
/// Programm, es baut keine Runtime.
fn build(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let target_name = args.value("--target").unwrap_or("x86_64");
    let Some(target) = takt_llvm::Target::by_name(target_name) else {
        eprintln!("Unbekanntes Ziel `{target_name}`. Bekannt: x86_64, aarch64, thumbv7em, riscv32imac");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };

    let instrument = match args.value("--instrument") {
        Some(name) => match takt_llvm::Instrument::parse(name) {
            Some(i) => i,
            None => {
                eprintln!("--instrument: `{name}` unbekannt; statements, states oder off (11.2)");
                return false;
            }
        },
        None => takt_llvm::Instrument::default_for(program.config.runtime_profile(), target),
    };
    println!("Instrumentierung: {} (11.2)", instrument.name());
    let diagnostics = match args.value("--diagnostics") {
        Some(name) => match takt_llvm::Diagnostics::parse(name) {
            Some(d) => d,
            None => {
                eprintln!("--diagnostics: `{name}` unbekannt; ids oder none (12.3)");
                return false;
            }
        },
        None => takt_llvm::Diagnostics::Ids,
    };
    let lowered =
        takt_llvm::lower::program_with_diagnostics(&program, target.triple, module_name(path), instrument, diagnostics);
    for s in &lowered.skipped {
        // Ein fehlender Schritt ist ein Loch, kein Schoenheitsfehler: Ohne
        // ihn meldet der Linker spaeter ein unbekanntes Symbol statt des
        // Konstrukts, das gefehlt hat (FB-104).
        eprintln!("{}_step fehlt: {}", s.machine, s.reason);
    }
    for name in &lowered.without_persist {
        // Der Code laeuft, aber ohne Lesepfad startet jeder Lauf beim
        // Default — ein Typ, den der Codegen noch nicht abbildet (5.9).
        eprintln!("{name}: `persist var` ohne Lesepfad im erzeugten Code — jeder Start beginnt beim Default (5.9)");
    }

    let emit = args.value("--emit").unwrap_or("obj");
    let stem = std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("programm");

    match emit {
        "ir" => {
            let out = args.value("--out").map_or_else(|| format!("{stem}.ll"), str::to_string);
            match std::fs::write(&out, &lowered.ir) {
                Ok(()) => {
                    println!("{out}: {} Byte IR fuer {}", lowered.ir.len(), target.name);
                    lowered.complete()
                }
                Err(e) => {
                    eprintln!("{out}: {e}");
                    false
                }
            }
        }
        "obj" => {
            let out = args.value("--out").map_or_else(|| format!("{stem}.o"), str::to_string);
            emit_object(&lowered.ir, target, &out) && lowered.complete()
        }
        "consts" | "consts-rs" => {
            let rust = emit == "consts-rs";
            let ext = if rust { "rs" } else { "h" };
            let out = args.value("--out").map_or_else(|| format!("{stem}.{ext}"), str::to_string);
            let hw = hardware(args);
            // 12.3: So lange haelt ein Journal-Vorgang den Kern hoechstens; 0 heisst nie.
            let nvm_blocking_ns =
                hw.as_ref().and_then(|_| calibration(args)).and_then(|t| t.nvm?.blocking_phase_ns()).unwrap_or(0);
            let text = if rust {
                constants_rust(&program, hw.as_ref(), nvm_blocking_ns)
            } else {
                constants_header(&program, stem, hw.as_ref(), nvm_blocking_ns)
            };
            match std::fs::write(&out, &text) {
                Ok(()) => {
                    println!("{out}: Konstanten des Programms");
                    true
                }
                Err(e) => {
                    eprintln!("{out}: {e}");
                    false
                }
            }
        }
        other => {
            eprintln!("Unbekannte Ausgabeart `{other}`. Bekannt: ir, obj, consts, consts-rs");
            false
        }
    }
}

/// Die Konstanten eines Programms als C-Header.
///
/// **Wogegen das hilft.** Die Tickperiode steht in `system: tick` und
/// muss der Runtime bekannt sein — beim ersten Hardwarelauf stand sie an
/// zwei Stellen, im Programm mit 10 ms und im Bring-up mit 1 ms. Die
/// logische Zeit lief zehnfach zu schnell, und jedes `after` haette zu
/// frueh gefeuert. Niemand hat es gemeldet, weil niemand beides kannte.
///
/// Ein Header statt einer Rust-Datei, weil der Rahmen (12.1) ohnehin C
/// ist: So liest ihn beides, und die Zahl steht einmal.
fn constants_header(
    p: &takt_mir::Program,
    stem: &str,
    hw: Option<&takt_mir::hardware::Hardware>,
    nvm_blocking_ns: i64,
) -> String {
    let guard = stem.to_uppercase().replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let mut s = String::new();
    s.push_str("/* Konstanten des Programms; erzeugt von `takt build --emit consts`. */\n");
    s.push_str(&format!("#ifndef TAKT_{guard}_H\n#define TAKT_{guard}_H\n\n"));
    s.push_str("/* Basis-Tick T0 in Nanosekunden (`system: tick`, 7.1). */\n");
    s.push_str(&format!("#define TAKT_TICK_NS {}LL\n\n", p.config.tick));
    s.push_str("/* So lange haelt ein Journal-Vorgang den Kern hoechstens, in ns (12.3); 0 heisst nie. */\n");
    s.push_str(&format!("#define TAKT_NVM_BLOCKING_NS {nvm_blocking_ns}LL\n\n"));
    s.push_str("/* `system: overrun` (7.3): 1 heisst `alert`, 0 heisst `fault`. */\n");
    s.push_str(&format!(
        "#define TAKT_OVERRUN_ALERT {}\n\n",
        u8::from(p.config.overrun == takt_mir::program::OverrunPolicy::Alert)
    ));
    s.push_str("/* Die Maschinen, in Deklarationsreihenfolge (9.4). */\n");
    for m in p.machines.iter().filter(|m| m.kind != takt_mir::machine::MachineKind::Template) {
        s.push_str(&format!("/*   {} — jeder {}. Tick */\n", m.name, m.period));
    }
    s.push_str(&format!("#define TAKT_MACHINES {}\n\n", p.machines.len()));
    if let Some(hw) = hw {
        s.push_str("/* Anschluesse aus der Hardware-Konfiguration (8.10); undurchsichtig, fuer das Board. */\n");
        for (ident, port) in ports(p, hw) {
            s.push_str(&format!("#define TAKT_PORT_{} \"{port}\"\n", ident.to_uppercase()));
        }
        s.push('\n');
    }
    s.push_str("#endif\n");
    s
}

/// Adresse → Anschluss fuer jeden `hw`-Kanal, den die Konfiguration kennt.
fn ports(p: &takt_mir::Program, hw: &takt_mir::hardware::Hardware) -> Vec<(String, String)> {
    p.channels
        .iter()
        .filter_map(|c| match &c.binding {
            takt_mir::program::Binding::Hw(a) => Some((a.ident(), hw.channel(&a.text())?.port.clone()?)),
            _ => None,
        })
        .collect()
}

/// Dieselben Konstanten als Rust, fuer die Bring-up-Seite.
///
/// **Warum zweimal dasselbe.** Der Rahmen (12.1) ist C, die Tickschleife
/// ist Rust, und beide brauchen die Tickperiode. Sie an beiden Stellen von
/// Hand zu schreiben war schon einmal falsch: Das Programm sagte 10 ms,
/// das Bring-up 1 ms, und die logische Zeit lief zehnfach zu schnell.
///
/// Dazu die Stellung der Ausgaenge im Latch. Wer `takt_mcu_output` ruft,
/// braucht einen Index, und ein handgeschriebener Index ist dieselbe
/// Fehlerquelle in kleiner: Er stimmt, bis jemand einen Ausgang davor
/// einfuegt.
fn constants_rust(p: &takt_mir::Program, hw: Option<&takt_mir::hardware::Hardware>, nvm_blocking_ns: i64) -> String {
    let mut s = String::new();
    // Regulaere Kommentare, keine `//!`: Die Datei wird per `include!` in
    // ein Modul gezogen, und dort darf kein innerer Doc-Kommentar stehen.
    s.push_str("// Konstanten des Programms; erzeugt von `takt build --emit consts-rs`.\n");
    s.push_str("// Nicht von Hand aendern — die Quelle ist die `.takt`-Datei.\n\n");
    s.push_str("/// Basis-Tick T0 in Nanosekunden (`system: tick`, 7.1).\n");
    s.push_str(&format!("pub const TICK_NS: i64 = {};\n\n", p.config.tick));
    s.push_str("/// So lange haelt ein Journal-Vorgang den Kern hoechstens, in ns (12.3); 0 heisst nie.\n");
    s.push_str(&format!("pub const NVM_BLOCKING_NS: i64 = {nvm_blocking_ns};\n\n"));
    s.push_str("/// `system: overrun = alert` (7.3); sonst `fault`.\n");
    s.push_str(&format!(
        "pub const OVERRUN_ALERT: bool = {};\n\n",
        p.config.overrun == takt_mir::program::OverrunPolicy::Alert
    ));

    s.push_str("/// Die Ausgaenge in der Reihenfolge, die `takt_mcu_output` erwartet.\n");
    let mut index = 0;
    for (i, c) in p.channels.iter().enumerate() {
        let id = takt_mir::ChannelId(i as u32);
        if c.dir == takt_mir::program::Direction::Input {
            continue;
        }
        if takt_llvm::image::latch_offset(id, p).is_none() || takt_llvm::ty::lower(c.ty, p).is_none() {
            continue;
        }
        let name = c.name.to_uppercase().replace(|ch: char| !ch.is_ascii_alphanumeric(), "_");
        s.push_str(&format!("pub const OUT_{name}: i32 = {index};\n"));
        index += 1;
    }
    s.push_str(&format!("\n/// Wie viele Ausgaenge das Programm hat.\npub const OUTPUTS: i32 = {index};\n"));
    // 5.9, 12.3: Das Journal traegt den Logik-Hash als Schluessel; die Runtime
    // braucht dazu die Nutzlastgrenze und das engste `min_interval`.
    s.push_str("\n/// Die ersten acht Byte des Logik-Hashes (9.4.4), Schluessel des `persist`-Journals (5.9).\n");
    let hash = takt_mir::hash::logic_hash(p);
    let key = u64::from_le_bytes(hash.0[..8].try_into().expect("acht Byte"));
    s.push_str(&format!("pub const LOGIC_HASH: u64 = {key:#018x};\n"));
    s.push_str("\n/// Hoechstlaenge der `persist`-Nutzlast in Byte; 0 ohne `persist`.\n");
    s.push_str(&format!("pub const PERSIST_BOUND: usize = {};\n", takt_mir::persist::max_payload(p).unwrap_or(0)));
    s.push_str("\n/// Das engste `min_interval` der `persist`-Variablen in Nanosekunden; 0 ohne Angabe.\n");
    s.push_str(&format!(
        "pub const PERSIST_MIN_INTERVAL_NS: i64 = {};\n",
        takt_mir::persist::min_interval_ns(p).unwrap_or(0)
    ));
    if let Some(hw) = hw {
        for (ident, port) in ports(p, hw) {
            s.push_str("\n/// Anschluss aus der Hardware-Konfiguration (8.10); undurchsichtig, fuer das Board.\n");
            s.push_str(&format!("pub const PORT_{}: &str = \"{port}\";\n", ident.to_uppercase()));
        }
    }
    s
}

/// Der Modulname in der IR: der Dateiname ohne Endung.
fn module_name(path: &str) -> &str {
    std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("programm")
}

/// Assembliert IR zu einem Objekt.
fn emit_object(ir: &str, target: takt_llvm::Target, out: &str) -> bool {
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        eprintln!("clang fehlt — ohne ihn gibt es nur `--emit ir`");
        return false;
    };
    let tmp = std::env::temp_dir().join(format!("takt-build-{}.ll", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, ir) {
        eprintln!("{}: {e}", tmp.display());
        return false;
    }

    let mut cmd = std::process::Command::new(&clang);
    let cmd = takt_llvm::toolchain::Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-c"])
        .args(takt_llvm::toolchain::object_flags(target.triple))
        .arg(format!("--target={}", target.triple));
    // Freistehend nur fuer die MCU: Auf dem Wirt gibt es eine libc, und
    // `-nostdlib` naehme sie dem Objekt ohne Not.
    if target.is_bare_metal() {
        cmd.args(["-ffreestanding", "-nostdlib"]);
    }
    if !target.march.is_empty() {
        cmd.arg(format!("-march={}", target.march));
    }
    let result = cmd.arg(&tmp).arg("-o").arg(out).output();
    let _ = std::fs::remove_file(&tmp);

    match result {
        Ok(o) if o.status.success() => {
            let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            println!("{out}: {size} Byte fuer {} ({})", target.name, target.class.name());
            true
        }
        Ok(o) => {
            eprintln!("{}", String::from_utf8_lossy(&o.stderr));
            false
        }
        Err(e) => {
            eprintln!("clang: {e}");
            false
        }
    }
}

/// `takt cost`: das Kostenbudget je Maschine und Zustand (9.4.3, 7.2).
///
/// Das Gegenstueck zu `takt size`: Dort Bytes, hier Operationen. Beide
/// sagen, was ihnen fehlt — `size` ueber die Herkunft je Posten (11.5),
/// `cost` ueber die Kalibrierung, ohne die aus Operationen keine Zeit
/// wird (13.8).
fn cost(args: &Args) -> bool {
    let policy =
        Policy { warnings_as_errors: args.has("--warnings-as-errors"), certification: args.has("--certification") };
    let mut ok = true;
    for path in &args.files {
        let Some(src) = read(path) else {
            ok = false;
            continue;
        };
        let map = SourceMap::single(path.as_str(), src.as_str());
        let options = takt_sema::Options { policy, build: build_of(args), profile: profile_of(args) };
        let checked = takt_sema::compile(&src, &options);
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("{}", map.render(d));
        }
        let Some(program) = &checked.program else {
            ok = false;
            continue;
        };
        println!("{path}:");
        for line in takt_mir::analysis::budget::report(program).lines() {
            println!("{line}");
        }
    }
    ok
}

/// `takt size`: das Speicherbudget eines Programms (11.5).
fn size(args: &Args) -> bool {
    let policy =
        Policy { warnings_as_errors: args.has("--warnings-as-errors"), certification: args.has("--certification") };
    let mut ok = true;
    for path in &args.files {
        let Some(src) = read(path) else {
            ok = false;
            continue;
        };
        let map = SourceMap::single(path.as_str(), src.as_str());
        let options = takt_sema::Options { policy, build: build_of(args), profile: profile_of(args) };
        let checked = takt_sema::compile(&src, &options);
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("{}", map.render(d));
        }
        let Some(program) = &checked.program else {
            ok = false;
            continue;
        };
        println!("{path}:");
        let (measured, residency) = measure(args, program);
        let mut report = takt_mir::analysis::size::size(program).with_object(&measured);
        let target = calibration(args);
        if let Some(target) = &target {
            report = report.with_hardware(target);
        }
        for line in report.lines() {
            println!("{line}");
        }
        // 12.3: Auf einem XIP-Ziel zaehlt, was vom Programm im RAM liegt;
        // mit asynchronem NVM muss es alles sein, sonst steht der Tick.
        if let Some(r) = residency.filter(|_| report.iram_total() > 0) {
            println!("  RAM-Residenz des Programms: {} von {} Symbolen", r.ram.len(), r.ram.len() + r.flash.len());
            let asynchronous = target.as_ref().is_some_and(|t| t.nvm.and_then(|n| n.blocking) == Some(false));
            if asynchronous && !r.flash.is_empty() {
                println!("  im Flash, bei asynchronem NVM ein Stillstand im Tick (12.3): {}", r.flash.join(", "));
                ok = false;
            }
        }
        if let Some(target) = target {
            // Pruefung 39: die Summen gegen das Ziel (11.5); IRAM nur auf
            // XIP-Zielen, wo beide Seiten es kennen (12.3).
            //
            // Teilen sich Daten und RAM-residenter Code denselben Bereich
            // (`ram == iram`, wie auf dem ESP32-C6), zaehlt die Summe
            // beider gegen die eine Grenze — getrennt geprueft passten
            // zwei Haelften, die zusammen nicht hineingehen.
            let m = target.memory;
            let iram = report.iram_total();
            let shared = iram > 0 && m.iram.is_some() && m.iram == m.ram;
            let limits = [
                (if shared { "RAM + IRAM" } else { "RAM" }, report.ram_total() + if shared { iram } else { 0 }, m.ram),
                ("Flash", report.flash_total(), m.flash),
                ("IRAM", iram, m.iram.filter(|_| iram > 0 && !shared)),
            ];
            for (what, have, limit) in limits {
                let Some(limit) = limit else { continue };
                let verdict = if have <= limit { "passt" } else { "zu viel" };
                println!("  {what}: {have} von {limit} Byte auf `{}` — {verdict}", target.name);
                if have > limit {
                    ok = false;
                }
            }
            if iram > 0 && m.iram.is_none() {
                println!("  IRAM: {iram} Byte gemessen, aber `iram` fehlt in der Konfiguration (8.10)");
            }
        }
        if let Some(file) = args.value("--save-baseline") {
            match std::fs::write(file, report.baseline()) {
                Ok(()) => println!("  Baseline geschrieben: {file}"),
                Err(e) => {
                    eprintln!("{file}: {e}");
                    ok = false;
                }
            }
        }
        if let Some(file) = args.value("--baseline") {
            ok &= against_baseline(&report, file);
        }
    }
    ok
}

/// `--baseline DATEI` (11.5, D1): die Posten gegen eine fruehere Rechnung.
/// Waechst RAM oder Flash, ist das ein Fehler — eine Budget-Regression
/// soll so sichtbar sein wie ein fehlgeschlagener Test.
fn against_baseline(report: &takt_mir::analysis::size::Size, file: &str) -> bool {
    let base = match std::fs::read_to_string(file).map_err(|e| e.to_string()) {
        Ok(text) => match takt_mir::analysis::size::Size::from_baseline(&text) {
            Ok(base) => base,
            Err(e) => {
                eprintln!("{file}: {e}");
                return false;
            }
        },
        Err(e) => {
            eprintln!("{file}: {e}");
            return false;
        }
    };
    let deltas = report.diff(&base);
    if deltas.is_empty() {
        println!("  Baseline {file}: keine Aenderung");
    }
    let show = |b: Option<u64>| b.map_or("-".to_string(), |b| b.to_string());
    for d in &deltas {
        println!("  {}: {} -> {} (Baseline {file})", d.name, show(d.before), show(d.after));
    }
    let mut ok = true;
    for (what, have, was) in
        [("RAM", report.ram_total(), base.ram_total()), ("Flash", report.flash_total(), base.flash_total())]
    {
        if have > was {
            println!("  {what}: {have} Byte, Baseline {was} — gewachsen");
            ok = false;
        }
    }
    ok
}

/// Was ein erzeugtes Objekt beisteuert (11.5, 12.3).
///
/// **Ohne `--object` bleibt es leer, und das ist kein Mangel.** Flash und
/// Stacktiefe entscheidet der Codegen, nicht die MIR; wer sie wissen will,
/// muss uebersetzt haben. `takt build --emit obj` liefert die Datei.
fn measure(
    args: &Args,
    p: &takt_mir::Program,
) -> (takt_mir::analysis::size::Measured, Option<takt_llvm::inspect::Residency>) {
    let mut out = takt_mir::analysis::size::Measured::default();
    let Some(file) = args.value("--object") else { return (out, None) };
    let path = std::path::Path::new(file);
    if !path.exists() {
        eprintln!("{file}: nicht gefunden; Flash und Stack bleiben offen");
        return (out, None);
    }

    // Ohne `--target` der Wirt: Ein Objekt ohne Angabe stammt meist aus
    // `takt build` ohne Ziel, und das uebersetzt fuer den Wirt.
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let target = args.value("--target").and_then(takt_llvm::Target::by_name).unwrap_or(host);
    let tools = takt_llvm::inspect::Binutils::best_for(target);

    if let Some(s) = tools.sections(path) {
        out.flash = Some(s.flash());
        out.iram_text = Some(s.iram_text);
        out.iram_rodata = Some(s.iram_rodata);
    }

    // Die Rahmen aller Funktionen in einem Durchlauf; die Rechnung
    // braucht sie vollstaendig, sonst meldet sie unbekannt.
    let symbols: Vec<String> = p.fns.iter().map(takt_llvm::fns::symbol).collect();
    let frames: Vec<Option<u32>> =
        tools.stack_frames(path, &symbols).into_iter().map(|f| f.and_then(|n| u32::try_from(n).ok())).collect();
    out.stack = takt_mir::analysis::stack::depth(p, &frames);

    let residency = tools
        .section_ranges(path)
        .zip(tools.symbols(path))
        .map(|(ranges, syms)| takt_llvm::inspect::residency(&syms, &ranges, |n| is_program_symbol(p, n)));
    (out, residency)
}

/// Symbole, die der Codegen und der Rahmen (12.1) fuer das Programm
/// erzeugen: Maschinen, Funktionen, Natives, `takt_mcu_*`.
fn is_program_symbol(p: &takt_mir::Program, name: &str) -> bool {
    ["takt_mcu_", "takt_fn_", "takt_native_"].iter().any(|pre| name.starts_with(pre))
        || p.machines.iter().any(|m| name.starts_with(&format!("{}_", takt_llvm::fns::sanitized(&m.name))))
}

/// Die Hardware-Konfiguration aus `--hardware DATEI` (8.10), gelesen und
/// geprueft; ein Lesefehler wird gemeldet und ergibt keine Konfiguration.
fn hardware(args: &Args) -> Option<takt_mir::hardware::Hardware> {
    let path = args.value("--hardware")?;
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}: {e}");
            return None;
        }
    };
    match takt_mir::hardware::parse(&text) {
        Ok(hw) => Some(hw),
        Err(e) => {
            eprintln!("{path}: {e}");
            None
        }
    }
}

/// Die Kalibrierung aus `--hardware DATEI --target NAME` (8.10, 13.8).
///
/// **Beides oder nichts.** Eine Konfiguration ohne Ziel liesse offen,
/// welche der Tabellen gilt, und ein Ziel ohne Konfiguration hat keine.
/// Fehlt der Schalter, ist das kein Fehler: Ein `takt check` ohne
/// Hardware-Konfiguration ist der Normalfall (Pruefung 60: „Fehlt die
/// Konfiguration, entfaellt die Pruefung").
fn calibration(args: &Args) -> Option<takt_mir::hardware::Target> {
    let path = args.value("--hardware")?;
    let hw = hardware(args)?;
    let name = args.value("--target").unwrap_or("x86_64");
    match hw.target(name) {
        Some(t) => Some(t.clone()),
        None => {
            let bekannt: Vec<&str> = hw.targets.keys().map(String::as_str).collect();
            eprintln!("{path}: kein Ziel `{name}`; enthalten: {}", bekannt.join(", "));
            None
        }
    }
}

/// Build aus `--build sim|hw` (Default `sim`).
fn build_of(args: &Args) -> takt_sema::Build {
    match args.value("--build") {
        Some("hw") => takt_sema::Build::Hw,
        _ => takt_sema::Build::Sim,
    }
}

/// Profil aus `--profile` (8.4).
/// `--params-profile P` (8.4); `--profile` bleibt als Alias, weil das
/// 12.8-Profil denselben Namen trug (plan/m6.md 2.13).
fn profile_of(args: &Args) -> Option<String> {
    if let Some(p) = args.value("--params-profile") {
        return Some(p.to_string());
    }
    let alias = args.value("--profile")?;
    eprintln!("--profile heisst jetzt --params-profile (8.4)");
    Some(alias.to_string())
}

/// Uebersetzt eine Datei und meldet die Diagnosen; `None` bei Fehlern.
/// Die Beweisdatei aus `--proof` (11.3); `None` heisst: nicht lesbar.
fn proof_of(args: &Args) -> Option<Option<takt_mir::analysis::proof::Proof>> {
    let Some(path) = args.value("--proof") else { return Some(None) };
    let text = read(path)?;
    match takt_mir::analysis::proof::parse(&text) {
        Ok(p) => Some(Some(p)),
        Err(e) => {
            eprintln!("{path}: {e}");
            None
        }
    }
}

fn compile_file(path: &str, args: &Args) -> Option<takt_mir::Program> {
    let src = read(path)?;
    let map = SourceMap::single(path, src.as_str());
    let options = takt_sema::Options { policy: Policy::default(), build: build_of(args), profile: profile_of(args) };
    let proof = proof_of(args)?;
    let out = takt_sema::compile_with(&src, &options, proof.as_ref());
    for d in &out.diagnostics {
        eprintln!("{}", map.render(d));
    }
    if out.has_errors() { None } else { out.program }
}

/// `takt sim`: fuehrt ein Programm mit einem Stimulus aus, schreibt den Trace
/// und vergleicht ihn mit einem Golden-Trace. Der Exit-Code folgt dem
/// Lauf-Verdikt (13.5) und dem Vergleich.
/// `takt tune DATEI --ticks N --save PROFIL [--stim S.trace] [--out DATEI]`
/// (8.4): Der Lauf mit seinen `tune`-Zeilen, dann der zuletzt
/// uebernommene Satz der Tunables als Profilblock — ein Werkzeug ohne
/// eigene Semantik.
fn tune(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(name) = args.value("--save") else {
        eprintln!("--save PROFIL fehlt");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    let Some(ticks) = args.value("--ticks").and_then(|t| t.parse::<u64>().ok()) else {
        eprintln!("--ticks N fehlt");
        return false;
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
    let options = RunOptions { ticks, profile: profile_of(args), ..Default::default() };
    let result = match takt_interp::run(&program, &stimulus, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    let mut block = format!("profile {name}:\n");
    for (p, (pname, value)) in program.params.iter().zip(&result.params) {
        if p.tunable {
            block.push_str(&format!("    {pname} = {value}\n"));
        }
    }
    match args.value("--out") {
        Some(out) => {
            if let Err(e) = std::fs::write(out, &block) {
                eprintln!("{out}: {e}");
                return false;
            }
            eprintln!("{out}: Profil `{name}` nach {ticks} Ticks geschrieben");
        }
        None => print!("{block}"),
    }
    true
}

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
    let steps_to = args.value("--steps");
    let options =
        RunOptions { ticks, profile: profile_of(args), order_seed, steps: steps_to.is_some(), ..Default::default() };
    let result = match takt_interp::run(&program, &stimulus, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    let text = result.trace.render();
    let mut ok = result.verdict != Verdict::Fail;
    if let Some(out) = steps_to {
        let src = read(path).unwrap_or_default();
        if let Err(e) = std::fs::write(out, steps_with_positions(&result.steps, &src)) {
            eprintln!("{out}: {e}");
            return false;
        }
    }
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

/// Ersetzt `@<versatz>` durch `<zeile>:<spalte>` (`takt sim --steps`).
fn steps_with_positions(steps: &str, src: &str) -> String {
    let mut starts = vec![0usize];
    starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let mut out = String::new();
    for line in steps.lines() {
        match line.split_once(" @").and_then(|(head, rest)| {
            let (offset, tail) = rest.split_once(' ').unwrap_or((rest, ""));
            Some((head, offset.parse::<usize>().ok()?, tail))
        }) {
            Some((head, offset, tail)) => {
                let row = starts.partition_point(|&s| s <= offset);
                let col = src.get(starts[row - 1]..offset).map_or(0, |s| s.chars().count()) + 1;
                out.push_str(head);
                let _ = write!(out, " {row}:{col}");
                if !tail.is_empty() {
                    let _ = write!(out, " {tail}");
                }
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// `takt test`: jedes Szenario als eigener Sim-Lauf (13.6); Verdikte je
/// Szenario, Coverage als Vereinigung (13.2), und jeder irreversible Output
/// muss von einem Szenario abgedeckt sein (12.7).
fn test(args: &Args) -> bool {
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
        None => 100_000,
    };
    let scenarios: Vec<String> = program
        .machines
        .iter()
        .filter(|m| m.kind == takt_mir::machine::MachineKind::Scenario)
        .map(|m| m.name.clone())
        .filter(|n| args.value("--scenario").is_none_or(|s| takt_mir::machine::scenario_name(s) == *n))
        .collect();
    if scenarios.is_empty() {
        eprintln!("{path}: kein Szenario (13.6)");
        return false;
    }
    let universe = takt_interp::coverage::universe(&program);
    let mut coverage = takt_interp::Coverage::default();
    // 13.4: je Pruefstelle die Szenarien, die sie durchliefen.
    let mut by_scenario: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut ok = true;
    for name in &scenarios {
        let options =
            RunOptions { ticks, profile: profile_of(args), scenario: Some(name.clone()), ..Default::default() };
        let result = match takt_interp::run(&program, &Trace::default(), &options) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{path}: {name}: {e:?}");
                return false;
            }
        };
        let last = result.trace.lines.iter().map(|l| l.tick).max().unwrap_or(0);
        println!("{name}: {} nach {last} Ticks ({})", result.verdict.name(), result.ended.name());
        for p in &result.properties {
            let word = if p.assumption { "assumption" } else { "property" };
            println!("  {word} {}: {}", p.name, p.outcome.text());
        }
        // Der Trace je Szenario, wie `takt campaign --out` ihn schreibt (12.5).
        if let Some(dir) = args.value("--out") {
            let file = std::path::Path::new(dir).join(format!("{name}.trace"));
            if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, result.trace.render())) {
                eprintln!("{}: {e}", file.display());
                return false;
            }
        }
        ok &= result.verdict != Verdict::Fail;
        for (kind, machine, key) in result.coverage.hits.keys() {
            if *kind == takt_interp::CoverKind::Check {
                by_scenario.entry((machine.clone(), key.clone())).or_default().push(name.clone());
            }
        }
        coverage.merge(&result.coverage);
    }
    println!("Coverage: {}", coverage.summary(universe));
    // 13.4: welche Szenarien welche Anforderung durchliefen.
    let index = takt_mir::requirements::index(&program);
    if !index.is_empty() {
        if let Some(src) = read(path) {
            let map = SourceMap::single(path.as_str(), src.as_str());
            for line in requirement_lines(&index, &map, Some(&by_scenario)) {
                println!("{line}");
            }
        }
    }
    for c in program.channels.iter().filter(|c| c.attrs.irreversible) {
        let covered = coverage.hits.keys().any(|(k, _, n)| *k == takt_interp::CoverKind::Irreversible && *n == c.name);
        if !covered {
            println!("FAIL: irreversibler Output `{}` von keinem Szenario abgedeckt (12.7)", c.name);
            ok = false;
        }
    }
    if let Some(out) = args.value("--coverage") {
        if let Err(e) = std::fs::write(out, coverage.render()) {
            eprintln!("{out}: {e}");
            return false;
        }
    }
    ok
}

/// `takt prove`: die Schrittfunktion als Transitionssystem; `--export
/// DATEI.smt2` schreibt BMC und Induktionsschritt bis `--depth`, sonst
/// prueft ein Solver (`--solver`, `TAKT_SOLVER`, `z3`/`cvc5` auf dem PATH)
/// jede Eigenschaft; ein Gegenbeispiel landet als Stimulus in `--out` (13.3).
fn prove(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    let depth = match args.value("--depth").map(str::parse::<u32>) {
        Some(Ok(n)) => n,
        Some(Err(e)) => {
            eprintln!("--depth: {e}");
            return false;
        }
        None => 5,
    };
    let model = match takt_prove::encode(&program) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{path}: nicht kodierbar: {} (plan/m6.md 2.8)", e.what);
            return false;
        }
    };
    println!(
        "{path}: {} Zustandsvariablen, {} Eingaben, {} Annahmen, {} Beweisziele",
        model.state.len(),
        model.inputs.len(),
        model.assumptions.len(),
        model.properties.len()
    );
    for n in &model.notes {
        println!("  Reichweite: {n}");
    }
    match args.value("--export") {
        Some(out) => {
            if let Err(e) = std::fs::write(out, takt_prove::export(&model, depth)) {
                eprintln!("{out}: {e}");
                return false;
            }
            println!("  {out}: BMC und Induktionsschritt bis Tiefe {depth}");
            true
        }
        None => {
            let solver = match args.value("--solver") {
                Some(p) => takt_prove::Solver::At(p.into()),
                None => takt_prove::find(),
            };
            if !solver.works() {
                eprintln!(
                    "{path}: kein Solver — `z3` oder `cvc5` auf den PATH oder nach `~/.takt/bin`, `TAKT_SOLVER` oder `--solver` setzen (13.3)"
                );
                return false;
            }
            let timeout = match args.value("--timeout").map(str::parse::<u64>) {
                Some(Ok(n)) => n,
                Some(Err(e)) => {
                    eprintln!("--timeout: {e}");
                    return false;
                }
                None => 60,
            };
            let reports = match takt_prove::prove(&model, &program, depth, &solver, timeout) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{path}: {e}");
                    return false;
                }
            };
            let mut ok = true;
            // B2: die Vertraege der Bloecke — aus jedem typkonformen Zustand.
            if !model.contracts.is_empty() {
                let contracts = match takt_prove::verify_contracts(&model, &solver, timeout) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("{path}: {e}");
                        return false;
                    }
                };
                let map = read(path).map(|src| SourceMap::single(path.as_str(), src.as_str()));
                println!("  Vertraege: {}", contracts.len());
                for c in &contracts {
                    let (line, col) = map.as_ref().map_or((0, 0), |m| m.line_col(c.span));
                    println!("    {}.step {line}:{col}: {}", c.block, c.verdict.text());
                    ok &= !matches!(c.verdict, takt_prove::ContractVerdict::Violated { .. });
                }
            }
            // B3: jede Pruefstelle klassifiziert — bewiesen unerreichbar,
            // erreichbar mit Pfad, unentschieden; ohne Budget-Effekt (FB-49).
            if !model.checks.is_empty() {
                let checks = match takt_prove::classify(&model, &program, depth, &solver, timeout) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("{path}: {e}");
                        return false;
                    }
                };
                let count = |f: fn(&takt_prove::CheckVerdict) -> bool| checks.iter().filter(|c| f(&c.verdict)).count();
                println!(
                    "  Pruefstellen: {} bewiesen unerreichbar, {} erreichbar mit Pfad, {} unentschieden",
                    count(|v| matches!(v, takt_prove::CheckVerdict::Unreachable { .. })),
                    count(|v| matches!(v, takt_prove::CheckVerdict::Reachable { .. })),
                    count(|v| matches!(v, takt_prove::CheckVerdict::Undecided { .. }))
                );
                let map = read(path).map(|src| SourceMap::single(path.as_str(), src.as_str()));
                for c in &checks {
                    let (line, col) = map.as_ref().map_or((0, 0), |m| m.line_col(c.span));
                    println!("    {} {}:{line}:{col}: {}", c.kind, c.machine, c.text());
                }
                // 11.3: Die bewiesenen Stellen neben das Programm, mit dem
                // Hash der Quelle; `--proof` laesst sie im Codegen aus.
                if let Some(out) = args.value("--save-proof") {
                    let sites: Vec<takt_mir::analysis::proof::Site> = checks
                        .iter()
                        .filter_map(|c| match c.verdict {
                            takt_prove::CheckVerdict::Unreachable { k } => {
                                Some(takt_mir::analysis::proof::Site { start: c.start, kind: c.kind.clone(), k })
                            }
                            _ => None,
                        })
                        .filter(|s| takt_mir::analysis::walk::tag_of_name(&s.kind).is_some())
                        .collect();
                    let Some(src) = read(path) else { return false };
                    let text = takt_mir::analysis::proof::render(&takt_mir::review::hash_of(src.as_bytes()), &sites);
                    if let Err(e) = std::fs::write(out, text) {
                        eprintln!("{out}: {e}");
                        return false;
                    }
                    println!("  {out}: {} bewiesene Stellen", sites.len());
                }
            }
            for r in &reports {
                let word = if r.assumption { "assumption" } else { "property" };
                println!("  {word} {}: {}", r.name, r.verdict.text());
                if let takt_prove::Verdict::Violated { stimulus, .. } = &r.verdict {
                    ok = false;
                    match args.value("--out") {
                        Some(dir) => {
                            let file = std::path::Path::new(dir).join(format!("{}.stim.trace", r.name));
                            if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, stimulus))
                            {
                                eprintln!("{}: {e}", file.display());
                                return false;
                            }
                            println!("    Gegenbeispiel: {}", file.display());
                        }
                        None => {
                            for line in stimulus.lines() {
                                println!("    {line}");
                            }
                        }
                    }
                }
            }
            ok
        }
    }
}

/// `takt campaign`: der Laufraum einer Kampagne als Tabelle, je Lauf eine
/// Aufzeichnung (13.7).
fn campaign(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    let campaign = match (args.files.get(1), program.campaigns.as_slice()) {
        (Some(name), all) => all.iter().find(|c| c.name == *name),
        (None, [one]) => Some(one),
        (None, _) => None,
    };
    let Some(campaign) = campaign else {
        let names: Vec<&str> = program.campaigns.iter().map(|c| c.name.as_str()).collect();
        let have = if names.is_empty() { "keine".to_string() } else { names.join(", ") };
        eprintln!("{path}: Kampagne nennen — vorhanden: {have}");
        return false;
    };
    if let Some(file) = &campaign.program {
        let stem = std::path::Path::new(path).file_name().and_then(|f| f.to_str()).unwrap_or(path);
        if file != stem {
            eprintln!("{path}: Kampagne `{}` gehoert zu `{file}` (13.7)", campaign.name);
            return false;
        }
    }
    // Pruefung 29 urteilt mit dem gemessenen Jitter der Konfiguration (13.7).
    if let Some(hw) = hardware(args) {
        let diags = takt_sema::calibrated::check_bindings(&program, &hw);
        for d in &diags {
            println!("{d}");
        }
        if diags.iter().any(|d| d.is_error()) {
            return false;
        }
    }
    let Some(ticks) = ticks_of(args) else { return false };
    let Some(stimulus) = stimulus_of(args) else { return false };
    let profile = profile_of(args).or_else(|| campaign.profile.map(|p| program.profiles[p.index()].name.clone()));
    let runs = match takt_interp::campaign::runs(&program, campaign) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    if let Some(dir) = args.value("--out") {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("{dir}: {e}");
            return false;
        }
    }
    let swept: Vec<String> =
        runs.first().map(|r| r.params.iter().map(|(n, _)| n.clone()).collect()).unwrap_or_default();
    let mut rows =
        vec![[vec!["Lauf".to_string()], swept, vec!["Wdh".into(), "Verdikt".into(), "Messwerte".into()]].concat()];
    let mut fails = 0;
    let mut stopped = None;
    for run in &runs {
        let options = RunOptions {
            ticks,
            profile: profile.clone(),
            scenario: args.value("--scenario").map(str::to_string),
            overrides: run.params.clone(),
            ..Default::default()
        };
        let result = match takt_interp::run(&program, &stimulus, &options) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{path}: Lauf {}: {e:?}", run.id);
                return false;
            }
        };
        let values: Vec<String> = run.params.iter().map(|(_, v)| v.clone()).collect();
        let tail = vec![run.repeat.to_string(), result.verdict.name().to_string(), measures_of(&result.trace)];
        rows.push([vec![run.id.to_string()], values, tail].concat());
        if let Some(dir) = args.value("--out") {
            let file = std::path::Path::new(dir).join(format!("{}-{:03}.trace", campaign.name, run.id));
            let header = takt_interp::record::Header::of(&program, profile.as_deref(), &result.start_params, ticks);
            let recording = takt_interp::record::Recording { header, inputs: stimulus.clone() }.seal(&result.trace);
            if let Err(e) = std::fs::write(&file, recording.render()) {
                eprintln!("{}: {e}", file.display());
                return false;
            }
        }
        if result.verdict == Verdict::Fail {
            fails += 1;
            if campaign.stop_on == takt_mir::program::StopOn::Fail {
                stopped = Some(run.id);
                break;
            }
        }
    }
    print!("{}", table(&rows));
    match stopped {
        Some(id) => println!("abgebrochen nach Lauf {id} von {} (stop_on fail)", runs.len()),
        None => println!("{} Laeufe, {fails} FAIL", runs.len()),
    }
    fails == 0
}

/// Die Messwerte eines Laufs: der letzte Wert je Name (13.5).
fn measures_of(trace: &Trace) -> String {
    let mut seen: Vec<(String, String)> = Vec::new();
    for line in &trace.lines {
        if let takt_interp::trace::LineKind::Measure { name, value, .. } = &line.kind {
            match seen.iter_mut().find(|(n, _)| n == name) {
                Some(slot) => slot.1 = value.clone(),
                None => seen.push((name.clone(), value.clone())),
            }
        }
    }
    seen.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join(", ")
}

/// Spalten linksbuendig, zwei Leerzeichen dazwischen.
fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let width: Vec<usize> =
        (0..cols).map(|i| rows.iter().map(|r| r.get(i).map_or(0, |c| c.chars().count())).max().unwrap_or(0)).collect();
    let mut out = String::new();
    for r in rows {
        let cells: Vec<String> = r.iter().enumerate().map(|(i, c)| format!("{c:<w$}", w = width[i])).collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

/// `takt run`: ein Lauf, der seine Eingaben aufzeichnet (12.5).
///
/// Der Unterschied zu `sim` ist die Aufzeichnung: `run` schreibt mit
/// `--record`, was hineinging, damit `replay` es wiederholen kann. Ohne
/// `--record` ist es `sim` mit anderem Namen — und das ist der Punkt:
/// 11.3 verlangt „gleiche Quelle, gleiche Toolchain, gleiches Ergebnis",
/// also darf die Aufzeichnung den Lauf nicht aendern.
fn run_cmd(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    let Some(ticks) = ticks_of(args) else { return false };
    let Some(stimulus) = stimulus_of(args) else { return false };

    let options = RunOptions { ticks, profile: profile_of(args), order_seed: None, ..Default::default() };
    let result = match takt_interp::run(&program, &stimulus, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    if let Some(out) = args.value("--record") {
        let header =
            takt_interp::record::Header::of(&program, profile_of(args).as_deref(), &result.start_params, ticks);
        let recording = takt_interp::record::Recording { header, inputs: stimulus }.seal(&result.trace);
        if let Err(e) = std::fs::write(out, recording.render()) {
            eprintln!("{out}: {e}");
            return false;
        }
        println!("{out}: aufgezeichnet");
    }
    if let Some(out) = args.value("--trace") {
        if let Err(e) = std::fs::write(out, result.trace.render()) {
            eprintln!("{out}: {e}");
            return false;
        }
    } else {
        print!("{}", result.trace.render());
    }
    println!("{path}: {} nach {ticks} Ticks", result.verdict.name());
    result.verdict != Verdict::Fail
}

/// `takt replay`: eine Aufzeichnung wiederholen und vergleichen (12.5).
///
/// 12.5: „`takt replay` fuehrt dasselbe Binaer im Sim-Modus mit den
/// aufgezeichneten Inputs aus und vergleicht Outputs und Zustandspfade;
/// Abweichung = Fehler in Runtime oder Treiber, nie in der Logik (Satz
/// 9.4.4)."
///
/// Der Satz gilt nur, wenn die Logik dieselbe ist — der Logik-Hash im
/// Kopf prueft das, und eine Aufzeichnung zu einem anderen Programm wird
/// abgelehnt statt verglichen.
fn replay(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(program) = compile_file(path, args) else { return false };
    let Some(record_path) = args.value("--record") else {
        eprintln!("--record fehlt: `takt replay PROGRAMM --record AUFZEICHNUNG`");
        return false;
    };
    let Some(text) = read(record_path) else { return false };
    let recording = match takt_interp::record::Recording::parse(&text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{record_path}: {e}");
            return false;
        }
    };
    if let Err(e) = recording.matches(&program) {
        eprintln!("{record_path}: {e}");
        return false;
    }
    // Eine Scheibe (12.5) spielt ihre Maschine allein ab; `--machine`
    // waehlt sie sonst aus dem Gesamtlauf aus.
    let machine = match (args.value("--machine"), &recording.header.machine) {
        (Some(a), Some(b)) if a != b.as_str() => {
            eprintln!("{record_path}: die Scheibe gehoert zu `{b}`, nicht zu `{a}`");
            return false;
        }
        (a, b) => a.map(str::to_string).or_else(|| b.clone()),
    };
    let machine = match machine {
        Some(name) => match program.machines.iter().position(|m| m.name == name) {
            Some(i) => Some((name, takt_mir::MachineId(i as u32))),
            None => {
                eprintln!("{path}: Maschine `{name}` gibt es nicht");
                return false;
            }
        },
        None => None,
    };
    // Die Zahl der Ticks steht im Kopf; `--ticks` darf sie ueberschreiben,
    // um einen Lauf abzukuerzen.
    let ticks = args.value("--ticks").and_then(|v| v.parse::<u64>().ok()).unwrap_or(recording.header.ticks);
    let options = RunOptions {
        ticks,
        profile: recording.header.profile.clone(),
        overrides: recording.header.overrides(),
        only: recording.header.machine.clone(),
        ..Default::default()
    };
    let result = match takt_interp::run(&program, &recording.inputs, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    let mut text = result.trace.render();
    if let Some((name, m)) = &machine {
        if recording.header.machine.is_none() {
            // A2a: Die Scheibe aus dem Gesamtlauf ziehen und allein
            // abspielen — sie muss dasselbe zeigen (Satz 9.4.1).
            let slice = takt_interp::record::machine_slice(&program, &recording, &result.trace, *m);
            if let Some(out) = args.value("--extract") {
                if let Err(e) = std::fs::write(out, slice.render()) {
                    eprintln!("{out}: {e}");
                    return false;
                }
                println!("{out}: Scheibe von `{name}` geschrieben");
            }
            let alone = RunOptions { only: Some(name.clone()), ..options.clone() };
            let alone = match takt_interp::run(&program, &slice.inputs, &alone) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{path}: Scheibe von `{name}`: {e:?}");
                    return false;
                }
            };
            let whole = takt_interp::record::machine_lines(&program, &result.trace, *m).render();
            let sliced = takt_interp::record::machine_lines(&program, &alone.trace, *m).render();
            if whole != sliced {
                print!("{}", diff(&whole, &sliced));
                eprintln!("Scheibe von `{name}` weicht vom Gesamtlauf ab — Fehler im Interpreter (12.5)");
                return false;
            }
        }
        text = takt_interp::record::machine_lines(&program, &result.trace, *m).render();
    }
    // Mit `--golden` wird verglichen, sonst ausgegeben. Der Vergleich ist
    // der eigentliche Zweck: Er zeigt, ob Runtime und Treiber sich
    // gleich verhalten haben.
    match args.value("--golden") {
        Some(golden) => {
            let Some(mut expected) = read(golden) else { return false };
            if let Some((_, m)) = &machine {
                expected = match Trace::parse(&expected) {
                    Ok(t) => takt_interp::record::machine_lines(&program, &t, *m).render(),
                    Err(e) => {
                        eprintln!("{golden}: {e}");
                        return false;
                    }
                };
            }
            if text != expected {
                print!("{}", diff(&expected, &text));
                eprintln!("{golden}: Trace weicht ab — Fehler in Runtime oder Treiber (12.5)");
                return false;
            }
            println!("{record_path}: reproduziert, {ticks} Ticks");
        }
        None => print!("{text}"),
    }
    result.verdict != Verdict::Fail
}

/// `takt timing`: die physische Zeit eines Laufs aus seinen
/// `time`-Metazeilen (7.3, 12.3).
fn timing(args: &Args) -> bool {
    let Some(path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(text) = read(path) else { return false };
    let trace = match Trace::parse(&text) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}: {e}");
            return false;
        }
    };
    let times: Vec<(u64, i64, i64, u64)> = trace
        .lines
        .iter()
        .filter_map(|l| match l.kind {
            takt_interp::trace::LineKind::Time { took, drift, slept } => Some((l.tick, took, drift, slept)),
            _ => None,
        })
        .collect();
    if times.is_empty() {
        eprintln!("{path}: keine `time`-Zeilen; nur native Laeufe schreiben sie (12.3)");
        return false;
    }
    let period: i64 = match args.value("--tick").map(str::parse) {
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            eprintln!("--tick: Nanosekunden erwartet");
            return false;
        }
        None => {
            eprintln!("--tick fehlt: `takt timing TRACE --tick <ns>` (T0 aus `system: tick`)");
            return false;
        }
    };
    let (mut late, mut lost, mut worst_drift, mut worst_took, mut slept) = (0u64, 0i64, 0i64, 0i64, 0u64);
    let mut last_drift = 0i64;
    for (_, took, drift, n) in &times {
        let drift = (*drift).max(0);
        if drift >= period {
            late += 1;
        }
        if drift > last_drift {
            lost += drift - last_drift;
        }
        last_drift = drift;
        worst_drift = worst_drift.max(drift);
        worst_took = worst_took.max(*took);
        slept += n;
    }
    let lost = if period > 0 { lost / period } else { 0 };
    println!("{path}: {} Ticks mit Zeit", times.len());
    println!("  verspaetete Ticks   {late}");
    println!("  verlorene Perioden  {lost}");
    println!("  groesster Rueckstand {worst_drift} ns");
    println!("  laengster Schritt   {worst_took} ns");
    println!("  geschlafene Ticks   {slept}");
    let mut spaeteste: Vec<&(u64, i64, i64, u64)> = times.iter().collect();
    spaeteste.sort_by_key(|(_, _, d, _)| -*d);
    for (k, took, drift, _) in spaeteste.iter().take(3).filter(|(_, _, d, _)| *d > 0) {
        println!("  Tick {k}: {drift} ns zu spaet, Schritt {took} ns");
    }
    true
}

/// `takt verify-trace`: rechnet die Hashkette eines Traces nach (12.5, A3).
fn verify_trace(args: &Args) -> bool {
    let Some(trace_path) = args.files.first() else {
        eprintln!("{USAGE}");
        return false;
    };
    let Some(record_path) = args.value("--record") else {
        eprintln!("--record fehlt: `takt verify-trace TRACE --record AUFZEICHNUNG`");
        return false;
    };
    let (Some(record_text), Some(trace_text)) = (read(record_path), read(trace_path)) else { return false };
    let recording = match takt_interp::record::Recording::parse(&record_text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{record_path}: {e}");
            return false;
        }
    };
    let trace = match Trace::parse(&trace_text) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{trace_path}: {e}");
            return false;
        }
    };
    match recording.verify(&trace) {
        Ok(h) => {
            println!("{trace_path}: Kette stimmt, {h}");
            true
        }
        Err(e) => {
            eprintln!("{trace_path}: {e}");
            false
        }
    }
}

/// `--ticks` lesen; die Meldung nennt, was fehlt.
fn ticks_of(args: &Args) -> Option<u64> {
    match args.value("--ticks").map(str::parse::<u64>) {
        Some(Ok(n)) => Some(n),
        Some(Err(e)) => {
            eprintln!("--ticks: {e}");
            None
        }
        None => {
            eprintln!("--ticks fehlt");
            None
        }
    }
}

/// `--stim` lesen; ohne Angabe ein leerer Stimulus.
fn stimulus_of(args: &Args) -> Option<Trace> {
    match args.value("--stim") {
        Some(p) => {
            let text = read(p)?;
            match Trace::parse(&text) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("{p}: {e}");
                    None
                }
            }
        }
        None => Some(Trace::default()),
    }
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
