//! Die Summanden der Safe-State-Latenz (9.4.5) an Programmen aus
//! Quelltext, und was der Interpreter im Lauf davon braucht.
//!
//! Sie stehen hier, weil `takt-prove` neben der MIR auch das Sema und den
//! Interpreter kennt.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_mir::analysis::latency::{Latency, SiteKind, latency};
use takt_sema::{Build, Options};

/// Ein Ventil, das in `RUN` offen und in `SAFE` zu ist; `check` steht in
/// `RUN`, `fault -> SAFE`.
fn program(head: &str, every: &str, check: &str) -> Program {
    compile(&format!(
        "system:
    language = 1
{head}
input c : bool @ hw(\"i/c\")
output valve : bool @ hw(\"o/valve\") with safe = false

machine m{every}:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            valve = true
            {check}
    state SAFE:
        loop:
            valve = false
"
    ))
}

fn compile(src: &str) -> Program {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die einzige `check`-Stelle.
fn check_site(l: &Latency) -> &takt_mir::analysis::latency::Site {
    l.sites.iter().find(|s| s.kind == SiteKind::Check).expect("die Stelle")
}

/// Der Tick, in dem `valve` auf `false` faellt, wenn `c` ab `from` falsch
/// ist.
fn safe_at(p: &Program, from: u64, ticks: u64) -> u64 {
    let stimulus: String = (0..ticks).map(|k| format!("t={k} in c {}\n", k < from)).collect();
    let out = run(p, &Trace::parse(&stimulus).expect("Stimulus"), &RunOptions { ticks, ..Default::default() })
        .expect("Lauf")
        .trace
        .render();
    out.lines()
        .find_map(|l| l.strip_suffix(" out valve false").and_then(|t| t.strip_prefix("t=")?.parse().ok()))
        .unwrap_or_else(|| panic!("das Ventil schliesst:\n{out}"))
}

/// SYN-027 (9.4.5): Die Bestaetigung zaehlt Basis-Ticks, `n_m ·
/// (max(1, ceil(d / P_m)) − 1)`: `for 7 ms` bei 2 ms sind vier verletzte
/// Auswertungen, die erste zaehlt schon die Erkennung. Bei `every 10 ms`
/// und `T0 = 1 ms` liegen die weiteren je zehn Basis-Ticks auseinander.
#[test]
fn the_confirmation_counts_base_ticks() {
    let p = program("    tick = 2 ms\n", "", "check c, \"c\" for 7 ms");
    let s = check_site(&latency(&p)).clone();
    assert_eq!((s.detect, s.confirm), (1, 3), "{s:?}");
    let p = program("    tick = 1 ms\n", " every 10 ms", "check c, \"c\" for 30 ms");
    let s = check_site(&latency(&p)).clone();
    assert_eq!((s.detect, s.confirm), (10, 20), "{s:?}");
    let p = program("    tick = 1 ms\n", "", "check c, \"c\"");
    assert_eq!(check_site(&latency(&p)).confirm, 0, "ohne `for` kein Summand");
}

/// SYN-027: `D_commit` ist 0 bei `asap` und 1 bei `boundary` (5.2 Punkt 4);
/// sonst unterscheiden sich die Schranken nicht.
#[test]
fn boundary_holds_the_unsafe_value_one_tick_longer() {
    let asap = latency(&program("    tick = 1 ms\n", "", "check c, \"c\""));
    let boundary = latency(&program("    tick = 1 ms\n    output_timing = boundary\n", "", "check c, \"c\""));
    assert_eq!((asap.commit, boundary.commit), (0, 1));
    assert_eq!(boundary.worst_case(), asap.worst_case() + 1);
    let ticks = |l: &Latency| check_site(l).ticks();
    assert_eq!(ticks(&asap), ticks(&boundary), "die Stelle selbst haengt nicht am Commit");
}

/// SYN-027: Im Lauf haelt die Schranke. Die Verletzung tritt im
/// schlimmsten Fall kurz nach einer Aktivierung (`before`) ein; von dort
/// braucht der Interpreter genau `D_detect + D_confirm + D_commit` Ticks —
/// das Gegenbeispiel `every 10 ms` mit `for 30 ms` 30 Ticks, wo die alte
/// Formel 13 sagte. Der Weg durch den Fault-Wald kostet im Lauf keinen
/// Tick (`the_fault_forest_is_walked_within_one_tick`); `depth` macht die
/// Schranke darum grob, nie zu klein.
#[test]
fn the_bound_holds_in_the_run() {
    for (head, every, check, before, want) in [
        ("    tick = 1 ms\n", "", "check c, \"c\"", 4, 1),
        ("    tick = 2 ms\n", "", "check c, \"c\" for 7 ms", 4, 4),
        ("    tick = 1 ms\n    output_timing = boundary\n", "", "check c, \"c\"", 4, 2),
        ("    tick = 1 ms\n", " every 10 ms", "check c, \"c\" for 30 ms", 10, 30),
    ] {
        let p = program(head, every, check);
        let l = latency(&p);
        let ticks = safe_at(&p, before + 1, 60) - before + l.commit;
        assert_eq!(ticks, want, "{every} {check}");
        let s = check_site(&l);
        assert_eq!(ticks, s.detect + s.confirm + l.commit, "ohne Fault-Wald exakt: {s:?}");
        assert!(ticks <= l.worst_case(), "{check}: {ticks} Ticks, Schranke {}", l.worst_case());
    }
}

/// SYN-027: Die Kette RUN -> X -> Y -> SAFE scheitert im Entry-Modus
/// desselben Ticks weiter (5.3, Lemma 9.3.1), auch bei `n_m = 10`: Das
/// Ventil faellt in der erkennenden Aktivierung, nicht eine oder drei
/// Aktivierungen spaeter. Der Summand `depth` (hier 4) zaehlt Ticks, die
/// der Lauf nicht braucht.
#[test]
fn the_fault_forest_is_walked_within_one_tick() {
    let p = compile(
        "system:
    language = 1
    tick = 1 ms

input c : bool @ hw(\"i/c\")
output valve : bool @ hw(\"o/valve\") with safe = false

machine m every 10 ms:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            valve = true
            check c, \"a\" -> X
    state X:
        fault -> Y
        loop:
            check c, \"x\"
    state Y:
        loop:
            check c, \"y\"
    state SAFE:
        loop:
            valve = false
",
    );
    let l = latency(&p);
    let run_state = p.machines[0].state_named("RUN");
    let site = l.sites.iter().find(|s| s.state == run_state).expect("die Stelle in RUN");
    assert_eq!((site.detect, site.fault), (10, 4), "{site:?}");
    assert_eq!(safe_at(&p, 11, 60), 20, "die ganze Kette in der Aktivierung von Tick 20");
}

/// SYN-027, Pruefung 61: Das Gegenbeispiel mit `within 20 ms` haelt die
/// Frist nicht (30 Ticks im Lauf, Schranke 10 + 20 + 2); die alte Formel
/// liess es mit 15 Ticks durch. Mit `within 40 ms` passt es.
#[test]
fn check_61_counts_the_confirmation_in_base_ticks() {
    let src = |within: &str| {
        format!(
            "system:
    language = 1
    tick = 1 ms

input c : bool @ hw(\"i/c\")
output valve : bool @ hw(\"o/valve\") with safe = false

machine m every 10 ms:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            valve = true
            check c, \"c\" for 30 ms within {within}
    state SAFE:
        loop:
            valve = false
"
        )
    };
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let missed = takt_sema::compile(&src("20 ms"), &options);
    let sc61: Vec<String> = missed.diagnostics.iter().filter(|d| d.code == "SC-61").map(|d| format!("{d}")).collect();
    assert_eq!(sc61.len(), 1, "{:?}", missed.diagnostics);
    assert!(sc61[0].contains("32 Ticks statt 20") && sc61[0].contains("10 erkennen + 20 bestaetigen"), "{}", sc61[0]);
    let met = takt_sema::compile(&src("40 ms"), &options);
    assert!(!met.has_errors(), "{:?}", met.diagnostics);
}
