//! `takt-trace-serial`: den Hardwarelauf gegen den Interpreter halten.
//!
//! ```text
//! takt-trace-serial PROGRAMM.takt --port COM3 [--ticks N] [--baud 115200]
//!                                 [--timeout S] [--save DATEI.trace]
//! ```
//!
//! **Das ist der M5-Exit in einem Kommando.** Satz 9.4.4 sagt, der
//! Interpreter sei die Spezifikation und ein abweichender Codegen falsch.
//! Auf dem Wirt wird das geprueft (`takt-conformance`); fuer die MCU stand
//! bis hierher nur, dass sie *uebersetzt* und *linkt*. Ob sie dasselbe
//! *rechnet*, hat niemand gemessen.
//!
//! Der Ablauf ist kurz, weil beide Haelften schon da waren: `takt_interp`
//! rechnet den Soll-Trace, das Board sendet seinen ueber UART, und
//! `takt_conformance::run::compare` vergleicht — dieselbe Funktion, die
//! Interpreter gegen erzeugten Code haelt. Neu ist nur das Lesen.
//!
//! Exit-Code 1 bei jedem Unterschied und bei einem stummen Board.

use std::process::ExitCode;
use std::time::Duration;

use takt_interp::RunOptions;

const USAGE: &str = "takt-trace-serial PROGRAMM.takt --port NAME [--ticks N] [--baud N] [--timeout S] [--save DATEI]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Ein Wert nach `--name`.
fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(String::as_str)
}

fn run(args: &[String]) -> Result<bool, String> {
    let Some(path) = args.first().filter(|a| !a.starts_with("--")) else {
        return Err(format!("{USAGE}\n\nDie Programmdatei fehlt."));
    };
    let Some(port) = value(args, "--port") else {
        let list = takt_trace_serial::available();
        let seen = if list.is_empty() { "keine gefunden".to_string() } else { format!("gesehen: {}", list.join(", ")) };
        return Err(format!("{USAGE}\n\n--port fehlt ({seen})."));
    };
    let ticks: u64 = value(args, "--ticks").unwrap_or("200").parse().map_err(|e| format!("--ticks: {e}"))?;
    let baud: u32 = value(args, "--baud").unwrap_or("115200").parse().map_err(|e| format!("--baud: {e}"))?;
    let seconds: f32 = value(args, "--timeout").unwrap_or("30").parse().map_err(|e| format!("--timeout: {e}"))?;

    // **Der Soll-Trace zuerst.** Uebersetzt das Programm nicht, ist der
    // Lauf sinnlos — und ein Fehler hier ist billiger als einer nach
    // dreissig Sekunden Warten am Board.
    let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Hw, profile: None };
    let checked = takt_sema::compile(&src, &options);
    let map = takt_diag::SourceMap::single(path, src.as_str());
    for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
        eprintln!("{}", map.render(d));
    }
    let program = checked.program.ok_or_else(|| format!("{path}: uebersetzt nicht"))?;

    let run_options = RunOptions { ticks, profile: None, order_seed: None };
    let expected = takt_interp::run(&program, &takt_interp::Trace::default(), &run_options)
        .map_err(|e| format!("{path}: {e:?}"))?;
    let want = expected.trace.render();
    println!("{path}: {ticks} Ticks erwartet, lese {port} mit {baud} Baud");

    let mut serial = takt_trace_serial::open(port, baud).map_err(|e| e.to_string())?;
    let got = takt_trace_serial::read_until(&mut *serial, ticks, Duration::from_secs_f32(seconds))
        .map_err(|e| e.to_string())?;

    for line in got.preamble() {
        println!("  {line}");
    }
    if let Some(out) = value(args, "--save") {
        std::fs::write(out, got.text()).map_err(|e| format!("{out}: {e}"))?;
        println!("  Trace geschrieben: {out}");
    }

    report(&want, &got)
}

/// Vergleicht und meldet.
fn report(want: &str, got: &takt_trace_serial::Reader) -> Result<bool, String> {
    let have = got.text();
    let differences = takt_conformance::run::compare(want, &have);
    let last = got.last_tick().unwrap_or(0);

    // **Ein Lauf ohne gemeinsame Outputs ist kein bestandener Lauf.**
    // `compare` vergleicht, was beide Seiten melden; meldet das Board
    // nichts Vergleichbares, ist die Liste leer — und das saehe aus wie
    // Uebereinstimmung. Der haeufigste Grund ist ein Trace ohne `t=`.
    let comparable = have.lines().filter(|l| l.contains(" out ")).count();
    if comparable == 0 {
        return Err(format!(
            "kein vergleichbarer Trace: {} Zeilen gelesen, keine der Form `t=<n> out <name> <wert>`.\n\
             Sendet das Board den Latch? `takt_mcu_dump` muss gerufen werden.",
            have.lines().count()
        ));
    }

    if differences.is_empty() {
        println!("\nGleich bis Tick {last}: {comparable} Ausgabezeilen, kein Unterschied.");
        return Ok(true);
    }

    println!("\n{} Unterschiede bis Tick {last}:", differences.len());
    for d in differences.iter().take(20) {
        println!("  t={} {}: Interpreter {}, Board {}", d.tick, d.output, d.interpreter, d.native);
    }
    if differences.len() > 20 {
        println!("  … und {} weitere", differences.len() - 20);
    }
    Ok(false)
}
