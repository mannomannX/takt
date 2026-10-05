//! `property`/`assumption` (13.3): Monitore ueber Tick-Rand-Snapshots mit
//! drei Ausgaengen, FAIL-Befund und Trace-Zeile bei Verletzung, Pruefung 56.

use takt_diag::Policy;
use takt_interp::{Outcome, RunOptions, Trace, Verdict, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

fn run_ticks(body: &str, stimulus: &str, ticks: u64) -> takt_interp::RunResult {
    let p = compile(body).expect("uebersetzt");
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(&p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf")
}

/// Ein Ventil, das auf `go` oeffnet und nach 30 ms wieder schliesst.
const VALVE: &str = "
command go
output valve : bool @ hw(\"o/valve\") with safe = false
output armed : bool @ hw(\"o/armed\") with safe = false

machine v:
    initial CLOSED
    state CLOSED:
        enter:
            valve = false
        when go: -> OPEN
    state OPEN:
        enter:
            valve = true
            armed = true
        after 30 ms: -> CLOSED
";

#[test]
fn a_satisfied_property_and_an_open_one_have_their_outcomes() {
    let r = run_ticks(
        &format!(
            "{VALVE}
property opens: always(go implies eventually[20 ms](valve))
property told_again: always(valve implies eventually[100 ms](go))
"
        ),
        "t=2 cmd go\n",
        8,
    );
    let outcomes: Vec<(&str, &Outcome)> = r.properties.iter().map(|p| (p.name.as_str(), &p.outcome)).collect();
    assert_eq!(outcomes, vec![("opens", &Outcome::Satisfied), ("told_again", &Outcome::Open { since: 3 })]);
    assert_eq!(r.verdict, Verdict::Inconclusive);
    assert!(!r.trace.render().contains("property"), "{}", r.trace.render());
}

#[test]
fn a_violation_is_a_fail_finding_at_the_tick_it_is_decided() {
    let r = run_ticks(
        &format!(
            "{VALVE}
property never_armed: never(armed)
assumption quick: always(go implies eventually[10 ms](not valve))
"
        ),
        "t=2 cmd go\n",
        10,
    );
    let t = r.trace.render();
    assert!(t.contains("t=2 property never_armed violated 2\n"), "{t}");
    assert!(t.contains("t=3 assumption quick violated 2\n"), "{t}");
    assert_eq!(r.verdict, Verdict::Fail);
    assert_eq!(r.properties[0].outcome, Outcome::Violated { at: 2, detected: 2 });
    assert_eq!(r.properties[1].outcome, Outcome::Violated { at: 2, detected: 3 });
}

#[test]
fn stable_and_once_look_along_the_window() {
    let r = run_ticks(
        &format!(
            "{VALVE}
property holds: always(valve implies stable[20 ms](armed))
property was_told: always(valve implies once[20 ms](go))
"
        ),
        "t=2 cmd go\n",
        8,
    );
    assert!(r.properties.iter().all(|p| p.outcome == Outcome::Satisfied), "{:?}", r.properties);
}

#[test]
fn check_56_rejects_unbounded_operators_under_bounded_ones_and_odd_windows() {
    for (prop, want) in [
        ("always(go implies eventually[20 ms](always(valve)))", "nicht unter einem beschraenkten"),
        ("always(eventually[15 ms](valve))", "Vielfaches des Ticks"),
        ("always(eventually[0 ms](valve))", "positives Vielfaches"),
    ] {
        let e = compile(&format!("{VALVE}\nproperty p: {prop}\n")).expect_err(prop).join("\n");
        assert!(e.contains("SC-56") && e.contains(want), "{prop}:\n{e}");
    }
    let e = compile(&format!("{VALVE}\nproperty p: always(valve)\nproperty p: always(armed)\n")).expect_err("doppelt");
    assert!(e.join("\n").contains("SC-2"), "{e:?}");
    let e =
        compile(&format!("{VALVE}\nproperty p: eventually[20 ms](valve) with monitor = true\n")).expect_err("Monitor");
    assert!(e.join("\n").contains("Laufzeitmonitor"), "{e:?}");
}

/// Ein Ventil, das `go` erst nach 20 ms fuer einen Tick oeffnet: `go` in
/// Tick 2, `valve` und `armed` nur in Tick 4.
const PULSE: &str = "
command go
output valve : bool @ hw(\"o/valve\") with safe = false
output armed : bool @ hw(\"o/armed\") with safe = false

machine v:
    initial CLOSED
    state CLOSED:
        enter:
            valve = false
            armed = false
        when go: -> WAIT
    state WAIT:
        after 20 ms: -> OPEN
    state OPEN:
        enter:
            valve = true
            armed = true
        after 10 ms: -> CLOSED
";

/// 13.3 an der Fenstergrenze, je Operator ein Paar: Das Fenster, das den
/// Tick gerade noch fasst, ist erfuellt, das um einen Tick kuerzere
/// verletzt — Position und Entscheidungstick nach der Zukunftstiefe. Dazu
/// ein `armed`, das nach einem Tick wieder faellt.
#[test]
fn each_operator_decides_exactly_at_its_window_boundary() {
    let r = run_ticks(
        &format!(
            "{PULSE}
property e_in: always(go implies eventually[20 ms](valve))
property e_out: always(go implies eventually[10 ms](valve))
property s_in: always(go implies stable[10 ms](not valve))
property s_out: always(go implies stable[20 ms](not valve))
property o_in: always(valve implies once[20 ms](go))
property o_out: always(valve implies once[10 ms](go))
property falls: always(valve implies stable[10 ms](armed))
"
        ),
        "t=2 cmd go\n",
        10,
    );
    let outcomes: Vec<(&str, &Outcome)> = r.properties.iter().map(|p| (p.name.as_str(), &p.outcome)).collect();
    assert_eq!(
        outcomes,
        vec![
            ("e_in", &Outcome::Satisfied),
            ("e_out", &Outcome::Violated { at: 2, detected: 3 }),
            ("s_in", &Outcome::Satisfied),
            ("s_out", &Outcome::Violated { at: 2, detected: 4 }),
            ("o_in", &Outcome::Satisfied),
            ("o_out", &Outcome::Violated { at: 4, detected: 4 }),
            ("falls", &Outcome::Violated { at: 4, detected: 5 }),
        ]
    );
}

/// 13.3: Ein Atom, das sich nicht auswerten laesst, zaehlt als falsch — ein
/// ungueltiger Input ebenso wie eine Division durch null —, und die
/// Eigenschaft loest dabei keinen Fault aus.
#[test]
fn an_atom_that_cannot_be_evaluated_counts_as_false() {
    let r = run_ticks(
        "
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\") with safe = 1 bar
output n     : int in 0..9 @ hw(\"o/n\") with safe = 1
command zero

machine m:
    initial RUN
    state RUN:
        loop:
            n = 0 if zero else 1

property valid_p: always(p < 50 bar)
property divides: always(10 / n > 1)
",
        "t=0 in p 10 bar\nt=3 in p bad reason=Driver\nt=5 cmd zero\n",
        8,
    );
    let outcomes: Vec<(&str, &Outcome)> = r.properties.iter().map(|p| (p.name.as_str(), &p.outcome)).collect();
    assert_eq!(
        outcomes,
        vec![
            ("valid_p", &Outcome::Violated { at: 3, detected: 3 }),
            ("divides", &Outcome::Violated { at: 5, detected: 5 })
        ]
    );
    let t = r.trace.render();
    assert!(!t.contains(" fault "), "eine Eigenschaft faultet nicht:\n{t}");
}

/// Pruefung 56: `never` ist so unbeschraenkt wie `always` und steht nicht
/// unter `eventually[d]`; ein Laufzeitmonitor mit `always` aussen und nur
/// beschraenkten Operatoren innen uebersetzt und ist erfuellt.
#[test]
fn never_under_a_bounded_operator_is_refused_and_a_monitor_is_accepted() {
    let e = compile(&format!("{PULSE}\nproperty p: always(go implies eventually[20 ms](never(valve)))\n"))
        .expect_err("never unter eventually")
        .join("\n");
    assert!(e.contains("SC-56") && e.contains("nicht unter einem beschraenkten"), "{e}");
    let r = run_ticks(
        &format!("{PULSE}\nproperty opens: always(go implies eventually[20 ms](valve)) with monitor = true\n"),
        "t=2 cmd go\n",
        10,
    );
    assert_eq!(r.properties[0].outcome, Outcome::Satisfied);
}

/// 13.3, `tprop_atom := "(" tprop ")"`: Eine Klammer um eine Teilformel mit
/// Temporaloperator ist eine Formel, kein Ausdruck (KON2-017).
#[test]
fn a_parenthesized_temporal_formula_is_a_formula() {
    for prop in [
        "always(valve implies (armed or once[20 ms](go)))",
        "always((once[20 ms](go)) implies eventually[50 ms](valve))",
    ] {
        let body = format!("{VALVE}\nproperty p: {prop}\n");
        assert!(compile(&body).is_ok(), "{prop}: {:?}", compile(&body).err());
    }
    let body = format!("{VALVE}\nproperty p: always(valve implies (armed or once[20 ms](go)))\n");
    let r = run_ticks(&body, "t=2 cmd go\n", 8);
    assert_eq!(r.properties[0].outcome, Outcome::Satisfied, "{}", r.trace.render());
}
