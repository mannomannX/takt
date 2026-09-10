//! Testrahmen der Pruefungen (plan/sema-m0.md, Abschnitt 4.2): je Pruefung ein
//! Verzeichnis `corpus-try/checks/SC-n/` mit `ok_*.takt` (keine Diagnose dieses
//! Codes) und `bad_*.takt` mit Zeilenanmerkungen `#~ SC-n` (dieselbe Zeile) oder
//! `#~^ SC-n` (die Zeile davor). Andere Codes werden nicht bewertet.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use takt_diag::{Policy, SourceMap};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/checks"))
}

/// Erwartete (Zeile, Code) aus den Anmerkungen.
fn expectations(src: &str, code: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for (i, line) in src.lines().enumerate() {
        let Some(pos) = line.find("#~") else { continue };
        let rest = line[pos + 2..].trim();
        let (up, rest) = match rest.strip_prefix('^') {
            Some(r) => (true, r.trim()),
            None => (false, rest),
        };
        for want in rest.split(',').map(str::trim) {
            if want == code {
                out.insert(if up { i as u32 } else { i as u32 + 1 });
            }
        }
    }
    out
}

fn check_dir(dir: &Path, code: &str, failures: &mut Vec<String>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .collect();
    files.sort();
    let mut seen_ok = false;
    let mut seen_bad = false;
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let map = SourceMap::single(name.as_str(), src.as_str());
        let checked = takt_sema::check(&src, Policy::default());
        let actual: BTreeSet<u32> =
            checked.diagnostics.iter().filter(|d| d.code == code).map(|d| map.line_col(d.span).0).collect();
        if name.starts_with("ok_") {
            seen_ok = true;
            if !actual.is_empty() {
                failures.push(format!("{code}/{name}: unerwartet {code} in Zeilen {actual:?}"));
            }
        } else if name.starts_with("bad_") {
            seen_bad = true;
            let expected = expectations(&src, code);
            if expected != actual {
                let rendered: Vec<String> =
                    checked.diagnostics.iter().filter(|d| d.code == code).map(|d| map.render_line(d)).collect();
                failures.push(format!(
                    "{code}/{name}: erwartet Zeilen {expected:?}, erhalten {actual:?}\n  {}",
                    rendered.join("\n  ")
                ));
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
    assert!(dirs.len() >= 3, "zu wenige Pruefverzeichnisse");
    let mut failures = Vec::new();
    for dir in dirs {
        let code = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        check_dir(&dir, &code, &mut failures);
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
