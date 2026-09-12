//! Gezielte Parserfaelle: Entscheidungen, die kein Korpusbeispiel abdeckt,
//! und Fehler, die der Parser mit Zeile und Vorschlag melden muss.

use takt_syntax::ast::{BinaryOp, ExprKind, Item, SnippetItem, StmtKind};
use takt_syntax::{Diagnostic, SourceMap, parse_file, parse_snippet, tokenize};

/// Zeile (ab 1) einer Diagnose im Quelltext.
fn line_of(src: &str, d: &Diagnostic) -> u32 {
    SourceMap::single("t", src).line_col(d.span).0
}

fn snippet(src: &str) -> (Vec<SnippetItem>, Vec<Diagnostic>) {
    let toks = tokenize(src);
    assert!(toks.errors.is_empty(), "Tokenizer: {:?}", toks.errors);
    parse_snippet(&toks)
}

fn file_errors(src: &str) -> Vec<Diagnostic> {
    let toks = tokenize(src);
    assert!(toks.errors.is_empty(), "Tokenizer: {:?}", toks.errors);
    parse_file(&toks).1
}

fn only_stmt(src: &str) -> StmtKind {
    let (items, errors) = snippet(src);
    assert!(errors.is_empty(), "{errors:?}");
    match items.into_iter().next() {
        Some(SnippetItem::Seq(takt_syntax::ast::SeqItem::Stmt(s))) => s.kind,
        other => panic!("keine Anweisung: {other:?}"),
    }
}

#[test]
fn shift_right_is_two_joint_greater_signs() {
    let StmtKind::Expr(e) = only_stmt("x >> 2\n") else { panic!() };
    assert!(matches!(e.kind, ExprKind::Binary { op: BinaryOp::Shr, .. }));
    let (_, errors) = snippet("x > > 2\n");
    assert_eq!(errors.len(), 1, "`> >` mit Leerzeichen ist kein Shift: {errors:?}");
}

#[test]
fn greater_sign_closes_type_arguments() {
    let (_, errors) =
        snippet("var a : bytes<64> = default\nvar b : vec<vec<u8, 4>, 2> = default\nvar c : str<8> = \"x\"\n");
    assert!(errors.is_empty(), "{errors:?}");
    let (_, errors) = snippet("var ok : bool = (n > 3)\n");
    assert!(errors.is_empty(), "in Klammern vergleicht `>` wieder: {errors:?}");
}

