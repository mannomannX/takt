//! Pathologische Eingaben (2.1, FB-396): Kein Werkzeug darf an ihnen
//! scheitern. Was zu tief verschachtelt ist, meldet der Parser mit
//! Vorschlag; was keine Zahl ergibt, der Tokenizer. Gezaehlt wird jede Ebene
//! des Baums, auch die Glieder einer Kette `a + b + …` und `a.b.c…`: Sie
//! bauen einen ebenso tiefen Baum wie Klammern.

use takt_syntax::{parse_snippet, tokenize};

/// Wie oft ein Muster wiederholt wird: weit ueber der Grenze von 64 Ebenen.
const MANY: usize = 200;

/// Die Fehler beim Parsen einer Zuweisung `x = <expr>`.
fn errors_of(expr: &str) -> Vec<String> {
    let src = format!("x = {expr}\n");
    let toks = tokenize(&src);
    assert!(toks.errors.is_empty(), "Tokenizer: {:?}", toks.errors);
    parse_snippet(&toks).1.iter().map(|d| d.to_string()).collect()
}

fn too_deep(expr: &str) {
    let errors = errors_of(expr);
    assert!(errors.iter().any(|e| e.contains("zu tief verschachtelt")), "nicht abgelehnt: {errors:?}");
}

#[test]
fn deep_parentheses_are_refused() {
    too_deep(&format!("{}1{}", "(".repeat(MANY), ")".repeat(MANY)));
}

#[test]
fn a_tower_of_not_is_refused() {
    too_deep(&format!("{}true", "not ".repeat(MANY)));
}

#[test]
fn a_tower_of_unary_operators_is_refused() {
    too_deep(&format!("{}1", "- ".repeat(MANY)));
    too_deep(&format!("{}1", "~".repeat(MANY)));
}

#[test]
fn a_long_chain_of_operators_is_refused() {
    too_deep(&vec!["1"; MANY].join(" + "));
    too_deep(&vec!["a"; MANY].join(" and "));
    too_deep(&vec!["1"; MANY].join(" * "));
}

#[test]
fn a_long_chain_of_members_and_indices_is_refused() {
    too_deep(&format!("a{}", ".b".repeat(MANY)));
    too_deep(&format!("a{}", "[0]".repeat(MANY)));
}

/// Unter der Grenze bleibt alles, was ein Programm sinnvoll schreibt.
#[test]
fn ordinary_chains_parse() {
    for expr in [vec!["1"; 20].join(" + "), format!("a{}", ".b".repeat(10)), format!("{}1", "- ".repeat(10))] {
        let errors = errors_of(&expr);
        assert!(errors.is_empty(), "{expr}: {errors:?}");
    }
}

/// Exponenten am Rand von `i32`: ein Fehler des Tokenizers, keine Panik
/// (im Debug-Build ueberlief die Negation von `i32::MIN`).
#[test]
fn extreme_exponents_of_a_duration_are_errors() {
    for text in ["1e-2147483648 s", "1.5e-2147483648 s", "1e2147483647 s", "1e-2147483647 ms", "1e99999999999 s"] {
        let src = format!("x = {text}\n");
        let toks = tokenize(&src);
        assert!(!toks.errors.is_empty(), "`{text}` angenommen");
    }
}
