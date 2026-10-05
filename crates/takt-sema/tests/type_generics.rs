//! Typvariablen in Generics (Referenz 3.12, v1.2; Pruefung 52):
//! `[type T: capability]` aus den Argumenten unifiziert oder explizit,
//! je Typ eine Instanz, Faehigkeit als Vorbedingung.

use takt_diag::Policy;
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> (Option<Program>, Vec<String>) {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let diags: Vec<String> = out.diagnostics.iter().map(|d| format!("{d}")).collect();
    let errors = out.diagnostics.iter().any(|d| d.is_error());
    (if errors { None } else { out.program }, diags)
}

fn ok(body: &str) -> Program {
    let (p, diags) = compile(body);
    p.unwrap_or_else(|| panic!("unerwartete Fehler:\n{}", diags.join("\n")))
}

fn errors(body: &str) -> String {
    let (p, diags) = compile(body);
    assert!(p.is_none(), "Fehler erwartet, bekam:\n{}", diags.join("\n"));
    diags.join("\n")
}

const BIGGEST: &str = "
fn biggest[type T: ord, const N in 1..8](xs: [N] T) -> T:
    var best = xs[0]
    for i in range(N):
        if xs[i] > best:
            best = xs[i]
    return best
";

/// Die Typvariable faellt aus der Struktur des Parameters (3.12).
#[test]
fn a_type_variable_is_inferred_from_the_argument() {
    let p = ok(&format!(
        "{BIGGEST}
output peak : int in 0..99 @ hw(\"o/peak\") with safe = 0

machine m:
    var xs : [4] int in 0..99 = default

    initial RUN

    state RUN:
        loop:
            peak = biggest(xs)
"
    ));
    let names: Vec<&str> = p.fns.iter().map(|f| f.name.as_str()).collect();
    assert!(names.iter().any(|n| n.contains("biggest[int in 0..99, 4]")), "{names:?}");
}

/// Zwei Typen, zwei Instanzen — der Memo dedupliziert nur Gleiches.
#[test]
fn each_type_gets_its_own_instance() {
    let p = ok(&format!(
        "{BIGGEST}
output peak  : int in 0..99 @ hw(\"o/peak\")  with safe = 0
output volts : float[V]     @ hw(\"o/volts\") with safe = 0 V

machine m:
    var xs : [4] int in 0..99 = default
    var ys : [4] float[V] = default

    initial RUN

    state RUN:
        loop:
            peak  = biggest(xs)
            volts = biggest(ys)
"
    ));
    let n = p.fns.iter().filter(|f| f.name.starts_with("biggest[")).count();
    assert_eq!(n, 2, "{:?}", p.fns.iter().map(|f| &f.name).collect::<Vec<_>>());
}

/// Derselbe Typ zweimal ist eine Instanz.
#[test]
fn the_same_type_is_memoised() {
    let p = ok(&format!(
        "{BIGGEST}
output a : int in 0..99 @ hw(\"o/a\") with safe = 0
output b : int in 0..99 @ hw(\"o/b\") with safe = 0

machine m:
    var xs : [4] int in 0..99 = default
    var ys : [4] int in 0..99 = default

    initial RUN

    state RUN:
        loop:
            a = biggest(xs)
            b = biggest(ys)
"
    ));
    let n = p.fns.iter().filter(|f| f.name.starts_with("biggest[")).count();
    assert_eq!(n, 1, "{:?}", p.fns.iter().map(|f| &f.name).collect::<Vec<_>>());
}

/// Ein explizites Typargument bindet die Variable ohne Inferenz.
#[test]
fn a_type_argument_can_be_given_explicitly() {
    ok(&format!(
        "{BIGGEST}
output peak : int in 0..99 @ hw(\"o/peak\") with safe = 0

machine m:
    var xs : [4] int in 0..99 = default

    initial RUN

    state RUN:
        loop:
            peak = biggest[int in 0..99, 4](xs)
"
    ));
}

/// Pruefung 52: die Faehigkeit gilt vor dem Rumpf und meldet an der Stelle.
#[test]
fn a_missing_capability_is_refused_at_the_call_site() {
    let e = errors(
        "
fn twice[type T: numeric](x: T) -> T:
    return x + x

output n : bool @ hw(\"o/n\") with safe = false

machine m:
    initial RUN

    state RUN:
        loop:
            n = twice(true)
",
    );
    assert!(e.contains("SC-52") && e.contains("braucht `numeric`"), "{e}");
}

/// `eq` schliesst Fliesskomma aus, wie `map` es fuer Schluessel tut (3.9).
#[test]
fn eq_does_not_hold_for_float() {
    let e = errors(
        "
fn same[type T: eq](a: T, b: T) -> bool:
    return a == b

input  x : float[V] in 0..5 V @ sim(\"i/x\")
output n : bool               @ hw(\"o/n\") with safe = false

machine m:
    initial RUN

    state RUN:
        loop:
            n = same(x, x)
",
    );
    assert!(e.contains("SC-52") && e.contains("braucht `eq`"), "{e}");
}

/// Pruefung 52: wachsende Argumente terminieren nicht.
#[test]
fn an_instantiation_cycle_is_refused() {
    let e = errors(
        "
fn grow[type T: pod](x: T) -> bool:
    var a : [1] T = [x]
    return grow(a)

output n : bool @ hw(\"o/n\") with safe = false

machine m:
    initial RUN

    state RUN:
        loop:
            n = grow(true)
",
    );
    assert!(e.contains("SC-52") && e.contains("endet nach"), "{e}");
}

/// Pruefung 52 als Tabelle (Faehigkeit, unpassender Typ): Die Meldung
/// nennt Variable, Faehigkeit und Typ an der Aufrufstelle.
#[test]
fn each_capability_refuses_an_unfitting_type() {
    for (decl, var, call, want) in [
        (
            "record Pair:\n    a : int\n    b : int\n",
            "var xs : [4] Pair = default",
            "n = biggest(xs).a",
            "`T` von `biggest` braucht `ord`, `Pair` hat es nicht",
        ),
        ("", "var v : float = 3.0", "n = half(v) > 1.0", "`T` von `half` braucht `integer`, `float` hat es nicht"),
        ("", "var v : int = 3", "n = root(v) > 1", "`T` von `root` braucht `float`, `int` hat es nicht"),
        ("", "var v : int? = none", "n = keep(v)", "`T` von `keep` braucht `pod`, `int?` hat es nicht"),
    ] {
        let e = errors(&format!(
            "{BIGGEST}
fn half[type T: integer](x: T) -> T:
    return x / 2

fn root[type T: float](x: T) -> T:
    return x

fn keep[type T: pod](x: T) -> bool:
    return true

{decl}
output n : int @ hw(\"o/n\") with safe = 0

machine m:
    {var}
    initial RUN
    state RUN:
        loop:
            {call}
"
        ));
        assert!(e.contains("SC-52") && e.contains(want), "{want}:\n{e}");
    }
}

/// 3.12: Ein Typparameter kann explizit stehen, auch als einfacher
/// Typname; widerspricht er dem Argument, ist das ein Typfehler.
#[test]
fn an_explicit_simple_type_argument_is_a_type() {
    let p = ok(&format!(
        "{BIGGEST}
output peak : bool @ hw(\"o/peak\") with safe = false

machine m:
    var xs : [4] bool = default
    initial RUN
    state RUN:
        loop:
            peak = biggest[bool, 4](xs)
"
    ));
    let names: Vec<&str> = p.fns.iter().map(|f| f.name.as_str()).collect();
    assert!(names.iter().any(|n| n.contains("biggest[bool, 4]")), "{names:?}");
    let e = errors(&format!(
        "{BIGGEST}
output peak : int in 0..99 @ hw(\"o/peak\") with safe = 0

machine m:
    var xs : [4] int in 0..99 = default
    initial RUN
    state RUN:
        loop:
            peak = biggest[u8, 4](xs)
"
    ));
    assert!(e.contains("SC-3") && e.contains("u8"), "{e}");
}

/// Eine Kette von `k` verschiedenen generischen Funktionen, jede ruft die
/// naechste mit demselben Typ.
fn chain(k: usize) -> String {
    let mut out = String::new();
    for i in 1..k {
        out.push_str(&format!("fn g{i}[type T: pod](x: T) -> T:\n    return g{}(x)\n\n", i + 1));
    }
    out.push_str(&format!(
        "fn g{k}[type T: pod](x: T) -> T:\n    return x\n\noutput n : bool @ hw(\"o/n\") with safe = false\n\n\
         machine m:\n    initial RUN\n    state RUN:\n        loop:\n            n = g1(true)\n"
    ));
    out
}

/// Pruefung 52 verlangt einen azyklischen Instanziierungsgraphen. Die
/// Tiefe 8 faengt wachsende Argumente ab (`an_instantiation_cycle_is_refused`);
/// eine endliche Kette verschiedener Funktionen ist kein Zyklus und
/// uebersetzt, mit acht Gliedern wie mit neun.
#[test]
fn a_finite_chain_of_instances_is_no_cycle() {
    ok(&chain(8));
    let (p, diags) = compile(&chain(9));
    assert!(p.is_some(), "neun verschiedene Instanzen sind kein Zyklus:\n{}", diags.join("\n"));
}

/// Ein Fehler im Rumpf einer Instanz nennt den Typ der Instanz.
#[test]
fn an_error_in_the_body_names_the_instance_type() {
    let e = errors(
        "
fn plus_one[type T: pod](x: T) -> T:
    return x + 1

output n : bool @ hw(\"o/n\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop:
            n = plus_one(true)
",
    );
    assert!(e.contains("SC-3") && e.contains("zwischen `bool` und `int`"), "{e}");
}

/// Ein `native` mit Typvariable meldet seine Stufe — und nur sie: Die
/// abgelehnte Deklaration zieht kein `nicht definiert` nach sich (FB-407).
#[test]
fn a_generic_native_reports_its_stage_once() {
    let src = format!(
        "{HEAD}native fn crc32[type T: pod](b: T) -> u32 with cost = 1600, stack = 32, total
output n : u32 @ hw(\"o/n\") with safe = 0
machine m:
    var b : bytes<8> = default
    initial RUN
    state RUN:
        loop:
            n = crc32(b)
"
    );
    let out = takt_sema::compile(&src, &Options::default());
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(out.diagnostics.iter().any(|d| d.stage.is_some()), "{errors:?}");
}
