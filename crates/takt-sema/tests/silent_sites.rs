//! Pruefstellen, die nie faulten (13.2): Sie zaehlen in der Coverage nur mit
//! dem ersten Ausgang, und die Ratsche fuehrt sie getrennt
//! (`takt_interp::coverage::Silent`). Je Grund belegt ein Test, dass der
//! zweite Ausgang nicht vorkommt.

use takt_diag::Policy;
use takt_interp::coverage::{Silent, sites};
use takt_interp::{CoverKind, RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&format!("{HEAD}{body}"), &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die Gruende der Stellen einer Art (`range`, `div` …), in Quellreihenfolge.
fn reasons(p: &Program, kind: &str) -> Vec<Option<Silent>> {
    let tag = takt_mir::analysis::walk::tag_of_name(kind).expect("Art");
    let mut all: Vec<_> = sites(p).into_iter().filter(|s| s.tag == tag).collect();
    all.sort_by_key(|s| s.start);
    all.into_iter().map(|s| s.silent).collect()
}

/// **Eine bewiesene Range faultet nie** (3.4, 11.3): Die Intervallanalyse
/// beweist `n + 1` in `0..99`; die Stelle bleibt im MIR, ausgewertet wird
/// sie in jedem Tick, und ein Verstoss waere ein Fehler des Compilers
/// (`Trap::Bug`, siehe `an_external_proof_drops_the_check_and_a_stale_one_is_refused`
/// in analysis.rs), nie ein `RangeFault`.
#[test]
fn a_proven_range_is_a_site_without_a_fault() {
    let p = compile(
        "output o : int in 0..99 @ hw(\"o/o\") with safe = 0

machine m:
    var n : int in 0..9 = 5
    initial RUN
    state RUN:
        loop:
            o = n + 1
",
    );
    assert_eq!(reasons(&p, "range"), [Some(Silent::Proven)]);
    let r = run(&p, &Trace::default(), &RunOptions { ticks: 3, ..Default::default() }).expect("Lauf");
    let key = sites(&p).into_iter().find(|s| s.silent == Some(Silent::Proven)).expect("Stelle").key();
    assert!(r.coverage.hits.contains_key(&(CoverKind::Site, String::new(), key.clone())), "ausgewertet");
    assert!(!r.coverage.hits.contains_key(&(CoverKind::SiteFailed, String::new(), key)), "nie verletzt");
}

/// **Ein Fault in einer Meldung wird `<invalid>`** (`format::render`, 3.9):
/// Die Division im Text eines `check` und eines `abort` teilt durch null,
/// der Lauf meldet den `check` mit `<invalid>` im Text und geht weiter; kein
/// Arithmetik-Fault entsteht. So deckt dieser Test den zweiten Ausgang
/// aller Stellen in Meldungen einmal ab.
#[test]
fn a_fault_inside_a_message_renders_as_invalid() {
    let p = compile(
        "input  a : int in 0..9 @ hw(\"i/a\")
input  b : int in 0..9 @ hw(\"i/b\")
output o : int in 0..9 @ hw(\"o/o\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            o = a.or(0)
            check a.or(0) < 5, \"zu gross: {a.or(0) / b.or(0)}\"
            if a.or(0) == 9:
                abort \"Abbruch: {a.or(0) % b.or(0)}\"
        when false: -> RUN
",
    );
    let all: Vec<_> = sites(&p).into_iter().map(|s| (s.key(), s.silent)).collect();
    assert_eq!(reasons(&p, "div"), [Some(Silent::Message), Some(Silent::Message)], "{all:?}");
    let stimulus = Trace::parse("t=0 in a 7\nt=0 in b 0\n").expect("Stimulus");
    let r = run(&p, &stimulus, &RunOptions { ticks: 2, ..Default::default() }).expect("Lauf");
    let t = r.trace.render();
    assert!(t.contains("CheckFailed \"zu gross: <invalid>\""), "{t}");
    assert!(!t.contains("Arithmetic"), "{t}");
}

/// **Eine Beobachtung faultet nie** (5.6): Die Bedingung eines `alert`, die
/// durch null teilt, laesst ihn als ungueltig feuern, und ein `measure`
/// mit derselben Division traegt keinen Wert; die Maschine faultet nicht.
#[test]
fn an_observation_never_faults() {
    let p = compile(
        "input  a : int in 0..9 @ hw(\"i/a\")
input  b : int in 0..9 @ hw(\"i/b\")
output o : int in 0..9 @ hw(\"o/o\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            o = a.or(0)
            alert a.or(1) / b.or(0) > 1, \"Verhaeltnis\"
            measure q = a.or(1) / b.or(0)
",
    );
    assert_eq!(reasons(&p, "div"), [Some(Silent::Observe), Some(Silent::Observe)]);
    let stimulus = Trace::parse("t=0 in a 7\nt=0 in b 0\n").expect("Stimulus");
    let r = run(&p, &stimulus, &RunOptions { ticks: 2, ..Default::default() }).expect("Lauf");
    let t = r.trace.render();
    assert!(!t.contains(" fault "), "{t}");
    assert!(r.faults.is_empty(), "{:?}", r.faults);
}
