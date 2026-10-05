//! Randfaelle des Formatters: leere und kommentarlose Dateien, Kommentare an
//! ungewoehnlichen Stellen, Zeilenenden, BOM und Tabulatoren, Fortsetzungen.

use takt_syntax::edition::declared_edition;
use takt_syntax::fmt::{insert_edition, verify};
use takt_syntax::{Edition, SourceMap, format, format_snippet};

fn snippet(src: &str) -> String {
    verify(src, true).unwrap_or_else(|e| panic!("{e}\n--- Eingabe ---\n{src}"))
}

#[test]
fn empty_and_comment_only_files() {
    assert_eq!(format("").unwrap(), "");
    assert_eq!(format("\n\n").unwrap(), "");
    assert_eq!(format("# nur ein Kommentar").unwrap(), "# nur ein Kommentar\n");
    assert_eq!(format("#a\n\n\n#b\n").unwrap(), "# a\n\n# b\n");
}

#[test]
fn line_endings_bom_and_tabs_are_repaired() {
    assert_eq!(format("const A = 1\r\nconst B = 2\r\n").unwrap(), "const A = 1\nconst B = 2\n");
    assert_eq!(format("\u{feff}const A = 1\n").unwrap(), "const A = 1\n");
    assert_eq!(format("const A = 1").unwrap(), "const A = 1\n");
    let tabs = "machine m:\n\tinitial A\n\tstate A:\n\t\tloop:\tpass\t# t\n";
    assert_eq!(format(tabs).unwrap(), "machine m:\n    initial A\n    state A:\n        loop: pass  # t\n");
    assert_eq!(format_snippet("log \"a\tb\"\n").unwrap(), "log \"a\tb\"\n", "Tabulator im String bleibt");
}

#[test]
fn comments_in_odd_places() {
    let src = "if a:  # kopf\n    x = 1\n    # vor elif, im Block\n# vor elif, aussen\nelif b:\n    x = 2\nelse:  # sonst\n    x = 3\n";
    assert_eq!(snippet(src), src);
    let src = "machine m:\n    initial A\n    state A:\n        loop:\n            pass\n            # tief\n        # mittel\n    # flach\n";
    assert_eq!(snippet(src), src);
    let src = "match x:\n    # vor case\n    case 1:\n        pass\n    case _:  # rest\n        pass\n";
    assert_eq!(snippet(src), src);
    let src = "enum Kind: A, B  # inline\nsystem:\n    tick = 1 ms  # eins\n    language = 1\n";
    assert_eq!(snippet(src), "enum Kind: A, B  # inline\nsystem:\n    tick     = 1 ms  # eins\n    language = 1\n");
}

#[test]
fn comments_and_blank_lines_inside_brackets() {
    let src = "var t = f(a,  # erstes\n          # zwischen\n\n          b)  # ende\n";
    assert_eq!(snippet(src), "var t = f(a,  # erstes\n          # zwischen\n          b)  # ende\n");
    let src = "var m = [\n    [1, 0],\n    [0, 1]\n]\n";
    assert_eq!(snippet(src), src);
    let src = "var n = g(h(a,\n            b),\n          c)\n";
    assert_eq!(snippet(src), src);
}

#[test]
fn comment_at_end_of_file_inside_block() {
    let src = "machine m:\n    initial A\n    state A:\n        loop:\n            pass\n        # letzte Zeile\n";
    assert_eq!(snippet(src), src);
    assert_eq!(snippet("x = 1\n# ende ohne Zeilenende"), "x = 1\n# ende ohne Zeilenende\n");
}

#[test]
fn comment_columns_align_only_over_neighbours() {
    let src = "x = 1  # a\nlonger = 2  # b\n\ny = 3  # c\nz = 4\nw = 5  # d\n";
    assert_eq!(snippet(src), "x = 1       # a\nlonger = 2  # b\n\ny = 3  # c\nz = 4\nw = 5  # d\n");
}

#[test]
fn author_layout_choices_survive() {
    let src = "record Rec:\n    a : u8\n\n    b : u8\nenum Kind:\n    A = 1\n    LONG = 2\n    B(x: u8)\n";
    assert_eq!(
        snippet(src),
        "record Rec:\n    a : u8\n\n    b : u8\nenum Kind:\n    A    = 1\n    LONG = 2\n    B(x: u8)\n"
    );
}

#[test]
fn non_ascii_in_strings_and_comments_counts_as_characters() {
    let src = "x = \"äöü\"  # ä\nlonger = 1  # b\n";
    assert_eq!(snippet(src), "x = \"äöü\"   # ä\nlonger = 1  # b\n", "9 und 10 Zeichen breit, Spalte 12");
    let src = "input  a : bool @ hw(\"ä\") with safe = false\ninput  bb : bool @ hw(\"x\") with safe = false\n";
    assert_eq!(
        snippet(src),
        "input  a  : bool @ hw(\"ä\") with safe = false\ninput  bb : bool @ hw(\"x\") with safe = false\n"
    );
}

#[test]
fn blank_line_after_a_leading_comment_in_a_block_survives() {
    let src = "loop:\n    # a\n\n    # b\n    x = 1\n";
    assert_eq!(snippet(src), src);
    assert_eq!(snippet("loop:\n\n    # a\n    x = 1\n"), "loop:\n    # a\n    x = 1\n");
}

