//! Die Innentick-Sicht (`takt sim --steps`; plan/esp32c6.md 6.2).
//!
//! Der Trace zeigt je Tick, was beobachtbar ist; zwischen zwei Zeilen
//! liegen alle Anweisungen. Dieser Mitschnitt zeigt sie einzeln — und
//! aendert am Trace nichts.

use takt_interp::{RunOptions, Trace};

const SRC: &str = "\
system:
    language = 1
    tick     = 10 ms

output n : int in 0..100 @ hw(\"o/n\") with safe = 0

machine m:
    var k : int in 0..100 = 0

    initial RUN

    state RUN:
        loop:
            k = k + 1
            n = k
";

fn run(steps: bool) -> takt_interp::RunResult {
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let p = takt_sema::compile(SRC, &options).program.expect("Programm");
    let options = RunOptions { ticks: 3, steps, ..Default::default() };
    takt_interp::run(&p, &Trace::default(), &options).expect("Lauf")
}

#[test]
fn every_statement_of_a_tick_appears_with_its_value() {
    let result = run(true);
    let lines: Vec<&str> = result.steps.lines().collect();
    assert_eq!(lines.len(), 6, "zwei Anweisungen je Tick in drei Ticks:\n{}", result.steps);
    // Tick 0 laeuft vor dem Mitschnitt, `k` steht bei 1; Tick 1 macht 2.
    assert!(lines[0].starts_with("t=1 step m @"), "{}", lines[0]);
    assert!(lines[0].ends_with("= 2"), "der geschriebene Wert fehlt: {}", lines[0]);
    assert!(lines[5].ends_with("= 4"), "{}", lines[5]);
}

/// Der Mitschnitt ist Beobachtung: Ohne ihn ist der Trace derselbe.
#[test]
fn the_step_view_does_not_change_the_trace() {
    assert_eq!(run(true).trace.render(), run(false).trace.render());
    assert!(run(false).steps.is_empty());
}
