//! Die Diagnosen gegen ihre Quellen (13.8, Schritt 25).
//!
//! - **Je Datei:** Jede Korpusdatei meldet genau die Fehler, die ihre
//!   Anmerkungen nennen (`takt_testkit::expect`); ohne Anmerkung uebersetzt
//!   sie fehlerfrei. Das gilt auch fuer die Ausschnitte der Referenz
//!   (`corpus-try/ref/`) ausser den Fragmenten in [`SNIPPET_FRAGMENTS`].
//!   Was `corpus-try/manifest.csv` nur als Syntax fuehrt (`parse ok`),
//!   prueft die Syntax; `corpus-try/checks/` hat einen eigenen Test.
//! - **Die Schwere steht in Tabelle 10.** Jede Diagnose einer Pruefung hat
//!   die Schwere, die die Tabelle nennt; die Anmerkungen nennen keine.
//! - **Jede Pruefung hat Faelle.** Ein Verzeichnis unter
//!   `corpus-try/checks/` oder einen Grund in [`WITHOUT_DIRECTORY`].
//! - **Jede Meldestelle hat einen Fall.** `Diagnostic::origin` haelt fest,
//!   wo im Compiler eine Diagnose entstand. Der Test vergleicht die
//!   Stellen, die der Korpus trifft, mit allen Stellen in Sema und MIR; die
//!   Zahl der nie getroffenen ist eine Ratsche.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use takt_diag::{Diagnostic, Policy, SourceMap};
use takt_sema::{Build, Options};
use takt_testkit::expect;

/// Meldestellen in Sema und MIR, die kein Programm des Korpus ausloest.
/// Die Zahl sinkt nur; der Test nennt die Stellen.
const UNTRIGGERED: usize = 320;

/// Pruefungen der Tabelle 10 ohne Verzeichnis unter `corpus-try/checks/`,
/// mit Grund. Die Liste schrumpft nur.
const WITHOUT_DIRECTORY: &[(&str, &str)] = &[
    ("SC-1", "Tokenizer und Parser melden eigene Codes; Faelle in tests/checks.rs und corpus-try/n0*.takt"),
    ("SC-5", "der implizite Check ist Default; Tabelle 10 nennt keine Meldung"),
    ("SC-12", "braucht Kostenmodell und Hardware-Konfiguration; Faelle in tests/analysis.rs und tests/calibrated.rs"),
    ("SC-28", "braucht gemessenen Jitter der Hardware-Konfiguration; Faelle in tests/hardware.rs"),
    ("SC-29", "eine Pruefung der Kampagnen; Faelle in tests/campaigns.rs"),
    ("SC-39", "braucht die Speichergrenzen der Hardware-Konfiguration; Faelle in tests/calibrated.rs"),
    ("SC-60", "braucht die Hardware-Konfiguration; Faelle in tests/hardware.rs und tests/sys_channels.rs"),
    ("SC-65", "braucht eine Beweisdatei; Faelle in tests/analysis.rs"),
];

/// Ausschnitte der Referenz, die bewusst keine Datei sind, mit Grund: Ihre
/// Syntax prueft `parse_snippet` (takt-syntax), als Datei uebersetzt sieht
/// die Sema nur Parserfehler. Jeder andere Ausschnitt traegt seine Fehler
/// als Anmerkung (`# ~`), auch ein SC-2 fuer Namen aus dem umgebenden Text.
/// Die Liste schrumpft nur.
const SNIPPET_FRAGMENTS: &[(&str, &str)] = &[
    ("corpus-try/ref/13_5_01.takt", "measure, verify und verdict ohne Szenario"),
    ("corpus-try/ref/3_11_01.takt", "Variablen und Zuweisungen einer Maschine ohne Maschine"),
    ("corpus-try/ref/3_11_03.takt", "Variablen und loop einer Maschine ohne Maschine"),
    ("corpus-try/ref/3_5_01.takt", "ein loop-Block ohne Maschine"),
    ("corpus-try/ref/3_7_05.takt", "eine match-Anweisung ohne Maschine"),
    ("corpus-try/ref/3_9_01.takt", "eine Variable einer Maschine auf Dateiebene"),
    ("corpus-try/ref/4_4_01.takt", "drei Anweisungen ohne Maschine"),
    ("corpus-try/ref/5_11_01.takt", "ein Zustand mit gescopten Instanzen ohne Maschine"),
    ("corpus-try/ref/5_12_01.takt", "ein Zustand mit resume ohne Maschine"),
    ("corpus-try/ref/6_3_01.takt", "ein Zustand mit sequence ohne Maschine"),
    ("corpus-try/ref/7_5_01.takt", "at, pulse und cancel ohne Maschine"),
    ("corpus-try/ref/7_5_05.takt", "arm, disarm und until auf Dateiebene neben dem Trigger"),
    ("corpus-try/ref/8_11_01.takt", "Kanaele und eine sequence ohne Zustand"),
    ("corpus-try/ref/8_7_01.takt", "vier Musterliterale, keine Deklaration"),
    ("corpus-try/ref/8_7_03.takt", "ein Zustand mit Handlern ohne Maschine"),
    ("corpus-try/ref/8_7_05.takt", "until und when ohne Zustand"),
    ("corpus-try/ref/8_8_01.takt", "send auf Dateiebene"),
    ("corpus-try/ref/8_9_01.takt", "check und for ohne Maschine"),
    ("corpus-try/ref/8_9_02.takt", "ein Handler ohne Zustand"),
];