#[test]
fn inline_body_does_not_join_alignment_runs() {
    let src = "enter: var x = 1\nvar longer = 2\nvar y = 3\n";
    assert_eq!(snippet(src), "enter: var x = 1\nvar longer = 2\nvar y      = 3\n");
}

#[test]
fn units_types_and_bit_headers_align() {
    let src = "unit psi = 6894.76 Pa\nunit torr = 133.322 Pa\ntype Temp = float[degC]\nrecord Rec:\n    magic : u16\n    flags_long : u16 with bits:\n        a : bool at 0\n";
    assert_eq!(
        snippet(src),
        "unit psi  = 6894.76 Pa\nunit torr = 133.322 Pa\ntype Temp = float[degC]\nrecord Rec:\n    magic      : u16\n    flags_long : u16 with bits:\n        a : bool at 0\n"
    );
}

#[test]
fn errors_do_not_panic_and_leave_input_alone() {
    for bad in ["machine m:\n    state A:\n", "x = (1\n", "        deep = 1\n", "a b c\n", "fn f(:\n"] {
        assert!(format(bad).is_err(), "{bad:?}");
        assert!(format_snippet(bad).is_err(), "{bad:?}");
    }
}

/// L1.5 erlaubt Tabulatoren im Kommentar, L7 und die Garantie aus format.md
/// lassen ihn unveraendert; auch hinter einem Anfuehrungszeichen im Kommentar.
#[test]
fn a_comment_keeps_its_tabs() {
    for src in ["x = 1  # a\tb\n", "x = 1  # \"a\tb\n", "# \"\tganze Zeile\n", "log \"a\"  # \"\tb\n"] {
        let out = verify(src, true).unwrap_or_else(|e| panic!("{src:?}: {e}"));
        assert_eq!(out, src);
    }
    assert_eq!(format_snippet("x\t= 1  # a\tb\n").unwrap(), "x = 1  # a\tb\n");
}

#[test]
fn mixed_line_endings_bom_and_continuation_tabs_are_repaired() {
    assert_eq!(
        format("const A = 1\r\nconst B = 2\nconst C = 3\r\n").unwrap(),
        "const A = 1\nconst B = 2\nconst C = 3\n"
    );
    assert_eq!(format("\u{feff}const A = 1\r\nconst B = 2\r\n").unwrap(), "const A = 1\nconst B = 2\n");
    assert_eq!(format_snippet("var t = f(a,\n\t  b)\n").unwrap(), "var t = f(a,\n          b)\n");
    let errors = format("const A = 1\rconst B = 2\n").expect_err("einzelnes \r");
    assert_eq!(errors.iter().map(|d| d.code).collect::<Vec<_>>(), ["E_CR"]);
}

/// Ein Fehler zeigt auf die Datei, wie sie auf der Platte liegt, nicht auf
/// den reparierten Text (BOM entfernt, Tabulator zu Leerzeichen).
#[test]
fn errors_point_into_the_unrepaired_file() {
    let place = |src: &str| {
        let errors = format_snippet(src).expect_err("Fehler");
        let d = &errors[0];
        (d.code, SourceMap::single("t", src).line_col(d.span), d.span.end - d.span.start)
    };
    assert_eq!(place("if a:\n\tx = = 1\n"), ("P", (2, 6), 1));
    assert_eq!(place("\u{feff}x = = 1\n"), ("P", (1, 6), 1), "die BOM zaehlt als Zeichen (L1.2)");
    assert_eq!(place("x =\t= 1\n"), ("P", (1, 5), 1));
}

/// `takt fmt --edition` (2.5) traegt `language` genau einmal ein, und das
/// Ergebnis parst: ohne und mit `system:`-Block, hinter fuehrenden
/// Kommentaren, hinter einer Deklaration vor `system:`, mit CRLF und BOM.
#[test]
fn the_edition_is_inserted_once_and_parses() {
    let n = Edition::LATEST.number();
    let cases = [
        (
            "machine m:\n    initial A\n".to_string(),
            format!("system:\n    language = {n}\n\nmachine m:\n    initial A\n"),
        ),
        (
            "# Kopf\n\nmachine m:\n    initial A\n".to_string(),
            format!("# Kopf\n\nsystem:\n    language = {n}\n\nmachine m:\n    initial A\n"),
        ),
        ("system:\n    tick = 1 ms\n".to_string(), format!("system:\n    language = {n}\n    tick = 1 ms\n")),
        (
            "const A = 1\nsystem:\n    tick = 1 ms\n".to_string(),
            format!("const A = 1\nsystem:\n    language = {n}\n    tick = 1 ms\n"),
        ),
        ("system:\r\n    tick = 1 ms\r\n".to_string(), format!("system:\r\n    language = {n}\n    tick = 1 ms\r\n")),
        ("\u{feff}system:\n    tick = 1 ms\n".to_string(), format!("system:\n    language = {n}\n    tick = 1 ms\n")),
    ];
    for (src, want) in cases {
        let out = insert_edition(&src, Edition::LATEST).unwrap_or_else(|| panic!("{src:?}: nichts eingetragen"));
        assert_eq!(out, want, "{src:?}");
        assert_eq!(declared_edition(&out).map(|(v, _)| v), Some(n.to_string()), "{src:?}");
        assert!(format(&out).is_ok(), "{src:?}: {:?}", format(&out));
    }
    assert_eq!(insert_edition("system:\n    language = 1\n", Edition::LATEST), None, "schon da");
    assert_eq!(insert_edition("x = = 1\n", Edition::LATEST), None, "parst nicht");
}
