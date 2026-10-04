//! Parst den Korpus: jedes Beispiel meldet genau die Syntaxfehler, die seine
//! Anmerkungen nennen (`takt_testkit::expect`, wie die Sema), die
//! Referenz-Schnipsel ueber den Schnipsel-Einstieg.

use std::path::{Path, PathBuf};

use takt_syntax::{SourceMap, parse_file, parse_snippet, tokenize};
use takt_testkit::expect;

fn takt_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .collect();
    files.sort();
    files
}

fn corpus_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try"))
}

/// Codes, die vor der Sema entstehen: Tokenizer (`E_…`) und Parser (`P`).
fn is_syntax(code: &str) -> bool {
    code == "P" || code.starts_with("E_")
}

/// **Jedes Beispiel parst, wie seine Anmerkungen sagen.** Ohne
/// Syntaxanmerkung parst es fehlerfrei und hat Deklarationen; mit ihnen
/// meldet es genau diese Fehler. Ein reserviertes Wort meldet der Tokenizer
/// als `E_RESERVED`, die Sema fuehrt es unter Pruefung 50 (2.5): Die
/// Anmerkung lautet darum `SC-50`.
#[test]
fn every_example_parses_as_its_annotations_say() {
    let files = takt_files(&corpus_root());
    assert!(files.len() >= 100, "nur {} Beispiele in corpus-try", files.len());
    let mut failures = Vec::new();
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let map = SourceMap::single(name.as_str(), src.as_str());
        let annotations = expect::expectations(&src);
        let expected: Vec<expect::Expected> = annotations.iter().filter(|e| is_syntax(&e.code)).cloned().collect();
        let toks = tokenize(&src);
        let (file, parse_errors) = parse_file(&toks);
        let mut actual = Vec::new();
        for d in toks.errors.iter().chain(&parse_errors) {
            let (line, col) = map.line_col(d.span);
            if d.code == "E_RESERVED" {
                if !annotations.iter().any(|e| e.line == line && e.code == "SC-50") {
                    failures.push(format!("{name}: reserviertes Wort in Zeile {line} ohne Anmerkung `SC-50`"));
                }
                continue;
            }
            actual.push(expect::Actual { line, col, code: d.code.to_string() });
        }
        failures.extend(expect::mismatches(&expected, &actual).into_iter().map(|m| format!("{name}: {m}")));
        if expected.is_empty() && file.items.is_empty() {
            failures.push(format!("{name}: keine Deklarationen"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn reference_snippets_parse() {
    let mut failures = Vec::new();
    let files = takt_files(&corpus_root().join("ref"));
    assert!(files.len() > 30, "zu wenige Schnipsel: {}", files.len());
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let toks = tokenize(&src);
        assert!(toks.errors.is_empty(), "{name}: Tokenizer {:?}", toks.errors);
        let (items, errors) = parse_snippet(&toks);
        if !errors.is_empty() {
            failures
                .push(format!("{name}:\n  {}", errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n  ")));
        }
        assert!(!items.is_empty(), "{name}: leer");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