#[test]
fn greater_sign_compares_again_inside_square_brackets() {
    let (_, errors) = snippet("var b : bytes<[8 >> 1, 2][0]> = default\n");
    assert!(errors.is_empty(), "{errors:?}");
    let (_, errors) = snippet("var c : bytes<x[1 > 0]> = default\n");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn property_atoms_are_comparisons() {
    let ok = "property p: not a and b < 3 implies always(f(x if c else y) == 1)\n";
    let (_, errors) = snippet(ok);
    assert!(errors.is_empty(), "{errors:?}");
    let (_, errors) = snippet("property q: x if a else b\n");
    assert!(errors[0].message.contains("Bedingungsform"), "{}", errors[0]);
    let (_, errors) = snippet("x = always(y)\n");
    assert!(!errors.is_empty(), "Temporaloperator ausserhalb einer Eigenschaft");
    let (_, errors) = snippet("property r: g(always(x))\n");
    assert!(!errors.is_empty(), "Temporaloperator in einem Argument");
}

#[test]
fn generic_arguments_are_classified_by_name_and_next_token() {
    let src = "x = f[U, 1/s, KiB/s, T?, N + 1, n * 2, (a > b), 3 K, bytes<4>, [4] u8](1)\n";
    let (items, errors) = snippet(src);
    assert!(errors.is_empty(), "{errors:?}");
    let sexpr = takt_syntax::sexpr::snippet(&items);
    assert!(sexpr.contains("[U 1/s KiB/s T? (+ N 1) (* n 2) (> a b) 3[K] bytes<4> [4]u8]"), "{sexpr}");
}

#[test]
fn nesting_is_bounded() {
    let deep = |n: usize| format!("x = {}1{}\n", "(".repeat(n), ")".repeat(n));
    assert!(snippet(&deep(30)).1.is_empty());
    let (_, errors) = snippet(&deep(80));
    assert!(errors[0].message.contains("verschachtelt"), "{}", errors[0]);
    let blocks =
        (0..80).map(|i| format!("{}if x:\n", "    ".repeat(i))).collect::<String>() + &"    ".repeat(80) + "pass\n";
    assert!(snippet(&blocks).1[0].message.contains("verschachtelt"));
}

#[test]
fn contextual_word_after_number_is_not_a_unit() {
    let (items, errors) = snippet("until x == 3 timeout 5 s -> FAULT\n");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(matches!(items[0], SnippetItem::Seq(takt_syntax::ast::SeqItem::Until { timeout: Some(_), .. })));
    let StmtKind::Var(v) = only_stmt("var p = 3 bar\n") else { panic!() };
    assert!(matches!(v.value.kind, ExprKind::Number { unit: Some(_), .. }));
}

#[test]
fn dimensionless_one_only_as_numerator() {
    let StmtKind::Var(v) = only_stmt("var k = 0.0005 1/s\n") else { panic!() };
    assert!(matches!(v.value.kind, ExprKind::Number { unit: Some(_), .. }));
    let (_, errors) = snippet("x = 1 1\n");
    assert_eq!(errors.len(), 1, "`1 1` ist kein Literal: {errors:?}");
}

#[test]
fn unit_literal_ends_at_the_first_space() {
    let StmtKind::Var(v) = only_stmt("var a = 5 K / min\n") else { panic!() };
    assert!(matches!(v.value.kind, ExprKind::Binary { op: BinaryOp::Div, .. }), "`5 K / min` ist eine Division");
    let (_, errors) = snippet("var b = 9.81 m/s^2 m/s^2\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn joint_operator_after_a_unit_literal_is_an_error() {
    assert!(snippet("x = 3 K^2 ^ y\n").1.is_empty());
    let (_, errors) = snippet("x = 3 K^2^y\n");
    assert!(errors[0].message.contains("Einheitenausdruck"), "{}", errors[0]);
    assert!(!snippet("x = 3 s*2\n").1.is_empty());
}

#[test]
fn stray_greater_sign_in_type_arguments_is_an_error() {
    let (_, errors) = snippet("var s : samples<float[A], 100> 100> = default\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn duration_type_takes_a_range() {
    let (_, errors) = snippet("param T : Duration in 1 ms..2 s = 25 ms\n");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn assignment_target_must_be_an_lvalue() {
    let (_, errors) = snippet("f(x) = 3\n");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("linke Seite"), "{}", errors[0]);
    let (_, errors) = snippet("a.b[1] += 3\nm[i, j] = 0\n");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn state_sections_keep_their_order() {
    let src = "machine m:\n    initial A\n    state A:\n        when x: -> B\n        enter:\n            y = 1\n    state B:\n        loop:\n            pass\n";
    let errors = file_errors(src);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(line_of(src, &errors[0]), 5);
    assert!(errors[0].message.contains("`enter` muss vor `when/after`"), "{}", errors[0]);
}

#[test]
fn machine_without_initial_reports_position() {
    let src = "machine m:\n    var x = 1\n    state A:\n        loop:\n            pass\n";
    let errors = file_errors(src);
    assert_eq!(line_of(src, &errors[0]), 3, "{}", errors[0]);
    assert!(errors[0].message.contains("initial"));
}

#[test]
fn transition_block_ends_with_goto() {
    let src = "machine m:\n    initial A\n    state A:\n        when x:\n            y = 1\n    state B:\n        loop:\n            pass\n";
    let errors = file_errors(src);
    assert!(errors.iter().any(|e| e.message.contains("-> ZIEL")), "{errors:?}");
}

#[test]
fn recovery_continues_after_a_bad_line() {
    let src = "const A = 1\nconst = 2\nconst C = 3\n";
    let toks = tokenize(src);
    let (file, errors) = parse_file(&toks);
    assert_eq!(errors.len(), 1);
    assert_eq!(line_of(src, &errors[0]), 2);
    assert_eq!(file.items.len(), 2);
    assert!(matches!(file.items[1], Item::Const(_)));
}

#[test]
fn fn_without_return_type_needs_inout() {
    let errors = file_errors("fn f(x: u8):\n    pass\n");
    assert!(errors[0].message.contains("->"), "{}", errors[0]);
    assert!(file_errors("fn f(inout b: bytes<8>, x: u8):\n    pass\n").is_empty());
}

/// FB-93: `->` setzt eine Zeile nicht fort (2.1), weil `-> ZIEL` eine
/// eigene Zeile bildet. Eine Signatur, die davor umbricht, lief in
/// dieselbe Meldung wie eine ohne Rueckgabetyp — und deren Vorschlag
/// (inout) passt dort nicht.
#[test]
fn a_signature_broken_before_the_arrow_names_the_bracket_rule() {
    let umbruch = file_errors("fn f(a: int, b: int)\n    -> int:\n        return a\n");
    let hint = umbruch[0].suggestion.as_deref().unwrap_or_default();
    assert!(hint.contains("Parameterliste"), "{:?}", umbruch[0]);

    // Ohne folgendes `->` bleibt der Hinweis auf `inout` der richtige.
    let ohne = file_errors("fn f(a: int, b: int):\n    return a\n");
    let hint = ohne[0].suggestion.as_deref().unwrap_or_default();
    assert!(hint.contains("inout"), "{:?}", ohne[0]);
}

#[test]
fn errors_carry_a_suggestion() {
    let errors = file_errors(
        "machine m:\n    initial A\n    state A:\n        exit:\n            pass\n        loop:\n            pass\n",
    );
    assert!(errors[0].suggestion.as_deref().is_some_and(|s| s.contains("Reihenfolge")), "{:?}", errors[0]);
}

/// L2.2a: Fortsetzung ohne Klammern. Der Korpus kann diese Faelle nicht
/// tragen, weil er kanonisch ist und der Formatter Fortsetzungen
/// zusammenzieht — geprueft wird darum hier, am Quelltext.
#[test]
fn a_line_continues_after_a_hanging_comma() {
    let wrapped = "\
input rx : stream<u8> @ hw(\"u/rx\") with max_rate = 1000 Hz,
                                         capacity = 8
";
    let joined = "input rx : stream<u8> @ hw(\"u/rx\") with max_rate = 1000 Hz, capacity = 8\n";
    assert!(file_errors(wrapped).is_empty(), "umgebrochen: {:?}", file_errors(wrapped));
    assert_eq!(sexpr_of(wrapped), sexpr_of(joined), "Umbruch aendert den Baum nicht");
}

#[test]
fn a_line_continues_before_a_joining_token() {
    for (wrapped, joined) in [
        ("x = a\n    + b\n", "x = a + b\n"),
        ("x = a\n    == b\n", "x = a == b\n"),
        ("x = a\n    and b\n", "x = a and b\n"),
        ("x = a\n    .f\n", "x = a.f\n"),
    ] {
        assert_eq!(sexpr_of_snippet(wrapped), sexpr_of_snippet(joined), "{wrapped:?}");
    }
}

#[test]
fn a_sign_and_an_arrow_still_open_a_line() {
    // `-` ist auch Vorzeichen, `->` leitet einen Uebergang ein: beide duerfen
    // die vorige Zeile nicht fortsetzen.
    let stmts = "x = 1\ny = -1\n";
    let (items, errors) = snippet(stmts);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(items.len(), 2, "zwei Anweisungen, keine Fortsetzung");

    let machine = "\
machine m:
    initial RUN
    state RUN:
        sequence:
            wait 1 ms
            -> DONE
    state DONE:
        loop:
            pass
";
    assert!(file_errors(machine).is_empty(), "{:?}", file_errors(machine));
}

/// S-Expression einer ganzen Datei, als Vergleichsform des Baums.
fn sexpr_of(src: &str) -> String {
    let toks = tokenize(src);
    assert!(toks.errors.is_empty(), "Tokenizer: {:?}", toks.errors);
    let (file, errors) = parse_file(&toks);
    assert!(errors.is_empty(), "Parser: {errors:?}");
    takt_syntax::sexpr::file(&file)
}

/// Baum eines Schnipsels ohne Spannen: der Umbruch verschiebt Positionen,
/// die Struktur darf sich nicht aendern.
fn sexpr_of_snippet(src: &str) -> String {
    let (items, errors) = snippet(src);
    assert!(errors.is_empty(), "{errors:?}");
    without_spans(&format!("{items:?}"))
}

/// Entfernt `Span { … }`-Bloecke aus einer Debug-Ausgabe.
fn without_spans(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("Span { ") {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// FB-21: `pub var` und `signal` auf Dateiebene sind richtig abgelehnt, aber
/// der Hinweis zaehlte nur auf, was *hier* erlaubt ist. Er nennt jetzt, wo
/// die Deklaration hingehoert — zwei Entwickler haben sie unabhaengig oben
/// geschrieben.
#[test]
fn a_machine_level_declaration_at_file_level_names_its_place() {
    let errors = file_errors("system:\n    language = 1\n    tick = 1 ms\n\npub var x : int in 0..9 = 0\n");
    let d = errors.first().expect("abgelehnt");
    let hint = d.suggestion.as_deref().unwrap_or_default();
    assert!(hint.contains("in einer Maschine"), "der Hinweis nennt die Maschinenebene: {hint}");

    let errors = file_errors("system:\n    language = 1\n    tick = 1 ms\n\nsignal fertig\n");
    let d = errors.first().expect("abgelehnt");
    let hint = d.suggestion.as_deref().unwrap_or_default();
    assert!(hint.contains("5.8"), "der Hinweis nennt 5.8: {hint}");
}
