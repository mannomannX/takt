//! `resume` (5.12): der gespeicherte Kindpfad, und wann er nicht gilt.

use takt_interp::{RunOptions, Trace};

fn run(src: &str, stimulus: &str, ticks: u64) -> String {
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let p = out.program.expect("Programm");
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    let options = RunOptions { ticks, ..Default::default() };
    takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render()
}

const KOPF: &str = "system:\n    language = 1\n    tick = 10 ms\n\ncommand leave\ncommand back\n\n\
                    output phase : int in 0..9 @ hw(\"o/phase\") with safe = 0\n\n";

const WORK: &str = "machine m:\n    initial WORK\n\n    state WORK resume:\n        initial FIRST\n\n        \
                    when leave: -> AWAY\n\n        state FIRST:\n            enter:\n                phase = 1\n\n            \
                    after 20 ms: -> SECOND\n\n        state SECOND:\n            enter:\n                phase = 2\n\n    \
                    state AWAY:\n        enter:\n            phase = 9\n\n        when back: -> WORK\n";

/// Der Wiedereintritt betritt den gespeicherten Pfad, nicht `initial`.
#[test]
fn a_resume_state_re_enters_the_saved_child() {
    let trace = run(&format!("{KOPF}{WORK}"), "t=3 cmd leave\nt=5 cmd back\n", 7);
    assert!(trace.contains("t=5 state m WORK.SECOND"), "kein Rueckweg nach SECOND:\n{trace}");
    assert!(!trace.contains("t=5 state m WORK.FIRST"), "{trace}");
}

/// Vor dem ersten Austritt gibt es keinen Pfad: `initial` gilt.
#[test]
fn the_first_entry_takes_initial() {
    let trace = run(&format!("{KOPF}{WORK}"), "", 3);
    assert!(trace.contains("t=0 state m WORK.FIRST"), "{trace}");
}

/// Ein Fault-Uebergang betritt `initial`, nicht den gespeicherten Pfad (5.12).
#[test]
fn a_fault_transition_enters_initial_not_saved() {
    let src = format!(
        "{KOPF}machine m:\n    fault -> WORK\n\n    initial WORK\n\n    state WORK resume:\n        initial FIRST\n\n        \
         when leave: -> AWAY\n\n        state FIRST:\n            enter:\n                phase = 1\n\n            \
         after 20 ms: -> SECOND\n\n        state SECOND:\n            enter:\n                phase = 2\n\n    \
         state AWAY:\n        enter:\n            phase = 9\n\n        loop:\n            check phase < 5, \"weg\"\n"
    );
    let trace = run(&src, "t=3 cmd leave\n", 7);
    assert!(trace.contains("fault m CheckFailed"), "kein Fault:\n{trace}");
    assert!(trace.contains("t=3 state m WORK.FIRST"), "der Fault-Weg nahm den gespeicherten Pfad:\n{trace}");
}
