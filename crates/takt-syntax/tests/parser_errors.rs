//! Gezielte Parserfaelle: Entscheidungen, die kein Korpusbeispiel abdeckt,
//! und Fehler, die der Parser mit Zeile und Vorschlag melden muss.

use takt_syntax::ast::{BinaryOp, ExprKind, Item, SnippetItem, StmtKind};
use takt_syntax::{ParseError, parse_file, parse_snippet, tokenize};

fn snippet(src: &str) -> (Vec<SnippetItem>, Vec<ParseError>) {
    let toks = tokenize(src);
    assert!(toks.errors.is_empty(), "Tokenizer: {:?}", toks.errors);
    parse_snippet(&toks)
}

fn file_errors(src: &str) -> Vec<ParseError> {
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
    assert_eq!(errors[0].line, 5);
    assert!(errors[0].message.contains("`enter` muss vor `when/after`"), "{}", errors[0]);
}

#[test]
fn machine_without_initial_reports_position() {
    let errors = file_errors("machine m:\n    var x = 1\n    state A:\n        loop:\n            pass\n");
    assert_eq!(errors[0].line, 3, "{}", errors[0]);
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
    assert_eq!(errors[0].line, 2);
    assert_eq!(file.items.len(), 2);
    assert!(matches!(file.items[1], Item::Const(_)));
}

#[test]
fn fn_without_return_type_needs_inout() {
    let errors = file_errors("fn f(x: u8):\n    pass\n");
    assert!(errors[0].message.contains("->"), "{}", errors[0]);
    assert!(file_errors("fn f(inout b: bytes<8>, x: u8):\n    pass\n").is_empty());
}

#[test]
fn errors_carry_a_suggestion() {
    let errors = file_errors(
        "machine m:\n    initial A\n    state A:\n        exit:\n            pass\n        loop:\n            pass\n",
    );
    assert!(errors[0].suggestion.as_deref().is_some_and(|s| s.contains("Reihenfolge")), "{:?}", errors[0]);
}
