//! Tokenisiert den Korpus (corpus-try/*.takt und corpus-try/ref/*.takt).
//! Positive Beispiele haben keine Fehler; n01 und n03 scheitern wie im Manifest.

use std::path::{Path, PathBuf};

use takt_syntax::{ErrorCode, TokenKind, tokenize};

fn takt_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .collect();
    files.sort();
    files
}

#[test]
fn corpus_tokenizes() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try"));
    let mut files = takt_files(root);
    files.extend(takt_files(&root.join("ref")));
    assert!(files.len() > 40, "Korpus unvollstaendig: {} Dateien", files.len());
    let mut failures = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path).expect("Korpusdatei lesbar");
        let toks = tokenize(&src);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        assert_eq!(toks.tokens.last().map(|t| t.kind), Some(TokenKind::Eof));
        let expected_code = match &name[..3] {
            "n01" => Some(ErrorCode::Indent),
            "n03" => Some(ErrorCode::Reserved),
            _ => None,
        };
        match (expected_code, toks.errors.first()) {
            (None, None) => {}
            (None, Some(e)) => failures.push(format!("{name}: unerwartet {e}")),
            (Some(code), Some(e)) if e.code == code => {}
            (Some(code), other) => failures.push(format!("{name}: erwartet {}, erhalten {other:?}", code.as_str())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
