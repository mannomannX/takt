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
    let dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-verify-trace-{}", std::process::id()));
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

/// Ein Lauf mit Aufzeichnung und Trace in einem eigenen Verzeichnis.
fn recorded(name: &str) -> (PathBuf, String, String) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("takt-verify-trace-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (record, trace) = (dir.join("run.trace"), dir.join("golden.trace"));
    let out = takt(&[
        "run",
        PROGRAM,
        "--ticks",
        "12",
        "--record",
        record.to_str().expect("Pfad"),
        "--trace",
        trace.to_str().expect("Pfad"),
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = |p: &PathBuf| std::fs::read_to_string(p).expect("lesbar");
    (dir.clone(), text(&record), text(&trace))
}

/// `verify-trace` auf veraenderte Fassungen von Trace und Aufzeichnung: Exit
/// und die erste Zeile der Meldung.
fn verify(dir: &std::path::Path, trace: &str, record: &str) -> (bool, String) {
    let (t, r) = (dir.join("t.trace"), dir.join("r.trace"));
    std::fs::write(&t, trace).expect("Trace");
    std::fs::write(&r, record).expect("Aufzeichnung");
    let out = takt(&["verify-trace", t.to_str().expect("Pfad"), "--record", r.to_str().expect("Pfad")]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text.lines().next().unwrap_or_default().to_string())
}

/// 12.5 und trace.md T6: `time`- und `rec`-Metazeilen stehen ausserhalb der
/// Kette; eine geloeschte Zeile, eine in einen anderen Tick verschobene Zeile,
/// ein geaendertes Kettenende und eine Aufzeichnung ohne Kette fallen auf.
#[test]
fn only_the_lines_of_the_chain_count() {
    let (dir, record, trace) = recorded("edges");
    let mut lines: Vec<&str> = trace.lines().collect();
    let last_of_tick_3 = lines.iter().rposition(|l| l.starts_with("t=3 ")).expect("Tick 3") + 1;
    lines.splice(last_of_tick_3..last_of_tick_3, ["t=3 time took=1000 drift=0 slept=0", "t=3 rec daq1_ai3 5"]);
    let with_meta = lines.join("\n") + "\n";
    let (ok, first) = verify(&dir, &with_meta, &record);
    assert!(ok && first.contains("Kette stimmt"), "Metazeilen zaehlen nicht: {first}");
    let changed_meta = with_meta.replace("took=1000", "took=9999").replace("rec daq1_ai3 5", "rec daq1_ai3 6");
    assert!(verify(&dir, &changed_meta, &record).0, "geaenderte Metazeilen zaehlen nicht");

    let deleted = trace.replacen("t=1 out level 2\n", "", 1);
    let moved = deleted.replacen("t=2 pub plant x 3\n", "t=2 pub plant x 3\nt=2 out level 2\n", 1);
    let new_end = record
        .lines()
        .map(|l| if l.starts_with("#! kette ") { format!("#! kette {}", "0".repeat(64)) } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let no_chain: String = record.lines().filter(|l| !l.starts_with("#! kette ")).map(|l| format!("{l}\n")).collect();
    for (name, t, r, want) in [
        ("geloeschte Zeile", deleted.as_str(), record.as_str(), "die Hashkette weicht ab"),
        ("verschobener Tick", moved.as_str(), record.as_str(), "die Hashkette weicht ab"),
        ("Kettenende", trace.as_str(), new_end.as_str(), "die Hashkette weicht ab"),
        ("ohne Kette", trace.as_str(), no_chain.as_str(), "die Aufzeichnung traegt kein Kettenende (`#! kette`)"),
    ] {
        let (ok, first) = verify(&dir, t, r);
        assert!(!ok && first.contains(want), "{name}: {first}");
    }
    std::fs::remove_dir_all(&dir).expect("aufraeumen");
}
