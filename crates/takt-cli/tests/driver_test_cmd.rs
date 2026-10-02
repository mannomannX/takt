//! `takt driver-test`, Wirtsmodus (Referenz 13.8): ein Treiber in Takt
//! gegen sein Geraetemodell; bestanden, wenn jede Reaktion ausgeloest ist.

use std::path::PathBuf;
use std::process::{Command, Output};

const PROGRAM: &str = "crates/takt-bringup-esp32c6/programs/test_uart_c6.takt";
const STIMULUS: &str = "crates/takt-bringup-esp32c6/programs/test_uart_c6.stim.trace";

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn the_uart_driver_passes_with_every_reaction_triggered() {
    let out = takt(&["driver-test", PROGRAM, "--stim", STIMULUS]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("a_full_tx_fifo_faults_the_port_until_it_drains: PASS"), "{stdout}");
    assert!(stdout.contains("Treiber uart_port: Zustaende 2/2, Transitionen 2/2"), "{stdout}");
    // Die Break-Pruefung haengt an `RX_BREAK_IS_FAULT`; sie gehoert in eine
    // Kampagne und steht darum nur im Bericht.
    assert!(stdout.contains("Nicht erreicht:\n") && stdout.contains(": uart_port check\n"), "{stdout}");
}

#[test]
fn a_reaction_without_a_scenario_fails_the_driver() {
    // Ohne den Stimulus gibt niemand das Kommando `uart_reset`.
    let out = takt(&["driver-test", PROGRAM]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}");
    assert!(stdout.contains(": uart_port Transition RUN->RUN\n"), "{stdout}");
    assert!(stdout.contains("FAIL: 1 Reaktionen des Treibers von keinem Szenario ausgeloest (13.8)"), "{stdout}");
}

#[test]
fn a_program_without_a_driver_is_refused() {
    let out = takt(&["driver-test", "corpus-try/38_scenarios.takt"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("keine `driver machine`"));
}

/// **Ein Treiber-Crate in Rust auf dem Wirt** (13.8, FB-285): Das
/// Pruefgeraet verletzt den Vertrag mit Absicht; jeder Verstoss steht mit
/// seiner Zeile aus 12.6 im Bericht, und der Treiber besteht nicht. Ohne
/// clang uebersprungen.
#[test]
fn a_driver_crate_is_judged_by_the_edge_on_the_host() {
    if matches!(takt_llvm::toolchain::find(), takt_llvm::toolchain::Clang::Missing) {
        eprintln!("clang fehlt; uebersprungen");
        return;
    }
    let program = "crates/takt-conformance/tests/programs/driver_edge.takt";
    let out = takt(&["driver-test", "--crate", "crates/takt-driver-probe", program, "--ticks", "16"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    for line in [
        "Zeile 1  t=1 driver edge_a warped p",
        "Zeile 1  t=2 driver edge_a degraded window",
        "Zeile 6  t=4 runtime Driver o",
        "Zeile 2  t=5 driver edge_u degraded seq",
        "Zeile 2  t=7 driver edge_u degraded maxpt",
        "Zeile 5  t=9 stream pairs dropped=0 overflowed=0 malformed=1",
        "Zeile 2  t=11 driver edge_b degraded timestamp",
    ] {
        assert!(stdout.contains(line), "{line}:\n{stdout}");
    }
    assert!(stdout.contains("Verstoesse gegen den Treibervertrag (12.6)") && stdout.ends_with("FAIL\n"), "{stdout}");
}
