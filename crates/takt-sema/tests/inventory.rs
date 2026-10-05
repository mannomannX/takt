//! Die Inventur gegen Referenz und Code (13.8, Schritt 25, FB-383, FB-400).
//!
//! `plan/features.csv` fuehrt je Abschnitt, Absatz, Regel, Konstrukt und
//! Werkzeug der Referenz eine Zeile mit Status. Bisher hielt sie niemand
//! fest: Zeilen standen auf `fertig`, deren Merkmal nie lief, und Absaetze
//! hatten keine Zeile. Dieser Test verlangt:
//!
//! - Jede nummerierte Ueberschrift und jeder fett gesetzte Absatz der
//!   Abschnitte 0 bis 14 hat eine Zeile, jede Pruefung der Tabelle 10 eine
//!   Zeile `SC-n`.
//! - Jede Zeile `fertig` hat in der Spalte `Beleg` einen Beleg, den es gibt:
//!   eine Testfunktion (`crates/…/datei.rs::name`), eine Datei oder ein
//!   Verzeichnis, das eine Suite faehrt, oder `abgeleitet`. Dann leitet der
//!   Test ihn selbst ab: Schluesselwoerter und kontextuelle Woerter aus dem
//!   Korpus, reservierte Woerter am Tokenizer, Membernamen, Attribute und
//!   `system`-Eintraege aus den Programmen, die fehlerfrei uebersetzen,
//!   Pruefungen aus `corpus-try/checks/`, Produktionen der Grammatik aus der
//!   Zaehlung des Parsers (`takt_syntax::parser::productions_of`) ueber
//!   dieselben Programme.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use takt_sema::{Build, Options};
use takt_syntax::{TokenKind, tokenize};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Eine Zeile der Inventur.
struct Row {
    id: String,
    category: String,
    section: String,
    name: String,
    status: String,
    evidence: String,
}

/// `plan/features.csv`, mit einem Leser fuer Felder in Anfuehrungszeichen.
fn inventory() -> Vec<Row> {
    let text = std::fs::read_to_string(workspace().join("plan/features.csv")).expect("Inventur lesbar");
    let mut lines = text.lines();
    let header = fields(lines.next().expect("Kopf"));
    let col = |name: &str| header.iter().position(|h| h == name).unwrap_or_else(|| panic!("Spalte {name}"));
    let (id, cat, sec, name, status, evidence) =
        (col("ID"), col("Kategorie"), col("Abschnitt"), col("Bezeichnung"), col("Status"), col("Beleg"));
    lines
        .map(|l| {
            let f = fields(l);
            Row {
                id: f[id].clone(),
                category: f[cat].clone(),
                section: f[sec].clone(),
                name: f[name].clone(),
                status: f[status].clone(),
                evidence: f[evidence].clone(),
            }
        })
        .collect()
}

/// Die Felder einer CSV-Zeile (RFC 4180 ohne Zeilenumbrueche im Feld).
fn fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                out.last_mut().expect("Feld").push('"');
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(String::new()),
            _ => out.last_mut().expect("Feld").push(c),
        }
    }
    out
}

/// Kleinbuchstaben und Ziffern ohne Klammerzusatz, Umlaute umschrieben.
fn norm(s: &str) -> String {
    let mut depth = 0;
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth > 0 => {}
            '\u{e4}' => out.push_str("ae"),
            '\u{f6}' => out.push_str("oe"),
            '\u{fc}' => out.push_str("ue"),
            '\u{df}' => out.push_str("ss"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            _ => {}
        }
    }
    out
}

