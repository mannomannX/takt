//! Nachverfolgbarkeit: jede Produktion aus grammar/takt.ebnf hat eine Funktion
//! `parse_<produktion>` im Parser; die Teilsprachen in Strings haben `fn <name>`
//! in subtext.rs.

use std::fs;
use std::path::Path;

const SUBTEXT: &[&str] =
    &["pattern_text", "pattern_kind", "format_text", "format_spec", "address_text", "address_segment"];

/// Produktionen der Eigenschaften (13.3), die die gewoehnliche Vorrangkette mit
/// gesetztem `temporal` parst; die Abdeckung zaehlt sie dort unter ihrem Namen.
const PARSE_ALIASES: &[(&str, &str)] =
    &[("tprop_and", "and_expr"), ("tprop_not", "not_expr"), ("tprop_atom", "cmp_expr")];

fn read_all(dir: &Path, out: &mut String) {
    for entry in fs::read_dir(dir).expect("Verzeichnis lesbar") {
        let path = entry.expect("Eintrag").path();
        if path.is_dir() {
            read_all(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push_str(&fs::read_to_string(&path).expect("Quelle lesbar"));
        }
    }
}

/// Entfernt `(* … *)`-Kommentare, auch mehrzeilige (der Notationskopf).
fn strip_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find("(*") {
        out.push_str(&rest[..open]);
        rest = match rest[open..].find("*)") {
            Some(close) => &rest[open + close + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Produktionen, die der Formatter ueber den Baum druckt statt je Stufe.
const FMT_ALIASES: &[(&str, &str)] = &[
    ("or_expr", "expr"),
    ("and_expr", "expr"),
    ("not_expr", "expr"),
    ("cmp_expr", "expr"),
    ("bitor_expr", "expr"),
    ("bitxor_expr", "expr"),
    ("bitand_expr", "expr"),
    ("shift_expr", "expr"),
    ("add_expr", "expr"),
    ("mul_expr", "expr"),
    ("unary", "expr"),
    ("cast_expr", "expr"),
    ("postfix", "expr"),
    ("primary", "expr"),
    ("tprop_implies", "tprop"),
    ("tprop_or", "tprop"),
    ("tprop_and", "tprop"),
    ("tprop_not", "tprop"),
    ("tprop_atom", "tprop"),
    ("unit_name", "unit_decl"),
];

fn production_names(grammar: &str) -> Vec<String> {
    strip_comments(grammar)
        .lines()
        .filter_map(|l| l.split_once(":=").map(|(n, _)| n.trim().to_string()))
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        .collect()
}

#[test]
fn every_production_has_a_formatter_function() {
    let grammar =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/takt.ebnf")).expect("Grammatik lesbar");
    let mut source = String::new();
    read_all(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/fmt")), &mut source);
    let missing: Vec<String> = production_names(&grammar)
        .into_iter()
        .filter(|name| !SUBTEXT.contains(&name.as_str()))
        .filter(|name| {
            let target = FMT_ALIASES.iter().find(|(n, _)| n == name).map_or(name.as_str(), |(_, t)| t);
            !source.contains(&format!("fn fmt_{target}("))
        })
        .collect();
    assert!(missing.is_empty(), "Produktionen ohne Formatter-Funktion: {}", missing.join(", "));
}

#[test]
fn every_production_has_a_parser_function() {
    let grammar =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/takt.ebnf")).expect("Grammatik lesbar");
    let grammar = strip_comments(&grammar);
    let mut source = String::new();
    read_all(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut source);
    let mut missing = Vec::new();
    let mut count = 0;
    for line in grammar.lines() {
        let Some((name, _)) = line.split_once(":=") else { continue };
        let name = name.trim();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
            continue;
        }
        count += 1;
        let target = PARSE_ALIASES.iter().find(|(n, _)| *n == name).map_or(name, |(_, t)| t);
        let wanted = if SUBTEXT.contains(&name) { format!("fn {name}(") } else { format!("fn parse_{target}(") };
        if !source.contains(&wanted) {
            missing.push(name.to_string());
        }
    }
    assert!(count > 100, "zu wenige Produktionen gelesen: {count}");
    assert!(missing.is_empty(), "Produktionen ohne Funktion: {}", missing.join(", "));
}

/// Alle `.takt`-Dateien unter `dir`, rekursiv.
fn takt_below(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("Verzeichnis lesbar") {
        let path = entry.expect("Eintrag").path();
        if path.is_dir() {
            takt_below(&path, out);
        } else if path.extension().is_some_and(|x| x == "takt") {
            out.push(path);
        }
    }
}

/// Die Eingaben der Fmt-Vektoren aus grammar/format.md (Schnipsel).
fn format_vector_inputs() -> Vec<String> {
    let text = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/format.md")).expect("format.md");
    text.split("```fmt\n")
        .skip(1)
        .filter_map(|block| block.split_once("\n---\n").map(|(input, _)| format!("{input}\n")))
        .collect()
}

/// Produktionen, die weder Korpus noch Schnipsel noch Vektoren erreichen.
/// Eine Ratsche: Verliert eine Produktion ihren letzten Fall, scheitert der
/// Test; bekommt eine dieser hier einen, faellt sie aus der Liste.
const UNREACHED: &[&str] = &[];

/// **Jede Produktion hat einen Fall** (plan.md 3): Die Parserfunktion jeder
/// Produktion aus `takt.ebnf` laeuft beim Parsen von Korpus, Pruefungs- und
/// Beispielprogrammen, Referenzschnipseln und Formatvektoren mindestens einmal.
#[test]
fn every_production_is_reached_by_some_input() {
    use takt_syntax::parser::productions_of;
    use takt_syntax::tokenize;
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let mut files = Vec::new();
    for dir in ["corpus-try", "crates", "examples"] {
        takt_below(&root.join(dir), &mut files);
    }
    let mut hits = std::collections::BTreeSet::new();
    for path in &files {
        let src = fs::read_to_string(path).expect("lesbar");
        let snippet = path.parent().is_some_and(|d| d.ends_with("ref"));
        hits.extend(productions_of(&tokenize(&src), snippet));
    }
    for input in format_vector_inputs() {
        hits.extend(productions_of(&tokenize(&input), true));
    }
    let grammar = fs::read_to_string(root.join("grammar/takt.ebnf")).expect("Grammatik lesbar");
    let unreached: Vec<String> = production_names(&grammar)
        .into_iter()
        .filter(|name| !SUBTEXT.contains(&name.as_str()) && !hits.contains(name.as_str()))
        .collect();
    assert_eq!(unreached, UNREACHED, "Produktionen ohne Fall (UNREACHED nachziehen, wenn eine hinzukam)");
}
