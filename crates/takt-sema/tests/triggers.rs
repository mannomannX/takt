//! Trigger (Referenz 7.5, v1.2): einschuessig, Feuerzeit `event.t + d`,
//! `fired` mit den Captures, Fault disarmiert.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const PROGRAM: &str = r#"system:
    language = 1
    tick     = 10 ms

input dut_log : stream<line<64>> @ hw("u/rx") with capacity = 8, max_rate = 200 Hz

output vbus  : bool        @ hw("o/vbus")  with safe = true
output phase : int in 0..9 @ hw("o/phase") with safe = 0
output n_out : int in 0..99 @ hw("o/n")    with safe = 0

trigger cut_on_erase:
    when dut_log matches "Erasing sector {n:int}"
    then at event.t + 20 ms: vbus = false
    bound 5 ms

machine ctrl:
    initial ARMING

    state ARMING:
        enter:
            phase = 1
            arm cut_on_erase

        when cut_on_erase.fired as f:
            n_out = f.n
            -> CUT

    state CUT:
        enter:
            phase = 2

        after 100 ms: -> ARMING
"#;

fn compile(src: &str) -> Program {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn trace(src: &str, stimulus: &str, ticks: u64) -> String {
    let p = compile(src);
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(&p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

/// 7.5: Die Ausgabe wird fuer `event.t + d` geplant — hier 20 ms nach
/// dem Element in Tick 2, also in Tick 4.
#[test]
fn the_trigger_fires_at_event_time_plus_delay() {
    let t = trace(PROGRAM, "t=2 in dut_log Erasing sector 7\n", 10);
    assert!(t.contains("t=4 out vbus false"), "{t}");
}

/// Dieselbe Frist bei halbem Tick: Die Feuerzeit haengt am Ereignis,
/// nicht an der Tick-Phase (7.5).
#[test]
fn the_firing_time_does_not_depend_on_the_tick_phase() {
    let fast = PROGRAM.replace("tick     = 10 ms", "tick     = 5 ms");
    // Das Element kommt in Tick 4 (20 ms), die Ausgabe 20 ms spaeter.
    let t = trace(&fast, "t=4 in dut_log Erasing sector 7\n", 16);
    assert!(t.contains("t=8 out vbus false"), "{t}");
}

/// 7.5: `fired` traegt die Captures des Musters.
#[test]
fn fired_carries_the_captures() {
    let t = trace(PROGRAM, "t=2 in dut_log Erasing sector 7\n", 10);
    assert!(t.contains("out n_out 7"), "{t}");
}

/// 7.5: einschuessig — das zweite Element feuert nicht mehr.
#[test]
fn a_trigger_fires_at_most_once_per_arming() {
    let t = trace(PROGRAM, "t=2 in dut_log Erasing sector 7\nt=3 in dut_log Erasing sector 9\n", 10);
    // Die Zeile in Tick 0 ist der Anfangswert, kein Feuern.
    assert_eq!(t.matches("out n_out").count(), 2, "{t}");
    assert!(t.contains("out n_out 7") && !t.contains("out n_out 9"), "das zweite Element hat gefeuert:\n{t}");
}

/// 7.5: `disarm` nimmt die Armierung zurueck; danach feuert nichts.
#[test]
fn a_disarmed_trigger_does_not_fire() {
    let src = PROGRAM
        .replace("            arm cut_on_erase\n", "            arm cut_on_erase\n            disarm cut_on_erase\n");
    let t = trace(&src, "t=2 in dut_log Erasing sector 7\n", 10);
    assert!(!t.contains("out vbus false"), "{t}");
}

/// 7.5: `armed` ist lesbarer Zustand der armierenden Maschine und faellt
/// mit dem Feuern.
#[test]
fn armed_falls_when_the_trigger_fires() {
    // `loop:` steht vor `when` (2.3), darum am `enter:` angehaengt.
    let src = PROGRAM.replace(
        "            arm cut_on_erase\n",
        "            arm cut_on_erase\n\n        loop:\n            if not cut_on_erase.armed:\n                phase = 5\n",
    );
    let t = trace(&src, "t=2 in dut_log Erasing sector 7\n", 10);
    assert!(t.contains("out phase 5"), "`armed` blieb stehen:\n{t}");
}

/// 7.5: Ein Fault-Uebergang der armierenden Maschine disarmiert ihre
/// Trigger — die Ausgabe waere sonst ein Effekt aus einem verlassenen
/// Zustand.
#[test]
fn a_fault_disarms_the_triggers_of_its_machine() {
    let src = PROGRAM.replace(
        "            arm cut_on_erase\n",
        "            arm cut_on_erase\n\n        loop:\n            check phase < 5, \"weg\"\n",
    );
    // `phase = 1` haelt den Check erfuellt; erst ein Fault disarmiert.
    let src = src.replace("            phase = 1\n", "            phase = 9\n");
    let t = trace(&src, "t=2 in dut_log Erasing sector 7\n", 10);
    assert!(t.contains("fault ctrl CheckFailed"), "kein Fault:\n{t}");
    assert!(!t.contains("out vbus false"), "der Trigger feuerte nach dem Fault:\n{t}");
}
