//! `s.peek() -> E?` (8.6, FB-15): das naechste Element, untersucht, nicht
//! konsumiert — ein Handler desselben Ticks sieht es noch, im naechsten Tick
//! ist es fort.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

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
fn peek_shows_the_element_the_handler_still_gets() {
    let p = compile(
        "input rx : stream<line<16>> @ hw(\"u/rx\") with max_rate = 100 Hz, framing = lines, overflow = fault
output peeked : bool @ hw(\"o/peeked\") with safe = false
output seen : int in 0..100 @ hw(\"o/seen\") with safe = 0
machine m:
    var n : int in 0..100 = 0
    initial RUN
    state RUN:
        loop:
            var next : line<16>? = rx.peek()
            peeked = next.valid
        on rx as e:
            n = n + 1
            seen = n
",
    );
    let t = trace(&p, "t=1 in rx \"a\"\n", 3);
    assert!(t.contains("t=1 out peeked true"), "{t}");
    assert!(t.contains("t=1 out seen 1"), "{t}");
    assert!(t.contains("t=2 out peeked false"), "{t}");
}

#[test]
fn peek_alone_consumes_at_the_end_of_the_tick() {
    // Lemma 9.6.1: Wer nur peekt, laesst das Fenster nicht wachsen.
    let p = compile(
        "input rx : stream<line<16>> @ hw(\"u/rx\") with max_rate = 100 Hz, framing = lines, overflow = fault
output pending : int in 0..100 @ hw(\"o/pending\") with safe = 0
machine m:
    var next : line<16>? = none
    initial RUN
    state RUN:
        loop:
            next = rx.peek()
            pending = rx.count
",
    );
    let t = trace(&p, "t=1 in rx \"a\"\nt=1 in rx \"b\"\n", 4);
    assert!(t.contains("t=1 out pending 2"), "{t}");
    assert!(t.contains("t=2 out pending 1"), "{t}");
    assert!(t.contains("t=3 out pending 0"), "{t}");
}