/// Abschnitte 0 bis 14 der Referenz: Ueberschriften und fette Absaetze, je
/// mit ihrem Abschnitt.
fn reference() -> (Vec<String>, Vec<(String, String)>) {
    let text = std::fs::read_to_string(workspace().join("plan/definition.md")).expect("Referenz lesbar");
    let body = text.find("\n## Anhang").map_or(text.as_str(), |i| &text[..i]);
    let mut headings = Vec::new();
    let mut paragraphs = Vec::new();
    let mut section = String::new();
    for line in body.lines() {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if (2..=4).contains(&hashes) {
            let rest = line[hashes..].trim_start();
            let number: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            let number = number.trim_end_matches('.').to_string();
            if !number.is_empty() && rest[number.len()..].starts_with([' ', '.']) {
                headings.push(number.clone());
                section = number;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("**")
            && let Some(end) = rest.find("**")
            && !section.is_empty()
        {
            paragraphs.push((section.clone(), rest[..end].trim().trim_end_matches('.').to_string()));
        }
    }
    (headings, paragraphs)
}

/// **Jede Ueberschrift, jeder fette Absatz und jede Pruefung hat eine Zeile.**
#[test]
fn every_part_of_the_reference_has_a_row() {
    let rows = inventory();
    let (headings, paragraphs) = reference();
    assert!(headings.len() >= 100 && paragraphs.len() >= 100, "Referenz nicht gelesen");
    let sections: BTreeSet<&str> =
        rows.iter().flat_map(|r| r.section.split([',', ';', '/']).map(str::trim)).filter(|s| !s.is_empty()).collect();
    let mut wrong = Vec::new();
    for h in &headings {
        let covered = sections.iter().any(|s| s == h || s.starts_with(&format!("{h}.")));
        if !covered {
            wrong.push(format!("Abschnitt {h}: keine Zeile"));
        }
    }
    let names: Vec<String> = rows.iter().map(|r| norm(&r.name)).collect();
    for (section, title) in &paragraphs {
        let key: String = norm(title).chars().take(30).collect();
        if !names.iter().any(|n| n.contains(&key)) {
            wrong.push(format!("{section}: Absatz `{title}` hat keine Zeile"));
        }
    }
    let text = std::fs::read_to_string(workspace().join("plan/definition.md")).expect("Referenz lesbar");
    let start = text.find("| # | Analyse | Fehler / Warnung |").expect("Tabelle 10");
    for row in text[start..].lines().skip(2).take_while(|l| l.starts_with('|')) {
        let n = row.trim_matches('|').split('|').next().expect("Nummer").trim();
        if !rows.iter().any(|r| r.id == format!("SC-{n}")) {
            wrong.push(format!("Pruefung {n}: keine Zeile `SC-{n}`"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Die Programme unter `corpus-try` und `crates` ausser dem Prelude:
/// Quelltext und ob er fehlerfrei uebersetzt.
fn corpus() -> Vec<(String, bool)> {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.expect("Verzeichniseintrag").path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|x| x == "takt") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for dir in ["corpus-try", "crates"] {
        collect(&workspace().join(dir), &mut files);
    }
    files.retain(|f| !f.ends_with("takt-sema/src/prelude.takt"));
    files
        .iter()
        .map(|f| {
            let src = std::fs::read_to_string(f).expect("lesbar");
            let dir = f.parent().expect("Verzeichnis");
            let channel_imports = takt_sema::channel_imports(&src)
                .into_iter()
                .filter_map(|name| std::fs::read_to_string(dir.join(&name)).ok().map(|text| (name, text)))
                .collect();
            let options = Options { build: Build::Sim, channel_imports, ..Default::default() };
            let clean = !takt_sema::compile(&src, &options).diagnostics.iter().any(|d| d.is_error());
            (src, clean)
        })
        .collect()
}

/// Die Produktionen, die der Parser in den Programmen lief, die fehlerfrei
/// uebersetzen.
fn productions(corpus: &[(String, bool)]) -> &'static BTreeSet<&'static str> {
    static REACHED: OnceLock<BTreeSet<&'static str>> = OnceLock::new();
    REACHED.get_or_init(|| {
        corpus
            .iter()
            .filter(|(_, ok)| *ok)
            .flat_map(|(src, _)| takt_syntax::parser::productions_of(&tokenize(src), false))
            .collect()
    })
}

/// Der Inhalt eines Bezeichners der Form `attr: "safe"`, sonst er selbst.
fn quoted(name: &str) -> &str {
    name.split('"').nth(1).unwrap_or(name)
}

/// Leitet den Beleg einer Zeile ab.
fn derived(row: &Row, corpus: &[(String, bool)]) -> bool {
    let word = quoted(&row.name).trim();
    let clean = || corpus.iter().filter(|(_, ok)| *ok).map(|(src, _)| src.as_str());
    let whole = |line: &str| line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').any(|w| w == word);
    match row.category.as_str() {
        // Ein lexikalisches Merkmal: Der Tokenizer erkennt das Wort in einer
        // Datei ohne Tokenizerfehler.
        "Schl\u{fc}sselwort" | "Kontextuelles Wort" => corpus.iter().any(|(src, _)| {
            let toks = tokenize(src);
            toks.errors.is_empty()
                && toks
                    .tokens
                    .iter()
                    .any(|t| matches!(t.kind, TokenKind::Keyword | TokenKind::Ident) && toks.text(t) == word)
        }),
        "Reserviertes Wort" => {
            let src = format!("var {word} = 1\n");
            tokenize(&src).errors.iter().any(|d| d.code == "E_RESERVED")
        }
        "Reservierter Membername" => {
            let stem = word.trim_end_matches('*');
            clean().any(|src| {
                src.match_indices(&format!(".{stem}")).any(|(i, m)| {
                    let next = src[i + m.len()..].chars().next();
                    word.ends_with('*') || !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                })
            })
        }
        "Channel-Attribut" => {
            clean().any(|src| src.lines().filter_map(|l| l.split_once(" with ").map(|(_, w)| w)).any(&whole))
        }
        "system-Eintrag" => clean().any(|src| {
            src.lines().skip_while(|l| !l.starts_with("system:")).skip(1).take_while(|l| l.starts_with(' ')).any(whole)
        }),
        "Statische Pr\u{fc}fung" => workspace().join("corpus-try/checks").join(&row.id).is_dir(),
        // Eine Produktion: Ihre Parserfunktion lief in einem Programm, das
        // fehlerfrei uebersetzt.
        "Grammatik" => productions(corpus).contains(word),
        _ => false,
    }
}

/// Ob ein Eintrag der Spalte `Beleg` existiert: eine Testfunktion mit
/// `#[test]` oder eine Datei bzw. ein Verzeichnis.
fn exists(entry: &str) -> bool {
    let root = workspace();
    match entry.split_once("::") {
        Some((file, name)) => {
            let Ok(text) = std::fs::read_to_string(root.join(file)) else { return false };
            let lines: Vec<&str> = text.lines().collect();
            lines.iter().enumerate().any(|(i, l)| {
                let l = l.trim_start();
                (l.starts_with(&format!("fn {name}(")) || l.starts_with(&format!("pub fn {name}(")))
                    && lines[..i]
                        .iter()
                        .rev()
                        .map(|a| a.trim_start())
                        .take_while(|a| a.starts_with("#[") || a.starts_with("///") || a.starts_with("//"))
                        .any(|a| a.starts_with("#[test]"))
            })
        }
        None => root.join(entry).exists(),
    }
}

/// **Jede Zeile `fertig` hat einen Beleg, den es gibt.**
#[test]
fn every_finished_row_names_evidence_that_exists() {
    let rows = inventory();
    let corpus = corpus();
    assert!(corpus.len() >= 300, "nur {} Korpusprogramme", corpus.len());
    let mut wrong = Vec::new();
    for row in rows.iter().filter(|r| r.status == "fertig") {
        let entries: Vec<&str> = row.evidence.split("; ").map(str::trim).filter(|e| !e.is_empty()).collect();
        if entries.is_empty() {
            wrong.push(format!("{}: `fertig` ohne Beleg", row.id));
        }
        for entry in entries {
            let ok = if entry == "abgeleitet" { derived(row, &corpus) } else { exists(entry) };
            if !ok {
                wrong.push(format!("{}: Beleg `{entry}` gibt es nicht", row.id));
            }
        }
    }
    assert!(wrong.is_empty(), "{} Zeilen:\n{}", wrong.len(), wrong.join("\n"));
}
