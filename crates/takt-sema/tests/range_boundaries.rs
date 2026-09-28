//! Ranges an den Bindestellen (3.4, FB-343): Ein Wert, der an einen
//! Parameter, ein Feld, eine Rueckgabe oder ein Sammlungselement mit Range
//! geht, wird geprueft wie eine Zuweisung. Die Analyse des Rumpfs und jeder Leser
//! verlassen sich darauf, und `range_checked` prueft einen Wert nicht mehr,
//! der den Typ schon traegt.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\noutput n : int @ hw(\"o/n\") with safe = 0\n\n";

fn compile(body: &str) -> Result<Program, String> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors.join("\n")) }
}

/// Der Trace der ersten Ticks.
fn traced(body: &str) -> String {
    let p = compile(body).unwrap_or_else(|e| panic!("unerwartete Fehler:\n{e}"));
    run(&p, &Trace::default(), &RunOptions { ticks: 2, ..Default::default() }).expect("Lauf").trace.render()
}

fn faults_at_once(body: &str) {
    let text = traced(body);
    assert!(text.contains("t=0 fault m RangeFault \"50 ausserhalb der Range\""), "{text}");
}

#[test]
fn a_return_outside_its_range_faults() {
    faults_at_once(
        "\
fn relay(v: int) -> int in 0..10:
    return v

machine m:
    var big : int = 50
    initial RUN
    state RUN:
        loop:
            n = relay(big)
",
    );
}

#[test]
fn an_argument_outside_its_parameter_range_faults() {
    faults_at_once(
        "\
fn twice(v: int in 0..10) -> int:
    return v * 2

machine m:
    var big : int = 50
    initial RUN
    state RUN:
        loop:
            n = twice(big)
",
    );
}

#[test]
fn a_field_outside_its_range_faults() {
    faults_at_once(
        "\
record Level:
    v : int in 0..10

machine m:
    var big : int = 50
    initial RUN
    state RUN:
        loop:
            var l : Level = Level(v = big)
            n = l.v
",
    );
}

#[test]
fn a_pushed_element_outside_its_range_faults() {
    faults_at_once(
        "\
machine m:
    var big : int = 50
    var levels : vec<int in 0..10, 4> = default
    initial RUN
    state RUN:
        loop:
            levels.clear()
            var put : bool = levels.push(big)
            n = levels.len
",
    );
}

#[test]
fn an_inout_argument_must_carry_the_parameter_range() {
    // 3.9: Das Argument ist eine Stelle; eine Pruefung davor liesse keine
    // Stelle uebrig, in die der Aufruf zurueckschreibt.
    let e = compile(
        "\
fn bump(inout v: int in 0..10):
    v = v

machine m:
    var wide : int = 5
    initial RUN
    state RUN:
        loop:
            bump(wide)
            n = wide
",
    )
    .expect_err("ein `inout`-Argument ohne die Range des Parameters");
    assert!(e.contains("SC-47") && e.contains("inout"), "{e}");
}
