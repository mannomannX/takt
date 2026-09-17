//! Ein negatives Literal ist ein Wert (3.1, 3.2, FB-184): Breite und Range
//! gelten fuer ihn, nicht fuer seinen Betrag.

use takt_diag::Policy;
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:
    language = 1
    tick = 1 ms

unit inc = 1
";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

#[test]
fn a_negative_default_lies_in_a_negative_range() {
    compile(
        "param LO : float[inc] in -100..0 inc = -20 inc
param E  : float in -100..0 = -20
param T  : float[degC] in -60..200 degC = -60 degC
param N  : i16 = -32768
",
    )
    .expect("alle vier Literale liegen in ihrer Range");
}

#[test]
fn the_magnitude_is_not_what_is_checked() {
    let e = compile("param HI : float[inc] in 0..100 inc = -20 inc\n").expect_err("-20 liegt nicht in 0..100");
    assert!(e.join("\n").contains("Range"), "{e:?}");
    let e = compile("param U : u8 = -1\n").expect_err("-1 passt nicht in u8");
    assert!(e.join("\n").contains("passt nicht"), "{e:?}");
}
