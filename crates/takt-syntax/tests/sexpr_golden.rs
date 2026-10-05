//! Golden-Test: Die S-Expression jedes positiven Korpusprogramms liegt in
//! `corpus-try/ast/<name>.sexpr`, die jedes Referenzschnipsels in
//! `corpus-try/ast/ref/<name>.sexpr`. Aendert sich ein Baum, scheitert der Test.
//!
//! `UPDATE_GOLDEN=1 cargo test -p takt-syntax --test sexpr_golden` schreibt nur
//! die geaenderten Dateien neu und scheitert danach mit ihrer Liste: Eine
//! stehengebliebene Variable faerbt den Test so nicht dauerhaft gruen, und der
//! zweite Lauf ohne Variable bestaetigt das Ergebnis. Eine Golden-Datei ohne
//! Programm ist ein Fehler; der Update-Lauf entfernt sie und nennt sie mit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use takt_syntax::{parse_file, parse_snippet, sexpr, tokenize};
use takt_testkit::corpus;

/// Der Baum einer Datei als S-Expression; Programme und Schnipsel haben je
/// einen eigenen Einstieg.
fn tree(path: &Path, snippet: bool) -> String {
    let src = std::fs::read_to_string(path).expect("lesbar");
    let toks = tokenize(&src);
    assert!(toks.errors.is_empty(), "{}: {:?}", path.display(), toks.errors);
    if snippet {
        let (items, errors) = parse_snippet(&toks);
        assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        sexpr::snippet(&items)
    } else {
        let (file, errors) = parse_file(&toks);
        assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        sexpr::file(&file)
    }
}

#[test]
fn corpus_trees_match_golden_files() {
    let update = std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1");
    let root = corpus::root();
    let snippets = corpus::takt_files(&root.join("ref"));
    assert!(snippets.len() > 30, "zu wenige Schnipsel: {}", snippets.len());
    let sets: [(Vec<PathBuf>, PathBuf, bool); 2] =
        [(corpus::programs(), root.join("ast"), false), (snippets, root.join("ast").join("ref"), true)];
    let mut failures = Vec::new();
    let mut written = Vec::new();
    for (programs, golden_dir, snippet) in sets {
        std::fs::create_dir_all(&golden_dir).expect("ast-Verzeichnis");
        let mut stems = BTreeSet::new();
        for path in programs {
            let stem = path.file_stem().and_then(|s| s.to_str()).expect("Dateiname").to_string();
            let actual = tree(&path, snippet);
            let golden = golden_dir.join(format!("{stem}.sexpr"));
            let shown = golden.strip_prefix(&root).unwrap_or(&golden).display().to_string();
            match std::fs::read_to_string(&golden) {
                Ok(expected) if expected == actual => {}
                _ if update => {
                    std::fs::write(&golden, &actual).expect("Golden-Datei schreibbar");
                    written.push(shown);
                }
                Ok(expected) => {
                    let line = expected.lines().zip(actual.lines()).position(|(a, b)| a != b).map(|i| i + 1);
                    failures.push(format!("{shown}: weicht ab ab Zeile {}", line.unwrap_or(0)));
                }
                Err(_) => failures.push(format!("{shown}: keine Golden-Datei")),
            }
            stems.insert(stem);
        }
        let orphans = std::fs::read_dir(&golden_dir)
            .expect("ast-Verzeichnis lesbar")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "sexpr"))
            .filter(|p| !p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| stems.contains(s)));
        for orphan in orphans {
            let shown = orphan.strip_prefix(&root).unwrap_or(&orphan).display().to_string();
            if update {
                std::fs::remove_file(&orphan).expect("Golden-Datei entfernbar");
                written.push(format!("{shown} (entfernt, ohne Programm)"));
            } else {
                failures.push(format!("{shown}: Golden-Datei ohne Programm"));
            }
        }
    }
    let failures = failures.join("\n");
    assert!(
        failures.is_empty(),
        "{failures}\nNeu schreiben: UPDATE_GOLDEN=1 cargo test -p takt-syntax --test sexpr_golden"
    );
    assert!(
        written.is_empty(),
        "{} Golden-Dateien neu geschrieben, bitte pruefen und ohne UPDATE_GOLDEN wiederholen:\n{}",
        written.len(),
        written.join("\n")
    );
}

/// Der Baum ohne Spannen, als Vergleichsmass fuer [`distinct_trees_print_distinctly`].
fn shape(src: &str) -> (String, String) {
    let toks = tokenize(src);
    let (items, errors) = parse_snippet(&toks);
    assert!(toks.errors.is_empty() && errors.is_empty(), "{src}: {:?} {errors:?}", toks.errors);
    let mut debug = format!("{items:?}");
    while let Some(at) = debug.find("Span {") {
        let end = at + debug[at..].find('}').expect("Spannenende") + 1;
        debug.replace_range(at..end, "");
    }
    (debug, sexpr::snippet(&items))
}

/// Die S-Expression ist das Orakel des Golden-Tests und des Baumvergleichs in
/// `fmt::verify`: Zwei verschiedene Baeume duerfen nie gleich aussehen. Geprueft
/// an minimalen Paaren, die sich nur in einem Knoten unterscheiden.
#[test]
fn distinct_trees_print_distinctly() {
    let pairs = [
        ("x = a.f\n", "x = a.f()\n"),
        ("x = (a)\n", "x = a\n"),
        ("x = (a + b)\n", "x = a + b\n"),
        ("x = A\n", "x = A()\n"),
        ("x = f(a)\n", "x = f(a = a)\n"),
        ("x = a.f(1)\n", "x = a.f[1]\n"),
        ("x = a[1]\n", "x = a[1, 2]\n"),
        ("x = -(1)\n", "x = -1\n"),
    ];
    for (a, b) in pairs {
        let (debug_a, sexpr_a) = shape(a);
        let (debug_b, sexpr_b) = shape(b);
        assert_ne!(debug_a, debug_b, "kein minimales Paar: `{a}` und `{b}` haben denselben Baum");
        assert_ne!(sexpr_a, sexpr_b, "`{}` und `{}` ergeben dieselbe S-Expression", a.trim(), b.trim());
    }
}
