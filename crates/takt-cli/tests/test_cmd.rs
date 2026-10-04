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
    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-test-{}", std::process::id()));
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
    // Ohne Inputs, sonst lehnt schon Pruefung 13 ab (FB-409).
    let out = takt(&["test", "corpus-try/106_machine_handler.takt"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("kein Szenario"));
}

/// **Ein Lauf ohne Aussage gilt nie als bestanden** (13.5, FB-395): Mit
/// einem Tick erreicht keines der Szenarien ein Verdikt.
#[test]
fn an_inconclusive_scenario_fails_the_run() {
    let out = takt(&["test", "corpus-try/38_scenarios.takt", "--ticks", "1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("INCONCLUSIVE"), "{stdout}");
    assert!(!out.status.success(), "INCONCLUSIVE bestand:\n{stdout}");
    assert!(stdout.contains("ohne Aussage"), "{stdout}");
}

/// **Unter `takt test` ist ein Input ohne `sim`-Quelle ein Fehler**
/// (Festlegung 6, 8.3, FB-409): Kein Szenario und kein Stimulus treibt ihn.
/// `takt sim` warnt nur, weil dort ein Stimulus ihn treiben darf.
#[test]
fn an_unsimulated_input_is_an_error_under_test_and_a_warning_under_sim() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-test-unsimulated");
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let file = dir.join("unsimulated.takt");
    std::fs::write(
        &file,
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         input  p   : int in 0..9 @ hw(\"d/p\")\n\
         output led : bool        @ hw(\"o/led\") with safe = false\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            led = p > 3\n\n\
         scenario \"idle\":\n    initial WAIT\n\n    state WAIT:\n        after 20 ms: -> DONE\n\n    state DONE:\n        enter:\n            verdict pass\n",
    )
    .expect("Programm");
    let path = file.to_str().expect("Pfad");
    let test = takt(&["test", path]);
    let stderr = String::from_utf8_lossy(&test.stderr);
    assert!(!test.status.success(), "takt test bestand:\n{stderr}");
    assert!(stderr.contains("error[SC-13]"), "{stderr}");
    let sim = takt(&["sim", path, "--ticks", "2"]);
    let stderr = String::from_utf8_lossy(&sim.stderr);
    assert!(stderr.contains("warning[SC-13]") && !stderr.contains("error[SC-13]"), "{stderr}");
}
