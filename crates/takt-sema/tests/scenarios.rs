//! Szenarien (Referenz 13.6, v1.1; Pruefung 26) und Coverage (13.2): ein
//! Szenario ist eine Maschine des Simulations-Builds, laeuft nur, wenn es
//! gewaehlt ist, und beendet den Lauf in seinem letzten Zustand.

use takt_diag::Policy;
use takt_interp::{CoverKind, Ended, RunOptions, Trace, Verdict, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> (Option<Program>, Vec<String>) {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let diags: Vec<String> = out.diagnostics.iter().map(|d| format!("{d}")).collect();
    let errors = out.diagnostics.iter().any(|d| d.is_error());
    (if errors { None } else { out.program }, diags)
}

fn ok(body: &str) -> Program {
    let (p, diags) = compile(body);
    p.unwrap_or_else(|| panic!("unerwartete Fehler:\n{}", diags.join("\n")))
}

const PROGRAM: &str = "
input  p     : float[bar] in 0..100 bar @ hw(\"daq/p\") with max_age = 10 ms
output valve : bool                     @ hw(\"o/valve\") with safe = false
output p_sim : float[bar]               @ sim(\"daq/p\") with safe = 1 bar

machine dut:
    initial CLOSED
    state CLOSED:
        loop:
            check p < 50 bar, \"overpressure\"
        when p > 5 bar: -> OPEN
    state OPEN:
        enter:
            valve = true
        when p < 2 bar: -> CLOSED

scenario \"pressure rises\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            p_sim = 1 bar
            wait 3 ms
            p_sim = 10 bar
            until dut.state == OPEN timeout 10 ms
            verify valve, \"valve should open\"
            verdict pass \"opened\"

scenario \"stays closed\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            p_sim = 1 bar
            wait 5 ms
            expect dut.state == CLOSED, \"must stay closed\"
            verdict pass \"closed\"
";

fn run_scenario(p: &Program, name: &str) -> takt_interp::RunResult {
    let options = RunOptions { ticks: 100, scenario: Some(name.to_string()), ..Default::default() };
    run(p, &Trace::default(), &options).expect("Lauf")
}

#[test]
fn a_chosen_scenario_runs_and_ends_the_run_in_its_last_state() {
    // 13.6: „takt test fuehrt jedes Szenario als eigenen Sim-Lauf aus".
    let p = ok(PROGRAM);
    let r = run_scenario(&p, "pressure rises");
    assert_eq!(r.verdict, Verdict::Pass, "{}", r.trace.render());
    assert_eq!(r.ended, Ended::Scenario);
    let t = r.trace.render();
    assert!(t.contains("state dut OPEN") && t.contains("end scenario"), "{t}");
    let r = run_scenario(&p, "stays closed");
    assert_eq!(r.verdict, Verdict::Pass, "{}", r.trace.render());
    assert!(!r.trace.render().contains("state dut OPEN"));
}

#[test]
fn without_a_choice_no_scenario_runs() {
    let p = ok(PROGRAM);
    let r = run(&p, &Trace::default(), &RunOptions { ticks: 20, ..Default::default() }).expect("Lauf");
    let t = r.trace.render();
    assert!(!t.contains("pressure rises") && !t.contains("stays closed"), "{t}");
    assert_eq!(r.ended, Ended::Ticks);
}

#[test]
fn an_unknown_scenario_is_an_error() {
    let p = ok(PROGRAM);
    let options = RunOptions { ticks: 10, scenario: Some("nope".into()), ..Default::default() };
    assert!(run(&p, &Trace::default(), &options).is_err());
}

#[test]
fn coverage_counts_states_transitions_and_checks() {
    // 13.2: Zustands-, Transitions- und Check-Coverage je Sim-Lauf.
    let p = ok(PROGRAM);
    let c = run_scenario(&p, "pressure rises").coverage;
    let has = |kind: CoverKind, machine: &str, name: &str| {
        c.hits.iter().any(|((k, m, n), _)| *k == kind && m == machine && n.starts_with(name))
    };
    assert!(has(CoverKind::State, "dut", "CLOSED") && has(CoverKind::State, "dut", "OPEN"), "{c:?}");
    assert!(has(CoverKind::Transition, "dut", "CLOSED->OPEN"), "{c:?}");
    assert!(has(CoverKind::Check, "dut", "check"), "{c:?}");
    assert!(!has(CoverKind::CheckFailed, "dut", "check"), "{c:?}");
    let u = takt_interp::coverage::universe(&p);
    assert_eq!(u.checks, 2, "ein `check` und ein `expect`");
    let text = c.render();
    assert!(text.starts_with("# takt-coverage 1\n"), "{text}");
}

#[test]
fn a_scenario_writes_only_sim_outputs() {
    // Pruefung 26 (13.6).
    let (p, diags) = compile(&PROGRAM.replace(
        "            p_sim = 1 bar\n            wait 3 ms",
        "            valve = true\n            wait 3 ms",
    ));
    assert!(p.is_none());
    assert!(diags.iter().any(|d| d.contains("SC-26") && d.contains("valve")), "{diags:?}");
}

#[test]
fn a_scenario_and_a_model_do_not_share_a_sim_output() {
    // 13.6: „Single-Writer gegenueber Modellmaschinen"; zwei Szenarien
    // duerfen denselben Output stellen, weil sie nie zusammen laufen.
    let (p, diags) = compile(&format!(
        "{PROGRAM}
machine model:
    initial RUN
    state RUN:
        loop:
            p_sim = 3 bar
"
    ));
    assert!(p.is_none());
    assert!(diags.iter().any(|d| d.contains("SC-26") && d.contains("p_sim")), "{diags:?}");
    ok(PROGRAM);
}

#[test]
fn an_irreversible_output_is_covered_by_a_scenario() {
    // 12.7: „mindestens ein Szenario deckt die Zuweisung ab".
    let p = ok("output fire : bool @ hw(\"o/fire\") with safe = false, irreversible = true
output go   : bool @ sim(\"x/go\") with safe = false
input  armed : bool @ hw(\"x/go\") with max_age = 10 ms

machine dut:
    initial WAIT
    state WAIT:
        when armed: -> FIRE
    state FIRE:
        sequence:
            expect armed, \"armed\"
            fire = true

scenario \"fires\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            go = true
            until dut.state == FIRE timeout 10 ms
            wait 2 ms
            verdict pass \"fired\"
");
    let c = run_scenario(&p, "fires").coverage;
    assert!(c.hits.keys().any(|(k, _, n)| *k == CoverKind::Irreversible && n == "fire"), "{c:?}");
}
