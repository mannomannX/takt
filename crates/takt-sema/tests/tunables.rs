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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

/// 12.5: Der Kopf einer Aufzeichnung nennt den Parametervektor zu Beginn
/// (`start_params`), nicht den am Ende, und ein Replay aus Kopf und
/// `tune`-Zeilen ergibt denselben Lauf. Der Startwert 3 kommt aus einer
/// Ueberlagerung, damit der Kopf ihn wirklich tragen muss.
#[test]
fn the_header_carries_the_start_values_and_replays() {
    let p = compile(PROGRAM).expect("uebersetzt");
    let tunes = "t=3 tune GAIN 5\nt=8 tune GAIN 7\n";
    let options = RunOptions { ticks: 9, overrides: vec![("GAIN".into(), "3".into())], ..Default::default() };
    let r = run(&p, &Trace::parse(tunes).expect("Stimulus"), &options).expect("Lauf");
    assert_eq!(r.start_params, vec![("GAIN".to_string(), "3".to_string())]);
    assert_eq!(r.params, vec![("GAIN".to_string(), "7".to_string())]);

    let header = takt_interp::record::Header::of(&p, None, &r.start_params, 9);
    let text = takt_interp::record::Recording { header, inputs: Trace::parse(tunes).expect("Stimulus") }.render();
    assert!(text.contains("#! param GAIN 3\n") && !text.contains("#! param GAIN 7"), "{text}");

    let rec = takt_interp::record::Recording::parse(&text).expect("lesbar");
    let again = run(
        &p,
        &rec.inputs,
        &RunOptions {
            ticks: rec.header.ticks,
            profile: rec.header.profile.clone(),
            overrides: rec.header.overrides(),
            ..Default::default()
        },
    )
    .expect("Replay");
    let want = r.trace.render();
    assert!(want.contains("t=0 out y 3"), "der Startwert wirkt:\n{want}");
    assert_eq!(again.trace.render(), want);
}

/// Ein Programm mit drei Tunables: Einheit, Ganzzahl, Dauer in `after`.
const THREE: &str = "\
system:
    language = 1
    tick     = 10 ms

tunable param KP    : float[pct/bar] in 0..10 pct/bar = 0.5 pct/bar
tunable param LEVEL : int in 0..100 = 2
tunable param HOLD  : Duration in 20 ms..1 s = 500 ms

output y : float[pct/bar] @ hw(\"o/y\") with safe = 0 pct/bar
output z : int in 0..100  @ hw(\"o/z\") with safe = 0
output s : bool           @ hw(\"o/s\") with safe = false

machine m:
    initial WAIT
    state WAIT:
        loop:
            y = KP
            z = LEVEL
        after HOLD: -> DONE
    state DONE:
        enter:
            s = true
        loop:
            y = KP
            z = LEVEL
";

/// 8.4: Die Grenzen der Range sind gueltig, knapp darueber wird verworfen,
/// und ein Tunable in `after` wird jeden Tick neu gelesen: `HOLD = 50 ms`
/// ab Tick 4 laesst den seit Tick 0 laufenden Zustand in Tick 5 enden.
#[test]
fn range_bounds_hold_and_after_rereads_its_tunable() {
    let p = compile(THREE).expect("uebersetzt");
    let t = trace(
        &p,
        "t=2 tune KP 10 pct/bar\nt=2 tune LEVEL 101\nt=3 tune LEVEL 100\nt=3 tune KP 0 pct/bar\nt=4 tune HOLD 50 ms\n",
        8,
    )
    .trace
    .render();
    for line in [
        "t=2 out y 10.0 pct/bar",
        "t=2 tune LEVEL 101 rejected",
        "t=3 out z 100",
        "t=3 out y 0.0 pct/bar",
        "t=4 tune HOLD 50 ms",
        "t=5 state m DONE",
    ] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}

/// 8.4: Ein `tune` wird gegen Range *und Einheit* validiert; `5 psi` an
/// einem Tunable in `pct/bar` ist zu verwerfen, nicht als `5 pct/bar` zu
/// uebernehmen.
#[test]
fn a_tune_value_in_a_foreign_unit_is_rejected() {
    let p = compile(THREE).expect("uebersetzt");
    let t = trace(&p, "t=1 tune KP 5 psi\n", 3).trace.render();
    assert!(t.contains("t=1 tune KP") && t.contains("rejected"), "{t}");
    assert!(!t.contains("out y 5.0"), "{t}");
}

/// 8.4: Ein Tunable ist keine Compile-Zeit-Konstante, auch nicht als
/// Zahl der Durchlaeufe von `repeat` (Pruefung 35).
#[test]
fn a_tunable_cannot_count_a_repeat() {
    let e = compile(&format!(
        "{HEAD}tunable param N : int in 1..64 = 8
output n : int in 0..64 @ hw(\"o/n\") with safe = 0
machine m:
    initial RUN
    state RUN:
        sequence:
            repeat N:
                n = 2
                wait 10 ms
            n = 1
"
    ))
    .expect_err("Fehler erwartet");
    assert!(e.join("\n").contains("SC-35"), "{e:?}");
}
