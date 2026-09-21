//! Typvariablen in Generics (Referenz 3.12, v1.2; Pruefung 52):
//! `[type T: capability]` aus den Argumenten unifiziert oder explizit,
//! je Typ eine Instanz, Faehigkeit als Vorbedingung.

use takt_diag::Policy;
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> (Option<Program>, Vec<String>) {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
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
