//! Satz 9.4.1 mit gescopten Instanzen (Referenz 5.11, 9.4.1): Die
//! Aktivitaet einer Instanz haengt an der Konfiguration ihres Besitzers
//! zu Tick-Beginn; zusammen mit dem globalen Single-Writer ist die
//! Schrittordnung damit trace-irrelevant.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const PROGRAM: &str = r#"system:
    language = 1
    tick     = 1 ms

input  mode : int in 0..2 @ sim("i/mode")
output a    : bool        @ hw("o/a") with safe = false
output b    : bool        @ hw("o/b") with safe = false
output c    : bool        @ hw("o/c") with safe = false

machine blip(o: output bool) every 1 ms:
    initial LOW

    state LOW:
        enter:
            o = false

        after 3 ms: -> HIGH

    state HIGH:
        enter:
            o = true

        after 3 ms: -> LOW

machine ctrl:
    initial FIRST

    state FIRST:
        instance p = blip(o = a)
        instance q = blip(o = b)

        when mode == 1: -> SECOND

    state SECOND:
        instance r = blip(o = c)

        when mode == 0: -> FIRST
"#;

fn compile() -> Program {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(PROGRAM, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn stimulus() -> Trace {
    let mut lines = String::new();
    for t in 0..24 {
        let mode = u32::from((8..16).contains(&t));
        lines.push_str(&format!("t={t} in mode {mode}\n"));
    }
    Trace::parse(&lines).expect("Stimulus")
}

fn trace_with(p: &Program, seed: Option<u64>) -> String {
    let options = RunOptions { ticks: 24, order_seed: seed, ..Default::default() };
    run(p, &stimulus(), &options).expect("Lauf").trace.render()
}

/// Jede Schrittordnung liefert denselben Trace (Satz 9.4.1).
#[test]
fn any_step_order_of_scoped_instances_yields_the_same_trace() {
    let p = compile();
    let reference = trace_with(&p, None);
    for seed in 0..32 {
        assert_eq!(trace_with(&p, Some(seed)), reference, "Startwert {seed}");
    }
}

/// Der Trace zeigt den Lebenszyklus: Eintritt, Austritt, frischer
/// Wiedereintritt (5.11).
#[test]
fn a_scoped_instance_restarts_on_re_entry() {
    let p = compile();
    let trace = trace_with(&p, None);
    // Beim Verlassen von FIRST werden `p` und `q` verworfen: leerer Pfad.
    assert!(trace.contains("t=8 state ctrl SECOND"), "{trace}");
    assert!(trace.contains("t=8 state p ") && trace.contains("t=8 state q "), "{trace}");
    // `r` betritt im selben Tick sein `initial`.
    assert!(trace.contains("t=8 state r LOW"), "{trace}");
    // Nach dem Wiedereintritt beginnt `p` erneut bei LOW mit frischem
    // Timer: der naechste Wechsel liegt drei Ticks spaeter, nicht frueher.
    assert!(trace.contains("t=16 state p LOW"), "{trace}");
    assert!(trace.contains("t=20 state p HIGH"), "{trace}");
}

/// Ausserhalb ihres Scopes schreitet eine Instanz nicht.
#[test]
fn an_inactive_instance_does_not_step() {
    let p = compile();
    let trace = trace_with(&p, None);
    let between: Vec<&str> = trace
        .lines()
        .filter(|l| {
            let t: u64 = l
                .split_whitespace()
                .next()
                .and_then(|x| x.strip_prefix("t="))
                .and_then(|x| x.parse().ok())
                .unwrap_or(0);
            (9..16).contains(&t) && l.contains(" state p ")
        })
        .collect();
    assert!(between.is_empty(), "`p` schritt ausserhalb seines Scopes: {between:?}");
}
