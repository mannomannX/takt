//! `takt prove` mit Solver (plan/m6.md 2.8): bewiesen, verletzt mit im
//! Interpreter bestaetigtem Gegenbeispiel, unbewiesen mit Grund. Die Tests
//! ueberspringen sich ohne Solver (`TAKT_SOLVER`, `z3`, `cvc5`).

use takt_mir::Program;
use takt_prove::{CheckVerdict, ContractVerdict, Solver, Verdict, classify, encode, find, prove, verify_contracts};

fn compile(src: &str) -> Program {
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn corpus_with(name: &str, properties: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    compile(&format!("{src}\n{properties}\n"))
}

fn solver() -> Option<Solver> {
    match find() {
        Solver::Missing => {
            eprintln!("uebersprungen: kein Solver (TAKT_SOLVER setzen oder z3/cvc5 installieren)");
            None
        }
        s => Some(s),
    }
}

#[test]
fn an_inductive_invariant_is_proven() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "property vent_on_safe: always(tank_guard.state == SAFE implies vent)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].verdict, Verdict::Proven { k: 2 }, "{:?}", reports[0]);
}

#[test]
fn a_violation_yields_a_counterexample_the_interpreter_confirms() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "property never_vents: never(vent)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 4, &solver, 60).expect("Solver laeuft");
    let Verdict::Violated { at, stimulus } = &reports[0].verdict else { panic!("{:?}", reports[0]) };
    // `start`, dann ein Tankdruck ueber LIMIT: der Fault-Pfad oeffnet das Ventil.
    assert!(*at <= 4, "{at}");
    assert!(stimulus.contains("cmd start") && stimulus.contains("in tank_p "), "{stimulus}");
}

/// B3: `check tank_p < LIMIT` in 01 ist erreichbar — der Pfad ist ein
/// Stimulus, den der Interpreter bestaetigt.
#[test]
fn a_reachable_check_gets_its_path() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "");
    let model = encode(&p).expect("kodierbar");
    assert_eq!(model.checks.len(), 1);
    let reports = classify(&model, &p, 3, &solver, 60).expect("Solver laeuft");
    let CheckVerdict::Reachable { at, stimulus } = &reports[0].verdict else { panic!("{:?}", reports[0]) };
    assert!(*at <= 3, "{at}");
    assert!(stimulus.contains("cmd start"), "{stimulus}");
    assert_eq!((reports[0].kind.as_str(), reports[0].machine.as_str()), ("check", "tank_guard"));
}

/// B3: eine Pruefung, die der Typ schon garantiert, ist bewiesen
/// unerreichbar — die Ranges des Zustands sind Invarianten der Kodierung.
#[test]
fn a_check_the_types_guarantee_is_proven_unreachable() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:
    language = 1
    tick     = 1 ms

output r : int in 0..200 @ hw(\"o/r\") with safe = 0

machine f:
    var n : int in 0..200 = 0
    initial RUN
    state RUN:
        loop:
            n = (n + 1) % 200
            r = n
            check n <= 200, \"never\"
",
    );
    let model = encode(&p).expect("kodierbar");
    let reports = classify(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    assert_eq!(reports[0].verdict, CheckVerdict::Unreachable { k: 2 }, "{:?}", reports[0]);
}

/// B2: das `ensures` des Begrenzers haelt aus jedem typkonformen Zustand;
/// sein `requires` kann an der Aufrufstelle verletzt sein — der Pfad kommt
/// aus dem Modell, weil der Interpreter Vertraege nicht prueft.
#[test]
fn block_contracts_are_proven_and_their_call_sites_classified() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("48_contracts.takt", "");
    let model = encode(&p).expect("kodierbar");
    let contracts = verify_contracts(&model, &solver, 60).expect("Solver laeuft");
    assert_eq!(contracts.len(), 1);
    assert_eq!(contracts[0].verdict, ContractVerdict::Proven, "{:?}", contracts[0]);
    let sites = classify(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "requires").expect("requires-Stelle");
    let CheckVerdict::Reachable { at, stimulus } = &site.verdict else { panic!("{site:?}") };
    assert!(*at <= 2 && stimulus.contains("in level -"), "{at} {stimulus}");
    // Ein Vertrag, der nicht haelt: das Ergebnis ist nicht immer kleiner als `x`.
    let p = corpus_with("48_contracts.takt", "").clone();
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/48_contracts.takt"))
        .expect("Korpus")
        .replace("ensures result <= hi", "ensures result < x");
    let bad = compile(&src);
    let model = encode(&bad).expect("kodierbar");
    let contracts = verify_contracts(&model, &solver, 60).expect("Solver laeuft");
    let ContractVerdict::Violated { values } = &contracts[0].verdict else { panic!("{:?}", contracts[0]) };
    assert!(values.contains("limiter.step.x = "), "{values}");
    let _ = p;
}

#[test]
fn a_true_but_not_inductive_property_stays_unproven_with_a_reason() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "property never_both: never(vent and tank_guard.state == WATCH)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 3, &solver, 60).expect("Solver laeuft");
    let Verdict::Unproven { reason } = &reports[0].verdict else { panic!("{:?}", reports[0]) };
    assert!(reason.contains("Induktionsschritt offen"), "{reason}");
}
