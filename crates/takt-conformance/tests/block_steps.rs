//! `step` einer Blockinstanz in beiden Implementierungen (5.7, Satz 9.4.4).
//!
//! Eine Instanz hat je Tick genau ein Ergebnis: Ein zweiter Aufruf
//! derselben Aktivierung schreitet nicht, sondern liefert den Wert oder
//! den Fault des ersten (FB-423). Ein Fault im Schritt nimmt den Fault-Pfad
//! der rufenden Maschine (FB-424).

mod common;

use takt_interp::{RunOptions, Trace};
use takt_llvm::toolchain::Clang;

const HEAD: &str = "\
system:
    language = 1
    tick     = 1 ms

output probe : int in 0..10000 @ hw(\"probe\") with safe = 0

block counter():
    var k : int in 0..3 = 0
    step() -> int:
        k = k + 1
        return k
";

/// Beide Implementierungen liefern dieselben Outputs; zurueck kommt der
/// Trace des Interpreters, `None` ohne clang.
fn agree(body: &str, name: &str, ticks: u64) -> Option<String> {
    let path = common::clang_path()?;
    let src = format!("{HEAD}{body}");
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    assert!(!out.has_errors(), "{name}: {:?}", out.diagnostics);
    let p = out.program.expect("Programm");
    let interpreted = takt_interp::run(&p, &Trace::default(), &RunOptions { ticks, ..Default::default() })
        .expect("Lauf")
        .trace
        .render();
    let native = common::run_native_all(&Clang::At(path), &p, name, ticks).expect("nativ");
    let diffs = takt_conformance::compare(&interpreted, &native);
    let list: Vec<String> = diffs.iter().take(6).map(|d| format!("  {d}")).collect();
    assert!(
        diffs.is_empty(),
        "{name}:\n{}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        list.join("\n")
    );
    Some(interpreted)
}

#[test]
fn a_self_transition_reruns_the_loop_with_the_first_result() {
    // 9.3: `-> RUN` aus RUN betritt RUN neu, sein `loop:` laeuft im
    // Entry-Modus ein zweites Mal. Der zweite `step` liefert den ersten
    // Wert; der Zaehler waechst um eins je Tick, nicht um zwei.
    let Some(trace) = agree(
        "
machine m:
    var c = counter()
    var seen : int in 0..10000 = 0
    initial RUN
    state RUN:
        loop:
            var k : int in 0..3 = c.step()
            seen = seen + 1
            probe = k * 100 + seen
        after 2 ms: -> RUN
",
        "self_transition",
        2,
    ) else {
        return;
    };
    assert!(trace.contains("t=2 out probe 304"), "Tick 2 rechnet zweimal mit k = 3:\n{trace}");
}

#[test]
fn a_fault_in_a_step_takes_the_fault_path_of_the_caller() {
    // `k` laeuft im vierten Schritt aus `0..3`: Der Range-Fault im Block
    // fuehrt die rufende Maschine nach SAFE (4.1, 5.3).
    let Some(trace) = agree(
        "
machine m:
    fault -> SAFE
    var c = counter()
    initial RUN
    state RUN:
        loop:
            probe = c.step()
    state SAFE:
        enter:
            probe = 99
        after 1 s: -> RUN
",
        "fault_in_step",
        5,
    ) else {
        return;
    };
    assert!(trace.contains("t=3 out probe 99"), "der vierte Schritt faultet:\n{trace}");
}

#[test]
fn a_second_step_after_a_faulted_one_faults_again() {
    // Das Ergebnis des Ticks ist der Fault: SAFE steppt im Entry-Modus
    // erneut und faultet mit derselben Art weiter nach FAULTED (5.3).
    let Some(trace) = agree(
        "
machine m:
    fault -> SAFE
    var c = counter()
    initial RUN
    state RUN:
        loop:
            probe = c.step()
    state SAFE:
        loop:
            var k : int in 0..3 = c.step()
            probe = k + 50
        after 1 s: -> RUN
",
        "second_after_fault",
        5,
    ) else {
        return;
    };
    assert!(trace.contains("t=3 state m FAULTED"), "der zweite Aufruf faultet weiter:\n{trace}");
}

/// Ein Abort aus `starter` im Tick 3 erreicht `m` in der Abort-Phase (5.4);
/// `SAFE` steppt dieselbe Instanz wie `RUN`.
fn abort_program(period: &str) -> String {
    format!(
        "
block tally():
    var k : int in 0..99 = 0
    step() -> int:
        k = k + 1
        return k

machine starter:
    initial WAIT
    state WAIT:
        after 3 ms: -> FIRE
    state FIRE:
        loop:
            abort \"stop\"
        after 1 s: -> WAIT

machine m{period}:
    fault -> SAFE
    var c = tally()
    initial RUN
    state RUN:
        loop:
            probe = c.step()
    state SAFE:
        enter:
            probe = c.step()
        after 1 s: -> RUN
"
    )
}

#[test]
fn the_abort_phase_of_an_active_machine_keeps_the_result_of_its_step() {
    // `m` schritt in Tick 3 schon (4); die Zustellung gehoert zur selben
    // Aktivierung, `SAFE` sieht 4 und schreitet nicht noch einmal.
    let Some(trace) = agree(&abort_program(""), "abort_active", 4) else {
        return;
    };
    assert!(trace.contains("t=3 out probe 4") && !trace.contains("probe 5"), "derselbe Wert in SAFE:\n{trace}");
}

#[test]
fn the_abort_phase_of_an_idle_machine_steps_afresh() {
    // `m` lief zuletzt in Tick 0; die Zustellung in Tick 3 ist ihre eigene
    // Ausfuehrung und schreitet: 2, nicht der alte Wert 1.
    let Some(trace) = agree(&abort_program(" every 10 ms"), "abort_idle", 4) else {
        return;
    };
    assert!(trace.contains("t=3 out probe 2"), "ein frischer Schritt in SAFE:\n{trace}");
}
