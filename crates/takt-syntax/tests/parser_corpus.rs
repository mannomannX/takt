//! Parst den Korpus: positive Beispiele ohne Fehler, Negativbeispiele mit dem
//! Fehler aus dem Manifest, Referenz-Schnipsel ueber den Schnipsel-Einstieg.

use std::path::{Path, PathBuf};

use takt_syntax::{parse_file, parse_snippet, tokenize};

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

#[test]
fn positive_examples_parse() {
    let mut failures = Vec::new();
    for path in takt_files(&corpus_root()) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if name.starts_with("n0") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let toks = tokenize(&src);
        assert!(toks.errors.is_empty(), "{name}: Tokenizer {:?}", toks.errors);
        let (file, errors) = parse_file(&toks);
        if !errors.is_empty() {
            failures
                .push(format!("{name}:\n  {}", errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n  ")));
        }
        assert!(!file.items.is_empty(), "{name}: keine Deklarationen");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn negative_examples_fail_as_documented() {
    let root = corpus_root();
    // (Datei, erwartete Fehlerzeile des ersten Parserfehlers)
    let cases = [("n02_transition_without_goto.takt", 16), ("n04_missing_initial.takt", 13)];
    for (name, line) in cases {
        let src = std::fs::read_to_string(root.join(name)).expect("lesbar");
        let toks = tokenize(&src);
        let (_, errors) = parse_file(&toks);
        let first = errors.first().unwrap_or_else(|| panic!("{name}: kein Parserfehler"));
        assert_eq!(first.line, line, "{name}: {first}");
    }
    // n01 und n03 (Einrueckung, reservierte Namen) meldet schon der Tokenizer.
    for name in ["n01_bad_indent.takt", "n03_reserved_names.takt"] {
        let src = std::fs::read_to_string(root.join(name)).expect("lesbar");
        assert!(!tokenize(&src).errors.is_empty(), "{name}: kein Tokenizer-Fehler");
    }
    // n05 ist ein Semantikfall und parst.
    let src = std::fs::read_to_string(root.join("n05_open_enum_match.takt")).expect("lesbar");
    let toks = tokenize(&src);
    let (_, errors) = parse_file(&toks);
    assert!(errors.is_empty(), "n05: {errors:?}");
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
