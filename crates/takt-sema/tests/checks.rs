//! Testrahmen der Pruefungen (plan/sema-m0.md, Abschnitt 4.2): je Pruefung ein
//! Verzeichnis `corpus-try/checks/SC-n/` mit `ok_*.takt` und `bad_*.takt`.
//!
//! - `ok_` uebersetzt ohne Fehler und ohne eine Diagnose des eigenen Codes.
//! - `bad_` erwartet mindestens eine Diagnose des eigenen Codes; die
//!   Anmerkungen (`#~ SC-n`, `takt_testkit::expect`) nennen jede Diagnose
//!   des eigenen Codes, jeden Fehler und jede angemerkte Warnung, mit Zeile
//!   und auf Wunsch Spalte. Ein Fehler, den niemand anmerkt, scheitert:
//!   Eine Datei soll an ihrer Pruefung scheitern, nicht an etwas anderem.
//!   Warnungen anderer Codes bleiben unbewertet.
//!
//! Eine Datei `bad_stage.takt` ist der Sonderfall aus plan/m3.md 1.4: Die
//! Pruefung ist erst moeglich, wenn ihr Konstrukt existiert. Bis dahin ist
//! die *Ablehnung* der Abnahmetest — der Compiler muss das Konstrukt mit
//! einer Stufe beantworten, nicht mit einem Absturz oder einem Durchwinken.
//! Solche Dateien werden nicht ueber den Code geprueft (Stufenmeldungen
//! tragen alle `SC-3`), sondern ueber das Stufenfeld der Diagnose.
//!
//! Die Dateien laufen durch `compile`, nicht durch `check`: die Pruefungen 6
//! bis 16 entstehen erst mit der MIR, und `compile` schliesst die Syntax- und
//! Namenspruefungen ein.
//!
//! Liegt im Verzeichnis eine `hardware.hw`, laufen zusaetzlich die
//! Pruefungen, die eine Konfiguration brauchen (`takt_sema::calibrated`).
//! Sie entscheiden aus zwei Eingaben und sind darum ohne Konfiguration
//! nicht nur stumm, sondern gar nicht anwendbar.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use takt_diag::{Policy, SourceMap};
use takt_sema::{Build, Options};
use takt_testkit::expect;

/// Pruefungen, die ihren Code nie melden, mit Grund: Ihre `bad_`-Dateien
/// koennen ihn nicht anmerken. Jede muss noch auftreten — meldet die
/// Pruefung ihren Code, scheitert der Test, bis sie hier verschwindet.
const SILENT: &[(&str, &str)] = &[];

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/checks"))
}

/// Die Hardware-Konfiguration eines Pruefverzeichnisses, falls es eine hat.
fn hardware(dir: &Path) -> Option<takt_mir::hardware::Hardware> {
    let text = std::fs::read_to_string(dir.join("hardware.hw")).ok()?;
    Some(takt_mir::hardware::parse(&text).expect("hardware.hw lesbar"))
}

