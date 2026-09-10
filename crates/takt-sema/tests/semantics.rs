//! Ausfuehrbare Semantik (Referenz 9.2 bis 9.4): Tick, Zustandswechsel,
//! Entry-Tick-Regel, Fault-Wald, Abort-Phase, Multirate, Beobachtungen und
//! das Lauf-Verdikt. Die Programme kommen aus `takt-sema`, der Trace aus
//! `grammar/trace.md`.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, Verdict, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

/// Uebersetzt ein Programm und verlangt Fehlerfreiheit.
fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Fuehrt ein Programm mit Stimulus aus und liefert den Trace als Text.
fn simulate(body: &str, stimulus: &str, ticks: u64) -> String {
    let program = compile(body);
    let stim = Trace::parse(stimulus).expect("Stimulus lesbar");
    let out = run(&program, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    out.trace.render()
}

/// Fuehrt aus und liefert Trace und Verdikt.
fn simulate_verdict(body: &str, stimulus: &str, ticks: u64) -> (String, Verdict) {
    let program = compile(body);
    let stim = Trace::parse(stimulus).expect("Stimulus lesbar");
    let out = run(&program, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    (out.trace.render(), out.verdict)
}

#[test]
fn tick_zero_enters_initial_and_commits_safe_outputs() {
    let body = "\
output valve : bool @ hw(\"plc/do0\") with safe = false

machine m:
    initial IDLE
    state IDLE:
        loop: pass
";
    let trace = simulate(body, "", 0);
    assert!(trace.contains("t=0 state m IDLE\n"), "{trace}");
    assert!(trace.contains("t=0 out valve false\n"), "{trace}");
    assert!(trace.ends_with("t=0 verdict-final INCONCLUSIVE\n"), "{trace}");
}

#[test]
fn transition_takes_one_tick_and_runs_enter_in_entry_mode() {
    let body = "\
output valve : bool @ hw(\"plc/do0\") with safe = false
command start

machine m:
    initial IDLE
    state IDLE:
        when start: -> OPEN
    state OPEN:
        enter:
            valve = true
        loop: pass
";
    // Der Command gilt genau einen Tick (8.5).
    let trace = simulate(body, "t=1 cmd start\n", 3);
    assert!(trace.contains("t=1 state m OPEN\n"), "Uebergang im Tick des Commands: {trace}");
    assert!(trace.contains("t=1 out valve true\n"), "enter im selben Tick (5.2): {trace}");
}

#[test]
fn after_fires_never_in_the_entry_tick() {
    let body = "\
output phase : int @ hw(\"o/step\") with safe = 0

machine m:
    initial A
    state A:
        enter:
            phase = 1
        after 2 ms: -> B
    state B:
        enter:
            phase = 2
        loop: pass
";
    let trace = simulate(body, "", 5);
    // Mindestverweildauer: `after 2 ms` bei 1 ms Tick feuert im Tick 2 (7.1).
    assert!(trace.contains("t=2 state m B\n"), "{trace}");
    assert!(!trace.contains("t=1 state m B"), "nie im Entry-Tick: {trace}");
}

#[test]
fn check_failure_goes_to_the_fault_target_in_the_same_tick() {
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output valve : bool                     @ hw(\"plc/do0\") with safe = false

machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        enter:
            valve = true
        loop:
            check p < 50 bar, \"ueberdruck {p}\"
    state SAFE:
        enter:
            valve = false
        loop: pass
";
    let stim = "t=0 in p 10 bar\nt=2 in p 80 bar\n";
    let (trace, verdict) = simulate_verdict(body, stim, 4);
    assert!(trace.contains("t=2 fault m CheckFailed \"ueberdruck 80.0\" -> SAFE\n"), "{trace}");
    assert!(trace.contains("t=2 state m SAFE\n"), "im selben Tick (5.2): {trace}");
    assert!(trace.contains("t=2 out valve false\n"), "Output vor dem Commit sicher: {trace}");
    assert_eq!(verdict, Verdict::Fail, "fault_is_fail (13.5)");
}

#[test]
fn invalid_input_is_a_sensor_fault_unless_dominated() {
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output ok    : bool                     @ hw(\"o/ok\") with safe = false

machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            check p < 50 bar, \"druck\"
    state SAFE:
        loop: pass
";
    let stim = "t=0 in p 10 bar\nt=2 in p bad reason=OutOfRange\n";
    let trace = simulate(body, stim, 3);
    assert!(trace.contains("t=2 fault m SensorFault"), "{trace}");
    assert!(trace.contains("-> SAFE\n"), "{trace}");
}

#[test]
fn or_and_valid_dominate_the_implicit_check() {
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output used  : float[bar]               @ hw(\"o/used\") with safe = 0 bar

machine m:
    initial RUN
    state RUN:
        loop:
            used = p.or(7 bar)
            if p.valid:
                used = p
";
    let stim = "t=0 in p bad\nt=2 in p 30 bar\n";
    let trace = simulate(body, stim, 3);
    assert!(trace.contains("t=0 out used 7.0 bar\n"), "`.or` faultet nie (3.5): {trace}");
    assert!(trace.contains("t=2 out used 30.0 bar\n"), "unter `.valid` dominiert: {trace}");
    assert!(!trace.contains("fault"), "kein Fault: {trace}");
}

#[test]
fn abort_reaches_every_machine_in_the_same_tick() {
    let body = "\
output a : bool @ hw(\"o/a\") with safe = false
output b : bool @ hw(\"o/b\") with safe = false
command stop

machine one:
    fault -> SAFE
    initial RUN
    state RUN:
        enter:
            a = true
        loop:
            if stop:
                abort \"operator\"
    state SAFE:
        enter:
            a = false
        loop: pass

machine two:
    fault -> SAFE2
    initial RUN2
    state RUN2:
        enter:
            b = true
        loop: pass
    state SAFE2:
        enter:
            b = false
        loop: pass
";
    let trace = simulate(body, "t=2 cmd stop\n", 4);
    assert!(trace.contains("t=2 state one SAFE\n"), "{trace}");
    assert!(trace.contains("t=2 state two SAFE2\n"), "Abort-Phase im selben Tick (5.4): {trace}");
    assert!(trace.contains("t=2 out a false\n") && trace.contains("t=2 out b false\n"), "{trace}");
}

#[test]
fn abort_is_idempotent_until_a_normal_transition() {
    let body = "\
output a : bool @ hw(\"o/a\") with safe = false
command stop

machine one:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            if stop:
                abort \"operator\"
    state SAFE:
        enter:
            a = false
        loop: pass
";
    // Zweimal abort: die Maschine eskaliert nicht nach FAULTED (5.4).
    let trace = simulate(body, "t=1 cmd stop\nt=2 cmd stop\n", 4);
    assert!(trace.contains("t=1 state one SAFE\n"), "{trace}");
    assert!(!trace.contains("FAULTED"), "Abort-Latch verhindert die Eskalation: {trace}");
}

#[test]
fn multirate_activates_by_countdown() {
    let body = "\
output fast : int @ hw(\"o/f\") with safe = 0
output slow : int @ hw(\"o/s\") with safe = 0

machine f:
    var n : int = 0
    initial RUN
    state RUN:
        loop:
            n = n + 1
            fast = n

machine s every 3 ms:
    var n : int = 0
    initial RUN2
    state RUN2:
        loop:
            n = n + 1
            slow = n
";
    let trace = simulate(body, "", 6);
    // Die schnelle Maschine zaehlt jeden Tick, die langsame jeden dritten (7.2).
    assert!(trace.contains("t=6 out fast 7\n"), "{trace}");
    assert!(trace.contains("t=6 out slow 3\n"), "{trace}");
}

#[test]
fn published_variables_and_signals_have_unit_delay() {
    let body = "\
output seen : int @ hw(\"o/seen\") with safe = 0
output got  : bool @ hw(\"o/got\") with safe = false

machine producer:
    pub var count : int = 0
    signal done
    initial RUN
    state RUN:
        loop:
            count = count + 1
            raise done

machine consumer:
    initial RUN2
    state RUN2:
        loop:
            seen = producer.count
            got = producer.done
";
    let trace = simulate(body, "", 3);
    // Unit-Delay: der Verbraucher sieht den Wert des vorigen Ticks (1.4).
    assert!(trace.contains("t=0 pub producer count 1\n"), "{trace}");
    assert!(trace.contains("t=1 out seen 1\n"), "Unit-Delay: {trace}");
    // Das Signal wird im Tick des Pulses gemeldet (T2), der Leser sieht es
    // einen Tick spaeter (5.8).
    assert!(trace.contains("t=1 signal producer done\n"), "{trace}");
    assert!(trace.contains("t=1 out got true\n"), "{trace}");
}

#[test]
fn sequence_runs_through_its_segments() {
    let body = "\
output valve : bool @ hw(\"plc/do0\") with safe = false
output ready : bool @ hw(\"plc/do1\") with safe = false

machine m:
    initial RUN
    state RUN:
        sequence:
            valve = true
            wait 2 ms
            ready = true
            -> DONE
    state DONE:
        loop: pass
";
    let trace = simulate(body, "", 5);
    assert!(trace.contains("t=0 out valve true\n"), "erstes Segment im Entry-Tick: {trace}");
    assert!(trace.contains("t=2 out ready true\n"), "nach `wait 2 ms`: {trace}");
    assert!(trace.contains("state m DONE\n"), "{trace}");
}

#[test]
fn observations_and_verdict_follow_13_5() {
    let body = "\
output x : int @ hw(\"o/x\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            log \"tick laeuft\"
            measure counter = 42
            verify true, \"immer wahr\"
        when true: -> DONE
    state DONE:
        enter:
            verdict pass \"fertig\"
        loop: pass
";
    let (trace, verdict) = simulate_verdict(body, "", 3);
    assert!(trace.contains("t=1 log m \"tick laeuft\"\n"), "{trace}");
    assert!(trace.contains("t=1 measure m counter 42\n"), "{trace}");
    assert!(trace.contains("t=1 verify m ok \"immer wahr\"\n"), "{trace}");
    assert!(trace.contains("verdict m pass \"fertig\"\n"), "{trace}");
    assert_eq!(verdict, Verdict::Pass);
}

#[test]
fn verify_failure_makes_the_run_fail() {
    let body = "\
output x : int @ hw(\"o/x\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            verify false, \"nie wahr\"
            verdict pass \"trotzdem\"
";
    let (trace, verdict) = simulate_verdict(body, "", 2);
    assert!(trace.contains("verify m fail \"nie wahr\"\n"), "{trace}");
    assert_eq!(verdict, Verdict::Fail, "FAIL absorbiert (13.5)");
}

#[test]
fn alerts_report_only_their_edges() {
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output x     : int                      @ hw(\"o/x\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            alert p < 50 bar, \"hoch\"
";
    let stim = "t=0 in p 10 bar\nt=2 in p 80 bar\nt=4 in p 20 bar\n";
    let trace = simulate(body, stim, 5);
    // `alert cond` meldet die Verletzung von `cond` (5.6), nicht ihr Zutreffen.
    assert!(trace.contains("t=2 alert m on \"hoch\"\n"), "steigende Flanke: {trace}");
    assert!(trace.contains("t=4 alert m off \"hoch\"\n"), "fallende Flanke: {trace}");
    assert_eq!(trace.matches("alert m on \"hoch\"").count(), 1, "genau eine steigende Flanke: {trace}");
    assert_eq!(trace.matches("alert m off \"hoch\"").count(), 1, "genau eine fallende Flanke: {trace}");
    assert!(!trace.contains("fault"), "ein Alert faultet nie (5.6): {trace}");
}

#[test]
fn confirmation_time_delays_the_check() {
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output x     : int                      @ hw(\"o/x\") with safe = 0

machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            check p < 50 bar, \"druck\" for 3 ms
    state SAFE:
        loop: pass
";
    let stim = "t=0 in p 10 bar\nt=2 in p 80 bar\n";
    let trace = simulate(body, stim, 8);
    // Erst nach 3 ms ununterbrochener Verletzung (5.6).
    assert!(!trace.contains("t=2 fault"), "nicht sofort: {trace}");
    assert!(!trace.contains("t=3 fault"), "nicht nach 1 ms: {trace}");
    assert!(trace.contains("t=4 fault m CheckFailed"), "nach 3 ms: {trace}");
}

#[test]
fn faulted_has_no_user_code_and_sets_outputs_safe() {
    let body = "\
output valve : bool @ hw(\"plc/do0\") with safe = false

machine m:
    initial RUN
    state RUN:
        enter:
            valve = true
        loop:
            check false, \"immer\"
";
    let trace = simulate(body, "", 3);
    // Ohne `fault ->` faultet die Maschine nach FAULTED (5.3).
    assert!(trace.contains("state m FAULTED\n"), "{trace}");
    assert!(trace.contains("out valve false\n"), "Outputs auf safe: {trace}");
    let after = trace.split("state m FAULTED").nth(1).unwrap_or_default();
    assert!(!after.contains("fault m"), "in FAULTED laeuft kein Nutzercode: {trace}");
}

#[test]
fn order_of_steps_does_not_change_the_trace() {
    let body = "\
output a : int @ hw(\"o/a\") with safe = 0
output b : int @ hw(\"o/b\") with safe = 0
output c : int @ hw(\"o/c\") with safe = 0

machine one:
    pub var n : int = 0
    initial RUN
    state RUN:
        loop:
            n = n + 1
            a = n

machine two:
    pub var n : int = 0
    initial RUN2
    state RUN2:
        loop:
            n = n + 2
            b = n

machine three:
    initial RUN3
    state RUN3:
        loop:
            c = one.n + two.n
";
    let program = compile(body);
    let stim = Trace::parse("").expect("leer");
    let base = run(&program, &stim, &RunOptions { ticks: 5, ..Default::default() }).expect("Lauf").trace.render();
    // Satz 9.4.1: die Schrittreihenfolge ist semantisch irrelevant.
    for seed in [1u64, 7, 42, 1234] {
        let permuted = run(&program, &stim, &RunOptions { ticks: 5, order_seed: Some(seed), ..Default::default() })
            .expect("Lauf")
            .trace
            .render();
        assert_eq!(permuted, base, "Reihenfolge {seed} aendert den Trace");
    }
}

#[test]
fn trace_round_trips_through_its_format() {
    let text = "\
t=0 in tank_p 45 bar
t=0 in ready true
t=1 in tank_p bad reason=OutOfRange
t=2 in lox_temp 90 K stale age=120 ms
t=3 cmd start
t=4 abort
t=5 out fuel_main CLOSED
t=5 state hotfire ARMED.IDLE
t=6 log hotfire \"starting hotfire\"
t=6 pub battery_cycle count 3
t=6 signal hotfire done
t=7 alert hotfire on \"LOX warming\"
t=7 measure hotfire boot_time 1500 ms
t=8 verify hotfire fail \"supply not off\"
t=8 fault hotfire Timeout \"no pressure\" -> SAFE
t=9 verdict hotfire pass \"recovery ok\"
t=9 verdict-final FAIL
";
    let trace = Trace::parse(text).expect("lesbar");
    assert_eq!(trace.render(), text, "Roundtrip zeichengleich");
}

#[test]
fn trace_vectors_from_the_specification() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/trace.md");
    let spec = std::fs::read_to_string(path).expect("grammar/trace.md lesbar");
    let mut blocks = 0;
    let mut lines = spec.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "```trace" {
            continue;
        }
        let mut text = String::new();
        for body in lines.by_ref() {
            if body.trim() == "```" {
                break;
            }
            text.push_str(body);
            text.push('\n');
        }
        let trace = Trace::parse(&text).unwrap_or_else(|e| panic!("Vektor nicht lesbar: {e}\n{text}"));
        assert_eq!(trace.render(), text, "Vektor nicht zeichengleich");
        blocks += 1;
    }
    assert!(blocks >= 3, "Vektoren gefunden: {blocks}");
}
