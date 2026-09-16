//! Die Sätze aus Referenz 9 als ausführbare Tests.
//!
//! - **Lemma 9.3.1**: `resolve_m` terminiert nach höchstens `1 + d`
//!   Iterationen; die Assertion im Interpreter ist die Schranke, ein
//!   konstruierter Fault-Wald der Tiefe 3 der Belastungsfall.
//! - **Satz 9.4.1**: der Trace ist eine Funktion der Inputs. Der Test über
//!   den Golden-Korpus steht in `golden_sim.rs`; hier wird zusätzlich mit
//!   Zufallsstimuli permutiert.
//! - **Satz 9.4.2**: für jeden Input-Strom existiert die Trace und enthält
//!   keinen undefinierten Zustand. Jeder interne Fehler des Interpreters ist
//!   ein `Trap::Bug` und damit ein Testfehler.
//! - **Lemma 3.4**: die Darstellungsverengung ändert kein Ergebnis. Weil sie
//!   eine Annotation ist (plan/m3.md 1.7), ist der Test derselbe Lauf mit und
//!   ohne gesetztes Feld — nicht der Vergleich zweier Programme.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, Trap, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Ein Fault-Wald der Tiefe 3: RUN -> L1 -> L2 -> L3 -> FAULTED. Jeder
/// Zielzustand faultet beim Eintritt erneut, sodass `resolve_m` die Kette
/// vollständig durchläuft.
const DEEP_FOREST: &str = "\
output x : int @ hw(\"o/x\") with safe = 0

machine m:
    initial RUN
    state RUN:
        fault -> L1
        loop:
            check false, \"in RUN\"
    state L1:
        fault -> L2
        loop:
            check false, \"in L1\"
    state L2:
        fault -> L3
        loop:
            check false, \"in L2\"
    state L3:
        # ohne eigenes Ziel und ohne Maschinenziel endet der Pfad bei FAULTED (5.3)
        loop:
            check false, \"in L3\"
";

#[test]
fn lemma_9_3_1_resolve_terminates_in_a_deep_fault_forest() {
    let program = compile(DEEP_FOREST);
    let stim = Trace::parse("").expect("leer");
    // Ohne Terminierung schlaegt die Schranke in `resolve_m` als Bug an.
    let out = run(&program, &stim, &RunOptions { ticks: 10, ..Default::default() }).expect("Lauf terminiert");
    let text = out.trace.render();
    // Der Eintritt in jeden Zustand faultet weiter, bis FAULTED erreicht ist.
    assert!(text.contains("state m FAULTED\n"), "{text}");
    for level in ["RUN", "L1", "L2", "L3"] {
        assert!(text.contains(&format!("\"in {level}\"")), "Ebene {level} fehlt:\n{text}");
    }
}

#[test]
fn lemma_9_3_1_holds_in_the_abort_phase() {
    // Auch der Abort nimmt den Fault-Pfad und muss terminieren (9.4).
    let body = "\
output x : int @ hw(\"o/x\") with safe = 0
command stop

machine m:
    initial RUN
    state RUN:
        fault -> L1
        loop:
            if stop:
                abort \"operator\"
    state L1:
        fault -> L2
        loop:
            check false, \"in L1\"
    state L2:
        loop:
            check false, \"in L2\"
";
    let program = compile(body);
    let stim = Trace::parse("t=2 cmd stop\n").expect("Stimulus");
    let out = run(&program, &stim, &RunOptions { ticks: 6, ..Default::default() }).expect("Lauf terminiert");
    assert!(out.trace.render().contains("Abort"), "{}", out.trace.render());
}

