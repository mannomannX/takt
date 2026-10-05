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

/// Ein Programm in einem eigenen Verzeichnis unter dem Testverzeichnis.
fn program(dir: &str, text: &str) -> PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-test-{dir}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let file = dir.join("program.takt");
    std::fs::write(&file, text).expect("Programm");
    file
}

/// Ein irreversibler Output (12.7), der nur brennt, wenn `ready` steht;
/// `burns` deckt ihn ab, `never_ready` nicht.
const FUSE: &str = r#"system:
    language = 1
    tick     = 1 ms

input  ready     : bool @ hw("otp/ready") with max_age = 10 ms
output fuse      : bool @ hw("otp/fuse")  with safe = false, irreversible = true
output ready_sim : bool @ sim("otp/ready") with safe = false

machine m:
    initial WAIT

    state WAIT:
        when ready.or(false): -> BURN

    state BURN:
        sequence:
            expect ready.or(false), "Brennspannung steht"
            fuse = true

scenario "never ready" every 1 ms:
    initial RUN

    state RUN:
        sequence:
            ready_sim = false
            wait 3 ms
            verdict pass "nie gebrannt"

scenario "burns" every 1 ms:
    initial RUN

    state RUN:
        sequence:
            ready_sim = true
            wait 5 ms
            verdict pass "gebrannt"
"#;

/// **12.7: `takt test` ist FAIL, solange ein irreversibler Output in keinem
/// Szenario-Lauf geschrieben wurde** - hier, weil nur das Szenario laeuft,
/// das ihn nie schreibt; die Stelle steht unter `Nicht erreicht` mit Datei
/// und Zeile (13.2), und `--out` schreibt den Trace des Szenarios (13.6).
#[test]
fn an_uncovered_irreversible_output_fails_the_run() {
    let file = program("fuse", FUSE);
    let path = file.to_str().expect("Pfad");
    let all = takt(&["test", path]);
    let stdout = String::from_utf8_lossy(&all.stdout);
    assert!(all.status.success(), "beide Szenarien zusammen decken `fuse` ab:\n{stdout}");
    let out_dir = file.with_file_name("traces");
    let one = takt(&["test", path, "--scenario", "never_ready", "--out", out_dir.to_str().expect("Pfad")]);
    let stdout = String::from_utf8_lossy(&one.stdout);
    assert!(!one.status.success(), "ohne Abdeckung bestanden:\n{stdout}");
    assert!(stdout.contains("never_ready: PASS"), "das Szenario selbst besteht:\n{stdout}");
    assert!(stdout.contains("FAIL: irreversibler Output `fuse` von keinem Szenario abgedeckt (12.7)"), "{stdout}");
    let gaps = stdout.split_once("Nicht erreicht:\n").map(|(_, rest)| rest).unwrap_or_default();
    assert!(gaps.contains(&format!("  {path}:13: m Transition WAIT->BURN\n")), "{stdout}");
    assert!(gaps.contains(&format!("  {path}:15: m Zustand BURN\n")), "{stdout}");
    let trace = std::fs::read_to_string(out_dir.join("never_ready.trace")).expect("Trace je Szenario");
    assert!(trace.contains("never_ready") && !trace.contains("burns"), "{trace}");
    let _ = std::fs::remove_dir_all(file.parent().expect("Verzeichnis"));
}

