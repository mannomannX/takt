//! Endlichkeit am Ende einer Kette (4.2, FB-294): Durch `+`, `-`, `*` und
//! den Zaehler einer Division pflanzt sich ein nicht endlicher Wert fort;
//! geprueft wird am Ergebnis der Kette und vor jedem Gebrauch, der ihn
//! verschlucken koennte — Divisor, Vergleich, Umwandlung. Jeder Fall laeuft
//! im Interpreter und im erzeugten Code und verlangt denselben Trace.

mod common;

use takt_interp::{RunOptions, Trace};
use takt_llvm::toolchain::{Clang, find};

const HEAD: &str = "\
system:
    language = 1
    tick     = 1 ms
    float    = f32

output probe : int in 0..100 @ hw(\"probe\") with safe = 0
output y     : float         @ hw(\"y\")     with safe = 0.0

";

/// Der Trace des Interpreters, wenn der erzeugte Code denselben liefert;
/// `None` ohne clang.
fn agree(body: &str, name: &str) -> Option<String> {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return None;
    };
    let src = format!("{HEAD}{body}");
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    assert!(!out.has_errors(), "{name}: {:?}", out.diagnostics);
    let p = out.program.expect("Programm");
    let interpreted =
        takt_interp::run(&p, &Trace::default(), &RunOptions { ticks: 4, ..Default::default() }).expect("Lauf");
    let interpreted = interpreted.trace.render();
    let native = common::run_native_all(&Clang::At(path), &p, name, 4).expect("nativ");
    let diffs = takt_conformance::compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{name}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    Some(interpreted)
}

/// Ein Programm, das in Tick 1 `expr` nach `y` schreibt; Tick 0 zeigt 1.
fn program(vars: &str, expr: &str) -> String {
    format!(
        "machine m:
    fault -> SAFE
{vars}
    initial RUN
    state RUN:
        enter:
            probe = 1
        after 1 ms: -> CALC
    state CALC:
        enter:
            y = {expr}
            probe = 2
    state SAFE:
        enter:
            probe = 9
"
    )
}

const BIG: &str =
    "    var a : float = 1e30\n    var b : float = 1e30\n    var c : float = 1.0\n    var z : float = 0.0\n";

#[test]
fn an_overflow_mid_chain_faults_at_the_end_of_the_chain() {
    let Some(t) = agree(&program(BIG, "a * b - a * b + c"), "chain_overflow") else { return };
    assert!(t.contains("t=1 fault m Arithmetic(NonFinite)"), "{t}");
    assert!(t.contains("t=1 out probe 9"), "{t}");
}

#[test]
fn a_non_finite_divisor_is_checked_before_it_swallows() {
    // `c / (a * b)` waere 0: Der Divisor prueft vorher.
    let Some(t) = agree(&program(BIG, "c / (a * b)"), "chain_divisor") else { return };
    assert!(t.contains("t=1 fault m Arithmetic(NonFinite)"), "{t}");
}

#[test]
fn a_zero_divisor_is_a_non_finite_numerator_of_the_next_step() {
    let Some(t) = agree(&program(BIG, "c / z * a + c"), "chain_zero") else { return };
    assert!(t.contains("t=1 fault m Arithmetic(NonFinite)"), "{t}");
}

#[test]
fn a_finite_chain_writes_its_value() {
    let vars = "    var a : float = 3.0\n    var b : float = 4.0\n";
    let Some(t) = agree(&program(vars, "-(a * b) + a / b"), "chain_finite") else { return };
    assert!(t.contains("t=1 out y -11.25"), "{t}");
    assert!(t.contains("t=1 out probe 2"), "{t}");
}
