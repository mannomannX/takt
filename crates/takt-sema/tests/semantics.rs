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
            alert p > 50 bar, \"hoch\"
";
    // Der `sim`-Output speist den Input in jedem Tick, in dem der Stimulus
    // schweigt (8.3); der Stimulus setzt ihn darum in jedem Tick.
    let stim = "t=0 in p 10 bar\nt=1 in p 10 bar\nt=2 in p 80 bar\nt=3 in p 80 bar\n\
                t=4 in p 20 bar\nt=5 in p 20 bar\n";
    let trace = simulate(body, stim, 5);
    // Die Bedingung eines Alerts nennt das Ereignis (5.6): er meldet, waehrend
    // der Druck hoch ist.
    assert!(trace.contains("t=2 alert m on \"hoch\"\n"), "steigende Flanke: {trace}");
    assert!(trace.contains("t=4 alert m off \"hoch\"\n"), "fallende Flanke: {trace}");
    assert_eq!(trace.matches("alert m on \"hoch\"").count(), 1, "genau eine steigende Flanke: {trace}");
    assert_eq!(trace.matches("alert m off \"hoch\"").count(), 1, "genau eine fallende Flanke: {trace}");
    assert!(!trace.contains("fault"), "ein Alert faultet nie (5.6): {trace}");
}

