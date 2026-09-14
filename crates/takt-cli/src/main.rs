//! `takt`: Kommandozeile fuer `check`, `build`, `sim`, `size`, `cost`,
//! `latency`, `mir`, `fmt`, `parse` und `tokens`.
//!
//! ```text
//! takt check DATEI… [--warnings-as-errors] [--certification] [--format text|line]
//!                   [--build sim|hw] [--profile P]
//! takt sim   DATEI --ticks N [--stim S.trace] [--golden G.trace] [--trace OUT.trace]
//!                   [--profile P] [--order random:SEED]
//! takt build DATEI [--target x86_64|aarch64|thumbv7em|riscv32imac]
//!                   [--emit ir|obj|consts|consts-rs] [--out PFAD]
//! takt size  DATEI… [--build sim|hw] [--profile P] [--object DATEI.o] [--target NAME]
//! takt cost  DATEI… [--build sim|hw] [--profile P]
//! takt latency DATEI… [--build sim|hw] [--profile P]
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

const USAGE: &str = "takt check|build|sim|run|replay|size|cost|latency|graph|mir|fmt|parse|tokens DATEI… (siehe crates/takt-cli/src/main.rs)";

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
        "run" => run_cmd(&args),
        "replay" => replay(&args),
        "mir" => mir(&args),
        "fmt" => fmt(&args),
        "build" => build(&args),
        "size" => size(&args),
        "cost" => cost(&args),
        "latency" => latency(&args),
        "graph" => graph(&args),
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
        // Kennzahlen des statischen Gates (3.4; plan/m3.md 5).
        if args.has("--report") && !checked.has_errors() {
            for line in checked.report.lines() {
                println!("  {line}");
            }
        }
    }
    ok
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

    let lowered = takt_llvm::lower::program(&program, target.triple, module_name(path));
    for s in &lowered.skipped {
        // Ein fehlender Schritt ist ein Loch, kein Schoenheitsfehler: Ohne
        // ihn meldet der Linker spaeter ein unbekanntes Symbol statt des
        // Konstrukts, das gefehlt hat (FB-104).
        eprintln!("{}_step fehlt: {}", s.machine, s.reason);
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
            let text = if rust { constants_rust(&program) } else { constants_header(&program, stem) };
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
fn constants_header(p: &takt_mir::Program, stem: &str) -> String {
    let guard = stem.to_uppercase().replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let mut s = String::new();
    s.push_str("/* Konstanten des Programms; erzeugt von `takt build --emit consts`. */\n");
    s.push_str(&format!("#ifndef TAKT_{guard}_H\n#define TAKT_{guard}_H\n\n"));
    s.push_str("/* Basis-Tick T0 in Nanosekunden (`system: tick`, 7.1). */\n");
    s.push_str(&format!("#define TAKT_TICK_NS {}LL\n\n", p.config.tick));
    s.push_str("/* Die Maschinen, in Deklarationsreihenfolge (9.4). */\n");
    for m in p.machines.iter().filter(|m| m.kind != takt_mir::machine::MachineKind::Template) {
        s.push_str(&format!("/*   {} — jeder {}. Tick */\n", m.name, m.period));
    }
    s.push_str(&format!("#define TAKT_MACHINES {}\n\n", p.machines.len()));
    s.push_str("#endif\n");
    s
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
fn constants_rust(p: &takt_mir::Program) -> String {
    let mut s = String::new();
    // Regulaere Kommentare, keine `//!`: Die Datei wird per `include!` in
    // ein Modul gezogen, und dort darf kein innerer Doc-Kommentar stehen.
    s.push_str("// Konstanten des Programms; erzeugt von `takt build --emit consts-rs`.\n");
    s.push_str("// Nicht von Hand aendern — die Quelle ist die `.takt`-Datei.\n\n");
    s.push_str("/// Basis-Tick T0 in Nanosekunden (`system: tick`, 7.1).\n");
    s.push_str(&format!("pub const TICK_NS: i64 = {};\n\n", p.config.tick));

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
        .args(["-Wno-override-module", "-O2", "-c"])
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
        let report = takt_mir::analysis::size::size(program).with_object(&measure(args, program));
        for line in report.lines() {
            println!("{line}");
        }
    }
    ok
}

/// Was ein erzeugtes Objekt beisteuert (11.5, 12.3).
///
/// **Ohne `--object` bleibt es leer, und das ist kein Mangel.** Flash und
/// Stacktiefe entscheidet der Codegen, nicht die MIR; wer sie wissen will,
/// muss uebersetzt haben. `takt build --emit obj` liefert die Datei.
fn measure(args: &Args, p: &takt_mir::Program) -> takt_mir::analysis::size::Measured {
    let mut out = takt_mir::analysis::size::Measured::default();
    let Some(file) = args.value("--object") else { return out };
    let path = std::path::Path::new(file);
    if !path.exists() {
        eprintln!("{file}: nicht gefunden; Flash und Stack bleiben offen");
        return out;
    }

    // Ohne `--target` der Wirt: Ein Objekt ohne Angabe stammt meist aus
    // `takt build` ohne Ziel, und das uebersetzt fuer den Wirt.
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let target = args.value("--target").and_then(takt_llvm::Target::by_name).unwrap_or(host);
    let tools = takt_llvm::inspect::Binutils::best_for(target);

    out.flash = tools.sections(path).map(|s| s.flash());

    // Die Rahmen aller Funktionen in einem Durchlauf; die Rechnung
    // braucht sie vollstaendig, sonst meldet sie unbekannt.
    let symbols: Vec<String> = p.fns.iter().map(takt_llvm::fns::symbol).collect();
    let frames: Vec<Option<u32>> =
        tools.stack_frames(path, &symbols).into_iter().map(|f| f.and_then(|n| u32::try_from(n).ok())).collect();
    out.stack = takt_mir::analysis::stack::depth(p, &frames);
    out
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

    let options = RunOptions { ticks, profile: profile_of(args), order_seed: None };
    let result = match takt_interp::run(&program, &stimulus, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    if let Some(out) = args.value("--record") {
        let recording = takt_interp::record::Recording {
            header: takt_interp::record::Header::of(&program, profile_of(args).as_deref(), ticks),
            inputs: stimulus,
        };
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
    // Die Zahl der Ticks steht im Kopf; `--ticks` darf sie ueberschreiben,
    // um einen Lauf abzukuerzen.
    let ticks = args.value("--ticks").and_then(|v| v.parse::<u64>().ok()).unwrap_or(recording.header.ticks);
    let options = RunOptions { ticks, profile: recording.header.profile.clone(), order_seed: None };
    let result = match takt_interp::run(&program, &recording.inputs, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e:?}");
            return false;
        }
    };
    let text = result.trace.render();
    // Mit `--golden` wird verglichen, sonst ausgegeben. Der Vergleich ist
    // der eigentliche Zweck: Er zeigt, ob Runtime und Treiber sich
    // gleich verhalten haben.
    match args.value("--golden") {
        Some(golden) => {
            let Some(expected) = read(golden) else { return false };
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
