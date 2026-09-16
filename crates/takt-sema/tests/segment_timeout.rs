//! Segment-Default `sequence with timeout = d [-> X]` (6.2, FB-13): jedes
//! `until` ohne eigenen Timeout erbt ihn; ein eigener geht vor.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
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
