//! `property`/`assumption` (13.3): Monitore ueber Tick-Rand-Snapshots mit
//! drei Ausgaengen, FAIL-Befund und Trace-Zeile bei Verletzung, Pruefung 56.

use takt_diag::Policy;
use takt_interp::{Outcome, RunOptions, Trace, Verdict, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
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
}
