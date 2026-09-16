//! Standardbibliothek (Referenz 11.4): die Bloecke laufen im Golden-Trace
//! `corpus-try/sim/11_4`; hier steht, was der Trace nicht zeigt — der
//! Sichtbereich der Vorlagen (2.5) und ein `writer` nach `reset()` (5.7).

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
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

fn trace(p: &Program, stimulus: &str, ticks: u64) -> String {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

#[test]
fn a_user_name_does_not_collide_with_library_parameters() {
    // `hysteresis.step(x: …)` nennt seinen Parameter `x`; der Input `x`
    // des Nutzers liegt in einem anderen Bereich, die Vorlage sieht nur
    // das Prelude.
    let p = compile(
        "input x : float[V] in 0..10 V @ hw(\"a/x\") with max_age = 10 ms
output above : bool @ hw(\"o/above\") with safe = false
machine m:
    var h = hysteresis[V](lo = 2 V, hi = 4 V)
    initial RUN
    state RUN:
        loop:
            above = h.step(x)
",
    );
    let t = trace(&p, "t=0 in x 5.0 V\n", 2);
    assert!(t.contains("t=0 out above true"), "{t}");
}

#[test]
fn the_library_uses_its_own_helpers() {
    // Ein `clamp` des Nutzers verdeckt das der Bibliothek (Warnung 2.5);
    // `pid` rechnet trotzdem mit seinem eigenen.
    let p = compile(
        "fn clamp[U](x: float[U], lo: float[U], hi: float[U]) -> float[U]:
    return lo

output u : float[pct] @ hw(\"o/u\") with safe = 0 pct
machine m:
    var ctrl = pid[pct, K](kp = 2 pct/K, ki = 0 pct/K/s, kd = 0 pct*s/K, lo = 0 pct, hi = 100 pct)
    initial RUN
    state RUN:
        loop:
            u = ctrl.step(1 K, 1 ms)
",
    );
    let t = trace(&p, "", 1);
    assert!(t.contains("t=0 out u 2.0 pct"), "{t}");
}

#[test]
fn a_writer_starts_over_after_reset() {
    let p = compile(
        "output n : int in 0..8 @ hw(\"o/n\") with safe = 0
machine m:
    var w     = writer[8]()
    var ok    : bool = false
    var frame : bytes<8> = default
    initial RUN
    state RUN:
        enter:
            ok = w.u16_le(0xBEEF)
            w.reset()
            ok = w.u8(7)
            frame = w.data()
            n = frame.len
",
    );
    let t = trace(&p, "", 1);
    assert!(t.contains("t=0 out n 1"), "{t}");
}
