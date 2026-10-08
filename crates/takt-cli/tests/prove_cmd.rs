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
    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-prove-{}", std::process::id()));
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
    // Eine Eigenschaft, die `check`-Stelle, zwei Endlichkeitsstellen (B3,
    // 4.2) und drei Lesestellen von `press` (3.5), je BMC und Induktion.
    assert!(text.contains("; Pruefstelle `fin`") && text.contains("; Pruefstelle `valid`"), "{text}");
    assert_eq!(text.matches("(check-sat)").count(), 14, "{text}");
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn a_missing_solver_is_named() {
    let out = takt(&["prove", "corpus-try/01_minimal.takt", "--solver", "takt-kein-solver"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("kein Solver"), "{}", String::from_utf8_lossy(&out.stderr));
}

/// Der Solver (13.3); fehlt er, scheitert der Test, es sei denn,
/// `TAKT_ALLOW_MISSING` erlaubt das Fehlen (FB-392).
fn solver() -> Option<()> {
    let found = Some(()).filter(|()| takt_prove::find().works());
    takt_testkit::require("solver", found, "`TAKT_SOLVER` setzen oder z3/cvc5 installieren")
}

/// Ein Programm in einem eigenen Verzeichnis unter dem Testverzeichnis.
fn program(name: &str, text: &str) -> PathBuf {
    let dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-prove-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let file = dir.join(format!("{name}.takt"));
    std::fs::write(&file, text).expect("Programm");
    file
}

/// Ein Zaehler: `bounded` gilt, `small` ist ab dem dritten Tick verletzt.
const COUNTER: &str = r#"system:
    language = 1
    tick     = 10 ms

input  go    : bool              @ hw("i/go")
output level : int in 0..10      @ hw("o/level") with safe = 0
output lamp  : bool              @ hw("o/lamp")  with safe = false

property bounded: always(level <= 10)
property small: always(level < 3)

machine m:
    var n : int in 0..10 = 0

    initial RUN

    state RUN:
        loop:
            if go and n < 10:
                n = n + 1
            level = n
            lamp = n > 5
"#;

/// Zwei Variablen, deren Summe immer 5 ist: Die Range-Pruefung von `level`
/// ist per Induktion unerreichbar, die von `b` bleibt offen.
const SUM: &str = r#"system:
    language = 1
    tick     = 10 ms

output level : int in 0..5 @ hw("o/level") with safe = 0

machine m:
    var a : int in 0..5 = 0
    var b : int in 0..5 = 5

    initial RUN

    state RUN:
        loop:
            if a < 5:
                a = a + 1
                b = b - 1
            level = a + b
"#;

/// 13.3: Die Urteile mit Exit-Code; ein Gegenbeispiel landet mit `--out` als
/// `<name>.stim.trace`, und `takt sim --stim` spielt es nach.
#[test]
fn a_violated_property_fails_and_its_stimulus_replays() {
    let Some(()) = solver() else { return };
    let file = program("counter", COUNTER);
    let path = file.to_str().expect("Pfad");
    let out_dir = file.with_file_name("out");
    let out = takt(&["prove", path, "--depth", "6", "--out", out_dir.to_str().expect("Pfad")]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "eine verletzte Eigenschaft besteht nicht:\n{stdout}");
    assert!(stdout.contains("property bounded: bewiesen (k-Induktion"), "{stdout}");
    // Welches Gegenbeispiel der Solver findet, steht ihm frei; nachgespielt
    // verletzt es die Eigenschaft im selben Tick.
    let at: u64 = stdout
        .split("property small: verletzt bei t=")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("{stdout}"));
    let stim = out_dir.join("small.stim.trace");
    assert!(stdout.contains(&format!("Gegenbeispiel: {}", stim.display())), "{stdout}");
    let ticks = (at + 2).to_string();
    let sim = takt(&["sim", path, "--ticks", &ticks, "--stim", stim.to_str().expect("Pfad")]);
    let trace = String::from_utf8_lossy(&sim.stdout);
    assert!(trace.contains(&format!("t={at} property small violated")), "{trace}");
    assert!(!sim.status.success(), "{trace}");
    let again = takt(&["prove", path, "--depth", "6"]);
    assert!(!again.status.success(), "ohne --out bleibt das Urteil");
    let _ = std::fs::remove_dir_all(file.parent().expect("Verzeichnis"));
}

/// 11.3 und Pruefung 65: `--save-proof` schreibt die bewiesenen Stellen, und
/// `takt build --proof` laesst genau diese Pruefung aus; eine Beweisdatei zu
/// einer anderen Quelle lehnt der Build ab.
#[test]
fn a_saved_proof_drops_its_check_from_the_build_and_a_stale_one_is_refused() {
    let Some(()) = solver() else { return };
    let file = program("sum", SUM);
    let path = file.to_str().expect("Pfad");
    let proof = file.with_file_name("sum.proof");
    let proof_path = proof.to_str().expect("Pfad");
    let out = takt(&["prove", path, "--depth", "4", "--save-proof", proof_path]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("range m:18:13: bewiesen unerreichbar"), "{stdout}");
    assert!(stdout.contains(&format!("{proof_path}: 1 bewiesene Stellen")), "{stdout}");
    // FB-380: Bericht und Beweisdatei nennen Solver und Version.
    let identity = stdout.lines().find_map(|l| l.trim().strip_prefix("Solver: ")).unwrap_or_else(|| panic!("{stdout}"));
    assert!(identity.split(' ').nth(1).is_some_and(|v| v.contains('.')), "Name und Version: {identity}");
    let saved = takt_mir::analysis::proof::parse(&std::fs::read_to_string(&proof).expect("Beweis")).expect("lesbar");
    assert_eq!(saved.solver, identity);
    let ir = |name: &str, extra: &[&str]| {
        let ll = file.with_file_name(name);
        let out = takt(&[&["build", path, "--emit", "ir", "--out", ll.to_str().expect("Pfad")][..], extra].concat());
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        std::fs::read_to_string(&ll).expect("IR").matches("; Range ").count()
    };
    let plain = ir("plain.ll", &[]);
    assert_eq!(ir("proven.ll", &["--proof", proof_path]), plain - 1, "eine Range-Pruefung weniger");
    let text = std::fs::read_to_string(&proof).expect("Beweis");
    let stale = file.with_file_name("stale.proof");
    std::fs::write(&stale, text.replacen("program ", "program 0", 1)).expect("Beweis");
    let out = takt(&["build", path, "--emit", "ir", "--proof", stale.to_str().expect("Pfad")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && stderr.contains("error[SC-65]"), "{stderr}");
    let _ = std::fs::remove_dir_all(file.parent().expect("Verzeichnis"));
}

/// `--depth abc` ist ein Fehler, keine Vorgabetiefe.
#[test]
fn a_bad_depth_is_refused() {
    let out = takt(&["prove", "corpus-try/01_minimal.takt", "--depth", "abc"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--depth:"), "{}", String::from_utf8_lossy(&out.stderr));
}
