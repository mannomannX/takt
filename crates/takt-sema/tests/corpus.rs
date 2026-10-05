//! Lowering des Korpus: die Beispiele 14.1 bis 14.8 uebersetzen ohne Fehler
//! (14.1 bis 14.5 waren die Abnahme M1, plan.md), ihre Simulationsprogramme
//! enthalten sie, das Manifest nennt jede Datei; alles andere meldet nur
//! Konstrukte spaeterer Stufen, nie einen Absturz.

use std::path::{Path, PathBuf};

use takt_diag::{Policy, SourceMap};
use takt_sema::{Build, Options};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try"))
}

fn options() -> Options {
    Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() }
}

/// Alle `.takt`-Dateien des Korpus, in stabiler Reihenfolge.
fn corpus_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [root(), root().join("ref")] {
        let entries = std::fs::read_dir(&dir).expect("Korpus lesbar");
        files.extend(
            entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "takt")),
        );
    }
    files.sort();
    files
}

/// Fehlermeldungen einer Datei.
fn errors_of(path: &Path) -> Vec<String> {
    let src = std::fs::read_to_string(path).expect("lesbar");
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    let map = SourceMap::single(name.as_str(), src.as_str());
    let out = takt_sema::compile(&src, &options());
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| map.render_line(d)).collect()
}

/// Die Beispiele aus 14 uebersetzen vollstaendig (Exit M1 fuer 14.1 bis
/// 14.5, seit FB-401 auch 14.6 bis 14.8).
#[test]
fn examples_14_1_to_14_8_lower_without_errors() {
    for name in ["14_1_01", "14_2_01", "14_3_01", "14_4_01", "14_5_01", "14_6_01", "14_7_01", "14_8_01"] {
        let path = root().join("ref").join(format!("{name}.takt"));
        let errors = errors_of(&path);
        assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    }
}

/// Zeilen der Referenz, die das Simulationsprogramm mit Grund anders
/// fuehrt: (Beispiel, Zeile, Grund).
const SIMULATION_DEVIATES: &[(&str, &str, &str)] = &[
    (
        "14_2",
        "machine chamber_model every 100 ms:",
        "die Strecke rechnet in 1-s-Schritten, sonst endeten die zwei Zyklen nicht in 4400 Ticks",
    ),
    (
        "14_2",
        "var drive : float[K/s] = (0.05 K/s if heater else 0 K/s) - (0.04 K/s if cooler else 0 K/s)",
        "dto., 1 K/s",
    ),
    ("14_2", "t = t + (drive - leak) * (100 ms).as(s)", "dto."),
    (
        "14_6",
        "input i_dut : samples<float[A], 100> @ hw(\"daq1/ai2\") with rate = 100 kHz",
        "das Modell liefert 4er-Fenster aus Konstanten",
    ),
    ("14_6", "campaign brownout_scan:", "die Kampagne nennt eine Programmdatei; der Golden-Lauf ist ein Lauf"),
    ("14_6", "program \"supply_interruption.takt\"", "dto."),
    ("14_6", "sweep BROWNOUT_DELAY = 2 ms..400 ms step 250 us", "dto."),
    ("14_6", "repeat 2", "dto."),
    ("14_6", "stop_on fail", "dto."),
];

