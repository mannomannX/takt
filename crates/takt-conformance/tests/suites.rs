//! Welche Suite welches Korpusprogramm faehrt (FB-378): Die Suiten lesen ihre
//! Programme aus `corpus-try/manifest.csv` ([`takt_conformance::suites`]);
//! dieser Test haelt das Manifest vollstaendig.

use takt_conformance::suites::{RULE, SUITES, check, manifest, parts, programs, selects};

/// **Jedes Korpusprogramm hat seinen Eintrag** (FB-378): eine Zeile im
/// Manifest und in ihr die Spalte `Suiten`, die jede Suite nennt — gefahren
/// oder mit Grund ausgelassen. Ein neues Programm ohne Eintrag laesst den
/// Test scheitern, statt still in keiner Suite zu laufen.
#[test]
fn every_corpus_program_names_its_suites() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try");
    let mut programs: Vec<String> = std::fs::read_dir(root)
        .expect("corpus-try lesbar")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".takt") && n.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    programs.sort();
    let mut failed = Vec::new();
    for name in &programs {
        match manifest().iter().find(|(file, _)| file == name).map(|(_, entry)| entry.trim()) {
            None => failed.push(format!("{name}: keine Zeile im Manifest")),
            Some("") => failed.push(format!("{name}: keine Suiten")),
            Some(entry) => failed.extend(check(entry).into_iter().map(|e| format!("{name}: {e}"))),
        }
    }
    assert!(failed.is_empty(), "{} Befunde:\n{}", failed.len(), failed.join("\n"));
}

/// **Jede Suite faehrt Programme**: Liefe eine leer, weil ihr Name im
/// Manifest anders geschrieben steht, bestuende sie ohne Lauf.
#[test]
fn every_suite_runs_some_program() {
    for suite in SUITES {
        assert!(!programs(suite).is_empty(), "die Suite `{suite}` faehrt kein Programm");
    }
}

/// **Jeder Eintrag folgt der Regel seiner Suite** ([`selects`]): Was eine
/// Suite faehrt, waehlt ihre Regel; `!suite Regel` steht genau dort, wo sie
/// das Programm nicht waehlt; eine Ausnahme mit eigenem Grund nur dort, wo
/// sie es waehlt — sonst waere der Grund keiner. Ein neues Programm mit
/// Fliesskomma muss so in `ziele` und `ieee`, ohne dass jemand daran denkt.
#[test]
fn every_entry_follows_the_rule_of_its_suite() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try");
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let mut failed = Vec::new();
    for (name, entry) in manifest().iter().filter(|(n, _)| n.starts_with(|c: char| c.is_ascii_digit())) {
        let Ok(src) = std::fs::read_to_string(root.join(name)) else { continue };
        let out = takt_sema::compile(&src, &options);
        let p = out.program.filter(|_| !out.diagnostics.iter().any(|d| d.is_error()));
        for (suite, runs, reason) in parts(entry).filter(|(s, _, _)| SUITES.contains(s)) {
            match (runs, reason == RULE, selects(suite, p.as_ref())) {
                (true, _, false) => failed.push(format!("{name}: `{suite}` faehrt es, die Regel waehlt es nicht")),
                (false, true, true) => failed.push(format!("{name}: `!{suite} Regel`, die Regel waehlt es aber")),
                (false, false, false) => {
                    failed.push(format!("{name}: `!{suite}` mit eigenem Grund, die Regel waehlt es ohnehin nicht"));
                }
                _ => {}
            }
        }
    }
    assert!(failed.is_empty(), "{} Befunde:\n{}", failed.len(), failed.join("\n"));
}
