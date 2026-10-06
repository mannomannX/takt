//! `takt bench` (13.8): die Kostentabelle eines Ziels, gemessen.
//!
//! ```text
//! takt bench --emit embed --target ZIEL [--hardware DATEI.hw] --out VERZEICHNIS
//! takt bench --import LOG… --target ZIEL [--loop LOG] [--natives LOG] [--board NAME]
//!            [--hardware DATEI.hw] [--conformance DATEI]
//! takt bench --board stm32f401|esp32c6 [--runs N] [--logs VERZEICHNIS]
//!            [--hardware DATEI.hw] [--conformance DATEI]
//! ```
//!
//! **Drei Wege, eine Rechnung** (plan/m11.md 2.12). `--emit embed` baut das
//! Messprogramm als Bibliothek fuer einen Wirt, der einen Zyklenzaehler und
//! eine Senke stellt; `--import` rechnet aus dem, was die Senke bekam —
//! dazu, wenn gemessen, die Tickschleife und die Natives —, Tabelle und
//! Bericht; `--board` faehrt beides auf einem eigenen Board und behaelt die
//! Protokolle unter `--logs`. `--hardware` traegt die Messwerte in die
//! Konfiguration ein, ohne ihre Kommentare zu verwerfen; `--conformance`
//! schreibt den Konformitaetsbericht daneben, die Quelle der Zahlen (13.8).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use takt_conformance::bench::library::{self, NAME};
use takt_conformance::bench::{self as suite, Kernel, Logs, Outcome, Role};
use takt_llvm::Target;
use takt_llvm::symbols::Prefix;

use crate::Args;
use crate::embed::Code;

pub(crate) fn bench(args: &Args) -> bool {
    match args.value("--emit") {
        Some("embed") => emit_embed(args),
        Some(other) => {
            eprintln!("takt bench --emit: nur `embed` (war: {other})");
            false
        }
        None if args.has("--import") => import(args),
        None => on_board(args),
    }
}

/// `--emit embed`: das Messprogramm als Bibliothek.
fn emit_embed(args: &Args) -> bool {
    let Some(out) = args.value("--out") else {
        eprintln!("takt bench --emit embed schreibt in ein Verzeichnis; `--out VERZEICHNIS` fehlt");
        return false;
    };
    let name = args.value("--target").unwrap_or("x86_64");
    let (target, triple) = match target_of(name) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("--target: {e}");
            return false;
        }
    };
    let hw = if args.value("--hardware").is_some() {
        let Some(hw) = crate::hardware(args) else { return false };
        Some(hw)
    } else {
        None
    };
    match build_library(target, &triple, hw.as_ref(), Path::new(out)) {
        Ok(steps) => {
            println!("{out}: `{NAME}` fuer {triple}, {steps} Schritte");
            true
        }
        Err(e) => {
            eprintln!("takt bench --emit embed: {e}");
            false
        }
    }
}

/// Ein Takt-Ziel beim Namen oder das Tripel des Wirts (12.11).
fn target_of(name: &str) -> Result<(Target, String), String> {
    match Target::by_name(name) {
        Some(t) => Ok((t, t.triple.to_string())),
        None => Target::by_host_triple(name).map(|t| (t, name.to_string())),
    }
}

/// Die Objekte eines Kerns nach seiner Stellung in der Suite.
type Built = (usize, Result<Vec<PathBuf>, String>);

