//! Golden-Test: die S-Expression-Ausgabe jeder positiven Korpusdatei liegt in
//! `corpus-try/ast/<name>.sexpr`. Aendert sich der Baum, schlaegt der Test fehl;
//! `UPDATE_GOLDEN=1 cargo test -p takt-syntax --test sexpr_golden` schreibt neu.

use std::path::{Path, PathBuf};

use takt_syntax::{parse_file, sexpr, tokenize};

fn corpus_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try"))
}

fn takt_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("Korpus lesbar")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .filter(|p| !p.file_name().and_then(|n| n.to_str()).unwrap_or_default().starts_with("n0"))
        .collect();
    files.sort();
    files
}

#[test]
fn corpus_trees_match_golden_files() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let golden_dir = corpus_root().join("ast");
    std::fs::create_dir_all(&golden_dir).expect("ast-Verzeichnis");
    let mut mismatches = Vec::new();
    for path in takt_files(&corpus_root()) {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("Dateiname").to_string();
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let toks = tokenize(&src);
        let (file, errors) = parse_file(&toks);
        assert!(errors.is_empty(), "{stem}: {errors:?}");
        let actual = sexpr::file(&file);
        let golden = golden_dir.join(format!("{stem}.sexpr"));
        if update {
            std::fs::write(&golden, &actual).expect("Golden-Datei schreibbar");
            continue;
        }
        match std::fs::read_to_string(&golden) {
            Ok(expected) if expected == actual => {}
            Ok(expected) => {
                let line = expected.lines().zip(actual.lines()).position(|(a, b)| a != b).map(|i| i + 1);
                mismatches.push(format!("{stem}: weicht ab ab Zeile {}", line.unwrap_or(0)));
            }
            Err(_) => mismatches.push(format!("{stem}: keine Golden-Datei")),
        }
    }
    assert!(
        mismatches.is_empty(),
        "{}\nNeu schreiben: UPDATE_GOLDEN=1 cargo test -p takt-syntax --test sexpr_golden",
        mismatches.join("\n")
    );
}
