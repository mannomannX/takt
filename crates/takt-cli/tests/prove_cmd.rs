//! `takt prove --export` (13.3): Transitionssystem als SMT-LIB2 mit BMC
//! und Induktionsschritt; die Reichweite steht im Bericht.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn the_export_writes_both_queries_and_names_the_reach() {
    let out_dir = std::env::temp_dir().join(format!("takt-prove-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).expect("Verzeichnis");
    let file = out_dir.join("modell.smt2");
    let out =
        takt(&["prove", "corpus-try/06_test_harness.takt", "--export", file.to_str().expect("Pfad"), "--depth", "2"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("1 Beweisziele"), "{stdout}");
    assert!(stdout.contains("Reichweite: `reaches_target` nicht kodiert"), "{stdout}");
    let text = std::fs::read_to_string(&file).expect("Export");
    assert!(text.contains("; BMC") && text.contains("; Induktionsschritt"), "{text}");
    assert!(text.contains("; Eigenschaft `pump_off_when_high`"), "{text}");
    // Eine Eigenschaft und eine Pruefstelle (B3), je BMC und Induktion.
    assert_eq!(text.matches("(check-sat)").count(), 4, "{text}");
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn a_missing_solver_is_named() {
    let out = takt(&["prove", "corpus-try/01_minimal.takt", "--solver", "takt-kein-solver"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("kein Solver"), "{}", String::from_utf8_lossy(&out.stderr));
}
