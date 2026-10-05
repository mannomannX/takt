//! Die Referenz als Korpus (FB-401, KOR-034): Jedes vollstaendige Programm
//! in `plan/definition.md` uebersetzt, und `corpus-try/ref/` ist genau die
//! heutige Extraktion ihrer Codebloecke (`grammar/extract_snippets.py`).

use std::path::PathBuf;

use takt_diag::{Policy, SourceMap};
use takt_sema::{Build, Options};

/// Ein Codeblock der Referenz: Abschnitt, laufende Nummer darin, Text
/// (auf seine kleinste Einrueckung zurueckgesetzt) und die Zeile seines
/// Anfangs.
struct Block {
    section: String,
    n: u32,
    text: String,
    line: usize,
}

impl Block {
    /// Der Dateiname der Extraktion: `<abschnitt>_<nr>.takt`.
    fn file(&self) -> String {
        format!("{}_{:02}.takt", self.section.replace('.', "_"), self.n)
    }
}

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Die Codebloecke wie `grammar/extract_snippets.py`: Abschnitte aus
/// `### N.M` und `## N.`, je Abschnitt durchgezaehlt.
fn blocks() -> Vec<Block> {
    let text = std::fs::read_to_string(root().join("plan/definition.md")).expect("Referenz lesbar");
    let mut out = Vec::new();
    let mut section = "0".to_string();
    let mut counter: std::collections::BTreeMap<String, u32> = Default::default();
    let mut open: Option<(usize, Vec<&str>)> = None;
    for (i, line) in text.lines().enumerate() {
        if let Some((start, lines)) = &mut open {
            if line.trim_start().starts_with("```") {
                let nonblank: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
                let indent = nonblank.iter().map(|l| l.len() - l.trim_start_matches(' ').len()).min().unwrap_or(0);
                let body: Vec<&str> =
                    lines.iter().map(|l| if l.trim().is_empty() { "" } else { &l[indent..] }).collect();
                let n = counter.entry(section.clone()).or_insert(0);
                *n += 1;
                out.push(Block {
                    section: section.clone(),
                    n: *n,
                    text: format!("{}\n", body.join("\n").trim_end_matches('\n')),
                    line: *start,
                });
                open = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("### ") {
            section = rest.split_whitespace().next().unwrap_or_default().to_string();
        } else if let Some(rest) = line.strip_prefix("## ") {
            if let Some(number) = rest.split_once('.').map(|(n, _)| n).filter(|n| n.chars().all(|c| c.is_ascii_digit()))
            {
                section = number.to_string();
            }
        } else if line.trim_start().starts_with("```") {
            open = Some((i + 1, Vec::new()));
        }
    }
    out
}

/// Bloecke der Referenz, die bewusst nicht uebersetzen, mit Grund.
const NOT_COMPILING: &[(&str, &str)] = &[];

/// **Jedes vollstaendige Programm der Referenz uebersetzt** (FB-401): ein
/// Block, der mit `system:` beginnt. Warnungen sind erlaubt.
#[test]
fn every_complete_program_of_the_reference_compiles() {
    let programs: Vec<Block> = blocks().into_iter().filter(|b| b.text.starts_with("system:")).collect();
    assert!(programs.len() >= 5, "zu wenige Programme: {}", programs.len());
    let mut failures = Vec::new();
    for b in &programs {
        let name = b.file();
        let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
        let out = takt_sema::compile(&b.text, &options);
        let map = SourceMap::single(name.as_str(), b.text.as_str());
        let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| map.render_line(d)).collect();
        match (errors.is_empty(), NOT_COMPILING.iter().find(|(n, _)| *n == name)) {
            (true, None) | (false, Some(_)) => {}
            (true, Some((_, why))) => {
                failures.push(format!("{name} uebersetzt jetzt ({why}); aus NOT_COMPILING nehmen"))
            }
            (false, None) => failures.push(format!(
                "{name} (plan/definition.md, Block ab Zeile {}):\n  {}",
                b.line,
                errors.join("\n  ")
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Ein Ausschnitt ohne die Anmerkungen der erwarteten Fehler (`# ~ SC-n`,
/// SEM1-034): Sie gehoeren dem Korpus, nicht der Referenz.
fn without_annotations(text: &str) -> String {
    text.split('\n')
        .map(|l| match l.find("# ~").or_else(|| l.find("#~")) {
            Some(p) => l[..p].trim_end(),
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// **`corpus-try/ref/` ist die heutige Extraktion** (KOR-034, SYN-016): Jeder
/// Block hat eine Zeile im Manifest; ein `Schnipsel` steht bis auf die
/// Anmerkungen zeichengleich in seiner Datei, und keine Datei ist ohne
/// Block. Nach einer Aenderung der Referenz: `python
/// grammar/extract_snippets.py`, verwaiste Dateien loeschen und die
/// Anmerkungen wieder setzen (die Diagnosetests nennen die Zeilen).
#[test]
fn the_reference_snippets_are_the_current_extraction() {
    let dir = root().join("corpus-try/ref");
    let manifest = std::fs::read_to_string(dir.join("manifest.csv")).expect("Manifest");
    let rows: Vec<(String, String)> = manifest
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut cells = l.splitn(4, ',');
            Some((cells.next()?.to_string(), cells.nth(1)?.to_string()))
        })
        .collect();
    let mut failures = Vec::new();
    let blocks = blocks();
    for b in &blocks {
        let name = b.file();
        match rows.iter().find(|(n, _)| *n == name).map(|(_, s)| s.as_str()) {
            None => failures.push(format!("{name}: keine Zeile im Manifest")),
            Some("Schnipsel") => match std::fs::read_to_string(dir.join(&name)) {
                Ok(text) if without_annotations(&text) == b.text => {}
                Ok(_) => failures.push(format!("{name}: weicht vom Block ab Zeile {} ab", b.line)),
                Err(_) => failures.push(format!("{name}: Datei fehlt")),
            },
            Some(_) => {}
        }
    }
    for (name, _) in &rows {
        if !blocks.iter().any(|b| b.file() == *name) {
            failures.push(format!("{name}: Manifestzeile ohne Block"));
        }
    }
    for entry in std::fs::read_dir(&dir).expect("ref lesbar") {
        let name = entry.expect("Eintrag").file_name().to_string_lossy().into_owned();
        if name.ends_with(".takt") && !rows.iter().any(|(n, s)| *n == name && s == "Schnipsel") {
            failures.push(format!("{name}: Datei ohne Block (verwaist)"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