/// 13.2: Jede Stelle unter `Nicht erreicht` traegt die Zeile, an der sie im
/// Quelltext steht - auch ein Abschnitt einer Sequenz (6.2), den der
/// Uebersetzer als Zustand anlegt. Zeile 1 ist `system:` und kann keine sein.
#[test]
fn every_unreached_place_names_its_line() {
    let file = program("fuse-lines", FUSE);
    let path = file.to_str().expect("Pfad");
    let out = takt(&["test", path, "--scenario", "never ready"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let gaps: Vec<&str> = stdout
        .split_once("Nicht erreicht:\n")
        .map(|(_, rest)| rest.lines().take_while(|l| l.starts_with("  ")).collect())
        .unwrap_or_default();
    assert!(!gaps.is_empty(), "{stdout}");
    let without_line: Vec<&&str> = gaps.iter().filter(|l| l.starts_with(&format!("  {path}:1: "))).collect();
    assert!(without_line.is_empty(), "Stellen ohne Zeile: {without_line:?}");
    let _ = std::fs::remove_dir_all(file.parent().expect("Verzeichnis"));
}

/// Beide Schreibweisen des Szenarionamens (13.6), und ein unbekannter Name
/// ist kein `Programm ohne Szenario`, sondern nennt sich und die vorhandenen.
#[test]
fn a_scenario_is_chosen_by_either_spelling_and_an_unknown_one_is_named() {
    for name in ["stays closed", "stays_closed"] {
        let out = takt(&["test", "corpus-try/38_scenarios.takt", "--scenario", name]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && stdout.contains("stays_closed: PASS"), "{name}: {stdout}");
        assert!(!stdout.contains("pressure_rises"), "{name}: {stdout}");
    }
    let out = takt(&["test", "corpus-try/38_scenarios.takt", "--scenario", "gibtsnicht"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("kein Szenario `gibtsnicht`; vorhanden: pressure_rises, stays_closed"),
        "eigene Meldung: {stderr}"
    );
}

/// 12.7: auch ohne jedes Szenario ist ein irreversibler Output nicht
/// abgedeckt, und der Lauf-Header einer Aufzeichnung nennt ihn.
#[test]
fn without_scenarios_an_irreversible_output_is_named_and_recorded() {
    let text = FUSE.split("scenario \"never ready\"").next().expect("Programmteil");
    let file = program("fuse-alone", text);
    let path = file.to_str().expect("Pfad");
    let out = takt(&["test", path]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("kein Szenario (13.6)"), "{stderr}");
    assert!(stdout.contains("FAIL: irreversibler Output `fuse` von keinem Szenario abgedeckt (12.7)"), "{stdout}");
    let record = file.with_file_name("run.trace");
    let run = takt(&["run", path, "--ticks", "3", "--record", record.to_str().expect("Pfad")]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let text = std::fs::read_to_string(&record).expect("Aufzeichnung");
    assert_eq!(text.lines().filter(|l| l.starts_with("#! irreversibel")).collect::<Vec<_>>(), ["#! irreversibel fuse"]);
    let _ = std::fs::remove_dir_all(file.parent().expect("Verzeichnis"));
}

/// 8.3: auch ein `hw`-Strom ohne `sim`-Quelle ist unter `takt test` ein
/// Fehler und unter `takt sim` eine Warnung (Pruefung 13).
#[test]
fn an_unsimulated_stream_is_an_error_under_test_and_a_warning_under_sim() {
    let file = program(
        "stream",
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         input  rx  : stream<u8> @ hw(\"uart/rx\") with max_rate = 100 Hz, capacity = 4\n\
         output led : bool       @ hw(\"o/led\")   with safe = false\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        on rx as b:\n            led = b.data > 3\n\n\
         scenario \"idle\":\n    initial WAIT\n\n    state WAIT:\n        after 20 ms: -> DONE\n\n    state DONE:\n        enter:\n            verdict pass\n",
    );
    let path = file.to_str().expect("Pfad");
    let test = takt(&["test", path]);
    let stderr = String::from_utf8_lossy(&test.stderr);
    assert!(!test.status.success(), "takt test bestand:\n{stderr}");
    assert!(stderr.contains("error[SC-13]") && stderr.contains("rx"), "{stderr}");
    let sim = takt(&["sim", path, "--ticks", "2"]);
    let stderr = String::from_utf8_lossy(&sim.stderr);
    assert!(stderr.contains("warning[SC-13]") && !stderr.contains("error[SC-13]"), "{stderr}");
    let _ = std::fs::remove_dir_all(file.parent().expect("Verzeichnis"));
}
