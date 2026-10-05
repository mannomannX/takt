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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

/// Ein Programm, das `bump` mit `arg` als `inout`-Argument ruft.
fn bumping(arg: &str) -> Result<Program, Vec<String>> {
    compile(&format!(
        "fn bump(inout x: int in 0..99):
    x = min(x + 1, 99)

record Holder:
    v : int in 0..99

input  n_in  : int in 0..99 @ hw(\"i/n\")
output n_sim : int @ sim(\"i/n\")
param  P     : int in 0..99 = 1
const  K : int in 0..99 = 3
output width : int in 0..99 @ sim(\"w\")

machine m:
    var h   : Holder = default
    var arr : [2] int in 0..99 = [0, 0]
    initial RUN
    state RUN:
        loop:
            bump({arg})
            width = h.v + arr[1]
"
    ))
}

/// 3.9: Das `inout`-Argument ist eine Stelle. Ein Input ist nicht
/// beschreibbar (Pruefung 7), Parameter, Konstante und `default` sind kein
/// Zuweisungsziel; ein Recordfeld und ein Array-Element sind Stellen.
#[test]
fn only_a_place_takes_an_inout_argument() {
    for (arg, want) in [("n_in", "SC-7"), ("P", "SC-3"), ("K", "SC-3"), ("default", "SC-3")] {
        let e = bumping(arg).expect_err(arg);
        assert_eq!(e.len(), 1, "{arg}: {e:?}");
        let message = if want == "SC-7" { "Input `n_in` ist nicht beschreibbar" } else { "kein Zuweisungsziel" };
        assert!(e[0].contains(want) && e[0].contains(message), "{arg}: {e:?}");
    }
    for arg in ["h.v", "arr[1]"] {
        let p = bumping(arg).unwrap_or_else(|e| panic!("{arg}: {e:?}"));
        let t =
            run(&p, &Trace::default(), &RunOptions { ticks: 2, ..Default::default() }).expect("Lauf").trace.render();
        assert!(t.contains("t=0 out width 1") && t.contains("t=2 out width 3"), "{arg}:\n{t}");
    }
}

/// 3.9: Im Ausdruck ist der Aufruf eine gewoehnliche Rueckgabe — `y`
/// bekommt den gefuellten Puffer, `b` bleibt, wie es war.
#[test]
fn a_call_in_an_expression_returns_without_writing_back() {
    let p = compile(&format!(
        "{FILL}
output width : int in 0..16 @ sim(\"w\")
output kept  : int in 0..16 @ sim(\"k\")

machine m:
    var b : bytes<16> = default
    var y : bytes<16> = default
    initial RUN
    state RUN:
        loop:
            var src : bytes<8> = default
            var a : bool = src.push(0x41)
            y = fill(src, b)
            width = y.len
            kept = b.len
"
    ))
    .expect("uebersetzt");
    let t = run(&p, &Trace::default(), &RunOptions { ticks: 1, ..Default::default() }).expect("Lauf").trace.render();
    assert!(t.contains("t=0 out width 2") && t.contains("t=0 out kept 0"), "{t}");
}
