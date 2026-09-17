//! Board 2, ESP32-C6: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/esp32c6.md Schritt 5).
//!
//! **Laeuft nur mit Board.** `TAKT_ESP32C6_PORT` nennt den seriellen Port
//! des USB-Serial-JTAG (`COM4`, `/dev/ttyACM0`); ohne ihn wird der Test
//! uebersprungen, wie die LLVM-Tests ohne clang. Je Programm: das
//! Bring-up-Binary mit `TAKT_PROGRAM` und `TAKT_TICKS` bauen, mit
//! `probe-rs` flashen, zuruecksetzen, den Trace bis `takt end` lesen und
//! wie im Linux-Vergleich pruefen.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use takt_conformance::compare;
use takt_mir::program::Program;

const TICKS: u64 = 60;

/// Der Differentialkorpus ohne das, was der MCU-Rahmen nicht traegt:
/// Systemkanaele (32, 34), geplante Ausgaben (28) und Jobs (40).
const KORPUS: &[&str] = &[
    "01_minimal.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
    "18_blocks.takt",
    "19_faults.takt",
    "20_native.takt",
    "21_fault_targets.takt",
    "22_faulted_outputs.takt",
    "23_patterns.takt",
    "24_send_has.takt",
    "25_format.takt",
    "26_samples.takt",
    "27_every.takt",
    "33_enum_param.takt",
    "35_persist.takt",
    "36_int_units.takt",
    "37_follows.takt",
    "39_sha256.takt",
    "41_tunables.takt",
    "42_map.takt",
    "43_sent.takt",
    "46_matrices.takt",
    "47_monitors.takt",
    "49_record_streams.takt",
    "50_clause_words.takt",
    "51_text_into_bytes.takt",
    "52_padding_fields.takt",
    "53_stream_kinds.takt",
    "54_inout.takt",
    "55_frames_with_bytes.takt",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus(name: &str) -> Program {
    let path = root().join("corpus-try").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// Baut das Bring-up-Binary fuer `name` und liefert den Pfad des ELF.
fn build(name: &str) -> Result<PathBuf, String> {
    let manifest = root().join("crates/takt-bringup-esp32c6/Cargo.toml");
    let program = root().join("corpus-try").join(name);
    let out = Command::new("cargo")
        .args(["build", "--release", "--target", "riscv32imac-unknown-none-elf", "--bin", "takt"])
        .arg("--message-format=json-render-diagnostics")
        .arg("--manifest-path")
        .arg(&manifest)
        .env("TAKT_PROGRAM", &program)
        .env("TAKT_TICKS", TICKS.to_string())
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    // Die JSON-Meldungen nennen das Binary; ein eigener Parser fuer eine
    // Zeichenkette in einer Zeile ist kuerzer als eine Abhaengigkeit.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let rest = &l[l.find("\"executable\":\"")? + "\"executable\":\"".len()..];
            Some(rest[..rest.find('"')?].replace("\\\\", "\\"))
        })
        .next_back()
        .map(PathBuf::from)
        .ok_or_else(|| "cargo meldete kein Binary".to_string())
}

fn probe_rs(args: &[&str]) -> Result<(), String> {
    let out = Command::new("probe-rs").args(args).output().map_err(|e| format!("probe-rs: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
}

/// Setzt das Board zurueck und liest den Trace bis `takt end`.
///
/// Kommt gar nichts, nicht einmal der ROM-Bootloader, hat der
/// USB-Serial-JTAG nach dem Flashen noch nicht wieder angelegt; ein
/// zweiter Reset genuegt dann. Ein Lauf, der etwas sagt und nicht endet,
/// wird nicht wiederholt — das waere ein Befund.
fn capture(port: &str) -> Result<String, String> {
    let mut serial = serialport::new(port, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| format!("{port}: {e}"))?;
    for attempt in 0..2 {
        probe_rs(&["reset", "--chip", "esp32c6"])?;
        let start = Instant::now();
        let mut text = String::new();
        let mut buf = [0u8; 4096];
        while start.elapsed() < Duration::from_secs(30) {
            match serial.read(&mut buf) {
                Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(format!("{port}: {e}")),
            }
            if text.contains("takt end") {
                return Ok(text);
            }
        }
        if !text.is_empty() || attempt == 1 {
            return Err(format!("kein `takt end` binnen 30 s; gelesen:\n{text}"));
        }
    }
    unreachable!("zwei Versuche")
}

/// Die Namen der Ausgaenge, die ein Trace nennt.
fn output_names(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out").then(|| w.next().map(String::from))?
        })
        .collect()
}

#[test]
fn the_board_agrees_with_the_interpreter() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    // `TAKT_ESP32C6_ONLY=42_map.takt` fuer einen einzelnen Fall.
    let only = std::env::var("TAKT_ESP32C6_ONLY").ok();
    let mut failed = Vec::new();
    for name in KORPUS.iter().filter(|n| only.as_deref().is_none_or(|o| o == **n)) {
        let p = corpus(name);
        let board = match build(name).and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        }) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein Lauf auf dem Board:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let missing: Vec<String> = output_names(&interpreted).difference(&output_names(&board)).cloned().collect();
        let diffs = compare(&interpreted, &board);
        if !missing.is_empty() || !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen, fehlende Ausgaenge {missing:?}\n{}\n--- Interpreter ---\n{}\n--- Board ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                board.lines().filter(|l| l.starts_with("t=")).take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        eprintln!("{name}: {} Abweichungen", diffs.len());
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
