//! Blockinstanzen (Referenz 5.7): benannt per `var f = …`, anonym je
//! Aufrufstelle per `if rose(start):`.
//!
//! Die anonyme Form entsteht ohne eigenen MIR-Knoten: eine versteckte
//! Instanz auf Maschinenebene und `tmp = inst.step(args)` vor der
//! umgebenden Anweisung. Was der Trace zeigt, muss darum dasselbe sein wie
//! bei der benannten Form.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Result<takt_mir::Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    match out.program {
        Some(p) if errors.is_empty() => Ok(p),
        _ => Err(errors),
    }
}

fn simulate(body: &str, stimulus: &str, ticks: u64) -> String {
    let p = compile(body).unwrap_or_else(|e| panic!("unerwartete Fehler:\n{}", e.join("\n")));
    let stim = Trace::parse(stimulus).expect("Stimulus lesbar");
    run(&p, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

/// Zaehlt steigende Flanken von `go`; `ANON` nutzt die anonyme, `NAMED`
/// die benannte Form.
const ANON: &str = "
command go
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var count : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            if rising(go):
                count = count + 1
            n = count
";

const NAMED: &str = "
command go
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var count : int in 0..99 = 0
    var edge = rising()
    initial RUN
    state RUN:
        loop:
            var rose : bool = edge.step(go)
            if rose:
                count = count + 1
            n = count
";

const STIM: &str = "t=2 cmd go
t=3 cmd go
t=7 cmd go
";

#[test]
fn an_anonymous_instance_detects_each_rising_edge_once() {
    // Drei Kommandos, davon zwei aufeinanderfolgend: zwei Flanken.
    let trace = simulate(ANON, STIM, 10);
    assert!(trace.contains("t=2 out n 1"), "erste Flanke fehlt:\n{trace}");
    assert!(!trace.contains("t=3 out n 2"), "eine Flanke wurde doppelt gezaehlt:\n{trace}");
    assert!(trace.contains("t=7 out n 2"), "zweite Flanke fehlt:\n{trace}");
}

#[test]
fn the_anonymous_and_the_named_form_produce_the_same_trace() {
    assert_eq!(simulate(ANON, STIM, 10), simulate(NAMED, STIM, 10));
}

#[test]
fn each_call_site_is_its_own_instance() {
    // Zwei Stellen, zwei Instanzen: Die zweite sieht die Flanke ebenso,
    // obwohl die erste sie schon gesteppt hat.
    let trace = simulate(
        "
command go
output a : int in 0..99 @ hw(\"o/a\") with safe = 0
output b : int in 0..99 @ hw(\"o/b\") with safe = 0

machine m:
    var x : int in 0..99 = 0
    var y : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            if rising(go):
                x = x + 1
            if rising(go):
                y = y + 1
            a = x
            b = y
",
        "t=2 cmd go\n",
        4,
    );
    assert!(trace.contains("t=2 out a 1") && trace.contains("t=2 out b 1"), "{trace}");
}

#[test]
fn a_call_in_the_condition_runs_before_the_body_not_inside_it() {
    // Der Aufruf in der Bedingung wird vor das `if` gehoben, nicht in den
    // Rumpf — sonst liefe er nur, wenn der Rumpf laeuft.
    let trace = simulate(
        "
command go
command inner
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var count : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            if go:
                if rising(inner):
                    count = count + 1
            n = count
",
        "t=1 cmd inner\nt=2 cmd inner\nt=2 cmd go\nt=3 cmd inner\nt=3 cmd go\n",
        5,
    );
    // Tick 1: `inner` steigt, aber `go` fehlt — die innere Instanz sieht
    // nichts. Tick 2: `go` und `inner`, die Instanz sieht ihre erste
    // Flanke. Tick 3: `inner` bleibt hoch, keine zweite Flanke.
    assert!(trace.contains("t=2 out n 1"), "{trace}");
    assert!(!trace.contains("out n 2"), "{trace}");
}

#[test]
fn a_falling_edge_is_the_mirror_image() {
    let trace = simulate(
        "
command go
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var count : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            if falling(go):
                count = count + 1
            n = count
",
        "t=2 cmd go\nt=3 cmd go\n",
        6,
    );
    assert!(trace.contains("t=4 out n 1"), "fallende Flanke fehlt:\n{trace}");
}

#[test]
fn integrate_accumulates_and_saturates() {
    let trace = simulate(
        "
output q : float[A*s] @ hw(\"o/q\") with safe = 0 A*s

machine m every 10 ms:
    var coulomb = integrate[A](limit = 5 A*s)
    var charge : float[A*s] = 0 A*s
    initial RUN
    state RUN:
        loop:
            charge = coulomb.step(2 A, 1 s)
            q = charge
",
        "",
        40,
    );
    assert!(trace.contains("t=0 out q 2.0 "), "erster Schritt:\n{trace}");
    assert!(trace.contains("out q 5.0 "), "Saettigung fehlt:\n{trace}");
    assert!(!trace.contains("out q 6"), "ueber dem Limit:\n{trace}");
}

#[test]
fn an_anonymous_instance_is_rejected_in_a_guard() {
    let err = compile(
        "
command go
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    initial A
    state A:
        when rising(go): -> B
    state B:
        loop:
            n = 1
",
    )
    .expect_err("Guard mit anonymer Instanz");
    assert!(err.iter().any(|e| e.contains("nur in einer Anweisung")), "{err:?}");
}

#[test]
fn an_anonymous_instance_is_rejected_in_a_for_loop() {
    // 5.7: hoechstens ein `step` je Tick, „kein Aufruf in `for`-Schleifen".
    let err = compile(
        "
command go
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                if rising(go):
                    n = 1
",
    )
    .expect_err("step in for");
    assert!(err.iter().any(|e| e.contains("for")), "{err:?}");
}

#[test]
fn a_block_with_constructor_parameters_keeps_the_named_form() {
    // `integrate[A](limit = …)` hat Konstruktorparameter: ein Aufruf mit
    // Argumenten ist die Instanz, kein anonymer Schritt.
    let err = compile(
        "
output q : float[A*s] @ hw(\"o/q\") with safe = 0 A*s

machine m:
    initial RUN
    state RUN:
        loop:
            q = integrate[A](limit = 5 A*s)
",
    )
    .expect_err("Instanz als Wert");
    assert!(!err.is_empty());
}

/// Ein Zaehler je Element; `step` liefert den neuen Stand.
const COUNTER: &str = "
block counter():
    var k : int in 0..99 = 0
    step() -> int:
        k = (k + 1) % 100
        return k
";

#[test]
fn every_element_of_an_instance_array_steps_once_per_tick() {
    // 5.7, FB-423: Das Flag eines Elements ging nie zurueck; der zweite
    // Tick brach mit „step zweimal in einer Aktivierung" ab.
    let trace = simulate(
        &format!(
            "{COUNTER}
output a : int in 0..99 @ sim(\"o/a\")
output b : int in 0..99 @ sim(\"o/b\")

machine m:
    var cs = [2] counter()
    initial RUN
    state RUN:
        loop:
            a = cs[0].step()
            b = cs[1].step()
"
        ),
        "",
        3,
    );
    assert!(trace.contains("t=2 out a 3"), "drei Schritte von cs[0]:\n{trace}");
    assert!(trace.contains("t=2 out b 3"), "drei Schritte von cs[1]:\n{trace}");
}

#[test]
fn an_instance_array_steps_in_a_for_loop_over_its_index() {
    // 5.7: kein `step` in `for`-Schleifen, „ausser ueber Arrays von
    // Instanzen"; die Ausnahme galt nur fuer eine Variable als Empfaenger,
    // nie fuer `cs[i]` (FB-423).
    let trace = simulate(
        &format!(
            "{COUNTER}
output a : int in 0..999 @ sim(\"o/a\")

machine m:
    var cs = [2] counter()
    var sum : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            for i in range(2):
                var x : int in 0..99 = cs[i].step()
                sum = (sum + x) % 1000
            a = sum
"
        ),
        "",
        3,
    );
    assert!(trace.contains("t=2 out a 12"), "1+1, 2+2, 3+3:\n{trace}");
}

#[test]
fn a_step_in_a_handler_is_rejected_like_one_in_a_loop() {
    // 8.7: Ein Handler laeuft je Element des Fensters, ein `step` darin
    // also mehrmals je Tick (FB-423).
    let err = compile(&format!(
        "{COUNTER}
stream<u8> q with capacity = 4
output v : int in 0..99 @ sim(\"o/v\")

machine producer:
    initial RUN
    state RUN:
        loop:
            send q, 1
            send q, 2

machine consumer:
    var c = counter()
    initial RUN
    on q as e:
        v = c.step()
    state RUN:
        loop:
            pass
"
    ))
    .expect_err("step im Handler");
    assert!(err.iter().any(|e| e.contains("SC-11") && e.contains("Handler")), "{err:?}");
}