/// Ein Lauf: Datei, Zertifizierungsmodus, Diagnosen mit Zeile und Spalte.
struct Run {
    file: String,
    build: Build,
    certification: bool,
    diagnostics: Vec<(u32, u32, Diagnostic)>,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Jede `.takt`-Datei unter `corpus-try` und `crates` ausser dem Prelude,
/// fuer Simulation und Hardware, dazu im Zertifizierungsmodus; mit den
/// kalibrierten Pruefungen, wo eine `hardware.hw` daneben liegt, und der
/// Review-Pruefung, wo das Programm `tcb_policy = reviewed(…)` setzt.
fn runs() -> &'static [Run] {
    static RUNS: OnceLock<Vec<Run>> = OnceLock::new();
    RUNS.get_or_init(|| {
        let root = workspace();
        let mut files = Vec::new();
        for dir in ["corpus-try", "crates"] {
            collect(&root.join(dir), "takt", &mut files);
        }
        files.retain(|f| !f.ends_with("takt-sema/src/prelude.takt"));
        assert!(files.len() >= 350, "nur {} Programme unter corpus-try und crates", files.len());
        files.sort();
        let mut out = Vec::new();
        for path in files {
            let src = std::fs::read_to_string(&path).expect("lesbar");
            let name = relative(&root, &path);
            let hw = std::fs::read_to_string(path.with_file_name("hardware.hw"))
                .ok()
                .map(|t| takt_mir::hardware::parse(&t).expect("hardware.hw lesbar"));
            let map = SourceMap::single(name.as_str(), src.as_str());
            for (build, certification) in [(Build::Sim, false), (Build::Hw, false), (Build::Sim, true)] {
                let options = Options {
                    build,
                    policy: Policy { certification, ..Policy::default() },
                    channel_imports: imports(&path, &src),
                    ..Default::default()
                };
                let checked = takt_sema::compile(&src, &options);
                let mut diagnostics = checked.diagnostics.clone();
                if let (Some(hw), Some(program)) = (&hw, &checked.program) {
                    let target = hw.targets.values().next();
                    if let Some(t) = target {
                        diagnostics.extend(takt_sema::calibrated::check(program, t, takt_diag::Span::new(0, 0)));
                    }
                    diagnostics.extend(takt_sema::calibrated::check_bindings(program, hw));
                    diagnostics.extend(takt_sema::calibrated::polling(program, hw, target));
                }
                if let Some(program) = checked.program.as_ref().filter(|p| p.config.tcb_reviewed) {
                    diagnostics.extend(reviewed(&path, program));
                }
                let diagnostics = diagnostics
                    .into_iter()
                    .map(|d| {
                        let (line, col) = map.line_col(d.span);
                        (line, col, d)
                    })
                    .collect();
                out.push(Run { file: name.clone(), build, certification, diagnostics });
            }
        }
        out
    })
}

/// Pruefung 31 mit `tcb_policy = reviewed(…)` wie `takt check`: gegen
/// `natives.review` neben dem Programm (leer, wenn sie fehlt), die Quellen
/// der Projekt-Natives daneben.
fn reviewed(path: &Path, program: &takt_mir::Program) -> Vec<Diagnostic> {
    let review = std::fs::read_to_string(path.with_file_name("natives.review"))
        .map(|t| takt_mir::review::parse(&t).expect("natives.review lesbar"))
        .unwrap_or_default();
    takt_sema::calibrated::reviewed(program, &review, &|from| std::fs::read(path.with_file_name(from)).ok())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn imports(path: &Path, src: &str) -> BTreeMap<String, String> {
    let dir = path.parent().expect("Verzeichnis");
    takt_sema::channel_imports(src)
        .into_iter()
        .filter_map(|f| std::fs::read_to_string(dir.join(&f)).ok().map(|t| (f, t)))
        .collect()
}

fn collect(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("Verzeichniseintrag").path();
        if path.is_dir() {
            collect(&path, ext, out);
        } else if path.extension().is_some_and(|x| x == ext) {
            out.push(path);
        }
    }
}

