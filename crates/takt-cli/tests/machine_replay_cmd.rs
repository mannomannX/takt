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
    let dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-machine-replay-{}", std::process::id()));
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

/// 12.5: `--golden` vergleicht mit `--machine` nur die Zeilen dieser
/// Maschine. Eine geaenderte Zeile von `ctrl` scheitert und nennt den Tick,
/// eine geaenderte Zeile von `watch` nicht; eine unbekannte Maschine ist ein
/// Fehler mit Namen.
#[test]
fn the_golden_of_a_machine_compares_only_its_own_lines() {
    let dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-machine-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (record, trace, changed) = (dir.join("run.trace"), dir.join("golden.trace"), dir.join("changed.trace"));
    let (record, trace, changed) =
        (record.to_str().expect("Pfad"), trace.to_str().expect("Pfad"), changed.to_str().expect("Pfad"));
    let out = takt(&["run", PROGRAM, "--ticks", "30", "--record", record, "--trace", trace]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let golden = std::fs::read_to_string(trace).expect("Golden");
    let replay = |text: String| {
        std::fs::write(changed, text).expect("Golden");
        takt(&["replay", PROGRAM, "--record", record, "--machine", "ctrl", "--golden", changed])
    };

    assert!(golden.contains("t=4 out level 7\n"), "{golden}");
    let out = replay(golden.replacen("t=4 out level 7\n", "t=4 out level 8\n", 1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}");
    assert!(
        stdout.contains("t=4 out level 7") && stdout.contains("t=4 out level 8"),
        "der Unterschied mit Tick:\n{stdout}"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("Trace weicht ab"));

    let watch = golden.lines().find(|l| l.contains(" out lagged ")).expect("Zeile von watch");
    let out = replay(golden.replacen(watch, &format!("{watch}9"), 1));
    assert!(out.status.success(), "eine Zeile von `watch` zaehlt nicht:\n{}", String::from_utf8_lossy(&out.stdout));

    let out = takt(&["replay", PROGRAM, "--record", record, "--machine", "gibtsnicht"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Maschine `gibtsnicht` gibt es nicht"));
    std::fs::remove_dir_all(&dir).expect("aufraeumen");
}

/// 12.5 (SEM2-045): Der Kopf einer Aufzeichnung traegt s0 der
/// `persist`-Variablen, und `takt replay` beginnt mit genau diesem Speicher:
/// Ein Lauf mit gefuelltem Speicher spielt aus der Aufzeichnung allein gleich ab.
#[test]
fn a_replay_begins_with_the_recorded_store() {
    use takt_interp::nvm::Nvm;
    use takt_interp::record::{Header, Recording};
    use takt_interp::{RunOptions, Trace, Value};
    let program = "corpus-try/35_persist.takt";
    let src = std::fs::read_to_string(root().join(program)).expect("Programm");
    let p = takt_sema::compile(&src, &takt_sema::Options::default()).program.expect("uebersetzt");
    let device = p.machines.iter().find(|m| m.name == "device").expect("Maschine");
    let mut store = Nvm::new();
    store.put(device.persist[0].type_hash, Value::Int(700));
    let options = RunOptions { ticks: 3, nvm: store.clone(), ..Default::default() };
    let first = takt_interp::run(&p, &Trace::default(), &options).expect("Lauf");
    let golden = first.trace.render();
    assert!(golden.contains("t=0 out count 700\n"), "der Speicher wirkt: {golden}");

    let dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-replay-store-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let header = Header::of(&p, None, &first.start_params, 3).with_store(&p, &store);
    let recording = Recording { header, inputs: Trace::default() }.seal(&first.trace);
    let (record, trace) = (dir.join("run.trace"), dir.join("golden.trace"));
    std::fs::write(&record, recording.render()).expect("Aufzeichnung");
    std::fs::write(&trace, &golden).expect("Golden");
    assert!(recording.render().contains("#! persist "), "{}", recording.render());

    let out = takt(&[
        "replay",
        program,
        "--record",
        record.to_str().expect("Pfad"),
        "--golden",
        trace.to_str().expect("Pfad"),
    ]);
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).expect("aufraeumen");
}
