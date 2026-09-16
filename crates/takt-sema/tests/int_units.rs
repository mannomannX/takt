//! Einheiten auf Ganzzahlen (Referenz 3.2, v1.1; Pruefung 38): nominal wie
//! auf Fliesskomma, `.to(U)` nur bei ganzzahligem Faktor, sonst `.to_float(U)`.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

fn ok(body: &str) -> Program {
    compile(body).unwrap_or_else(|e| panic!("unerwartete Fehler:\n{}", e.join("\n")))
}

fn errors(body: &str) -> String {
    compile(body).expect_err("Fehler erwartet").join("\n")
}

fn trace(p: &Program, ticks: u64) -> String {
    run(p, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

const OUT: &str = "output n : int[uV] @ hw(\"o/n\") with safe = 0\n";

#[test]
fn a_literal_with_a_unit_types_an_integer() {
    let p = ok(&format!(
        "{OUT}
machine m:
    var v : int[mV] = 3300 mV
    initial RUN
    state RUN:
        loop:
            n = v.to(uV)
"
    ));
    let t = trace(&p, 2);
    assert!(t.contains("out n 3300000"), "{t}");
}

#[test]
fn to_multiplies_by_the_integral_factor_and_overflow_is_a_fault() {
    // 3.2: „Overflow ist ein Fault wie 4.1" — 3 000 000 mV sind 3e9 uV, mehr als i32.
    let p = ok("output n : i32[uV] @ hw(\"o/n\") with safe = 0
machine m:
    var v : i32[mV] = 3_000_000 mV
    initial RUN
    state RUN:
        loop:
            n = v.to(uV)
");
    let t = trace(&p, 2);
    assert!(t.contains("Ueberlauf"), "{t}");
}

#[test]
fn a_factor_that_cannot_fit_the_width_is_a_compile_error() {
    let e = errors(
        "output n : i8[uV] @ hw(\"o/n\") with safe = 0
machine m:
    var v : i8[mV] = 3 mV
    initial RUN
    state RUN:
        loop:
            n = v.to(uV)
",
    );
    assert!(e.contains("SC-38") && e.contains("i8"), "{e}");
}

#[test]
fn a_fractional_factor_needs_to_float() {
    let e = errors(
        "output n : int[mV] @ hw(\"o/n\") with safe = 0
machine m:
    var v : int[uV] = 5 uV
    initial RUN
    state RUN:
        loop:
            n = v.to(mV)
",
    );
    assert!(e.contains("SC-38") && e.contains("to_float"), "{e}");
}

#[test]
fn to_float_is_exact() {
    let p = ok("output f : float[V] @ hw(\"o/f\") with safe = 0
machine m:
    var v : int[mV] = 3300 mV
    initial RUN
    state RUN:
        loop:
            f = v.to_float(V)
");
    let t = trace(&p, 2);
    assert!(t.contains("out f 3.3"), "{t}");
}

#[test]
fn sums_need_equal_units_and_products_combine_them() {
    let e = errors(
        "output n : int[mV] @ hw(\"o/n\") with safe = 0
machine m:
    var v : int[mV] = 5 mV
    var w : int[uV] = 5 uV
    initial RUN
    state RUN:
        loop:
            n = v + w
",
    );
    assert!(e.contains("passen nicht") && e.contains(".to(U)"), "{e}");
    ok("unit inc = 1
output s : i32[inc/s] @ hw(\"o/s\") with safe = 0
machine m:
    var d : i32[inc] = 250 inc
    var r : i32[1/s] = 4 1/s
    initial RUN
    state RUN:
        loop:
            s = d * r
");
    let e = errors(
        "unit inc = 1
output s : i32[inc] @ hw(\"o/s\") with safe = 0
machine m:
    var d : i32[inc] = 250 inc
    var r : i32[1/s] = 4 1/s
    initial RUN
    state RUN:
        loop:
            s = d * r
",
    );
    assert!(e.contains("inc/s"), "{e}");
}

#[test]
fn a_unitless_literal_in_a_unit_context_is_an_error() {
    // 3.6: die Range ist die einzige Ausnahme.
    let e = errors(
        "output n : int[mV] @ hw(\"o/n\") with safe = 0
machine m:
    var v : int[mV] = 3300
    initial RUN
    state RUN:
        loop:
            n = v
",
    );
    assert!(e.contains("3300 mV"), "{e}");
    ok("output n : int[mV] in 0..5000 mV @ hw(\"o/n\") with safe = 0
machine m:
    var v : int[mV] in 0..5000 mV = 0
    initial RUN
    state RUN:
        loop:
            n = v
");
}

#[test]
fn as_changes_the_width_and_keeps_the_unit() {
    ok("output n : u16[mV] @ hw(\"o/n\") with safe = 0
machine m:
    var v : int[mV] = 3300 mV
    initial RUN
    state RUN:
        loop:
            n = v as u16
");
    let e = errors(
        "output n : u16 @ hw(\"o/n\") with safe = 0
machine m:
    var v : int[mV] = 3300 mV
    initial RUN
    state RUN:
        loop:
            n = v as u16
",
    );
    assert!(e.contains("u16[mV]"), "{e}");
}

#[test]
fn dividing_by_a_unit_literal_yields_the_raw_value() {
    // 3.2: die Division durch ein Einheitenliteral ist das Idiom der Entdimensionierung.
    let p = ok("output raw : u32 @ hw(\"o/raw\") with safe = 0
machine m:
    var v : int[mV] = 3300 mV
    initial RUN
    state RUN:
        loop:
            raw = (v / (1 mV)) as u32
");
    let t = trace(&p, 2);
    assert!(t.contains("out raw 3300"), "{t}");
}

#[test]
fn the_remainder_needs_equal_units() {
    let e = errors(
        "output n : int[mV] @ hw(\"o/n\") with safe = 0
machine m:
    var v : int[mV] = 3300 mV
    var w : int[uV] = 7 uV
    initial RUN
    state RUN:
        loop:
            n = v % w
",
    );
    assert!(e.contains("gleiche Einheiten"), "{e}");
}
