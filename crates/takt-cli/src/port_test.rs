//! `takt port-test` (12.11, 13.8; M11 Schritt 15): die Portpruefung.
//!
//! ```text
//! takt port-test --emit embed --form FORM [--target ZIEL] [--hardware DATEI.hw] [--drivers TYP] --out VERZEICHNIS
//! takt port-test --judge LOG [--port NAME --conformance DATEI]
//! ```
//!
//! `--emit embed` liefert das Pruefprogramm, wie `takt build --emit embed`
//! jedes Programm liefert: mit dem Praefix `port_test` und im Sim-Build, weil
//! seine Ausgaenge ohne Treiber in den Trace gehen; die Quelle liegt daneben.
//! Der Port faehrt es `takt_conformance::port::TICKS` Ticks in Echtzeit,
//! rechnet in seiner fremden Aufgabe die Last aus `takt_rt_core::port_test`
//! und schreibt alles in seine Senke. `--judge` urteilt ueber diese Ausgabe
//! und traegt das Urteil mit `--conformance` als `[port NAME]` in den
//! Konformitaetsbericht ein; ein Urteil desselben Ports ersetzt es.

use std::path::Path;

use takt_conformance::port;
use takt_conformance::report::{self, Report};

use crate::Args;

pub(crate) fn port_test(args: &Args) -> bool {
    match (args.value("--emit"), args.value("--judge")) {
        (Some("embed"), None) => emit_embed(args),
        (None, Some(log)) => judge(args, log),
        (Some(other), None) => {
            eprintln!("takt port-test --emit: nur `embed` (war: {other})");
            false
        }
        _ => {
            eprintln!("takt port-test: entweder `--emit embed` oder `--judge LOG`");
            false
        }
    }
}

/// `--emit embed`: das Pruefprogramm als Baustein.
fn emit_embed(args: &Args) -> bool {
    let Some(out) = args.value("--out") else {
        eprintln!("takt port-test --emit embed schreibt in ein Verzeichnis; `--out VERZEICHNIS` fehlt");
        return false;
    };
    let bytes = match job_bytes(args) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("takt port-test --emit embed: {e}");
            return false;
        }
    };
    let text = match port::source(bytes) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("takt port-test --emit embed: {e}");
            return false;
        }
    };
    println!("Nachricht je Job: {bytes} Byte");
    let source = Path::new(out).join(format!("{}.takt", port::PREFIX));
    if let Err(e) = std::fs::create_dir_all(out).and_then(|()| std::fs::write(&source, &text)) {
        eprintln!("{}: {e}", source.display());
        return false;
    }
    let mut values = args.values.clone();
    values.push(("--prefix".to_string(), port::PREFIX.to_string()));
    values.push(("--build".to_string(), "sim".to_string()));
    crate::build(&Args { flags: args.flags.clone(), files: vec![source.display().to_string()], values })
}

/// Die Nachricht eines Jobs: `--job-bytes N`, sonst auf einem Wirt nach dem
/// Durchsatz von `sha256` auf dieser Maschine gemessen, auf einem Board die
/// Laenge des Korpusprogramms (M11 Schritt 18c).
fn job_bytes(args: &Args) -> Result<u64, String> {
    if let Some(n) = args.value("--job-bytes") {
        return n.parse::<u64>().map_err(|e| format!("--job-bytes {n}: {e}"));
    }
    let target = args.value("--target").and_then(crate::resolve_target);
    let hosted = target
        .is_none_or(|t| matches!(t.class, takt_llvm::target::Class::Linux64 | takt_llvm::target::Class::Linux32F64));
    Ok(if hosted { port::job_bytes_for(sha256_bytes_per_ms()) } else { port::JOB_BYTES })
}

/// Wie viele Byte `sha256` auf dieser Maschine je Millisekunde hasht; das
/// Beste aus fuenf Laeufen ueber 1 MiB, damit eine Stoerung nicht kuerzt.
fn sha256_bytes_per_ms() -> u64 {
    let data = vec![0x5au8; 1 << 20];
    let best = (0..5)
        .map(|_| {
            let started = std::time::Instant::now();
            std::hint::black_box(takt_native::sha256::sha256(std::hint::black_box(&data)));
            started.elapsed().as_nanos().max(1)
        })
        .min()
        .unwrap_or(1);
    u64::try_from((1u128 << 20) * 1_000_000 / best).unwrap_or(u64::MAX)
}

/// `--judge LOG`: das Urteil, und mit `--conformance` im Bericht.
/// `--program DATEI` nennt das Pruefprogramm, das der Port fuhr (die Quelle
/// aus `--emit embed`); ohne es das Korpusprogramm.
fn judge(args: &Args, log: &str) -> bool {
    let text = match std::fs::read(log) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            eprintln!("{log}: {e}");
            return false;
        }
    };
    let program = match args.value("--program") {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{path}: {e}");
                return false;
            }
        },
        None => port::PROGRAM.to_string(),
    };
    let verdict = port::judge(&text, &program);
    print!("{verdict}");
    if let Some(path) = args.value("--conformance") {
        let Some(name) = args.value("--port") else {
            eprintln!("--conformance: `--port NAME` fehlt; er nennt den Abschnitt `[port NAME]`");
            return false;
        };
        if let Err(e) = record(path, verdict.entry(name, &report::today())) {
            eprintln!("{path}: {e}");
            return false;
        }
        println!("  Bericht: {path}");
    }
    verdict.passed()
}

/// Traegt das Urteil in den Bericht unter `path` ein; ohne Bericht beginnt
/// einer mit Datum und Werkzeug.
fn record(path: &str, entry: report::PortEntry) -> Result<(), String> {
    let mut r = match std::fs::read_to_string(path) {
        Ok(text) => report::parse(&text).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut r = Report { format_version: report::FORMAT_VERSION, ..Report::default() };
            r.run.date = report::today();
            r.run.tool = format!("takt {}", env!("CARGO_PKG_VERSION"));
            r
        }
        Err(e) => return Err(e.to_string()),
    };
    match r.ports.iter_mut().find(|p| p.name == entry.name) {
        Some(old) => *old = entry,
        None => r.ports.push(entry),
    }
    std::fs::write(path, report::render(&r)).map_err(|e| e.to_string())
}
