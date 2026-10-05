//! Pathologische Eingaben (2.1, FB-396): Kein Werkzeug darf an ihnen
//! scheitern. Was zu tief verschachtelt ist, meldet der Parser mit
//! Vorschlag; was keine Zahl ergibt, der Tokenizer. Eine Ebene ist ein
//! eingerueckter Block, ein Klammerpaar (rund, eckig, Typklammer), ein
//! Praefixoperator und der `else`-Zweig eines bedingten Ausdrucks, gezaehlt
//! ueber alle Arten zusammen von der Datei an; Anweisungen, aeussere
//! Ausdruecke, Infix-Operatoren und Kettenglieder zaehlen nicht. Daneben ist
//! ein Ausdruck hoechstens 256 Knoten tief, jedes Kettenglied eingerechnet.

use takt_syntax::{format_snippet, parse_file, parse_snippet, sexpr, tokenize};

/// Die Grenze der Ebenen aus 2.1.
const LIMIT: usize = 64;

/// Die Grenze der Knoten eines Ausdrucks aus 2.1.
const NODES: usize = 256;

/// Die Parserfehler eines Schnipsels.
fn errors_of(src: &str) -> Vec<String> {
    let toks = tokenize(src);
    assert!(toks.errors.is_empty(), "Tokenizer: {:?}", toks.errors);
    parse_snippet(&toks).1.iter().map(|d| d.to_string()).collect()
}

