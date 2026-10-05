//! Der Korpus ist kanonisch: `format(s) == s` fuer jede positive Korpusdatei.
//! Damit ist der Korpus zugleich der Golden-Test des Formatters.

use takt_syntax::{format, format_snippet};
use takt_testkit::corpus;

#[test]
fn corpus_is_canonical() {
    let mut failures = Vec::new();
    for path in corpus::programs() {
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
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Auch die Programme ausserhalb des Korpus sind kanonisch: Bring-ups,
/// Mess- und Testprogramme, Beispiele, `feedback/`. Eine Datei, die von Hand
/// ausgerichtet wurde, faellt sonst erst beim naechsten `takt fmt` auf.
#[test]
fn every_program_outside_the_corpus_is_canonical() {
    let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let mut files = Vec::new();
    // Nur die Quellverzeichnisse; der Korpus hat seine eigenen Tests, und
    // was im Wurzelverzeichnis liegt (`out/`, Notizen), ist nicht eingecheckt.
    let mut dirs: Vec<_> = ["crates", "examples", "feedback"].iter().map(|d| root.join(d)).collect();
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("Verzeichnis").flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                // Bauverzeichnisse sind keine Quelle.
                if !(name.starts_with('.') || name.starts_with("target")) {
                    dirs.push(path);
                }
            } else if name.ends_with(".takt") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(!files.is_empty(), "keine Programme gefunden");
    let mut failures = Vec::new();
    for path in &files {
        let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string().replace('\\', "/");
        let src = std::fs::read_to_string(path).expect("lesbar");
        match format(&src) {
            Ok(out) if out == src => {}
            Ok(out) => {
                let line = src.lines().zip(out.lines()).position(|(a, b)| a != b).map_or(0, |i| i + 1);
                failures
                    .push(format!("{shown}: nicht kanonisch ab Zeile {line} (cargo run -p takt-cli -- fmt {shown})"));
            }
            Err(e) => failures.push(format!("{shown}: {}", e[0])),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