#[test]
fn an_invalid_input_makes_an_alert_fire_with_a_note() {
    // 3.5: Beobachtung schweigt nicht, wenn ihr die Grundlage fehlt; der
    // Alert feuert mit Zusatz, ohne die Steuerung zu beeinflussen.
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output x     : int                      @ hw(\"o/x\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            alert p > 50 bar, \"hoch\"
";
    let stim = "t=0 in p 10 bar\nt=1 in p 10 bar\nt=2 in p bad reason=Driver\n\
                t=3 in p bad reason=Driver\n";
    let trace = simulate(body, stim, 3);
    assert!(trace.contains("t=2 alert m on \"hoch (sensor invalid)\"\n"), "{trace}");
    assert!(!trace.contains("fault"), "ein Alert faultet nie (5.6): {trace}");
}

#[test]
fn an_alert_in_a_loop_reports_every_element() {
    // 5.6: eine Alert-Stelle in einer Schleife hat je Durchlauf eine eigene
    // Flanke; sonst meldete nur das erste betroffene Element.
    let body = "\
output n : int @ hw(\"o/n\") with safe = 0
command quiet

machine m:
    var limit : int in 0..8 = 8
    initial RUN
    state RUN:
        loop:
            if quiet:
                limit = 0
            for i in range(4):
                alert i < limit, \"element {i}\"
";
    let trace = simulate(body, "t=2 cmd quiet\n", 4);
    // In den ersten Ticks trifft die Bedingung fuer alle vier Durchlaeufe zu:
    // vier steigende Flanken an derselben Stelle, je Schleifenindex eine.
    for i in 0..4 {
        assert!(trace.contains(&format!("t=0 alert m on \"element {i}\"\n")), "Element {i} fehlt: {trace}");
    }
    // Danach trifft sie fuer keinen mehr zu: vier fallende Flanken.
    for i in 0..4 {
        assert!(trace.contains(&format!("t=2 alert m off \"element {i}\"\n")), "Element {i} ohne Flanke: {trace}");
    }
    assert_eq!(trace.matches("alert m on").count(), 4, "genau vier steigende Flanken: {trace}");
    assert_eq!(trace.matches("alert m off").count(), 4, "genau vier fallende Flanken: {trace}");
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
    // Wie oben: ohne Stimuluszeile uebernimmt die `sim`-Bindung (8.3).
    let stim = "t=0 in p 10 bar\nt=1 in p 10 bar\nt=2 in p 80 bar\nt=3 in p 80 bar\n\
                t=4 in p 80 bar\nt=5 in p 80 bar\nt=6 in p 80 bar\nt=7 in p 80 bar\n\
                t=8 in p 80 bar\n";
    let trace = simulate(body, stim, 8);
    // Erst nach 3 ms ununterbrochener Verletzung (5.6).
    assert!(!trace.contains("t=2 fault"), "nicht sofort: {trace}");
    assert!(!trace.contains("t=3 fault"), "nicht nach 1 ms: {trace}");
    assert!(trace.contains("t=4 fault m CheckFailed"), "nach 3 ms: {trace}");
}

#[test]
fn a_confirmation_counter_in_a_loop_counts_per_element() {
    // 5.6: eine Stelle in einer Schleife zaehlt je Durchlauf getrennt. Mit
    // einem gemeinsamen Zaehler setzten die erfuellten Durchlaeufe den der
    // verletzten zurueck, und die Auslesever\u00f6gerung entschaerfte den Check
    // dauerhaft.
    let body = "\
input  tcs     : [4] float[degC] in 0..1000 degC @ hw(\"d/tc[0:4]\")
output tcs_sim : [4] float[degC]                 @ sim(\"d/tc[0:4]\")
output x       : int                             @ hw(\"o/x\") with safe = 0

machine guard:
    fault -> SAFE
    initial WATCH
    state WATCH:
        loop:
            for i in range(4):
                check tcs[i] < 500 degC, \"TC {i} heiss\" for 5 ms
    state SAFE:
        loop: pass

machine model:
    initial RUN
    state RUN:
        loop:
            for i in range(4):
                tcs_sim[i] = 900 degC if i == 2 else 20 degC
";
    let trace = simulate(body, "", 12);
    // Genau ein Element verletzt dauerhaft: der Fault kommt nach 5 ms.
    assert!(!trace.contains("t=4 fault"), "nicht vor der Bestaetigungszeit: {trace}");
    assert!(trace.contains("t=5 fault guard CheckFailed \"TC 2 heiss\""), "{trace}");
}

#[test]
fn an_every_block_in_a_loop_runs_for_each_element() {
    // Derselbe Schluessel gilt fuer `every` (5.8): ohne Trennung liefe nur
    // der erste Durchlauf je Periode.
    let body = "\
output n : int in 0..1000 @ hw(\"o/n\") with safe = 0

machine m:
    var count : int in 0..1000 = 0
    initial RUN
    state RUN:
        loop:
            for i in range(4):
                every 2 ms:
                    count = count + 1
            n = count
";
    let trace = simulate(body, "", 6);
    // Der Zaehler beginnt bei `d` (5.8), also laeuft der Block erstmals nach
    // einer vollen Periode: vier Durchlaeufe bei t=2, t=4 und t=6.
    assert!(!trace.contains("t=0 out n 4"), "nicht im Eintritts-Tick: {trace}");
    assert!(trace.contains("t=2 out n 4\n"), "erste Periode: {trace}");
    assert!(trace.contains("t=4 out n 8\n"), "zweite Periode: {trace}");
    assert!(trace.contains("t=6 out n 12\n"), "dritte Periode: {trace}");
}

#[test]
fn a_value_on_the_declared_boundary_stays_in_range_with_float32() {
    // 3.4: die Range ist eine Invariante ueber dem deklarierten Typ. Die
    // Grenzen stehen als f64 in der MIR; ohne Rundung auf f32 waere die
    // Obergrenze in `float = f32` nie erreichbar und ein Wert genau darauf
    // faultete.
    let src = "\
system:
    language = 1
    tick = 1 ms
    float = f32

output y : float[bar] in 0..0.1 bar @ hw(\"o/y\") with safe = 0 bar

machine m:
    initial RUN
    state RUN:
        loop:
            y = 0.1 bar
";
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let program = out.program.expect("Programm");
    let stim = Trace::parse("").expect("leer");
    let result = run(&program, &stim, &RunOptions { ticks: 3, ..Default::default() }).expect("Lauf");
    let trace = result.trace.render();
    assert!(!trace.contains("fault"), "Wert auf der Grenze faultet: {trace}");
    assert!(trace.contains("out y 0.1 bar\n"), "{trace}");
}

#[test]
fn the_driver_edge_enforces_declared_channel_ranges() {
    // 3.5 und 12.6: ein Wert ausserhalb der deklarierten Range wird `Bad` mit
    // Grund `OutOfRange`, nicht geklemmt und nicht zu einem Fault. Ohne diese
    // Durchsetzung waeren die deklarierten Ranges keine gueltigen Annahmen
    // der Intervallanalyse (3.4). Die Simulation ist der Treiberrand.
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output used  : float[bar]               @ hw(\"o/used\") with safe = 0 bar
output ok    : bool                     @ hw(\"o/ok\")   with safe = false
output grund : Reason?                  @ hw(\"o/g\")    with safe = none

machine m:
    initial RUN
    state RUN:
        loop:
            used = p.or(7 bar)
            ok = p.valid
            grund = p.reason
";
    let stim = "t=1 in p 30 bar\nt=2 in p 250 bar\nt=3 in p 40 bar\n";
    let trace = simulate(body, stim, 4);
    assert!(trace.contains("t=1 out used 30.0 bar\n"), "gueltiger Wert: {trace}");
    assert!(trace.contains("t=2 out used 7.0 bar\n"), "Wert ausserhalb wird Bad: {trace}");
    assert!(trace.contains("t=2 out ok false\n"), "Abtastung ungueltig: {trace}");
    assert!(trace.contains("t=2 out grund OUT_OF_RANGE\n"), "Grund OutOfRange: {trace}");
    assert!(trace.contains("t=3 out used 40.0 bar\n"), "danach wieder gueltig: {trace}");
    assert!(!trace.contains("fault"), "kein Fault am Rand (3.5): {trace}");
}

#[test]
fn a_self_transition_leaves_and_reenters_the_state() {
    // 9.3: der kleinste gemeinsame Vorfahr liegt echt oberhalb des Ziels.
    // Ohne das war `-> S` aus `S` wirkungslos: kein exit, kein enter, keine
    // frischen zustandslokalen Variablen, und der Timer lief weiter, sodass
    // der ausloesende `after`-Trigger danach nie wieder feuerte.
    let body = "\
output n : int in 0..100 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        var count : int in 0..100 = 0
        enter:
            log \"enter\"
        loop:
            count = count + 1
            n = count
        after 3 ms: -> RUN
        exit:
            log \"exit\"
";
    let trace = simulate(body, "", 7);
    assert!(trace.contains("t=3 log m \"exit\"\n"), "exit beim Wechsel: {trace}");
    assert!(trace.contains("t=3 log m \"enter\"\n"), "enter beim Wechsel: {trace}");
    assert!(trace.contains("t=3 out n 1\n"), "zustandslokale Variable neu: {trace}");
    // Der Timer ist zurueckgesetzt, der Trigger feuert periodisch.
    assert!(trace.contains("t=6 log m \"enter\"\n"), "zweiter Wechsel: {trace}");
}

#[test]
fn an_operator_abort_reaches_an_inactive_machine_in_the_same_tick() {
    // 5.4: „ob in diesem Tick aktiv oder nicht"; die Latenz ist unabhaengig
    // von den Maschinenperioden. Vorher wartete der Abort auf die naechste
    // Aktivierung der Maschine.
    let body = "\
output v : bool @ hw(\"o/v\") with safe = false

machine slow every 10 ms:
    initial RUN
    state RUN:
        enter:
            v = true
        loop: pass
";
    let trace = simulate(body, "t=3 abort\n", 14);
    assert!(trace.contains("t=3 fault slow Abort"), "im Tick des Aborts: {trace}");
    assert!(trace.contains("t=3 out v false\n"), "Output beim Commit sicher: {trace}");
}

#[test]
fn a_suppressed_abort_is_discarded_not_stored() {
    // 5.4: „ignoriert weitere Aborts". Wurde der unterdrueckte Abort
    // aufbewahrt, feuerte er nach der naechsten normalen Transition ohne
    // neue Operator-Eingabe erneut.
    let body = "\
output v : bool @ hw(\"o/v\") with safe = false
command reset

machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        enter:
            v = true
        loop: pass
    state SAFE:
        when reset: -> RUN
";
    let trace = simulate(body, "t=1 abort\nt=2 abort\nt=5 cmd reset\n", 9);
    assert!(trace.contains("t=1 fault m Abort"), "erster Abort: {trace}");
    assert_eq!(trace.matches("fault m Abort").count(), 1, "genau ein Abort: {trace}");
    assert!(trace.contains("t=5 state m RUN\n"), "Rueckkehr nach RUN: {trace}");
    // Nach der Rueckkehr bleibt die Maschine dort; kein alter Abort feuert.
    let after = trace.split("t=5 state m RUN").nth(1).unwrap_or_default();
    assert!(!after.contains("Abort"), "alter Abort feuert erneut: {trace}");
}

#[test]
fn an_every_in_a_state_never_runs_in_the_entry_tick() {
    // 5.8: der Zaehler beginnt bei `d`. Mit 0 lief der Block schon im
    // Eintritts-Tick, und ein Zustand, der kuerzer als `d` aktiv ist, fuehrte
    // ihn bei jedem Eintritt aus statt nie.
    let body = "\
output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial A
    state A:
        loop:
            every 4 ms:
                log \"beat\"
        after 2 ms: -> B
    state B:
        after 2 ms: -> A
";
    let trace = simulate(body, "", 16);
    // A ist nie laenger als 2 ms aktiv, also darf `every 4 ms` nie feuern.
    assert!(!trace.contains("beat"), "feuert bei jedem Eintritt: {trace}");
}

#[test]
fn a_machine_wide_every_keeps_its_period_across_state_changes() {
    // 5.8: eine Stelle im maschinenweiten `loop:` misst gegen `now`. Mit
    // `time_in_state` verstummte sie, sobald die Maschine oefter wechselte
    // als die Periode lang ist.
    let body = "\
output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial A
    loop:
        every 2 ms:
            log \"mbeat\"
    state A:
        enter:
            v = true
        after 3 ms: -> B
    state B:
        enter:
            v = false
        after 3 ms: -> A
";
    let trace = simulate(body, "", 14);
    for t in [2, 4, 6, 8, 10, 12, 14] {
        assert!(trace.contains(&format!("t={t} log m \"mbeat\"\n")), "Tick {t} fehlt: {trace}");
    }
}

#[test]
fn a_fresh_sample_is_read_with_age_zero() {
    // 9.4: `I_k = sample()` steht am Tick-Anfang, das Alter einer frischen
    // Lieferung ist das ihres Treibers. Lag die Alterung hinter dem Stimulus,
    // war jede Abtastung beim ersten Lesen schon einen Tick alt, und
    // `max_age` wirkte um einen Tick kuerzer als deklariert.
    let body = "\
input  p  : float[bar] in 0..100 bar @ hw(\"d/p\") with max_age = 2 ms
output a  : Duration                 @ hw(\"o/a\")  with safe = 0 ms
output st : bool                     @ hw(\"o/st\") with safe = false

machine m:
    initial S
    state S:
        loop:
            a = p.age
            st = p.stale
";
    let trace = simulate(body, "t=0 in p 1 bar\n", 5);
    assert!(trace.contains("t=0 out a 0 ns\n"), "frische Abtastung: {trace}");
    assert!(trace.contains("t=1 out a 1 ms\n"), "{trace}");
    // Stale, sobald das Alter `max_age` ueberschreitet.
    assert!(trace.contains("t=3 out st true\n"), "{trace}");
}

#[test]
fn a_suspect_sample_also_becomes_stale() {
    // 3.5 formuliert Stale unbedingt ueber das Alter. Ein entprellter Kanal,
    // dessen Treiber danach ausfaellt, blieb sonst dauerhaft gueltig und
    // hielt seinen letzten Wert unbegrenzt.
    let body = "\
input  p  : float[bar] in 0..100 bar @ hw(\"d/p\") with max_age = 2 ms
output st : bool                     @ hw(\"o/st\") with safe = false
output ok : bool                     @ hw(\"o/ok\") with safe = false

machine m:
    initial S
    state S:
        loop:
            st = p.stale
            ok = p.valid
";
    let trace = simulate(body, "t=0 in p suspect 5 bar\n", 5);
    assert!(trace.contains("t=3 out st true\n"), "Suspect altert ebenfalls: {trace}");
    assert!(trace.contains("t=3 out ok false\n"), "danach ungueltig: {trace}");
}

#[test]
fn max_age_defaults_to_twice_the_fastest_reader_period() {
    // 3.5: ohne Angabe ist der Default das Doppelte der kuerzesten Periode
    // unter den Lesern. Ohne Default wurde ein toter Sensor nie `Stale`.
    let body = "\
input  p  : float[bar] in 0..100 bar @ hw(\"d/p\")
output st : bool                     @ hw(\"o/st\") with safe = false

machine m every 3 ms:
    initial S
    state S:
        loop:
            st = p.stale
";
    let trace = simulate(body, "t=0 in p 1 bar\n", 12);
    // Default 6 ms: bei t=9 ist das Alter 9 ms und damit darueber.
    assert!(!trace.contains("t=6 out st true"), "nicht vor der Grenze: {trace}");
    assert!(trace.contains("t=9 out st true\n"), "{trace}");
}

#[test]
fn an_int_to_float32_conversion_rounds_exactly_once() {
    // 4.1 und Satz 9.4.4: das Ziel emittiert `sitofp`, also eine Rundung.
    // Der Umweg ueber f64 rundete zweimal und lieferte fuer Werte ueber 2^53
    // ein anderes Ergebnis als die uebersetzte Fassung.
    let src = "\
system:
    language = 1
    tick = 1 ms
    float = f32

output v : bool @ hw(\"o/v\") with safe = false

machine m:
    var a : int = 9007199791611905
    initial S
    state S:
        loop:
            log \"{a as float}\"
";
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let program = out.program.expect("Programm");
    let stim = Trace::parse("").expect("leer");
    let result = run(&program, &stim, &RunOptions { ticks: 1, ..Default::default() }).expect("Lauf");
    let trace = result.trace.render();
    // 9007199791611905 as f32 == 9007200328482816, kuerzeste Darstellung
    // 9007200000000000; doppelt gerundet waere es 9007199254740992.
    assert!(trace.contains("\"9007200000000000\""), "{trace}");
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
t=9 out dut_tx [0x50, 0x49]
t=9 stream dut_log dropped=2 overflowed=0 malformed=1
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

#[test]
fn a_byte_can_be_written_through_its_index() {
    // 3.9: `b[i] = x` schreibt an eine belegte Stelle, ohne die Laenge zu
    // aendern. Backpatching (Laengenpraefix, CRC am Ende) braucht genau das.
    let trace = simulate(
        "\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0
output l : int in 0..99 @ hw(\"o/l\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var b : bytes<8> = default
            b.push(1)
            b.push(2)
            b[0] = 7
            n = b[0] as int
            l = b.len as int
",
        "",
        2,
    );
    assert!(trace.contains("t=0 out n 7\n"), "das Byte ist geschrieben: {trace}");
    assert!(trace.contains("t=0 out l 2\n"), "die Laenge bleibt: {trace}");
}

#[test]
fn writing_a_byte_beyond_the_length_faults() {
    // 3.9: derselbe Range-Check wie beim Lesen — ein Index jenseits von `len`
    // ist ein `RangeFault`, kein stiller Anhang.
    let trace = simulate(
        "\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var b : bytes<8> = default
            b.push(1)
            b[5] = 7
            n = 1
",
        "",
        2,
    );
    assert!(trace.contains("fault m RangeFault"), "Index jenseits der Laenge: {trace}");
}

