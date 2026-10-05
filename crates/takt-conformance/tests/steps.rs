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
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
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

/// Zwei Maschinen, ein Uebergang mit `exit` und `enter`, ein `check`, der
/// faultet, und der Weg ins Fault-Ziel (KON2-039).
const RICH: &str = "\
system:
    language = 1
    tick     = 10 ms

output n : int in 0..100 @ hw(\"o/n\") with safe = 0
output w : int in 0..100 @ hw(\"o/w\") with safe = 0

machine a:
    fault -> SAFE
    var k : int in 0..100 = 0
    initial RUN

    state RUN:
        enter:
            k = 10
        loop:
            k = k + 1
            n = k
        when k > 11: -> NEXT
        exit:
            n = 50

    state NEXT:
        enter:
            n = 99
        loop:
            check k < 5, \"too big\"

    state SAFE:
        enter:
            n = 7

machine b:
    initial ON

    state ON:
        loop:
            w = 3
";

/// **Jede Anweisung steht mit Maschine und Wert im Mitschnitt, auch in
/// `enter`, `exit`, einem `check` und dem Fault-Ziel, und eine zweite
/// Maschine mit ihrem Namen** (KON2-039): In Tick 1 rechnet `a` den
/// `loop:`, verlaesst `RUN` (`exit`), tritt in `NEXT` ein, faultet am
/// `check` und tritt in `SAFE` ein — alles im selben Tick. Der Trace bleibt
/// derselbe.
#[test]
fn every_kind_of_statement_appears_in_its_tick() {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(RICH, &options).program.expect("Programm");
    let with = takt_interp::run(&p, &Trace::default(), &RunOptions { ticks: 3, steps: true, ..Default::default() })
        .expect("Lauf");
    let without =
        takt_interp::run(&p, &Trace::default(), &RunOptions { ticks: 3, ..Default::default() }).expect("Lauf");
    assert_eq!(with.trace.render(), without.trace.render(), "der Mitschnitt aendert den Trace");
    let steps = &with.steps;
    // Die Zeile einer Anweisung: Tick, Maschine und der Text an ihrer Stelle.
    let line = |tick: u64, machine: &str, statement: &str| -> Option<String> {
        let at = RICH.find(statement)?;
        let head = format!("t={tick} step {machine} @");
        steps.lines().find(|l| l.starts_with(&head) && l.contains(&format!("@{at}"))).map(str::to_string)
    };
    for (tick, machine, statement, value) in [
        (1, "a", "k = k + 1", "= 12"),
        (1, "a", "n = k\n", "= 12"),
        (1, "a", "n = 50", "= 50"),
        (1, "a", "n = 99", "= 99"),
        (1, "b", "w = 3", "= 3"),
        (1, "a", "check k < 5", "= fault too big"),
        (1, "a", "n = 7", "= 7"),
        (2, "b", "w = 3", "= 3"),
    ] {
        let found = line(tick, machine, statement)
            .unwrap_or_else(|| panic!("t={tick} {machine} `{statement}` fehlt:\n{steps}"));
        assert!(found.ends_with(value), "t={tick} {machine} `{statement}`: {found}");
    }
}
