//! Der Korpus ist kanonisch: `format(s) == s` fuer jede positive Korpusdatei.
//! Damit ist der Korpus zugleich der Golden-Test des Formatters.

use takt_syntax::{format, format_snippet};

#[test]
fn corpus_is_canonical() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try");
    let mut files: Vec<_> = std::fs::read_dir(root)
        .expect("Korpus lesbar")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .filter(|p| !p.file_name().and_then(|n| n.to_str()).unwrap_or_default().starts_with("n0"))
        .collect();
    files.sort();
    let mut failures = Vec::new();
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let src = std::fs::read_to_string(&path).expect("lesbar");
        match format(&src) {
            Ok(out) if out == src => {}
            Ok(out) => {
                let line = src.lines().zip(out.lines()).position(|(a, b)| a != b).map(|i| i + 1);
                failures.push(format!(
                    "{name}: nicht kanonisch ab Zeile {} (cargo run -p takt-cli -- fmt corpus-try/{name})",
                    line.unwrap_or(0)
                ));
            }
            Err(e) => failures.push(format!("{name}: {}", e[0])),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Die Codebloecke der Referenz (als Schnipsel extrahiert) sind kanonisch;
/// `python grammar/format_examples.py` schreibt sie um.
#[test]
fn reference_snippets_are_canonical() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/ref");
    let mut files: Vec<_> = std::fs::read_dir(root)
        .expect("Schnipsel lesbar")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .collect();
    files.sort();
    assert!(files.len() > 30);
    let mut failures = Vec::new();
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let src = std::fs::read_to_string(&path).expect("lesbar");
        match format_snippet(&src) {
            Ok(out) if out == src => {}
            Ok(out) => {
                let line = src.lines().zip(out.lines()).position(|(a, b)| a != b).map(|i| i + 1);
                failures.push(format!(
                    "{name}: nicht kanonisch ab Zeile {} (python grammar/format_examples.py)",
                    line.unwrap_or(0)
                ));
            }
            Err(e) => failures.push(format!("{name}: {}", e[0])),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
