//! Einheiten auf Ganzzahlen (Referenz 3.2, v1.1; Pruefung 38): nominal wie
//! auf Fliesskomma, `.to(U)` nur bei ganzzahligem Faktor, sonst `.to_float(U)`.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

/// 3.2, 4.1 mit exakten Werten: Skalar mal `int[U]`, Division trunkiert
/// gegen null, `.to` negativer Werte; `i32::MIN` in `uV`, `+` und `*` ueber
/// die Breite laufen ueber (`Arithmetic(Overflow)`), Division durch null ist
/// `Arithmetic(DivZero)` — je in ihrer Runde.
#[test]
fn integer_units_compute_exact_values_and_fault_by_kind() {
    let p = ok("output a : int[mV] @ hw(\"o/a\") with safe = 0 mV
output b : int[mV] @ hw(\"o/b\") with safe = 0 mV
output c : int[mV] @ hw(\"o/c\") with safe = 0 mV
output d : int[uV] @ hw(\"o/d\") with safe = 0 uV
output e : i32[uV] @ hw(\"o/e\") with safe = 0 uV

machine m:
    fault -> SAFE
    var v : int[mV] = 5 mV
    var neg : int[mV] = -7 mV
    var lo : i32[mV] = -2147483648 mV
    var hi : i32[mV] = 2147483647 mV
    var z : int = 0
    var turn : int in 0..9 = 0
    initial RUN
    state RUN:
        loop:
            a = 3 * v
            b = neg / 2
            c = v / 2
            d = neg.to(uV)
            turn = turn + 1
            if turn == 2:
                e = lo.to(uV)
            if turn == 3:
                var h : i32[mV] = hi + 1 mV
            if turn == 4:
                var g : i32[mV] = hi * 2
            if turn == 5:
                c = v / z
    state SAFE:
        when true: -> RUN
");
    let t = trace(&p, 6);
    for line in ["t=0 out a 15 mV", "t=0 out b -3 mV", "t=0 out c 2 mV", "t=0 out d -7000 uV"] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
    let faults: Vec<&str> = t.lines().filter(|l| l.contains(" fault ")).collect();
    assert_eq!(
        faults,
        [
            "t=1 fault m Arithmetic(Overflow) \"Ueberlauf in i32\" -> SAFE",
            "t=2 fault m Arithmetic(Overflow) \"Ueberlauf in i32\" -> SAFE",
            "t=3 fault m Arithmetic(Overflow) \"Ueberlauf in i32\" -> SAFE",
            "t=4 fault m Arithmetic(DivZero) \"Division durch null\" -> SAFE",
        ],
        "{t}"
    );
}

/// 3.2: Subtraktion und Vergleich verlangen gleiche Einheiten wie die
/// Summe.
#[test]
fn differences_and_comparisons_need_equal_units() {
    for (line, want) in [("n = v - w", "passen nicht"), ("f = v < w", "`<` zwischen `int[mV]` und `int[uV]`")] {
        let e = errors(&format!(
            "output n : int[mV] @ hw(\"o/n\") with safe = 0 mV
output f : bool @ hw(\"o/f\") with safe = false
machine m:
    var v : int[mV] = 5 mV
    var w : int[uV] = 5 uV
    initial RUN
    state RUN:
        loop:
            {line}
"
        ));
        assert!(e.contains("SC-3") && e.contains(want), "{line}: {e}");
    }
}
