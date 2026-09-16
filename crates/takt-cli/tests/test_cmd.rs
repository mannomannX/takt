//! `takt test` (Referenz 13.6, 13.2): jedes Szenario ein Lauf, Verdikte je
//! Szenario, Coverage als Datei.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn every_scenario_runs_and_the_coverage_lands_in_a_file() {
    let out_dir = std::env::temp_dir().join(format!("takt-test-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).expect("Verzeichnis");
    let coverage = out_dir.join("coverage.csv");
    let out = takt(&["test", "corpus-try/38_scenarios.takt", "--coverage", coverage.to_str().expect("Pfad")]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("pressure_rises: PASS") && stdout.contains("stays_closed: PASS"), "{stdout}");
    assert!(stdout.contains("Coverage: Zustaende"), "{stdout}");
    let text = std::fs::read_to_string(&coverage).expect("Coverage-Datei");
    assert!(text.starts_with("# takt-coverage 1\n"), "{text}");
    assert!(text.contains("state,dut,OPEN,"), "{text}");
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn a_single_scenario_can_be_chosen() {
    let out = takt(&["test", "corpus-try/38_scenarios.takt", "--scenario", "stays closed"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("stays_closed: PASS") && !stdout.contains("pressure_rises"), "{stdout}");
}

#[test]
fn a_program_without_scenarios_is_refused() {
    let out = takt(&["test", "corpus-try/01_minimal.takt"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("kein Szenario"));
}