/// Baut Kerne, C-Referenzen und Laeufer nach `out` und bindet sie zu einer
/// Bibliothek, mit Kopf, Modul, Manifest und unter `xip_flash` den
/// Fragmenten fuer den Linker. Liefert die Zahl der Schritte.
fn build_library(
    target: Target,
    triple: &str,
    hw: Option<&takt_mir::hardware::Hardware>,
    out: &Path,
) -> Result<usize, String> {
    let kernels = suite::suite();
    let vectors = suite::math_vectors()?;
    let id = suite::suite_id(&kernels, &vectors);
    let src = out.join("src");
    std::fs::create_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let march = if target.march.is_empty() { None } else { Some(format!("-march={}", target.march)) };
    let flags: Vec<String> = march.into_iter().collect();

    // Die Kerne sind voneinander unabhaengig; ein Kern bleibt fuer den Rest
    // der Maschine frei.
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
    let next = AtomicUsize::new(0);
    let built: Mutex<Vec<Built>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers.min(kernels.len()) {
            let work = || {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(k) = kernels.get(at) else { break };
                    let objects = kernel_objects(k, target, hw, &src, &flags);
                    built.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((at, objects));
                }
            };
            // Sema und Codegen brauchen den Stapel des Hauptfadens (`STACK`).
            let spawned = std::thread::Builder::new().stack_size(crate::STACK).spawn_scoped(scope, work);
            if let Err(e) = spawned {
                built.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((usize::MAX, Err(e.to_string())));
            }
        }
    });
    let mut built = built.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    built.sort_by_key(|(at, _)| *at);
    let mut objects = Vec::new();
    for (_, result) in built {
        objects.extend(result?);
    }

    let runner = src.join(format!("{NAME}.c"));
    write(&src.join(format!("{NAME}.h")), &library::header(&kernels, &id))?;
    write(&runner, &library::runner(&kernels, &vectors))?;
    let runner_obj = src.join(format!("{NAME}.o"));
    let include = format!("-I{}", src.display());
    let runner_flags: Vec<&str> = flags.iter().map(String::as_str).chain([include.as_str()]).collect();
    // Der Laeufer ist das Messgeraet: ohne ausgelagerte Folgen zwischen den Lesungen.
    crate::embed::compile(&runner, &runner_obj, target, Code::Frame, &runner_flags)?;
    objects.push(runner_obj);

    let lib = out.join(if triple.ends_with("-msvc") { format!("{NAME}.lib") } else { format!("lib{NAME}.a") });
    let refs: Vec<&Path> = objects.iter().map(PathBuf::as_path).collect();
    crate::embed::archive(&lib, &refs)?;
    write(&out.join(format!("{NAME}.h")), &library::header(&kernels, &id))?;
    write(&out.join(format!("{NAME}.rs")), &library::rust_module(&kernels, &id))?;
    // 12.3: Unter `xip_flash` misst der Kern aus dem RAM, wie das Programm
    // spaeter laeuft; aus dem Flash maesse er den Cache mit.
    let xip = hw.and_then(|hw| hw.target(target.name)).is_some_and(|t| t.memory.iram.is_some());
    if xip {
        write(&out.join(format!("{NAME}_ram.x")), &takt_frame::linker::bench_ld_fragment())?;
        write(&out.join(format!("{NAME}.lf")), &takt_frame::linker::bench_ldgen_fragment())?;
    }
    write(&out.join(format!("{NAME}.manifest")), &library::manifest(&kernels, &id, target.name, triple, xip))?;
    Ok(library::steps(&kernels))
}

/// Die Objekte eines Kerns: erzeugter Code, Rahmen und, bei einem
/// Referenzkern, seine C-Referenz.
fn kernel_objects(
    k: &Kernel,
    target: Target,
    hw: Option<&takt_mir::hardware::Hardware>,
    src: &Path,
    flags: &[String],
) -> Result<Vec<PathBuf>, String> {
    let fail = |e: String| format!("Kern {}: {e}", k.name);
    let (program, _) = suite::program_of(&k.source).map_err(fail)?;
    let x = Prefix::new(&library::prefix(k)).map_err(fail)?;
    // 12.8: das Profil der eigenen Schleife, wie ein Bring-up es baut.
    let profile = Some(takt_mir::program::RuntimeProfile::Baremetal);
    let instrument = takt_llvm::Instrument::default_for(profile, target);
    let diagnostics = takt_llvm::Diagnostics::Ids;
    let lowered = takt_llvm::lower::program_with_diagnostics(&program, target.triple, &x, instrument, diagnostics);
    if !lowered.complete() {
        let missing: Vec<String> = lowered.skipped.iter().map(|s| s.reason.clone()).collect();
        return Err(fail(format!("unvollstaendig uebersetzt: {}", missing.join("; "))));
    }
    let frame = takt_frame::mcu::build_with(
        &program,
        takt_frame::mcu::Frame {
            diagnostics,
            hardware: hw,
            protect: None,
            prefix: x.clone(),
            // Ein Messkern misst den Tick, nicht die Peripherie (12.6).
            stubs: true,
            job_stack_reserve: takt_frame::mcu::JOB_STACK_RESERVE,
        },
    );
    let (ll, c) = (src.join(format!("{x}.ll")), src.join(format!("{x}_frame.c")));
    write(&ll, &lowered.ir)?;
    write(&c, &frame.source)?;
    write(&src.join(format!("{x}.h")), &frame.header)?;
    let base: Vec<&str> = flags.iter().map(String::as_str).collect();
    let (obj, obj_frame) = (src.join(format!("{x}.o")), src.join(format!("{x}_frame.o")));
    crate::embed::compile(&ll, &obj, target, Code::Generated, &base).map_err(fail)?;
    let trace = library::defines(library::TRACE);
    let frame_flags: Vec<&str> = base.iter().copied().chain(trace.iter().map(String::as_str)).collect();
    crate::embed::compile(&c, &obj_frame, target, Code::Frame, &frame_flags).map_err(fail)?;
    let mut objects = vec![obj, obj_frame];
    if let Role::Reference { c: text } = &k.role {
        let reference = src.join(format!("{x}_c.c"));
        write(&reference, text)?;
        let names = library::reference_names(k);
        let defines = library::defines(names.iter().map(|(a, b)| (a.as_str(), b.as_str())));
        let reference_flags: Vec<&str> =
            base.iter().copied().chain(library::REFERENCE_FLAGS).chain(defines.iter().map(String::as_str)).collect();
        let obj_c = src.join(format!("{x}_c.o"));
        crate::embed::compile(&reference, &obj_c, target, Code::Generated, &reference_flags).map_err(fail)?;
        objects.push(obj_c);
    }
    Ok(objects)
}

