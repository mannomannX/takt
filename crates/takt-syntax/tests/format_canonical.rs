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

/// Auch die Pruefungs-Korpora sind kanonisch. Sie liegen ein Verzeichnis
/// tiefer und blieben darum lange unbeachtet — sieben Dateien waren
/// abgedriftet. Ausgenommen ist `SC-50`: Dort stehen reservierte Woerter,
/// die der Lexer ablehnt, bevor der Formatter sie sieht.
#[test]
fn check_corpora_are_canonical() {
    let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/checks"));
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .expect("Korpus lesbar")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && !p.ends_with("SC-50"))
        .collect();
    dirs.sort();
    assert!(dirs.len() > 40, "die Pruefungs-Korpora sind da: {}", dirs.len());
    let mut failures = Vec::new();
    for dir in dirs {
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("Verzeichnis lesbar")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "takt"))
            .collect();
        files.sort();
        for path in files {
            let name = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
            let src = std::fs::read_to_string(&path).expect("lesbar");
            match format(&src) {
                Ok(out) if out == src => {}
                Ok(out) => {
                    let line = src.lines().zip(out.lines()).position(|(a, b)| a != b).map(|i| i + 1);
                    failures.push(format!("{name}: nicht kanonisch ab Zeile {}", line.unwrap_or(0)));
                }
                Err(e) => failures.push(format!("{name}: {}", e[0])),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        failures.join(
            "
"
        )
    );
}
