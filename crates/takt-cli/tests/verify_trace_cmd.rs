//! `takt verify-trace` (12.5, A3): die Aufzeichnung traegt das Kettenende,
//! der Trace rechnet es nach.

use std::path::PathBuf;
use std::process::{Command, Output};

const PROGRAM: &str = "corpus-try/37_follows.takt";

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn the_recorded_chain_matches_the_trace_and_catches_a_change() {
    let dir = std::env::temp_dir().join(format!("takt-verify-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (record, trace) = (dir.join("run.trace"), dir.join("golden.trace"));
    let (record, trace) = (record.to_str().expect("Pfad"), trace.to_str().expect("Pfad"));
    let out = takt(&["run", PROGRAM, "--ticks", "12", "--record", record, "--trace", trace]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(std::fs::read_to_string(record).expect("Aufzeichnung").contains("#! kette "));

    let out = takt(&["verify-trace", trace, "--record", record]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Kette stimmt"));

    let text = std::fs::read_to_string(trace).expect("Trace").replacen("out level 7", "out level 8", 1);
    std::fs::write(trace, text).expect("schreiben");
    let out = takt(&["verify-trace", trace, "--record", record]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("weicht ab"));
    std::fs::remove_dir_all(&dir).expect("aufraeumen");
}