/// `--import`: Tabelle und Bericht aus den Protokollen.
fn import(args: &Args) -> bool {
    let read = |path: &str| std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    let logs = (|| -> Result<Logs, String> {
        Ok(Logs {
            bench: args.files.iter().map(|f| read(f)).collect::<Result<_, _>>()?,
            looped: args.value("--loop").map(read).transpose()?,
            natives: args.value("--natives").map(read).transpose()?,
        })
    })();
    let Some(target) = args.value("--target") else {
        eprintln!("takt bench --import: `--target ZIEL` nennt den Abschnitt der Konfiguration (8.10)");
        return false;
    };
    let target = target_of(target).map_or(target, |(t, _)| t.name);
    match logs.and_then(|logs| suite::import(&logs)) {
        Ok(outcome) => finish(&outcome, args.value("--board").unwrap_or("unbenannt"), target, args),
        Err(e) => {
            eprintln!("takt bench --import: {e}");
            false
        }
    }
}

/// `--board`: messen, die Protokolle behalten, rechnen.
fn on_board(args: &Args) -> bool {
    let runs = match args.value("--runs").map(str::parse::<u64>) {
        None => 200,
        Some(Ok(n)) => n,
        Some(Err(e)) => {
            eprintln!("--runs: {e}");
            return false;
        }
    };
    let Some(mut board) = crate::board_of(args, "bench") else { return false };
    let measured = suite::run(board.as_mut(), runs, |line| eprintln!("  {line}"));
    let logs = match measured {
        Ok(logs) => logs,
        Err(e) => {
            eprintln!("takt bench: {e}");
            return false;
        }
    };
    if let Some(dir) = args.value("--logs")
        && let Err(e) = keep(&logs, Path::new(dir), board.placements())
    {
        eprintln!("{dir}: {e}");
        return false;
    }
    match suite::import(&logs) {
        Ok(outcome) => finish(&outcome, board.name(), board.target(), args),
        Err(e) => {
            eprintln!("takt bench: {e}");
            false
        }
    }
}

/// Die Protokolle eines Laufs, je Lage eines, fuer einen spaeteren Import.
fn keep(logs: &Logs, dir: &Path, placements: &[u32]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (text, shift) in logs.bench.iter().zip(placements) {
        write(&dir.join(format!("bench-{shift}.log")), text)?;
    }
    for (name, text) in [("loop.log", &logs.looped), ("natives.log", &logs.natives)] {
        if let Some(text) = text {
            write(&dir.join(name), text)?;
        }
    }
    println!("  Protokolle: {}", dir.display());
    Ok(())
}

/// Meldet die Tabelle und schreibt Bericht und Konfiguration.
fn finish(outcome: &Outcome, board: &str, target: &str, args: &Args) -> bool {
    for line in outcome.calibration.lines().into_iter().chain(outcome.kernel_lines()) {
        println!("{line}");
    }
    let tool = format!("takt {}", env!("CARGO_PKG_VERSION"));
    if let Some(path) = args.value("--conformance") {
        let text = takt_conformance::report::render(&outcome.report(board, target, &tool));
        if let Err(e) = std::fs::write(path, text) {
            eprintln!("{path}: {e}");
            return false;
        }
        println!("  Bericht: {path}");
    }
    if let Some(path) = args.value("--hardware") {
        let old = std::fs::read_to_string(path).unwrap_or_default();
        let values = takt_mir::hardware::measured_values(&outcome.target(target));
        match takt_mir::hardware::with_values(&old, target, &values, &crate::origin()) {
            Ok(text) => {
                if let Err(e) = std::fs::write(path, text) {
                    eprintln!("{path}: {e}");
                    return false;
                }
                println!("  Konfiguration: {path}");
            }
            Err(e) => {
                eprintln!("{path}: {e}");
                return false;
            }
        }
    }
    // 4.2: Rechnet eine Subnormal-Probe falsch, hat das Ziel FTZ oder DAZ
    // an; der Bericht steht, aber keine Zahl dieses Laufs gilt fuer Takt.
    if outcome.subnormal > 0 {
        eprintln!("takt bench: {} Subnormal-Proben falsch, FTZ/DAZ ist an (4.2)", outcome.subnormal);
    }
    outcome.subnormal == 0 && outcome.calibration.checks.iter().all(|c| c.measured_ps <= c.bound_ps)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}