/// Jede Zeile eines Beispiels steht in seinem Simulationsprogramm
/// (`corpus-try/sim/14_x/program.takt`), bis auf Leerraum und Kommentare —
/// sonst laege der Golden-Trace neben der Referenz statt auf ihr.
#[test]
fn the_simulated_examples_contain_their_reference() {
    let norm = |l: &str| l.split('#').next().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
    let mut failures = Vec::new();
    for example in ["14_1", "14_2", "14_3", "14_4", "14_5", "14_6", "14_7", "14_8"] {
        let reference = std::fs::read_to_string(root().join("ref").join(format!("{example}_01.takt"))).expect("ref");
        let sim = std::fs::read_to_string(root().join("sim").join(example).join("program.takt")).expect("sim");
        let lines: std::collections::BTreeSet<String> = sim.lines().map(norm).collect();
        for line in reference.lines().map(norm).filter(|l| !l.is_empty()) {
            let excused = SIMULATION_DEVIATES.iter().any(|(e, l, _)| *e == example && *l == line);
            if !lines.contains(&line) && !excused {
                failures.push(format!("{example}: `{line}` fehlt in sim/{example}/program.takt"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Der Korpus der Kernkonstrukte uebersetzt ebenfalls vollstaendig.
#[test]
fn core_corpus_lowers_without_errors() {
    // Dass jede Korpusdatei genau ihre angemerkten Fehler meldet — auch die
    // Stroeme und Protokolle aus 05 (FB-378) —, prueft `diagnostics.rs`;
    // hier stehen nur die Kernkonstrukte.
    for name in ["01_minimal", "03_sequences_and_faults"] {
        let path = root().join(format!("{name}.takt"));
        let errors = errors_of(&path);
        assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    }
}

/// Jede Datei des Korpus laeuft ohne Panik durch; Konstrukte spaeterer Stufen
/// erscheinen als Diagnose mit Stufe, nicht als Absturz.
#[test]
fn every_corpus_file_lowers_without_panic() {
    let files = corpus_files();
    assert!(files.len() > 40, "Korpus gefunden: {}", files.len());
    for path in files {
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let out = takt_sema::compile(&src, &options());
        // Ein Programm ohne Fehler hat ein Programm in Kernform.
        if !out.has_errors() {
            assert!(out.program.as_ref().is_some_and(|p| p.is_core()), "{}", path.display());
        }
    }
}

/// Konstrukte spaeterer Stufen tragen ihre Stufe in der Diagnose.
#[test]
fn later_stages_are_reported_with_their_stage() {
    let path = root().join("08_reserved_future.takt");
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let out = takt_sema::compile(&src, &options());
    let staged = out.diagnostics.iter().filter(|d| d.stage.is_some()).count();
    assert!(
        staged > 0,
        "kein Konstrukt mit Stufe gemeldet:\n{}",
        out.diagnostics.iter().map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
    );
}

/// Ende M2 (plan/m2.md, Abschnitt 7): kein M2-Konstrukt meldet noch eine
/// Stufe. Die Meldungen nannten „v1.1" fuer Konstrukte, die die Grammatik
/// ohne `@stage` fuehrt — das war eine Umsetzungsschuld, keine Sprachstufe.
#[test]
fn no_m2_construct_is_reported_as_a_later_stage() {
    let m2 = [
        "Streams",
        "Fenster ueber Streams",
        "Muster",
        "layout",
        "`send`",
        "`at`",
        "`pulse`",
        "`cancel`",
        "samples",
        "`.t`",
        "`.seq`",
        "`.text`",
        "`.data`",
        "`.count`",
        "`.dropped`",
        "`.overflowed`",
        "`.malformed`",
        "`.free`",
    ];
    let mut found: Vec<String> = Vec::new();
    for path in corpus_files() {
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let out = takt_sema::compile(&src, &options());
        for d in out.diagnostics.iter().filter(|d| d.stage.is_some()) {
            if m2.iter().any(|m| d.message.contains(m)) {
                found.push(format!("{}: {}", path.display(), d.message));
            }
        }
    }
    assert!(found.is_empty(), "M2-Konstrukte mit Stufenmeldung:\n{}", found.join("\n"));
}

/// Nummern, die zwei Korpusdateien tragen, mit Grund. Die Liste schrumpft
/// nur.
const SHARED_NUMBERS: &[(&str, &str)] = &[(
    "13",
    "13_framing und 13_protocol_analysis stehen unter diesen Namen in Tests von takt-conformance, takt-llvm und takt-cli",
)];

/// **Jede Korpusdatei hat genau eine Zeile in `manifest.csv`** (KOR-001),
/// jede Zeile nennt eine vorhandene Datei, und jede Nummer gehoert einer
/// Datei — ausser den begruendeten in `SHARED_NUMBERS`.
#[test]
fn the_manifest_lists_every_corpus_file_once() {
    let text = std::fs::read_to_string(root().join("manifest.csv")).expect("Manifest");
    let mut lines = text.lines();
    let header = lines.next().expect("Kopf");
    assert!(header.starts_with("Datei,") && header.ends_with(",Suiten"), "{header}");
    let named: Vec<&str> = lines.map(|l| l.split(',').next().unwrap_or_default()).collect();
    let mut files: Vec<String> = std::fs::read_dir(root())
        .expect("Korpus lesbar")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".takt"))
        .collect();
    files.sort();
    let mut failures = Vec::new();
    for f in &files {
        match named.iter().filter(|n| **n == f.as_str()).count() {
            1 => {}
            0 => failures.push(format!("{f}: keine Zeile")),
            k => failures.push(format!("{f}: {k} Zeilen")),
        }
    }
    for n in &named {
        if !files.iter().any(|f| f == n) {
            failures.push(format!("{n}: Zeile ohne Datei"));
        }
    }
    let mut by_number: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for f in &files {
        by_number.entry(f.split('_').next().unwrap_or_default()).or_default().push(f);
    }
    for (number, holders) in &by_number {
        let shared = SHARED_NUMBERS.iter().any(|(n, _)| n == number);
        if (holders.len() > 1) != shared {
            failures.push(format!("Nummer {number}: {holders:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
