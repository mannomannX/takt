//! Segment-Default `sequence with timeout = d [-> X]` (6.2, FB-13): jedes
//! `until` ohne eigenen Timeout erbt ihn; ein eigener geht vor.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn trace(p: &Program, stimulus: &str, ticks: u64) -> String {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

const PROGRAM: &str = "
command go
output done : bool @ hw(\"o/done\") with safe = false
machine m:
    initial RUN
    state RUN:
        sequence with timeout = 20 ms -> LATE:
            until go
            done = true
            -> DONE
    state DONE:
        when false: -> RUN
    state LATE:
        when false: -> RUN
";

#[test]
fn an_until_without_timeout_inherits_the_segment_default() {
    let p = compile(PROGRAM);
    let t = trace(&p, "", 5);
    assert!(t.contains("t=2 state m LATE"), "{t}");
    assert!(!t.contains("done true"), "{t}");
}

#[test]
fn the_command_in_time_takes_the_sequence_on() {
    let p = compile(PROGRAM);
    let t = trace(&p, "t=1 cmd go\n", 5);
    assert!(t.contains("out done true"), "{t}");
    assert!(t.contains("state m DONE"), "{t}");
    assert!(!t.contains("LATE"), "{t}");
}

#[test]
fn an_own_timeout_wins_over_the_default() {
    let p = compile(
        "
command go
machine m:
    initial RUN
    state RUN:
        sequence with timeout = 50 ms -> LATE:
            until go timeout 10 ms -> EARLY
            -> DONE
    state DONE:
        when false: -> RUN
    state LATE:
        when false: -> RUN
    state EARLY:
        when false: -> RUN
",
    );
    let t = trace(&p, "", 8);
    assert!(t.contains("t=1 state m EARLY"), "{t}");
}

/// 6.2: Ohne `-> X` faellt der geerbte Timeout ins Fault-Ziel des Zustands,
/// als Fault der Art `Timeout`.
#[test]
fn a_default_without_target_faults_into_the_fault_target() {
    let p = compile(
        "
command go
output done : bool @ hw(\"o/done\") with safe = false
machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        sequence with timeout = 20 ms:
            until go
            done = true
    state SAFE:
        when false: -> RUN
",
    );
    let t = trace(&p, "", 5);
    assert!(t.contains("t=2 fault m Timeout") && t.contains("-> SAFE"), "{t}");
    assert!(t.contains("t=2 state m SAFE") && !t.contains("done true"), "{t}");
}

/// Zwei `until` ohne eigenen Timeout erben beide, jedes ab dem Beginn
/// seines Segments: `a` in Tick 1, `b` kommt nie — spaet in Tick 3.
#[test]
fn every_until_inherits_from_the_start_of_its_segment() {
    let p = compile(
        "
command a
command b
output done : bool @ hw(\"o/done\") with safe = false
machine m:
    initial RUN
    state RUN:
        sequence with timeout = 20 ms -> LATE:
            until a
            until b
            done = true
    state LATE:
        when false: -> RUN
",
    );
    let t = trace(&p, "t=1 cmd a\n", 6);
    assert!(t.contains("t=3 state m LATE"), "{t}");
    assert!(!t.contains("t=2 state m LATE"), "{t}");
}

/// Der weiche Timeout `until … timeout d else:` hat seinen eigenen Timeout
/// (die Grammatik verlangt ihn) und geht dem Segment-Default vor; das
/// `else:` laeuft, und die Sequenz geht weiter.
#[test]
fn a_soft_timeout_wins_over_the_default() {
    let p = compile(
        "
command go
output soft : bool @ hw(\"o/soft\") with safe = false
output done : bool @ hw(\"o/done\") with safe = false
machine m:
    initial RUN
    state RUN:
        sequence with timeout = 20 ms -> LATE:
            until go timeout 50 ms else:
                soft = true
            done = true
    state LATE:
        when false: -> RUN
",
    );
    let t = trace(&p, "", 8);
    assert!(t.contains("t=5 out soft true") && t.contains("out done true"), "{t}");
    assert!(!t.contains("LATE"), "{t}");
}

/// `until s matches …` ohne eigenen Timeout erbt den Segment-Default wie
/// jedes andere `until`.
#[test]
fn an_until_matches_inherits_the_default() {
    let src = "
input  rx    : stream<line<32>> @ hw(\"uart/rx\") with capacity = 8, max_rate = 200 Hz
output rx_in : stream<line<32>> @ sim(\"uart/rx\")
output done  : bool @ hw(\"o/done\") with safe = false
machine m:
    initial RUN
    state RUN:
        sequence with timeout = 20 ms -> LATE:
            until rx matches \"READY\"
            done = true
    state LATE:
        when false: -> RUN
";
    let p = compile(src);
    let late = trace(&p, "", 5);
    assert!(late.contains("t=2 state m LATE"), "{late}");
    let ready = trace(&p, "t=1 in rx READY\n", 5);
    assert!(ready.contains("out done true") && !ready.contains("LATE"), "{ready}");
}

/// 6.2: `until c` wird zu `when c` vor `after d` — kommt das Command genau
/// im Tick des Timeouts, gewinnt es.
#[test]
fn a_command_in_the_timeout_tick_wins() {
    let p = compile(PROGRAM);
    let t = trace(&p, "t=2 cmd go\n", 5);
    assert!(t.contains("out done true") && !t.contains("LATE"), "{t}");
}
