//! **Zeitoperatoren im Beweiser** (13.3, FB-374, M11 Schritt 27b):
//! `eventually`, `stable` und `once` sind Zustand im Modell — Zaehler in der
//! Vergangenheit und fuer die Antwort `a implies eventually[d](b)`, sonst
//! ein Ring je Atom wie im nativen Monitor. Jede Form ist an einem
//! Beispiel bewiesen und an einem anderen verletzt, und jedes Gegenbeispiel
//! hat der Interpreter bestaetigt. Die Tests brauchen einen Solver
//! (`takt_testkit::require`).

use takt_mir::Program;
use takt_prove::{Solver, Verdict, encode, find, prove};

fn compile(src: &str) -> Program {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn solver() -> Option<Solver> {
    let found = Some(find()).filter(|s| !matches!(s, Solver::Missing));
    takt_testkit::require("solver", found, "`TAKT_SOLVER` setzen oder z3/cvc5 installieren")
}

/// Ein Ventil: `go` oeffnet es fuer drei Ticks und schaerft `armed` fuer
/// immer; in Tick 0 laeuft nur der Eintritt.
const VALVE: &str = "system:
    language = 1
    tick     = 10 ms

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

/// Das Urteil ueber `property p: <formula>` am Ventil, bis Tiefe 8.
fn verdict(formula: &str) -> Option<Verdict> {
    let solver = solver()?;
    let p = compile(&format!("{VALVE}\nproperty p: {formula}\n"));
    let model = encode(&p).expect("kodierbar");
    assert!(model.notes.iter().all(|n| !n.contains("nicht kodiert")), "{:?}", model.notes);
    let reports = prove(&model, &p, 8, &solver, 60).expect("Solver laeuft");
    Some(reports.into_iter().next().expect("eine Eigenschaft").verdict)
}

fn proven(formula: &str) {
    let Some(v) = verdict(formula) else { return };
    assert!(matches!(v, Verdict::Proven { .. }), "`{formula}`: {v:?}");
}

fn violated(formula: &str) {
    let Some(v) = verdict(formula) else { return };
    assert!(matches!(v, Verdict::Violated { .. }), "`{formula}`: {v:?}");
}

/// **Vergangenheit**: Offen ist das Ventil hoechstens drei Ticks nach `go`.
#[test]
fn once_is_a_counter_since_the_last_time() {
    proven("always(valve implies once[30 ms](go))");
    violated("always(valve implies once[10 ms](go))");
}

/// **Antwort**: Nach dem Oeffnen schliesst es innerhalb von 50 ms, nicht
/// innerhalb von 20 ms.
#[test]
fn a_response_is_a_counter_of_the_oldest_open_obligation() {
    proven("always(valve implies eventually[50 ms](not valve))");
    violated("always(valve implies eventually[10 ms](not valve))");
}

/// **`a implies stable[d](b)`** als `b or not once[d](a)`: `armed` bleibt,
/// das Ventil nicht.
#[test]
fn stable_under_an_implication_is_a_past_formula() {
    proven("always(valve implies stable[50 ms](armed))");
    violated("always(valve implies stable[20 ms](valve))");
}

/// **Ring**: Was keiner Form folgt, entscheidet die Position `k − F` im
/// Tick `k` ueber die Werte der Atome der letzten Ticks.
#[test]
fn a_general_formula_is_a_ring_of_its_atoms() {
    proven("always(stable[20 ms](armed) or not armed)");
    violated("always(eventually[20 ms](valve) implies once[50 ms](go))");
}

/// `never(φ)` ist `always(not φ)`, auch mit Zeitoperatoren.
#[test]
fn never_negates_its_formula() {
    proven("never(valve and not once[30 ms](go))");
    violated("never(valve and stable[20 ms](valve))");
}

/// Das Zeitgeber-Programm aus FB-375: `A` zaehlt je Tick, nach 500 ms
/// setzt `B` zurueck.
const TIMER: &str = "system:
    language = 1
    tick     = 10 ms

output o : int @ hw(\"o/o\") with safe = 0

machine m:
    var x : int in 0..1000 = 0
    initial A

    state A:
        loop:
            x = x + 1
            o = x
        after 500 ms: -> B

    state B:
        enter:
            x = 0
            o = 0
        after 10 ms: -> A

property small: always(o <= 60)
";

/// **FB-375**: Die k-Induktion mit der Vorgabe k = 5 sieht den Zeitablauf
/// nicht; die Invariantensuche (Horn-Klauseln, Spacer) beweist die
/// Eigenschaft fuer jede Tiefe.
#[test]
fn a_timer_program_is_proven_with_the_default_depth() {
    let Some(solver) = solver() else { return };
    let p = compile(TIMER);
    let model = encode(&p).expect("kodierbar");
    assert_eq!(model.horizon, 50);
    let reports = prove(&model, &p, 5, &solver, 60).expect("Solver laeuft");
    assert_eq!(reports[0].verdict, Verdict::Proven { k: 0, assumptions: vec![] }, "{:?}", reports[0]);
    assert_eq!(reports[0].verdict.text(), "bewiesen (induktive Invariante, Spacer)");
}

/// Ausserhalb des ganzzahligen Fragments sucht niemand eine Invariante;
/// der offene Schritt nennt dann die Tiefe der laengsten Frist.
#[test]
fn an_open_step_names_the_depth_of_the_longest_deadline() {
    let Some(solver) = solver() else { return };
    let src = TIMER
        .replace("var x : int in 0..1000 = 0", "var x : float in 0.0..1000.0 = 0.0")
        .replace("x = x + 1", "x = x + 1.0")
        .replace("output o : int", "output o : float")
        .replace("            o = 0\n", "            o = 0.0\n")
        .replace("always(o <= 60)", "always(o <= 60.0)");
    let p = compile(&src);
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 5, &solver, 60).expect("Solver laeuft");
    let Verdict::Unproven { reason } = &reports[0].verdict else { panic!("{:?}", reports[0]) };
    assert!(reason.contains("k = 51 (`--depth auto`)"), "{reason}");
}
