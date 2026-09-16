//! Tunables (8.4, Pruefung 35): ein Input mit Halte-Semantik. Die
//! `tune`-Zeile des Stimulus gilt ab ihrer Tick-Grenze, ausserhalb der
//! Range wird sie verworfen und so aufgezeichnet; der Lauf-Header behaelt
//! die Startwerte.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";
const PROGRAM: &str = include_str!("../../../corpus-try/41_tunables.takt");

fn compile(src: &str) -> Result<Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

fn trace(p: &Program, stimulus: &str, ticks: u64) -> takt_interp::RunResult {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf")
}

#[test]
fn a_tune_line_holds_from_its_tick_on() {
    let p = compile(PROGRAM).expect("uebersetzt");
    let r = trace(&p, "t=3 tune GAIN 5\nt=6 tune GAIN 200\nt=8 tune GAIN 7\n", 9);
    let t = r.trace.render();
    // k zaehlt ab Tick 0: y = 2, 4, 6; ab Tick 3 mit 5: 20, 25, 30; 200 verworfen; ab 8 mit 7: 63, 70.
    for line in [
        "t=1 out y 4",
        "t=3 tune GAIN 5",
        "t=3 out y 20",
        "t=6 tune GAIN 200 rejected",
        "t=6 out y 35",
        "t=8 tune GAIN 7",
        "t=8 out y 63",
        "t=9 out y 70",
    ] {
        assert!(t.contains(line), "{line} fehlt:\n{t}");
    }
    assert_eq!(r.params, vec![("GAIN".to_string(), "7".to_string())], "der zuletzt uebernommene Satz");
    // Die Aufzeichnung ist als Stimulus lesbar und ergibt denselben Lauf (12.5).
    let again = trace(&p, &t, 9).trace.render();
    assert_eq!(again, t);
}

#[test]
fn a_plain_param_cannot_be_tuned() {
    let p = compile(&format!("{HEAD}param GAIN : int in 0..100 = 2\noutput y : int in 0..100 @ hw(\"o/y\") with safe = 0\nmachine m:\n    initial RUN\n    state RUN:\n        loop:\n            y = GAIN\n")).expect("uebersetzt");
    let stimulus = Trace::parse("t=1 tune GAIN 5\n").expect("Stimulus");
    let e = run(&p, &stimulus, &RunOptions { ticks: 2, ..Default::default() }).expect_err("kein tunable");
    assert!(format!("{e:?}").contains("kein `tunable param`"), "{e:?}");
}

#[test]
fn a_tunable_is_no_compile_time_constant() {
    let e = compile(&format!("{HEAD}tunable param N : int in 1..64 = 8\noutput n : int in 0..64 @ hw(\"o/n\") with safe = 0\nmachine m:\n    var buf : bytes<N> = default\n    initial RUN\n    state RUN:\n        loop:\n            n = buf.len\n")).expect_err("Fehler erwartet");
    assert!(e.join("\n").contains("SC-35"), "{e:?}");
}
