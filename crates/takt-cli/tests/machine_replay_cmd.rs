//! `takt replay --machine` (12.5, A2a): Scheibe ziehen, allein abspielen,
//! gegen den Golden der Maschine vergleichen.

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
fn the_slice_of_a_machine_replays_alone_against_the_golden() {
    let dir = std::env::temp_dir().join(format!("takt-machine-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (record, trace, slice) = (dir.join("run.trace"), dir.join("golden.trace"), dir.join("ctrl.trace"));
    let (record, trace, slice) =
        (record.to_str().expect("Pfad"), trace.to_str().expect("Pfad"), slice.to_str().expect("Pfad"));
    let out = takt(&["run", PROGRAM, "--ticks", "30", "--record", record, "--trace", trace]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let out =
        takt(&["replay", PROGRAM, "--record", record, "--machine", "ctrl", "--extract", slice, "--golden", trace]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("Scheibe von `ctrl` geschrieben") && text.contains("reproduziert"), "{text}");
    let written = std::fs::read_to_string(slice).expect("Scheibe");
    assert!(written.contains("#! maschine ctrl") && written.contains("pub plant x"), "{written}");

    let out = takt(&["replay", PROGRAM, "--record", slice, "--golden", trace]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = takt(&["replay", PROGRAM, "--record", slice]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("out level") && !text.contains("out lagged"), "nur die Zeilen von `ctrl`:\n{text}");

    let out = takt(&["replay", PROGRAM, "--record", slice, "--machine", "watch"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("gehoert zu `ctrl`"));
    std::fs::remove_dir_all(&dir).expect("aufraeumen");
}
