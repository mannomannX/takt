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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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
fn an_invalid_input_is_logged_measured_and_counted_as_a_violation() {
    // 3.5: `log` und `measure` schreiben `<invalid>`, `verify` zaehlt eine
    // Verletzung — keine der Beobachtungen faultet, auch ueber eine
    // Funktion hinweg nicht.
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output x     : int                      @ hw(\"o/x\") with safe = 0

fn scaled(v: float[bar]) -> float:
    return v / (1 bar)

machine m:
    initial RUN
    state RUN:
        loop:
            log \"p {p}\"
            measure level = scaled(p)
            verify p < 50 bar, \"unter der Grenze\"
            x = 1
";
    let stim = "t=0 in p 10 bar\nt=1 in p bad reason=Driver\n";
    let trace = simulate(body, stim, 1);
    assert!(trace.contains("t=0 measure m level 10.0\n"), "{trace}");
    assert!(trace.contains("t=1 log m \"p <invalid>\"\n"), "{trace}");
    assert!(trace.contains("t=1 measure m level <invalid>\n"), "{trace}");
    assert!(trace.contains("t=1 verify m fail \"unter der Grenze\"\n"), "{trace}");
    assert!(!trace.contains("fault"), "eine Beobachtung faultet nie (5.6): {trace}");
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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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
fn a_driver_suspect_keeps_its_value_when_the_edge_lets_it_pass() {
    // 3.5, „Qualitaet vom Treiber“: Der Rand prueft den Wert einer Lieferung
    // `Suspect` wie jeden. Besteht er, ist die Abtastung `Suspect` mit dem
    // Wert des Treibers, lesbar; besteht er nicht, gilt die Range wie sonst.
    // `Suspect` ohne Wert bricht den Vertrag (12.6 Zeile 2).
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output used  : float[bar]               @ hw(\"o/used\") with safe = 0 bar
output doubt : bool                     @ hw(\"o/doubt\") with safe = false
output ok    : bool                     @ hw(\"o/ok\")   with safe = false

machine m:
    initial RUN
    state RUN:
        loop:
            used = p.or(7 bar)
            doubt = p.suspect
            ok = p.valid
";
    let stim = "t=1 in p 30 bar\nt=2 in p 35 bar suspect\nt=3 in p 250 bar suspect\nt=4 in p suspect\n";
    let trace = simulate(body, stim, 5);
    assert!(trace.contains("t=2 out used 35.0 bar\n"), "der Wert des Treibers: {trace}");
    assert!(trace.contains("t=2 out doubt true\n"), "`.suspect` ist wahr: {trace}");
    assert!(!trace.contains("t=2 out ok false\n"), "`Suspect` ist gueltig: {trace}");
    assert!(trace.contains("t=3 out used 7.0 bar\n"), "ausserhalb der Range `Bad`: {trace}");
    assert!(trace.contains("t=4 driver d degraded flags\n"), "ohne Wert ein Vertragsbruch: {trace}");
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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let program = out.program.expect("Programm");
    let stim = Trace::parse("").expect("leer");
    let result = run(&program, &stim, &RunOptions { ticks: 1, ..Default::default() }).expect("Lauf");
    let trace = result.trace.render();
    // 9007199791611905 as f32 == 9007200328482816, kuerzeste Darstellung
    // 9007200000000000, ab 1e7 in Exponentform (3.9); doppelt gerundet
    // waere es 9007199254740992 (`9.007199e15`).
    assert!(trace.contains("\"9.0072e15\""), "{trace}");
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

#[test]
fn a_declaration_can_take_the_result_of_a_mutating_method() {
    // 3.9, 4.4: `push` steht als Statement, sein Ergebnis nimmt eine Stelle
    // entgegen — auch die gerade deklarierte Variable.
    let trace = simulate(
        "\
output a : bool @ hw(\"o/a\") with safe = true
output b : bool @ hw(\"o/b\") with safe = true

machine m:
    initial RUN
    state RUN:
        loop:
            var buf : bytes<2> = default
            var fits = buf.push(1)
            var also = buf.push(2)
            var full = buf.push(3)
            a = also
            b = full
",
        "",
        2,
    );
    assert!(trace.contains("t=0 out a true\n"), "der zweite passt: {trace}");
    assert!(trace.contains("t=0 out b false\n"), "der dritte nicht mehr: {trace}");
}

#[test]
fn a_declaration_takes_the_result_of_a_block_step() {
    // 5.7: dasselbe fuer `step` einer Blockinstanz.
    let trace = simulate(
        "\
block acc():
    var s : int in 0..99 = 0
    step(x: int in 0..9) -> int:
        s = s + x
        return s

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var inst = acc()
    initial RUN
    state RUN:
        loop:
            var total = inst.step(3)
            n = total
",
        "",
        3,
    );
    assert!(trace.contains("t=0 out n 3\n"), "erster Schritt: {trace}");
    assert!(trace.contains("t=1 out n 6\n"), "der Block behaelt seinen Zustand: {trace}");
}

#[test]
fn a_mutating_method_stays_out_of_expressions() {
    // 4.4: verschachtelt waere die Auswertungsreihenfolge sichtbar.
    let out = errors_of(
        "\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var b : bytes<4> = default
            if b.push(1):
                n = 1
",
    );
    assert!(out.contains("nur als Anweisung"), "verschachtelt abgelehnt: {out}");
}

#[test]
fn a_declaration_rejects_a_method_without_a_result() {
    let out = errors_of(
        "\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var b : bytes<4> = default
            var x = b.clear()
            n = 1
",
    );
    assert!(out.contains("liefert keinen Wert"), "`clear` hat kein Ergebnis: {out}");
}

/// Fehlermeldungen eines Programms als Text.
fn errors_of(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_prefixed_rate_counts_in_hertz() {
    let body = "\
output tx  : stream<u8>   @ hw(\"o/tx\")  with max_rate = 2 kHz, capacity = 16
output got : int in 0..16 @ hw(\"o/got\") with safe = 0
command go

machine dut:
    var msg : bytes<4> = default
    initial IDLE
    state IDLE:
        enter:
            msg.push(1)
            msg.push(2)
            msg.push(3)
        when go:
            send tx, msg
            -> SENT
    state SENT:
        loop:
            got = tx.sent.or(default).len
";
    let trace = simulate(body, "t=1 cmd go\n", 4);
    assert!(trace.contains("t=2 out got 2\n"), "2 kHz sind 2 Byte je Millisekunde (8.8, FB-188): {trace}");
}

/// 12.10: Ein Strom an `mmio/ADR/r` schaltet je Lesen weiter — ein
/// Datenregister, das eine FIFO leert; leer liest der Port das letzte
/// Element, vor dem ersten den Default.
#[test]
fn a_stream_at_the_read_address_advances_per_port_read() {
    let trace = simulate(
        "\
record Data layout little:
    b : u8

port dr : Data @ mmio(0x40001000)

output regs : stream<Data> @ sim(\"mmio/0x40001000/r\") with capacity = 64
output sum  : int @ hw(\"o/sum\") with safe = 0

machine model:
    var k : int in 0..255 = 1
    initial RUN
    state RUN:
        loop:
            send regs, Data(b = k as u8)
            send regs, Data(b = (k + 1) as u8)
            k = (k + 2) % 200

driver machine pump:
    initial RUN
    state RUN:
        loop:
            var s : int = 0
            for _i in range(3):
                s = s + (dr.b as int)
            sum = s
",
        "",
        3,
    );
    assert!(trace.contains("t=0 out sum 0\n"), "vor dem ersten Element der Default:\n{trace}");
    assert!(trace.contains("t=1 out sum 5\n"), "1, 2 und dann wieder 2:\n{trace}");
    assert!(trace.contains("t=2 out sum 11\n"), "3, 4 und wieder 4:\n{trace}");
}

#[test]
fn every_fault_kind_of_the_corpus_arrives_at_its_tick() {
    // Korpus 87: je Maschine ein Fault, den kein anderes Programm ausloest.
    // Das Differential sieht nur die Outputs; die Art steht im Trace des
    // Interpreters (5.3, 7.5, 9.8, FB-324, FB-325).
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/87_fault_kinds.takt");
    let src = std::fs::read_to_string(path).expect("Korpus 87");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let program = takt_sema::compile(&src, &options).program.expect("Programm");
    let trace = run(&program, &Trace::default(), &RunOptions { ticks: 30, ..Default::default() }).expect("Lauf");
    let trace = trace.trace.render();
    for line in [
        "t=1 fault invert Arithmetic(Singular)",
        "t=2 fault timing TimingFault",
        "t=2 fault crowd ScheduleOverflow",
        "t=2 fault divide Arithmetic(DivZero)",
        "t=2 fault grow Arithmetic(Overflow)",
        "t=3 out planned 2\n",
        "t=4 job restart v done start=1\n",
    ] {
        assert!(trace.contains(line), "`{line}` fehlt:\n{trace}");
    }
    // Derselbe Zeitpunkt in der vollen Warteschlange ersetzt, statt zu
    // faulten; der Fault leert sie, kein geplanter Wert ueberschreibt `safe`.
    assert!(!trace.contains("t=1 fault crowd"), "{trace}");
    assert!(trace.lines().filter(|l| l.contains(" out crowded ")).all(|l| l.ends_with(" 0")), "{trace}");
}

#[test]
fn a_state_entered_without_its_permissive_faults_before_the_commit() {
    // 5.6: Der Guard einer `when`-Transition ist das Permissive. Wer den
    // Zustand ohne ihn betritt — hier `force`, das den Druck nicht
    // prueft —, den faengt der Check des Ziels im Eintrittstick: Der Fault
    // kommt im selben Tick, das Fault-Ziel setzt das Ventil, und `valve =
    // true` aus `enter:` erreicht den Commit nie (5.2, 9.3).
    let body = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
output valve : bool                     @ hw(\"plc/do0\") with safe = false
command open
command force

machine m:
    fault -> SAFE
    initial CLOSED
    state CLOSED:
        when open and p < 5 bar: -> OPEN
        when force: -> OPEN
    state OPEN:
        enter:
            valve = true
        loop:
            check p < 5 bar, \"Druck zu hoch\"
    state SAFE:
        enter:
            valve = false
        loop: pass
";
    let stim = "t=0 in p 10 bar\nt=1 in p 10 bar\nt=1 cmd open\nt=2 in p 10 bar\nt=2 cmd force\n";
    let trace = simulate(body, stim, 4);
    assert!(!trace.contains("t=1 state m OPEN"), "das Permissive haelt: {trace}");
    assert!(trace.contains("t=2 fault m CheckFailed \"Druck zu hoch\" -> SAFE\n"), "{trace}");
    assert!(trace.contains("t=2 state m SAFE\n"), "{trace}");
    assert!(!trace.contains("out valve true"), "kein unsicherer Commit: {trace}");
}

/// **Hinter einem `expect` laeuft nur, was die Erwartung voraussetzt**
/// (6.2, FB-283): Scheitert sie im Eintrittstick, wird die Zuweisung
/// dahinter nicht committet — der Output bleibt `safe`, auch ohne dass das
/// Fault-Ziel ihn neu setzt. Gilt sie, laeuft die Zuweisung im selben Tick.
#[test]
fn an_assignment_after_a_failed_expect_is_not_committed() {
    let body = "\
input  ready     : bool @ hw(\"d/ready\")
output ready_sim : bool @ sim(\"d/ready\")
output valve     : bool @ hw(\"plc/do0\") with safe = false

machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        sequence:
            expect ready, \"nicht bereit\"
            valve = true
            wait 5 ms
    state SAFE:
        loop: pass
";
    let failed = simulate(body, "t=0 in ready false\n", 3);
    assert!(failed.contains("t=0 fault m Expect \"nicht bereit\" -> SAFE\n"), "{failed}");
    assert!(!failed.contains("out valve true"), "die Zuweisung lief trotz gescheiterter Erwartung: {failed}");
    let held = simulate(body, "t=0 in ready true\n", 3);
    assert!(held.contains("t=0 out valve true\n"), "im Eintrittstick: {held}");
}

/// **Ein Fault in `exit:` beginnt beim kleinsten gemeinsamen Vorfahren**
/// (9.3, FB-289): Das Ziel des Wechsels ist noch nicht betreten, sein
/// `exit:` laeuft nicht, und das Fault-Ziel ist das des Vorfahren.
#[test]
fn a_fault_in_exit_is_handled_from_the_common_ancestor() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/99_exit_fault.takt"))
        .expect("Quelle");
    let options = Options { policy: Policy::default(), build: Build::Sim, ..Default::default() };
    let program = takt_sema::compile(&src, &options).program.expect("Programm");
    let trace =
        run(&program, &Trace::default(), &RunOptions { ticks: 10, ..Default::default() }).expect("Lauf").trace.render();
    assert!(trace.contains("t=2 fault m RangeFault"), "{trace}");
    assert!(trace.contains("-> RECOVER\n"), "Fault-Ziel von WORK, nicht SAFE: {trace}");
    assert!(!trace.contains("out mark 42"), "`exit:` des nie betretenen Ziels lief: {trace}");
}

/// Die Zeilen von `trace`, die mit `t=<tick> ` beginnen, ohne Verdikt.
fn lines_of(trace: &str, tick: u64) -> Vec<&str> {
    let prefix = format!("t={tick} ");
    trace.lines().filter(|l| l.starts_with(&prefix) && !l.contains("verdict-final")).collect()
}

/// 5.2 Schritt 2: Outer-first — die Transition des Elternzustands geht der
/// des Kindes vor, beide sind im selben Tick wahr.
#[test]
fn the_outer_transition_wins_over_the_inner() {
    let body = "\
command go
output o : int in 0..9 @ hw(\"o/o\") with safe = 0
machine m:
    initial P
    state P:
        initial C
        when go: -> Q
        state C:
            when go: -> D
        state D:
            enter:
                o = 1
    state Q:
        enter:
            o = 2
";
    let trace = simulate(body, "t=2 cmd go\n", 4);
    assert_eq!(lines_of(&trace, 2), ["t=2 state m Q", "t=2 out o 2"], "{trace}");
}

/// 5.3: Ein Zustand ohne eigenes Fault-Ziel erbt das seines Elternzustands.
#[test]
fn a_child_inherits_the_fault_target_of_its_parent() {
    let body = "\
output o : int in 0..9 @ hw(\"o/o\") with safe = 0
machine m:
    initial P
    state P:
        fault -> SAFE
        initial C
        state C:
            loop:
                check time_in_state < 2 ms, \"c\"
    state SAFE:
        enter:
            o = 3
";
    let trace = simulate(body, "", 4);
    assert_eq!(
        lines_of(&trace, 2),
        ["t=2 fault m CheckFailed \"c\" -> SAFE", "t=2 state m SAFE", "t=2 out o 3"],
        "{trace}"
    );
}

/// 5.3: `check … -> X` ueberschreibt das Fault-Ziel fuer genau diesen Check.
#[test]
fn a_check_with_its_own_target_goes_there() {
    let body = "\
output o : int in 0..9 @ hw(\"o/o\") with safe = 0
machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            check time_in_state < 2 ms, \"eigen\" -> OWN
    state OWN:
        enter:
            o = 4
    state SAFE:
        enter:
            o = 5
";
    let trace = simulate(body, "", 4);
    assert_eq!(
        lines_of(&trace, 2),
        ["t=2 fault m CheckFailed \"eigen\" -> OWN", "t=2 state m OWN", "t=2 out o 4"],
        "{trace}"
    );
}

/// 5.2 Schritt 4: Im Entry-Modus ist `-> ZIEL` wirkungslos; erst der
/// naechste Tick nimmt den Uebergang.
#[test]
fn a_goto_in_the_entry_tick_has_no_effect() {
    let body = "\
command go
output o : int in 0..9 @ hw(\"o/o\") with safe = 0
machine m:
    initial A
    state A:
        when go: -> B
    state B:
        enter:
            o = 1
        loop:
            -> C
    state C:
        enter:
            o = 2
";
    let trace = simulate(body, "t=1 cmd go\n", 4);
    assert_eq!(lines_of(&trace, 1), ["t=1 state m B", "t=1 out o 1"], "{trace}");
    assert_eq!(lines_of(&trace, 2), ["t=2 state m C", "t=2 out o 2"], "{trace}");
}

/// 5.2 Schritt 5, 5.3: In `FAULTED` sind Operator-Abort und Runtime-Fault
/// wirkungslos — die Outputs stehen schon auf `safe`.
#[test]
fn abort_and_runtime_faults_do_nothing_in_faulted() {
    let body = "\
output o : int in 0..9 @ hw(\"o/o\") with safe = 0
machine m:
    initial RUN
    state RUN:
        enter:
            o = 1
        loop:
            check time_in_state < 2 ms, \"weg\"
";
    let trace = simulate(body, "t=4 abort\nt=5 runtime Overrun\n", 7);
    assert_eq!(
        lines_of(&trace, 2),
        ["t=2 fault m CheckFailed \"weg\" -> FAULTED", "t=2 state m FAULTED", "t=2 out o 0"],
        "{trace}"
    );
    for t in 3..=7 {
        assert!(lines_of(&trace, t).is_empty(), "Tick {t}:\n{trace}");
    }
}

/// 5.4, zweiter Teil des Abort-Latch: Ein Operator-Abort im Fault-Ziel
/// eines Aborts ist wirkungslos; nach einer normalen Transition wirkt der
/// naechste wieder.
#[test]
fn the_abort_latch_releases_after_a_normal_transition() {
    let body = "\
output a : int in 0..9 @ hw(\"o/a\") with safe = 0
command stop
command back

machine one:
    fault -> SAFE
    initial RUN
    state RUN:
        enter:
            a = 1
        loop:
            if stop:
                abort \"operator\"
    state SAFE:
        enter:
            a = 2
        when back: -> RUN
";
    let trace = simulate(body, "t=1 cmd stop\nt=2 abort\nt=3 cmd back\nt=4 abort\n", 6);
    assert_eq!(
        lines_of(&trace, 1),
        ["t=1 fault one Abort \"operator\" -> SAFE", "t=1 state one SAFE", "t=1 out a 2"],
        "{trace}"
    );
    assert!(lines_of(&trace, 2).is_empty(), "der Latch haelt den Abort ab:\n{trace}");
    assert_eq!(lines_of(&trace, 3), ["t=3 state one RUN", "t=3 out a 1"], "{trace}");
    assert_eq!(
        lines_of(&trace, 4),
        ["t=4 fault one Abort \"operator abort\" -> SAFE", "t=4 state one SAFE", "t=4 out a 2"],
        "{trace}"
    );
    assert!(!trace.contains("FAULTED"), "{trace}");
}

/// 5.6: Eine erfuellte Auswertung setzt den Bestaetigungszaehler zurueck.
/// Zwei verletzte Ticks, ein erfuellter, dann drei verletzte: Der Fault
/// kommt erst nach den drei.
#[test]
fn a_fulfilled_evaluation_resets_the_confirmation_counter() {
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
    let stim = "t=0 in p 10 bar\nt=1 in p 80 bar\nt=2 in p 80 bar\nt=3 in p 10 bar\n\
                t=4 in p 80 bar\nt=5 in p 80 bar\nt=6 in p 80 bar\nt=7 in p 80 bar\n";
    let trace = simulate(body, stim, 8);
    let faults: Vec<&str> = trace.lines().filter(|l| l.contains(" fault ")).collect();
    assert_eq!(faults, ["t=6 fault m CheckFailed \"druck\" -> SAFE"], "{trace}");
}

#[test]
fn a_period_that_is_no_multiple_of_the_tick_acts_as_the_next_multiple() {
    // 7.2, Pruefung 14: `every 2500 us` bei 1 ms Tick warnt und laeuft als
    // `every 3 ms` — Aktivierungen in Tick 0, 3, 6.
    let trace = simulate(
        "output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m every 2500 us:
    var k : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            k = k + 1
            n = k
",
        "",
        7,
    );
    for line in ["t=0 out n 1\n", "t=3 out n 2\n", "t=6 out n 3\n"] {
        assert!(trace.contains(line), "`{line}` fehlt:\n{trace}");
    }
    for t in [1, 2, 4, 5] {
        assert!(!trace.contains(&format!("t={t} out n")), "Tick {t} ist keine Aktivierung:\n{trace}");
    }
}

/// 7.5: `pulse o = v for d` stellt nach `d` den Latch von *vor* dem
/// Statement wieder her — sonst endete der Puls nie —, und `cancel`
/// verwirft die geplante Abschaltung (`corpus-try/107_cancel_and_pulse.takt`).
#[test]
fn a_pulse_ends_and_cancel_drops_the_plan() {
    let src =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/107_cancel_and_pulse.takt"))
            .expect("Quelle");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let p = takt_sema::compile(&src, &options).program.expect("Programm");
    let trace =
        run(&p, &Trace::default(), &RunOptions { ticks: 20, ..Default::default() }).expect("Lauf").trace.render();
    let seen: Vec<&str> =
        trace.lines().filter(|l| l.contains(" out reset_n ") || l.contains(" out vbus_en ")).collect();
    assert_eq!(
        seen,
        [
            "t=0 out reset_n true",
            "t=0 out vbus_en true",
            "t=1 out reset_n false",
            "t=3 out reset_n true",
            "t=10 out reset_n false",
            "t=12 out reset_n true",
            "t=15 out vbus_en false",
            "t=18 out vbus_en true",
            "t=19 out reset_n false",
        ],
        "{trace}"
    );
}

/// 3.5: Eine Lieferung mit Qualitaet `stale` ist ungueltig, und `x.reason`
/// nennt den Grund `STALE` — wie `bad` ohne Grund `DRIVER` nennt. Das Alter
/// 250 ms liegt ueber `max_age`, der Messzeitpunkt (300 - 250 ms) faellt
/// nicht hinter den der Lieferung davor (12.6, Zeile 2).
#[test]
fn a_stale_delivery_reads_as_stale() {
    let body = "\
input  p     : float[bar] in 0..250 bar @ hw(\"d/p\") with max_age = 200 ms
output why   : Reason @ hw(\"o/why\") with safe = DRIVER
output valid : bool   @ hw(\"o/valid\") with safe = false

machine m every 100 ms:
    initial RUN
    state RUN:
        loop:
            valid = p.valid
            why = p.reason.or(OUT_OF_RANGE) if not p.valid else IMPLAUSIBLE
";
    let trace = simulate(body, "t=0 in p 10 bar\nt=300 in p stale age=250 ms\n", 300);
    assert!(trace.contains("t=0 out valid true"), "{trace}");
    assert!(trace.contains("t=300 out valid false"), "{trace}");
    assert!(trace.contains("t=300 out why STALE"), "{trace}");
}
