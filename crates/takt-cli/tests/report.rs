//! Anforderungsreferenzen im Report (Referenz 13.4): `takt check --report`
//! nennt je `req`-ID ihre Stellen, `takt test` die Szenarien dazu.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn the_report_lists_every_requirement_with_its_sites() {
    let out = takt(&["check", "corpus-try/61_requirements.takt", "--report"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("Anforderungen:        3 (13.4)"), "{stdout}");
    assert!(stdout.contains("check corpus-try/61_requirements.takt:19 dut CLOSED.loop"), "{stdout}");
    assert!(stdout.contains("verify corpus-try/61_requirements.takt:61 stays_closed"), "{stdout}");
}

#[test]
fn takt_test_names_the_scenarios_that_ran_each_site() {
    let out = takt(&["test", "corpus-try/61_requirements.takt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("dut CLOSED.loop — pressure_rises, stays_closed"), "{stdout}");
    assert!(stdout.contains("pressure_rises RUN.S2.enter — pressure_rises"), "{stdout}");
}

/// Eine Stelle, die kein Szenario durchlief, ist eine Zeile — keine Warnung.
#[test]
fn a_site_without_a_scenario_is_reported_as_such() {
    let out = takt(&["test", "corpus-try/61_requirements.takt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dut LOCKED.loop — kein Szenario"), "{stdout}");
}

/// Ohne `req` im Programm entfaellt der Abschnitt.
#[test]
fn a_program_without_requirements_has_no_section() {
    let out = takt(&["check", "corpus-try/01_minimal.takt", "--report"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(!stdout.contains("Anforderungen:"), "{stdout}");
}
