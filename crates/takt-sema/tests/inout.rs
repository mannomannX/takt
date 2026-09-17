//! `inout` (3.9): der Aufruf als Anweisung schreibt zurueck, `-> T` ist
//! daneben nicht erlaubt, und das Argument muss eine Stelle sein.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:
    language = 1
    tick = 1 ms

";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

const FILL: &str = "fn fill(src: bytes<8>, inout dst: bytes<16>):
    dst.clear()
    for i in range(8):
        if i >= src.len:
            break
        var put : bool = dst.push(src[i])
    var mark : bool = dst.push(0)
";

#[test]
fn the_call_statement_writes_the_parameter_back() {
    let p = compile(&format!(
        "{FILL}
output width : int in 0..16 @ sim(\"w\")

machine m:
    var enc : bytes<16> = default
    initial RUN
    state RUN:
        loop:
            var src : bytes<8> = default
            var a : bool = src.push(0x41)
            fill(src, enc)
            width = enc.len
"
    ))
    .expect("uebersetzt");
    let out = run(&p, &Trace::default(), &RunOptions { ticks: 1, ..Default::default() }).expect("Lauf");
    assert!(out.trace.render().contains("out width 2"), "{}", out.trace.render());
}

#[test]
fn a_return_type_next_to_inout_is_rejected() {
    let e = compile(
        "fn fill(src: bytes<8>, inout dst: bytes<16>) -> bool:
    dst.clear()
    return true
",
    )
    .expect_err("inout und -> T");
    assert!(e.join("\n").contains("schliessen sich aus"), "{e:?}");
}

#[test]
fn the_inout_argument_must_be_a_place() {
    let e = compile(&format!(
        "{FILL}
output width : int in 0..16 @ sim(\"w\")

machine m:
    initial RUN
    state RUN:
        loop:
            var src : bytes<8> = default
            fill(src, default)
            width = 0
"
    ))
    .expect_err("kein Ziel");
    assert!(!e.is_empty(), "{e:?}");
}
