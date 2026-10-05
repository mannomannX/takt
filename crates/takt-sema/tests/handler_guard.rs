//! Handler-Guard `on s as e when g:` (8.7, FB-14): der Guard laeuft nach
//! dem Muster mit der Bindung; `false` reicht das Element an den naechsten
//! Handler weiter, und ohne Treffer gilt es als untersucht.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn trace(p: &Program, stimulus: &str, ticks: u64) -> String {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

const RX: &str =
    "input rx : stream<line<16>> @ hw(\"u/rx\") with max_rate = 100 Hz, framing = lines, overflow = fault\n";

#[test]
fn the_guard_selects_the_handler() {
    let p = compile(&format!(
        "{RX}
output a : int in 0..100 @ hw(\"o/a\") with safe = 0
output other : int in 0..100 @ hw(\"o/other\") with safe = 0
machine m:
    var na : int in 0..100 = 0
    var no : int in 0..100 = 0
    initial RUN
    state RUN:
        on rx as e when e.text == \"a\":
            na = na + 1
            a = na
        on rx as e:
            no = no + 1
            other = no
"
    ));
    let t = trace(&p, "t=1 in rx \"a\"\nt=2 in rx \"b\"\nt=3 in rx \"a\"\n", 4);
    assert!(t.contains("t=1 out a 1"), "{t}");
    assert!(t.contains("t=2 out other 1"), "{t}");
    assert!(t.contains("t=3 out a 2"), "{t}");
    assert!(!t.contains("out other 2"), "{t}");
}

#[test]
fn a_false_guard_still_consumes_the_element() {
    let p = compile(&format!(
        "{RX}
output a : int in 0..100 @ hw(\"o/a\") with safe = 0
output pending : int in 0..100 @ hw(\"o/pending\") with safe = 0
machine m:
    var na : int in 0..100 = 0
    initial RUN
    state RUN:
        loop:
            pending = rx.count
        on rx as e when e.text == \"a\":
            na = na + 1
            a = na
"
    ));
    let t = trace(&p, "t=1 in rx \"b\"\nt=2 in rx \"a\"\n", 4);
    // Das `b` ist im Tick 1 untersucht: im Tick 2 steht nur das `a` im Fenster.
    assert!(t.contains("t=1 out pending 1"), "{t}");
    assert!(!t.contains("out pending 2"), "{t}");
    assert!(t.contains("t=2 out a 1"), "{t}");
}

/// Muster, Captures und Guard: Der Guard liest die Captures des Musters;
/// `false` reicht das Element an den Auffanghandler weiter (8.7).
const CAPTURES: &str = "
output big : int in 0..100 @ hw(\"o/big\") with safe = 0
output rest : int in 0..100 @ hw(\"o/rest\") with safe = 0
machine m:
    fault -> SAFE
    var nb : int in 0..100 = 0
    var nr : int in 0..100 = 0
    initial RUN
    state RUN:
        on rx matches \"v={n:int}\" as e when GUARD:
            nb = nb + 1
            big = nb
        on rx as e:
            nr = nr + 1
            rest = nr
    state SAFE:
        loop:
            big = 99
";

#[test]
fn a_guard_reads_the_captures_of_its_pattern() {
    let p = compile(&format!("{RX}{}", CAPTURES.replace("GUARD", "e.n > 5")));
    let t = trace(&p, "t=1 in rx \"v=3\"\nt=2 in rx \"v=7\"\nt=3 in rx \"x\"\n", 4);
    assert!(t.contains("t=1 out rest 1\n"), "3 ist nicht groesser 5: {t}");
    assert!(t.contains("t=2 out big 1\n"), "7 schon: {t}");
    assert!(t.contains("t=3 out rest 2\n"), "kein Muster, Auffanghandler: {t}");
}

#[test]
fn a_guard_that_faults_takes_the_fault_path() {
    // Der Guard ist ein Ausdruck wie jeder andere: Seine implizite Pruefung
    // faultet die Maschine (4.1, 5.3).
    let p = compile(&format!("{RX}{}", CAPTURES.replace("GUARD", "100 / e.n > 5")));
    let t = trace(&p, "t=1 in rx \"v=0\"\n", 3);
    assert!(t.contains("t=1 fault m Arithmetic(DivZero)"), "{t}");
    assert!(t.contains("t=1 state m SAFE"), "{t}");
}