/// Die Dateien, die `corpus-try/manifest.csv` nur als Syntax fuehrt.
fn parse_only() -> BTreeSet<String> {
    let text = std::fs::read_to_string(workspace().join("corpus-try/manifest.csv")).expect("Manifest lesbar");
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().expect("Kopf").split(',').collect();
    let col = header.iter().position(|h| *h == "Erwartung").expect("Spalte Erwartung");
    lines
        .filter_map(|l| {
            let cells: Vec<&str> = l.split(',').collect();
            let expectation = cells.get(col)?.trim();
            (expectation == "parse ok").then(|| format!("corpus-try/{}", cells[0]))
        })
        .collect()
}

/// **Je Datei genau die angemerkten Fehler.** Ein Programm, das der Korpus
/// als gueltig fuehrt, und dennoch nicht uebersetzt, fiel bisher still aus
/// jeder Suite (05 und 07, FB-378).
#[test]
fn every_corpus_file_reports_exactly_its_annotated_errors() {
    let skip = parse_only();
    let mut wrong = Vec::new();
    for (file, _) in SNIPPET_FRAGMENTS {
        if !runs().iter().any(|r| r.file == *file) {
            wrong.push(format!("{file}: steht in SNIPPET_FRAGMENTS, gibt es aber nicht"));
        }
    }
    for run in runs().iter().filter(|r| r.build == Build::Sim && !r.certification) {
        if run.file.starts_with("corpus-try/checks/") || skip.contains(&run.file) {
            continue;
        }
        if let Some((_, why)) = SNIPPET_FRAGMENTS.iter().find(|(f, _)| *f == run.file) {
            if !run.diagnostics.iter().any(|(_, _, d)| d.is_error()) {
                wrong.push(format!("{}: uebersetzt jetzt ({why}); aus SNIPPET_FRAGMENTS nehmen", run.file));
            }
            continue;
        }
        let src = std::fs::read_to_string(workspace().join(&run.file)).expect("lesbar");
        let expected = expect::expectations(&src);
        let annotated: BTreeSet<&str> = expected.iter().map(|e| e.code.as_str()).collect();
        let actual: Vec<expect::Actual> = run
            .diagnostics
            .iter()
            .filter(|(_, _, d)| d.is_error() || annotated.contains(d.code))
            .map(|(line, col, d)| expect::Actual { line: *line, col: *col, code: d.code.to_string() })
            .collect();
        for m in expect::mismatches(&expected, &actual) {
            wrong.push(format!("{}: {m}", run.file));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Was Tabelle 10 einer Pruefung erlaubt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Allowed {
    Error,
    Warning,
    Either,
    /// `—`: Die Pruefung meldet nichts.
    Nothing,
}

/// Die Schwere je Pruefung aus Tabelle 10 der Referenz.
fn table_10() -> BTreeMap<String, Allowed> {
    let text = std::fs::read_to_string(workspace().join("plan/definition.md")).expect("Referenz lesbar");
    let start = text.find("| # | Analyse | Fehler / Warnung |").expect("Tabelle 10");
    let mut out = BTreeMap::new();
    for row in text[start..].lines().skip(2).take_while(|l| l.starts_with('|')) {
        let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
        let cell: String = cells[2].chars().filter(|c| !c.is_whitespace()).collect();
        let allowed = match cell.as_str() {
            s if s.starts_with("F/W") || s.starts_with("W/F") => Allowed::Either,
            s if s.starts_with('F') => Allowed::Error,
            s if s.starts_with('W') => Allowed::Warning,
            s if s.starts_with('—') => Allowed::Nothing,
            _ => panic!("Tabelle 10, Pruefung {}: Schwere `{}`", cells[0], cells[2]),
        };
        out.insert(format!("SC-{}", cells[0]), allowed);
    }
    out
}

/// **Die Schwere jeder Diagnose ist die der Tabelle 10**, ausserhalb des
/// Zertifizierungsmodus, der Warnungen zu Fehlern heben darf.
#[test]
fn every_check_reports_with_the_severity_of_table_10() {
    let table = table_10();
    assert_eq!(table.len(), 65, "Tabelle 10 hat 65 Pruefungen");
    let mut wrong = BTreeSet::new();
    for run in runs().iter().filter(|r| !r.certification) {
        for (line, _, d) in &run.diagnostics {
            if !d.code.starts_with("SC-") {
                continue;
            }
            let ok = match table.get(d.code) {
                Some(Allowed::Either) => true,
                Some(Allowed::Error) => d.is_error(),
                Some(Allowed::Warning) => !d.is_error(),
                Some(Allowed::Nothing) | None => false,
            };
            if !ok {
                wrong.insert(format!("{}:{line}: {d} (Tabelle 10: {:?})", run.file, table.get(d.code)));
            }
        }
    }
    let wrong: Vec<String> = wrong.into_iter().collect();
    assert!(wrong.is_empty(), "{} Diagnosen gegen Tabelle 10:\n{}", wrong.len(), wrong.join("\n"));
}

/// **Jede Pruefung hat ein Verzeichnis oder einen Grund.**
#[test]
fn every_check_of_table_10_has_cases() {
    let dirs: BTreeSet<String> = std::fs::read_dir(workspace().join("corpus-try/checks"))
        .expect("corpus-try/checks lesbar")
        .filter_map(|e| e.ok().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    let excused: BTreeSet<&str> = WITHOUT_DIRECTORY.iter().map(|(c, _)| *c).collect();
    let mut wrong = Vec::new();
    for code in table_10().keys() {
        match (dirs.contains(code), excused.contains(code.as_str())) {
            (false, false) => wrong.push(format!("{code}: kein Verzeichnis und kein Grund in `WITHOUT_DIRECTORY`")),
            (true, true) => wrong.push(format!("{code}: hat jetzt ein Verzeichnis; aus `WITHOUT_DIRECTORY` streichen")),
            _ => {}
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Jede Meldestelle in `crates/takt-sema/src` und `crates/takt-mir/src`:
/// Datei und Zeile jedes Aufrufs, der eine Diagnose baut, ausser in
/// Kommentaren, Testmodulen und Hilfen mit `#[track_caller]`, die den Ort
/// ihres Aufrufers weitergeben.
fn sites() -> BTreeSet<(String, u32)> {
    const BUILDS: [&str; 9] = [
        "Diagnostic::error(",
        "Diagnostic::warning(",
        "Diagnostic::new(",
        ".error(",
        ".error_hint(",
        ".warn(",
        ".warn_hint(",
        ".stage(",
        "duplicate(",
    ];
    let root = workspace();
    let mut files = Vec::new();
    for dir in ["crates/takt-sema/src", "crates/takt-mir/src"] {
        collect(&root.join(dir), "rs", &mut files);
    }
    let mut out = BTreeSet::new();
    for path in files {
        let name = relative(&root, &path);
        let text = std::fs::read_to_string(&path).expect("lesbar");
        let mut forwarding: Option<String> = None;
        let mut pending = false;
        for (i, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("#[cfg(test)]") {
                break;
            }
            if let Some(close) = &forwarding {
                if line == close {
                    forwarding = None;
                }
                continue;
            }
            if code.starts_with("#[track_caller]") {
                pending = true;
                continue;
            }
            if pending && code.contains("fn ") {
                pending = false;
                forwarding = Some(format!("{}}}", &line[..line.len() - code.len()]));
                continue;
            }
            if !code.starts_with("//") && BUILDS.iter().any(|b| code.contains(b)) {
                out.insert((name.clone(), u32::try_from(i + 1).expect("Zeile")));
            }
        }
    }
    out
}

/// **Jede Meldestelle hat einen Fall im Korpus.**
#[test]
fn every_diagnostic_site_has_a_triggering_program() {
    let sites = sites();
    let hit: BTreeSet<(String, u32)> = runs()
        .iter()
        .flat_map(|r| &r.diagnostics)
        .map(|(_, _, d)| (d.origin.file().replace('\\', "/"), d.origin.line()))
        .filter(|(file, _)| file.starts_with("crates/takt-sema/") || file.starts_with("crates/takt-mir/"))
        .collect();
    let unknown: Vec<String> = hit.difference(&sites).map(|(f, l)| format!("{f}:{l}")).collect();
    assert!(unknown.is_empty(), "Meldestellen, die die Suche nicht kennt:\n{}", unknown.join("\n"));
    let untriggered: Vec<String> = sites.difference(&hit).map(|(f, l)| format!("{f}:{l}")).collect();
    assert!(
        untriggered.len() == UNTRIGGERED,
        "{} von {} Meldestellen ohne Fall im Korpus, eingecheckt sind {UNTRIGGERED}; weniger: `UNTRIGGERED` \
         senken, mehr: ein Programm mit der Meldung ergaenzen\n{}",
        untriggered.len(),
        sites.len(),
        untriggered.join("\n")
    );
}