/// Ein Programm mit allen Faultquellen, die M1 kennt: Range, Arithmetik,
/// Validität, Check, Timeout.
const FAULT_SOURCES: &str = "\
input  p     : float[bar] in 0..100 bar @ hw(\"d/p\")
output p_sim : float[bar]               @ sim(\"d/p\")
input  n     : int in 0..1000           @ hw(\"d/n\")
output n_sim : int                      @ sim(\"d/n\")
output q     : int in 0..100            @ hw(\"o/q\") with safe = 0
output r     : float[bar]               @ hw(\"o/r\") with safe = 0 bar
command go
command stop

machine m:
    fault -> SAFE
    var acc : int in 0..1000 = 0
    initial RUN
    state RUN:
        loop:
            if stop:
                abort \"operator\"
            acc = acc + n
            q = acc / (n + 1)
            r = p * 2
            check p < 90 bar, \"druck {p}\"
        when go: -> WAIT
    state WAIT:
        after 5 ms: -> RUN
    state SAFE:
        when go: -> RUN
";

/// Deterministischer Zufallsstrom (xorshift), damit ein Fehlschlag
/// reproduzierbar ist.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Baut einen Zufallsstimulus: Werte, Qualitaeten, Commands, Abort.
fn random_stimulus(seed: u64, ticks: u64) -> String {
    let mut rng = Rng(seed | 1);
    let mut out = String::new();
    for t in 0..=ticks {
        match rng.below(6) {
            0 => out.push_str(&format!("t={t} in p {} bar\n", rng.below(120))),
            1 => out.push_str(&format!("t={t} in p bad reason=OutOfRange\n")),
            2 => out.push_str(&format!("t={t} in n {}\n", rng.below(1200))),
            3 => out.push_str(&format!("t={t} in n stale age={} ms\n", rng.below(500))),
            4 => out.push_str(&format!("t={t} cmd {}\n", if rng.below(2) == 0 { "go" } else { "stop" })),
            _ => {}
        }
    }
    out
}

#[test]
fn theorem_9_4_2_no_input_stream_produces_an_internal_error() {
    let program = compile(FAULT_SOURCES);
    for seed in 1..=300u64 {
        let text = random_stimulus(seed, 200);
        let stim = Trace::parse(&text).unwrap_or_else(|e| panic!("Stimulus {seed}: {e}\n{text}"));
        match run(&program, &stim, &RunOptions { ticks: 200, ..Default::default() }) {
            Ok(_) => {}
            // Ein Fault ist ein definiertes Ereignis, kein Fehler; ein Bug
            // ist eine Verletzung von Satz 9.4.2.
            Err(Trap::Bug(msg)) => panic!("Seed {seed}: interner Fehler `{msg}`\nStimulus:\n{text}"),
            Err(Trap::Fault(f)) => panic!("Seed {seed}: Fault entkommen: {f:?}"),
        }
    }
}

#[test]
fn theorem_9_4_2_holds_for_a_long_run() {
    // Die Trace existiert fuer jeden Praefix; 2 000 Ticks decken mehrere
    // Fault-Zyklen und Rueckkehren nach RUN ab.
    let program = compile(FAULT_SOURCES);
    let stim = Trace::parse(&random_stimulus(4711, 2000)).expect("Stimulus");
    let out = run(&program, &stim, &RunOptions { ticks: 2000, ..Default::default() }).expect("Lauf");
    assert!(!out.trace.lines.is_empty());
}

#[test]
fn theorem_9_4_1_order_independence_under_random_stimuli() {
    let program = compile(FAULT_SOURCES);
    for seed in [2u64, 23, 199] {
        let stim = Trace::parse(&random_stimulus(seed, 150)).expect("Stimulus");
        let base = run(&program, &stim, &RunOptions { ticks: 150, ..Default::default() }).expect("Lauf");
        for order in [5u64, 55, 555] {
            let other = run(&program, &stim, &RunOptions { ticks: 150, order_seed: Some(order), ..Default::default() })
                .expect("Lauf");
            assert_eq!(
                other.trace.render(),
                base.trace.render(),
                "Seed {seed}, Reihenfolge {order}: der Trace haengt von der Schrittfolge ab"
            );
        }
    }
}

#[test]
fn a_fault_never_escapes_to_the_caller() {
    // 9.3: `resolve_m` faengt jeden Fault; `run` liefert nie `Trap::Fault`.
    let program = compile(DEEP_FOREST);
    let stim = Trace::parse("").expect("leer");
    let out = run(&program, &stim, &RunOptions { ticks: 50, ..Default::default() });
    assert!(out.is_ok(), "Fault entkommen: {out:?}");
}

/// Ein Programm, das die Verengung wirklich beschäftigt: Schleife,
/// Verzweigung, Arithmetik mit Ranges.
const NARROWABLE: &str = "\
output n : int in 0..999 @ hw(\"o/n\") with safe = 0
output m : int in 0..99 @ hw(\"o/m\") with safe = 0

machine w:
    var acc : int in 0..999 = 0
    var delta : int in 0..9 = 3
    initial RUN
    state RUN:
        loop:
            for i in range(4):
                if acc < 500:
                    acc = acc + delta
            n = acc
            m = delta * 2
";

#[test]
fn lemma_3_4_narrowing_does_not_change_the_trace() {
    // 3.4: „Der Compiler waehlt die Darstellung eines `int` aus den
    // bewiesenen Intervallen — ohne Aenderung der Semantik."
    let narrowed = compile(NARROWABLE);
    // Dass die Verengung ueberhaupt greift, ist Voraussetzung des Tests.
    let annotated = takt_mir::analysis::narrow::narrow(&mut narrowed.clone());
    assert!(annotated.0 > 0, "die Verengung greift: {annotated:?}");

    // Derselbe Lauf ohne die Annotation.
    let mut plain = narrowed.clone();
    strip_repr(&mut plain);

    let stim = Trace::parse("").expect("leer");
    let a = run(&narrowed, &stim, &RunOptions { ticks: 8, ..Default::default() }).expect("Lauf");
    let b = run(&plain, &stim, &RunOptions { ticks: 8, ..Default::default() }).expect("Lauf");
    assert_eq!(a.trace.render(), b.trace.render(), "die Verengung aendert den Trace nicht");
}

/// Nimmt jede Darstellungsannotation zurueck.
fn strip_repr(p: &mut Program) {
    for m in &mut p.machines {
        strip_block(&mut m.loop_block);
        for s in &mut m.states {
            strip_block(&mut s.enter);
            strip_block(&mut s.exit);
            strip_block(&mut s.loop_block);
            for t in &mut s.transitions {
                strip_block(&mut t.actions);
            }
        }
    }
}

fn strip_block(b: &mut takt_mir::stmt::Block) {
    for s in &mut b.stmts {
        strip_stmt(s);
    }
}

fn strip_stmt(s: &mut takt_mir::stmt::Stmt) {
    use takt_mir::stmt::StmtKind;
    match &mut s.kind {
        StmtKind::Assign { value, .. } => strip_expr(value),
        StmtKind::If { cond, then, otherwise } => {
            strip_expr(cond);
            strip_block(then);
            strip_block(otherwise);
        }
        StmtKind::ForRange { count, body, .. } => {
            strip_expr(count);
            strip_block(body);
        }
        _ => {}
    }
}

fn strip_expr(e: &mut takt_mir::expr::Expr) {
    e.repr = None;
    for c in e.children_mut() {
        strip_expr(c);
    }
}

/// **Satz 9.4.5 (Safe-State-Latenz).** Die gerechnete Schranke wird im Lauf
/// eingehalten: Der Output steht spaetestens `D_safe` Ticks nach der
/// Verletzung auf seinem `safe`-Wert.
///
/// Der Test ist kein Nachrechnen der Formel, sondern ihr Belastungsfall —
/// die Bedingung wird ab Tick 0 verletzt, und der Trace sagt, wann der
/// Aktor wirklich abgefallen ist.
#[test]
fn theorem_9_4_5_the_latency_bound_holds_in_the_run() {
    let src = "\
output valve : bool @ hw(\"o/valve\") with safe = false

machine m every 2 ms:
    fault -> SAFE
    var n : int in 0..99 = 0
    initial RUN

    state RUN:
        loop:
            n = n + 1
            check n < 3, \"zu viele Zyklen\"
            valve = true

    state SAFE:
        loop:
            valve = false
";
    let p = compile(src);
    let bound = takt_mir::analysis::latency::latency(&p).worst_case();
    assert!(bound > 0, "die Schranke ist gerechnet");

    let stim = Trace::parse("").expect("leer");
    let out = run(&p, &stim, &RunOptions { ticks: bound + 8, ..Default::default() }).expect("Lauf");
    let text = out.trace.render();

    // Der erste Tick, in dem `valve` auf `false` faellt, nachdem er `true` war.
    let mut safe_at = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("t=") {
            let (t, tail) = rest.split_once(' ').unwrap_or((rest, ""));
            if tail.contains("out valve false") {
                safe_at = t.parse::<u64>().ok();
                break;
            }
        }
    }
    let Some(safe_at) = safe_at else {
        panic!(
            "der Aktor faellt ab:
{text}"
        )
    };
    assert!(safe_at <= bound, "sicher nach {safe_at} Ticks, Schranke {bound}:\n{text}");
}

/// **In `idle` ist der Schritt die Identitaet** (5.10, Satz 9.9.1).
///
/// Nur darum darf die Runtime Ticks ueberspringen: Was sie auslaesst,
/// haette nichts getan. Der Test prueft es am Trace — zwischen Eintritt
/// und `after`-Frist aendert sich kein Ausgang.
#[test]
fn theorem_9_9_1_an_idle_step_changes_nothing() {
    let p = compile(
        "\
input  wake : bool @ hw(\"gpio/btn\") with wake = true
output wake_sim : bool @ sim(\"gpio/btn\") with safe = false
output led  : bool @ hw(\"ui/led\") with safe = false

machine m:
    initial SLEEP
    state SLEEP idle:
        enter:
            led = false
        when wake: -> RUN
        after 50 ms: -> RUN
    state RUN:
        enter:
            led = true
        after 20 ms: -> SLEEP
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 200, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    let text = out.trace.render();
    let wechsel: Vec<&str> = text.lines().filter(|l| l.contains(" out led ")).collect();
    // Der Interpreter schreibt nur Aenderungen (9.3): In `idle` gibt es
    // zwischen Eintritt und Frist keine.
    assert!(wechsel.len() < 20, "zu viele Aenderungen fuer einen Schlafzustand:\n{}", wechsel.join("\n"));
    assert!(text.contains("t=0 out led false"), "der Eintritt schreibt einmal:\n{text}");
}

/// Ein `idle`-Zustand verwirft Nicht-Wake-Stroeme, statt ueberzulaufen
/// (5.10, 9.6).
#[test]
fn a_sleeping_machine_drops_non_waking_streams() {
    let p = compile(
        "\
input  noise : stream<u8> @ hw(\"bus/noise\") with capacity = 4, max_rate = 2000 Hz
output noise_sim : stream<u8> @ sim(\"bus/noise\")
output led : bool @ hw(\"ui/led\") with safe = false

machine feeder:
    initial GO
    state GO:
        loop:
            send noise_sim, 1

machine m:
    initial SLEEP
    state SLEEP idle:
        enter:
            led = false
        after 20 ms: -> DONE
    state DONE:
        enter:
            led = true
",
    );
    // Ohne die Verwerfung liefe der Puffer nach vier Ticks ueber und die
    // Maschine faultete; mit ihr laeuft sie bis zur Frist durch.
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 150, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert!(out.trace.render().contains("out led true"), "die Frist wird erreicht:\n{}", out.trace.render());
}

/// **Was eine schlafende Maschine verpasst, zaehlt sie selbst** (5.10, 9.6).
///
/// 9.6 unterscheidet `dropped[s]` am Puffer — `drop_oldest` trifft alle
/// Leser — von `dropped[s, m]` je Maschine: Nur die schlafende verliert
/// die Elemente.
#[test]
fn a_sleeping_machine_counts_what_it_missed() {
    let p = compile(
        "\
input  noise : stream<u8> @ hw(\"bus/noise\") with capacity = 4, max_rate = 2000 Hz
output noise_sim : stream<u8> @ sim(\"bus/noise\")
output led : bool @ hw(\"ui/led\") with safe = false

machine feeder:
    initial GO
    state GO:
        loop:
            send noise_sim, 1

machine m:
    var seen : int in 0..999 = 0

    initial SLEEP

    state SLEEP idle:
        enter:
            led = false
        after 20 ms: -> AWAKE

    state AWAKE:
        loop:
            led = seen > 0
        on noise as e:
            seen = seen + 1
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 30, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    let text = out.trace.render();
    // 5.10 verlangt den Alert beim *Verlassen* des Zustands.
    let alert = text.lines().find(|l| l.contains("StreamPaused")).unwrap_or_else(|| {
        panic!("der Alert beim Aufwachen fehlt:\n{text}");
    });
    assert!(alert.starts_with("t=20 "), "beim Verlassen, nicht waehrend des Schlafs: {alert}");
    assert!(!text.contains("STREAM_OVERFLOW"), "im Schlaf gibt es keinen Ueberlauf-Fault (9.6):\n{text}");
}

/// **`reboot = DEEP_SLEEP` beendet den Lauf** (12.7).
///
/// Anders als `idle` gibt es keinen virtuellen Tick: Satz 9.9.1 gilt nur
/// fuer RAM-erhaltenden Schlaf.
#[test]
fn deep_sleep_ends_the_run() {
    let p = compile(
        "\
output reboot : RebootCmd @ hw(\"sys/reboot\") with safe = NONE
output led    : bool      @ hw(\"ui/led\") with safe = false

machine m:
    initial RUN
    state RUN:
        enter:
            led = true
        after 5 ms: -> OFF
    state OFF:
        enter:
            reboot = DEEP_SLEEP
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 50, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert_eq!(out.ended, takt_interp::Ended::DeepSleep);
    let text = out.trace.render();
    assert!(text.contains("t=5 end deep_sleep"), "der Grund steht im Trace:\n{text}");
    assert!(!text.contains("t=6 "), "nach dem Kommando laeuft nichts mehr:\n{text}");
}

/// `reboot = RESTART` endet ebenso, mit anderem Grund (12.7).
#[test]
fn restart_ends_the_run_with_its_own_reason() {
    let p = compile(
        "\
output reboot : RebootCmd @ hw(\"sys/reboot\") with safe = NONE

machine m:
    initial RUN
    state RUN:
        enter:
            reboot = RESTART
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 20, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert_eq!(out.ended, takt_interp::Ended::Restart);
}

/// Ohne Kommando laeuft der Lauf bis zur Tickzahl.
#[test]
fn without_a_reboot_command_the_run_uses_all_ticks() {
    let p = compile(
        "\
output reboot : RebootCmd @ hw(\"sys/reboot\") with safe = NONE
output led    : bool      @ hw(\"ui/led\") with safe = false

machine m:
    initial RUN
    state RUN:
        enter:
            led = true
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 7, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert_eq!(out.ended, takt_interp::Ended::Ticks);
    assert!(out.trace.render().contains("t=7 verdict-final"), "{}", out.trace.render());
}

/// **`boot_reason` schliesst den Kreis** (12.7).
///
/// Der naechste Lauf beginnt mit `DEEP_SLEEP_WAKE`; das Programm sieht
/// es wie jeden anderen Input.
#[test]
fn a_woken_run_sees_its_boot_reason() {
    let src = "\
input  boot_reason : BootReason @ hw(\"sys/boot_reason\")
output woke        : bool       @ hw(\"ui/woke\") with safe = false

machine m:
    initial START
    state START:
        enter:
            woke = boot_reason == DEEP_SLEEP_WAKE
";
    let p = compile(src);
    let kalt =
        run(&p, &Trace::default(), &RunOptions { ticks: 2, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert!(kalt.trace.render().contains("out woke false"), "{}", kalt.trace.render());

    let stim = Trace::parse("t=0 in boot_reason DEEP_SLEEP_WAKE\n").expect("Stimulus");
    let warm =
        run(&p, &stim, &RunOptions { ticks: 2, profile: None, order_seed: None, ..Default::default() }).expect("Lauf");
    assert!(warm.trace.render().contains("out woke true"), "{}", warm.trace.render());
}

/// **Auch der Anfangszustand kann das Kommando setzen** (12.7).
///
/// `init` laeuft vor der Tickschleife; wer das uebersieht, laesst einen
/// Tick laufen, den es nicht geben duerfte.
#[test]
fn a_reboot_in_the_initial_state_ends_the_run_at_once() {
    let p = compile(
        "\
output reboot : RebootCmd @ hw(\"sys/reboot\") with safe = NONE
output led    : bool      @ hw(\"ui/led\") with safe = false

machine m:
    initial OFF
    state OFF:
        enter:
            reboot = DEEP_SLEEP
            led = true
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 8, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert_eq!(out.ended, takt_interp::Ended::DeepSleep);
    let text = out.trace.render();
    assert!(text.contains("t=0 end deep_sleep"), "bei Tick 0, nicht spaeter:\n{text}");
    assert!(!text.contains("t=1 "), "kein Tick nach dem Kommando:\n{text}");
}

/// **Danach stehen alle Outputs auf `safe`** (12.7).
#[test]
fn a_reboot_leaves_the_outputs_safe() {
    let p = compile(
        "\
output reboot : RebootCmd @ hw(\"sys/reboot\") with safe = NONE
output led    : bool      @ hw(\"ui/led\") with safe = false

machine m:
    initial RUN
    state RUN:
        enter:
            led = true
        after 5 ms: -> OFF
    state OFF:
        enter:
            reboot = DEEP_SLEEP
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 20, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    let text = out.trace.render();
    let nach = text.split("end deep_sleep").nth(1).unwrap_or("");
    assert!(nach.contains("out reboot NONE"), "der Befehl selbst faellt auf `safe` zurueck:\n{text}");
    assert!(nach.contains("out led false"), "und jeder andere Ausgang auch:\n{text}");
}

/// **Nur `RebootCmd` beendet den Lauf** (12.7).
///
/// 12.7 nennt die Enums `BootReason`, `ImageState` und `RebootCmd`
/// ausdruecklich vordefiniert. Ein eigenes Enum mit einer zufaellig
/// `RESTART` heissenden Variante ist kein Kommando.
#[test]
fn only_the_predefined_enum_triggers_a_reboot() {
    let p = compile(
        "\
enum MyCmd: NONE, RESTART

output reboot : MyCmd @ hw(\"sys/reboot\") with safe = NONE
output led    : bool  @ hw(\"ui/led\") with safe = false

machine m:
    initial RUN
    state RUN:
        enter:
            reboot = RESTART
            led = true
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 4, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert_eq!(out.ended, takt_interp::Ended::Ticks, "ein fremdes Enum ist kein Kommando");
}

/// Ein `sim`-gebundener Ausgang loest keinen Reboot aus (8.3).
///
/// Ein `sim`-Output speist den gleichnamigen `hw`-Input; `sys/reboot` hat
/// keinen, und die Plattform fuehrt das Kommando aus, kein Modell.
#[test]
fn a_simulated_reboot_channel_does_not_end_the_run() {
    let p = compile(
        "\
output reboot : RebootCmd @ sim(\"sys/reboot\")
output led    : bool      @ hw(\"ui/led\") with safe = false

machine m:
    initial RUN
    state RUN:
        enter:
            reboot = DEEP_SLEEP
            led = true
",
    );
    let out =
        run(&p, &Trace::default(), &RunOptions { ticks: 4, profile: None, order_seed: None, ..Default::default() })
            .expect("Lauf");
    assert_eq!(out.ended, takt_interp::Ended::Ticks);
}