fn accepted(what: &str, src: &str) {
    let errors = errors_of(src);
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

fn too_deep(what: &str, src: &str) {
    let errors = errors_of(src);
    assert!(errors.iter().any(|e| e.contains("zu tief verschachtelt")), "{what}: nicht abgelehnt: {errors:?}");
}

/// `n` verschachtelte `if`-Bloecke, innen `body`.
fn blocks(n: usize, body: &str) -> String {
    (0..n).map(|i| format!("{}if x:\n", "    ".repeat(i))).collect::<String>() + &"    ".repeat(n) + body + "\n"
}

/// Ein Programm mit `n` Ebenen einer Art.
type Nested = fn(usize) -> String;

/// Je Art genau 64 Ebenen angenommen, 65 abgelehnt.
#[test]
fn every_kind_of_level_is_bounded_at_64() {
    let kinds: [(&str, Nested); 8] = [
        ("runde Klammern", |n| format!("x = {}1{}\n", "(".repeat(n), ")".repeat(n))),
        ("eckige Klammern", |n| format!("x = {}1{}\n", "[".repeat(n), "]".repeat(n))),
        ("Typklammern", |n| format!("var v : {}u8{} = default\n", "vec<".repeat(n), ", 1>".repeat(n))),
        ("Vorzeichen", |n| format!("x = {}1\n", "- ".repeat(n))),
        ("not", |n| format!("x = {}true\n", "not ".repeat(n))),
        ("Bitnegation", |n| format!("x = {}1\n", "~".repeat(n))),
        ("else-Zweige", |n| format!("x = {}1\n", "1 if a else ".repeat(n))),
        ("Bloecke", |n| blocks(n, "pass")),
    ];
    for (what, make) in kinds {
        accepted(what, &make(LIMIT));
        too_deep(what, &make(LIMIT + 1));
    }
}

/// Die Arten zaehlen zusammen: 32 Bloecke und 32 Klammern sind 64 Ebenen.
#[test]
fn levels_of_different_kinds_add_up() {
    let mixed = |b: usize, p: usize| blocks(b, &format!("y = {}1{}", "(".repeat(p), ")".repeat(p)));
    accepted("32 + 32", &mixed(32, 32));
    too_deep("33 + 32", &mixed(33, 32));
    too_deep("32 + 33", &mixed(32, 33));
    let signs = |n: usize| format!("x = {}1{}\n", "(-".repeat(n), ")".repeat(n));
    accepted("32 Klammern mit Vorzeichen", &signs(32));
    too_deep("33 Klammern mit Vorzeichen", &signs(33));
}

/// Von der Datei an: Maschine, Zustand und `loop:` sind drei Bloecke.
#[test]
fn blocks_count_from_the_file() {
    let program = |n: usize| {
        let body: String = blocks(n, "pass").lines().map(|l| format!("            {l}\n")).collect();
        format!("machine m:\n    initial A\n    state A:\n        loop:\n{body}")
    };
    let errors = |src: &str| parse_file(&tokenize(src)).1.iter().map(|d| d.to_string()).collect::<Vec<_>>();
    assert!(errors(&program(LIMIT - 3)).is_empty(), "{:?}", errors(&program(LIMIT - 3)));
    assert!(errors(&program(LIMIT - 2)).iter().any(|e| e.contains("zu tief verschachtelt")));
}

/// Infix-Operatoren und Kettenglieder sind keine Ebene: 200 Glieder liegen weit
/// ueber 64 Ebenen und unter 256 Knoten.
#[test]
fn chains_and_sequences_are_no_levels() {
    for (what, expr) in [
        ("Summe", vec!["1"; 200].join(" + ")),
        ("und", vec!["a"; 200].join(" and ")),
        ("Felder", format!("a{}", ".b".repeat(199))),
        ("Indizes", format!("a{}", "[0]".repeat(199))),
        ("Aufrufe", format!("a{}", ".f()".repeat(199))),
    ] {
        accepted(what, &format!("x = {expr}\n"));
    }
}

/// Die Meldung eines zu tiefen Ausdrucks: (Byte-Anfang, Meldung, Vorschlag).
fn tree_error(src: &str) -> Option<(u32, String, String)> {
    let toks = tokenize(src);
    let errors = parse_snippet(&toks).1;
    errors
        .iter()
        .find(|d| d.message.starts_with("Ausdruck zu tief"))
        .map(|d| (d.span.start, d.message.clone(), d.suggestion.clone().unwrap_or_default()))
}

/// 2.1: Ein Ausdruck ist hoechstens 256 Knoten tief, jedes Kettenglied
/// eingerechnet - als Summe, als Methodenkette, als Feld- und Indexkette.
#[test]
fn an_expression_is_at_most_256_nodes_deep() {
    let kinds: [(&str, Nested); 5] = [
        ("Summe", |n| format!("x = {}\n", vec!["1"; n].join(" + "))),
        ("und", |n| format!("x = {}\n", vec!["a"; n].join(" and "))),
        ("Methodenkette", |n| format!("x = a{}\n", ".f()".repeat(n - 1))),
        ("Felder", |n| format!("x = a{}\n", ".b".repeat(n - 1))),
        ("Indizes", |n| format!("x = a{}\n", "[0]".repeat(n - 1))),
    ];
    for (what, make) in kinds {
        let ok = make(NODES);
        assert_eq!(tree_error(&ok), None, "{what}: {NODES} Glieder abgelehnt");
        accepted(what, &ok);
        assert!(tree_error(&make(NODES + 1)).is_some(), "{what}: {} Glieder angenommen", NODES + 1);
    }
}

/// Die Meldung steht am Glied, das die Grenze ueberschreitet, und schlaegt die
/// `var` vor (2.1).
#[test]
fn a_too_deep_expression_is_named_at_its_link() {
    let sum = format!("x = {}\n", vec!["1"; NODES + 1].join(" + "));
    let (at, message, suggestion) = tree_error(&sum).expect("abgelehnt");
    // `x = ` und je Glied `1 + `: das `+` vor dem 257. Glied.
    assert_eq!(at as usize, 4 + 4 * (NODES - 1) + 2);
    assert_eq!(message, "Ausdruck zu tief (mehr als 256 Knoten, jedes Kettenglied eingerechnet)");
    assert_eq!(suggestion, "den Ausdruck ueber eine `var` teilen (2.1)");
    let chain = format!("x = a{}\n", ".f()".repeat(NODES));
    let (at, ..) = tree_error(&chain).expect("abgelehnt");
    assert_eq!(at as usize, 5 + 4 * (NODES - 1), "der Punkt des 257. Glieds");
}

/// Klammern zaehlen als Knoten, und die Tiefe einer Kette in Klammern addiert
/// sich zu der aeusseren: 15 Klammerebenen mit je 17 Gliedern sind
/// 15 * 17 + 1 = 256 Knoten tief.
#[test]
fn chains_inside_brackets_add_up() {
    let nested = |levels: usize, links: usize, extra: usize| {
        let mut e = "1".to_string();
        for level in 0..levels {
            let more = if level + 1 == levels { extra } else { 0 };
            e = format!("({e}){}", " + 1".repeat(links - 1 + more));
        }
        format!("x = {e}\n")
    };
    let ok = nested(15, 17, 0);
    assert_eq!(tree_error(&ok), None);
    accepted("15 x 17", &ok);
    assert!(tree_error(&nested(15, 17, 1)).is_some(), "257 Knoten angenommen");
    // Dieselbe Tiefe in eckigen Klammern eines Index.
    let index = |n: usize| format!("x = a[{}]{}\n", vec!["1"; n].join(" + "), "[0]".repeat(NODES - n - 1));
    assert_eq!(tree_error(&index(200)), None);
    let deeper = format!("x = a[{}]{}\n", vec!["1"; 200].join(" + "), "[0]".repeat(NODES - 200));
    assert!(tree_error(&deeper).is_some(), "201 Knoten bis zum ersten Index, 56 Glieder dahinter");
    let under = format!("x = a[{}]\n", vec!["1"; NODES].join(" + "));
    assert!(tree_error(&under).is_some(), "Index mit 256 tiefem Inhalt: 257 Knoten");
}

/// Ein Ausdruck an der Grenze bringt weder Parser noch S-Expression noch
/// Formatter zum Absturz, auch auf dem Stapel des Aufrufers.
#[test]
fn an_expression_at_the_limit_passes_every_tool() {
    for src in [format!("x = {}\n", vec!["1"; NODES].join(" + ")), format!("x = a{}\n", ".f()".repeat(NODES - 1))] {
        let toks = tokenize(&src);
        let (items, errors) = parse_snippet(&toks);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(sexpr::snippet(&items).starts_with("(= x ("));
        assert!(!format!("{items:?}").is_empty());
        assert_eq!(items.clone(), items);
        assert_eq!(format_snippet(&src).expect("formatiert"), src);
        drop(items);
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

/// Ein Megabyte-String und hunderttausend Tokens in einer Zeile: kein Fehler,
/// nichts geht verloren.
#[test]
fn very_long_lines_and_literals_tokenize() {
    let text = "a".repeat(1 << 20);
    let src = format!("x = \"{text}\"\n");
    let toks = tokenize(&src);
    assert!(toks.errors.is_empty(), "{:?}", toks.errors);
    assert_eq!(toks.unescape(&toks.tokens[2]), text);
    let n = 100_000;
    let src = format!("x = [{}]\n", vec!["1"; n].join(", "));
    let toks = tokenize(&src);
    assert!(toks.errors.is_empty(), "{:?}", toks.errors);
    // x = [ … ] NEWLINE EOF, dazwischen n Zahlen und n - 1 Kommas
    assert_eq!(toks.tokens.len(), 3 + 2 * n - 1 + 3);
}

/// Auch eine Eigenschaft (13.3) ist ein Ausdruck mit hoechstens 256 Knoten:
/// `always(...)` ueber einer Summe aus 255 Gliedern geht, aus 256 nicht.
#[test]
fn a_property_is_bounded_like_an_expression() {
    let property = |n: usize| format!("property p: always({} > 0)\n", vec!["a"; n].join(" + "));
    // Temporal, Vergleich und die Summe: 2 + n Knoten.
    assert_eq!(tree_error(&property(NODES - 2)), None);
    assert!(tree_error(&property(NODES - 1)).is_some(), "257 Knoten angenommen");
}