fn check_dir(dir: &Path, code: &str, failures: &mut Vec<String>, silent_seen: &mut Vec<String>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .collect();
    files.sort();
    let hw = hardware(dir);
    let mut seen_ok = false;
    let mut seen_bad = false;
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let map = SourceMap::single(name.as_str(), src.as_str());
        let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
        let checked = takt_sema::compile(&src, &options);
        let mut diagnostics = checked.diagnostics.clone();
        if let (Some(hw), Some(program)) = (&hw, &checked.program) {
            diagnostics.extend(takt_sema::calibrated::polling(program, hw, hw.targets.values().next()));
        }
        if name.starts_with("ok_") {
            seen_ok = true;
            let wrong: Vec<String> =
                diagnostics.iter().filter(|d| d.is_error() || d.code == code).map(|d| map.render_line(d)).collect();
            if !wrong.is_empty() {
                failures.push(format!("{code}/{name}: muss fehlerfrei uebersetzen\n  {}", wrong.join("\n  ")));
            }
        } else if name == "bad_stage.takt" {
            seen_bad = true;
            // Die Ablehnung selbst ist der Test: mindestens eine Diagnose
            // nennt eine Stufe (plan/m3.md 1.4).
            let staged: Vec<String> =
                checked.diagnostics.iter().filter(|d| d.stage.is_some()).map(|d| map.render_line(d)).collect();
            if staged.is_empty() {
                let all: Vec<String> = checked.diagnostics.iter().map(|d| map.render_line(d)).collect();
                failures.push(format!("{code}/{name}: keine Stufenmeldung\n  {}", all.join("\n  ")));
            }
        } else if name.starts_with("bad_") {
            seen_bad = true;
            let expected = expect::expectations(&src);
            if !expected.iter().any(|e| e.code == code) {
                if SILENT.iter().any(|(c, _)| *c == code) {
                    silent_seen.push(code.to_string());
                } else {
                    failures.push(format!("{code}/{name}: keine Anmerkung `#~ {code}`"));
                }
            }
            let annotated: BTreeSet<&str> = expected.iter().map(|e| e.code.as_str()).collect();
            let actual: Vec<expect::Actual> = diagnostics
                .iter()
                .filter(|d| d.is_error() || d.code == code || annotated.contains(d.code))
                .map(|d| {
                    let (line, col) = map.line_col(d.span);
                    expect::Actual { line, col, code: d.code.to_string() }
                })
                .collect();
            let wrong = expect::mismatches(&expected, &actual);
            if !wrong.is_empty() {
                let rendered: Vec<String> = diagnostics.iter().map(|d| map.render_line(d)).collect();
                failures.push(format!("{code}/{name}: {}\n  {}", wrong.join("; "), rendered.join("\n  ")));
            }
        } else {
            failures.push(format!("{code}/{name}: Datei heisst weder ok_ noch bad_"));
        }
    }
    if !(seen_ok && seen_bad) {
        failures.push(format!("{code}: braucht mindestens eine ok_- und eine bad_-Datei"));
    }
}

#[test]
fn every_check_directory_passes() {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root())
        .expect("corpus-try/checks lesbar")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(dirs.len() >= 12, "zu wenige Pruefverzeichnisse: {}", dirs.len());
    let mut failures = Vec::new();
    let mut silent_seen = Vec::new();
    for dir in dirs {
        let code = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        check_dir(&dir, &code, &mut failures, &mut silent_seen);
    }
    for (code, why) in SILENT {
        if !silent_seen.iter().any(|c| c == code) {
            failures.push(format!("{code} meldet sich jetzt ({why}); aus `SILENT` streichen"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn certification_mode_makes_missing_edition_an_error() {
    let src = "machine m:\n    initial A\n    state A:\n        loop:\n            pass\n";
    let relaxed = takt_sema::check(src, Policy::default());
    assert!(!relaxed.has_errors());
    assert!(relaxed.diagnostics.iter().any(|d| d.code == "SC-49"));
    let strict = takt_sema::check(src, Policy { certification: true, ..Policy::default() });
    assert!(strict.has_errors());
}

#[test]
fn syntax_errors_stop_before_semantic_checks() {
    let src = "system:\n    language = 1\nrecord R:\n    valid : bool\nmachine m:\n    state A:\n";
    let checked = takt_sema::check(src, Policy::default());
    assert!(checked.file.is_none());
    assert!(checked.diagnostics.iter().all(|d| d.code != "SC-50"), "{:?}", checked.diagnostics);
    assert!(checked.diagnostics.iter().any(|d| d.code == "P"));
}

/// Pruefung 1 (Tokenizer und Parser) hat kein Verzeichnis unter
/// `corpus-try/checks/`: Sie meldet nie den Code `SC-1`, sondern die Codes
/// des Lexers (`E_INDENT`, `E_DEDENT`, `E_TAB`, …) und des Parsers (`P`).
/// Ihre Vektoren stehen in `grammar/lexer.md`, ihr Korpus ist
/// `corpus-try/n0*.takt`. Dieser Test haelt die drei Klassen fest, damit die
/// Pruefung eine Fundstelle hat.
#[test]
fn check_1_reports_lexer_and_parser_codes() {
    let cases = [
        ("system:\n    language = 1\n  tick = 1 ms\n", "E_DEDENT"),
        ("system:\n      language = 1\n", "E_INDENT"),
        ("system:\n\tlanguage = 1\n", "E_TAB"),
        ("machine m:\n    state\n", "P"),
    ];
    for (src, code) in cases {
        let checked = takt_sema::check(src, Policy::default());
        assert!(
            checked.diagnostics.iter().any(|d| d.code == code),
            "{code} fehlt fuer {src:?}: {:?}",
            checked.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }
}
