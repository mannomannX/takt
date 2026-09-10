//! Lowering des Korpus: die Beispiele 14.1 bis 14.5 (Abnahme M1, plan.md)
//! uebersetzen ohne Fehler; alles andere meldet nur Konstrukte spaeterer
//! Stufen, nie einen Absturz.

use std::path::{Path, PathBuf};

use takt_diag::{Policy, SourceMap};
use takt_sema::{Build, Options};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try"))
}

fn options() -> Options {
    Options { policy: Policy::default(), build: Build::Sim, profile: None }
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

/// Die Beispiele der Kernsemantik uebersetzen vollstaendig (Exit M1).
#[test]
fn examples_14_1_to_14_5_lower_without_errors() {
    for name in ["14_1_01", "14_2_01", "14_3_01", "14_4_01", "14_5_01"] {
        let path = root().join("ref").join(format!("{name}.takt"));
        let errors = errors_of(&path);
        assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    }
}

/// Der Korpus der Kernkonstrukte uebersetzt ebenfalls vollstaendig.
#[test]
fn core_corpus_lowers_without_errors() {
    // `05_streams_and_protocol` fehlt hier: es nutzt `follows` (M6, v1.1).
    // Alles andere daran senkt fehlerfrei.
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
