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
