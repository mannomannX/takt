//! `takt port-test` (12.11, 13.8; M11 Schritt 15): das Pruefprogramm als
//! Baustein und das Urteil im Konformitaetsbericht.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use takt_llvm::toolchain::find;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// **Das Pruefprogramm kommt als Baustein**, wie `takt build --emit embed`
/// jedes Programm liefert: mit dem Praefix `port_test`, die Quelle daneben.
#[test]
fn the_test_program_comes_as_a_building_block() {
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_none() {
        return;
    }
    let dir = scratch("takt-port-test-embed");
    let out = dir.to_str().expect("Pfad");
    let run = takt(&["port-test", "--emit", "embed", "--form", "interrupt", "--target", "thumbv7em", "--out", out]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let source = std::fs::read_to_string(dir.join("port_test.takt")).expect("Quelle");
    assert_eq!(source, takt_conformance::port::PROGRAM);
    for file in ["libport_test.a", "port_test.h", "port_test.rs", "port_test.manifest"] {
        assert!(dir.join(file).is_file(), "{file} fehlt");
    }
}

/// **Die Nachricht eines Jobs ist je Ziel bemessen** (M11 Schritt 18c): Auf
/// einem Wirt rechnet `sha256` ueber 4 KiB in Mikrosekunden, kein Job liefe
/// ueber eine Tickgrenze. `--job-bytes` setzt die Laenge in die gelieferte
/// Quelle, und das Urteil ueber `--program` erkennt sie als Pruefprogramm;
/// ein anderes Programm erkennt es nicht.
#[test]
fn the_message_of_a_job_is_sized_for_the_target() {
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_none() {
        return;
    }
    let dir = scratch("takt-port-test-sized");
    let out = dir.join("embed");
    let out = out.to_str().expect("Pfad");
    let run = takt(&["port-test", "--emit", "embed", "--form", "logical", "--job-bytes", "65536", "--out", out]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let source = std::fs::read_to_string(dir.join("embed").join("port_test.takt")).expect("Quelle");
    assert_eq!(Ok(source.clone()), takt_conformance::port::source(65536));
    assert_eq!(takt_conformance::port::job_bytes(&source), Ok(65536));
    let log = dir.join("port.log");
    std::fs::write(&log, "takt auf wirt\ntakt end\n").expect("Protokoll");
    let program = dir.join("embed").join("port_test.takt");
    let judged =
        takt(&["port-test", "--judge", log.to_str().expect("Pfad"), "--program", program.to_str().expect("Pfad")]);
    let text = String::from_utf8_lossy(&judged.stdout);
    assert!(!text.contains("nicht das Pruefprogramm"), "{text}");
    let other = dir.join("anders.takt");
    std::fs::write(&other, source.replacen("wait 10 ms", "wait 20 ms", 1)).expect("Quelle");
    let judged =
        takt(&["port-test", "--judge", log.to_str().expect("Pfad"), "--program", other.to_str().expect("Pfad")]);
    assert!(String::from_utf8_lossy(&judged.stdout).contains("nicht das Pruefprogramm"));
    let refused = takt(&["port-test", "--emit", "embed", "--form", "logical", "--job-bytes", "4100", "--out", out]);
    assert!(!refused.status.success(), "kein Vielfaches von 64");
}

/// **Die Pollform weist das Pruefprogramm ab** (12.11): Ihr Schritt
/// unterbricht keinen Job, und die Jobs des Pruefprogramms reichen mit der
/// Runde des F401 ueber den Tick. Die Portpruefung gilt den Formen mit
/// unterbrechbarem Job-Kontext.
#[test]
fn the_poll_form_refuses_the_test_program() {
    let dir = scratch("takt-port-test-poll");
    let out = dir.to_str().expect("Pfad");
    let run = takt(&[
        "port-test",
        "--emit",
        "embed",
        "--form",
        "poll",
        "--target",
        "thumbv7em",
        "--hardware",
        "corpus-try/hw/stm32f401.hw",
        "--out",
        out,
    ]);
    assert!(!run.status.success(), "die Pollform baut das Pruefprogramm nicht");
    let e = String::from_utf8_lossy(&run.stderr);
    assert!(e.contains("unterbricht der Schritt keinen Job") && e.contains("`sha256`"), "{e}");
}

/// **Das Urteil steht im Konformitaetsbericht** (13.8): Eine Ausgabe, die
/// nichts belegt, faellt durch und steht als `[port NAME]` darin; ein zweites
/// Urteil desselben Ports ersetzt das erste, eines anderen kommt dazu.
#[test]
fn a_verdict_goes_into_the_conformance_report() {
    let dir = scratch("takt-port-test-judge");
    let log = dir.join("port.log");
    std::fs::write(&log, "takt auf wirt\r\ntakt end\n").expect("Protokoll");
    let report = dir.join("board.report");
    let (log, path) = (log.to_str().expect("Pfad"), report.to_str().expect("Pfad"));
    let judged = |port: &str| takt(&["port-test", "--judge", log, "--port", port, "--conformance", path]);
    let run = judged("interrupt");
    assert!(!run.status.success(), "eine leere Ausgabe besteht nicht");
    let text = String::from_utf8_lossy(&run.stdout);
    assert!(text.contains("durchgefallen") && text.contains("fehlt"), "{text}");
    judged("interrupt");
    judged("embassy");
    let r = takt_conformance::report::parse(&std::fs::read_to_string(&report).expect("Bericht")).expect("lesbar");
    let names: Vec<(&str, bool)> = r.ports.iter().map(|p| (p.name.as_str(), p.passed)).collect();
    assert_eq!(names, [("interrupt", false), ("embassy", false)]);
    let unnamed = takt(&["port-test", "--judge", log, "--conformance", path]);
    assert!(String::from_utf8_lossy(&unnamed.stderr).contains("--port NAME"));
}
